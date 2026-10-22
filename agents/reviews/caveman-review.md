# caveman-review

Consolidated review of implementation, tests, tooling and release readiness (2026-10-22). Findings only; no implementation fixes applied. This is the single review file, incorporating the still-relevant items from the former issue log and `PROJECT_REVIEW.md`.

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

## Additional whole-codebase review — 2026-10-22

Scope: current working tree at `317355426514d0b3510e3174cdab9aa974e69fe9`, including existing uncommitted edits. Reviewed runtime, search, shell integration, installation and removal against repository docs and architecture rules. This is a whole-tree review, not a branch diff. Existing findings above are preserved, not revalidated or endorsed by this addendum. No implementation fixes applied.

### Standards

`crates/smcli-core/src/suggestion_index.rs:182`: 🟡 **P2 — Accepted future timestamps produce nonfinite service scores.** `history.rs:163` permits timestamps through `1e11`. A run at that time moves the index base forward; querying now makes `2^((base-now)/half-life)` overflow to infinity. Older weights underflow to zero, then become NaN at `suggestion_index.rs:216`; the future run scores infinity. The SQL fallback clamps future age to zero (`history.rs:447`) and remains finite. This breaks the service/in-process behavior requirement in `AGENTS.md`. Bound the factorization or handle future runs separately; test both paths with a far-future accepted timestamp. Verification: inspected rebasing and scoring; numeric exponent/underflow check, without an end-to-end Rust repro. No additional defensible baseline smell findings.

### Spec

`crates/smcli-core/src/suggestion_index.rs:115`: 🟡 **P2 — Delayed captures corrupt command transitions until the index rebuilds.** README.md:54-56 says recording normally happens "in the background". The service feeds records straight into `index.add` (`service.rs:290-292`); affinity uses arrival order unconditionally. Repro: seed `first@100`, add `third@300`, then delayed `second@200`. The index reports previous=`second`, followers(first)={third}, followers(third)={second}. The SQL fallback orders by start time and ID (`history.rs:330-350`), giving first→second→third and previous=`third`. Detect out-of-order arrivals and rebuild/invalidate chronological affinity. Verification: standalone Rust repro linked against the current core library.

`scripts/uninstall.sh:63`: 🔴 **P1 — Fallback export can lose committed history.** `../../docs/install.md:205-207` promises `history.db` is "a consistent copy of the database". When `sqlite3` is absent, `cp` copies only the main database while the service is still running; committed rows may exist only in its WAL. The script later deletes that WAL at line 93. Repro using the exact copy operation with a live WAL database: source contains one committed command, exported database contains zero. A JSON export, when available, does not make the advertised restorable database consistent. Require a SQLite backup implementation or abort before removal; never silently fall back to a raw live-file copy.

`scripts/uninstall.sh:58`: 🟡 **P2 — History export loses privacy and cannot be restored as documented.** The export directory and SQLite backup inherit umask; with `022`, `.backup` creates `history.db` with mode `0644`, and `history.json` is also world-readable. Captured databases are deliberately `0600`, and opening a database rejects any group/other permission (`history.rs:512-528`). Thus the documented `SMCLI_DB=…/history.db` restore (`../../docs/install.md:206-207`) fails, while a default export may expose history to other local users when parent directories are traversable. Create a private export directory and enforce `0600` on exported history/config files, including existing destinations. Verification: system `sqlite3 .backup` changed a `0600` source into a `0644` backup under umask `022`; the current CLI rejected that backup with exit 1 and its private-permissions error.

### Validation

- Targeted reproductions used temporary data; no real history was modified.
- `make test` passed: 69 Rust tests, interactive Bash/Zsh/Fish and plugin-composition checks, concurrent capture, service checks (including a 50,002-file repository), file-picker quoting/insertion, install/uninstall and benchmark smoke test. These existing checks do not cover the four new findings. Build emitted one dead-code warning for `shell_integration::render`.
- `git diff --check -- agents/reviews/caveman-review.md` passed. Only this report was edited by this review; pre-existing implementation edits were preserved.

Additional findings: Standards **1** (worst: nonfinite service scores); Spec **3** (worst: inconsistent export loses committed history).

## Revalidated backlog — 2026-10-22

`crates/smcli/src/picker.rs:734`: 🟡 **P2 — Picker ignores terminal resizing.** `TerminalSession::new` reads the terminal dimensions once and creates `Viewport::Fixed` at line 736. The event loop discards every non-key event at line 482, including `Event::Resize`. Growing the terminal leaves the old layout; shrinking can leave content outside the visible area. Update the terminal viewport on resize and recompute layout. Confirmed from the current construction and event loop; no live resize repro performed.

`scripts/test-shells.py:878`: 🔵 **P3 — Concurrent-writer failures still lack diagnostics and skip remaining waits.** Thirteen subprocesses inherit stderr, then line 882 asserts `all(process.wait() == 0 for process in writers)`. A nonzero result produces a bare assertion, without identifying the writer or its exit code; `all` also short-circuits, leaving later children without explicit waits before service cleanup. Collect every result and report command, exit status and captured stderr for each failure. The earlier reported concurrency failure was not reproduced: the full suite passed during the preceding review. The harness defect itself remains present.

`crates/smcli-core/src/context_cache.rs:442`: 🔵 **P3 — Nonblocking-refresh test remains timing-sensitive.** The assertion requires the initial `cache.context` call to finish in less than 300 ms while the injected reader sleeps for 600 ms. That initial call includes project discovery and watcher creation (`context_cache.rs:222-240`), so scheduling or filesystem delays can fail a correct asynchronous implementation. Replace the sleep/latency distinction with synchronization that proves the call returns while the reader is held pending. The focused test passed in this investigation and the preceding full suite passed; the historical flake was not reproduced, so this remains a test robustness risk rather than a confirmed current test failure.

`THIRD_PARTY_NOTICES.md:1874`: 🟡 **Release-readiness backlog — License materials and distribution decisions remain incomplete.** The current notices still list ten crates with metadata-only license coverage: blake3, glidesort, heed, heed-traits, heed-types, lmdb-master-sys, number_prefix, objc2-core-foundation, objc2-core-services and taladb (versions are listed there). There is still no root `LICENSE`; the notices explicitly record the unresolved xdiff decision at line 47. Reconcile each missing-text entry with the actual selected license/materials and finish the owner decisions and distribution checks in `../plans/shipit-checklist.md`. That checklist's count of eight and references to Swift packages are stale after the Rust port. This finding records repository evidence and unfinished release work, not a new legal compliance determination. Keep `THIRD_PARTY_NOTICES.md`, `../reference/fff-licenses.md` and the release checklist as supporting artifacts, not duplicate review reports.

### Quality limitation

`crates/smcli-search/tests/ranking.rs:289`: 🔵 **Project-search ranking still misses one top result; no aggregate regression demonstrated.** `make ranking` passes with project search MRR **0.750**, top-1 **0.50** (two cases). For `swift test` in `api/Sources/App`, the expected `swift test --filter Lib` ranks second behind `swift test`. Overall search remains MRR **0.955**, top-1 **0.91**, matching the recorded baseline in `../reports/2026-10-20-backend.md`; the ranking test is unchanged from phase-three commit `9b2e1c7`. Retain this as a measured quality limitation, not an established port regression or a failing acceptance gate. Any tuning needs a targeted ranking comparison before changing weights.

### Verification still blocked

`Makefile:42` (`linux` target): 🟡 **Historical Linux Fish prompt timeout remains unverified.** The former issue log reports `make linux` timing out in `scripts/test-shells.py` during Fish setup in a Debian `rust:latest` container. No relevant fix or current Linux pass was established by this investigation. The local Docker client cannot connect to its configured OrbStack socket because the daemon is unavailable, so the Linux run could not be performed. Keep this platform-specific report pending a real `make linux` run; the preceding macOS Fish pass does not resolve it. Record the container Fish version and prompt/setup transcript if it recurs.

### Consolidation checks

- Rechecked all seven source items: the six topics above remain relevant, with uncertainty distinguished from confirmed defects. Removed the obsolete `.build/` and `Sources/smcli/Resources/F2LLM-v2-80M` cleanup item because both paths are absent.
- Removed the unsupported ranking-regression claim after the current ranking run matched the recorded overall baseline.
- `make ranking` and the focused `cargo test -p smcli-core refresh_is_nonblocking_and_failure_keeps_status -- --nocapture` passed. The earlier full-suite result above is from the preceding review; it was not rerun for this documentation consolidation.
- Retired the former issue log and `PROJECT_REVIEW.md` after migration. Pre-existing implementation changes were preserved.

## Final audit (ponytail-audit) — 2026-10-22

Scope: whole tree at `cfe9b2f` (branch `rust-port`). Read in full: `smcli-service` (`service.rs`, `main.rs`, `file_search.rs`), `smcli-core` (`history.rs`, `service_client.rs`, `suggestion_index.rs`, `config.rs`, socket error path), `smcli/src/cli.rs`, the hook text in `shell_integration.rs`, picker file/terminal code, `smcli-search/src/index.rs` search/sync, `scripts/uninstall.sh`. Skimmed: `ranking.rs`, `corpus.rs`, `context_cache.rs`, `test-shells.py`. Not read: `highlight.rs`, `menu.rs`, `parse.rs`, `bench-baseline.py`, `gen-notices.py`.

**What this repo does:** smcli records every shell command (with directory, exit code and timing) into its own SQLite file, then suggests and searches those commands with a ranked list and a hybrid keyword + embedding search. A per-user background service keeps the database and search model warm; every command also works without it.

**Load assumed:** one user, a handful of terminals, history up to ~100k runs. Records arrive in the background after every command, often several at once.

### Corrections to earlier findings

- `shell_integration.rs:195/314/424` (hooks pass the line without `--`): **downgraded 🔴 → 🟡**. Checked with the built CLI: `smcli pick --help`, `pick -la` and `pick --version` all exit **2**, not 0, so the line is never replaced. The real effect: on a line starting with `-`, Up/Ctrl-R does not open the picker and prints usage over the prompt; a line of `--files`/`--grep` silently switches mode. `smcli pick -- -la` already works, so the hook change alone fixes it; `allow_hyphen_values` is not needed.
- `service_client.rs:145` (Linux spawn keeps fds): the suggested fix is wrong. `POSIX_SPAWN_CLOEXEC_DEFAULT` is Apple-only. On glibc use `posix_spawn_file_actions_addclosefrom_np(&actions, 3)` (in `libc` 0.2.191); musl has no equivalent.
- `picker.rs:734` (no resize handling): carries a `shortcut:` comment naming the limit, so it is a recorded decision, not a defect. Left in backlog.
- `unix_socket.rs:25` (`strerror`): the smallest fix is deletion. `io::Error`'s `Display` already prints the OS message; drop the `unsafe` block.
- `history.rs:178` (zero event id): the 5-field `record` path is the only caller; make `new_event_id` return `Result` and `?` the read.

### Must fix

1. **Uninstall can export an empty or old database, then delete the real one** (`scripts/uninstall.sh:63`)
   - **What this is:** Before deleting history, uninstall copies the database so the user can restore it.
   - **Problem:** Without `sqlite3`, it uses `cp`, which skips rows still in the `-wal` file (SQLite's write-ahead log, where recent writes live). Line 93 then deletes that WAL.
   - **Fix:** Without `sqlite3`, stop with "install sqlite3 or pass --no-export". Never fall back to `cp`.
   - **If we skip it:** Users on a Linux box without `sqlite3` lose recent history they were told was exported.

2. **Folder pick in the current directory inserts the wrong path** (`crates/smcli/src/picker.rs:635`)
   - **What this is:** In folder mode the picker shows the parent folder of each matching file.
   - **Problem:** For a file right in the cwd, `insert` is `a.rs` (no `/`), so it falls back to the root-relative `src/`. From inside `src/` that path does not exist.
   - **Fix:** Fall back to `"./"` instead of `path.clone()`. One line, plus a unit test.
   - **If we skip it:** Ctrl-T folder picks in a subdirectory insert broken paths.

3. **Uninstall export is world-readable and cannot be restored as documented** (`scripts/uninstall.sh:58`)
   - **What this is:** The export holds the full command history, which can contain secrets.
   - **Problem:** Files get the umask mode (`0644`). smcli refuses to open a database that others can read, so `SMCLI_DB=…/history.db` fails.
   - **Fix:** `umask 077` at the top of the script and `chmod 700` the export dir, `chmod 600` the files written.
   - **If we skip it:** Other local users can read the export, and the documented restore fails.

4. **A far-future timestamp turns service suggestions into NaN** (`crates/smcli-core/src/suggestion_index.rs:116,182`)
   - **What this is:** The service keeps suggestion scores in memory, stored relative to a base time.
   - **Problem:** `append` accepts start times up to 1e11. One such run moves `base` to the future; at query time `2^((base−now)/h)` is infinity and old scores are 0, so `0 × inf = NaN`. The SQL path clamps age to 0 and stays finite.
   - **Fix:** In `SuggestionIndex::add`, clamp `started_at` to `now()` before it touches `base` or the tally. Test with a run at 1e11.
   - **If we skip it:** One bad clock or crafted record breaks suggestion order until the service restarts.

5. **Background records arriving out of order corrupt "what comes next"** (`crates/smcli-core/src/suggestion_index.rs:115`, `crates/smcli-service/src/service.rs:290`)
   - **What this is:** Hooks record each command in a background process (`&!` in zsh, `&` in fish), so two quick commands can race.
   - **Problem:** The index learns transitions in arrival order. `a@100`, `c@300`, then late `b@200` gives "previous = b" and "c is followed by b". The SQL fallback orders by time and gets it right.
   - **Fix:** Have `SuggestionIndex::add` return `false` when a run is older than its session's last run; the service then drops the index (`state.index = None`) so it rebuilds in order. Test the false return.
   - **If we skip it:** Transition suggestions are quietly wrong for fast typists, and differ between service and fallback (AGENTS.md says they must match).

### Should fix

6. **Hooks send the command line as flags** (`crates/smcli/src/shell_integration.rs:195,314,424`)
   - **What this is:** Up/Ctrl-R run `smcli pick "$@" <line>`.
   - **Problem:** A line like `-la` is parsed as a flag: exit 2, usage printed over the prompt, picker never opens.
   - **Fix:** Insert `--` before the line in all three hooks.
   - **If we skip it:** The picker fails on any line starting with `-`.

7. **Records wait behind model load and corpus build** (`crates/smcli-service/src/service.rs:391,438-469`)
   - **What this is:** Every record, suggest and pick takes the one `state` lock.
   - **Problem:** `warm_search` holds it across embedding-model load; `current_corpus` holds it while reading every native history file and running a full `GROUP BY` over the database.
   - **Fix:** Build the engine outside the lock and store it after. For the corpus: stamp under the lock, read files outside, re-take the lock to store.
   - **If we skip it:** The first suggest after the service starts, and each picker draw during a search rebuild, waits hundreds of ms.

8. **Pruned secrets stay in the search index** (`crates/smcli/src/cli.rs:587-662`)
   - **What this is:** `smcli prune` removes commands the capture filter now rejects (for example a leaked token) and VACUUMs the database.
   - **Problem:** The search index (`history.db.search/*.taladb`) still holds the command text until the next search syncs, and the file is not rewritten.
   - **Fix:** After a non-dry prune that removed anything, call the existing `search_engine(...).reindex(&corpus(...))`, which deletes and rebuilds the index file.
   - **If we skip it:** "Removed" secrets remain readable on disk.

9. **Bad duration env vars panic the service** (`crates/smcli-service/src/main.rs:48`, `service.rs:84`)
   - **What this is:** `SMCLI_SERVICE_IDLE` and `SMCLI_FFF_IDLE` set timeouts.
   - **Problem:** `Duration::from_secs_f64` panics on `inf`, `NaN` or negative.
   - **Fix:** `.filter(|v: &f64| v.is_finite() && *v >= 0.0)` before the default.
   - **If we skip it:** A typo kills the service at start; every command silently falls back.

10. **Shutdown runs `exit` while other threads may be mid-call** (`crates/smcli-service/src/service.rs:159-164`)
    - **What this is:** Idle timeout and `service stop` end the process.
    - **Problem:** `std::process::exit` runs libc exit handlers while connection and fff threads still run. The signal path already uses `_exit`.
    - **Fix:** Unlink the socket, then `libc::_exit(0)`. Commits are already durable (WAL), so nothing is lost.
    - **If we skip it:** Rare hangs at shutdown that leave a dead service owning the lock.

11. **Thread-unsafe `strerror` on connection threads** (`crates/smcli-core/src/unix_socket.rs:25`)
    - **What this is:** Socket errors are turned into text.
    - **Problem:** `strerror` shares one static buffer; two failing connections at once race.
    - **Fix:** Delete the `unsafe` block; use `error.to_string()`.
    - **If we skip it:** Garbled or crashing error text under load. Lean: −3 lines.

12. **The service on Linux keeps the spawning shell's open files** (`crates/smcli-core/src/service_client.rs:145`)
    - **What this is:** The CLI starts the service with `posix_spawn`.
    - **Problem:** macOS closes inherited fds; Linux does not, so `exec 3>log` in a shell stays open for the service's 30-minute life.
    - **Fix:** On glibc add `posix_spawn_file_actions_addclosefrom_np(&mut actions, 3)`.
    - **If we skip it:** Files and pipes stay held open on Linux.

13. **Health check copies the index file name by hand** (`crates/smcli/src/cli.rs:1035-1041`)
    - **What this is:** `smcli health` must find the same file `HistorySearch::index_name` builds.
    - **Problem:** The format string is duplicated, and the comment says so. Bumping `VERSION` in `index.rs` makes health report "not built" forever.
    - **Fix:** Make `index_name` `pub` and call it. Lean: −6 lines.
    - **If we skip it:** A wrong health report after the next index format change.

14. **One random-read failure drops records** (`crates/smcli-core/src/history.rs:176-186`)
    - **What this is:** The 5-field `record` makes its own event id.
    - **Problem:** If `/dev/urandom` cannot be read, the id is all zeros; the second such record hits the unique index and is silently skipped.
    - **Fix:** Return `Result` and `?` the open and the read.
    - **If we skip it:** Silent data loss in a broken sandbox.

15. **Missing `HOME` puts the database under `/`** (`crates/smcli/src/cli.rs:184`)
    - **What this is:** The CLI finds the db and config from `HOME`.
    - **Problem:** It falls back to `/`; `ServiceClient` falls back to `getpwuid`. They can then pick different configs.
    - **Fix:** Move the `getpwuid` fallback into one `home_dir` helper in `smcli-core` and use it in both places.
    - **If we skip it:** cron or `env -i` runs read and write the wrong files.

16. **File-index build leaks its slot on panic** (`crates/smcli-service/src/file_search.rs:65-88`)
    - **What this is:** At most `capacity` fff indexes build at once; waiters sleep on a condvar.
    - **Problem:** If `FilePicker::new_with_shared_state` panics, `building` stays high and `pending` stays `None`; waiters for that root hang forever.
    - **Fix:** Wrap the build in `catch_unwind` (turn a panic into an `Err`), so the existing cleanup always runs.
    - **If we skip it:** A bad repository makes later Ctrl-T requests for it hang until the service restarts.

### Nice to have

17. **`shell-init` hides a broken config** (`cli.rs:208`): print `smcli: <error>; using default keys` to stderr, then continue with defaults. Failing outright would leave the shell with no hooks at all.
18. **Picker leaves the alternate screen on setup error** (`picker.rs:727-760`): send `LeaveAlternateScreen` in the `Err` arm. Also delete the unused `height` parameter of `TerminalSession::new` (`let _ = height;`) and its two call-site arguments.
19. **Test robustness:** `test-shells.py:882` should wait on every writer and print the failing command and exit code; `context_cache.rs:442` should hold the reader on a channel instead of racing 600 ms against 300 ms.
20. **`try_lock` error treated as "another service won"** (`smcli-service/src/main.rs:42`): match `TryLockError::WouldBlock` for the silent exit; report other errors.

Left out: 2 smaller items (imports skip the capture filter by design; `ProjectContext::discover` runs twice in `pick`).

Verdict: fix 1–5 first (data loss, broken export, wrong inserts, wrong service rankings); 6–16 are small and safe to batch.

Lean: about −15 lines possible, −0 dependencies.

Not checked: Linux run (`make linux` — Docker daemon is down), resize behavior live, `highlight.rs`/`menu.rs`/`parse.rs`, license backlog (unchanged; see above).
