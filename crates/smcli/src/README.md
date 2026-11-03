# `crates/smcli/src` — Interactive CLI and picker

## Overview

The `smcli` executable source tree: CLI dispatch, the ratatui picker and menu, shell hooks, and communication with `smcli-service` (with in-process fallback). This crate does not link `fff-search`; it links `smcli-search` for in-process history and tldr search and the bundled Potion model.

## Interface

- `cli.rs:run` dispatches subcommands; `cli.rs:pick_tldr` runs the tldr command picker for `smcli pick --tldr`.
- `cli.rs:indexes` lists history indexes and tldr vector-cache files beside the database; `--prune` removes non-current entries.
- `complete.rs:complete_local` provides in-process `@` path completion; `LocalFileSearch` handles file and grep fallback.
- `context.rs:run` prints repository/project context.
- `picker.rs:Mode`, `Choice`, and `Picker` implement the interactive history, files, grep, and tldr views.
- `shell_integration.rs` generates Bash, Zsh, and Fish hooks.

## Natural-language command help

In Bash, Zsh, or Fish, type `<program> @@` and press Tab to search that program’s bundled tldr examples in the picker. For example, `bun @@` opens Bun examples; type a question such as “install a package globally” to rank them. Text after `@@` can prefill the query: `bun @@ install globally`. A bare `@@` searches tldr examples for installed programs and labels each result with its program.

Choose an example to replace the prompt text; smcli does not execute it. tldr placeholders become smcli `@` placeholders (for example, `bun add --global {{package}}` becomes `bun add --global @`) for the existing edit flow. Use `smcli pick --tldr` to open this picker directly. Its optional `SMCLI_LBUFFER` context provides the program and command prefix, and `SMCLI_TLDR_QUERY` provides the initial question.

For a program without a bundled page, the picker offers Ctrl-A to search all installed programs. `smcli indexes` includes the tldr vector cache; `--prune` removes stale caches as well as old history indexes.

## Invariants

- `Cli` requires a subcommand; no args on a terminal enters the menu first.
- History picker/search falls back locally when the service is unavailable; fff-specific evidence remains service-only.
- Native shell history is read-only. Configuration, database, and cache writes are separate.
- `SMCLI_DB` must not be the same file or hard link as the native `HISTFILE`.

## Side Effects

- Reads/writes captured history SQLite and `{db}.search/` indexes, including the tldr vector cache.
- May spawn or stop `smcli-service`.
- The picker opens `/dev/tty` and switches terminal mode.
- Reads model/configuration files, bundled tldr JSONL, native history, and project directories.

## Verification

- `make test-rust PKG=smcli`
- `make test-cli`
- `make test-picker`
