# Design

## Context

The Zsh hook reads `local exit_code=$? pipe="${pipestatus[*]}"` first thing in precmd and passes `pipe` as the optional argument after the exit code (`smcli record` takes nine arguments without it, ten with it). The Bash hook (`_smcli_prompt`) starts with `local status=$? ...` and calls `_smcli_record "$status" "$ended"`. Its install order puts `_smcli_prompt` first in `PROMPT_COMMAND` (PS0 and DEBUG modes) and first in `precmd_functions` (bash-preexec mode).

## Goals / Non-Goals

**Goals:** Bash records the same `pipestatus` field Zsh and Fish do, through the existing record argument.

**Non-Goals:** Changing how pipeline status is stored or ranked; changing Zsh or Fish.

## Decisions

**Read `PIPESTATUS` in the same `local` statement as `$?`.** Words of one `local` command are expanded before it runs, so `local status=$? pipe="${PIPESTATUS[*]}"` sees the user's pipeline in PS0 and DEBUG modes, where `_smcli_prompt` is the first `PROMPT_COMMAND` entry. A separate statement would see `local`'s own status. The rest of `PROMPT_COMMAND` is unaffected because `PIPESTATUS` there already reflects whatever ran before it.

**bash-preexec: use its saved copy or nothing.** bash-preexec runs other commands before precmd functions, so `PIPESTATUS` is already overwritten there. Newer bash-preexec saves it as `BP_PIPESTATUS`; use it when set, otherwise pass no pipeline status. The minimum bash-preexec version that sets it is confirmed during implementation and named in the header note.

**Pass it like Zsh.** `_smcli_record` adds the pipeline argument between the exit code and the start time when non-empty, keeping the nine-argument form when there is none.

**EXIT trap.** `_smcli_prompt exit` reads `PIPESTATUS` the same way; if a test shows the trap does not see the last pipeline, it passes none (allowed by the spec).

## Risks / Trade-offs

- [A user's `PROMPT_COMMAND` reorders entries after smcli is sourced] → status read then reflects their command; same limitation `$?` already has today, documented in the header note.
- [bash-preexec versions differ] → fall back to no pipeline status, never a wrong one.
