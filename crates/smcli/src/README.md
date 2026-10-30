# `crates/smcli/src` — Interactive CLI and picker

## Overview

The `smcli` executable's source tree. It parses command-line arguments with `clap`, dispatches subcommands, drives the interactive ratatui picker and menu, generates shell hooks, and talks to `smcli-service` through `smcli-core` (falling back to in-process work when the service is absent or incompatible).

This crate intentionally does **not** link `fff-search`; file/grep search prefers the service and falls back to local filesystem search. It does link `smcli-search` for in-process history search and the bundled Potion model.

## Interface

- `main.rs:main` — entry point; prints `smcli: {error}` and exits `1` on failure.
- `cli.rs:run` — top-level dispatcher after argument/menu resolution.
  - `cli.rs:Cli` / `cli.rs:Command` — `clap` parser and subcommand enum (subcommand required).
  - `cli.rs:Init` — arguments for `smcli init`.
  - `cli.rs:print_json` — pretty-prints any serializable value.
  - `cli.rs:stop_service` — sends `Operation::Shutdown` to the service.
  - `cli.rs:indexes` — lists search indexes next to the database; `--prune` deletes non-current indexes.
  - `cli.rs:index_health` — checks index coverage, model health, and ownership.
- `complete.rs:complete_local` — in-process `@` path completion (max depth 6, max entries 20,000), with subsequence/glob ranking. `cli.rs:LocalFileSearch` separately handles file and content grep fallback: its recursive walk follows directories, skips `.git`, and does not use those completion budgets.
- `context.rs:run` — prints repository/project context; prefers the service, falls back to `ProjectContext::discover`.
- `highlight.rs:highlight` — ratatui syntax highlighting for command lines (display lexer, preserves every byte).
- `initialize.rs:run` — implements `smcli init`, including key choices and model switching.
- `menu.rs:is_interactive` / `menu.rs:run` — interactive no-argument menu returning argv for the dispatcher.
- `picker.rs:Mode`, `picker.rs:Choice`, `picker.rs:FileResults`, `picker.rs:Picker`, `picker.rs:run` — inline/fullscreen interactive picker for history, files, and grep.
  - `picker.rs:history_label` / `picker.rs:history_insert` — display/insertion helpers for aliases.
  - `picker.rs:TerminalSession` — opens inline or fullscreen terminal session.
- `shell_integration.rs:render_config` / `shell_integration.rs:render_with_keys` — generates Bash/Zsh/Fish hook text.
- `shell_quoting.rs:quote` / `shell_quoting.rs:insertable` — shell quoting and insertion-safety check.

## Invariants

- `Cli` requires a subcommand; no args on a terminal enters `menu::run` first.
- `--dir` values must be absolute paths (`cli.rs:directory`).
- `--limit` must be a positive integer (`cli.rs:positive`).
- History search and picker paths fall back locally when the service is unavailable; fff-specific evidence remains service-only (`cli.rs:run`, `LocalFileSearch`).
- Native shell history is read-only (`smcli_search::native::HistoryFile::read`); configuration, database and cache writes are separate.
- `SMCLI_DB` must not be the same file or a hard link to the native `HISTFILE` (`smcli_core::history::CapturedHistory::check_separate`).
- Import sources are rejected if they resolve to the destination database (`cli.rs:import`).
- When the requested model changes, initialization attempts to stop the service before loading it, but proceeds if shutdown fails; concurrent model residency is therefore possible (`initialize.rs:run`, `cli.rs:stop_service`).

## Side Effects

- **SQLite**: reads/writes the captured-history database via `CapturedHistory` (`history.db` by default, overridden by `SMCLI_DB`).
- **Search index**: reads/writes `{db}.search/` (TalaDB files, lock file); `reindex` rebuilds it, `prune` compacts the database and reindexes.
- **Service process**: may spawn or stop `smcli-service` via `ServiceClient` (`smcli_core::service_client::ServiceClient`).
- **Terminal**: `picker` and `menu` open `/dev/tty`, switch to raw mode, and use an inline viewport or alternate screen.
- **Filesystem reads**: reads native history files, the Potion model directory, `~/.config/smcli/config.json` (or `SMCLI_CONFIG`), and project directories for completion/context and local file/content search; the latter recursively walks directories without completion’s depth or entry limits.
- **Standard I/O**: prints results/hooks/errors to stdout/stderr; exits `10` for a file insertion and `11` for path-edit mode.

## Verification

- Run: `make test-rust PKG=smcli`
- Run focused CLI smoke after `make build`: `make test-cli`
- Run picker/menu tests: `make test-picker`
