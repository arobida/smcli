#!/usr/bin/env python3
"""Exercise the generated hooks in real interactive shells, using only temporary files."""
import json
import os
from pathlib import Path
import pty
import re
import select
import shlex
import signal
import subprocess
import sys
import tempfile
import time

binary = str(Path(sys.argv[1]).resolve())
prompt = b"__SMCLI_TEST_PROMPT__ "


def read_records(env):
    return json.loads(subprocess.check_output([binary, "history", "--json"], env=env))


def run_shell(shell):
    with tempfile.TemporaryDirectory() as directory:
        directory = str(Path(directory).resolve())
        home = Path(directory)
        env = dict(os.environ, HOME=directory, HISTFILE=str(home / "native"),
                   SMCLI_DB=str(home / "captured.db"), SMCLI_SYNC="1", SHELL=f"/bin/{shell}", TERM="dumb")
        pid, fd = pty.fork()
        if pid == 0:
            os.chdir(directory)
            args = [f"/bin/{shell}"] + (["--noprofile", "--norc", "-i"] if shell == "bash" else ["-d", "-f", "-i"])
            os.execve(args[0], args, env)

        def until_prompt():
            data = b""
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                if select.select([fd], [], [], 0.1)[0]:
                    data += os.read(fd, 65536)
                    clean = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", data)
                    if clean.endswith(prompt):
                        return clean.decode(errors="replace")
            raise AssertionError(f"{shell}: prompt timeout: {data.decode(errors='replace')}")

        def send(command):
            os.write(fd, command.encode() + b"\n")
            return until_prompt()

        alive = True
        try:
            # Drain the initial prompt without assuming the user's shell prompt format.
            time.sleep(0.2)
            while select.select([fd], [], [], 0)[0]:
                os.read(fd, 65536)
            send("PS1='__SMCLI_TEST_PROMPT__ '; PS2=''; mkdir sub")
            if shell == "bash":
                send("PROMPT_COMMAND='printf \"%s\\n\" \"$?\" >> \"$HOME/prompt-status\"'")
            else:
                send("precmd() { printf '%s\\n' \"$?\" >> \"$HOME/prompt-status\"; }")
            output = send(f'eval "$({shlex.quote(binary)} shell-init {shell})"')
            assert "smcli:" not in output, output
            send("printf repeat")
            send("printf repeat")
            send("false")
            assert (home / "prompt-status").read_text().splitlines()[-1] == "1", "existing prompt lost exit status"
            send("cd sub")
            send("pwd")
            send("sleep 1.1")
            send("printf '%s\\n' 'hello world'")
            records = read_records(env)
            assert [r["command"] for r in records] == ["printf repeat", "printf repeat", "false", "cd sub", "pwd", "sleep 1.1", "printf '%s\\n' 'hello world'"], records
            assert [r["exitCode"] for r in records] == [0, 0, 1, 0, 0, 0, 0], records
            # Bash < 5 only has whole seconds, so the 1.1s sleep may round to 1000 or 2000ms.
            assert 1000 <= records[5]["durationMs"] < 3000, records[5]
            assert len({r["session"] for r in records}) == 1 and all(r["host"] for r in records), records
            assert [r["runCount"] for r in records[:2]] == [1, 2]
            assert records[3]["directory"] == directory, "cd captured destination, not starting directory"
            assert records[4]["directory"] == str(home / "sub")
            assert all(r["shell"] == shell and r["user"] for r in records)
            # Re-sourcing must not install duplicate hooks; the eval itself is a recorded command.
            send(f'eval "$({shlex.quote(binary)} shell-init {shell})"')
            send("printf repeat")
            records = read_records(env)
            assert len(records) == 9, records
            assert records[-1]["runCount"] == 3
            # A recording error must be visible without changing the command's status.
            os.chmod(home / "captured.db", 0o644)
            output = send("false")
            assert "metadata was not saved" in output, output
            assert (home / "prompt-status").read_text().splitlines()[-1] == "1"
            os.chmod(home / "captured.db", 0o600)
            # Default mode records in the background: the prompt returns before the write finishes.
            send("unset SMCLI_SYNC")
            send("printf async")
            deadline = time.monotonic() + 10
            while not any(r["command"] == "printf async" for r in read_records(env)):
                assert time.monotonic() < deadline, "background record never arrived"
                time.sleep(0.05)
            assert (home / "prompt-status").read_text().splitlines()[-1] == "0"
            # Leading-space commands and likely secrets stay out, even when the shell drops the space itself;
            # duplicates the shell leaves out of its own history are still recorded.
            send("set -o history; HISTCONTROL=ignoreboth" if shell == "bash" else "setopt HIST_IGNORE_SPACE HIST_IGNORE_DUPS")
            send(" printf hidden")
            send("echo AKIAABCDEFGHIJKLMNOP")
            send("printf dup")
            send("printf dup")
            deadline = time.monotonic() + 10
            while [r["command"] for r in read_records(env)][-2:] != ["printf dup", "printf dup"]:
                assert time.monotonic() < deadline, read_records(env)
                time.sleep(0.05)
            assert not any("hidden" in r["command"] or "AKIA" in r["command"] for r in read_records(env)), (shell, [r["command"] for r in read_records(env)][-6:])
            os.write(fd, b"exit 7\n")
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                if select.select([fd], [], [], 0)[0]:
                    try:
                        os.read(fd, 65536)
                    except OSError:
                        pass
                result, status = os.waitpid(pid, os.WNOHANG)
                if result:
                    alive = False
                    assert os.WIFEXITED(status) and os.WEXITSTATUS(status) == 7
                    break
                time.sleep(0.05)
            assert not alive, "shell exit timed out"
            records = read_records(env)
            assert len(records) == 15 and records[-1]["command"] == "exit 7" and records[-1]["exitCode"] == 7, records
            print(f"{shell} interactive metadata capture passed.")
        finally:
            os.close(fd)
            if alive:
                os.kill(pid, signal.SIGKILL)
                os.waitpid(pid, 0)


for shell in ("bash", "zsh"):
    run_shell(shell)

with tempfile.TemporaryDirectory() as directory:
    env = dict(os.environ, SMCLI_DB=str(Path(directory) / "concurrent.db"))
    writers = [subprocess.Popen([binary, "record", "bash", f"s{i}", directory, "0", str(1_700_000_000 + i), "", "", "echo shared"], env=env)
               for i in range(12)]
    # The six-argument form used by hooks from earlier versions must keep working.
    writers.append(subprocess.Popen([binary, "record", "zsh", directory, "0", "echo shared", ""], env=env))
    assert all(process.wait() == 0 for process in writers)
    records = read_records(env)
    assert [record["runCount"] for record in records] == list(range(1, 14)), records
    output = subprocess.run(["/bin/bash", "-c", f'trap ":" DEBUG; eval "$({shlex.quote(binary)} shell-init bash)"; trap -p DEBUG'], capture_output=True, text=True)
    assert "capture not installed" in output.stderr and "':' DEBUG" in output.stdout, output
    print("Concurrent append, legacy record arguments, JSON output, and existing trap protection passed.")
