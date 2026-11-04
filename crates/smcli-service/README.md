# smcli-service Blueprint

## Overview

`smcli-service` is the long-lived daemon for one history database. It holds the captured-history SQLite
connection and the in-memory suggestion index, serves `smcli` clients over a private Unix-domain socket,
and is the only crate that links `fff-search` (the file/grep picker). `smcli-service` owns runtime model loading and
embedding through `smcli-search`, including protocol-v4 `embed` requests for `@@` query vectors and command-index preparation. It is started on demand by `smcli-core`'s client and
exits when idle. CLI fallbacks may also write the database; `service.rs:Service::refresh` invalidates caches on external writes.

## Interface

No production source here: `Cargo.toml` (package `smcli-service`, edition 2024) plus `src/` and
`tests/`. Dependencies: `anyhow`, `fff-search` (pinned git rev, `definitions` feature), `libc`, `notify`,
`rusqlite`, `smcli-core` (path), `smcli-search` (path); dev-dependency `tempfile`. All logic
and symbols live in [`./src/README.md`](src/README.md). It must never link `fff-search` into the CLI:
ownership of file search stays here.

## Subdirectories & System Boundaries

- [src](src/README.md) — daemon loop, request dispatch, fff file/grep search, evidence, test discovery.
- tests/ — `tests/client.rs` integration tests exercising the real service over its Unix socket.

## Invariants

- One service per database-path socket; the lock file (`main.rs:start`, `try_lock`) ensures only the holder binds the
  per-database socket, and the socket is removed only while its inode is still this service's.
- Peer identity is enforced both ways (server in `service.rs::Service::serve`, client in
  `smcli_core::service_client`): only the same uid may use the socket.
- The search index and corpus are rebuildable caches: `service.rs:Service::refresh` invalidates history caches after external database writes;
  `Service::build_corpus` checks native-history stamps and a five-minute corpus lifetime.
- `service.rs:Service::load_engine` drops the old engine and cached command index before building the new one, and clears the synced program/PATH cache. Command indexes are keyed by their index name.
- Search, `embed` and background command sync hold `Service::searching` for their whole model-using operation. Background sync skips busy work and retries on a later capture; command-index preparation can block searches, whose clients may time out and degrade.
- The service takes `<db>.search/model.lock` before its first model load and retains the same descriptor until exit, including across model switches. If init/reindex holds it, search and `embed` fail without loading; background warmup ignores that failure and later requests retry.
- Protocol-v4 `embed` requires `query` (empty means prepare only). `mode:"commands"` prepares the index on the caller’s `shellPath` and ensures an optional `command`; nonempty queries return finite vectors with the matching command-index name in `model`.

## Side Effects

- Disk: the database (via `smcli-core`), native history files (read), and the socket/lock files under
  the runtime directory.
- Processes: the on-demand `git` calls for status, and fff's file-system watchers.
- Threads/process lifecycle: a thread per accepted connection; admission is limited to 64 active requests after spawning, with excess requests rejected as busy, not a hard thread cap; idle exit
  (`SMCLI_SERVICE_IDLE`, default 1800 s), fff idle eviction (`SMCLI_FFF_IDLE`, default 600 s) and fff
  capacity (`SMCLI_FFF_MAX`, default 4).
- Network: none; Unix-domain socket only.

## Verification

- Run: `make test-rust PKG=smcli-service` (from the repo root), `make test-service` for the black-box
  service gate, or the full `make test`.
