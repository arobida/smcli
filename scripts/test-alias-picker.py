#!/usr/bin/env python3
"""Alias presentation/insertion on a real controlling tty, with isolated history and no shell execution."""
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time

binary = os.path.abspath(sys.argv[1])
ansi = re.compile(rb"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07]*\x07")
with tempfile.TemporaryDirectory(prefix="smcli-alias.", dir="/tmp") as directory:
    root = Path(directory)
    env = {k: v for k, v in os.environ.items() if not k.startswith("SMCLI_") and not k.startswith("XDG_")}
    env.update(HOME=directory, PWD=directory, HISTFILE="", SMCLI_DB=str(root / "history.db"),
               SMCLI_RUNTIME_DIR=str(root / "run"), SMCLI_CONFIG=str(root / "config.json"), TERM="xterm-256color")
    marker = root / "must-not-execute"
    expansion = f"touch {marker}"
    for typed, expanded in [("zzalias", expansion), ("gl", "git log"), ("gl", "git ls-files")]:
        subprocess.run([binary, "record", "--expanded", expanded, "zsh", "alias", directory, "0", "", "", "", typed],
                       env=dict(env, SMCLI_SERVICE="0"), check=True, capture_output=True)

    def choose(query, expected, mode):
        output = root / "chosen"
        pid, fd = pty.fork()
        if pid == 0:
            os.chdir(directory)
            os.dup2(os.open(output, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), 1)
            os.execve(binary, [binary, "pick", "--", query], dict(env, SMCLI_SERVICE=mode))
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 180, 0, 0))
        screen = b""
        reaped = False
        sent = False
        deadline = time.monotonic() + 30
        try:
            while time.monotonic() < deadline:
                if select.select([fd], [], [], 0.05)[0]:
                    try:
                        screen += os.read(fd, 65536)
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise
                visible = ansi.sub(b"", screen).decode(errors="replace")
                if not sent and "esc cancel" in visible:
                    assert "→" in visible and expected in visible, visible
                    os.write(fd, b"\r")
                    sent = True
                done, status = os.waitpid(pid, os.WNOHANG)
                if done:
                    reaped = True
                    assert os.waitstatus_to_exitcode(status) == 0, visible
                    assert output.read_text() == expected + "\n", output.read_text()
                    assert not marker.exists(), "picking must insert, never execute"
                    return
            raise AssertionError("alias picker timed out")
        finally:
            if not reaped:
                try:
                    os.kill(pid, 9)
                    os.waitpid(pid, 0)
                except ProcessLookupError:
                    pass
            os.close(fd)

    try:
        for mode in ("0", "on"):
            choose("zzalias", expansion, mode)
            choose("ls-files", "git ls-files", mode)
        records = json.loads(subprocess.check_output([binary, "history", "--json"], env=env))
        assert [(r["command"], r["expandedCommand"]) for r in records] == [
            ("zzalias", expansion), ("gl", "git log"), ("gl", "git ls-files")]
        print("Alias picker: arrow labels, conflicting variants, service/fallback insertion, no execution passed.")
    finally:
        subprocess.run([binary, "service", "stop"], env=env, capture_output=True)
