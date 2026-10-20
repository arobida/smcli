#!/usr/bin/env python3
"""Drive the file picker in real interactive Bash, Zsh and Fish on a pty: Up opens the picker, Ctrl-T switches to
files, Enter inserts the chosen path into the command line (never runs it), and the shell then sees exactly one
word for each awkward file name. Everything lives in throwaway directories; uninstalled shells are skipped.

Usage: test-file-picker.py <path to smcli>  (smcli-service must sit next to it).
"""
import os
import pty
import re
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

binary = str(Path(sys.argv[1]).resolve())
CSI = re.compile(rb"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07]*\x07|\x1b[=>]")
# Each file name starts with a unique token, which is what the picker is asked for.
NAMES = {
    "t01": "t01 plain.txt", "t02": "t02 sp ace.txt", "t03": "t03 it's.txt", "t04": 't04 q"uote.txt',
    "t05": "t05 $HOME.txt", "t06": "t06 `id`.txt", "t07": "t07 star*.txt", "t08": "t08 q?.txt",
    "t09": "t09 [br].txt", "t10": "t10 a;b&c|d.txt", "t11": "t11 back\\slash", "t12": "t12 new\nline.txt",
    "t13": "t13 tab\there", "t14": "t14 東京 é.txt",
    "-t15": "-t15 dash.txt",
}
root = tempfile.mkdtemp(prefix="smcli-fp.", dir="/tmp")


def printed(text):
    """What the printf printed (not the echoed command line, which still holds `%s`)."""
    return [m for m in re.findall(r"GOT<(.*?)>END", text.replace("\r\n", "\n"), re.S) if m != "%s"]


def shell_setup(shell, home):
    init = f'eval "$({binary} shell-init {shell})"\n'
    if shell == "fish":
        return ["--no-config", "--interactive", "-C", f"function fish_prompt; echo -n 'READY> '; end; {binary} shell-init fish | source"], {}
    if shell == "zsh":
        Path(home, ".zshrc").write_text("PROMPT='READY> '\nunsetopt BEEP\n" + init)
        return ["-i"], {"ZDOTDIR": home}
    Path(home, "rc").write_text("PS1='READY> '\n" + init)
    return ["--noprofile", "--rcfile", f"{home}/rc", "-i"], {}


def run(shell, path):
    home = tempfile.mkdtemp(prefix="h.", dir=root)
    repo = os.path.realpath(tempfile.mkdtemp(prefix="repo.", dir=root))
    for name in NAMES.values():
        Path(repo, name).write_text("x\n")
    Path(repo, "t00 decoy").write_text("x\n")
    for cmd in (["init", "-q"], ["add", "."], ["-c", "user.name=t", "-c", "user.email=t@e.x", "commit", "-q", "-m", "one"]):
        subprocess.run(["git", "-C", repo, *cmd], check=True, capture_output=True)
    args, extra = shell_setup(shell, home)
    env = dict(os.environ, HOME=home, TERM="xterm", SMCLI_DB=f"{home}/h.db", SMCLI_RUNTIME_DIR=f"{home}/run",
               SMCLI_CONFIG=f"{home}/config.json", **extra)
    env.pop("SMCLI_SERVICE", None)
    pid, fd = pty.fork()
    if pid == 0:
        signal.signal(signal.SIGINT, signal.SIG_DFL)
        os.chdir(repo)
        os.execve(path, [path] + args, env)
    buffer = b""

    def until(test, what, timeout=20):
        nonlocal buffer
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if select.select([fd], [], [], 0.1)[0]:
                try:
                    buffer += os.read(fd, 65536)
                except OSError:
                    break
            text = CSI.sub(b"", buffer).decode(errors="replace")
            if test(text):
                return text
        raise AssertionError(f"{shell}: timeout waiting for {what}: {CSI.sub(b'', buffer).decode(errors='replace')!r}")

    try:
        until(lambda t: "READY> " in t, "first prompt")
        for token, name in NAMES.items():
            buffer = b""
            os.write(fd, b"printf 'GOT<%s>END\\n' ")
            os.write(fd, b"\x1b[A")  # Up opens the picker with the typed text as its history query
            until(lambda t: "type to search" in t, "picker")
            os.write(fd, b"\x14")  # Ctrl-T: files
            until(lambda t: "files" in t and "enter insert path" in t, "files mode")
            os.write(fd, token.lstrip("-").encode())
            until(lambda t: "files results" in t, "results")
            os.write(fd, b"\r")
            # Let the shell finish repainting: read until the terminal has been quiet for a moment.
            while select.select([fd], [], [], 0.5)[0]:
                buffer += os.read(fd, 65536)
            before = CSI.sub(b"", buffer).decode(errors="replace")
            # Inserted, not run: nothing printed yet.
            assert not printed(before), f"{shell}: {name!r} ran: {before!r}"
            buffer = b""
            os.write(fd, b"\r")
            out = until(lambda t: printed(t), "command output")
            got = printed(out)[0]
            want = "./" + name if name.startswith("-") else name
            assert got == want, f"{shell}: {name!r}: shell received {got!r}, wanted {want!r}"
        print(f"{shell}: {len(NAMES)} awkward names inserted as one word each, none executed.")
    finally:
        try:
            os.kill(pid, signal.SIGKILL)
        except OSError:
            pass
        os.close(fd)
        subprocess.run([binary, "service", "stop"], env=env, capture_output=True, timeout=30)


try:
    for shell in ("bash", "zsh", "fish"):
        path = next((p for p in (f"/opt/homebrew/bin/{shell}", f"/usr/local/bin/{shell}", f"/bin/{shell}", f"/usr/bin/{shell}")
                     if os.access(p, os.X_OK)), None)
        if shell == "bash" and path:
            version = subprocess.run([path, "-c", "echo ${BASH_VERSINFO[0]}"], capture_output=True, text=True).stdout.strip()
            if not version.isdigit() or int(version) < 4:
                path = next((p for p in ("/opt/homebrew/bin/bash", "/usr/local/bin/bash") if os.access(p, os.X_OK)), None)
        if path is None:
            print(f"{shell}: not installed, skipped.")
            continue
        run(shell, path)
    print("File picker tests passed.")
finally:
    shutil.rmtree(root, ignore_errors=True)
