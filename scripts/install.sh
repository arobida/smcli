#!/bin/bash
# Installs smcli: both executables, the uninstaller, and the shell hooks, then chooses the picker keys and starts the
# service. Safe to rerun (upgrades in place). From a release archive it installs the binaries beside this script;
# from a source checkout it builds them first (`make release`).
#
# Usage: install.sh [--prefix DIR] [--from DIR] [--shells LIST|all|none] [--keys LIST] [--yes]
#   --prefix DIR    where the executables go (default ~/.local/bin)
#   --from DIR      a directory holding smcli and smcli-service (default: beside this script, else build)
#   --shells LIST   zsh,bash,fish to hook (default: your login shell; `all`: every one installed; `none`)
#   --keys LIST     picker keys: ctrl-r,left | ctrl-r | left | none (default: ask in a terminal, else ctrl-r)
#   --yes           don't ask anything; with no --keys, keeps the default keys (Up and Ctrl-R)
set -euo pipefail

prefix="$HOME/.local/bin"
from=""
shells=""
keys=""
yes=""
while (( $# )); do
    case $1 in
        --prefix) prefix=${2:?--prefix needs a directory}; shift 2 ;;
        --from) from=${2:?--from needs a directory}; shift 2 ;;
        --shells) shells=${2:?--shells needs a list}; shift 2 ;;
        --keys) keys=${2:?--keys needs a list}; shift 2 ;;
        --yes|-y) yes=1; shift ;;
        -h|--help) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) printf 'install.sh: unknown option %s (see --help)\n' "$1" >&2; exit 2 ;;
    esac
done

say() { printf '==> %s\n' "$*"; }
die() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

[[ $(uname) == Darwin ]] || die "smcli installs on macOS 14 or later."
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
[[ $prefix == /* ]] || prefix="$PWD/$prefix"

# 1. The executables: given, shipped beside this script, or built from the checkout around it.
if [[ -z $from ]]; then
    if [[ -x $here/smcli && -x $here/smcli-service ]]; then
        from=$here
    elif [[ -f $here/../Makefile && -f $here/../Package.swift ]]; then
        command -v cargo > /dev/null || die "building from source needs Rust (cargo); install it from https://rustup.rs"
        say "Building from source (make release)"
        make -C "$here/.." release > /dev/null
        from="$here/../dist"
    else
        die "no smcli and smcli-service beside this script; pass --from DIR"
    fi
fi
[[ -x $from/smcli && -x $from/smcli-service ]] || die "$from must hold executable smcli and smcli-service"

say "Installing smcli and smcli-service to $prefix"
mkdir -p "$prefix"
# A running service is from the old build: stop it first (the next command would replace it anyway).
[[ -x $prefix/smcli ]] && "$prefix/smcli" service stop > /dev/null 2>&1 || true
# Copy then rename, so a shell running the old binary never sees a half-written file.
for name in smcli smcli-service; do
    cp -L "$from/$name" "$prefix/.$name.new"  # -L: dist/ holds symlinks to the timestamped build
    chmod 755 "$prefix/.$name.new"
    xattr -d com.apple.quarantine "$prefix/.$name.new" 2>/dev/null || true
    mv -f "$prefix/.$name.new" "$prefix/$name"
done
# The uninstaller ships with the install, so it works without the release archive or checkout.
uninstaller="$here/uninstall.sh"
[[ -f $uninstaller ]] && install -m 755 "$uninstaller" "$prefix/smcli-uninstall"
smcli="$prefix/smcli"
"$smcli" --help > /dev/null || die "the installed smcli does not run"

# 2. Shell hooks, as a marked block at the end of each startup file (after tools that bind keys). Rerunning replaces it.
begin='# >>> smcli >>>'
end='# <<< smcli <<<'
quoted=$(printf "'%s'" "${smcli//\'/\'\\\'\'}")
rc_file() {
    case $1 in
        zsh) printf '%s\n' "${ZDOTDIR:-$HOME}/.zshrc" ;;
        bash) printf '%s\n' "$HOME/.bashrc" ;;
        fish) printf '%s\n' "${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish" ;;
    esac
}
hook_line() {
    case $1 in
        zsh|bash) printf 'eval "$(%s shell-init %s)"\n' "$quoted" "$1" ;;
        fish) printf '%s shell-init fish | source\n' "$quoted" ;;
    esac
}
remove_block() {  # $1: file
    [[ -f $1 ]] && grep -qxF "$begin" "$1" || return 0
    local tmp
    tmp=$(mktemp "$1.smcli.XXXXXX")
    awk -v b="$begin" -v e="$end" '$0 == b {skip = 1; next} skip && $0 == e {skip = 0; next} !skip' "$1" > "$tmp"
    cat "$tmp" > "$1" && rm -f "$tmp"  # rewrite in place: keeps the file's permissions and any symlink
}

case $shells in
    "") shells=$(basename "${SHELL:-zsh}") ;;
    all) shells=$(for s in zsh bash fish; do command -v "$s" > /dev/null && printf '%s,' "$s"; done) ;;
esac
hooked=()
for shell in ${shells//,/ }; do
    [[ $shell == none ]] && continue
    case $shell in zsh|bash|fish) ;; *) die "unsupported shell: $shell (zsh, bash or fish)" ;; esac
    file=$(rc_file "$shell")
    mkdir -p "$(dirname "$file")"
    [[ -f $file ]] && cp -p "$file" "$file.smcli-backup"
    remove_block "$file"
    printf '\n%s\n# Added by smcli install.sh; remove with smcli-uninstall. Keep it below other tools that bind keys.\n%s\n%s\n' \
        "$begin" "$(hook_line "$shell")" "$end" >> "$file"
    hooked+=("$shell ($file)")
    if [[ $shell == bash && -f $HOME/.bash_profile ]] && ! grep -q bashrc "$HOME/.bash_profile"; then
        printf 'note: ~/.bash_profile does not read ~/.bashrc, so login shells (the macOS default) skip the hooks;\n' >&2
        printf '      add: [[ -f ~/.bashrc ]] && . ~/.bashrc\n' >&2
    fi
done
(( ${#hooked[@]} )) && say "Shell hooks added: ${hooked[*]}"

# 3. Picker keys (and, when asked interactively, the search model).
if [[ -n $keys ]]; then
    "$smcli" init --keys "$keys"
elif [[ -z $yes && -t 0 ]]; then
    "$smcli" init
fi

# 4. The service starts on demand; start it now to check it runs.
if "$smcli" service start > /dev/null; then
    say "smcli-service is running"
else
    printf 'warning: smcli-service did not start; smcli still works, without the file and grep picker.\n' >&2
fi

case ":$PATH:" in
    *":$prefix:"*) ;;
    *) printf 'note: %s is not on your PATH; add it to use `smcli` by name.\n' "$prefix" >&2 ;;
esac
say "Done. Open a new terminal to load the hooks. Uninstall with: $prefix/smcli-uninstall"
