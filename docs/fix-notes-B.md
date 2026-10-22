# Batch B fix notes

Scope: `crates/smcli-core` (except `config.rs` and `service_client.rs`) and `crates/smcli-service`.
Source: `docs/caveman-review.md` Final audit findings 4, 5, 7, 9, 10, 11, 14, 16, 19b, 20.

## Changes

### `crates/smcli-core/src/suggestion_index.rs`
- `CommandAffinity` now tracks `last_start` per session (`:18`).
- `CommandAffinity::add` returns `bool` and returns `false` when a run is older than the session's last start (`:28`).
- `SuggestionIndex::add` clamps `started_at` to `now()` before touching `base` or tallies, stores the clamped run, and returns `false` for out-of-order arrivals (`:118`).
- Service call in `crates/smcli-service/src/service.rs:290` drops the in-memory index when `add` returns `false` so it rebuilds in chronological order.

### `crates/smcli-core/src/history.rs`
- `new_event_id` now returns `Result<String>` and propagates `/dev/urandom` open/read failures (`:176`).
- `CapturedHistory::append` uses `run.event.clone().map(Ok).unwrap_or_else(new_event_id)?` (`:248`).

### `crates/smcli-core/src/unix_socket.rs`
- Removed thread-unsafe `libc::strerror` path in `system()`; errors use `error.to_string()` (`:25`).

### `crates/smcli-core/src/context_cache.rs`
- `refresh_is_nonblocking_and_failure_keeps_status` test now blocks the reader on an `mpsc` receiver and releases it only after asserting `status_pending`, removing the 600 ms / 300 ms race (`:429`).

### `crates/smcli-service/src/service.rs`
- `warm_search` loads config and builds the search engine outside `state`, then locks only to store it (`:391`).
- `search` now loads the index/engine under `state`, drops the lock, then calls `current_corpus` to read native files and `command_stats` outside the lock (`:419`).
- `current_corpus` signature changed to take no `state` reference: it checks staleness under the lock, drops the lock for I/O, then re-locks to store (`:454`).
- `stop` uses `unsafe { libc::_exit(0) }` after unlinking the socket (`:159`).
- `SMCLI_FFF_IDLE` rejects non-finite or negative values (`:84`).

### `crates/smcli-service/src/main.rs`
- `try_lock()` matches `std::fs::TryLockError::WouldBlock` for silent exit and reports other lock errors (`:42`).
- `SMCLI_SERVICE_IDLE` rejects non-finite or negative values (`:52`).

### `crates/smcli-service/src/file_search.rs`
- `FileSearch::index` wraps the build closure in `catch_unwind(AssertUnwindSafe(...))`, mapping panics to `Err("file index build panicked")` so pool cleanup still runs (`:68`).

## Tests added

- `suggestion_index.rs` `far_future_timestamp_stays_finite`: a run at `1e11` keeps all suggestion scores finite and the clamped run outranks older runs.
- `suggestion_index.rs` `out_of_order_run_invalidates_index`: `first@100`, `third@300`, `second@200` makes `add` return `false`; a fresh chronologically-sorted index has `previous_command("s") == "third"`.

## Verification

- `cargo check -p smcli-core --tests` passes.
- `cargo check -p smcli-service --tests` passes (one warning fixed).

## Not done / notes

- No `make`, `cargo build`, or `cargo test` workspace runs were performed per batch rules.
- `current_engine` still builds the embedding model while holding `state` if `warm_search` did not preload it or the config changed; this is outside the scope of finding 7.
