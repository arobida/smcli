# smcli-core Blueprint

## Overview

`smcli-core` is the shared library crate for the two smcli executables. It holds the captured-history
SQLite database and its migrations, the capture filter, command-line parsing, ranking and suggestion
scoring, the client/service wire protocol, and the Unix-socket transport the CLI uses to reach
`smcli-service`. It is the home of every behavior both executables need; `smcli` (the CLI) and
`smcli-service` (the daemon) each depend on it, and `smcli-core` itself links no file-search code
(`fff-search` is `smcli-service`-only). Native shell history files are read, never written.

## Interface

This directory has no production source: it holds `Cargo.toml` (package `smcli-core`, edition 2024)
and the `src/` and `tests/` trees. Dependencies are `anyhow`, `libc`, `notify`, `regex`, `rusqlite`
(bundled SQLite, with the `functions` feature for the SQL helpers `smcli_decay`, `smcli_key`,
`smcli_internal`), `serde` and `serde_json`; dev-dependency `tempfile`. All logic and symbols live in
[`./src/README.md`](src/README.md).

## Subdirectories & System Boundaries

- [src](src/README.md) — all production modules: database, parsing, ranking, protocol, transport, client.
- [tests](tests/fixtures/service_protocol/README.md) — integration tests and the frozen
  `ServiceProtocol` wire fixtures (protocol-v1/v2 requests, responses, decoding boundaries).

## Invariants

- Native shell history (`~/.bash_history`, `~/.zsh_history`, Bash/Zsh session files, exported
  `HISTFILE`) is read-only; smcli keeps its own SQLite log and never rewrites those files.
- The captured database must stay a private, uid-owned, non-symlink regular file (created `0600`; no group/other access); `SMCLI_DB`
  must not alias the shell's native history file (path or hard link).
- All capture decisions go through `config.rs::CaptureFilter`: leading-space commands, likely-secret
  tokens, and the user's `historyFilter`/`cwdFilter` regexes are rejected when the corresponding filters are enabled (`CaptureFilter::allows_run`).
- `time.rs` formats ISO 8601 timestamps and parses `Z`/`±HH:MM` forms; stored run times are epoch seconds. Ranking
  decay halves usage every `history.rs::USAGE_HALF_LIFE` (14 days).
- The client/service protocol is versioned (`service_protocol.rs::VERSION == 3`) and frozen in
  fixtures; a version mismatch or a stale binary must fall back or be replaced, never half-parse.

## Side Effects

- Disk: the SQLite database (and its `-wal`/`-shm` files), `~/.config/smcli/config.json`, the runtime
  directory (`SMCLI_RUNTIME_DIR` / `$XDG_RUNTIME_DIR/smcli` / `$TMPDIR/smcli-<uid>`) with per-database
  `.sock` and `.lock` files, and migration artifacts (`.migration.lock`, `.v7.bak`).
- Processes: the client can spawn `smcli-service` on demand and stop a stale one; the crate otherwise
  spawns no processes except `context_cache.rs::GitStatus::read`'s `git` (see src contract).
- Network: none. All transport is a local Unix-domain socket.

## Verification

- Run: `make test-rust PKG=smcli-core` (from the repo root), or the full gate `make test`.
