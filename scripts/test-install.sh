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

printf 'alias ll="ls -l"\n' > "$home/zshrc-target"
chmod 640 "$home/zshrc-target"
ln -s zshrc-target "$home/.zshrc"
for _ in 1 2; do  # twice: a rerun replaces the block rather than adding another
    bash scripts/install.sh --from "$from" --prefix "$home/bin" --shells zsh,bash,fish --keys left --yes > "$home/out" 2>&1 \
        || { cat "$home/out"; fail "install failed"; }
done
[[ -L $home/.zshrc ]] || fail "install replaced startup symlink"
[[ $(stat -f %Lp "$home/zshrc-target") == 640 ]] || fail "install changed startup permissions"
# An incomplete model is rejected before either existing executable is replaced.
mkdir -p "$home/incomplete/potion-shell-16m"
cp "$from/smcli" "$home/incomplete/smcli"
cp "$from/smcli-service" "$home/incomplete/smcli-service"
printf x > "$home/incomplete/potion-shell-16m/model.safetensors"
cp "$home/bin/smcli" "$home/old-smcli"
cp "$home/bin/smcli-service" "$home/old-service"
bash scripts/install.sh --from "$home/incomplete" --prefix "$home/bin" --shells none --yes > "$home/out" 2>&1 \
    && fail "install should reject incomplete model"
cmp "$home/old-smcli" "$home/bin/smcli" || fail "invalid model replaced smcli"
cmp "$home/old-service" "$home/bin/smcli-service" || fail "invalid model replaced service"
# A failed model copy must leave both old executables and the old model untouched.
mkdir "$home/fail-tools"
real_cp=$(command -v cp)
cat > "$home/fail-tools/cp" <<EOF
#!/bin/bash
case "\$*" in *potion-shell-16m*) exit 1 ;; esac
exec "$real_cp" "\$@"
EOF
chmod +x "$home/fail-tools/cp"
cp "$home/bin/smcli" "$home/old-smcli"
cp "$home/bin/smcli-service" "$home/old-service"
PATH="$home/fail-tools:$PATH" bash scripts/install.sh --from "$from" --prefix "$home/bin" --shells none --yes > "$home/out" 2>&1 \
    && fail "install should fail when staging model fails"
cmp "$home/old-smcli" "$home/bin/smcli" || fail "failed staging replaced smcli"
cmp "$home/old-service" "$home/bin/smcli-service" || fail "failed staging replaced service"
[[ -f $home/bin/potion-shell-16m/model.safetensors ]] || fail "failed staging removed model"
rm "$home/fail-tools/cp"
# A failed filter must not truncate the symlink target on either install or uninstall.
printf '#!/bin/bash\nexit 1\n' > "$home/fail-tools/awk"
chmod +x "$home/fail-tools/awk"
cp "$home/zshrc-target" "$home/old-rc"
PATH="$home/fail-tools:$PATH" bash scripts/install.sh --from "$from" --prefix "$home/bin" --shells zsh --yes > "$home/out" 2>&1 \
    && fail "install should fail when filtering fails"
cmp "$home/old-rc" "$home/zshrc-target" || fail "failed install damaged startup file"
PATH="$home/fail-tools:$PATH" bash "$home/bin/smcli-uninstall" --keep-data --yes > "$home/out" 2>&1 \
    && fail "uninstall should fail when filtering fails"
cmp "$home/old-rc" "$home/zshrc-target" || fail "failed uninstall damaged startup file"
rm "$home/fail-tools/awk"
# Refuse hard-linked startup files, including through a symlink, without detaching either name.
ln "$home/zshrc-target" "$home/zshrc-hardlink"
cp "$home/zshrc-target" "$home/old-rc"
for operation in install uninstall; do
    if [[ $operation == install ]]; then
        bash scripts/install.sh --from "$from" --prefix "$home/bin" --shells zsh --yes > "$home/out" 2>&1 \
            && fail "install should refuse hard-linked startup file"
    else
        bash "$home/bin/smcli-uninstall" --keep-data --yes > "$home/out" 2>&1 \
            && fail "uninstall should refuse hard-linked startup file"
    fi
    grep -q 'hard-linked startup file' "$home/out" || fail "$operation missing hard-link diagnostic"
    [[ -L $home/.zshrc && $home/zshrc-target -ef $home/zshrc-hardlink ]] || fail "$operation detached hard links"
    cmp "$home/old-rc" "$home/zshrc-target" || fail "$operation modified hard-linked startup file"
done
rm "$home/zshrc-hardlink"
# --atuin yes imports a found Atuin database; the import itself is covered by the Rust tests.
mkdir -p "$home/.local/share/atuin"
sqlite3 "$home/.local/share/atuin/history.db" "CREATE TABLE history (id TEXT PRIMARY KEY, timestamp INTEGER NOT NULL, duration INTEGER NOT NULL, exit INTEGER NOT NULL, command TEXT NOT NULL, cwd TEXT NOT NULL, session TEXT NOT NULL, hostname TEXT NOT NULL, deleted_at INTEGER); INSERT INTO history VALUES ('a1', 1700000000000000000, 5000000, 0, 'echo from-atuin', '/tmp', 'sess', 'host:user', NULL);"
HOME="$home" bash scripts/install.sh --from "$from" --prefix "$home/bin" --shells none --keys left --atuin yes --yes > "$home/out" 2>&1 \
    || { cat "$home/out"; fail "install with --atuin yes failed"; }
HOME="$home" SMCLI_SERVICE=off "$home/bin/smcli" history | grep -q 'echo from-atuin' || { cat "$home/out"; fail "Atuin history not imported"; }
for f in smcli smcli-service smcli-uninstall; do [[ -x $home/bin/$f ]] || fail "$f not installed"; done
[[ -f $home/bin/potion-shell-16m/model.safetensors ]] || fail "model not installed"
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
python3 - "$home/export/history.json" <<'PY'
import json
import sys
with open(sys.argv[1]) as f:
    records = json.load(f)
rec = next(r for r in records if r["command"] == "echo exported-marker")
assert rec["directory"] == "/tmp", "directory"
assert rec["exitCode"] == 0, "exitCode"
assert rec["session"] == "s1", "session"
assert rec["shell"] == "zsh", "shell"
assert isinstance(rec["timestamp"], str) and rec["timestamp"], "timestamp"
# Optional metadata is present only when captured; when present it keeps its type.
assert isinstance(rec.get("flags", []), list) and isinstance(rec.get("pipestatus", []), list), rec
PY
[[ $(sqlite3 "$home/export/history.db" "SELECT count(*) FROM commands WHERE command = 'echo exported-marker'") == 1 ]] \
    || fail "database copy is missing the recorded command"
[[ -f $home/export/config.json ]] || fail "settings not exported"
for f in bin/smcli bin/smcli-service bin/smcli-uninstall bin/potion-shell-16m .local/share/smcli .config/smcli; do
    [[ ! -e $home/$f ]] || fail "$f was not removed"
done
for f in .zshrc .bashrc .config/fish/config.fish; do
    ! grep -q smcli "$home/$f" || fail "$f still mentions smcli"
done
grep -q '^alias ll="ls -l"$' "$home/.zshrc" || fail "uninstall lost the user's own .zshrc lines"
[[ -L $home/.zshrc && $(stat -f %Lp "$home/zshrc-target") == 640 ]] || fail "uninstall changed startup symlink or permissions"
[[ ! -d $runtime || -z $(ls -A "$runtime") ]] || fail "service socket left behind: $(ls -A "$runtime")"
printf 'Install and uninstall (with export) passed.\n'
