#!/usr/bin/env python3
"""Black-box Unix baseline. Synthetic history only; no third-party Python packages."""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import pty
import pwd
import select
import shlex
import shutil
import signal
import sqlite3
import statistics
import subprocess
import sys
import tempfile
import time


def positive(text):
    value = int(text)
    if value < 1:
        raise argparse.ArgumentTypeError("must be positive")
    return value


def summary(samples):
    ordered = sorted(samples)
    return {"median": statistics.median(ordered),
            "p95": ordered[math.ceil(len(ordered) * 0.95) - 1],
            "samples": samples}


def rss_bytes(usage):
    # wait4's units differ between Darwin and Linux.
    return usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024)


@contextmanager
def deadline(seconds=60):
    def expired(*_):
        raise TimeoutError(f"benchmark operation exceeded {seconds}s")
    previous = signal.signal(signal.SIGALRM, expired)
    signal.setitimer(signal.ITIMER_REAL, seconds)
    try:
        yield
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous)


def measure(argv, env):
    start = time.perf_counter_ns()
    with tempfile.TemporaryFile() as error:
        process = subprocess.Popen(argv, env=env, stdout=subprocess.DEVNULL, stderr=error)
        try:
            with deadline():
                _, status, usage = os.wait4(process.pid, 0)
        except BaseException:
            process.kill()
            process.wait()
            raise
        process.returncode = os.waitstatus_to_exitcode(status)
        elapsed = (time.perf_counter_ns() - start) / 1e6
        if process.returncode:
            error.seek(0)
            raise RuntimeError(f"{argv}: {error.read().decode(errors='replace')}")
    return elapsed, rss_bytes(usage)


def repeated(argv, env, samples):
    print(f"  {' '.join(argv[1:3])}", file=sys.stderr, flush=True)
    # First invocation is separate; "warm" still starts a new CLI process.
    first_ms, first_rss = measure(argv, env)
    warm = [measure(argv, env) for _ in range(samples)]
    return {"first_ms": first_ms, "first_peak_rss_bytes": first_rss,
            "warm_ms": summary([x[0] for x in warm]),
            "warm_peak_rss_bytes": summary([x[1] for x in warm])}


class Terminal:
    def __init__(self, argv, env, directory):
        self.start = time.perf_counter_ns()
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(directory)
            os.execve(argv[0], argv, env)
        self.reaped = False

    def until(self, marker):
        data = b""
        end = time.monotonic() + 60
        while time.monotonic() < end:
            if select.select([self.fd], [], [], 0.1)[0]:
                try:
                    part = os.read(self.fd, 65536)
                except OSError as error:
                    raise RuntimeError(f"terminal closed: {data!r}") from error
                if not part:
                    break
                data += part
                if marker in data:
                    return data
        raise TimeoutError(f"terminal did not reach {marker!r}: {data!r}")

    def send(self, command, marker):
        start = time.perf_counter_ns()
        os.write(self.fd, command.encode() + b"\n")
        self.until(marker)
        return (time.perf_counter_ns() - start) / 1e6

    def wait(self, expected):
        end = time.monotonic() + 60
        while True:
            pid, status, usage = os.wait4(self.pid, os.WNOHANG)
            if pid:
                break
            if time.monotonic() > end:
                raise TimeoutError("terminal did not exit")
            # Restoring termios with TCSAFLUSH can wait for output to drain.
            # Keep consuming the PTY while waiting, just like a real terminal.
            if select.select([self.fd], [], [], 0.001)[0]:
                try:
                    os.read(self.fd, 65536)
                except OSError:
                    pass
        self.reaped = True
        if os.waitstatus_to_exitcode(status) != expected:
            raise RuntimeError(f"unexpected terminal exit: {status}")
        return rss_bytes(usage)

    def close(self):
        if not self.reaped:
            try:
                os.kill(self.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            os.waitpid(self.pid, 0)
        os.close(self.fd)


def picker(binary, env, directory):
    terminal = Terminal([binary, "pick"], env, directory)
    try:
        terminal.until(b"cancel")
        elapsed = (time.perf_counter_ns() - terminal.start) / 1e6
        # Ctrl-C cancels immediately; Escape deliberately waits for an arrow sequence.
        os.write(terminal.fd, b"\x03")
        return elapsed, terminal.wait(expected=1)
    finally:
        terminal.close()


def hooks(binary, shell, mode, env, directory, samples):
    name = Path(shell).name
    print(f"  hooks {name} {mode}", file=sys.stderr, flush=True)
    options = ["--noprofile", "--norc", "-i"] if name == "bash" else ["-d", "-f", "-i"]
    env = dict(env, HISTFILE="", SMCLI_NO_KEYS="1")
    env.pop("SMCLI_SYNC", None)
    if mode == "sync":
        env["SMCLI_SYNC"] = "1"
    terminal = Terminal([shell] + options, env, directory)
    # Build the marker in the shell, so terminal echo cannot masquerade as a prompt.
    marker = b"__SMCLI_BENCH_READY__ "
    command = f": benchmark-{name}-{mode}"
    try:
        history = "set -o history" if name == "bash" else "setopt"
        terminal.send("PS1='__SMCLI_BENCH_'\"READY__ \"; PS2=''; "
                      f"unset PROMPT_COMMAND; HISTSIZE=100000; {history}", marker)
        if mode != "off":
            terminal.send(f'eval "$({shlex.quote(binary)} shell-init {name})"', marker)
        for _ in range(3):
            terminal.send(command, marker)
        timings = [terminal.send(command, marker) for _ in range(samples)]
        if mode != "off":
            # Async prompt return is not a persistence acknowledgement. Drain writes
            # outside the timed section and fail instead of silently losing records.
            end = time.monotonic() + 60
            while True:
                with sqlite3.connect(env["SMCLI_DB"]) as db:
                    count = db.execute("SELECT count(*) FROM commands WHERE command = ?", (command,)).fetchone()[0]
                if count == samples + 3:
                    break
                if time.monotonic() > end:
                    raise TimeoutError(f"{name} {mode}: {count}/{samples + 3} records persisted")
                time.sleep(0.01)
        os.write(terminal.fd, b"exit\n")
        return {"roundtrip_ms": summary(timings), "shell_peak_rss_bytes": terminal.wait(expected=0)}
    finally:
        terminal.close()


def seed(binary, home, rows, service):
    directory = home / "projects" / "p0"
    directory.mkdir(parents=True)
    config = home / "config.json"
    config.write_text("{}\n")
    # Drop overrides that could read or contact the user's real history/model config.
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("SMCLI_", "XDG_", "OLLAMA_", "BASH_FUNC_"))
           and k not in ("HISTFILE", "BASH_ENV", "ENV", "ZDOTDIR")}
    env.update(HOME=str(home), HISTFILE="", SMCLI_DB=str(home / "history.db"),
               SMCLI_CONFIG=str(config), PWD=str(directory), TERM="dumb",
               XDG_DATA_HOME=str(home / "data"), XDG_CONFIG_HOME=str(home / "config"),
               SMCLI_SERVICE="1" if service else "0")
    if service:
        # Unix socket paths are short; the fixture's own directory is not.
        env["SMCLI_RUNTIME_DIR"] = tempfile.mkdtemp(prefix="smcli-bench.", dir="/tmp")
    distinct = min(rows, max(100, rows // 5), 5000)
    templates = ["git status --short # {n}", "git log --oneline -n {n}",
                 "npm run test -- src/test{n}.ts", "cargo test package{n}",
                 "swift test --filter Test{n}", "rg needle src/file{n}.swift",
                 "python3 scripts/task{n}.py", "ls 'directory {n}'",
                 "printf '東京 {n}\\n'", "git diff -- src/file{n}.swift"]
    now = 1_792_281_600  # Fixed 2026-10-18 epoch: fixture distribution is reproducible.
    legacy = home / "fixture.jsonl"
    with legacy.open("w") as output:
        for i in range(rows):
            n = i % distinct
            output.write(json.dumps({
                "timestamp": datetime.fromtimestamp(now - (rows - i) * 60, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
                "user": pwd.getpwuid(os.getuid()).pw_name, "shell": "zsh",
                "directory": str(home / "projects" / f"p{i % 20}"),
                "command": templates[n % len(templates)].format(n=n),
                "exitCode": 1 if i % 7 == 0 else 0,
            }, ensure_ascii=False) + "\n")
    subprocess.run([binary, "import", str(legacy)], env=env, check=True, stdout=subprocess.DEVNULL, timeout=120)
    with sqlite3.connect(env["SMCLI_DB"]) as db:
        actual = db.execute("SELECT count(*), count(DISTINCT command) FROM commands").fetchone()
        if actual != (rows, distinct):
            raise RuntimeError(f"invalid fixture: {actual}, expected {(rows, distinct)}")
    native_rows = max(1, rows // 10)
    (home / ".bash_history").write_text("".join(f"echo native-{i % distinct}\n" for i in range(native_rows)))
    return env, directory, distinct, native_rows


def repository_search(binary, files, samples):
    """File count -> directory context, fff files and grep latency through the service, plus its memory.

    Builds a Git repository of `files` tracked files (100 per directory) and 5 untracked ones. Cold = the first request
    of a fresh service; warm = the median of `samples` repeats. RSS is the service's resident size right after, not a peak.
    """
    import json as _json
    import socket
    with tempfile.TemporaryDirectory(prefix="smcli-repo-") as temp:
        home = Path(temp).resolve()
        repo = home / "repo"
        for d in range(files // 100):
            folder = repo / f"pkg{d // 50}" / f"dir{d}"
            folder.mkdir(parents=True)
            for f in range(100):
                (folder / f"file{f}.txt").write_text(f"line {d} {f}\n")
        (repo / "pkg0" / "dir0" / "unique_zebra_marker.swift").write_text("let marker = 1 // quokka_token_9137\n")
        git = lambda *a: subprocess.run(["git", "-C", str(repo), "-c", "user.name=t", "-c", "user.email=t@e.x", "-c", "gc.auto=0", *a],
                                        check=True, capture_output=True)
        git("init", "-q", "-b", "main")
        git("add", ".")
        git("commit", "-q", "-m", "fixture")
        for n in range(5):
            (repo / f"untracked{n}.txt").write_text("u")
        runtime = tempfile.mkdtemp(prefix="smcli-bench.", dir="/tmp")
        env = {k: v for k, v in os.environ.items() if not k.startswith(("SMCLI_", "XDG_"))}
        env.update(HOME=str(home), SMCLI_DB=str(home / "history.db"), SMCLI_CONFIG=str(home / "config.json"),
                   SMCLI_RUNTIME_DIR=runtime, SMCLI_SERVICE="1")
        (home / "config.json").write_text("{}\n")
        subprocess.run([binary, "service", "start"], env=env, check=True, capture_output=True, timeout=60)
        status = subprocess.run([binary, "service", "status"], env=env, check=True, capture_output=True, text=True).stdout
        path, pid = status.split(" at ")[1].split(" for ")[0], int(status.split("pid ")[1].split(")")[0])

        def ask(op, query=""):
            start = time.perf_counter()
            with socket.socket(socket.AF_UNIX) as connection:
                connection.settimeout(120)
                connection.connect(path)
                connection.sendall((_json.dumps({"v": 1, "op": op, "directory": str(repo / "pkg0" / "dir0"), "query": query}) + "\n").encode())
                reply = b""
                while not reply.endswith(b"\n"):
                    reply += connection.recv(1 << 20)
            return (time.perf_counter() - start) * 1000, _json.loads(reply)

        def rss():
            return int(subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True).stdout) / 1024
        try:
            result = {"files": files, "rss_start_mib": rss()}
            result["context_first_ms"] = ask("context")[0]
            begin = time.perf_counter()
            while "status" not in ask("context")[1]["context"] and time.perf_counter() - begin < 120:
                time.sleep(0.02)
            result["context_status_ready_ms"] = (time.perf_counter() - begin) * 1000 + result["context_first_ms"]
            result["context_warm_ms"] = summary([ask("context")[0] for _ in range(samples)])
            first, reply = ask("files", "unique_zebra_marker")
            assert reply["ok"] and reply["files"][0]["path"].endswith("unique_zebra_marker.swift"), reply
            result["files_first_ms"] = first
            result["files_warm_ms"] = summary([ask("files", "unique_zebra_marker")[0] for _ in range(samples)])
            result["grep_first_ms"] = ask("grep", "quokka_token_9137")[0]
            result["grep_warm_ms"] = summary([ask("grep", "quokka_token_9137")[0] for _ in range(samples)])
            result["rss_after_mib"] = rss()
            return result
        finally:
            subprocess.run([binary, "service", "stop"], env=env, capture_output=True, timeout=60)
            shutil.rmtree(runtime, ignore_errors=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--rows", type=positive, nargs="+", default=[1000, 10000, 100000])
    parser.add_argument("--samples", type=positive, default=20)
    parser.add_argument("--output", type=Path, default=Path("dist/baseline.json"))
    parser.add_argument("--service", choices=("on", "off"), default="off",
                        help="measure through smcli-service (on) or in-process (off, the default; smcli itself defaults to on)")
    parser.add_argument("--repo-files", type=positive, nargs="*", default=None,
                        help="tracked-file counts for the directory context / fff files / grep section "
                             "(needs the service; default 10000 50000 with --service on, none otherwise)")
    args = parser.parse_args()
    if args.repo_files is None:
        args.repo_files = [10000, 50000] if args.service == "on" and sys.platform == "darwin" else []
    binary = str(args.binary.resolve(strict=True))
    shells = [shutil.which(name) for name in ("bash", "zsh")]
    report = {
        "format": 1, "created_at": datetime.now(timezone.utc).isoformat(),
        "platform": platform.platform(), "machine": platform.machine(),
        "python": platform.python_version(),
        "logical_cpus": os.cpu_count(),
        "binary_sha256": hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
        "git_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
        "rustc": subprocess.check_output(["rustc", "--version"], stderr=subprocess.STDOUT, text=True).strip(),
        "samples": args.samples, "fixtures": [], "service": args.service,
        "notes": [
            f"Service mode: {args.service}. " + ("Every command goes through a smcli-service started for the fixture (record, suggest, search, picker)."
                                                 if args.service == "on" else "SMCLI_SERVICE=0: every command works in-process."),
            "Synthetic captured history: 20 directories, repeated commands, Unicode, 1/7 failures; native lines = rows/10.",
            "First = first invocation for this fixture, NOT a flushed filesystem/model cache.",
            "Warm = repeated one-shot processes with OS caches warm, NOT a persistent backend or in-process search.",
            "Hook latency = PTY command-to-prompt roundtrip; subtract same-shell off median to estimate overhead.",
            "Async hooks can return before persistence; all measured records are checked outside timed sections.",
            "wait4 high-water RSS is not summed process-tree or steady-state memory; shell results may include waited-for descendants.",
            "Picker = exec to first rendered footer, no Tab/semantic query; cancellation is excluded from latency.",
        ],
    }
    if sys.platform == "darwin":
        report["cpu"] = subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip()
        report["memory_bytes"] = int(subprocess.check_output(["sysctl", "-n", "hw.memsize"], text=True))
    for rows in args.rows:
        print(f"Measuring {rows} captured rows…", file=sys.stderr, flush=True)
        with tempfile.TemporaryDirectory(prefix="smcli-baseline-") as temp:
            home = Path(temp).resolve()
            env, directory, distinct, native_rows = seed(binary, home, rows, args.service == "on")
            fixture = {"rows": rows, "distinct_captured": distinct, "native_rows": native_rows}
            record = [binary, "record", "zsh", "benchmark-record", str(directory), "0", "", "", "", ": benchmark-record"]
            fixture["record"] = repeated(record, env, args.samples)
            fixture["suggest"] = repeated([binary, "suggest", "--json"], env, args.samples)
            fixture["suggest_prefix"] = repeated([binary, "suggest", "git", "--json"], env, args.samples)
            if sys.platform == "darwin":
                fixture["search_literal"] = repeated([binary, "search", "git status", "--json"], env, args.samples)
                fixture["search_semantic"] = repeated([binary, "search", "show changes in working tree", "--json"], env, args.samples)
            else:
                fixture["search_unavailable"] = "Current hybrid command search requires macOS."
            print("  picker first draw", file=sys.stderr, flush=True)
            first = picker(binary, env, directory)
            warm = [picker(binary, env, directory) for _ in range(args.samples)]
            fixture["picker"] = {"first_ms": first[0], "first_peak_rss_bytes": first[1],
                                 "warm_ms": summary([x[0] for x in warm]),
                                 "warm_peak_rss_bytes": summary([x[1] for x in warm])}
            fixture["hooks"] = {}
            for shell in shells:
                if not shell:
                    continue
                name = Path(shell).name
                modes = {mode: hooks(binary, shell, mode, env, directory, args.samples)
                         for mode in ("off", "async", "sync")}
                for mode in ("async", "sync"):
                    modes[mode]["median_overhead_ms"] = (
                        modes[mode]["roundtrip_ms"]["median"] - modes["off"]["roundtrip_ms"]["median"])
                fixture["hooks"][name] = {
                    "path": shell,
                    "version": subprocess.check_output([shell, "--version"], text=True).splitlines()[0],
                    "modes": modes}
            if args.service == "on":
                status = subprocess.run([binary, "service", "status"], env=env, capture_output=True, text=True).stdout
                if "pid " in status:
                    pid = status.split("pid ")[1].split(")")[0]
                    rss = subprocess.run(["ps", "-o", "rss=", "-p", pid], capture_output=True, text=True).stdout.strip()
                    # Resident size of the service after everything above (suggest, search, picker, hooks), not a peak.
                    fixture["service_rss_mib"] = int(rss) / 1024 if rss else None
                subprocess.run([binary, "service", "stop"], env=env, capture_output=True, timeout=60)
                shutil.rmtree(env["SMCLI_RUNTIME_DIR"], ignore_errors=True)
            fixture["database_bytes"] = Path(env["SMCLI_DB"]).stat().st_size
            report["fixtures"].append(fixture)
            # Persist completed fixtures, so an interrupted large run still leaves evidence.
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(report, indent=2) + "\n")
    if args.repo_files:
        report["repositories"] = []
        for files in args.repo_files:
            print(f"Measuring a {files}-file repository…", file=sys.stderr, flush=True)
            report["repositories"].append(repository_search(binary, files, args.samples))
            args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(f"Wrote {args.output}", file=sys.stderr)


if __name__ == "__main__":
    main()
