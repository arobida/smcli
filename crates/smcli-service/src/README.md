# smcli-service/src

## Overview

`smcli-service` is the long-lived daemon that owns one history database. It keeps the database and the
in-memory suggestion index open, serves `smcli` clients over a private Unix-domain socket, runs
embedding search (`smcli-search`) and the `fff-search` file/grep picker, and exits when idle. It is
started on demand by `smcli-core`'s client and stops itself when unused.

## Interface

- `main::{main, start}` — binary entrypoint; expects `--db <absolute path to history.db>`, prepares the
  runtime directory, takes the per-database lock (`try_lock`; a loser exits `0`), then runs the service.
- `service::Service::new` — builds the service: opens the database, reads `data_version`, and wires
  file-search callbacks back into history state.
- `service::Service::run` — binds the socket, installs signal handlers, spawns the 15-second idle
  watcher, warms search, then accepts and serves connections (one thread each).
- `service::Service::{dispatch, record, ranked, select_command, search, context, files, current_engine, load_engine, current_corpus, build_corpus, suggestion_snapshot, config_path, home}` — request handling.
- `service::Service::{lock, touch}` — `State` access and idle-time refresh.
- `service::signals::install` — `SIGTERM`/`SIGINT`/`SIGHUP` remove the socket (if still ours) and `_exit(0)`.
- `file_search::FileSearch::{new, capacity, root, ready, index, files, grep, complete, select, evict_idle, instances}` — one `fff-search` instance per root, pooled and idle-evicted.
- `file_search::evidence::IndexEvidence` and `file_search::evidence::identifier_flags` — `smcli_search::ProjectEvidence` over a live fff instance.
- `test_candidates.rs:test_candidates` — project test-runner commands (`cargo test …`, `pytest -k …`, `npm test …`) from test definitions found in the fff index.

## Invariants

- The socket name fingerprints the supplied database path (`smcli_core::service_location::ServiceLocation::fingerprint`); `main.rs:start` locks the associated lock file before binding.
- Only the lock holder binds (`main.rs::start`); a concurrent start exits `0` instead of stealing the socket.
- `service.rs:Service::serve` rejects peers whose uid differs from the service uid; `smcli_core::service_client` checks the server peer.
- Protocol version is enforced (`smcli_core::service_protocol::VERSION == 3`); a rejected version returns a `version` failure and must not refresh idle time, so an old service still exits.
- `record` validates through `smcli_core::history::CapturedHistory::append` and applies the caller's `CaptureFilter`; a retried event ID inserts no new row (`Service::record`, `CapturedHistory::append`).
- `ranked` requires an absolute directory and `limit` in `1..=1000`; evidence signals (`files`, `git`, `evidence`) are only applied while a ready, non-scanning fff index exists.
- `search` requires a query and absolute directory, `limit` in `1..=1000`; the corpus is reused for the same directory/project/native-file stamp up to 5 minutes.
- File search reuses a ready ancestor index when available. When creating an index outside a repository, content indexing is disabled outside home and at home or its direct children; deeper home scopes may enable it (`file_search.rs::content_indexing_enabled`, `scoped_index`).
- `file_search.rs::FileSearch::select` records selection metadata after resolving the root and rejecting absolute, empty, NUL-containing, overlong, empty-component or `..`-component paths. It does not require the target to exist or resolve symlinks, so this lexical validation does not guarantee filesystem containment.

## Side Effects

- Disk: reads native history files (as a snapshot stamp of path/size/mtime/dev/ino); writes
  `file_selections` and `command_selections` rows and new `commands` rows into the database; removes the
  socket file on shutdown, idle exit, or signal (only while its inode is still ours).
- Processes: runs `git status` for Git status via `smcli_core::context_cache::GitStatus::read` (no shell, no
  optional locks, timeout-killed); spawns fff file-system watchers per indexed root.
- Threads: one per accepted connection; `MAX_CONNECTIONS == 64` limits admission after spawning and excess requests are rejected as busy, so it is not a thread cap. Also: a 15-second idle watcher, a
  background search warm thread, `ContextCache` background Git-status refreshers, and fff scan/watcher
  threads. Searches run one at a time behind `service::Service::searching` so records are not blocked.
- In-memory: holds the database connection, the `SuggestionIndex`, the cached search corpus and engine. `test_candidates.rs` maintains a process-wide, capacity-bounded cache with fff invalidation watches; pruning on access removes dirty, expired (30 seconds), or dead-index entries, and dropping entries unsubscribes watches.
- Network: none. All transport is a local Unix-domain socket.

## Verification

- Run: `make test-rust PKG=smcli-service` (from the repo root), plus the black-box gate `make test-service`; the full suite is `make test`.
