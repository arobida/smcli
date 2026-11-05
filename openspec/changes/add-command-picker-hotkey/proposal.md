# Proposal

## Why

The command-example picker (`smcli pick --tldr`) is only reachable by typing `@@` and pressing Tab. History has Ctrl-R, files Ctrl-T and grep Ctrl-G; the command index should get its own key, Ctrl-H, so it opens the same way.

## What Changes

- Ctrl-H in bash, zsh and fish opens the existing command picker (no new picker UI). The current line is its context: a line naming a program (`tar `, `sudo git `) opens that program's examples; an empty line opens all installed examples. The chosen command replaces the line, keeping any leading `sudo`/`env`/`command`, as `@@` does today.
- New config key `ctrlHKey` (default on), a `ctrl-h` entry in `smcli init --keys` and the interactive `init` prompt, and in `scripts/install.sh --keys`.
- Ctrl-H is often the terminal's Backspace. smcli only takes it when the tty's erase character is not `^H`; otherwise it keeps the existing binding and reports it in the existing "kept existing key bindings" note.
- Inside the picker, Ctrl-H stays Backspace (no in-picker mode switch).

## Capabilities

### New Capabilities

- `picker-hotkeys`: shell keys that open smcli pickers, starting with Ctrl-H for the command picker, and how they yield to existing bindings.

### Modified Capabilities

None. `command-completion-index` is unchanged: the hotkey only reads the index, the same as `@@`.

## Impact

- `crates/smcli/src/shell_integration.rs` (bash, zsh, fish hook templates, `render_config`/`render_with_keys`, hook tests).
- `crates/smcli-core/src/config.rs` (`ctrl_h_key`), `crates/smcli/src/initialize.rs` (`--keys`, prompt).
- `scripts/install.sh`, `scripts/test-shells.py`, `docs/install.md`, `crates/smcli/src/usage.txt` if it lists keys.
- No protocol, service or index changes.
