use anyhow::{Result, bail};

/// Render hooks with the default picker keys. The caller prints the trailing newline.
pub fn render(shell: &str, binary_path: &str) -> Result<String> {
    render_with_keys(shell, binary_path, true, false, true, true)
}

pub fn render_with_keys(
    shell: &str,
    binary_path: &str,
    ctrl_r: bool,
    left: bool,
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
            r###"# smcli: opt-in metadata capture. Native HISTFILE is left untouched.
# Hook: bash-preexec when it is loaded, else PS0 (Bash 4.4+), else a DEBUG trap (SMCLI_BASH_HOOK=debug forces it).
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
        _smcli_record() {
            "$_smcli_bin" record bash "$_smcli_session" "$_smcli_directory" "$1" "$_smcli_started" "$2" \
                "${HISTFILE-}" "$_smcli_command" || printf 'smcli: command metadata was not saved.\n' >&2
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
            if [[ $_smcli_armed == 1 && $1 != _smcli_prompt && $1 != _smcli_ready && $1 != _smcli_pick && $1 != _smcli_pick_left && $1 != _smcli_pick_files && $1 != _smcli_pick_grep ]]; then
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
            fi
            return "$prior_status"
        }
        # bash-preexec calls this before each command (PS0 sets the same state inline).
        _smcli_bp_preexec() {
            _smcli_pending=1
            _smcli_directory=$PWD
            if [[ -n ${EPOCHREALTIME-} ]]; then _smcli_t0=${EPOCHREALTIME/[.,]/}; else _smcli_t0=$(( (_smcli_epoch + SECONDS) * 1000000 )); fi
        }
        _smcli_prompt() {
            local status=$? ended=${EPOCHREALTIME:-$((_smcli_epoch + SECONDS))} entry keep=1
            _smcli_armed=0
            if [[ $_smcli_pending == 1 ]]; then
                _smcli_pending=0
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
            trap "_smcli_prompt exit${existing:+; $existing}" EXIT
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
# keep readline's keys; SMCLI_EXTRA_KEYS=1 adds Left on an empty line, Ctrl-Right and Alt-;.
if [[ $- == *i* && -z ${SMCLI_NO_KEYS-} ]] && (( BASH_VERSINFO[0] >= 4 )); then
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
        chosen=$(SMCLI_SHELL=bash SMCLI_LBUFFER=$_smcli_before "$_smcli_pick_bin" pick "$@" "$READLINE_LINE" </dev/tty)
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
    # Left opens the picker on directory suggestions only on an empty line; otherwise it moves the cursor as usual.
    _smcli_pick_left() {
        if [[ -z $READLINE_LINE ]]; then _smcli_pick --suggest
        elif (( READLINE_POINT > 0 )); then READLINE_POINT=$((READLINE_POINT - 1)); fi
    }
    # Ctrl-T opens the picker on this directory's files; Tab there searches the whole project.
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
    # From `smcli init`: Ctrl-R, and Left on an empty line.
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
    if [[ -n ${SMCLI_EXTRA_KEYS-} ]]; then
        _smcli_bind '\e[1;5C' _smcli_pick forward-word
        _smcli_bind '\e[5C' _smcli_pick forward-word
        _smcli_bind '\e;' _smcli_pick ''
    fi
    [[ -z $_smcli_kept ]] || printf 'smcli: kept existing key bindings:%s (bind -x them to _smcli_pick to switch).\n' "$_smcli_kept" >&2
    unset _smcli_maps
fi"###
        }
        "zsh" => {
            r###"# smcli: opt-in metadata capture. Native HISTFILE is left untouched.
if [[ ${_smcli_installed-} != 1 ]]; then
    zmodload -F zsh/datetime p:EPOCHREALTIME p:EPOCHSECONDS 2>/dev/null
    _smcli_bin=\#(binary)
    _smcli_installed=1
    _smcli_pending=0
    _smcli_session=$$-${EPOCHSECONDS:-0}-$RANDOM
    export SMCLI_SESSION=$_smcli_session
    _smcli_start() {
        _smcli_pending=0
        # HISTORY_IGNORE is the user's own list of commands to keep out of history.
        if [[ -n ${HISTORY_IGNORE-} && $1 == ${~HISTORY_IGNORE} ]]; then return; fi
        _smcli_started=${EPOCHREALTIME-}
        _smcli_command=$1
        _smcli_directory=$PWD
        _smcli_pending=1
    }
    _smcli_record() {
        "$_smcli_bin" record zsh "$_smcli_session" "$_smcli_directory" "$1" "$3" "$_smcli_started" "$2" \
            "${HISTFILE-}" "$_smcli_command" || printf 'smcli: command metadata was not saved.\n' >&2
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
# keep zle's keys; SMCLI_EXTRA_KEYS=1 adds Left on an empty line, Ctrl-Right and Alt-;.
if [[ -o interactive && -z ${SMCLI_NO_KEYS-} ]]; then
    _smcli_pick_bin=\#(binary)
    _smcli_pick() {
        local chosen code
        chosen=$(SMCLI_SHELL=zsh SMCLI_LBUFFER=$LBUFFER "$_smcli_pick_bin" pick "$@" "$BUFFER" </dev/tty)
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
    # Left opens the picker on directory suggestions only on an empty line; otherwise it moves the cursor as usual.
    _smcli_pick_left() {
        if [[ -z $BUFFER ]]; then _smcli_pick --suggest; else zle backward-char; fi
    }
    zle -N _smcli_pick_up
    zle -N _smcli_pick_left
    # Ctrl-T opens the picker on this directory's files; Tab there searches the whole project.
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
        # From `smcli init`: Ctrl-R, and Left on an empty line.
        (( \#(keyR) )) && _smcli_bind $_smcli_keymap '^R' _smcli_pick 'redisplay|history-incremental-(pattern-|)search-backward|history-search-backward'
        _smcli_bind $_smcli_keymap '^[[A' _smcli_pick_up 'up-line-or-(history|beginning-search)'
        _smcli_bind $_smcli_keymap '^[OA' _smcli_pick_up 'up-line-or-(history|beginning-search)'
        (( \#(keyT) )) && _smcli_bind $_smcli_keymap '^T' _smcli_pick_files 'transpose-chars|self-insert'
        (( \#(keyG) )) && _smcli_bind $_smcli_keymap '^G' _smcli_pick_grep 'send-break|list-expand|self-insert'
        if [[ -n ${SMCLI_EXTRA_KEYS-} ]] || (( \#(keyLeft) )); then
            _smcli_bind $_smcli_keymap '^[[D' _smcli_pick_left backward-char
            _smcli_bind $_smcli_keymap '^[OD' _smcli_pick_left backward-char
        fi
        if [[ -n ${SMCLI_EXTRA_KEYS-} ]]; then
            _smcli_bind $_smcli_keymap '^[[1;5C' _smcli_pick forward-word
            _smcli_bind $_smcli_keymap '^[[5C' _smcli_pick forward-word
            _smcli_bind $_smcli_keymap '^[;' _smcli_pick 'undefined-key'
        fi
    done
    # BSD and macOS ttys send Ctrl-T to the kernel as STATUS (load report) before zle sees it.
    (( \#(keyT) )) && [[ ${(j: :)_smcli_kept} != *'^T ('* ]] && stty status undef 2>/dev/null
    (( $#_smcli_kept )) && print -u2 "smcli: kept existing key bindings: ${(j:, :)${(u)_smcli_kept}} (bindkey them to _smcli_pick to switch)."
    unset _smcli_keymap _smcli_kept
fi"###
        }
        "fish" => {
            r###"# smcli: opt-in metadata capture. Native history is left untouched.
if not set -q _smcli_installed
    set -g _smcli_installed 1
    set -g _smcli_bin \#(fishBinary)
    set -g _smcli_session $fish_pid-(date +%s)-(random)
    set -gx SMCLI_SESSION $_smcli_session
    set -g _smcli_pending 0
    # Runs before each command: fish's private mode and fish_should_add_to_history keep a command out, as does smcli's own
    # filter for a leading space.
    function _smcli_preexec --on-event fish_preexec
        set -g _smcli_pending 0
        test -n "$fish_private_mode"; and return
        if functions -q fish_should_add_to_history
            fish_should_add_to_history "$argv[1]"; or return
        end
        set -g _smcli_command "$argv[1]"
        set -g _smcli_directory $PWD
        # Fish has no clock builtin, so start times are whole seconds; the duration is fish's own CMD_DURATION.
        set -g _smcli_started (date +%s)
        set -g _smcli_pending 1
    end
    function _smcli_record
        set -l args record fish $_smcli_session $_smcli_directory $argv[1] $argv[2] $argv[3] $argv[4] '' $_smcli_command
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
        set -l chosen (SMCLI_SHELL=fish command $_smcli_bin pick $argv (commandline | string collect) </dev/tty)
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
    # Left opens the picker on directory suggestions only on an empty line; otherwise it moves the cursor as usual.
    function _smcli_pick_left
        if test -z (commandline | string collect)
            _smcli_pick --suggest
        else
            commandline -f backward-char
        end
    end
    # Ctrl-T opens the picker on this directory's files; Tab there searches the whole project.
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
    test (count $_smcli_kept) -eq 0; or echo "smcli: kept existing key bindings: $_smcli_kept (bind them to _smcli_pick to switch)." >&2
    functions -e _smcli_bind
    set -e _smcli_kept
end"###
        }
        _ => bail!("shell-init requires bash, zsh or fish."),
    };
    // Substitute the path last: its text must never be interpreted as a template token.
    Ok(template
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
    fn quotes_paths_and_preserves_template_like_path_text() {
        let path = "a'\\b\\#(keyR)";
        assert!(
            render("zsh", path)
                .unwrap()
                .contains("_smcli_bin='a'\\''\\b\\#(keyR)'")
        );
        assert!(
            render("fish", path)
                .unwrap()
                .contains("set -g _smcli_bin 'a\\'\\\\b\\\\#(keyR)'")
        );
        let hooks = render_with_keys("zsh", "smcli", false, true, false, false).unwrap();
        assert!(hooks.contains("(( 0 )) && _smcli_bind $_smcli_keymap '^R'"));
        assert!(hooks.contains("|| (( 1 )); then"));
        assert!(render("unknown", path).is_err());
    }
}
