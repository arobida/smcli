use anyhow::{Result, bail};
use smcli_core::config::SmcliConfig;

/// Render hooks from the saved key config. Configs written before `rightKey` existed used
/// `leftKey` to open directory suggestions on an empty line; that feature now lives on Right,
/// so an unset `rightKey` falls back to `leftKey` rather than silently disabling it.
pub fn render_config(shell: &str, binary_path: &str, config: &SmcliConfig) -> Result<String> {
    render_with_keys(
        shell,
        binary_path,
        config.ctrl_r_key.unwrap_or(true),
        config.left_key.unwrap_or(false),
        config.right_key.or(config.left_key).unwrap_or(false),
        config.ctrl_t_key.unwrap_or(true),
        config.ctrl_g_key.unwrap_or(true),
    )
}

pub fn render_with_keys(
    shell: &str,
    binary_path: &str,
    ctrl_r: bool,
    left: bool,
    right: bool,
    ctrl_t: bool,
    ctrl_g: bool,
) -> Result<String> {
    let binary = if shell == "fish" {
        format!(
            "'{}'",
            binary_path.replace('\\', "\\\\").replace('\'', "\\'")
        )
    } else {
        format!("'{}'", binary_path.replace('\'', "'\\''"))
    };
    let template = match shell {
        "bash" => {
            r###"# smcli: opt-in metadata capture. Native HISTFILE is left untouched. Records commands Bash retains in history;
# pipeline status is unavailable. In PS0/bash-preexec mode, an unchanged history number is ambiguous with
# ignoredups, so when HISTCONTROL/HISTIGNORE exclusions are configured that command is conservatively omitted.
# HISTIGNORE's bare & pattern is not interpreted. Hook: bash-preexec when loaded, else PS0 (Bash 4.4+),
# else a DEBUG trap (SMCLI_BASH_HOOK=debug forces it).
if [[ ${_smcli_installed-} != 1 ]]; then
    _smcli_mode=debug
    if [[ ${SMCLI_BASH_HOOK-} != debug ]]; then
        if [[ -n ${bash_preexec_imported-}${__bp_imported-} ]] && declare -p preexec_functions precmd_functions &>/dev/null; then
            _smcli_mode=bash-preexec
        elif (( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 4) )); then
            _smcli_mode=ps0
        fi
    fi
    if [[ $_smcli_mode == debug ]] && { [[ -n $(trap -p DEBUG) || -n $(trap -p EXIT) || $- == *T* ]] || shopt -q extdebug; }; then
        printf 'smcli: capture not installed: existing DEBUG/EXIT trap, functrace, or extdebug.\n' >&2
    else
        _smcli_bin=\#(binary)
        _smcli_installed=1
        _smcli_pending=0
        _smcli_armed=0
        _smcli_nul=
        _smcli_history_number=
        _smcli_directory=$PWD
        # Bash before 5.0 has no EPOCHREALTIME; derive whole seconds from SECONDS without forking per command.
        _smcli_epoch=$(( $(date +%s) - SECONDS ))
        _smcli_session=$$-$_smcli_epoch-$RANDOM
        # Exported so the picker and search know which session is asking (its last command picks transitions).
        export SMCLI_SESSION=$_smcli_session
        _smcli_expanded=
        # PS0 writes in a subshell; transfer text through a private, uniquely created file.
        _smcli_expdir= _smcli_expfile=
        if [[ $_smcli_mode == ps0 ]]; then
            _smcli_expdir=$(umask 077; mktemp -d "${SMCLI_RUNTIME_DIR:-${TMPDIR:-/tmp}}/smcli-exp.XXXXXXXXXX")
            if [[ -n $_smcli_expdir ]]; then
                _smcli_expfile=$(umask 077; mktemp "$_smcli_expdir/body.XXXXXXXXXX")
            fi
        fi
        # A cheap pre-check; smcli record applies the full rule (`safe_expansion`). Nothing that runs code or redirects is sent.
        _smcli_safe() {
            local LC_ALL=C s=$1
            local assign="(^|[ ({;&|\"'])[A-Za-z_][A-Za-z0-9_]*\\+?="
            [[ -n $s && ${#s} -le 8192 ]] || return 1
            [[ $s != *[^\ -~]* && $s != *'$('* && $s != *'`'* && $s != *[\<\>]* ]] && ! [[ $s =~ $assign ]]
        }
        # bash 3.2 has no BASH_ALIASES. Parse `alias --` output; never eval it.
        _smcli_alias_body() {
            local line q="'"
            line=$(alias -- "$1" 2>/dev/null) || return 0
            [[ $line == "alias $1="* ]] || return 0
            line=${line#"alias $1="}
            [[ $line == "$q"*"$q" ]] || return 0
            # Printed quote escapes are shell syntax, not the original body. Keep these typed-only.
            [[ $line != *"$q\\$q$q"* ]] || return 0
            line=${line#"$q"}
            line=${line%"$q"}
            printf '%s' "$line"
        }
        # Look up the first word now. The next prompt would see an alias the command itself redefined.
        _smcli_set_expanded() {
            local typed=$1 name body
            _smcli_expanded=
            shopt -q expand_aliases || return 0
            [[ $typed =~ ^[A-Za-z_][A-Za-z0-9_]*($|[[:space:]]) ]] || return 0
            name=${typed%%[[:space:]]*}
            if (( BASH_VERSINFO[0] >= 4 )); then
                body=${BASH_ALIASES[$name]-}
            else
                body=$(_smcli_alias_body "$name")
            fi
            [[ -n $body && $body != *' ' ]] || return 0
            _smcli_expanded=$body${typed#"$name"}
            _smcli_safe "$_smcli_expanded" && [[ $_smcli_expanded != "$typed" ]] || _smcli_expanded=
        }
        _smcli_ps0_alias() {
            local entry
            _smcli_expanded=
            entry=$(HISTTIMEFORMAT= builtin history 1 2>/dev/null) || true
            # An unchanged history number is the previous command, not this one.
            if [[ $entry =~ ^[[:space:]]*([0-9]+)[[:space:]]+(.*)$ && ${BASH_REMATCH[1]} != "$_smcli_history_number" ]]; then
                _smcli_set_expanded "${BASH_REMATCH[2]}"
            fi
            [[ -n $_smcli_expfile ]] || return 0
            (umask 077; printf '%s\n' "$_smcli_expanded" >| "$_smcli_expfile") 2>/dev/null || true
        }
        _smcli_record() {
            # --expanded precedes the existing positional fields and is omitted without a trusted expansion.
            local -a args
            args=(record)
            _smcli_safe "${_smcli_expanded-}" && args+=(--expanded "$_smcli_expanded")
            args+=(bash "$_smcli_session" "$_smcli_directory" "$1" "$_smcli_started" "$2" "${HISTFILE-}" "$_smcli_command")
            "$_smcli_bin" "${args[@]}" || printf 'smcli: command metadata was not saved.\n' >&2
        }
        # Whether $1 matches one of the user's HISTIGNORE patterns (a bare & means "the previous line": not checked).
        _smcli_histignore() {
            local IFS=: pattern
            local -a patterns
            read -ra patterns <<<"${HISTIGNORE-}"
            for pattern in "${patterns[@]}"; do
                [[ -n $pattern && $pattern != '&' && $1 == $pattern ]] && return 0
            done
            return 1
        }
        # PS0 and bash-preexec don't pass the typed text, so take it from Bash's own history. That also means
        # a line Bash kept out of history (HISTCONTROL=ignorespace/ignoreboth, HISTIGNORE) is never recorded.
        _smcli_history_command() {
            local entry number
            shopt -oq history || return 1
            entry=$(HISTTIMEFORMAT= builtin history 1)
            [[ $entry =~ ^[[:space:]]*([0-9]+)[[:space:]]+(.*)$ ]] || return 1
            number=${BASH_REMATCH[1]}
            _smcli_command=${BASH_REMATCH[2]}
            if [[ $number == "$_smcli_history_number" ]]; then
                # No new entry: ignoredups merged it (record it), or an exclusion dropped it (cannot tell which
                # when one is configured, so leave it out).
                if [[ $HISTCONTROL == *ignorespace* || $HISTCONTROL == *ignoreboth* || -n ${HISTIGNORE-} ]]; then
                    return 1
                fi
            fi
            _smcli_history_number=$number
        }
        _smcli_preexec() {
            local prior_status=$2 entry
            if [[ $_smcli_armed == 1 && $1 != _smcli_prompt && $1 != _smcli_ready && $1 != _smcli_pick && $1 != _smcli_pick_left && $1 != _smcli_pick_right && $1 != _smcli_pick_files && $1 != _smcli_pick_grep ]]; then
                _smcli_started=${EPOCHREALTIME:-$((_smcli_epoch + SECONDS))}
                _smcli_armed=0
                _smcli_pending=1
                _smcli_directory=$PWD
                _smcli_command=$1
                entry=$(HISTTIMEFORMAT= builtin history 1)
                if [[ $entry =~ ^[[:space:]]*([0-9]+)[[:space:]]+(.*)$ ]]; then
                    if [[ ${BASH_REMATCH[1]} != "$_smcli_history_number" ]]; then
                        _smcli_command=${BASH_REMATCH[2]}
                    elif [[ -n ${HISTIGNORE-} ]] && _smcli_histignore "$1"; then
                        # HISTIGNORE kept the command out of history; a leading space makes smcli skip it too.
                        _smcli_command=" $1"
                    elif [[ ${BASH_REMATCH[2]} == *"$1"* ]]; then
                        # A duplicate bash left out of history (ignoredups): the full line is the previous entry.
                        _smcli_command=${BASH_REMATCH[2]}
                    elif [[ $HISTCONTROL == *ignorespace* || $HISTCONTROL == *ignoreboth* ]]; then
                        # Bash kept a leading-space command out of history; restore the space so smcli skips it too.
                        _smcli_command=" $1"
                    fi
                    _smcli_history_number=${BASH_REMATCH[1]}
                fi
                _smcli_set_expanded "$_smcli_command"
            fi
            return "$prior_status"
        }
        # bash-preexec calls this before each command (PS0 sets the same state inline).
        _smcli_bp_preexec() {
            _smcli_pending=1
            _smcli_directory=$PWD
            if [[ -n ${EPOCHREALTIME-} ]]; then _smcli_t0=${EPOCHREALTIME/[.,]/}; else _smcli_t0=$(( (_smcli_epoch + SECONDS) * 1000000 )); fi
            _smcli_set_expanded "$1"
        }
        _smcli_prompt() {
            local status=$? ended=${EPOCHREALTIME:-$((_smcli_epoch + SECONDS))} entry keep=1
            _smcli_armed=0
            if [[ $_smcli_pending == 1 ]]; then
                _smcli_pending=0
                if [[ $_smcli_mode == ps0 && -n ${_smcli_expfile-} ]]; then
                    _smcli_expanded=
                    [[ -s $_smcli_expfile ]] && IFS= read -r _smcli_expanded < "$_smcli_expfile"
                    : >| "$_smcli_expfile"
                fi
                if [[ $_smcli_mode != debug ]]; then
                    printf -v _smcli_started '%d.%06d' $((_smcli_t0 / 1000000)) $((_smcli_t0 % 1000000))
                    _smcli_history_command || keep=
                fi
                if [[ -n $keep ]]; then
                    export SMCLI_DB="${SMCLI_DB:-$HOME/.local/share/smcli/history.db}"
                    # Record in a detached process so the prompt never waits; at exit, finish first.
                    if [[ -n ${SMCLI_SYNC-} || ${1-} == exit ]]; then
                        _smcli_record "$status" "$ended"
                    else
                        ( _smcli_record "$status" "$ended" & )
                    fi
                fi
            else
                entry=$(HISTTIMEFORMAT= builtin history 1)
                if [[ $entry =~ ^[[:space:]]*([0-9]+)[[:space:]] ]]; then
                    _smcli_history_number=${BASH_REMATCH[1]}
                fi
            fi
            return "$status"
        }
        _smcli_ready() { local status=$?; _smcli_armed=1; _smcli_directory=$PWD; return "$status"; }
        # Runs _smcli_prompt first on exit, then whatever EXIT trap was there.
        _smcli_exit_trap() {
            local existing
            existing=$(trap -p EXIT)
            if [[ -n $existing ]]; then
                eval "set -- $existing"
                existing=$3
            fi
            trap '_smcli_prompt exit; [[ -z $_smcli_expfile ]] || rm -f -- "$_smcli_expfile"; [[ -z $_smcli_expdir ]] || rmdir -- "$_smcli_expdir";'"${existing:+ $existing}" EXIT
        }
        case $_smcli_mode in
            bash-preexec)
                precmd_functions=(_smcli_prompt "${precmd_functions[@]}")
                preexec_functions+=(_smcli_bp_preexec)
                _smcli_exit_trap
                ;;
            *)
                if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == 'declare -a '* ]]; then
                    PROMPT_COMMAND=(_smcli_prompt "${PROMPT_COMMAND[@]}" _smcli_ready)
                else
                    PROMPT_COMMAND="_smcli_prompt${PROMPT_COMMAND:+; $PROMPT_COMMAND}; _smcli_ready"
                fi
                if [[ $_smcli_mode == ps0 ]]; then
                    # Arithmetic in a parameter expansion runs in this shell, so PS0 can set state before the command runs.
                    if [[ -n ${EPOCHREALTIME-} ]]; then
                        PS0+='${_smcli_nul:$((_smcli_pending=1,_smcli_t0=${EPOCHREALTIME/[.,]/},0)):0}'
                    else
                        PS0+='${_smcli_nul:$((_smcli_pending=1,_smcli_t0=(_smcli_epoch+SECONDS)*1000000,0)):0}'
                    fi
                    # Length 0, so nothing is printed; the function writes the expansion before the command runs.
                    PS0+='${_smcli_nul:0:$(_smcli_ps0_alias; printf 0)}'
                    _smcli_exit_trap
                else
                    trap '_smcli_preexec "$BASH_COMMAND" "$?"' DEBUG
                    trap '_smcli_prompt exit' EXIT
                fi
                ;;
        esac
    fi
fi
# smcli: history picker on Up and Ctrl-R, for keys no other tool has bound. Set SMCLI_NO_KEYS=1 before this line to
# keep readline's keys; SMCLI_EXTRA_KEYS=1 adds Left and Right on an empty line, Ctrl-Right and Alt-;.
if [[ $- == *i* && -z ${SMCLI_NO_KEYS-} ]] && (( BASH_VERSINFO[0] < 4 )); then
    # Warn once per database, not on every shell start.
    _smcli_dir=${SMCLI_DB:-$HOME/.local/share/smcli/history.db}; _smcli_dir=${_smcli_dir%/*}
    if [[ ! -e $_smcli_dir/.bash3-warned ]]; then
        echo "smcli: picker key bindings require Bash 4+; use 'smcli pick' or upgrade Bash (history recording remains active)." >&2
        mkdir -p "$_smcli_dir" && : > "$_smcli_dir/.bash3-warned"
    fi
    unset _smcli_dir
elif [[ $- == *i* && -z ${SMCLI_NO_KEYS-} ]]; then
    _smcli_pick_bin=\#(binary)
    # Bash 4 counts READLINE_POINT in bytes (5.0 and later in characters) while ${var:offset} counts characters,
    # so on Bash 4 slice and measure in the C locale. The local LC_ALL ends with the function.
    _smcli_split() {
        (( BASH_VERSINFO[0] >= 5 )) || local LC_ALL=C
        _smcli_before=${READLINE_LINE:0:READLINE_POINT} _smcli_after=${READLINE_LINE:READLINE_POINT} _smcli_bytes=${#1}
    }
    _smcli_pick() {
        local chosen code
        _smcli_split
        chosen=$(SMCLI_SHELL=bash SMCLI_LBUFFER=$_smcli_before "$_smcli_pick_bin" pick "$@" -- "$READLINE_LINE" </dev/tty)
        code=$?
        if (( code == 0 )); then
            READLINE_LINE=$chosen
            _smcli_split "$chosen"
            READLINE_POINT=$_smcli_bytes
        elif (( code == 10 )); then
            # A file or grep result: one quoted word, inserted at the cursor. Never run.
            _smcli_split "$chosen"
            READLINE_LINE=$_smcli_before$chosen\ $_smcli_after
            READLINE_POINT=$(( READLINE_POINT + _smcli_bytes + 1 ))
        fi
        unset _smcli_before _smcli_after _smcli_bytes
    }
    # Left opens history search on an empty line (the same picker as Ctrl-R); otherwise it moves the cursor.
    _smcli_pick_left() {
        if [[ -z $READLINE_LINE ]]; then _smcli_pick
        elif (( READLINE_POINT > 0 )); then READLINE_POINT=$((READLINE_POINT - 1)); fi
    }
    # Right opens directory suggestions on an empty line; otherwise it moves the cursor.
    _smcli_pick_right() {
        if [[ -z $READLINE_LINE ]]; then _smcli_pick --suggest
        elif (( READLINE_POINT < ${#READLINE_LINE} )); then READLINE_POINT=$((READLINE_POINT + 1)); fi
    }
    # Ctrl-T opens the picker on this directory's files; Ctrl-A toggles whole-project search.
    _smcli_pick_files() { _smcli_pick --files; }
    # Ctrl-G searches file contents.
    _smcli_pick_grep() { _smcli_pick --grep; }
    # Bind $1 to function $2 unless something other than readline's own $3 (or nothing) holds it.
    _smcli_bind() {
        local marker=$'\n'"\"$1\": " current= word free=
        if [[ $_smcli_maps == *"$marker"* ]]; then
            current=${_smcli_maps#*"$marker"}
            current=${current%%$'\n'*}
        fi
        for word in "" $3 "\"$2\""; do [[ $current == "$word" ]] && free=1; done
        if [[ -n $free ]]; then bind -x "\"$1\": $2"; else _smcli_kept+=" $1"; fi
    }
    # Functions, macros and shell commands bound to keys; `bind -X` prints `"keys" "command"` without the colon.
    _smcli_maps=$(bind -X 2>/dev/null)
    _smcli_maps=$'\n'$(bind -p 2>/dev/null; bind -s 2>/dev/null)$'\n'${_smcli_maps//\" \"/\": \"}
    _smcli_kept=
    # From `smcli init`: Ctrl-R, and Left and Right on an empty line.
    (( \#(keyR) )) && _smcli_bind '\C-r' _smcli_pick 'reverse-search-history history-search-backward'
    _smcli_bind '\e[A' _smcli_pick 'previous-history history-search-backward'
    _smcli_bind '\eOA' _smcli_pick 'previous-history history-search-backward'
    if (( \#(keyT) )); then
        _smcli_bind '\C-t' _smcli_pick_files transpose-chars
        # BSD and macOS ttys send Ctrl-T to the kernel as STATUS (load report) before readline sees it.
        [[ $_smcli_kept == *'\C-t'* ]] || stty status undef 2>/dev/null
    fi
    (( \#(keyG) )) && _smcli_bind '\C-g' _smcli_pick_grep abort
    if [[ -n ${SMCLI_EXTRA_KEYS-} ]] || (( \#(keyLeft) )); then
        _smcli_bind '\e[D' _smcli_pick_left backward-char
        _smcli_bind '\eOD' _smcli_pick_left backward-char
    fi
    if [[ -n ${SMCLI_EXTRA_KEYS-} ]] || (( \#(keyRight) )); then
        _smcli_bind '\e[C' _smcli_pick_right forward-char
        _smcli_bind '\eOC' _smcli_pick_right forward-char
    fi
    if [[ -n ${SMCLI_EXTRA_KEYS-} ]]; then
        _smcli_bind '\e[1;5C' _smcli_pick forward-word
        _smcli_bind '\e[5C' _smcli_pick forward-word
        _smcli_bind '\e;' _smcli_pick ''
    fi
    # Told once per change, not in every new shell.
    _smcli_seen="${XDG_STATE_HOME:-$HOME/.local/state}/smcli/kept-keys-bash"
    if [[ $_smcli_kept != "$(cat "$_smcli_seen" 2>/dev/null)" ]]; then
        [[ -z $_smcli_kept ]] || printf 'smcli: kept existing key bindings:%s (bind -x them to _smcli_pick to switch).\n' "$_smcli_kept" >&2
        mkdir -p "${_smcli_seen%/*}" 2>/dev/null && printf '%s' "$_smcli_kept" >| "$_smcli_seen" 2>/dev/null
    fi
    unset _smcli_seen
    unset _smcli_maps
fi
# smcli: @ path completion. Typed text is unchanged unless completion is invoked.
if [[ $- == *i* && -z ${SMCLI_NO_KEYS-} ]]; then
    _smcli_complete_bin=\#(binary)
    if (( BASH_VERSINFO[0] >= 4 )); then
        _smcli_tab_orig=complete
        while IFS= read -r _smcli_bind_line; do
            [[ $_smcli_bind_line == '"\t": '* ]] || continue
            _smcli_tab_orig=${_smcli_bind_line#'"\t": '}
            _smcli_tab_orig=${_smcli_tab_orig%.}
            break
        done < <(bind -p 2>/dev/null)
        _smcli_tab() {
            (( BASH_VERSINFO[0] >= 5 )) || local LC_ALL=C
            local before=${READLINE_LINE:0:READLINE_POINT} after=${READLINE_LINE:READLINE_POINT}
            local word=${before##*[[:space:]]} cmd=${READLINE_LINE%%[[:space:]]*} hit
            if [[ $word == @* ]]; then
                hit=$(SMCLI_SHELL=bash "$_smcli_complete_bin" complete --word "$word" --command "$cmd" --limit 1)
                if [[ -n $hit ]]; then
                    READLINE_LINE=${before%"$word"}$hit$after
                    READLINE_POINT=$((${#before} - ${#word} + ${#hit}))
                fi
                bind '"\e[0n": redraw-current-line'
            else
                bind "\"\e[0n\": $_smcli_tab_orig"
            fi
        }
        bind -x '"\C-x@": _smcli_tab'
        bind '"\t": "\C-x@\e[0n"'
    else
        _smcli_complete() {
            local cur=${COMP_WORDS[COMP_CWORD]} cmd=${COMP_WORDS[0]} line
            COMPREPLY=()
            [[ $cur == @* ]] || return
            while IFS= read -r line; do
                [[ -n $line ]] && COMPREPLY+=("$line")
            done < <(SMCLI_SHELL=bash "$_smcli_complete_bin" complete --word "$cur" --command "$cmd")
        }
        complete -o bashdefault -o default -o nospace -F _smcli_complete printf echo cd pushd z cat npm true 2>/dev/null || true
    fi
fi"###
        }
        "zsh" => {
            r###"# smcli: opt-in metadata capture. Native HISTFILE is left untouched. HISTORY_IGNORE is honored; other
# zshaddhistory hooks may reject commands without smcli knowing. Pipeline statuses are recorded.
if [[ ${_smcli_installed-} != 1 ]]; then
    zmodload -F zsh/datetime p:EPOCHREALTIME p:EPOCHSECONDS 2>/dev/null
    _smcli_bin=\#(binary)
    _smcli_installed=1
    _smcli_pending=0
    _smcli_session=$$-${EPOCHSECONDS:-0}-$RANDOM
    export SMCLI_SESSION=$_smcli_session
    _smcli_expanded=
    _smcli_executed=
    # A cheap pre-check; smcli record applies the full rule (`safe_expansion`). Nothing that runs code or redirects is sent.
    _smcli_safe() {
        local s=$1
        (( $#s > 0 && $#s <= 8192 )) || return 1
        [[ $s != *[^\ -~]* && $s != *'$('* && $s != *'`'* && $s != *[\<\>]* ]] || return 1
        setopt localoptions norematchpcre
        ! [[ $s =~ "(^|[ ({;&|\"'])[A-Za-z_][A-Za-z0-9_]*\\+?=" ]]
    }
    # $1 is preexec's typed text, $2 is preexec's $3. Only a confirmed leading regular alias is kept.
    _smcli_alias() {
        local typed=$1 executed=$2 name body expanded
        [[ $typed == [A-Za-z_]* ]] || return
        name=${typed%% *}
        [[ $name != *[^A-Za-z0-9_]* && ( $typed == "$name" || $typed == "$name "* ) ]] || return
        [[ -n ${aliases[$name]-} && -z ${galiases[$name]-} && -z ${saliases[$name]-} ]] || return
        body=${aliases[$name]}
        [[ $body == *' ' ]] && return
        expanded=$body${typed#$name}
        # $3 must be that expansion: a differing format is not proof, and a suppressed alias must not be labeled.
        [[ $executed == "$expanded" ]] || return
        _smcli_safe "$expanded" || return
        [[ $expanded != "$typed" ]] || return
        _smcli_expanded=$expanded
    }
    _smcli_start() {
        _smcli_pending=0
        _smcli_expanded=
        _smcli_executed=
        # HISTORY_IGNORE is the user's own list of commands to keep out of history.
        if [[ -n ${HISTORY_IGNORE-} && $1 == ${~HISTORY_IGNORE} ]]; then return; fi
        _smcli_started=${EPOCHREALTIME-}
        _smcli_command=$1
        _smcli_executed=$3
        _smcli_directory=$PWD
        _smcli_pending=1
        _smcli_alias "$1" "$3"
    }
    _smcli_record() {
        local -a args
        args=(record)
        _smcli_safe "${_smcli_expanded-}" && args+=(--expanded "$_smcli_expanded")
        args+=(zsh "$_smcli_session" "$_smcli_directory" "$1" "$3" "$_smcli_started" "$2" "${HISTFILE-}" "$_smcli_command")
        "$_smcli_bin" "${args[@]}" || printf 'smcli: command metadata was not saved.\n' >&2
    }
    _smcli_finish() {
        local exit_code=$1 pipe=$2 ended=${EPOCHREALTIME-}
        if [[ $_smcli_pending == 1 ]]; then
            _smcli_pending=0
            export SMCLI_DB="${SMCLI_DB:-$HOME/.local/share/smcli/history.db}"
            # Record in a disowned process so the prompt never waits; at exit, finish first.
            if [[ -n ${SMCLI_SYNC-} || ${3-} == exit ]]; then
                _smcli_record "$exit_code" "$ended" "$pipe"
            else
                _smcli_record "$exit_code" "$ended" "$pipe" &!
            fi
        fi
        return "$exit_code"
    }
    # Hook arrays rather than precmd()/zshexit() themselves, so later definitions of those can't replace ours.
    # Zsh restores $? and $pipestatus before each hook; ours goes first to read them unaltered.
    _smcli_precmd() {
        local exit_code=$? pipe="${pipestatus[*]}"
        _smcli_finish "$exit_code" "$pipe"
    }
    _smcli_zshexit() {
        local exit_code=$? pipe="${pipestatus[*]}"
        _smcli_finish "$exit_code" "$pipe" exit
    }
    autoload -Uz add-zsh-hook
    add-zsh-hook preexec _smcli_start
    add-zsh-hook precmd _smcli_precmd
    add-zsh-hook zshexit _smcli_zshexit
    precmd_functions=(_smcli_precmd ${precmd_functions:#_smcli_precmd})
    zshexit_functions=(_smcli_zshexit ${zshexit_functions:#_smcli_zshexit})
fi
# smcli: history picker on Up and Ctrl-R, for keys no other tool has bound. Set SMCLI_NO_KEYS=1 before this line to
# keep zle's keys; SMCLI_EXTRA_KEYS=1 adds Left and Right on an empty line, Ctrl-Right and Alt-;.
if [[ -o interactive && -z ${SMCLI_NO_KEYS-} ]]; then
    _smcli_pick_bin=\#(binary)
    _smcli_pick() {
        local chosen code
        chosen=$(env SMCLI_SHELL=zsh SMCLI_LBUFFER=$LBUFFER "$_smcli_pick_bin" pick "$@" -- "$BUFFER" </dev/tty)
        code=$?
        if (( code == 0 )); then
            BUFFER=$chosen
            CURSOR=$#BUFFER
        elif (( code == 10 )); then
            # A file or grep result: one quoted word, inserted at the cursor. Never run.
            LBUFFER+="$chosen "
        fi
        zle reset-prompt
    }
    # Multiline buffers keep Up for moving between lines.
    _smcli_pick_up() {
        if [[ $BUFFER == *$'\n'* ]]; then zle up-line-or-history; else _smcli_pick; fi
    }
    zle -N _smcli_pick
    # Left opens history search on an empty line (the same picker as Ctrl-R); otherwise it moves the cursor.
    _smcli_pick_left() {
        if [[ -z $BUFFER ]]; then _smcli_pick; else zle backward-char; fi
    }
    # Right opens directory suggestions on an empty line; otherwise it moves the cursor.
    _smcli_pick_right() {
        if [[ -z $BUFFER ]]; then _smcli_pick --suggest; else zle forward-char; fi
    }
    zle -N _smcli_pick_up
    zle -N _smcli_pick_left
    zle -N _smcli_pick_right
    # Ctrl-T opens the picker on this directory's files; Ctrl-A toggles whole-project search.
    _smcli_pick_files() { _smcli_pick --files; }
    zle -N _smcli_pick_files
    # Ctrl-G searches file contents.
    _smcli_pick_grep() { _smcli_pick --grep; }
    zle -N _smcli_pick_grep
    # Bind $2 in keymap $1 to widget $3 unless something other than zle's own $4 (or nothing) holds it.
    _smcli_bind() {
        local current=${${(z)"$(bindkey -M $1 "$2" 2>/dev/null)"}[-1]}
        if [[ -z $current || $current == (undefined-key|_smcli_pick*|${~4}) ]]; then
            bindkey -M $1 "$2" $3
        else
            _smcli_kept+=("$2 ($current)")
        fi
    }
    _smcli_kept=()
    for _smcli_keymap in emacs viins; do
        # From `smcli init`: Ctrl-R, and Left and Right on an empty line.
        (( \#(keyR) )) && _smcli_bind $_smcli_keymap '^R' _smcli_pick 'redisplay|history-incremental-(pattern-|)search-backward|history-search-backward'
        _smcli_bind $_smcli_keymap '^[[A' _smcli_pick_up 'up-line-or-(history|beginning-search)'
        _smcli_bind $_smcli_keymap '^[OA' _smcli_pick_up 'up-line-or-(history|beginning-search)'
        (( \#(keyT) )) && _smcli_bind $_smcli_keymap '^T' _smcli_pick_files 'transpose-chars|self-insert'
        (( \#(keyG) )) && _smcli_bind $_smcli_keymap '^G' _smcli_pick_grep 'send-break|list-expand|self-insert'
        if [[ -n ${SMCLI_EXTRA_KEYS-} ]] || (( \#(keyLeft) )); then
            _smcli_bind $_smcli_keymap '^[[D' _smcli_pick_left 'backward-char|vi-backward-char'
            _smcli_bind $_smcli_keymap '^[OD' _smcli_pick_left 'backward-char|vi-backward-char'
        fi
        if [[ -n ${SMCLI_EXTRA_KEYS-} ]] || (( \#(keyRight) )); then
            _smcli_bind $_smcli_keymap '^[[C' _smcli_pick_right 'forward-char|vi-forward-char'
            _smcli_bind $_smcli_keymap '^[OC' _smcli_pick_right 'forward-char|vi-forward-char'
        fi
        if [[ -n ${SMCLI_EXTRA_KEYS-} ]]; then
            _smcli_bind $_smcli_keymap '^[[1;5C' _smcli_pick forward-word
            _smcli_bind $_smcli_keymap '^[[5C' _smcli_pick forward-word
            _smcli_bind $_smcli_keymap '^[;' _smcli_pick 'undefined-key'
        fi
    done
    # BSD and macOS ttys send Ctrl-T to the kernel as STATUS (load report) before zle sees it.
    (( \#(keyT) )) && [[ ${(j: :)_smcli_kept} != *'^T ('* ]] && stty status undef 2>/dev/null
    # Told once per change, not in every new shell.
    typeset _smcli_seen=${XDG_STATE_HOME:-$HOME/.local/state}/smcli/kept-keys-zsh _smcli_list=${(j:, :)${(u)_smcli_kept}}
    if [[ $_smcli_list != "$(cat $_smcli_seen 2>/dev/null)" ]]; then
        (( $#_smcli_kept )) && print -u2 "smcli: kept existing key bindings: $_smcli_list (bindkey them to _smcli_pick to switch)."
        mkdir -p ${_smcli_seen:h} 2>/dev/null && print -rn -- $_smcli_list >| $_smcli_seen 2>/dev/null
    fi
    unset _smcli_keymap _smcli_kept _smcli_seen _smcli_list
fi
# smcli: @ path completion. Typed text is unchanged unless completion is invoked.
if [[ -o interactive && -z ${SMCLI_NO_KEYS-} ]]; then
    _smcli_complete_bin=\#(binary)
    if [[ -z ${_smcli_tab_orig-} || $_smcli_tab_orig == _smcli_complete_widget ]]; then
        _smcli_tab_orig=${${(z)"$(bindkey '^I')"}[-1]}
        [[ -n $_smcli_tab_orig && $_smcli_tab_orig != (undefined-key|_smcli_complete_widget) ]] || _smcli_tab_orig=expand-or-complete
    fi
    _smcli_complete_widget() {
        setopt localoptions noglob noshwordsplit
        local word=${LBUFFER##*[[:space:]]} cmd=${${(z)BUFFER}[1]}
        if [[ $word != @* ]]; then
            zle "$_smcli_tab_orig"
            return
        fi
        local -a hits
        hits=("${(@f)${"$(SMCLI_SHELL=zsh "$_smcli_complete_bin" complete --word "$word" --command "$cmd" 2>/dev/null)"}}")
        if (( $#hits == 1 )) && [[ -n $hits[1] ]]; then
            LBUFFER="${LBUFFER%$word}${hits[1]}"
        elif (( $#hits > 1 )); then
            # Several matches: pick one in the files picker, filtered by the word.
            local chosen
            chosen=$(env SMCLI_SHELL=zsh SMCLI_LBUFFER=${LBUFFER%$word} SMCLI_FILES_QUERY=${word#@} SMCLI_FILES_COMMAND=$cmd ${${(M)cmd:#(cd|pushd|z)}:+SMCLI_FILES_DIRS=1} "$_smcli_complete_bin" pick --files </dev/tty)
            (( $? == 10 )) && LBUFFER="${LBUFFER%$word}$chosen"
            zle reset-prompt
        fi
    }
    zle -N _smcli_complete_widget
    # Deferred plugins (fzf-tab, zsh-defer) rebind Tab after init: take it back before each prompt, chaining to theirs.
    _smcli_tab_rebind() {
        local bound=${${(z)"$(bindkey '^I')"}[-1]}
        [[ $bound == _smcli_complete_widget ]] && return
        [[ -n $bound && $bound != undefined-key ]] && _smcli_tab_orig=$bound
        bindkey '^I' _smcli_complete_widget
        bindkey -M viins '^I' _smcli_complete_widget 2>/dev/null || true
    }
    _smcli_tab_rebind
    autoload -Uz add-zsh-hook
    add-zsh-hook precmd _smcli_tab_rebind
fi"###
        }
        "fish" => {
            r###"# smcli: opt-in metadata capture. Native history is left untouched. fish_should_add_to_history and private mode
# exclusions are honored; pipeline statuses are recorded. Start times have whole-second precision.
if not set -q _smcli_installed
    set -g _smcli_installed 1
    set -g _smcli_bin \#(fishBinary)
    set -g _smcli_session $fish_pid-(date +%s)-(random)
    set -gx SMCLI_SESSION $_smcli_session
    set -g _smcli_pending 0
    set -g _smcli_expanded
    # A cheap pre-check; smcli record applies the full rule (`safe_expansion`). Nothing that runs code or redirects is sent.
    function _smcli_safe
        set -l s $argv[1]
        set -l n (string length -- $s)
        test $n -gt 0; and test $n -le 8192; or return 1
        not string match -qr -- '[^ -~]|\$\(|`|[<>]' $s; or return 1
        not string match -qr -- '(^|[ ({;&|"\'])[A-Za-z_][A-Za-z0-9_]*\\+?=' $s
    end
    # A generated alias function is one literal command forwarding $argv. Anything else, including abbreviations, is typed text.
    function _smcli_alias_body
        set -l name $argv[1]
        functions -q -- $name; or return 0
        set -l cmd
        for line in (functions -- $name)
            set -l trimmed (string trim -- $line)
            if test -z "$trimmed"
                continue
            end
            switch $trimmed
                case '#*' 'function *' end
                    continue
            end
            if test -n "$cmd"
                return 0
            end
            set cmd $trimmed
        end
        set cmd (string trim -- (string replace -r -- ';$' '' $cmd))
        # [$] is a literal dollar: fish single quotes do not treat backslash as an escape.
        string match -qr -- '^[^ ].* [$]argv$' $cmd; or return 0
        string replace -r -- ' [$]argv$' '' $cmd
    end
    function _smcli_set_expanded
        set -g _smcli_expanded
        set -l m (string match -r -- '^([A-Za-z_][A-Za-z0-9_]*)( .*)?$' $argv[1])
        or return 0
        set -l body (_smcli_alias_body $m[2])
        test -n "$body"; or return 0
        set -l suffix ""
        test (count $m) -ge 3; and set suffix $m[3]
        set -l expanded "$body$suffix"
        _smcli_safe "$expanded"; or return 0
        test "$expanded" != "$argv[1]"; or return 0
        set -g _smcli_expanded $expanded
    end
    # Runs before each command: fish's private mode and fish_should_add_to_history keep a command out, as does smcli's own
    # filter for a leading space. Abbreviations are already expanded in the buffer, so they are recorded as typed text.
    function _smcli_preexec --on-event fish_preexec
        set -g _smcli_pending 0
        set -g _smcli_expanded
        test -n "$fish_private_mode"; and return
        if functions -q fish_should_add_to_history
            fish_should_add_to_history "$argv[1]"; or return
        end
        set -g _smcli_command "$argv[1]"
        set -g _smcli_directory $PWD
        # Fish has no clock builtin, so start times are whole seconds; the duration is fish's own CMD_DURATION.
        set -g _smcli_started (date +%s)
        set -g _smcli_pending 1
        _smcli_set_expanded "$argv[1]"
    end
    function _smcli_record
        set -l args record
        if _smcli_safe "$_smcli_expanded"
            set -a args --expanded $_smcli_expanded
        end
        set -a args fish $_smcli_session $_smcli_directory $argv[1] $argv[2] $argv[3] $argv[4] '' $_smcli_command
        # Record in a disowned process so the prompt never waits.
        if test -n "$SMCLI_SYNC"
            command $_smcli_bin $args; or echo 'smcli: command metadata was not saved.' >&2
        else
            begin
                command $_smcli_bin $args; or echo 'smcli: command metadata was not saved.' >&2
            end &
            disown
        end
    end
    function _smcli_postexec --on-event fish_postexec
        set -l exit_code $status
        set -l pipe $pipestatus
        test "$_smcli_pending" = 1; or return
        set -g _smcli_pending 0
        _smcli_record $exit_code "$pipe" $_smcli_started (math $_smcli_started + $CMD_DURATION / 1000)
    end
end
# smcli: history picker on Up and Ctrl-R, for keys you haven't bound yourself. Set SMCLI_NO_KEYS=1 before this line to
# keep fish's keys. Selections are put on the command line, never run; autosuggestions are left alone.
if status is-interactive; and test -z "$SMCLI_NO_KEYS"
    function _smcli_pick
        # The text left of the cursor, so smcli can refuse to insert a path inside an open quote.
        set -l before (commandline -c | string collect)
        set -lx SMCLI_LBUFFER "$before"
        set -l chosen (SMCLI_SHELL=fish command $_smcli_bin pick $argv -- (commandline | string collect) </dev/tty)
        set -l code $status
        if test $code -eq 0
            commandline -r -- (string join \n -- $chosen | string collect)
            commandline -C 999999
        else if test $code -eq 10
            # A file or grep result: one quoted word, inserted at the cursor. Never run.
            commandline -i -- (string join \n -- $chosen | string collect)" "
        end
        commandline -f repaint
    end
    # Up moves within a multiline command or an open pager instead.
    function _smcli_pick_up
        if commandline --paging-mode; or test (count (commandline)) -gt 1
            commandline -f up-or-search
        else
            _smcli_pick
        end
    end
    set -g _smcli_kept
    # Bind $argv[1] to function $argv[2] unless the user has already bound that key to something else.
    function _smcli_bind
        set -l current (bind --user $argv[1] 2>/dev/null)
        if test -z "$current"; or string match -q '*_smcli_pick*' -- $current
            bind $argv[1] $argv[2]
        else
            set -a _smcli_kept (string escape -- $argv[1])
        end
    end
    # Left opens history search on an empty line. Right stays fish's key (it accepts an autosuggestion).
    function _smcli_pick_left
        if test -z (commandline | string collect)
            _smcli_pick
        else
            commandline -f backward-char
        end
    end
    # Ctrl-T opens the picker on this directory's files; Ctrl-A toggles whole-project search.
    function _smcli_pick_files
        _smcli_pick --files
    end
    # Ctrl-G searches file contents.
    function _smcli_pick_grep
        _smcli_pick --grep
    end
    # From `smcli init`: Ctrl-R, and Left on an empty line.
    test \#(keyR) = 1; and _smcli_bind \cr _smcli_pick
    test \#(keyT) = 1; and _smcli_bind \ct _smcli_pick_files
    test \#(keyG) = 1; and _smcli_bind \cg _smcli_pick_grep
    _smcli_bind \e\[A _smcli_pick_up
    _smcli_bind \eOA _smcli_pick_up
    if test \#(keyLeft) = 1
        _smcli_bind \e\[D _smcli_pick_left
        _smcli_bind \eOD _smcli_pick_left
    end
    # Told once per change, not in every new shell.
    set -l _smcli_seen (set -q XDG_STATE_HOME; and echo $XDG_STATE_HOME; or echo $HOME/.local/state)/smcli/kept-keys-fish
    if test "$_smcli_kept" != "$(cat $_smcli_seen 2>/dev/null)"
        test (count $_smcli_kept) -eq 0; or echo "smcli: kept existing key bindings: $_smcli_kept (bind them to _smcli_pick to switch)." >&2
        mkdir -p (dirname $_smcli_seen) 2>/dev/null; and printf '%s' "$_smcli_kept" > $_smcli_seen 2>/dev/null
    end
    functions -e _smcli_bind
    set -e _smcli_kept
end
# smcli: @ path completion. Typed text is unchanged unless completion is invoked.
if status is-interactive; and test -z "$SMCLI_NO_KEYS"
    set -g _smcli_complete_bin \#(fishBinary)
    function _smcli_complete_widget
        set -l token (commandline -ct)
        if string match -q -- '@*' -- $token
            set -l cmd (commandline -op)
            set -l hits (SMCLI_SHELL=fish command $_smcli_complete_bin complete --word "$token" --command $cmd[1])
            test (count $hits) -eq 1; and commandline -t -- $hits[1]
        else
            commandline -f complete
        end
    end
    bind \t _smcli_complete_widget
end"###
        }
        _ => bail!("shell-init requires bash, zsh or fish."),
    };
    // Substitute the path last: its text must never be interpreted as a template token.
    // keyRight before keyR: the shorter token is a prefix of the longer one.
    Ok(template
        .replace(r"\#(keyRight)", if right { "1" } else { "0" })
        .replace(r"\#(keyR)", if ctrl_r { "1" } else { "0" })
        .replace(r"\#(keyLeft)", if left { "1" } else { "0" })
        .replace(r"\#(keyT)", if ctrl_t { "1" } else { "0" })
        .replace(r"\#(keyG)", if ctrl_g { "1" } else { "0" })
        .replace(
            if shell == "fish" {
                r"\#(fishBinary)"
            } else {
                r"\#(binary)"
            },
            &binary,
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_completion_hooks_call_smcli_complete() {
        for shell in ["bash", "zsh", "fish"] {
            let hooks = render_with_keys(shell, "smcli", true, false, false, false, false).unwrap();
            assert!(hooks.contains("complete --word"), "{shell}");
            assert!(hooks.contains("_smcli_complete"), "{shell}");
            assert!(hooks.contains("SMCLI_NO_KEYS"), "{shell}");
        }
    }

    #[test]
    fn record_hooks_pass_optional_expanded_flag() {
        for shell in ["bash", "zsh", "fish"] {
            let hooks = render_with_keys(shell, "smcli", true, false, false, false, false).unwrap();
            assert!(hooks.contains("--expanded"), "{shell} missing --expanded");
            assert!(hooks.contains("8192"), "{shell} missing the 8192-byte gate");
        }
        let bash = render_with_keys("bash", "smcli", true, false, false, false, false).unwrap();
        assert!(bash.contains("BASH_ALIASES") && bash.contains("alias --"));
        let zsh = render_with_keys("zsh", "smcli", true, false, false, false, false).unwrap();
        assert!(zsh.contains("_smcli_executed=$3") && zsh.contains("_smcli_alias \"$1\" \"$3\""));
        let fish = render_with_keys("fish", "smcli", true, false, false, false, false).unwrap();
        assert!(fish.contains("[$]argv") && !fish.contains("abbr -"));
    }

    #[test]
    fn quotes_paths_and_preserves_template_like_path_text() {
        let path = "a'\\b\\#(keyR)";
        assert!(
            render_config("zsh", path, &SmcliConfig::default())
                .unwrap()
                .contains("_smcli_bin='a'\\''\\b\\#(keyR)'")
        );
        assert!(
            render_config("fish", path, &SmcliConfig::default())
                .unwrap()
                .contains("set -g _smcli_bin 'a\\'\\\\b\\\\#(keyR)'")
        );
        let hooks = render_with_keys("zsh", "smcli", false, true, false, false, false).unwrap();
        assert!(hooks.contains("(( 0 )) && _smcli_bind $_smcli_keymap '^R'"));
        assert!(hooks.contains("|| (( 1 )); then"));
        assert!(render_config("unknown", path, &SmcliConfig::default()).is_err());
    }

    #[test]
    fn empty_prompt_arrows_open_history_and_suggestions() {
        let bash = render_with_keys("bash", "smcli", true, true, true, true, true).unwrap();
        assert!(bash.contains("(( 1 )) && _smcli_bind '\\C-r' _smcli_pick "));
        assert!(bash.contains("if (( 1 )); then\n        _smcli_bind '\\C-t' _smcli_pick_files"));
        assert!(bash.contains("if [[ -z $READLINE_LINE ]]; then _smcli_pick\n"));
        assert!(bash.contains("READLINE_POINT=$((READLINE_POINT - 1))"));
        assert!(bash.contains("if [[ -z $READLINE_LINE ]]; then _smcli_pick --suggest\n"));
        assert!(bash.contains("READLINE_POINT=$((READLINE_POINT + 1))"));
        assert!(bash.contains("|| (( 1 )); then\n        _smcli_bind '\\e[D' _smcli_pick_left"));
        assert!(bash.contains("|| (( 1 )); then\n        _smcli_bind '\\e[C' _smcli_pick_right"));
        assert!(!bash.contains("#(key"));
        assert!(!bash.contains("ight)"));

        let zsh = render_with_keys("zsh", "smcli", true, true, true, true, true).unwrap();
        assert!(zsh.contains("(( 1 )) && _smcli_bind $_smcli_keymap '^R' _smcli_pick "));
        assert!(zsh.contains("(( 1 )) && _smcli_bind $_smcli_keymap '^T' _smcli_pick_files"));
        assert!(zsh.contains("if [[ -z $BUFFER ]]; then _smcli_pick; else zle backward-char; fi"));
        assert!(zsh.contains("if [[ -z $BUFFER ]]; then _smcli_pick --suggest; else zle forward-char; fi"));
        assert!(zsh.contains("|| (( 1 )); then\n            _smcli_bind $_smcli_keymap '^[[D' _smcli_pick_left"));
        assert!(zsh.contains("|| (( 1 )); then\n            _smcli_bind $_smcli_keymap '^[[C' _smcli_pick_right"));
        assert!(zsh.contains("'backward-char|vi-backward-char'"));
        assert!(zsh.contains("'forward-char|vi-forward-char'"));

        let off = render_with_keys("zsh", "smcli", true, false, false, true, true).unwrap();
        assert!(off.contains("(( 1 )) && _smcli_bind $_smcli_keymap '^R'"));
        assert!(off.contains("(( 1 )) && _smcli_bind $_smcli_keymap '^T'"));
        assert!(off.contains("|| (( 0 )); then\n            _smcli_bind $_smcli_keymap '^[[D'"));
        assert!(off.contains("|| (( 0 )); then\n            _smcli_bind $_smcli_keymap '^[[C'"));

        let right_only = render_with_keys("bash", "smcli", false, false, true, false, false).unwrap();
        assert!(right_only.contains("(( 0 )) && _smcli_bind '\\C-r'"));
        assert!(right_only.contains("if (( 0 )); then\n        _smcli_bind '\\C-t' _smcli_pick_files"));
        assert!(right_only.contains("|| (( 0 )); then\n        _smcli_bind '\\e[D'"));
        assert!(right_only.contains("|| (( 1 )); then\n        _smcli_bind '\\e[C'"));

        let fish = render_with_keys("fish", "smcli", true, true, true, true, true).unwrap();
        assert!(fish.contains("test 1 = 1; and _smcli_bind \\cr _smcli_pick"));
        assert!(fish.contains("test 1 = 1; and _smcli_bind \\ct _smcli_pick_files"));
        assert!(!fish.contains("_smcli_pick_right"));
        assert!(!fish.contains("--suggest"));
        assert!(fish.contains("_smcli_pick\n        else\n            commandline -f backward-char"));
    }

    #[test]
    fn legacy_left_key_still_binds_right_suggestions() {
        // Pre-`rightKey` configs used `leftKey` for directory suggestions on an empty line.
        let legacy = SmcliConfig { ctrl_r_key: Some(true), left_key: Some(true), ..Default::default() };
        let zsh = render_config("zsh", "smcli", &legacy).unwrap();
        assert!(zsh.contains("if [[ -z $BUFFER ]]; then _smcli_pick; else zle backward-char; fi"));
        assert!(zsh.contains("if [[ -z $BUFFER ]]; then _smcli_pick --suggest; else zle forward-char; fi"));
        assert!(zsh.contains("|| (( 1 )); then\n            _smcli_bind $_smcli_keymap '^[[C' _smcli_pick_right"));

        // An explicit `rightKey: false` still wins over the legacy fallback.
        let off = SmcliConfig { left_key: Some(true), right_key: Some(false), ..Default::default() };
        let zsh = render_config("zsh", "smcli", &off).unwrap();
        assert!(zsh.contains("|| (( 0 )); then\n            _smcli_bind $_smcli_keymap '^[[C' _smcli_pick_right"));
    }
}
