# Tasks

Implementation runs through the Orchestrator (`orchestrator` agent) per AGENTS.md: it opens each team agent in Kero and relays results. Suggested routing is noted per group; the Orchestrator may rebalance. Groups 1 and 2 can run in parallel; 3 waits on both.

## 1. Config and init (route: `kikcode`)

- [x] 1.1 Add `ctrl_o_key: Option<bool>` (`ctrlOKey`) to `SmcliConfig` in `crates/smcli-core/src/config.rs`; verify a config round-trip test in `make test-rust PKG=smcli-core`.
- [x] 1.2 `crates/smcli/src/initialize.rs`: accept `ctrl-o` in `--keys` (and error text), add "Open command examples with Ctrl-O?" prompt; verify existing init tests updated and pass in `make test-rust PKG=smcli`.
- [x] 1.3 `scripts/install.sh`: `ctrl-o` in `--keys` help, validation and defaults; verify `make test-install`.

## 2. Shell hooks (route: `pigrok47`)

- [x] 2.1 `render_config`/`render_with_keys` take `ctrl_h` from `config.ctrl_o_key.unwrap_or(true)` and fill `\#(keyO)`; verify unit test that `keyO` 0/1 renders for all shells.
- [x] 2.2 bash: `_smcli_pick_commands` runs `pick --tldr` with `SMCLI_LBUFFER=${READLINE_LINE:0:READLINE_POINT}`, replaces the line on output; bind `\C-h` (replacing `backward-delete-char`) unless `stty` erase is `^H`, then record it in `_smcli_kept`.
- [x] 2.3 zsh: same widget with `$LBUFFER`, `_smcli_bind` `^H` in emacs and viins (`backward-delete-char|vi-backward-delete-char`), same erase guard.
- [x] 2.4 fish: same function with `commandline -c`, `_smcli_bind \ch`, same erase guard.
- [x] 2.5 Hook unit tests in `shell_integration.rs` assert each shell's Ctrl-O bind line and guard; verify `make test-rust PKG=smcli FILTER=shell`.

## 3. Black-box checks and docs (route: `piterra`)

- [x] 3.1 `scripts/test-shells.py`: Ctrl-O on an empty line opens the picker, Esc leaves the line; with `stty erase ^H` Ctrl-O still deletes. Verify `make test-shells`.
- [x] 3.2 `docs/install.md` (and `usage.txt` if it lists keys): add Ctrl-O and the Backspace note; run `make docs`.
- [x] 3.3 Full gate `make test` passes; `make fresh` and relay the eval line to the user.

## Workflow follow-up

- Code review of the diff (route: `piterra`) before commit; commit message via `pigrok45` only when the user asks.
