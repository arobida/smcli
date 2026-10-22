# caveman-review

Consolidated review of implementation, tests, tooling and release readiness (2026-10-22). Findings only; no implementation fixes applied. This is the single review file, incorporating the still-relevant items from the former `.plans/issues.md` and `PROJECT_REVIEW.md`.

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

`scripts/uninstall.sh:63`: 🔴 **P1 — Fallback export can lose committed history.** docs/install.md:205-207 promises `history.db` is "a consistent copy of the database". When `sqlite3` is absent, `cp` copies only the main database while the service is still running; committed rows may exist only in its WAL. The script later deletes that WAL at line 93. Repro using the exact copy operation with a live WAL database: source contains one committed command, exported database contains zero. A JSON export, when available, does not make the advertised restorable database consistent. Require a SQLite backup implementation or abort before removal; never silently fall back to a raw live-file copy.

`scripts/uninstall.sh:58`: 🟡 **P2 — History export loses privacy and cannot be restored as documented.** The export directory and SQLite backup inherit umask; with `022`, `.backup` creates `history.db` with mode `0644`, and `history.json` is also world-readable. Captured databases are deliberately `0600`, and opening a database rejects any group/other permission (`history.rs:512-528`). Thus the documented `SMCLI_DB=…/history.db` restore (docs/install.md:206-207) fails, while a default export may expose history to other local users when parent directories are traversable. Create a private export directory and enforce `0600` on exported history/config files, including existing destinations. Verification: system `sqlite3 .backup` changed a `0600` source into a `0644` backup under umask `022`; the current CLI rejected that backup with exit 1 and its private-permissions error.

### Validation

- Targeted reproductions used temporary data; no real history was modified.
- `make test` passed: 69 Rust tests, interactive Bash/Zsh/Fish and plugin-composition checks, concurrent capture, service checks (including a 50,002-file repository), file-picker quoting/insertion, install/uninstall and benchmark smoke test. These existing checks do not cover the four new findings. Build emitted one dead-code warning for `shell_integration::render`.
- `git diff --check -- docs/caveman-review.md` passed. Only this report was edited by this review; pre-existing implementation edits were preserved.

Additional findings: Standards **1** (worst: nonfinite service scores); Spec **3** (worst: inconsistent export loses committed history).

## Revalidated backlog — 2026-10-22

`crates/smcli/src/picker.rs:734`: 🟡 **P2 — Picker ignores terminal resizing.** `TerminalSession::new` reads the terminal dimensions once and creates `Viewport::Fixed` at line 736. The event loop discards every non-key event at line 482, including `Event::Resize`. Growing the terminal leaves the old layout; shrinking can leave content outside the visible area. Update the terminal viewport on resize and recompute layout. Confirmed from the current construction and event loop; no live resize repro performed.

`scripts/test-shells.py:878`: 🔵 **P3 — Concurrent-writer failures still lack diagnostics and skip remaining waits.** Thirteen subprocesses inherit stderr, then line 882 asserts `all(process.wait() == 0 for process in writers)`. A nonzero result produces a bare assertion, without identifying the writer or its exit code; `all` also short-circuits, leaving later children without explicit waits before service cleanup. Collect every result and report command, exit status and captured stderr for each failure. The earlier reported concurrency failure was not reproduced: the full suite passed during the preceding review. The harness defect itself remains present.

`crates/smcli-core/src/context_cache.rs:442`: 🔵 **P3 — Nonblocking-refresh test remains timing-sensitive.** The assertion requires the initial `cache.context` call to finish in less than 300 ms while the injected reader sleeps for 600 ms. That initial call includes project discovery and watcher creation (`context_cache.rs:222-240`), so scheduling or filesystem delays can fail a correct asynchronous implementation. Replace the sleep/latency distinction with synchronization that proves the call returns while the reader is held pending. The focused test passed in this investigation and the preceding full suite passed; the historical flake was not reproduced, so this remains a test robustness risk rather than a confirmed current test failure.

`THIRD_PARTY_NOTICES.md:1874`: 🟡 **Release-readiness backlog — License materials and distribution decisions remain incomplete.** The current notices still list ten crates with metadata-only license coverage: blake3, glidesort, heed, heed-traits, heed-types, lmdb-master-sys, number_prefix, objc2-core-foundation, objc2-core-services and taladb (versions are listed there). There is still no root `LICENSE`; the notices explicitly record the unresolved xdiff decision at line 47. Reconcile each missing-text entry with the actual selected license/materials and finish the owner decisions and distribution checks in `.plans/shipit-checklist.md`. That checklist's count of eight and references to Swift packages are stale after the Rust port. This finding records repository evidence and unfinished release work, not a new legal compliance determination. Keep `THIRD_PARTY_NOTICES.md`, `docs/fff-licenses.md` and the release checklist as supporting artifacts, not duplicate review reports.

### Quality limitation

`crates/smcli-search/tests/ranking.rs:289`: 🔵 **Project-search ranking still misses one top result; no aggregate regression demonstrated.** `make ranking` passes with project search MRR **0.750**, top-1 **0.50** (two cases). For `swift test` in `api/Sources/App`, the expected `swift test --filter Lib` ranks second behind `swift test`. Overall search remains MRR **0.955**, top-1 **0.91**, matching the recorded baseline in `.plans/taladb-port.md:120`; the ranking test is unchanged from phase-three commit `9b2e1c7`. Retain this as a measured quality limitation, not an established port regression or a failing acceptance gate. Any tuning needs a targeted ranking comparison before changing weights.

### Verification still blocked

`Makefile:42` (`linux` target): 🟡 **Historical Linux Fish prompt timeout remains unverified.** The former issue log reports `make linux` timing out in `scripts/test-shells.py` during Fish setup in a Debian `rust:latest` container. No relevant fix or current Linux pass was established by this investigation. The local Docker client cannot connect to its configured OrbStack socket because the daemon is unavailable, so the Linux run could not be performed. Keep this platform-specific report pending a real `make linux` run; the preceding macOS Fish pass does not resolve it. Record the container Fish version and prompt/setup transcript if it recurs.

### Consolidation checks

- Rechecked all seven source items: the six topics above remain relevant, with uncertainty distinguished from confirmed defects. Removed the obsolete `.build/` and `Sources/smcli/Resources/F2LLM-v2-80M` cleanup item because both paths are absent.
- Removed the unsupported ranking-regression claim after the current ranking run matched the recorded overall baseline.
- `make ranking` and the focused `cargo test -p smcli-core refresh_is_nonblocking_and_failure_keeps_status -- --nocapture` passed. The earlier full-suite result above is from the preceding review; it was not rerun for this documentation consolidation.
- Retired `.plans/issues.md` and `PROJECT_REVIEW.md` after migration. Pre-existing implementation changes were preserved.
