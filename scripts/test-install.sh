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
export HOME=$home SMCLI_RUNTIME_DIR=$runtime SHELL=/bin/zsh
unset SMCLI_DB SMCLI_CONFIG ZDOTDIR XDG_CONFIG_HOME SMCLI_SERVICE
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
"$home/bin/smcli" service status > /dev/null || fail "service not running after install"
SMCLI_SYNC=1 "$home/bin/smcli" record zsh s1 /tmp 0 "" "" "" "" "echo exported-marker"
[[ -f $home/.local/share/smcli/history.db ]] || fail "no database"

# A missing confirmation changes nothing.
"$home/bin/smcli-uninstall" < /dev/null > /dev/null 2>&1 && fail "uninstall without --yes should refuse"
[[ -x $home/bin/smcli ]] || fail "refused uninstall removed files"

"$home/bin/smcli-uninstall" --yes --export "$home/export" > "$home/out" 2>&1 || { cat "$home/out"; fail "uninstall failed"; }
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
