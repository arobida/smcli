#!/bin/bash
# Removes smcli completely: exports your recorded history first, stops the service, removes the shell hooks, the
# executables, the database, search index, settings and service socket. Native Bash, Zsh and Fish history is never
# touched. Installed as `smcli-uninstall` next to smcli.
#
# Usage: smcli-uninstall [--export DIR] [--no-export] [--keep-data] [--prefix DIR] [--yes]
#   --export DIR   where the export goes (default ~/smcli-export-<date>)
#   --no-export    delete the history without exporting it
#   --keep-data    remove the program and hooks but keep the history, index and settings
#   --prefix DIR   where smcli is installed (default: this script's directory, else ~/.local/bin)
#   --yes          don't ask for confirmation
set -euo pipefail
umask 077

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
prefix=""
export_dir="$HOME/smcli-export-$(date +%Y%m%d-%H%M%S)"
no_export=""
keep_data=""
yes=""
while (( $# )); do
    case $1 in
        --export) export_dir=${2:?--export needs a directory}; shift 2 ;;
        --no-export) no_export=1; shift ;;
        --keep-data) keep_data=1; shift ;;
        --prefix) prefix=${2:?--prefix needs a directory}; shift 2 ;;
        --yes|-y) yes=1; shift ;;
        -h|--help) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) printf 'smcli-uninstall: unknown option %s (see --help)\n' "$1" >&2; exit 2 ;;
    esac
done

say() { printf '==> %s\n' "$*"; }
die() { printf 'smcli-uninstall: %s\n' "$*" >&2; exit 1; }

if [[ -z $prefix ]]; then
    if [[ -x $here/smcli ]]; then prefix=$here; else prefix="$HOME/.local/bin"; fi
fi
smcli="$prefix/smcli"
# Explicit SMCLI_DB is used as given (no canonicalize). Default matches Path::join (no // if HOME ends in /).
if [[ -n ${SMCLI_DB:-} ]]; then db=$SMCLI_DB; else db="${HOME%/}/.local/share/smcli/history.db"; fi
config="${SMCLI_CONFIG:-${XDG_CONFIG_HOME:-$HOME/.config}/smcli/config.json}"
begin='# >>> smcli >>>'
end='# <<< smcli <<<'
rc_files=("${ZDOTDIR:-$HOME}/.zshrc" "$HOME/.bashrc" "${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish")

printf 'This removes smcli from %s and the smcli lines from your shell startup files.\n' "$prefix"
if [[ -z $keep_data ]]; then
    printf 'It also deletes %s (with its search index) and %s.\n' "$db" "$(dirname "$config")"
    [[ -n $no_export ]] && printf 'Your recorded history will NOT be exported.\n' || printf 'History is exported to %s first.\n' "$export_dir"
fi
if [[ -z $yes ]]; then
    [[ -t 0 ]] || die "not a terminal; pass --yes to confirm"
    read -r -p "Continue? [y/N] " answer
    [[ $answer == [yY]* ]] || { echo "Nothing changed."; exit 1; }
fi

# 1. Export, before anything is removed. A failed export stops the uninstall.
if [[ -z $keep_data && -z $no_export && -f $db ]]; then
    command -v sqlite3 > /dev/null || die "sqlite3 is needed to export history; install it or pass --no-export"
    mkdir -p "$export_dir"
    chmod 700 "$export_dir"
    # A consistent copy of the live database (including recent writes still in its WAL): restorable with SMCLI_DB.
    sqlite3 "$db" ".backup '${export_dir//\'/\'\'}/history.db'" || die "could not copy $db; nothing removed"
    # Readable JSON of every recorded command with its directory, exit code, times and session.
    if [[ -x $smcli ]]; then
        SMCLI_SERVICE=off SMCLI_DB="$db" "$smcli" history --json > "$export_dir/history.json" \
            || die "could not export history as JSON; nothing removed (the copy is in $export_dir)"
    fi
    [[ -f $config ]] && cp "$config" "$export_dir/config.json"
    for file in "$export_dir/history.db" "$export_dir/history.json" "$export_dir/config.json"; do
        if [[ -f $file ]]; then chmod 600 "$file"; fi
    done
    say "Exported history to $export_dir"
fi

# 2. Stop the service (it would otherwise keep the database open until it idles out).
[[ -x $smcli ]] && SMCLI_DB="$db" "$smcli" service stop > /dev/null 2>&1 || true

validate_block() {  # $1: file
    [[ -f $1 ]] || return 0
    local state=closed line
    while IFS= read -r line || [[ -n $line ]]; do
        if [[ $line == "$begin" ]]; then
            [[ $state == closed ]] || return 1
            state=open
        elif [[ $line == "$end" ]]; then
            [[ $state == open ]] || return 1
            state=closed
        fi
    done < "$1"
    [[ $state == closed ]]
}

# Resolve symlinks so rename replaces the target, not the link itself.
resolve_rc() {
    local path=$1 link count=0
    while [[ -L $path ]]; do
        (( count += 1 ))
        (( count <= 40 )) || return 1
        link=$(readlink "$path") || return 1
        if [[ $link == /* ]]; then path=$link; else path="$(dirname "$path")/$link"; fi
    done
    printf '%s\n' "$path"
}
remove_hook() (
    local target tmp
    target=$(resolve_rc "$1") || exit 1
    if [[ -f $target ]]; then
        local links
        if [[ $(uname) == Darwin ]]; then
            links=$(stat -f %l "$target") || exit 1
        else
            links=$(stat -c %h "$target") || exit 1
        fi
        if [[ $links != 1 ]]; then
            printf 'refusing to replace hard-linked startup file %s\n' "$target" >&2
            exit 1
        fi
    fi
    tmp=$(mktemp "$target.smcli.XXXXXX") || exit 1
    trap 'rm -f "$tmp"' EXIT
    cp -p "$target" "$tmp" || exit 1
    awk -v b="$begin" -v e="$end" '$0 == b {skip = 1; next} skip && $0 == e {skip = 0; next} !skip' "$target" > "$tmp" || exit 1
    mv -f "$tmp" "$target"
)

# 3. Shell hooks: only the marked block install.sh wrote; the rest of each file is kept as is.
for file in "${rc_files[@]}"; do
    [[ -f $file ]] && grep -qxF "$begin" "$file" || continue
    validate_block "$file" || die "malformed smcli block in $file; not modifying"
    remove_hook "$file" || die "could not update $file; not modifying"
    rm -f "$file.smcli-backup"
    say "Removed the smcli hooks from $file"
done
for file in "${rc_files[@]}" "$HOME/.bash_profile" "$HOME/.profile"; do
    [[ -f $file ]] && grep -q 'shell-init' "$file" && grep -q smcli "$file" \
        && printf 'note: %s still mentions smcli shell-init (added by hand?); remove that line yourself.\n' "$file" >&2
done

# 4. Data: database (with its WAL and shared-memory files), search index, settings, service socket.
if [[ -z $keep_data ]]; then
    rm -rf "$db" "$db-wal" "$db-shm" "$db-journal" "$db.search" "$db.v7.bak" "$db.migration.lock"
    rmdir "$(dirname "$db")" 2>/dev/null || true
    rm -f "$config"
    rmdir "$(dirname "$config")" 2>/dev/null || true
    # Same FNV-1a name as service_location.rs (bytes, LC_ALL=C): only this database's socket/lock.
    service_fp=$(export LC_ALL=C; h=$((0xcbf29ce484222325)); t=$db
        for ((i = 0; i < ${#t}; i++)); do printf -v c '%d' "'${t:i:1}"; h=$(( (h ^ c) * 0x100000001b3 )); done
        printf '%016x' "$h")
    # Absolute dirs only, same precedence filters as ServiceLocation::new.
    runtime_dirs=()
    if [[ ${SMCLI_RUNTIME_DIR:-} == /* ]]; then runtime_dirs+=("$SMCLI_RUNTIME_DIR")
    elif [[ ${XDG_RUNTIME_DIR:-} == /* ]]; then runtime_dirs+=("$XDG_RUNTIME_DIR/smcli")
    else
        if [[ ${TMPDIR:-} == /* ]]; then runtime_dirs+=("$TMPDIR/smcli-$(id -u)")
        else runtime_dirs+=("/tmp/smcli-$(id -u)")
        fi
    fi
    for runtime in "${runtime_dirs[@]}"; do
        if [[ -d $runtime ]]; then
            rm -f "$runtime/service-${service_fp}.sock" "$runtime/service-${service_fp}.lock"
            rmdir "$runtime" 2>/dev/null || true
        fi
    done
    say "Deleted smcli's history, search index and settings"
fi

# 5. The executables, this script last.
rm -f "$prefix/smcli" "$prefix/smcli-service"
rm -rf "$prefix/potion-shell-16m"
say "Removed smcli. Open a new terminal (the current one keeps the old hooks until it exits)."
rm -f "$prefix/smcli-uninstall"
