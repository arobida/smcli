# Design

## Context

The command picker already exists: `smcli pick --tldr` reads `SMCLI_LBUFFER` (line context) and `SMCLI_TLDR_QUERY`, ranks index rows and prints `leading + command`. Today only the `@@` Tab path calls it (`_smcli_tldr_split` in each shell template). Ctrl-T and Ctrl-G show the pattern for a key: a `_smcli_pick_*` function, a `_smcli_bind` guarded by `\#(keyX)`, a `ctrl_x_key` config field, and `init --keys` support.

## Goals / Non-Goals

**Goals:** one key per shell that runs the existing `pick --tldr` with the current left-of-cursor line as context.

**Non-Goals:** a new picker mode or UI; switching into the command picker from inside the history/files picker (Ctrl-O there is Backspace, and the tldr mode needs its own rows and ranker); new index or service behavior.

## Decisions

- **Reuse `pick --tldr`, not a new subcommand.** Hook passes `SMCLI_LBUFFER=<left of cursor>` and an empty `SMCLI_TLDR_QUERY`; output replaces the line, cursor at end. Each shell's `@@` branch already does this replacement, so the new function copies that call minus the `@@` split. Alternative (`pick --commands` alias) adds a flag for nothing.
- **Line text right of the cursor is dropped**, as the `@@` path does today, since the result is a whole command.
- **Backspace guard.** Before binding, check `stty -a` erase; if it is `^H`, skip and record Ctrl-O as kept. Otherwise `_smcli_bind` treats the shell's default (`backward-delete-char` in zsh/bash, `backward-delete-char` in fish) as replaceable, like `transpose-chars` for Ctrl-T. Alternative (another key) rejected: the user asked for Ctrl-O.
- **Config `ctrlOKey`, default true**, mirroring `ctrlGKey`: `SmcliConfig.ctrl_o_key`, `render_with_keys` gains a `ctrl_h` arg and `\#(keyO)` placeholder.

## Risks / Trade-offs

- [Terminals that send `^H` for Backspace while erase is `^?`] → rare misconfiguration; user turns it off with `smcli init --keys`; documented in install.md.
- [Ctrl-O also used by some users for delete-word muscle memory] → default-delete bindings are replaced; any custom binding is kept.

## Migration Plan

New shells pick up the key after `make install`/`smcli init`; existing configs without `ctrlOKey` get the default (on). Rollback: `--keys` without `ctrl-o`.

## Revision: Ctrl-H to Ctrl-O

Ctrl-H was bound but never reached the shell in the user's terminal (taken by the terminal and by zsh-autopair's delete). The key moved to Ctrl-O, whose shell defaults are rarely used; the Backspace/erase guard was dropped and replaced by clearing the BSD tty DISCARD character, mirroring Ctrl-T's STATUS.
