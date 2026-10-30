#!/bin/bash
# Black-box smoke of the smcli binary: usage, native-history reading (and that it is never rewritten),
# search, init --keys, shell-init, clean-dist and @-completion. Throwaway HOME and runtime directory;
# no install, no long-lived service.
# Usage: test-cli.sh <path to smcli>
set -euo pipefail
bin="$(cd "$(dirname "${1:?usage: test-cli.sh <path to smcli>}")" && pwd)/$(basename "$1")"
cd "$(dirname "$0")/.."
home="$(mktemp -d)"
runtime="$(mktemp -d /tmp/smcli-cli.XXXXXX)"
# An inherited SMCLI_* or XDG_* variable can change results; these checks set their own.
unset "${!SMCLI_@}" XDG_CONFIG_HOME XDG_DATA_HOME XDG_STATE_HOME
export SMCLI_DB="$home/captured.db" SMCLI_RUNTIME_DIR="$runtime"
trap '"$bin" service stop >/dev/null 2>&1 || true; rm -rf "$home" "$runtime"' EXIT

output="$("$bin")"
[[ "$output" == 'Usage: smcli init'* ]]
[[ "$(HOME="$home" HISTFILE='' "$bin" history)" == 'No saved Bash or Zsh history found.' ]]
printf '#1700000000\necho bash\n' > "$home/.bash_history"
printf ': 1700000000:0;echo zsh\n' > "$home/.zsh_history"
cp "$home/.bash_history" "$home/bash-history.before"
cp "$home/.zsh_history" "$home/zsh-history.before"
output="$(HOME="$home" HISTFILE='' "$bin" history)"
expected="$(printf '==> %s/.bash_history <==\necho bash\n==> %s/.zsh_history <==\necho zsh' "$home" "$home")"
[[ "$output" == "$expected" ]]

if HOME="$home" HISTFILE="$home/missing" "$bin" history > "$home/output" 2> "$home/error"; then
    printf 'Missing custom history should fail.\n' >&2
    exit 1
fi
[[ ! -s "$home/output" && -s "$home/error" ]]
if "$bin" unknown > "$home/output" 2> "$home/error"; then
    printf 'Unknown command should fail.\n' >&2
    exit 1
fi
[[ "$(HOME="$home" HISTFILE='' "$bin" search echo zsh -n 1)" == 'echo zsh'* ]]
# init --keys sets the picker keys without asking, and shell-init follows them.
cfg="$home/keys.json"
SMCLI_CONFIG="$cfg" "$bin" init --keys left > /dev/null
grep -Eq '"ctrlRKey" ?: false' "$cfg" && grep -Eq '"leftKey" ?: true' "$cfg" && grep -Eq '"ctrlTKey" ?: false' "$cfg"
init="$(SMCLI_CONFIG="$cfg" "$bin" shell-init zsh)"
[[ $init == *"(( 0 )) && _smcli_bind \$_smcli_keymap '^R'"* && $init == *"|| (( 1 )); then"* ]]
[[ $init == *"(( 0 )) && _smcli_bind \$_smcli_keymap '^T'"* && $init == *"(( 0 )) && _smcli_bind \$_smcli_keymap '^G'"* ]]
SMCLI_CONFIG="$cfg" "$bin" init --keys ctrl-r,left > /dev/null
grep -Eq '"ctrlRKey" ?: true' "$cfg"
if SMCLI_CONFIG="$cfg" "$bin" init --keys up > /dev/null 2>&1; then
    printf 'An unknown key should fail.\n' >&2
    exit 1
fi
# make clean keeps the latest CLI and service pair and both symlinks.
dist="$home/dist"
mkdir -p "$dist"
touch "$dist"/smcli-1-a "$dist"/smcli-service-1-a "$dist"/smcli-2-b "$dist"/smcli-service-2-b
ln -s smcli-2-b "$dist/smcli"
ln -s smcli-service-2-b "$dist/smcli-service"
bash scripts/clean-dist.sh "$dist"
[[ "$(cd "$dist" && ls | tr '\n' ' ')" == 'smcli smcli-2-b smcli-service smcli-service-2-b ' ]]
atdir="$(mktemp -d /tmp/smcli-at.XXXXXX)"
touch "$atdir/smcli-at-probe42.txt"
mkdir "$atdir/smcli-at-dir"
[[ "$(cd "$atdir" && "$bin" complete --word @probe42)" == smcli-at-probe42.txt ]]
[[ "$(cd "$atdir" && "$bin" complete --word @at-dir --command cd)" == smcli-at-dir/ ]]
[[ -z "$(cd "$atdir" && "$bin" complete --word @probe42 --command cd)" ]]
rm -rf "$atdir"
# A selection rejected by both command and query is counted only once.
prunedb="$home/prune.db"
prunecfg="$home/prune.json"
printf '%s\n' '{"historyFilter":["^secret"]}' > "$prunecfg"
HOME="$home" SMCLI_DB="$prunedb" SMCLI_SERVICE=0 "$bin" record bash session /tmp 0 1700000000 '' '' 'secret-command'
python3 - "$prunedb" <<'PY'
import sqlite3, sys
with sqlite3.connect(sys.argv[1]) as db:
    db.execute("INSERT INTO command_selections(query,command,kind,directory,selected_at) VALUES('secret-query','secret-command','search','/tmp',1)")
PY
[[ "$(HOME="$home" SMCLI_DB="$prunedb" SMCLI_CONFIG="$prunecfg" SMCLI_SERVICE=0 "$bin" prune --dry-run)" == *'1 commands and 1 selections would be removed.' ]]
[[ "$(HOME="$home" SMCLI_DB="$prunedb" SMCLI_CONFIG="$prunecfg" SMCLI_SERVICE=0 "$bin" prune)" == *'Removed 1 commands and 1 selections.' ]]
cmp "$home/bash-history.before" "$home/.bash_history"
cmp "$home/zsh-history.before" "$home/.zsh_history"
printf 'smcli CLI smoke tests passed.\n'
