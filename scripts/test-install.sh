#!/bin/bash
# install.sh and uninstall.sh end to end in a throwaway home: binaries, hooks for all three shells (idempotent,
# keeping the rest of each startup file), keys, a recorded command, then a full uninstall with export.
# Usage: test-install.sh <directory holding smcli and smcli-service>
set -euo pipefail
cd "$(dirname "$0")/.."
from=$(cd "${1:?usage: test-install.sh <bin dir>}" && pwd)
home=$(mktemp -d)
runtime=$(mktemp -d /tmp/smcli-inst.XXXXXX)
trap 'HOME="$home" "$home/bin/smcli" service stop >/dev/null 2>&1 || true; rm -rf "$home" "$runtime"' EXIT
# An inherited SMCLI_* or XDG_* variable can change results; the checks set their own.
unset "${!SMCLI_@}" XDG_CONFIG_HOME XDG_DATA_HOME XDG_STATE_HOME ZDOTDIR
export HOME=$home SMCLI_RUNTIME_DIR=$runtime SHELL=/bin/zsh
fail() { printf 'test-install: %s\n' "$*" >&2; exit 1; }

printf 'alias ll="ls -l"\n' > "$home/.zshrc"
for _ in 1 2; do  # twice: a rerun replaces the block rather than adding another
    bash scripts/install.sh --from "$from" --prefix "$home/bin" --shells zsh,bash,fish --keys left --yes > "$home/out" 2>&1 \
        || { cat "$home/out"; fail "install failed"; }
done
for f in smcli smcli-service smcli-uninstall; do [[ -x $home/bin/$f ]] || fail "$f not installed"; done
for f in .zshrc .bashrc .config/fish/config.fish; do
    [[ $(grep -c '^# >>> smcli >>>$' "$home/$f") == 1 ]] || fail "$f should hold one smcli block"
    grep -q "$home/bin/smcli' shell-init" "$home/$f" || fail "$f does not load the installed smcli"
done
grep -q '^alias ll="ls -l"$' "$home/.zshrc" || fail "install lost the user's own .zshrc lines"
grep -Eq '"leftKey" ?: true' "$home/.config/smcli/config.json" || fail "--keys was not saved"
zsh -fc "source '$home/.zshrc'; typeset -f _smcli_record > /dev/null" || fail "zsh did not load the hooks"
fish --version > /dev/null 2>&1 && { fish -c "source '$home/.config/fish/config.fish'; functions -q _smcli_record" || fail "fish did not load the hooks"; }

# Malformed marker blocks are rejected and the file is preserved.
malformed_check() {
    grep -q 'malformed smcli block' "$home/out" || fail "$1 diagnostic"
}
printf 'pre\n# >>> smcli >>>\npartial\n' > "$home/.zshrc"
bash scripts/install.sh --from "$from" --prefix "$home/bin" --shells zsh --keys left --yes > "$home/out" 2>&1 \
    && fail "install should refuse a file with a missing end marker"
malformed_check "missing-end marker"
grep -q '^partial$' "$home/.zshrc" || fail "missing-end file lost content"
! grep -q '^# <<< smcli <<<$' "$home/.zshrc" || fail "missing-end file gained an end marker"

printf 'pre\n# <<< smcli <<<\n# >>> smcli >>>\npost\n' > "$home/.zshrc"
bash scripts/install.sh --from "$from" --prefix "$home/bin" --shells zsh --keys left --yes > "$home/out" 2>&1 \
    && fail "install should refuse a file with reversed markers"
malformed_check "reversed marker"
grep -q '^post$' "$home/.zshrc" || fail "reversed-marker file lost content"

printf 'pre\n# >>> smcli >>>\n# >>> smcli >>>\npost\n# <<< smcli <<<\n' > "$home/.zshrc"
bash scripts/install.sh --from "$from" --prefix "$home/bin" --shells zsh --keys left --yes > "$home/out" 2>&1 \
    && fail "install should refuse a file with duplicate begin markers"
malformed_check "duplicate marker"
grep -q '^post$' "$home/.zshrc" || fail "duplicate-marker file lost content"

# Restore a valid .zshrc before the remaining checks.
printf 'alias ll="ls -l"\n' > "$home/.zshrc"
bash scripts/install.sh --from "$from" --prefix "$home/bin" --shells zsh --keys left --yes > "$home/out" 2>&1 \
    || { cat "$home/out"; fail "install failed after malformed tests"; }

"$home/bin/smcli" service status > /dev/null || fail "service not running after install"
SMCLI_SYNC=1 "$home/bin/smcli" record zsh s1 /tmp 0 "" "" "" "" "echo exported-marker"
[[ -f $home/.local/share/smcli/history.db ]] || fail "no database"

# Malformed blocks are also rejected during uninstall.
printf 'pre\n# >>> smcli >>>\npartial\n' > "$home/.zshrc"
bash "$home/bin/smcli-uninstall" --yes --prefix "$home/bin" > "$home/out" 2>&1 \
    && fail "uninstall should refuse a file with a missing end marker"
malformed_check "uninstall missing-end marker"
grep -q '^partial$' "$home/.zshrc" || fail "uninstall missing-end file lost content"

printf 'pre\n# <<< smcli <<<\n# >>> smcli >>>\npost\n' > "$home/.zshrc"
bash "$home/bin/smcli-uninstall" --yes --prefix "$home/bin" > "$home/out" 2>&1 \
    && fail "uninstall should refuse a file with reversed markers"
malformed_check "uninstall reversed marker"
grep -q '^post$' "$home/.zshrc" || fail "uninstall reversed-marker file lost content"

printf 'pre\n# >>> smcli >>>\n# >>> smcli >>>\npost\n# <<< smcli <<<\n' > "$home/.zshrc"
bash "$home/bin/smcli-uninstall" --yes --prefix "$home/bin" > "$home/out" 2>&1 \
    && fail "uninstall should refuse a file with duplicate begin markers"
malformed_check "uninstall duplicate marker"
grep -q '^post$' "$home/.zshrc" || fail "uninstall duplicate-marker file lost content"

# Restore valid hooks before the normal uninstall checks.
printf 'alias ll="ls -l"\n' > "$home/.zshrc"
bash scripts/install.sh --from "$from" --prefix "$home/bin" --shells zsh --keys left --yes > "$home/out" 2>&1 \
    || { cat "$home/out"; fail "install failed restoring hooks before uninstall tests"; }

# A missing confirmation changes nothing.
"$home/bin/smcli-uninstall" < /dev/null > /dev/null 2>&1 && fail "uninstall without --yes should refuse"
[[ -x $home/bin/smcli ]] || fail "refused uninstall removed files"

# Without sqlite3, refuse before removing hooks, executables or data.
mkdir "$home/no-sqlite"
for tool in dirname date; do ln -s "$(command -v "$tool")" "$home/no-sqlite/$tool"; done
PATH="$home/no-sqlite" "$home/bin/smcli-uninstall" --yes --export "$home/export" > "$home/out" 2>&1 \
    && fail "uninstall without sqlite3 should refuse"
grep -q 'sqlite3 is needed to export history' "$home/out" || fail "missing sqlite3 diagnostic"
[[ -x $home/bin/smcli && -f $home/.local/share/smcli/history.db && ! -e $home/export ]] || fail "refused uninstall changed files"
grep -q '^# >>> smcli >>>$' "$home/.zshrc" || fail "refused uninstall removed hooks"

# Existing, permissive destinations must also become private.
mkdir "$home/export"
chmod 755 "$home/export"
for f in history.db history.json config.json; do touch "$home/export/$f"; chmod 644 "$home/export/$f"; done
(umask 022; "$home/bin/smcli-uninstall" --yes --export "$home/export") > "$home/out" 2>&1 || { cat "$home/out"; fail "uninstall failed"; }
python3 - "$home/export" <<'PY'
import stat
import sys
from pathlib import Path
export = Path(sys.argv[1])
assert stat.S_IMODE(export.stat().st_mode) == 0o700
for name in ("history.db", "history.json", "config.json"):
    assert stat.S_IMODE((export / name).stat().st_mode) == 0o600, name
PY
grep -q 'exported-marker' "$home/export/history.json" || fail "JSON export is missing the recorded command"
[[ $(sqlite3 "$home/export/history.db" "SELECT count(*) FROM commands WHERE command = 'echo exported-marker'") == 1 ]] \
    || fail "database copy is missing the recorded command"
[[ -f $home/export/config.json ]] || fail "settings not exported"
for f in bin/smcli bin/smcli-service bin/smcli-uninstall .local/share/smcli .config/smcli; do
    [[ ! -e $home/$f ]] || fail "$f was not removed"
done
for f in .zshrc .bashrc .config/fish/config.fish; do
    ! grep -q smcli "$home/$f" || fail "$f still mentions smcli"
done
grep -q '^alias ll="ls -l"$' "$home/.zshrc" || fail "uninstall lost the user's own .zshrc lines"
[[ ! -d $runtime || -z $(ls -A "$runtime") ]] || fail "service socket left behind: $(ls -A "$runtime")"
printf 'Install and uninstall (with export) passed.\n'
