# Fix plan — final audit (2026-10-22)

Source: "Final audit" in `docs/caveman-review.md`. Numbers below are its finding numbers.

## Rules for every batch

- Run `graphify query "<question>"` before your first grep or source read; read raw files only for lines the graph located.
- Smallest complete change. No new dependencies, options or abstractions.
- **Do not run `make`, `cargo build` or `cargo test` for the whole workspace.** Builds are slow and shared. When your edits are done, run `cargo check -p <your crate> --tests` once; fix what it reports in your files; errors in another batch's files mean that agent is mid-edit, so ignore them. The orchestrator runs `make test` once for everything.
- Stay inside your files. Another agent is editing the other batches at the same time.
- New non-trivial logic gets one small unit test.
- Finish by writing a short summary to `docs/fix-notes-<batch>.md`: what changed (file:line), tests added, anything you could not do.

## Batch A — `crates/smcli` (+ one line in `smcli-search`, `smcli-core/config.rs` and `smcli-core/service_client.rs`) → pikcode

| # | Change |
|---|--------|
| 2 | `picker.rs` `filter_kind`: fall back to `"./"` when `insert` has no `/`. Unit test: hit `{path: "src/a.rs", insert: "a.rs"}` in folder mode gives insert `./`. |
| 6 | `shell_integration.rs`: add `--` before the line in the bash, zsh and fish `pick` calls. |
| 8 | `cli.rs` `prune`: after a non-dry prune that removed ≥1 record, rebuild the search index with `search_engine(c, home, env)?.reindex(&corpus(c, home, env, None, now())?)`. `prune` needs `home`/`env` passed in. |
| 13 | `smcli-search/src/index.rs`: make `index_name` `pub`. `cli.rs` `index_health`: use `engine.index_name()`, delete the copied format string and its comment. |
| 15 | Add `pub fn home_dir(env: &HashMap<String,String>) -> PathBuf` to `smcli-core/src/config.rs` (HOME, else `getpwuid` `pw_dir`, else `/`; move the code from `service_client.rs:34-43`). Use it in `cli.rs:184` and `service_client.rs`. |
| 12 | `service_client.rs` `launch`: on `#[cfg(all(target_os = "linux", target_env = "gnu"))]` add `posix_spawn_file_actions_addclosefrom_np(&mut actions, 3)` and fold its status in. Fix the comment to say musl keeps inherited fds. |
| 17 | `cli.rs:208` `shell-init`: on config load error, `eprintln!("smcli: {e:#}; using default keys")`, continue with defaults. |
| 18 | `picker.rs` `TerminalSession::new`: in the `Err` arm, also execute `LeaveAlternateScreen` on the tty (keep a handle, e.g. `try_clone` before moving it). Remove the unused `height` parameter and update the two callers (`picker.rs:260`, `menu.rs:135`). |

## Batch B — `crates/smcli-core` (except `config.rs` and `service_client.rs`) and `crates/smcli-service` → pikcode

| # | Change |
|---|--------|
| 4 | `suggestion_index.rs` `add`: clamp `started_at` to `now()` (local copy of the run) before it moves `base` or tallies. Test: index with old runs plus one at `1e11` returns finite scores in the right order. |
| 5 | `suggestion_index.rs`: `add` returns `bool`, `false` when the run is older than that session's last run (keep the last start time per session in `CommandAffinity` or the index). `new` ignores it. `service.rs` `record`: if `add` returns false, set `state.index = None`. Test: `first@100`, `third@300`, then `second@200` returns false; a fresh index from the sorted runs has previous=`third`. |
| 7 | `service.rs` `warm_search`: build the engine without holding `state` (load config, call `search_engine`), then lock and store. `current_corpus`: compute stamps under the lock; if stale, drop the lock, read native files and `command_stats`, re-lock, store. Keep `searching` held as today so only one search builds at a time. |
| 9 | `main.rs:48`, `service.rs:84`: ignore non-finite or negative values (`.filter(|v: &f64| v.is_finite() && *v >= 0.0)`). |
| 10 | `service.rs` `stop`: after removing the socket, `unsafe { libc::_exit(0) }` instead of `std::process::exit(0)`. |
| 11 | `unix_socket.rs:25`: delete the `strerror` block; use `error.to_string()`. |
| 14 | `history.rs` `new_event_id`: return `Result<String>`, `?` on open and `read_exact`. Update the one caller in `append`. |
| 16 | `file_search.rs` `index`: wrap the build closure in `std::panic::catch_unwind(AssertUnwindSafe(..))`, mapping a panic to `Err("file index build panicked")`, so the existing cleanup always runs. |
| 19b | `context_cache.rs:442` test: make the reader block on a `std::sync::mpsc` receiver the test releases after asserting `status_pending`, instead of the 600 ms / 300 ms race. |
| 20 | `main.rs:42`: exit 0 only on `TryLockError::WouldBlock`; other errors return `Err`. |

## Batch C — `scripts/` → pisol

| # | Change |
|---|--------|
| 1 | `uninstall.sh`: if `sqlite3` is missing and an export is due, `die "sqlite3 is needed to export history; install it or pass --no-export"` before anything is removed. Delete the `cp` fallback. |
| 3 | `uninstall.sh`: `umask 077` right after `set -euo pipefail`; after `mkdir -p "$export_dir"` run `chmod 700 "$export_dir"`; after the export files are written, `chmod 600` each one that exists. Update `docs/install.md` if it describes the export mode. |
| 19a | `test-shells.py:878-882`: wait on every writer, collect `(args, returncode)`, and assert with a message listing each failure. Leave stderr inherited. |

Check: `bash -n scripts/uninstall.sh`, `python3 -m py_compile scripts/test-shells.py`. No `make`.

## Investigations (no code until resolved)

- **Linux Fish prompt timeout** (`make linux`): needs the Docker daemon (OrbStack is down). Leftover.
- **Resize handling**: recorded `shortcut:` decision. Leftover backlog.
- **License backlog**: owner decisions, not code. Leftover.

## Verification (orchestrator)

1. Read each agent's diff (`git diff`) and notes.
2. `make test` once after all batches land; fix or send back failures.
3. `make ranking` (batch B touches suggestion scoring).
4. `graphify update .`
5. `make fresh`; give the user the `eval` line.
6. Write `docs/after-action-report.md`.
