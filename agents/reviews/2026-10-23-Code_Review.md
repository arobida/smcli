# Consolidated code review — 2026-10-23

The current working tree fixes most of the October 22 safety defects, but still has four immediate data/privacy risks: shell configuration truncation, failed-commit recovery, unfiltered file-selection queries, and over-broad pruning. Search also has a cache-publication race and several retrieval/filtering defects, including both new Codex findings. This review checks the supplied reports against the working tree at HEAD `16c3200`, including pre-existing uncommitted search changes, rather than assuming that an old finding or a claimed fix remains true. No implementation code was changed. `make ranking` passed: suggest MRR 1.000/top-1 1.00; search MRR 0.962/top-1 0.92 across 13 cases. The remaining Swift project-search miss is still present; the changed query set means the new aggregate is not a like-for-like improvement over the older 11-case baseline.

## Fix status — 2026-10-24

Fixed in the working tree (uncommitted), each with a regression test; `make test` and `make fresh` pass, `make ranking` unchanged (search MRR 0.962):

- **Fixed:** #1–#12, #14, #15, #18, #19, #21.
- **#19 note:** the Bash 3.2 notice prints once per database directory (stamp file `.bash3-warned`); `scripts/test-shells.py` ignores that line.
- **#8 note:** an index built before the fix may show as stale in `smcli health` for commands over 1,500 characters until the next search rewrites them.
- **#11 note:** the model-identity collision was not tested directly.

Not done:

- **#13 picker fallback performance:** it touches `cli.rs`, which another fix was already changing at the same time; it was held back to avoid conflicts. Next in line.
- **#16 musl descriptors:** needs an owner decision: support musl or exclude it.
- **#17 licensing:** owner decisions and license texts, tracked in `agents/plans/shipit-checklist.md`.
- **#20, #22–#25:** nice-to-have or accepted limitations; deferred to a later pass.

## Sources and verification

Source labels below mean:

- **Backend:** `agents/reports/2026-10-20-backend.md`.
- **Fix report:** `agents/reports/2026-10-22-audit-fixes.md`.
- **Earlier review:** `agents/reviews/caveman-review.md`, including its addenda and final audit.
- **Core audit:** `/private/tmp/claude-501/-Users-arobida-Dev-Apps-smcli/23e52d8d-5141-4e37-b289-575da1033003/scratchpad/audit-core.md`.
- **CLI audit:** the same scratchpad directory's `audit-cli.md`.
- **Codex:** the two medium findings supplied with this task.

Every finding below is still open in the current code. Today's audit claims were spot-checked at the cited implementation paths; their suggested fixes were not accepted blindly. Historical fixes were checked against current files and commits, principally `5a43d1e`, the Rust ports `9b2e1c7`/`16dafa1`/`d050644`, and render removal `102cacd`. Repeated historical descriptions are consolidated into one finding or one resolution row. Performance measurements and platform failures from earlier reports remain historical unless explicitly re-run here.

## Must fix

### 1. A damaged hook block can erase the rest of a shell startup file

**File:** `scripts/install.sh:87-92`; `scripts/uninstall.sh:82-87`.

- **What it is:** Both scripts remove smcli's marked block with an awk `skip` flag.
- **Problem:** If `.zshrc` has `# >>> smcli >>>` but no closing marker, a normal upgrade drops every subsequent line, including unrelated user configuration. Another upgrade overwrites the backup with the damaged file; uninstall also removes the backup.
- **Fix:** Validate complete, correctly ordered marker pairs before any rewrite or backup replacement. Abort and preserve the file on malformed blocks. Test missing, reversed and duplicate markers; matching counts alone do not prove valid pairing.
- **If skipped:** Routine install/uninstall can silently destroy arbitrary shell configuration.
- **Sources:** CLI audit #4.

### 2. A failed commit leaves the service connection inside a transaction

**File:** `crates/smcli-core/src/history.rs:611-637`, especially `:632`.

- **What it is:** `insert` manually begins a transaction and rolls back insert errors, but not commit errors.
- **Problem:** With a current-schema DELETE-journal database and a reader holding a lock beyond the busy timeout, `COMMIT` can fail while leaving the transaction active. Subsequent service records fail with “cannot start a transaction within a transaction,” even after the reader leaves. `connect` only enables WAL during migration, so this is reachable.
- **Fix:** Roll back failed commits before returning, or use a transaction guard that rolls back every unsuccessful path. Test recovery using a second connection holding a read transaction.
- **If skipped:** One temporary lock failure can disable service recording until restart; failed writes may remain visible to that connection's reads.
- **Sources:** Core audit #2. The audit's temporary SQLite reproduction was reported there; this consolidation verified the unguarded commit and WAL setup in current code, not a new service reproduction.

### 3. File and grep selections persist queries that the capture filter rejects

**File:** `crates/smcli-service/src/file_search.rs:154-168`; `crates/smcli-service/src/service.rs:95-100`; `crates/smcli-core/src/history.rs:284-289`.

- **What it is:** Selecting a file writes the query to `file_selections`; this path does not apply `CaptureFilter`.
- **Problem:** Paste an AWS-key-shaped secret into grep and select its match: the key is stored in SQLite even though ordinary capture would reject it. Excluded working directories also do not prevent this write. Command selections already use the configured filter.
- **Fix:** Carry the request's config and actual working directory into file-selection recording, apply the same query/directory policy, and skip rejected persistence without preventing insertion of the selected path. Test a standard-filter secret and an excluded directory.
- **If skipped:** Secrets intentionally excluded from history remain in that same database indefinitely through file search.
- **Sources:** Core audit #3.

### 4. Pruning one rejected query deletes unrelated clean selections

**File:** `crates/smcli/src/cli.rs:647-669`.

- **What it is:** `prune` mixes commands from rejected command rows with commands mentioned by rejected selection queries, then deletes all selections for each command.
- **Problem:** For filter `^vault`, picks `(git push, vault-token-xyz)` and `(git push, push)` both disappear, although only the first query is rejected. Dry-run output also presents the allowed command as being removed.
- **Fix:** Keep the actual pruned-command set separate from rejected selection IDs. Only sweep all selections for commands whose command rows are being removed; report command and selection counts accurately. Test mixed clean/rejected queries for one command.
- **If skipped:** Running the privacy cleanup silently destroys unrelated ranking history.
- **Sources:** CLI audit #1.

## Should fix

### 5. A concurrent record can be overwritten when publishing the search corpus

**File:** `crates/smcli-service/src/service.rs:289-300`, `:466-486`.

- **What it is:** Corpus construction happens outside `state`, and publication compares only SQLite `data_version`.
- **Problem:** SQLite does not advance that value for writes through the same connection. A record arriving after `command_stats` finishes but before publication updates the old cache, if present; publication then replaces it with the stale corpus. The acknowledged command can be missing from cached searches for five minutes. The comment claiming an own write moved `data_version` is incorrect.
- **Fix:** Track a local history revision, increment after successful insertions, and compare it alongside the external-write version when publishing. Add a deterministic paused-build/record/resume test.
- **If skipped:** Search intermittently cannot find a command that was successfully recorded. This also means the fix report's claim that mid-rebuild records are always detected is incomplete.
- **Sources:** Core audit #1; Fix report's corpus follow-up and rebuild-window limitation; Backend's rebuild-cost limitation.

### 6. Unresolved dollar markers suppress lock-contention fallback retrieval

**File:** `crates/smcli-search/src/index.rs:411-426`; `crates/smcli-search/src/corpus.rs:275-303`.

- **What it is:** Marker parsing reports syntax separately from successful filter resolution; fallback incorrectly treats any marker syntax as a reason not to retrieve unfiltered candidates.
- **Problem:** With the index lock held, `echo $HOME` sets `has_markers` but typically resolves no program/subcommand/directory filter. `parsed_any` is false, so the filtered pass never runs; `!has_markers` is false, so the ordinary pass never runs either. Ordinary matching history is suppressed, although later signal additions may still return something.
- **Fix:** Distinguish resolved strict filters from unresolved marker text. With no resolved constraint, allow normal lexical retrieval under the explicit filter. Preserve the strict behavior for resolved markers; test both cases while holding the lock.
- **If skipped:** Searches containing ordinary shell variables can unexpectedly return no history during reindexing or concurrent search.
- **Sources:** Codex finding (a). Confirmed against the current parser and fallback branches.

### 7. Strict marker constraints disappear before selection-based additions

**File:** `crates/smcli-search/src/index.rs:235-237`; also `:435-450` in `text_fallback` and `crates/smcli-search/src/corpus.rs:345-357`.

- **What it is:** `final_filter` keeps the merged marker filter only when parsed retrieval produced hits.
- **Problem:** If a resolved marker combination returns no indexed matches, `parsed_useful` is false. A remembered selection for the same query can then be added using only the explicit filter and violate the supposedly strict markers. The fallback path makes the same final-filter decision.
- **Fix:** Keep resolved strict marker constraints for additions even when retrieval is empty. Only discard unsuccessful *automatic* inferred filters. Cover indexed and lock-held paths with an empty strict intersection and a remembered selection outside it.
- **If skipped:** A strict search can suggest commands outside the requested tool/directory constraints precisely when it should return nothing.
- **Sources:** Codex finding (b); additional spot-check of the shared fallback pattern.

### 8. Long commands sharing an embedding document overwrite each other's identity

**File:** `crates/smcli-search/src/index.rs:254-285`; `crates/smcli-search/src/corpus.rs:142-149`.

- **What it is:** `sync` keys `wanted` by `search_document`, which truncates commands to 1,500 characters.
- **Problem:** Two pasted scripts with identical first 1,500 characters and identical intent collapse into one map entry. One cannot be retrieved. Subsequent metadata updates do not replace the stored command identity, and health comparisons can stay inconsistent with the corpus.
- **Fix:** Give documents a stable full-command identity independent of the truncated embedding text, and use it consistently in sync, metadata updates and health checks. Keep embeddings based on the bounded text; do not append an identifier to model input unnecessarily. Test two colliding prefixes across initial and subsequent syncs.
- **If skipped:** Real recorded commands silently disappear from keyword and semantic search.
- **Sources:** CLI audit #2.

### 9. An unreadable native-session directory breaks otherwise usable read commands

**File:** `crates/smcli-search/src/native.rs:83`.

- **What it is:** `history_files` propagates `read_dir` failure while individual history-file reads are best-effort.
- **Problem:** An unreadable `~/.bash_sessions` or `~/.zsh_sessions` makes search/history/health/reindex and picker fallback fail, even when SQLite and ordinary history files are readable.
- **Fix:** Treat an inaccessible optional session directory like an unreadable optional history file; skip it, optionally warn, and retain other sources. Test under permissions that actually deny access, or inject a listing failure.
- **If skipped:** A single optional directory disables several core commands.
- **Sources:** CLI audit #3.

### 10. Native-history cache stamps discard subsecond changes

**File:** `crates/smcli-service/src/service.rs:460-465`.

- **What it is:** The stamp includes path, size and whole-second `mtime` only.
- **Problem:** An equal-length rewrite within the same second can reuse the previous corpus for five minutes, returning a removed command and omitting its replacement.
- **Fix:** Include nanosecond modification time and replacement identity such as device/inode. Test equal-length rewrites in one second.
- **If skipped:** Native-history search remains stale despite actual file changes.
- **Sources:** Core audit #4.

### 11. Different model directories can share an index identity

**File:** `crates/smcli-search/src/potion.rs:40-44`.

- **What it is:** Model path identity replaces all punctuation with underscores.
- **Problem:** `/x/a.b` and `/x/a/b` produce the same slug. Two different same-dimension models then reuse an index of incompatible vectors while health still recognizes the filename.
- **Fix:** Include a stable hash of the canonical path in the identity; test colliding slugs. If replacing a model in place is supported, include a model-content/version fingerprint too.
- **If skipped:** Switching models can silently produce incorrect ranking.
- **Sources:** CLI audit #6.

### 12. Index recovery deletes the cache for every open/setup error

**File:** `crates/smcli-search/src/index.rs:298-324`.

- **What it is:** Any first open/setup failure discards index files and retries, losing the first error.
- **Problem:** A transient setup I/O failure in an otherwise writable directory can destroy a healthy cache and force a full re-embed. A read-only directory does not necessarily permit deletion, so that particular audit example is not proof of successful removal.
- **Fix:** Limit destructive recovery to recognized corruption/incompatible-format cases; preserve and report other errors with their original context. Test a non-corruption setup failure as well as real corruption recovery.
- **If skipped:** Temporary storage problems cause unnecessary rebuilds and conceal useful diagnostics.
- **Sources:** CLI audit #8, with its read-only-directory example qualified.

### 13. Service-disabled picker repeatedly scans all history

**File:** `crates/smcli/src/cli.rs:534-548`, picker fallback near `:850-875`, semantic callback `:904-919`.

- **What it is:** Picker recents use the full export-style window query; each fallback semantic request reconstructs the corpus and engine.
- **Problem:** With `SMCLI_SERVICE=0`, 100k command rows and large native history, opening the picker sorts/materializes all rows, and repeated Tab presses reread and aggregate the same history. Historical latency numbers are not fresh Rust benchmarks.
- **Fix:** Use a bounded picker-specific recent query without changing full-history export semantics. Reuse the corpus/engine within a picker process, with an explicit refresh point if needed; benchmark on synthetic history.
- **If skipped:** The promised in-process fallback becomes progressively less interactive as history grows.
- **Sources:** CLI audit #5; Backend's fallback/rebuild performance limitations.

### 14. Legacy JSONL import is not idempotent

**File:** `crates/smcli/src/cli.rs:768-796`.

- **What it is:** The legacy path inserts records without an event ID or duplicate check, unlike Atuin import.
- **Problem:** Import the same backup twice and every command's run count doubles, skewing suggestions and search.
- **Fix:** Define import-only identity, such as a stable source-record ID or a carefully specified natural-key check, and test repeat import. Do not add a global uniqueness constraint that could reject legitimate repeated captured commands.
- **If skipped:** Retrying a migration silently inflates history statistics.
- **Sources:** CLI audit #9.

### 15. Model reload still blocks recording behind the shared state lock

**File:** `crates/smcli-service/src/service.rs:376-389`, search call near `:426`.

- **What it is:** Startup warm-up now loads outside `state`, but `current_engine` still constructs a changed/missing engine while the caller holds it.
- **Problem:** After a config/model change, or failed warm-up, a search loads the model while record/suggest requests wait on the same mutex.
- **Fix:** Build outside `state`, then publish after confirming the relevant config is still current, following the existing warm-up pattern.
- **If skipped:** Rare configuration changes or recovery recreate the original prompt/capture latency spike.
- **Sources:** Earlier review's model-lock issue, partially fixed; Fix report's explicit remaining case.

### 16. Linux service startup still inherits descriptors on musl

**File:** `crates/smcli-core/src/service_client.rs:114-149`.

- **What it is:** Descriptor cleanup exists on macOS and glibc, not musl.
- **Problem:** On musl, a shell's non-CLOEXEC FIFO or redirected fd can remain open for the service lifetime, preventing EOF or resource release. This is explicit in the current comment, not a proven failure on glibc.
- **Fix:** Supply a musl-safe close strategy or explicitly exclude that target from support. Retain the glibc implementation and document/test its runtime requirements; the added function requires glibc 2.34 or later.
- **If skipped:** musl users retain the descriptor leak; older glibc environments may not run the service binary.
- **Sources:** Earlier review's Linux descriptor finding; Fix report's remaining musl/platform limitation.

### 17. Binary distribution still has unresolved licensing materials and decisions

**File:** `THIRD_PARTY_NOTICES.md:47,1883`; `agents/plans/shipit-checklist.md:7-36`; root `LICENSE` absent.

- **What it is:** The root license, xdiff distribution route, and ten missing crate license texts remain unfinished.
- **Problem:** A public binary would ship before the repository's own release checklist is satisfied. The notices still list blake3, glidesort, heed, heed-traits, heed-types, lmdb-master-sys, number_prefix, objc2-core-foundation, objc2-core-services and taladb as metadata-only coverage.
- **Fix:** Finish the owner decisions and actual license materials, regenerate notices for the locked graph and currently bundled model, and verify artifact contents. Use the existing checklist rather than duplicating it.
- **If skipped:** Release readiness and distribution obligations remain unresolved. Treat this as a must-fix gate before public distribution, not a legal conclusion from this review.
- **Sources:** Backend owner decisions; Earlier review release backlog; Fix report release backlog.

## Nice to have

### 18. File-picker help promises Tab, but the actual toggle is Ctrl-A

**File:** `crates/smcli/src/shell_integration.rs:231,362,491`; `crates/smcli/src/picker.rs:569-577`, `:43`.

- **What it is:** All three hook comments promise whole-project search on Tab; Tab only initiates semantic history search. `Mode::Directory` also has no production assignment; folder filtering uses `kind`.
- **Problem:** Following the documented file-mode key does nothing. The unused mode makes the implementation harder to follow.
- **Fix:** Correct the comments to the existing Ctrl-A behavior and remove the unused mode if no caller needs it; avoid inventing another shortcut solely to match stale comments.
- **If skipped:** Misleading help and unnecessary mode branches persist.
- **Sources:** CLI audit #7.

### 19. Bash 3.2 silently skips picker key installation

**File:** `crates/smcli/src/shell_integration.rs:196`.

- **What it is:** The binding block is gated on Bash 4+, but capture is not.
- **Problem:** A user of macOS's bundled Bash gets recording without smcli picker keys or an explanation. Ctrl-R retains Bash's own behavior; it does not literally do nothing, and manual `smcli pick` remains available.
- **Fix:** Warn once that picker bindings require Bash 4+ and recording remains active; mention manual invocation or upgrading Bash.
- **If skipped:** An apparently successful installation leaves users unsure why picker keys do not work.
- **Sources:** CLI audit #10, with its “can't be opened” claim narrowed.

### 20. Picker geometry is fixed until it closes

**File:** `crates/smcli/src/picker.rs:496-498`, `:749-752`.

- **What it is:** A documented `shortcut:` creates a fixed viewport and discards non-key events.
- **Problem:** Resizing while picking keeps the original dimensions; shrinking can hide content.
- **Fix:** Handle resize and update viewport/layout, with a live terminal or PTY regression check.
- **If skipped:** Users must close and reopen the picker after resizing. This is an accepted limitation, not a newly introduced regression.
- **Sources:** Earlier review resize backlog; Fix report.

### 21. The context-cache test still has a 300 ms scheduling deadline

**File:** `crates/smcli-core/src/context_cache.rs:431-449`.

- **What it is:** The reader is now held on a channel, but the test still asserts the whole initial discovery/watcher call takes under 300 ms.
- **Problem:** A slow filesystem or heavily scheduled CI host can fail a correct nonblocking implementation. The fix report overstates the removal of the timing race: the old 600 ms sleep is gone, the deadline is not.
- **Fix:** Prove completion while the reader remains blocked using synchronization, with a generous watchdog only to avoid hanging a broken test; remove the narrow performance assertion.
- **If skipped:** Intermittent CI failures remain possible. No current failing run was established here.
- **Sources:** Earlier review's test robustness finding; Fix report #19, partially fixed by `5a43d1e`.

### 22. Project-search ranking still misses the preferred Swift test command

**File:** `crates/smcli-search/tests/ranking.rs:55,199`.

- **What it is:** A fixed project-search case expects `swift test --filter Lib` first.
- **Problem:** Today's `make ranking` still places it second behind `swift test`; project search MRR is 0.750/top-1 0.50 across two cases. The suite passes its regression floor.
- **Fix:** Add more representative project queries before tuning weights, then compare both targeted and aggregate results.
- **If skipped:** This measured quality limitation persists; it is not evidence of an overall regression.
- **Sources:** Earlier review ranking backlog; Fix report; re-run during this consolidation.

### 23. Large-tree grep remains partial and can exceed the intended budget

**File:** `crates/smcli-service/src/file_search.rs:124-131`; `crates/smcli/src/picker.rs:328-343`.

- **What it is:** Requests use a 1,000 ms fff budget and report a continuation offset as `partial`, but never resume it. The picker dispatches changed queries without a debounce/cancel mechanism.
- **Problem:** Users can receive only part of a large tree and wait on an obsolete scan before the next query runs. The earlier ~1.5 s/50k measurement and upstream zero-match budget issue were not rebenchmarked in Rust here.
- **Fix:** Keep the existing partial warning; verify the pinned fff budget behavior, enforce it for zero-match searches, coalesce obsolete work, and resume when full coverage is required. Choose an interactive budget based on measurement.
- **If skipped:** Content search remains laggy/incomplete on large trees. Do not describe it as silently truncated: partial reporting is already implemented.
- **Sources:** Backend grep limitation/owner decision; `agents/plans/fgrep-notes.md`; current service/picker spot-check.

### 24. Shell capture still has documented fidelity gaps

**File:** `crates/smcli/src/shell_integration.rs:64-65,90-95,287-288,423-424`; `crates/smcli-search/src/native.rs:76-99`.

- **What it is:** Bash records no pipeline statuses, excludes ambiguous ignoredups captures in PS0/bash-preexec modes, and does not interpret bare `&` in HISTIGNORE. Zsh checks HISTORY_IGNORE but does not consult other zshaddhistory exclusions. Fish timestamps have whole-second starts, and native Fish history is not enumerated.
- **Problem:** A repeated Bash command can be absent from smcli despite running, a zsh user exclusion can differ from smcli capture, and existing Fish-only history is absent from search until captured/imported separately. Subsecond Fish starts cannot be reconstructed from the current hook.
- **Fix:** Document the exact supported semantics; add targeted shell tests and extend adapters/native readers only where required. Preserve the conservative privacy choice when Bash cannot distinguish exclusion from duplication.
- **If skipped:** These compatibility/precision limits persist. They are not a reason to weaken filtering or modify native history files.
- **Sources:** Backend known limitations; current hook/native-reader checks.

### 25. Swift manifest discovery is still deliberately incomplete

**File:** `crates/smcli-core/src/context.rs:142`; `crates/smcli-core/src/ranking.rs` project candidate generation.

- **What it is:** Package.swift target extraction uses a regex for literal target declarations, not Swift evaluation.
- **Problem:** Computed names or nonliteral declarations are invisible, so project suggestions can omit valid targets. The old claim that every `name:` dependency becomes a target is no longer accurate; the regex is now constrained to target-like constructors.
- **Fix:** Retain and document the declarative subset, and test it. If full manifest support is required, use a bounded structured discovery path rather than widening the regex indiscriminately.
- **If skipped:** Some Swift projects have incomplete generated suggestions, without affecting recorded command availability.
- **Sources:** Backend's Swift script-candidate/parser limitation, narrowed to current code.

## Resolved since earlier reviews

The following rows classify the earlier issues that are not open findings. “No longer relevant” means the old claim describes removed architecture, a deliberate contract, or an unreproduced historical observation—not a newly verified implementation fix.

| Earlier issue                                                                                 | Classification                                                                     | Current evidence                                                                                                                                                                                                                                                    |
| --------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Uninstall copies a live database without its WAL                                              | Fixed                                                                              | `5a43d1e`; `scripts/uninstall.sh:60-64` requires sqlite3 and uses `.backup` before removal.                                                                                                                                                                         |
| Uninstall export exposes history and cannot be restored with private-mode checks              | Fixed                                                                              | `5a43d1e`; uninstall sets umask 077 and enforces directory 0700/files 0600, including existing outputs.                                                                                                                                                             |
| Folder pick in cwd inserts a root-relative path                                               | Fixed                                                                              | `5a43d1e`; `picker.rs:663` falls back to `./`; unit coverage remains.                                                                                                                                                                                               |
| Future timestamps make suggestion scores nonfinite                                            | Fixed                                                                              | `5a43d1e`; `suggestion_index.rs:125` clamps start to now before tallying; far-future test remains.                                                                                                                                                                  |
| Out-of-order background captures corrupt transitions; callers must honor new return contract  | Fixed                                                                              | `5a43d1e`; index rejects older session arrivals and `service.rs:293-295` invalidates on false; chronological rebuild/tests remain.                                                                                                                                  |
| Hooks pass a leading-hyphen buffer as flags; proposed allow_hyphen_values workaround          | Fixed / workaround no longer needed                                                | `5a43d1e`; all three hook calls now put `--` before the line. Direct CLI callers must likewise use `--`. The original replacement/data-loss severity was already withdrawn in the earlier review.                                                                   |
| Startup warm-up and corpus file/database reads hold the shared lock                           | Fixed in those paths                                                               | `5a43d1e`; warm-up builds before acquiring state; current_corpus reads native contents and command_stats outside it. Reload blocking remains #15; publication correctness remains #5.                                                                               |
| Corpus follow-up could unwrap a cache invalidated by another process                          | Fixed                                                                              | `5a43d1e`; current_corpus returns local copies, not a later state.corpus unwrap. Its version check is still insufficient for own writes (#5).                                                                                                                       |
| Pruned secrets remain in the search cache without warning                                     | Fixed for the reported path                                                        | `5a43d1e`; `cli.rs:675-680` rebuilds after removals and warns with `smcli reindex` on failure. This does not resolve the distinct over-deletion bug (#4) or promise secure erasure of all physical storage copies.                                                  |
| Negative/NaN/infinite idle variables panic                                                    | Fixed for reported inputs                                                          | `5a43d1e`; both service idle parsers filter nonfinite/negative values before constructing durations. No blanket claim about every extreme finite duration.                                                                                                          |
| Idle/shutdown runs libc exit handlers concurrently                                            | Fixed                                                                              | `5a43d1e`; `service.rs:160-165` unlinks its own socket then calls `_exit`; shutdown uses this path.                                                                                                                                                                 |
| Socket errors call thread-unsafe strerror                                                     | Fixed                                                                              | `5a43d1e`; `unix_socket.rs` uses io::Error display.                                                                                                                                                                                                                 |
| glibc spawn keeps inherited non-CLOEXEC descriptors                                           | Fixed in glibc code; execution unverified here                                     | `5a43d1e`; `service_client.rs:143-145` adds closefrom(3). Apple-only flag suggestion was invalid. musl remains #16.                                                                                                                                                 |
| Health duplicates index filename formatting                                                   | Fixed                                                                              | `5a43d1e`; `cli.rs:1054,1072` calls index_name().                                                                                                                                                                                                                   |
| Failed random read creates duplicate zero event IDs                                           | Fixed                                                                              | `5a43d1e`; `history.rs:176-185,248` propagates open/read errors.                                                                                                                                                                                                    |
| CLI and client disagree on home when HOME is absent                                           | Fixed for reported callers                                                         | `5a43d1e`; CLI and ServiceClient use shared config::home_dir, with passwd fallback.                                                                                                                                                                                 |
| File-index builder panic leaks pool capacity and blocks waiters                               | Fixed                                                                              | `5a43d1e`; file_search.rs catches builder unwind, then decrements building/removes pending/notifies.                                                                                                                                                                |
| shell-init silently replaces broken config with default keys                                  | Fixed                                                                              | `5a43d1e`; `cli.rs:208-216` emits a diagnostic and still supplies usable hooks.                                                                                                                                                                                     |
| Alternate screen remains entered on Terminal construction error; unused height argument       | Fixed for reported setup branch                                                    | `5a43d1e`; picker Err arm leaves alternate screen/disables raw mode; height removed. Resize is separate (#20).                                                                                                                                                      |
| Concurrent-writer test short-circuits waits and hides failing writer                          | Fixed                                                                              | `5a43d1e`; `scripts/test-shells.py:882-884` collects every wait result and reports args/status. Child stderr remains inherited, not separately captured.                                                                                                            |
| Service lock treats every error as another instance                                           | Fixed                                                                              | `5a43d1e`; main.rs matches WouldBlock separately and reports other errors.                                                                                                                                                                                          |
| shell_integration::render dead code warning                                                   | Fixed                                                                              | `102cacd` removed it; render_config is the live entry point.                                                                                                                                                                                                        |
| Historical Bash Ctrl-C test hang / wrong PROMPT_COMMAND expectation                           | Fixed in the historical harness; no current recurrence established                 | Backend records the corrections; current `scripts/test-shells.py` resets child SIGINT and exercises adapter variants. These are not new runtime findings.                                                                                                           |
| fff watcher not ready after initial indexing                                                  | Fixed in current integration                                                       | Backend correction retained in Rust: `file_search.rs:74` waits for scan and watcher readiness.                                                                                                                                                                      |
| GCD git-output-reader scheduling problem                                                      | No longer relevant                                                                 | Rust cutover `9b2e1c7`/`d050644` removed the Swift/GCD implementation; current context cache uses Rust worker/subprocess paths.                                                                                                                                     |
| Ranking fixture mismatched transition sessions                                                | Fixed historically; no current failing claim                                       | Backend records the fixture correction before baseline; today's ranking transition cases pass. The independent Swift project miss remains #22.                                                                                                                      |
| Swift unit/shell tests not executed on x86_64; old universal/C-wrapper build steps            | No longer relevant as current test targets                                         | `d050644` removed Swift sources; current Makefile builds the Rust workspace. Current architecture validation is still required before shipping.                                                                                                                     |
| “Linux is not supported” / historical Linux Fish prompt timeout                               | Unsupported statement obsolete; timeout unverified, not a confirmed current defect | Rust core/search/service support Linux and Makefile:42 supplies a Docker suite. No new Linux execution was performed. install.sh still intentionally gates its installer to macOS; do not infer a supported Linux installer from the runtime port. See Not checked. |
| Embedded model adds ~34 MB to both executables                                                | No longer relevant as an issue                                                     | AGENTS.md explicitly requires both executables to embed the model; current potion.rs loads bundled bytes. Model version/size changed, so the old exact number is not asserted.                                                                                      |
| Old binaries reject newer schema v7                                                           | No longer relevant as a defect                                                     | `history.rs:550-551` deliberately refuses future schemas rather than corrupting them; downgrade compatibility was not promised.                                                                                                                                     |
| Transition signals require reloading shell-init to export SMCLI_SESSION                       | No longer relevant as a defect                                                     | Current Bash/Zsh/Fish templates all export session identity. Existing shells must reload changed hooks by design.                                                                                                                                                   |
| One unexplained 963-second Bash duration                                                      | No longer relevant as an actionable finding                                        | No reproducible failing path or corrective commit established; retain as historical observation, not a confirmed present clock bug.                                                                                                                                 |
| Work was wholly uncommitted / commit the old 51-path change                                   | Historical state obsolete                                                          | Rust port and audit fixes are committed through HEAD 16c3200. Current pre-existing uncommitted changes are a different working-tree state; this review does not commit them.                                                                                        |
| `.build/` and retired F2LLM resource cleanup                                                  | No longer relevant                                                                 | Earlier review already verified the retired paths absent; Rust cutover removed that architecture.                                                                                                                                                                   |
| Aggregate ranking regressed                                                                   | No longer relevant / unsupported claim                                             | Earlier revalidation matched its baseline; today's passing run uses a changed 13-case search set. No equivalent-set aggregate regression was established.                                                                                                           |
| Swift parser treats dependency-only CSQLite as a target; candidates only ever use swift build | No longer relevant as stated                                                       | Current context.rs:142 matches target-like constructors; ranking generates test-filter candidates too, exercised by ranking.rs:55. Remaining regex limitations are #25.                                                                                             |
| Release checklist still says eight missing texts and Swift packages                           | Fixed documentation count/architecture wording                                     | Current shipit-checklist says ten Rust crates and Rust binaries. Actual licensing tasks remain #17.                                                                                                                                                                 |
| Imports bypass capture filter; project discovery called twice                                 | No longer relevant as reported defects                                             | Earlier audit explicitly excluded them: import behavior is deliberate, repeated discovery is a small optimization without a demonstrated failure. They are not promoted into findings here.                                                                         |

## Verdict

Do not treat the old audit-fix report as a clean bill of health. Fix #1–4 before encouraging upgrades or privacy cleanup; then address corpus publication and the search identity/strict-filter defects (#5–8). Finish the licensing checklist before public binary distribution. The remaining items are bounded reliability, performance and compatibility work, not justification for a redesign or additional dependencies. This consolidation created one review document and changed no implementation code.

## Not checked

No full `make test`, `make linux`, fresh release build, live resize exercise, new concurrency/commit-failure reproduction, large-tree benchmark, power-loss test, or release-architecture run was performed for this consolidation. The historical Linux Fish timeout is unresolved validation, not a reproduced current bug; Docker availability was not rechecked. Today's audits supplied some reproductions, but their results are attributed rather than claimed as newly run. `make ranking` was re-run successfully; it does not cover the newly reported races or marker failures. License materials were checked for repository completeness, not independently legally certified. No real user history was opened or modified. Pre-existing code and documentation changes were left intact; `make docs` is the required hand-written Markdown formatting step, not an implementation/test certification.
