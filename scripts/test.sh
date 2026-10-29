#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
# An inherited SMCLI_* or XDG_* variable can change results (SMCLI_RANKING_OFF disables the ranking floors).
unset "${!SMCLI_@}" XDG_CONFIG_HOME XDG_DATA_HOME XDG_STATE_HOME

cargo test --workspace
cargo build --workspace
bin_dir="$(cd "${CARGO_TARGET_DIR:-target}/debug" && pwd)"

# Each black-box suite is isolated (its own HOME, database, runtime directory and Unix socket), so the
# six run concurrently; a line-tagged prefix keeps the interleaved output readable.
pids=()
names=()
run() {
    local name="$1"; shift
    ( "$@" 2>&1 | while IFS= read -r line || [[ -n "$line" ]]; do printf '[%-13s] %s\n' "$name" "$line"; done ) &
    pids+=("$!")
    names+=("$name")
}
run cli          bash scripts/test-cli.sh "$bin_dir/smcli"
run shells       python3 scripts/test-shells.py "$bin_dir/smcli"
run service      python3 scripts/test-service.py "$bin_dir/smcli"
run file-picker  python3 scripts/test-file-picker.py "$bin_dir/smcli"
run alias-picker python3 scripts/test-alias-picker.py "$bin_dir/smcli"
run install      bash scripts/test-install.sh "$bin_dir"

failed=()
for i in "${!pids[@]}"; do
    wait "${pids[$i]}" || failed+=("${names[$i]}")
done
if (( ${#failed[@]} )); then
    printf '\nsmcli test suite FAILED: %s\n' "${failed[*]}" >&2
    exit 1
fi
printf 'smcli test suite passed.\n'
