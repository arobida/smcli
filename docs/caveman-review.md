# caveman-review

Whole-tree pass over `crates/` (2026-10-22). Comments only. No fixes applied.

## Findings

`crates/smcli/src/shell_integration.rs:195`: 🔴 bug: `pick "$@" "$READLINE_LINE"` has no `--`, so a buffer of `--help` / `-h` is clap's help (exit 0) and bash then does `READLINE_LINE=$chosen`. Same at `:314` (zsh) and `:424` (fish). Pass `--` before the line (`pick "$@" -- "$READLINE_LINE"`).

`crates/smcli/src/cli.rs:60`: 🟡 risk: `Pick.query` is not `allow_hyphen_values`, so a line that is only `-la` or `--version` is a flag, not the query. Same hole on `Search.query` (`:115`) and `Suggest.prefix` (`:51`). Mark them `allow_hyphen_values` once the hooks send `--`.

`crates/smcli/src/picker.rs:635`: 🔴 bug: folder mode (`kind == 1`) does `parent(&h.insert).unwrap_or_else(|| path.clone())`. A hit in the cwd has insert `a.rs` (no slash) and path `src/a.rs`, so the inserted text is root-relative `src/` instead of `./`. Use `./` when `insert` has no slash.

`crates/smcli-core/src/service_client.rs:145`: 🟡 risk: Linux `posix_spawn` sets only `POSIX_SPAWN_SETSID`. macOS also sets `POSIX_SPAWN_CLOEXEC_DEFAULT` (`:144`). Comment on `:123` says no inherited descriptors; on Linux every non-CLOEXEC fd (fifos, extra redirects) stays open in the long-lived service. Set `POSIX_SPAWN_CLOEXEC_DEFAULT` on Linux too.

`crates/smcli-core/src/unix_socket.rs:25`: 🟡 risk: `strerror` is not thread-safe, and `system()` runs on every connection thread in `smcli-service`. Two socket errors at once is a data race. Use `strerror_r`.

`crates/smcli-service/src/service.rs:391`: 🟡 risk: `warm_search` holds `state` across `Potion::bundled()` (`current_engine` → `search_engine` → safetensors decode + `encode_single`). `record` / `suggest` block on that same mutex for the whole model load. Build the `HistorySearch` outside the lock; store it under the lock.

`crates/smcli-service/src/service.rs:459`: 🟡 risk: `current_corpus` reads every native history file while `state` is held (`search` keeps the guard until `:426`). A large HISTFILE stalls captures. Stat under the lock, read and parse outside it.

`crates/smcli-service/src/main.rs:48`: 🟡 risk: `Duration::from_secs_f64` panics on negative, NaN, or infinite. `SMCLI_SERVICE_IDLE=inf` kills the process at startup. Same for `SMCLI_FFF_IDLE` in `service.rs:84`. Reject non-finite / negative and fall back to the default.

`crates/smcli-service/src/service.rs:163`: 🟡 risk: idle and `Shutdown` call `std::process::exit` (shutdown does it while holding `state`, `:198`). Other threads may be in SQLite or `malloc`; `exit` can deadlock. The signal path already uses `_exit` (`:501`). Unlink the socket, then `_exit`.

`crates/smcli-core/src/history.rs:178`: 🟡 risk: `new_event_id` ignores `read_exact` failure and still stamps a UUID. Failed `/dev/urandom` yields one all-zero id, so later `append`s hit the unique uuid index and drop the command. The 5-arg `record` path (`cli.rs:492`) uses this. Bail or retry; don't insert a zero id.

`crates/smcli/src/cli.rs:184`: 🟡 risk: missing `HOME` becomes `/`, so the db and config are `/.local/share/smcli` and `/.config/smcli`. `ServiceClient::new` (`service_client.rs:34`) falls back to `getpwuid` for the config path it sends, so the CLI filter and the service can load different configs, and root's filesystem gets the db. One home lookup for both.

`crates/smcli/src/cli.rs:208`: 🟡 risk: `shell-init` uses `SmcliConfig::load(...).unwrap_or_default()`. A broken `config.json` silently emits the default key set. Other commands already fail the load. Surface the error.

`crates/smcli/src/picker.rs:727`: 🟡 risk: `EnterAlternateScreen` runs before `Terminal::with_options`. On that error the `Err` arm only `disable_raw_mode`s (`:730`) and never `LeaveAlternateScreen`. Leave the alt screen before returning the error.

`crates/smcli-service/src/main.rs:42`: 🔵 nit: `try_lock().is_err()` treats a real lock error like "another service won" and `exit(0)`. Match `WouldBlock` and log anything else.

`crates/smcli-service/src/file_search.rs:65`: 🟡 risk: `pool.building += 1` is not tied to a drop guard. A panic inside `FilePicker::new_with_shared_state` leaves `building` high and `pending` stuck on `None`, so waiters block forever and the capacity slot never returns. Decrement and `notify_all` in a `Drop`.

## Checked, not filed

- Native HISTFILE is read-only. `check_separate` refuses the db path and hard links before SQLite open.
- Socket dir is `0700` + `peer_uid` check. Same-uid clients are trusted; the service is not a security boundary against the user who owns it.
- Record event ids from the hook are `uuid` v4, and a lost reply re-appends the same id (unique index, no double row).
- Fish `\\` / `\'` inside single quotes matches fish 4 docs. `shell_quoting.rs` and the fish binary quote are fine.
- Suggestion-index directory bonuses (×4 / ×3 / ×2) match the SQL scan. Tests cover that.
- `record` argv order (bash 8 fields, zsh/fish 9 with pipestatus) matches `cli.rs:500-513`. `allow_hyphen_values` is already set there.
