# `crates/smcli/src` — Interactive CLI and picker

## Overview

The `smcli` executable source tree: CLI dispatch, the ratatui picker and menu, shell hooks, and communication with `smcli-service` (with in-process fallback). This crate does not link `fff-search`; it links `smcli-search` for keyword fallback and model-free index readers. Only `initialize.rs` loads Potion, for model-changing init and reindex, after stopping the service and acquiring the model lock.

## Interface

- `cli/mod.rs:run` dispatches subcommands; `cli/tldr.rs` runs the tldr command picker for `smcli pick --tldr`.
- `cli/index.rs` lists history and command-example indexes beside the database; `--prune` removes non-current entries.
- `complete.rs:complete_local` provides in-process `@` path completion; `LocalFileSearch` handles file and grep fallback.
- `context.rs:run` prints repository/project context.
- `picker.rs:Mode`, `Choice`, and `Picker` implement the interactive history, files, grep, and tldr views.
- `shell_integration.rs` generates Bash, Zsh, and Fish hooks.
- `initialize.rs:run` describes an unchanged model without loading it; a changed model and `initialize.rs:reindex` stop the service and refuse if its model lock stays held for five seconds. Reindex also prepares command examples.
- `cli/index.rs` uses `configured_model` and model-free index naming; health never embeds a probe.
- `cli/tldr.rs:tldr_ranker` requests service embeddings with a two-second timeout, accepts only the current command-index model name and matching vector dimensions, and latches word ranking after the first unusable reply.

## Natural-language command help

In Bash, Zsh, or Fish, type `<program> @@` and press Tab to search that program’s bundled tldr examples in the picker. For example, `bun @@` opens Bun examples; type a question such as “install a package globally” to rank them. Text after `@@` can prefill the query: `bun @@ install globally`. A bare `@@` searches tldr examples for installed programs and labels each result with its program.

Choose an example to replace the prompt text; smcli does not execute it. tldr placeholders become smcli `@` placeholders (for example, `bun add --global {{package}}` becomes `bun add --global @`) for the existing edit flow. Use `smcli pick --tldr` to open this picker directly. Its optional `SMCLI_LBUFFER` context provides the program and command prefix, and `SMCLI_TLDR_QUERY` provides the initial question.

For a program without a bundled page, the picker offers Ctrl-A to search all installed programs. `smcli indexes` includes the tldr vector cache; `--prune` removes stale caches as well as old history indexes.

The CLI reads the command-example index without loading Potion. It asks the service to prepare only a missing index (up to 600 seconds) or a missing program (up to 10 seconds); a complete index opens immediately. Per-query service embeddings are bounded to two seconds. An unavailable service, failed request, timeout, or model mismatch latches word ranking for the picker session. If the index is missing, the status asks for smcli-service or `smcli reindex`. `SMCLI_SERVICE=0` uses keyword history search and word-ranked examples, never runtime model loading.

## Invariants

- `Cli` requires a subcommand; no args on a terminal enters the menu first.
- History picker/search falls back to keyword matching without opening the index or loading Potion when the service is unusable; only the search command prints the degradation notice. fff-specific evidence remains service-only.
- Native shell history is read-only. Configuration, database, and cache writes are separate.
- `SMCLI_DB` must not be the same file or hard link as the native `HISTFILE`.

## Side Effects

- Reads/writes captured history via `smcli-core` and `{db}.search/` indexes via `smcli-search`, including command examples.
- May spawn or stop `smcli-service`.
- The picker opens `/dev/tty` and switches terminal mode.
- Reads model/configuration files, bundled tldr JSONL, native history, and project directories.

## Verification

- `make test-rust PKG=smcli`
- `make test-cli`
- `make test-picker`
