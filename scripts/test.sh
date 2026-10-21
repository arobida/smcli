#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

swift="${SWIFT:-/usr/bin/swift}"
cargo test --workspace
cargo build --workspace
bin_dir="$(cd "${CARGO_TARGET_DIR:-target}/debug" && pwd)"
# Swift build only for the shell-init parity check below; drop both when the Swift sources go.
if [[ -x $swift ]]; then
    "$swift" build --product smcli
    swift_bin="$("$swift" build --show-bin-path)/smcli"
fi
home="$(mktemp -d)"
# The service is on by default; keep the one these checks start private, short-pathed and stopped on exit.
runtime="$(mktemp -d /tmp/smcli-test.XXXXXX)"
export SMCLI_DB="$home/captured.db" SMCLI_RUNTIME_DIR="$runtime"
trap '"$bin_dir/smcli" service stop >/dev/null 2>&1 || true; rm -rf "$home" "$runtime"' EXIT

if [[ -n ${swift_bin-} ]]; then
    # Hooks embed their executable path; run both builds at the same path for a byte-for-byte comparison.
    mkdir -p "$home/shell-parity"
    cp "$swift_bin" "$home/shell-parity/smcli"
    HOME="$home" SMCLI_CONFIG="$home/parity-config.json" "$home/shell-parity/smcli" shell-init zsh > "$home/swift-shell-init"
    cp "$bin_dir/smcli" "$home/shell-parity/smcli"
    diff "$home/swift-shell-init" <(HOME="$home" SMCLI_CONFIG="$home/parity-config.json" "$home/shell-parity/smcli" shell-init zsh)
fi

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
# init --keys sets the picker keys without asking, and shell-init follows them.
cfg="$home/keys.json"
SMCLI_CONFIG="$cfg" "$bin_dir/smcli" init --keys left > /dev/null
grep -Eq '"ctrlRKey" ?: false' "$cfg" && grep -Eq '"leftKey" ?: true' "$cfg" && grep -Eq '"ctrlTKey" ?: false' "$cfg"
init="$(SMCLI_CONFIG="$cfg" "$bin_dir/smcli" shell-init zsh)"
[[ $init == *"(( 0 )) && _smcli_bind \$_smcli_keymap '^R'"* && $init == *"|| (( 1 )); then"* ]]
[[ $init == *"(( 0 )) && _smcli_bind \$_smcli_keymap '^T'"* && $init == *"(( 0 )) && _smcli_bind \$_smcli_keymap '^G'"* ]]
SMCLI_CONFIG="$cfg" "$bin_dir/smcli" init --keys ctrl-r,left > /dev/null
grep -Eq '"ctrlRKey" ?: true' "$cfg"
if SMCLI_CONFIG="$cfg" "$bin_dir/smcli" init --keys up > /dev/null 2>&1; then
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
python3 scripts/test-shells.py "$bin_dir/smcli"
python3 scripts/test-service.py "$bin_dir/smcli"
python3 scripts/test-file-picker.py "$bin_dir/smcli"
bash scripts/test-install.sh "$bin_dir"
python3 scripts/bench-baseline.py --binary "$bin_dir/smcli" --rows 10 --samples 1 --output "$home/baseline.json"
printf 'smcli history tests passed.\n'
