#!/usr/bin/env python3
"""Exercise the generated hooks in real interactive shells, using only temporary files.

Each variant is a shell plus the hook it will use (Bash: DEBUG trap, PS0, bash-preexec; Zsh; Fish). Shells that
aren't installed are skipped and said so. Everything runs against a throwaway home, database and service runtime
directory; the service is on by default, as it is for users.
"""
import json
import os
from pathlib import Path
import pty
import re
import select
import shlex
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time

binary = str(Path(sys.argv[1]).resolve())
# The prompt carries a counter the shell itself increments each time it draws one, so a wait can tell the prompt that
# follows a command from one already on screen (the echoed command line can't contain it).
prompt = re.compile(rb"__SMCLI_TEST_PROMPT__(\d+)__ $")
CSI = re.compile(rb"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07]*\x07")


def read_records(env):
    return json.loads(subprocess.check_output([binary, "history", "--json"], env=env))


def commands(env):
    return [r["command"] for r in read_records(env)]


def bash_version(path):
    try:
        out = subprocess.check_output([path, "-c", "echo ${BASH_VERSINFO[0]} ${BASH_VERSINFO[1]}"], text=True).split()
        return int(out[0]), int(out[1])
    except (OSError, subprocess.CalledProcessError, ValueError):
        return None


def stop_services(env):
    subprocess.run([binary, "service", "stop"], env=env, capture_output=True, timeout=30)


class Terminal:
    """A shell on a pty. `send` runs a line and waits for the next prompt; `type` only types."""

    def __init__(self, shell, args, env, directory):
        self.shell = shell
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            # A harness started in the background has SIGINT ignored, and the shell passes that on to every command,
            # so Ctrl-C would never stop `sleep`. Shells must start with the default disposition, as from a terminal.
            signal.signal(signal.SIGINT, signal.SIG_DFL)
            os.chdir(directory)
            os.execve(shell, [shell] + args, env)
        self.alive = True
        self.log = []
        self.last_prompt = 0
        # Output read while typing, kept for the next wait (a shell blocks on a full terminal if nobody reads).
        self.pending = b""

    def read(self, timeout):
        data = b""
        if select.select([self.fd], [], [], timeout)[0]:
            try:
                data = os.read(self.fd, 65536)
            except OSError:
                data = b""
        self.log.append(data)
        return data

    def until(self, test, what, timeout=20):
        deadline = time.monotonic() + timeout
        data, self.pending = self.pending, b""
        while time.monotonic() < deadline:
            chunk = self.read(0.1)
            data += chunk
            clean = CSI.sub(b"", data)
            if test(clean) and not chunk:
                # Quiet for a tick: the shell has finished repainting.
                return clean.decode(errors="replace")
        processes = subprocess.run(["ps", "-axo", "pid,ppid,pgid,stat,etime,command"], capture_output=True, text=True).stdout
        busy = "\n".join(line for line in processes.splitlines() if "smcli" in line or "sleep" in line)
        raise AssertionError(f"{self.shell}: timeout waiting for {what}: {data.decode(errors='replace')!r}\nprocesses:\n{busy}")

    def until_prompt(self, timeout=20, redrawn=False):
        """Waits for the prompt after a command. A cancelled line may redraw the prompt it already had (Fish does)."""
        def fresh(clean):
            match = prompt.search(clean)
            return bool(match) and int(match.group(1)) >= self.last_prompt + (0 if redrawn else 1)
        text = self.until(fresh, "a new prompt", timeout)
        self.last_prompt = int(prompt.search(text.encode()).group(1))
        return text

    def until_text(self, text):
        return self.until(lambda clean: text.encode() in clean, repr(text))

    def type(self, text):
        # Key by key, as a person types: a burst can land before the shell has put the terminal in raw mode.
        for char in text:
            os.write(self.fd, char.encode())
            time.sleep(0.003)
            while select.select([self.fd], [], [], 0)[0]:
                self.pending += self.read(0)

    def send(self, command):
        self.type(command + "\n")
        return self.until_prompt()

    def interrupt(self):
        """Ctrl-C until the prompt returns; one sent while the shell is still forking can miss the command."""
        for attempt in range(4):
            self.type("\x03")
            try:
                self.until_prompt(timeout=3)
                return
            except AssertionError:
                if attempt == 3:
                    raise

    def drain(self):
        time.sleep(0.3)
        while select.select([self.fd], [], [], 0.2)[0]:
            try:
                os.read(self.fd, 65536)
            except OSError:
                break

    def wait_exit(self, expected):
        os.write(self.fd, b"exit %d\n" % expected)
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            self.read(0.05)
            result, status = os.waitpid(self.pid, os.WNOHANG)
            if result:
                self.alive = False
                assert os.WIFEXITED(status) and os.WEXITSTATUS(status) == expected, status
                return
        raise AssertionError("shell exit timed out")

    def close(self):
        os.close(self.fd)
        if self.alive:
            os.kill(self.pid, signal.SIGKILL)
            os.waitpid(self.pid, 0)


def eventually(test, what, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = test()
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError(f"timed out: {what}")


# Prelude for the "bash-preexec" variant: the parts of bash-preexec that decide how hooks compose (the arrays, a DEBUG
# trap that calls preexec functions once per prompt, a PROMPT_COMMAND entry that calls precmd functions with $?
# restored). The real project isn't a dependency here.
BASH_PREEXEC = r"""
bash_preexec_imported=defined; __bp_imported=defined
preexec_functions=(); precmd_functions=()
__bp_interactive=0
__bp_precmd_invoke_cmd() {
    local last=$? cmd
    for cmd in "${precmd_functions[@]}"; do
        (exit "$last"); "$cmd"
    done
    return "$last"
}
__bp_preexec_invoke_exec() {
    [[ $__bp_interactive == 1 && $BASH_COMMAND != __bp_* ]] || return 0
    __bp_interactive=0
    local cmd line
    line=$(HISTTIMEFORMAT= builtin history 1); line=${line#*[0-9]  }
    for cmd in "${preexec_functions[@]}"; do "$cmd" "$line"; done
}
__bp_arm() { __bp_interactive=1; }
PROMPT_COMMAND="__bp_precmd_invoke_cmd${PROMPT_COMMAND:+; $PROMPT_COMMAND}; __bp_arm"
trap '__bp_preexec_invoke_exec' DEBUG
"""


class Variant:
    def __init__(self, label, kind, path, args, env=None, prelude=""):
        self.label, self.kind, self.path, self.args = label, kind, path, args
        self.extra_env, self.prelude = env or {}, prelude
        # Bash binds keys with `bind -x`, which needs 4.0.
        self.keys = kind != "bash" or (bash_version(path) or (0, 0)) >= (4, 0)
        # The DEBUG-trap hook refuses to install over an EXIT trap; the others chain onto it.
        self.exit_trap = kind == "bash" and self.extra_env.get("SMCLI_BASH_HOOK") != "debug"


def variants():
    found, skipped = [], []
    bash_args = ["--noprofile", "--norc", "-i"]
    system = "/bin/bash"
    if os.path.exists(system):
        found.append(Variant(f"bash {'.'.join(map(str, bash_version(system) or ()))} (DEBUG trap)", "bash", system, bash_args,
                             env={"SMCLI_BASH_HOOK": "debug"}))
    modern = next((p for p in (shutil.which("bash"), "/opt/homebrew/bin/bash", "/usr/local/bin/bash")
                   if p and (bash_version(p) or (0, 0)) >= (4, 4) and p != system), None)
    if modern:
        version = ".".join(map(str, bash_version(modern)))
        found.append(Variant(f"bash {version} (PS0)", "bash", modern, bash_args))
        found.append(Variant(f"bash {version} (DEBUG fallback)", "bash", modern, bash_args, env={"SMCLI_BASH_HOOK": "debug"}))
        found.append(Variant(f"bash {version} (bash-preexec)", "bash", modern, bash_args, prelude=BASH_PREEXEC))
    else:
        skipped.append("bash >= 4.4 (PS0 and bash-preexec variants)")
    if os.path.exists("/bin/bash") and not modern:
        pass
    zsh = shutil.which("zsh") or "/bin/zsh"
    if os.path.exists(zsh):
        found.append(Variant("zsh", "zsh", zsh, ["-d", "-f", "-i"]))
    else:
        skipped.append("zsh")
    fish = shutil.which("fish")
    if fish:
        found.append(Variant("fish", "fish", fish, ["--interactive"]))
    else:
        skipped.append("fish")
    return found, skipped


def new_environment(home, runtime, variant, **extra):
    env = dict(os.environ, HOME=str(home), HISTFILE=str(home / "native"), SMCLI_DB=str(home / "captured.db"),
               SMCLI_RUNTIME_DIR=runtime, SMCLI_CONFIG=str(home / "config.json"), SMCLI_SYNC="1",
               SHELL=variant.path, TERM="dumb", SMCLI_CANARY="canary-secret-4242", XDG_CONFIG_HOME=str(home / "config"), XDG_STATE_HOME=str(home / "state"))
    for name in ("SMCLI_SERVICE", "SMCLI_BASH_HOOK", "SMCLI_NO_KEYS", "SMCLI_EXTRA_KEYS", "SMCLI_SERVICE_BIN", "ZDOTDIR"):
        env.pop(name, None)
    env.update(variant.extra_env)
    env.update(extra)
    return env


def setup_prompt(t, kind):
    if kind == "fish":
        t.send("set -g _t 0; function fish_prompt; set -g _t (math $_t + 1); printf '__SMCLI_TEST_PROMPT__%s__ ' $_t; end; "
               "function fish_right_prompt; end; function fish_greeting; end; mkdir sub")
    else:
        t.send("_t=0; PS1='__SMCLI_TEST_PROMPT__$((++_t))__ '; PS2=''; mkdir sub" if kind == "bash" else
               "setopt prompt_subst; _t=0; PS1='__SMCLI_TEST_PROMPT__$((++_t))__ '; PS2=''; mkdir sub")


def init_line(kind):
    quoted = shlex.quote(binary)
    return f"{quoted} shell-init fish | source" if kind == "fish" else f'eval "$({quoted} shell-init {kind})"'


def run_variant(variant, scenario="base"):
    kind = variant.kind
    with tempfile.TemporaryDirectory() as directory:
        directory = str(Path(directory).resolve())
        home = Path(directory)
        # Unix socket paths are short; the temporary directory's own path is not.
        runtime = tempfile.mkdtemp(prefix="smcli-sh.", dir="/tmp")
        env = new_environment(home, runtime, variant)
        t = Terminal(variant.path, variant.args, env, directory)
        try:
            t.drain()
            setup_prompt(t, kind)
            if variant.prelude:
                t.send(variant.prelude.replace("\n", "\n"))
            check(t, variant, env, home, directory)
            print(f"{variant.label}: interactive metadata capture passed.")
        finally:
            t.close()
            stop_services(env)
            shutil.rmtree(runtime, ignore_errors=True)


def check(t, variant, env, home, directory):
    kind = variant.kind
    log = home / "prompt-status"
    # Something already in the prompt hook, which smcli must run and must not disturb ($? included).
    if kind == "bash" and variant.prelude:
        # Under bash-preexec, user code belongs in its arrays (a PROMPT_COMMAND added after it runs after $? is gone).
        t.send("_user_precmd() { printf '%s\\n' \"$?\" >> \"$HOME/prompt-status\"; }; precmd_functions+=(_user_precmd)")
    elif kind == "bash":
        t.send("PROMPT_COMMAND=\"${PROMPT_COMMAND:+$PROMPT_COMMAND; }printf '%s\\n' \\\"\\$?\\\" >> \\\"\\$HOME/prompt-status\\\"\"")
    elif kind == "zsh":
        t.send("precmd() { printf '%s\\n' \"$?\" >> \"$HOME/prompt-status\"; }")
    else:
        t.send("function _test_status --on-event fish_postexec; printf '%s\\n' $status >> $HOME/prompt-status; end")
    if variant.exit_trap:
        t.send("trap 'printf bye >> \"$HOME/exit-trap\"' EXIT")
    output = t.send(init_line(kind))
    # macOS ships Bash 3.2, which gets a one-time notice that picker keys need Bash 4+.
    output = "\n".join(l for l in output.splitlines() if "require Bash 4+" not in l)
    assert "smcli:" not in output, output
    t.send("printf repeat")
    t.send("printf repeat")
    t.send("false")
    assert log.read_text().splitlines()[-1] == "1", "existing prompt lost exit status"
    t.send("cd sub")
    t.send("pwd")
    t.send("sleep 1.1")
    t.send("printf '%s\\n' 'hello world'")
    records = read_records(env)
    assert [r["command"] for r in records] == ["printf repeat", "printf repeat", "false", "cd sub", "pwd", "sleep 1.1", "printf '%s\\n' 'hello world'"], [(r["command"], r["exitCode"]) for r in records]
    assert [r["exitCode"] for r in records] == [0, 0, 1, 0, 0, 0, 0], records
    # Bash < 5 only has whole seconds, so the 1.1s sleep may round to 1000 or 2000ms (fish takes whole seconds too).
    assert 1000 <= records[5]["durationMs"] < 3000, records[5]
    assert len({r["session"] for r in records}) == 1 and all(r["host"] for r in records), records
    assert [r["runCount"] for r in records[:2]] == [1, 2]
    assert records[3]["directory"] == directory, "cd captured destination, not starting directory"
    assert records[4]["directory"] == str(home / "sub")
    assert all(r["shell"] == kind and r["user"] for r in records)
    if kind in ("zsh", "fish"):
        assert records[2]["pipestatus"] == [1], records[2]
    else:
        assert all("pipestatus" not in r for r in records)

    # Re-sourcing must not install duplicate hooks; the eval itself is a recorded command.
    before = len(read_records(env))
    t.send(init_line(kind))
    t.send("printf repeat")
    records = read_records(env)
    assert len(records) == before + 2 and records[-1]["runCount"] == 3, records[-3:]
    if kind == "zsh":
        count = t.send("print -r -- HOOKS ${#${(M)precmd_functions:#_smcli_precmd}} ${#${(M)preexec_functions:#_smcli_start}} ${#${(M)zshexit_functions:#_smcli_zshexit}}")
        assert "HOOKS 1 1 1" in count, count
    elif kind == "bash":
        hooks = t.send("declare -p PROMPT_COMMAND precmd_functions preexec_functions 2>/dev/null; echo \"PS0=[$PS0]\"; trap -p EXIT DEBUG")
        # One PROMPT_COMMAND or precmd entry and one EXIT trap; at most one PS0 marker and DEBUG trap.
        assert hooks.count("_smcli_prompt") == 2, hooks
        assert hooks.count("_smcli_pending=1") <= 1 and hooks.count("_smcli_preexec") <= 1 and hooks.count("_smcli_bp_preexec") <= 1, hooks
    else:
        lines = t.send("functions --handlers | string match -r 'fish_p\\w+exec\\s+_smcli_\\w+' | count").splitlines()
        assert [line for line in lines if line.strip().isdigit()] == ["2"], lines

    # Pipelines: Zsh and Fish keep every stage's status.
    if kind in ("zsh", "fish"):
        t.send("false | true | false")
        records = read_records(env)
        assert records[-1]["exitCode"] == 1 and records[-1]["pipestatus"] == [1, 0, 1], records[-1]

    # A recording error must be visible without changing the command's status.
    # (The service would keep working on its open file, so this goes straight to the database.)
    t.send("set -gx SMCLI_SERVICE 0" if kind == "fish" else "export SMCLI_SERVICE=0")
    os.chmod(home / "captured.db", 0o644)
    output = t.send("false")
    assert "metadata was not saved" in output, output
    assert log.read_text().splitlines()[-1] == "1"
    os.chmod(home / "captured.db", 0o600)
    t.send("set -e SMCLI_SERVICE" if kind == "fish" else "unset SMCLI_SERVICE")

    # Multiline commands and Unicode are stored whole.
    t.send("printf '%s\\n' 'first line\nsecond line'" if kind != "fish" else "printf '%s\\n' 'first line\nsecond line'")
    t.send("printf '%s\\n' '東京 ✓ é'")
    latest = commands(env)
    assert latest[-1] == "printf '%s\\n' '東京 ✓ é'", latest[-1]
    multi = latest[-2]
    assert "first line" in multi and "second line" in multi and "\n" in multi, repr(multi)

    # Cancelling a half-typed line records nothing; Ctrl-C of a running command records the interrupt.
    count = len(read_records(env))
    t.type("echo never-run")
    time.sleep(0.2)
    t.type("\x03")
    t.until_prompt(redrawn=True)
    assert len(read_records(env)) == count, "a cancelled line was recorded"
    t.type("sleep 30\n")
    t.until_text("sleep 30")
    time.sleep(0.6)
    t.interrupt()
    interrupted = read_records(env)[-1]
    assert interrupted["command"] == "sleep 30" and interrupted["exitCode"] == 130, (interrupted["command"], b"".join(t.log[-60:]))

    # The picker: Up opens it, Ctrl-G cancels and leaves the line alone; Enter puts a command on the line without running it.
    t.send("echo picked-marker")
    if variant.keys:
        picker(t, env)

    # Leading-space commands and likely secrets stay out, whatever the shell itself does with the space.
    exclusions(t, variant, env)
    rest(t, variant, env, home, log)


def picker(t, env):
    count = len(read_records(env))
    t.type("echo abc")
    t.type("\x1b[A")
    t.until_text("esc cancel")
    t.type("\x03")
    t.until_text("echo abc")
    t.send("def")
    assert commands(env)[-1] == "echo abcdef", commands(env)[-3:]
    count = len(read_records(env))
    t.type("echo picked")
    t.type("\x12")
    t.until_text("esc cancel")
    t.type("\r")
    t.until_text("echo picked-marker")
    time.sleep(0.3)
    assert len(read_records(env)) == count, "selecting from the picker ran the command"
    t.type(" # tail\n")
    t.until_prompt()
    assert commands(env)[-1] == "echo picked-marker # tail", commands(env)[-2:]


def rest(t, variant, env, home, log):
    kind = variant.kind
    # No environment is sent along with a command.
    t.send("printf '%s' \"$SMCLI_CANARY\" > /dev/null")
    database = (home / "captured.db").read_bytes() + (home / "captured.db-wal").read_bytes() if (home / "captured.db-wal").exists() else (home / "captured.db").read_bytes()
    assert b"canary-secret-4242" not in database, "environment leaked into the database"

    # Default mode records in the background: the prompt returns before the write finishes.
    t.send("unset SMCLI_SYNC" if kind != "fish" else "set -e SMCLI_SYNC")
    t.send("printf async")
    eventually(lambda: any(r["command"] == "printf async" for r in read_records(env)), "background record")
    assert log.read_text().splitlines()[-1] == "0"
    # The service is the default, and it took the records.
    status = subprocess.run([binary, "service", "status"], env=dict(env, SMCLI_SERVICE="connect"), capture_output=True, text=True)
    assert status.returncode == 0, ("service was not started by default", status.stdout, status.stderr)

    # Service down: killed mid-session, then unstartable. Commands are still recorded.
    stop_services(env)
    t.send("printf after-stop")
    eventually(lambda: "printf after-stop" in commands(env), "record after the service stopped")
    pid = int(subprocess.run([binary, "service", "status"], env=dict(env, SMCLI_SERVICE="connect"), capture_output=True,
                             text=True).stdout.split("pid ")[1].split(")")[0]) if status.returncode == 0 else None
    t.send("export SMCLI_SERVICE_BIN=/nonexistent" if kind != "fish" else "set -gx SMCLI_SERVICE_BIN /nonexistent")
    stop_services(env)
    t.send("printf no-service")
    eventually(lambda: "printf no-service" in commands(env), "record without any service")
    t.send("export SMCLI_SERVICE=0" if kind != "fish" else "set -gx SMCLI_SERVICE 0")
    t.send("printf service-off")
    eventually(lambda: "printf service-off" in commands(env), "record with the service off")

    exit_status(t, variant, env, home)


def exclusions(t, variant, env):
    kind = variant.kind
    if kind == "bash":
        t.send("set -o history; HISTCONTROL=ignoredups")
        t.send("printf dup")
        t.send("printf dup")
        assert commands(env)[-2:] == ["printf dup", "printf dup"], commands(env)[-4:]
        t.send("HISTCONTROL=ignoreboth")
        t.send(" printf hidden")
        t.send("HISTIGNORE='printf ignored*:printf bare&'")
        t.send("printf ignored1")
        t.send("printf bare&")
        t.send("HISTIGNORE=; HISTCONTROL=")
    elif kind == "zsh":
        t.send("setopt HIST_IGNORE_SPACE HIST_IGNORE_DUPS")
        t.send("printf dup")
        t.send("printf dup")
        assert commands(env)[-2:] == ["printf dup", "printf dup"], commands(env)[-4:]
        t.send(" printf hidden")
        t.send("HISTORY_IGNORE='printf ignored*'")
        t.send("printf ignored1")
        t.send("unset HISTORY_IGNORE")
    else:
        t.send("printf dup")
        t.send("printf dup")
        assert commands(env)[-2:] == ["printf dup", "printf dup"], commands(env)[-4:]
        t.send(" printf hidden")
        t.send("function fish_should_add_to_history; string match -qr '^printf ignored' -- $argv[1]; and return 1; return 0; end")
        t.send("printf ignored1")
        t.send("functions -e fish_should_add_to_history")
        t.send("set -g fish_private_mode 1")
        t.send("printf private1")
        t.send("set -e fish_private_mode")
    t.send("echo AKIAABCDEFGHIJKLMNOP")
    t.send("printf visible-after")
    seen = commands(env)
    assert seen[-1] == "printf visible-after", seen[-4:]
    assert not any(word in c for c in seen for word in ("hidden", "AKIA", "ignored1", "private1")), seen[-8:]
    if kind == "bash":
        # Bare & in HISTIGNORE means previous-command duplicate to Bash, not a glob to smcli.
        assert "printf bare&" in seen, seen[-8:]


def exit_status(t, variant, env, home):
    kind = variant.kind
    before = len(read_records(env))
    t.wait_exit(7)
    records = read_records(env)
    assert len(records) == before + 1 and records[-1]["command"] == "exit 7" and records[-1]["exitCode"] == 7, records[-2:]
    if variant.exit_trap:
        assert (home / "exit-trap").read_text() == "bye", "the existing EXIT trap did not run"


# --- Plugin composition -----------------------------------------------------------------------------------------

ZSH_PLUGINS_BEFORE = r"""
autoload -Uz add-zsh-hook up-line-or-beginning-search
# oh-my-zsh style: hook functions through add-zsh-hook, history-aware Up.
_omz_precmd() { print -r -- "omz-precmd:$?" >> $HOME/plugin-log }
_omz_preexec() { print -r -- "omz-preexec:$1" >> $HOME/plugin-log }
add-zsh-hook precmd _omz_precmd
add-zsh-hook preexec _omz_preexec
zle -N up-line-or-beginning-search
bindkey -M emacs '^[[A' up-line-or-beginning-search
bindkey -M viins '^[[A' up-line-or-beginning-search
# atuin style: arrays appended to directly, its own Up widget on the application-mode sequence.
_atuin_preexec() { print -r -- "atuin-preexec:$1" >> $HOME/plugin-log }
_atuin_precmd() { print -r -- "atuin-precmd:$?" >> $HOME/plugin-log }
preexec_functions+=(_atuin_preexec)
precmd_functions+=(_atuin_precmd)
atuin-up-search() { BUFFER=ATUIN; CURSOR=$#BUFFER; zle redisplay }
zle -N atuin-up-search
bindkey -M emacs '^[OA' atuin-up-search
bindkey -M viins '^[OA' atuin-up-search
# fzf style: Ctrl-R widget.
fzf-history-widget() { BUFFER=FZF; CURSOR=$#BUFFER; zle redisplay }
zle -N fzf-history-widget
bindkey -M emacs '^R' fzf-history-widget
bindkey -M viins '^R' fzf-history-widget
# zsh-autosuggestions style: at the first prompt, wrap every widget it knows about in one that calls the original.
_fake_as_wrap() {
    local w
    for w in self-insert backward-delete-char up-line-or-history _smcli_pick _smcli_pick_up; do
        (( $+widgets[$w] )) || continue
        zle -A $w autosuggest-orig-$w
        eval "_fake_as_bound_${w}() { zle autosuggest-orig-${w} -- \"\$@\"; print -r -- wrapped:${w} >> \$HOME/plugin-log }"
        zle -N $w _fake_as_bound_$w
    done
    add-zsh-hook -d precmd _fake_as_wrap
}
"""

ZSH_PLUGINS_AFTER = r"""
add-zsh-hook precmd _fake_as_wrap
# User code defined after smcli's hooks: the classic functions must not displace smcli's.
precmd() { print -r -- "global-precmd:$?" >> $HOME/plugin-log }
preexec() { print -r -- "global-preexec:$1" >> $HOME/plugin-log }
zshexit() { print -r -- "global-zshexit" >> $HOME/plugin-log }
"""


def run_zsh_plugins(variant):
    with tempfile.TemporaryDirectory() as directory:
        directory = str(Path(directory).resolve())
        home = Path(directory)
        runtime = tempfile.mkdtemp(prefix="smcli-sh.", dir="/tmp")
        env = new_environment(home, runtime, variant)
        t = Terminal(variant.path, variant.args, env, directory)
        plugin_log = home / "plugin-log"
        try:
            t.drain()
            setup_prompt(t, "zsh")
            t.type(ZSH_PLUGINS_BEFORE.replace("\n", "\n"))
            t.type("\n")
            t.until_prompt()
            output = t.send(init_line("zsh"))
            # Ctrl-R (fzf) and the application-mode Up (atuin) are not smcli's to take; it says so.
            assert "kept existing key bindings" in output and "^R (fzf-history-widget)" in output and "^[OA (atuin-up-search)" in output, output
            t.type(ZSH_PLUGINS_AFTER)
            t.type("\n")
            t.until_prompt()
            t.send(":")  # first prompt after init: the autosuggestions wrapper runs
            bindings = t.send("bindkey -M emacs '^R'; bindkey -M emacs '^[OA'; bindkey -M emacs '^[[A'")
            assert '"^R" fzf-history-widget' in bindings and '"^[OA" atuin-up-search' in bindings, bindings
            assert '"^[[A" _smcli_pick_up' in bindings, bindings  # the history-aware default was ours to replace
            t.send("false")
            t.send("true | false")
            records = read_records(env)
            assert [r["command"] for r in records][-2:] == ["false", "true | false"], records[-3:]
            assert records[-2]["exitCode"] == 1 and records[-1]["pipestatus"] == [0, 1], records[-2:]
            lines = plugin_log.read_text().splitlines()
            for line in ("omz-preexec:false", "atuin-preexec:false", "global-preexec:false",
                         "omz-precmd:1", "atuin-precmd:1", "global-precmd:1"):
                assert line in lines, (line, lines)
            # The wrapped widget (autosuggestions) still reaches smcli's picker, and the picker still inserts without running.
            t.send("echo plugin-marker")
            count = len(read_records(env))
            t.type("\x1b[A")
            t.until_text("esc cancel")
            t.type("\x03")
            time.sleep(0.3)
            assert "wrapped:_smcli_pick_up" in plugin_log.read_text(), "the wrapper never saw smcli's widget"
            t.type("\x15")  # kill the line
            t.type("echo plugin")
            t.type("\x1b[A")
            t.until_text("esc cancel")
            t.type("\r")
            t.until_text("echo plugin-marker")
            assert len(read_records(env)) == count
            t.type("\x03")
            t.until_prompt(redrawn=True)
            assert len(read_records(env)) == count
            # Exit: smcli's hook and the user's zshexit both run.
            t.wait_exit(5)
            assert "global-zshexit" in plugin_log.read_text()
            assert read_records(env)[-1]["command"] == "exit 5"
            print("zsh plugin composition (oh-my-zsh, atuin, fzf, autosuggestions patterns) passed.")
        finally:
            t.close()
            stop_services(env)
            shutil.rmtree(runtime, ignore_errors=True)


BASH_PLUGINS = r"""
# fzf style (current releases): Ctrl-R is a readline macro. atuin style: a bind -x on the application-mode Up.
bind -m emacs-standard '"\C-r": "FZF-MACRO"'
_atuin_up() { READLINE_LINE=ATUIN; READLINE_POINT=5; }
bind -x '"\eOA": _atuin_up'
PS0='[ps0-user]'
"""


def run_bash_plugins(variant):
    with tempfile.TemporaryDirectory() as directory:
        directory = str(Path(directory).resolve())
        home = Path(directory)
        runtime = tempfile.mkdtemp(prefix="smcli-sh.", dir="/tmp")
        env = new_environment(home, runtime, variant)
        t = Terminal(variant.path, variant.args, env, directory)
        try:
            t.drain()
            setup_prompt(t, "bash")
            if variant.prelude:
                t.send(variant.prelude)
            # PROMPT_COMMAND as an array (Bash 5.1+) with entries of its own, an EXIT trap, and a user PS0.
            t.send("PROMPT_COMMAND+=('printf \"a%s\\n\" $? >> $HOME/pc-log' 'printf \"b%s\\n\" $? >> $HOME/pc-log')")
            t.send("trap 'printf bye >> $HOME/exit-trap' EXIT")
            for line in BASH_PLUGINS.strip().splitlines():
                if line.startswith("#"):
                    continue
                t.send(line)
            output = t.send(init_line("bash"))
            assert "kept existing key bindings: \\C-r \\eOA" in output, output
            assert "kept existing" not in t.send(init_line("bash")), "the kept-bindings note repeats in every shell"
            declared = t.send("declare -p PROMPT_COMMAND precmd_functions 2>/dev/null")
            # smcli's entry is in PROMPT_COMMAND, or in precmd_functions when bash-preexec owns the prompt hook.
            assert declared.count("printf") >= 2 and "_smcli_prompt" in declared, declared
            ps0 = t.send("echo PS0=$PS0")
            assert "ps0-user" in ps0 or variant.prelude, f"the user's PS0 was lost: {ps0!r}"
            t.send("false")
            t.send("true")
            records = read_records(env)
            assert [r["command"] for r in records][-2:] == ["false", "true"] and records[-2]["exitCode"] == 1, records[-2:]
            pc = (home / "pc-log").read_text().splitlines()
            assert "a1" in pc and "b1" in pc, f"user PROMPT_COMMAND entries lost $?: {pc}"
            keys = t.send("bind -X | grep -c _atuin_up; bind -s | grep -c FZF-MACRO; bind -X | grep -c '_smcli_pick\"'")
            # The user's PS0 ([ps0-user]) prints just before the first count.
            counts = re.findall(r"^(?:\[ps0-user\])?(\d+)\s*$", keys.replace("\r", ""), re.M)
            assert counts[-3:] == ["1", "1", "1"], f"atuin, fzf and smcli bindings should each be bound once: {keys!r}"
            t.type("\x1b[A")  # Up, not the application-mode sequence atuin took
            t.until_text("esc cancel")
            t.type("\x03")
            time.sleep(0.3)
            t.type("\x03")
            t.until_prompt(redrawn=True)
            t.wait_exit(3)
            assert (home / "exit-trap").read_text() == "bye", "the existing EXIT trap did not run"
            assert read_records(env)[-1]["command"] == "exit 3"
            print(f"{variant.label}: PROMPT_COMMAND array, EXIT trap, PS0 and key-binding composition passed.")
        finally:
            t.close()
            stop_services(env)
            shutil.rmtree(runtime, ignore_errors=True)


def run_fish_plugins(variant):
    with tempfile.TemporaryDirectory() as directory:
        directory = str(Path(directory).resolve())
        home = Path(directory)
        runtime = tempfile.mkdtemp(prefix="smcli-sh.", dir="/tmp")
        env = new_environment(home, runtime, variant)
        t = Terminal(variant.path, variant.args, env, directory)
        log = home / "plugin-log"
        try:
            t.drain()
            setup_prompt(t, "fish")
            t.send("function _plug_pre --on-event fish_preexec; echo pre:$argv >> $HOME/plugin-log; end")
            t.send("function _plug_post --on-event fish_postexec; echo post:$status:$argv >> $HOME/plugin-log; end")
            t.send("function _fzf_history; commandline -r FZF; end; bind \\cr _fzf_history")
            output = t.send(init_line("fish"))
            assert "kept existing key bindings: \\cr" in output, output
            assert "_fzf_history" in t.send("bind --user \\cr"), "fish: Ctrl-R binding was replaced"
            assert "_smcli_pick_up" in t.send("bind --user \\e\\[A"), "fish: Up was not bound"
            t.send("false")
            assert read_records(env)[-1]["command"] == "false"
            lines = log.read_text().splitlines()
            assert "pre:false" in lines and "post:1:false" in lines, lines
            # Up opens the picker; Enter puts the choice on the line without running it. Autosuggestion keys are untouched.
            t.send("echo fish-marker")
            count = len(read_records(env))
            t.type("echo fish")
            t.type("\x1b[A")
            t.until_text("esc cancel")
            t.type("\r")
            t.until_text("echo fish-marker")
            time.sleep(0.3)
            assert len(read_records(env)) == count, "selecting from the picker ran the command"
            t.type("\x03")
            t.until_prompt(redrawn=True)
            assert "forward-char" in t.send("bind \\e\\[C") or "right" in t.send("bind right"), "autosuggestion accept key changed"
            t.wait_exit(4)
            assert read_records(env)[-1]["command"] == "exit 4"
            print("fish plugin composition (existing handlers and key bindings) passed.")
        finally:
            t.close()
            stop_services(env)
            shutil.rmtree(runtime, ignore_errors=True)


def run_extra_keys(variant):
    """Left on an empty line is an extra key: off unless SMCLI_EXTRA_KEYS is set."""
    for enabled in (False, True):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory).resolve()
            runtime = tempfile.mkdtemp(prefix="smcli-sh.", dir="/tmp")
            env = new_environment(home, runtime, variant, **({"SMCLI_EXTRA_KEYS": "1"} if enabled else {}))
            t = Terminal(variant.path, variant.args, env, str(home))
            try:
                t.drain()
                setup_prompt(t, variant.kind)
                t.send(init_line(variant.kind))
                query = "bind -X" if variant.kind == "bash" else "bindkey '^[[D'"
                bound = t.send(query)
                assert ("_smcli_pick_left" in bound) == enabled, (variant.label, enabled, bound)
                t.wait_exit(0)
            finally:
                t.close()
                stop_services(env)
                shutil.rmtree(runtime, ignore_errors=True)
    print(f"{variant.label}: extra keys are opt-in.")


def run_init_keys(variant):
    """The keys chosen in `smcli init` (saved in the config) decide Ctrl-R and Left; Up stays bound."""
    query = {"bash": "bind -X", "zsh": "bindkey -M emacs", "fish": "bind --user"}[variant.kind]
    for ctrl_r, left in ((True, False), (False, True)):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory).resolve()
            (home / "config.json").write_text(json.dumps({"ctrlRKey": ctrl_r, "leftKey": left}))
            runtime = tempfile.mkdtemp(prefix="smcli-sh.", dir="/tmp")
            env = new_environment(home, runtime, variant)
            t = Terminal(variant.path, variant.args, env, str(home))
            try:
                t.drain()
                setup_prompt(t, variant.kind)
                t.send(init_line(variant.kind))
                bound = t.send(query)
                has_r = bool(re.search(r'(\\C-r|"\^R"|\\cr|ctrl-r)\S*\s+"?_smcli_pick\b', bound))
                assert has_r == ctrl_r, (variant.label, "Ctrl-R", ctrl_r, bound)
                assert ("_smcli_pick_left" in bound) == left, (variant.label, "Left", left, bound)
                assert "_smcli_pick_up" in bound or "_smcli_pick" in bound, (variant.label, "Up", bound)
                t.wait_exit(0)
            finally:
                t.close()
                stop_services(env)
                shutil.rmtree(runtime, ignore_errors=True)
    print(f"{variant.label}: Ctrl-R and Left follow the keys chosen in smcli init.")


def run_no_keys(variant):
    with tempfile.TemporaryDirectory() as directory:
        home = Path(directory).resolve()
        runtime = tempfile.mkdtemp(prefix="smcli-sh.", dir="/tmp")
        env = new_environment(home, runtime, variant, SMCLI_NO_KEYS="1")
        t = Terminal(variant.path, variant.args, env, str(home))
        try:
            t.drain()
            setup_prompt(t, variant.kind)
            t.send(init_line(variant.kind))
            query = {"bash": "bind -X", "zsh": "bindkey '^R'", "fish": "bind --user"}[variant.kind]
            assert "_smcli_pick" not in t.send(query)
            t.send("printf still-recorded")
            assert commands(env)[-1] == "printf still-recorded"
            t.wait_exit(0)
        finally:
            t.close()
            stop_services(env)
            shutil.rmtree(runtime, ignore_errors=True)
    print(f"{variant.label}: SMCLI_NO_KEYS leaves the keys alone and capture still works.")


def run_shared_service(chosen):
    """Several live shells (one per kind found, plus extra zsh) share one service; the service is killed mid-session.

    Every command must be recorded exactly once, by its own session, whether it went through the service, a
    restarted one or the direct fallback."""
    from concurrent.futures import ThreadPoolExecutor
    with tempfile.TemporaryDirectory() as directory:
        home = Path(directory).resolve()
        runtime = tempfile.mkdtemp(prefix="smcli-sh.", dir="/tmp")
        env = new_environment(home, runtime, chosen[0])
        env.pop("SMCLI_SYNC")  # the service path is the one under test, not the synchronous in-process one
        terminals = []
        try:
            for n, variant in enumerate(chosen):
                shell_env = new_environment(home, runtime, variant)
                shell_env.pop("SMCLI_SYNC")
                t = Terminal(variant.path, variant.args, shell_env, str(home))
                t.drain()
                setup_prompt(t, variant.kind)
                if variant.prelude:
                    t.send(variant.prelude)
                t.send(init_line(variant.kind))
                terminals.append((n, variant, t))
            per_shell = 12
            killed = []

            def session(item):
                n, variant, t = item
                for k in range(per_shell):
                    t.send(f"echo shared-{n}-{k}")
                    if k == per_shell // 2 and n == 0:
                        pids = subprocess.run([binary, "service", "status"], env=dict(env, SMCLI_SERVICE="connect"),
                                              capture_output=True, text=True).stdout
                        if "pid " in pids:
                            os.kill(int(pids.split("pid ")[1].split(")")[0]), signal.SIGKILL)
                            killed.append(pids)
                return n
            with ThreadPoolExecutor(len(terminals)) as pool:
                list(pool.map(session, terminals))
            assert killed, "the service should have been running to kill"
            for _, _, t in terminals:
                t.wait_exit(0)
            expected = {f"echo shared-{n}-{k}" for n in range(len(terminals)) for k in range(per_shell)}
            done = lambda: [r for r in read_records(env) if r["command"].startswith("echo shared-")]
            eventually(lambda: len(done()) >= len(expected), "every record to land", timeout=20)
            records = done()
            assert sorted(r["command"] for r in records) == sorted(expected), \
                (len(records), sorted(expected - {r["command"] for r in records}))
            sessions = {r["command"].split("-")[1]: set() for r in records}
            for r in records:
                sessions[r["command"].split("-")[1]].add(r.get("session"))
            assert all(len(ids) == 1 for ids in sessions.values()) and len({next(iter(i)) for i in sessions.values()}) == len(terminals), sessions
        finally:
            for _, _, t in terminals:
                t.close()
            stop_services(env)
            shutil.rmtree(runtime, ignore_errors=True)
    print(f"{len(chosen)} live shells ({', '.join(v.label for v in chosen)}): concurrent capture through one service, "
          "killed mid-session, nothing lost or duplicated.")


ARGV_STUB = """#!/usr/bin/env python3
import os, sys
path = os.environ.get("SMCLI_ARGV_LOG")
if path:
    with open(path, "a") as handle:
        handle.write("\\0".join(sys.argv[1:]) + "\\n")
"""


def record_rows(path):
    if not path.exists():
        return []
    rows = []
    for line in path.read_bytes().split(b"\n"):
        if line:
            rows.append([part.decode() for part in line.split(b"\0")])
    return rows


def typed_records(rows, command):
    found = []
    for argv in rows:
        if not argv or argv[0] != "record":
            continue
        if argv[1:2] == ["--expanded"]:
            expansion, fields = argv[2], argv[3:]
        else:
            expansion, fields = None, argv[1:]
        if fields and fields[-1] == command:
            found.append((expansion, fields, argv))
    return found


def define_alias(t, kind, name, body):
    quoted = shlex.quote(body)
    t.send(f"alias {name} {quoted}" if kind == "fish" else f"alias {name}={quoted}")


def run_alias_capture(variant):
    """A stub logs record argv, so the test sees exactly what each hook sends."""
    kind = variant.kind
    with tempfile.TemporaryDirectory() as directory:
        home = Path(directory).resolve()
        runtime = tempfile.mkdtemp(prefix="smcli-sh.", dir="/tmp")
        env = new_environment(home, runtime, variant)
        log = home / "argv-log"
        stub = home / "argv-stub"
        stub.write_text(ARGV_STUB)
        stub.chmod(0o755)
        env["SMCLI_ARGV_LOG"] = str(log)
        t = Terminal(variant.path, variant.args, env, str(home))
        try:
            t.drain()
            setup_prompt(t, kind)
            if variant.prelude:
                t.send(variant.prelude)
            t.send(init_line(kind))
            quoted_stub = shlex.quote(str(stub))
            t.send(f"set -g _smcli_bin {quoted_stub}" if kind == "fish" else f"_smcli_bin={quoted_stub}")

            def expect(command, expansion):
                rows = record_rows(log)
                hits = typed_records(rows, command)
                assert hits, (variant.label, command, rows[-6:])
                got, fields, argv = hits[-1]
                assert got == expansion, (variant.label, command, got, argv)
                assert len(fields) == (8 if kind == "bash" else 9), (variant.label, command, fields)
                assert fields[0] == kind and "--expanded" not in fields, (variant.label, fields)
                if expansion is None:
                    assert argv[1] != "--expanded", (variant.label, argv)

            t.send("printf plain")
            expect("printf plain", None)
            define_alias(t, kind, "gst", "printf status")
            t.send("gst")
            expect("gst", "printf status")
            t.send("gst --short")
            expect("gst --short", "printf status --short")
            define_alias(t, kind, "deploy", "TOKEN=abc123 ./deploy.sh")
            t.send("deploy")
            expect("deploy", None)
            if kind != "fish":
                for name, body in (("parenassign", "(TOKEN=abc printf ok)"), ("appendassign", "TOKEN+=abc printf ok")):
                    define_alias(t, kind, name, body)
                    t.send(name)
                    if name == "parenassign" and kind == "bash" and "PS0" not in variant.label:
                        assert not any(hit[0] for hit in typed_records(record_rows(log), name)), variant.label
                    else:
                        expect(name, None)
            if kind == "bash":
                define_alias(t, kind, "singlequoted", "printf '%s' hi")
                t.send("singlequoted")
                expect("singlequoted", None if "3.2" in variant.label else "printf '%s' hi")
                # Exercise the fallback directly even when the host only has modern Bash.
                t.send(f'_smcli_alias_body singlequoted > {shlex.quote(str(home / "fallback-body"))}')
                assert (home / "fallback-body").read_text() == "", variant.label
                if "PS0" in variant.label:
                    t.send(f'printf "%s" "$_smcli_expfile" > {shlex.quote(str(home / "transfer-path"))}')
                    transfer = Path((home / "transfer-path").read_text())
                    assert transfer.stat().st_mode & 0o777 == 0o600, variant.label
                    assert transfer.parent.stat().st_mode & 0o777 == 0o700, variant.label
            define_alias(t, kind, "sub", "printf $(printf hi)")
            t.send("sub")
            expect("sub", None)
            define_alias(t, kind, "op", "printf a | cat")
            t.send("op")
            expect("op", "printf a | cat")
            define_alias(t, kind, "quoted", 'printf "hi"')
            t.send("quoted")
            expect("quoted", 'printf "hi"')
            define_alias(t, kind, "redir", "printf a > /dev/null")
            t.send("redir")
            expect("redir", None)
            # Variables are stored as written and never read or evaluated.
            assign = (lambda n, v: f"set -gx {n} {v}") if kind == "fish" else (lambda n, v: f"export {n}={v}")
            t.send(assign("EDITOR", "printf"))
            t.send(assign("ZDOTDIR", shlex.quote(str(home))))
            t.send(assign("TOK", "abc123"))
            mkal = 'printf $ZDOTDIR/x && printf "$ZDOTDIR/y"' if kind == "fish" else '$EDITOR $ZDOTDIR/x && printf "$ZDOTDIR/y"'
            define_alias(t, kind, "mkal", mkal)
            t.send("mkal")
            expect("mkal", mkal)
            define_alias(t, kind, "tok", "printf $TOK")
            t.send("tok")
            expect("tok", "printf $TOK")
            t.send(assign("EVIL", shlex.quote(f"$(touch {home}/pwned)")))
            define_alias(t, kind, "ev", "printf $EVIL")
            t.send("ev")
            expect("ev", "printf $EVIL")
            assert not (home / "pwned").exists(), variant.label
            # Lookup is at preexec: the command redefines its own alias, and the next prompt must not win.
            if kind == "fish":
                t.send("function redefine_gl; alias gl 'printf files'; end")
            else:
                t.send("redefine_gl() { alias gl='printf files'; }")
            define_alias(t, kind, "gl", "redefine_gl")
            t.send("gl")
            expect("gl", "redefine_gl")
            define_alias(t, kind, "gl", "printf log")
            t.send("gl")
            expect("gl", "printf log")
            define_alias(t, kind, "gl", "printf files")
            t.send("gl extra")
            expect("gl extra", "printf files extra")
            define_alias(t, kind, "longcmd", "printf " + ("a" * 130))
            t.send("longcmd")
            expect("longcmd", "printf " + ("a" * 130))
            t.send("printf after-aliases")
            expect("printf after-aliases", None)
            if kind == "fish":
                t.send("function custom_fn; echo custom-fn; end")
                t.send("custom_fn")
                expect("custom_fn", None)
                t.send("abbr -a agst 'printf abbr-marker'")
                t.send("agst")
                rows = record_rows(log)
                expanded = typed_records(rows, "printf abbr-marker") or typed_records(rows, "printf abbr-marker ")
                assert expanded and expanded[-1][0] is None, (variant.label, rows[-4:])
                assert not typed_records(rows, "agst"), (variant.label, rows[-4:])
            sent = [argv[i + 1] for argv in record_rows(log) for i, a in enumerate(argv[:-1]) if a == "--expanded"]
            assert not any(token in value for value in sent for token in ("abc123", "$(", ">")), sent
            t.wait_exit(0)
            if kind == "bash" and "PS0" in variant.label:
                assert not transfer.exists() and not transfer.parent.exists(), variant.label
            print(f"{variant.label}: alias expansion capture passed.")
        finally:
            t.close()
            stop_services(env)
            shutil.rmtree(runtime, ignore_errors=True)


def run_at_completion(variant):
    """Tab on @word offers a cwd file; ordinary Tab and SMCLI_NO_KEYS stay the shell's."""
    kind = variant.kind
    with tempfile.TemporaryDirectory() as directory:
        home = Path(directory).resolve()
        runtime = tempfile.mkdtemp(prefix="smcli-sh.", dir="/tmp")
        env = new_environment(home, runtime, variant)
        (home / "smcli-at-probe42.txt").write_text("ok\n")
        (home / "my notes.txt").write_text("x\n")
        (home / "zz space.txt").write_text("ok\n")
        t = Terminal(variant.path, variant.args, env, str(home))
        try:
            t.drain()
            setup_prompt(t, kind)
            if variant.prelude:
                t.send(variant.prelude)
            t.send(init_line(kind))
            t.type("printf %s my")
            t.type("\t")
            t.until_text("notes")
            t.type("\n")
            t.until_prompt()
            t.type("printf %s @probe42")
            t.type("\t")
            t.until_text("smcli-at-probe42.txt")
            t.type("\n")
            out = t.until_prompt()
            assert "smcli-at-probe42.txt" in out, (variant.label, out)
            t.type("printf '%s\\n' @zz")
            t.type("\t")
            t.until_text("zz space.txt")
            t.type("\n")
            out = t.until_prompt()
            assert "zz space.txt" in out, (variant.label, out)
            t.send("true npm i @types/node")
            assert commands(env)[-1] == "true npm i @types/node", (variant.label, commands(env)[-3:])
            t.wait_exit(0)
            print(f"{variant.label}: @ completion, quoted spaces, and ordinary Tab passed.")
        finally:
            t.close()
            stop_services(env)
            shutil.rmtree(runtime, ignore_errors=True)
    with tempfile.TemporaryDirectory() as directory:
        home = Path(directory).resolve()
        runtime = tempfile.mkdtemp(prefix="smcli-sh.", dir="/tmp")
        env = new_environment(home, runtime, variant, SMCLI_NO_KEYS="1")
        (home / "smcli-at-probe42.txt").write_text("ok\n")
        t = Terminal(variant.path, variant.args, env, str(home))
        try:
            t.drain()
            setup_prompt(t, kind)
            if variant.prelude:
                t.send(variant.prelude)
            t.send(init_line(kind))
            bound = t.send({"bash": "bind -X", "zsh": "bindkey '^I'", "fish": "bind --user \\t"}[kind])
            assert "_smcli_tab" not in bound and "_smcli_complete_widget" not in bound, (variant.label, bound)
            t.type("printf %s @probe42")
            t.type("\t")
            t.type("\n")
            t.until_prompt()
            assert commands(env)[-1] == "printf %s @probe42", (variant.label, commands(env)[-3:])
            t.wait_exit(0)
            print(f"{variant.label}: SMCLI_NO_KEYS leaves Tab alone.")
        finally:
            t.close()
            stop_services(env)
            shutil.rmtree(runtime, ignore_errors=True)


found, skipped = variants()
# SMCLI_TEST_ONLY=<substring of a variant label> narrows a run while debugging.
only = os.environ.get("SMCLI_TEST_ONLY", "")
found = [v for v in found if only in v.label]
for variant in found:
    run_variant(variant)
    run_alias_capture(variant)
    run_at_completion(variant)
by_label = {v.label: v for v in found}
zsh_variant = next((v for v in found if v.kind == "zsh"), None)
if zsh_variant:
    run_zsh_plugins(zsh_variant)
for v in found:
    # (The DEBUG-trap hook refuses to install over the EXIT trap this scenario starts with.)
    if v.kind == "bash" and v.exit_trap:
        run_bash_plugins(v)
fish_variant = next((v for v in found if v.kind == "fish"), None)
if fish_variant:
    run_fish_plugins(fish_variant)
for v in found:
    if v.keys and v.kind in ("zsh", "bash") and "fallback" not in v.label and "bash-preexec" not in v.label:
        run_extra_keys(v)
    if v.keys and (v.kind != "bash" or "PS0" in v.label):
        run_no_keys(v)
        run_init_keys(v)
for name in skipped:
    print(f"SKIPPED (not installed): {name}")

# One live shell of each kind plus a second zsh, sharing a service that is killed while they work.
shared = [next((v for v in found if v.kind == kind and (kind != "bash" or "PS0" in v.label)), None) for kind in ("zsh", "bash", "fish")]
shared = [v for v in shared if v] or [next((v for v in found if v.kind == "bash"), None)]
shared = [v for v in shared if v]
if shared:
    run_shared_service(shared + [v for v in shared if v.kind == "zsh"])


# --- Without shells ---------------------------------------------------------------------------------------------------

with tempfile.TemporaryDirectory() as directory:
    runtime = tempfile.mkdtemp(prefix="smcli-sh.", dir="/tmp")
    env = dict(os.environ, SMCLI_DB=str(Path(directory) / "concurrent.db"), SMCLI_RUNTIME_DIR=runtime)
    env.pop("SMCLI_SERVICE", None)
    try:
        writers = [subprocess.Popen([binary, "record", "bash", f"s{i}", directory, "0", str(1_700_000_000 + i), "", "", "echo shared"], env=env)
                   for i in range(12)]
        # The six-argument form used by hooks from earlier versions must keep working.
        writers.append(subprocess.Popen([binary, "record", "zsh", directory, "0", "echo shared", ""], env=env))
        results = [(process.args, process.wait()) for process in writers]
        failures = [(args, code) for args, code in results if code != 0]
        assert not failures, f"Concurrent writers failed (command, exit code): {failures}"
        records = read_records(env)
        assert [record["runCount"] for record in records] == list(range(1, 14)), records
        # Nine arguments (no pipestatus) and ten (with it), and a bad pipestatus is refused.
        subprocess.check_call([binary, "record", "zsh", "p", directory, "2", "1700000100", "1700000101", "", "echo nine"], env=env)
        subprocess.check_call([binary, "record", "zsh", "p", directory, "1", "0 1 1", "1700000200", "1700000201", "", "echo ten"], env=env)
        rejected = subprocess.run([binary, "record", "zsh", "p", directory, "1", "0 x", "1700000300", "1700000301", "", "echo bad"],
                                  env=env, capture_output=True, text=True)
        assert rejected.returncode != 0 and "pipestatus" in rejected.stderr, rejected
        records = {r["command"]: r for r in read_records(env)}
        assert "pipestatus" not in records["echo nine"] and records["echo ten"]["pipestatus"] == [0, 1, 1]
        assert "echo bad" not in records
        output = subprocess.run(["/bin/bash", "-c", f'trap ":" DEBUG; eval "$({shlex.quote(binary)} shell-init bash)"; trap -p DEBUG'], capture_output=True, text=True)
        assert "capture not installed" in output.stderr and "':' DEBUG" in output.stdout, output
        print("Concurrent append, legacy record arguments, pipestatus, JSON output, and existing trap protection passed.")
    finally:
        stop_services(env)
        shutil.rmtree(runtime, ignore_errors=True)

# fish and bash both parse what shell-init prints.
with tempfile.TemporaryDirectory() as directory:
    env = dict(os.environ, HOME=directory)
    for kind, check_command in (("bash", ["/bin/bash", "-n"]), ("zsh", [shutil.which("zsh") or "/bin/zsh", "-n"]),
                                ("fish", [shutil.which("fish") or "", "--no-execute"])):
        if not check_command[0]:
            continue
        script = Path(directory) / f"init.{kind}"
        script.write_text(subprocess.check_output([binary, "shell-init", kind], env=env, text=True))
        result = subprocess.run(check_command + [str(script)], capture_output=True, text=True)
        assert result.returncode == 0, (kind, result.stderr)
    print("Generated hooks pass each shell's syntax check.")
