import Foundation

func shellQuote(_ text: String) -> String {
    "'" + text.replacingOccurrences(of: "'", with: "'\\''") + "'"
}

func shellIntegration(shell: String, executable: String) throws -> String {
    let binary = shellQuote(executable)
    switch shell {
    case "bash":
        return #"""
        # smcli: opt-in metadata capture. Native HISTFILE is left untouched.
        if [[ ${_smcli_installed-} != 1 ]]; then
            if [[ -n $(trap -p DEBUG) || -n $(trap -p EXIT) || $- == *T* ]] || shopt -q extdebug; then
                printf 'smcli: capture not installed: existing DEBUG/EXIT trap, functrace, or extdebug.\n' >&2
            else
                _smcli_bin=\#(binary)
                _smcli_installed=1
                _smcli_pending=0
                _smcli_armed=0
                _smcli_history_number=
                # Bash before 5.0 has no EPOCHREALTIME; derive whole seconds from SECONDS without forking per command.
                _smcli_epoch=$(( $(date +%s) - SECONDS ))
                _smcli_session=$$-$_smcli_epoch-$RANDOM
                _smcli_record() {
                    "$_smcli_bin" record bash "$_smcli_session" "$_smcli_directory" "$1" "$_smcli_started" "$2" \
                        "${HISTFILE-}" "$_smcli_command" || printf 'smcli: command metadata was not saved.\n' >&2
                }
                _smcli_preexec() {
                    local prior_status=$2 entry
                    if [[ $_smcli_armed == 1 && $1 != _smcli_prompt && $1 != _smcli_ready && $1 != _smcli_pick && $1 != _smcli_pick_left ]]; then
                        _smcli_started=${EPOCHREALTIME:-$((_smcli_epoch + SECONDS))}
                        _smcli_armed=0
                        _smcli_pending=1
                        _smcli_directory=$PWD
                        _smcli_command=$1
                        entry=$(HISTTIMEFORMAT= builtin history 1)
                        if [[ $entry =~ ^[[:space:]]*([0-9]+)[[:space:]]+(.*)$ ]]; then
                            if [[ ${BASH_REMATCH[1]} != "$_smcli_history_number" ]]; then
                                _smcli_command=${BASH_REMATCH[2]}
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
                _smcli_prompt() {
                    local status=$? ended=${EPOCHREALTIME:-$((_smcli_epoch + SECONDS))}
                    _smcli_armed=0
                    if [[ $_smcli_pending == 1 ]]; then
                        _smcli_pending=0
                        export SMCLI_DB="${SMCLI_DB:-$HOME/.local/share/smcli/history.db}"
                        # Record in a detached process so the prompt never waits; at exit, finish first.
                        if [[ -n ${SMCLI_SYNC-} || ${1-} == exit ]]; then
                            _smcli_record "$status" "$ended"
                        else
                            ( _smcli_record "$status" "$ended" & )
                        fi
                    else
                        local entry
                        entry=$(HISTTIMEFORMAT= builtin history 1)
                        if [[ $entry =~ ^[[:space:]]*([0-9]+)[[:space:]] ]]; then
                            _smcli_history_number=${BASH_REMATCH[1]}
                        fi
                    fi
                    return "$status"
                }
                _smcli_ready() { local status=$?; _smcli_armed=1; return "$status"; }
                if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == 'declare -a '* ]]; then
                    PROMPT_COMMAND=(_smcli_prompt "${PROMPT_COMMAND[@]}" _smcli_ready)
                else
                    PROMPT_COMMAND="_smcli_prompt${PROMPT_COMMAND:+; $PROMPT_COMMAND}; _smcli_ready"
                fi
                trap '_smcli_preexec "$BASH_COMMAND" "$?"' DEBUG
                trap '_smcli_prompt exit' EXIT
            fi
        fi
        # smcli: history picker on Up and Ctrl-R. Set SMCLI_NO_KEYS=1 before this line to keep readline's keys.
        if [[ $- == *i* && -z ${SMCLI_NO_KEYS-} && ${_smcli_keys-} != 1 ]] && (( BASH_VERSINFO[0] >= 4 )); then
            _smcli_keys=1
            _smcli_pick_bin=\#(binary)
            _smcli_pick() {
                local chosen
                if chosen=$("$_smcli_pick_bin" pick "$READLINE_LINE" </dev/tty); then
                    READLINE_LINE=$chosen
                    READLINE_POINT=${#chosen}
                fi
            }
            bind -x '"\C-r": _smcli_pick'
            bind -x '"\e[A": _smcli_pick'
            bind -x '"\eOA": _smcli_pick'
            # Left opens the picker only on an empty line; otherwise it moves the cursor as usual.
            _smcli_pick_left() {
                if [[ -z $READLINE_LINE ]]; then _smcli_pick
                elif (( READLINE_POINT > 0 )); then READLINE_POINT=$((READLINE_POINT - 1)); fi
            }
            bind -x '"\e[D": _smcli_pick_left'
            bind -x '"\eOD": _smcli_pick_left'
            bind -x '"\e[1;5C": _smcli_pick'
            bind -x '"\e[5C": _smcli_pick'
            bind -x '"\e;": _smcli_pick'
        fi
        """#
    case "zsh":
        return #"""
        # smcli: opt-in metadata capture. Native HISTFILE is left untouched.
        if [[ ${_smcli_installed-} != 1 ]]; then
            zmodload -F zsh/datetime p:EPOCHREALTIME p:EPOCHSECONDS 2>/dev/null
            _smcli_bin=\#(binary)
            _smcli_installed=1
            _smcli_pending=0
            _smcli_session=$$-${EPOCHSECONDS:-0}-$RANDOM
            _smcli_start() {
                _smcli_started=${EPOCHREALTIME-}
                _smcli_command=$1
                _smcli_directory=$PWD
                _smcli_pending=1
            }
            _smcli_record() {
                "$_smcli_bin" record zsh "$_smcli_session" "$_smcli_directory" "$1" "$_smcli_started" "$2" \
                    "${HISTFILE-}" "$_smcli_command" || printf 'smcli: command metadata was not saved.\n' >&2
            }
            _smcli_finish() {
                local exit_code=$1 ended=${EPOCHREALTIME-}
                if [[ $_smcli_pending == 1 ]]; then
                    _smcli_pending=0
                    export SMCLI_DB="${SMCLI_DB:-$HOME/.local/share/smcli/history.db}"
                    # Record in a disowned process so the prompt never waits; at exit, finish first.
                    if [[ -n ${SMCLI_SYNC-} || ${2-} == exit ]]; then
                        _smcli_record "$exit_code" "$ended"
                    else
                        _smcli_record "$exit_code" "$ended" &!
                    fi
                fi
                return "$exit_code"
            }
            # The global precmd runs before precmd_functions; capture before either can alter $?.
            if (( $+functions[precmd] )); then
                functions[_smcli_saved_precmd]=$functions[precmd]
            fi
            precmd() {
                local exit_code=$?
                if (( $+functions[_smcli_saved_precmd] )); then
                    _smcli_finish "$exit_code"
                    _smcli_saved_precmd
                else
                    _smcli_finish "$exit_code"
                fi
            }
            if (( $+functions[zshexit] )); then
                functions[_smcli_saved_zshexit]=$functions[zshexit]
            fi
            zshexit() {
                local exit_code=$?
                if (( $+functions[_smcli_saved_zshexit] )); then
                    _smcli_finish "$exit_code" exit
                    _smcli_saved_zshexit
                else
                    _smcli_finish "$exit_code" exit
                fi
            }
            autoload -Uz add-zsh-hook
            add-zsh-hook preexec _smcli_start
        fi
        # smcli: history picker on Up and Ctrl-R. Set SMCLI_NO_KEYS=1 before this line to keep zle's keys.
        if [[ -o interactive && -z ${SMCLI_NO_KEYS-} && ${_smcli_keys-} != 1 ]]; then
            _smcli_keys=1
            _smcli_pick_bin=\#(binary)
            _smcli_pick() {
                local chosen
                if chosen=$("$_smcli_pick_bin" pick "$BUFFER" </dev/tty); then
                    BUFFER=$chosen
                    CURSOR=$#BUFFER
                fi
                zle reset-prompt
            }
            # Multiline buffers keep Up for moving between lines.
            _smcli_pick_up() {
                if [[ $BUFFER == *$'\n'* ]]; then zle up-line-or-history; else _smcli_pick; fi
            }
            zle -N _smcli_pick
            # Left opens the picker only on an empty line; otherwise it moves the cursor as usual.
            _smcli_pick_left() {
                if [[ -z $BUFFER ]]; then _smcli_pick; else zle backward-char; fi
            }
            zle -N _smcli_pick_up
            zle -N _smcli_pick_left
            for _smcli_keymap in emacs viins; do
                bindkey -M $_smcli_keymap '^R' _smcli_pick
                bindkey -M $_smcli_keymap '^[[A' _smcli_pick_up
                bindkey -M $_smcli_keymap '^[OA' _smcli_pick_up
                bindkey -M $_smcli_keymap '^[[D' _smcli_pick_left
                bindkey -M $_smcli_keymap '^[OD' _smcli_pick_left
                bindkey -M $_smcli_keymap '^[[1;5C' _smcli_pick
                bindkey -M $_smcli_keymap '^[[5C' _smcli_pick
                bindkey -M $_smcli_keymap '^[;' _smcli_pick
            done
            unset _smcli_keymap
        fi
        """#
    default:
        throw HistoryError(message: "shell-init requires bash or zsh.")
    }
}
