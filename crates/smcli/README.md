# `crates/smcli` — User-facing CLI executable

## Overview

This crate builds the `smcli` binary: the command-line interface, ratatui-based picker/menu, shell-hook generator, and dispatcher. It owns parsing, presentation, and the local history/search and picker fallbacks; fff-specific evidence remains service-only.

It depends on:

- `smcli-core` — history database, config, service protocol, socket client.
- `smcli-search` — hybrid BM25 + Potion embedding search and native-history reading.
- `taladb` — the search index backing store.

Per the workspace architecture, **this crate does not link `fff-search`**; file/grep indexing lives in `smcli-service`.

## Interface

- `Cargo.toml` declares the crate and its dependencies (`clap`, `crossterm`, `ratatui`, `smcli-core`, `smcli-search`, `taladb`, etc.).
- `src/usage.txt` is embedded as the usage string (`cli.rs:USAGE`).
- All source modules live under `src/`; there are no public library exports — the crate is an executable.
- The default model is bundled as `potion-shell-16m/` beside the executable; dev builds and tests fall back to `../../Models/potion-shell-16m`.

## Subdirectories & System Boundaries

- [`src/`](src/README.md) — interactive CLI, picker, menu, shell hooks, completion, and command dispatch.

## Invariants

- The default bundled installation places `smcli`, `smcli-service` and `potion-shell-16m/` side by side. An absolute `SMCLI_SERVICE_BIN` overrides the service binary; configured `potionPath` selects another model, and development builds can fall back to the checkout model.
- History, suggestion, search, completion and picker paths retain local fallbacks when the service is missing, disabled or incompatible; service-only fff evidence is unavailable locally (`src/cli.rs:run`, `LocalFileSearch`).
- Native shell history files are read-only everywhere in the system.
- The service client identifies the service executable by path/mtime/size and replaces a stale service (`smcli_core::service_location::build`, `service_client::ServiceClient`).

## Side Effects

- Spawns `smcli-service` on demand through the socket client.
- Reads/writes the user's captured-history SQLite database and search-index directory.
- Generates shell hook text that, when `eval`ed by the user, modifies shell key bindings and records command metadata.

## Verification

- Run: `make test-rust PKG=smcli`
- Run CLI smoke tests after `make build`: `make test-cli`
- Run picker/menu tests: `make test-picker`
- Full gate: `make test`
