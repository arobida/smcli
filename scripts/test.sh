#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

swift="${SWIFT:-/usr/bin/swift}"
"$swift" test
bin_dir="$("$swift" build --show-bin-path)"
home="$(mktemp -d)"
trap 'rm -rf "$home"' EXIT
export SMCLI_DB="$home/captured.db"

output="$("$bin_dir/smcli")"
[[ "$output" == 'Usage: smcli init'* ]]
[[ "$(HOME="$home" HISTFILE='' "$bin_dir/smcli" history)" == 'No saved Bash or Zsh history found.' ]]
printf '#1700000000\necho bash\n' > "$home/.bash_history"
printf ': 1700000000:0;echo zsh\n' > "$home/.zsh_history"
output="$(HOME="$home" HISTFILE='' "$bin_dir/smcli" history)"
expected="$(printf '==> %s/.bash_history <==\necho bash\n==> %s/.zsh_history <==\necho zsh' "$home" "$home")"
[[ "$output" == "$expected" ]]

if HOME="$home" HISTFILE="$home/missing" "$bin_dir/smcli" history > "$home/output" 2> "$home/error"; then
    printf 'Missing custom history should fail.\n' >&2
    exit 1
fi
[[ ! -s "$home/output" && -s "$home/error" ]]
if "$bin_dir/smcli" unknown > "$home/output" 2> "$home/error"; then
    printf 'Unknown command should fail.\n' >&2
    exit 1
fi
[[ "$(HOME="$home" HISTFILE='' "$bin_dir/smcli" search echo zsh -n 1)" == 'echo zsh'* ]]
python3 scripts/test-shells.py "$bin_dir/smcli"
python3 scripts/bench-baseline.py --binary "$bin_dir/smcli" --rows 10 --samples 1 --output "$home/baseline.json"
printf 'smcli history tests passed.\n'
