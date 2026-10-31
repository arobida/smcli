# Codebase review and fff investigation — 2026-10-30

What this repo does: smcli captures shell history in SQLite, searches commands locally, and provides command/file/content pickers. Expected load: one desktop user with several shells sharing one history database and service, occasionally switching projects.

## Scope and evidence

Reviewed commit `90f13a21fe59c7df5ef81e3627ec762764b778e1`, including the existing uncommitted changes in `picker.rs` and `shell_integration.rs`. No implementation fixes, commits, or pushes were made. This file preserves the earlier chat audit as Scope A and adds the requested fff investigation as Scope B. The index previously pointed to a missing 2026-10-26 report; no historical report body was available to migrate.

Scope A's isolated `make test` passed: **198 Rust tests, zero failures, three ignored**, followed by CLI, shell capture, service, file/alias picker, and install/uninstall checks. Its separate reproductions still found the defects below. Scope B used an isolated `make release`, direct socket requests, a temporary upstream API probe, and a temporary test of the actual picker sorting function. The full suite was not repeated for this documentation-only addition.

## Scope A — whole-repository audit, preserved from the earlier chat report

These findings were reproduced in the preceding audit in this conversation. They have not been fixed. Numbers remain stable so follow-up work can name them.

### Must fix

#### 1. **Prune leaves rejected file-search queries in SQLite** (`crates/smcli/src/cli.rs:692`).

- **What this is:** File/grep selections store the typed query.
- **Problem:** Prune handles commands and command selections but ignores `file_selections`; a newly rejected `secret query` remained after pruning.
- **Fix:** Filter file selections in the same transaction before compaction.
- **If we skip it:** Privacy cleanup leaves excluded text stored.

#### 2. **Atuin re-import misses later deletions** (`crates/smcli/src/cli.rs:855`).

- **What this is:** UUIDs deduplicate imported records.
- **Problem:** `INSERT OR IGNORE` leaves a previously imported command visible after it is deleted in Atuin and re-imported.
- **Fix:** Update deletion state on UUID conflict.
- **If we skip it:** Deliberately deleted commands remain searchable.

#### 3. **Unicode crashes the new edit-path header** (`crates/smcli/src/picker.rs:1035`).

- **What this is:** The uncommitted header numbers path placeholders.
- **Problem:** Iterating byte offsets and slicing at each offset panics for `echo café @` (exit 101).
- **Fix:** Use character boundaries in both loops and add a Unicode regression case.
- **If we skip it:** Ordinary accented/non-Latin commands can crash path editing.

#### 4. **Home-abbreviated insertion paths do not expand** (`crates/smcli/src/shell_quoting.rs:17`; `crates/smcli-core/src/service_location.rs:102`).

- **What this is:** Results outside the current directory may use `~/repo/file`.
- **Problem:** Quoting produces `'~/repo/file'`; a real sibling completion then fails `test -f`.
- **Fix:** Use absolute paths for insertion and abbreviate only display text.
- **If we skip it:** Valid picks insert nonexistent literal-tilde paths.

#### 5. **Local file fallback follows directory cycles without a work budget** (`crates/smcli/src/cli.rs:1246`).

- **What this is:** A separate recursive walker handles service failure.
- **Problem:** Two directory links back to the current folder prevented the synthetic scan from returning within two seconds. No depth/entry budget or cycle detection exists.
- **Fix:** Reuse bounded traversal rules and avoid following directory symlinks.
- **If we skip it:** Service failure can cause runaway background scanning.

#### 6. **Fallback history picker loses older captures** (`crates/smcli/src/cli.rs:989`).

- **What this is:** Typing filters the captured-command picker.
- **Problem:** Fallback loads 50 rows; service mode returns all distinct commands. With 52 captures and no native history, the oldest selection succeeded with the service and failed without it.
- **Fix:** Supply the same distinct-command set in both paths.
- **If we skip it:** Older commands disappear when the service is unavailable.

#### 7. **Timestamp parser accepts impossible values and can panic** (`crates/smcli-core/src/time.rs:41`).

- **What this is:** Imported timestamps and time filters use this parser.
- **Problem:** February 31 and `+99:99` are accepted. Importing `2026-01-01T00:00:00+1é00` panics at a string boundary.
- **Fix:** Validate ASCII, dates, and timezone ranges before conversion.
- **If we skip it:** Imports can crash or silently store wrong dates.

#### 8. **Uninstall removes other databases' service sockets** (`scripts/uninstall.sh:146`).

- **What this is:** Each database has separate runtime files.
- **Problem:** Cleanup deletes every `service-*` file. Uninstalling database A removed running database B's socket in an isolated reproduction.
- **Fix:** Delete only the selected database's runtime files.
- **If we skip it:** Removing one instance disrupts another.

#### 9. **Fallback folder picker cannot find empty directories** (`crates/smcli/src/cli.rs:1186`).

- **What this is:** Folder-only mode supports `cd` and similar commands.
- **Problem:** Fallback derives directories from files, after truncating files to 100. Empty folders never appear.
- **Fix:** Collect directory entries directly before limiting results.
- **If we skip it:** Folder picking changes behavior or fails without the service.

### Should fix

#### 10. **Directory selections are rejected instead of remembered** (`crates/smcli-service/src/file_search.rs:355`).

- **What this is:** Picks affect later file ranking.
- **Problem:** Normal directory hits end in `/`; validation rejects their empty final component. `folder/` returned `bad_request`.
- **Fix:** Normalize the trailing slash before validation/storage.
- **If we skip it:** Repeated directory choices never improve ranking.

#### 11. **Service suggestions ignore the caller's ranking switches** (`crates/smcli-core/src/service_client.rs:112`).

- **What this is:** `SMCLI_RANKING_OFF` disables signals.
- **Problem:** Only search forwards it. An existing service still suggested a project script with `SMCLI_RANKING_OFF=script`; fallback excluded it.
- **Fix:** Forward the setting for suggest/pick requests too.
- **If we skip it:** Ranking depends on which shell started the service.

## Scope B — fff implementation, indexing, storage, ranking, grep, and resources

### What is actually integrated

Only `smcli-service` links `fff-search` 0.11.0, pinned to commit `95fd777c2529fc7b4d7572dabff64cc07268f2c5`, with `definitions` enabled. The upstream default `ripgrep` feature selects the pure-Rust file walker/glob backend; this build does not enable the optional Zig/zlob backend. This is an SDK integration, not the fff Neovim plugin or a spawned `rg` command.

`FileSearch` owns a pool of `SharedFilePicker` instances. Git projects normally use their worktree root; scoped file/grep requests can create a smaller root or reuse a ready ancestor index. Defaults: **four cached roots**, **600 seconds idle per root**, eviction checked by requests and the service's **15-second maintenance loop**. Service idle exit defaults to 1,800 seconds. In-flight references may outlive pool eviction, and concurrent builds can coexist with cached indexes: four is not a hard process-memory cap. These values are controlled by existing `SMCLI_FFF_MAX`, `SMCLI_FFF_IDLE`, and `SMCLI_SERVICE_IDLE` variables.

Construction waits up to ten seconds for file scanning, then up to ten seconds for watcher readiness. The CLI normally waits ten seconds for file requests before invoking its separate local fallback. A large first scan can therefore overlap fallback work. This overlap is a source-level observation, not a reproduced timeout in these fixtures.

References: `crates/smcli-service/Cargo.toml`, `src/file_search.rs:65-165`, `src/service.rs:85-89,149-157`; `crates/smcli/src/cli.rs:1080-1093`.

### Indexing and storage

fff keeps path records, directories, metadata, Git state, chunked path storage, and content-filter bitsets **in memory**. The content filter tracks pairs of characters (bigrams), so grep can reject files that cannot contain a query before opening them. It is a candidate filter, not a database of full matching lines. Matching candidate contents are still read from source files.

smcli enables content indexing in repositories. For scoped non-repository searches, its policy disables it at home, direct children of home, and outside home, enabling it deeper under home. `complete` and unscoped file operations instead request content indexing unconditionally. A ready ancestor instance is reused with its existing settings. Upstream separately refuses indexing `/` and the exact home directory by default.

Upstream ignores excluded files through its walker, does not follow symlinks by default, and watches changes. Content indexing covers eligible text files up to **2 MiB**; grep permits eligible files up to **10 MiB**. Binary detection and ignored-file rules limit coverage. A file being absent from grep is not proof it is absent from disk.

`SharedFrecency::default()` holds no access-history tracker. smcli opens **no fff LMDB store**. It persists selected paths and typed queries in its own SQLite `file_selections` table. TalaDB under `history.db.search` is the separate command-history index, not fff's file/content index.

Measured disk changes after indexing and repeated file/grep requests: **zero additional persistent fff bytes**. Each isolated service had a 36 KiB SQLite database and 32 KiB shared-memory file before and after those requests; selection requests can grow SQLite. No full file-index cache survives service restart, so each fresh service rescans its requested roots.

Read-only size check of the installed paths on this machine (point-in-time values, no history contents read):

| Item                              | Logical size | Allocated disk space | Meaning                                      |
| --------------------------------- |-------------:|---------------------:| -------------------------------------------- |
| `~/.local/share/smcli/history.db` |     7.23 MiB |             8.04 MiB | Captures and selections; not exclusively fff |
| `history.db.search/`              |    68.46 MiB |            48.21 MiB | TalaDB command search, not fff               |
| `~/.local/bin/potion-shell-16m/`  |    31.96 MiB |            31.98 MiB | Shared command embedding model, not fff      |
| Installed `smcli-service`         |    17.98 MiB |            17.98 MiB | Whole executable, not a standalone fff index |

These are selected path sizes, not a complete installation total or an audit of database/WAL contents. No running installed service was present in the process listing at that check.

### Memory limits and lifecycle

Upstream's automatic **per-index content-mapping cache** permits up to 512 MiB/30,000 files at at most 10,000 indexed files; 256 MiB/10,000 cached files at 10,001–50,000; and 128 MiB/5,000 above 50,000. These limits do not cap all process memory: metadata, the model, content-filter construction, query results, and concurrently alive instances are extra. smcli supplies no explicit byte budget.

`enable_mmap_cache=false` disables prewarming, not on-demand caching during grep. On this ARM64 Mac, files below 16 KiB use reads instead of persistent mappings. A 200-file fixture totaling 50 MiB grew RSS from 146.0 to 196.3 MiB after grep, while physical footprint barely changed: clean file-backed pages and private memory are different costs.

The pinned bigram compressor intentionally discards very rare columns and columns present in at least 90% of files. Repetitive source therefore gives poor filtering even after indexing completes. The direct upstream probe confirmed readiness and still searched all 50,000 repetitive files for rare/missing queries. This is a measured design tradeoff, not a claim that indexing was disabled.

The four-root test retained four instances and evicted one when a fifth arrived. Physical footprint went from 134.0 to 159.5 MiB while holding 10k repetitive, 50k repetitive, 50k varied, and checkout indexes. RSS fell sharply during that run as macOS reclaimed/compressed pages; use footprint alongside RSS. With a one-second test idle setting and a 16-second wait, the instance list became empty, but RSS stayed around 151 MiB. Eviction closes index/watch ownership; it does not promise the allocator immediately returns all pages. This observation alone does not establish a leak.

### Ranking: four separate decisions

1. **fff filename ranking:** SIMD fuzzy/typo matching, filename/path alignment bonuses, Git status and recent-commit bonuses, and optional access/modification history. Exact filename matches receive a 40% base-score bonus; modified files receive 15%. smcli supplies neither a current file nor an fff access tracker, so current-file/distance/access-history signals are unavailable. Git recency is enabled with ten qualifying commits and at most fifty files per qualifying commit. In this integration, modification frecency also depends on the absent tracker.
2. **smcli's file-service rerank:** Nonblank files searches retrieve up to `min(limit*3,500)` candidates. Near-best, non-exact matches can move up by at most fifteen positions from SQLite selection history: five positions per decayed pick, capped at three picks, and requiring at least 80% of the best fff score. Exact matches receive an additional priority offset. Completion has a similar rerank and can search directories directly. Nonblank `files` uses file-only fuzzy search; blank `files` uses mixed files/directories.
3. **smcli's picker rerank:** `suggested_first` then moves picked/suggested paths containing the query ahead of other paths. That can undo the service's exact-first ordering (finding 16).
4. **Command-history ranking:** fff does not embed or own history search. It can supply ready-index existence checks, Git activity, identifier/definition evidence, and query expansion to the separate command-ranking code. Identifier and expansion probes have 20 ms budgets; discovered test commands use five grep patterns with 100 ms each and a watcher-invalidated/30-second cache. The test-discovery loop can read the same full source file once per matching line before deduplicating commands; this was inspected but not separately benchmarked as a defect.

Blank-search stars are primarily Git-derived in this integration, not evidence that smcli knows which files an editor opens most often. The absent fff access tracker and the blank branch's removal of `picked` tags matter (finding 15).

### Grep features and limits

The public service operation uses **literal text**, smart case (uppercase in the query makes it case-sensitive), fff's file/path/type/exclusion query parsing, and definition classification. A probe of `a.*b` matched literal `a.*b`, not `axxxb`; `mixed` matched `MiXeD`, while `MIXED` did not. Upstream supports regex, fuzzy content search, context lines, external abort flags, and resumable file offsets, but smcli does not expose/select those modes/options here. A parsed filename constraint can scope a query, but the picker's separate single-file `request.path` is applied too late (finding 12).

Current wrapper settings: 10 MiB file cap, 1,000 ms enforced search budget including zero-match queries, definition classification on, and **unlimited matches per individual file**. fff finishes a file before applying its soft page limit; that is unsafe with the wrapper's unlimited per-file setting (finding 13). Time budgeting does not bound result construction/JSON serialization or a single blocking file operation.

The service forwards a partial flag from `next_file_offset` but drops the actual offset and searched/eligible counts. The picker cannot continue the remaining page. A broad query may display only the first page in fff's usage/mtime file order, not every matching file or globally best definition. Definition markers annotate lines; smcli does not ask fff to sort all definitions globally first.

The picker already has a **100 ms debounce**, one in-flight file worker, and stale-reply rejection. It does not cancel an in-flight grep when the user changes the query. A slow obsolete query can still delay the useful one. Empty grep waits for input rather than scanning everything.

### Measured performance

Release build on MacBookPro18,3, macOS 27.0.1, 16 GiB RAM, eight logical CPUs/six performance cores. Fresh services and temporary homes/databases; no real history. Source creation warmed the OS cache: **cold below means fresh service/index, not a cold physical disk**. File/blank batches used twenty requests; each grep batch used ten. Timings include Unix-socket exchange and JSON handling. Sampled memory every 20 ms; sampled peaks can miss shorter spikes.

| Dataset                                | First files request | Warm filename median / p95 | Rare grep median / p95 | Missing grep median / p95 | Added RSS after indexing |
| -------------------------------------- |--------------------:|---------------------------:|-----------------------:|--------------------------:|-------------------------:|
| 10k repetitive 1 KiB files (~9.8 MiB)  |              161 ms |             3.74 / 9.34 ms |       74.77 / 78.57 ms |          76.50 / 77.27 ms |                  7.4 MiB |
| 50k repetitive 1 KiB files (~48.8 MiB) |              750 ms |             5.12 / 7.33 ms |     778.74 / 821.38 ms |        775.86 / 780.25 ms |                 22.6 MiB |
| 50k varied-template files (~57 MiB)    |            1,483 ms |             5.51 / 6.91 ms |       23.99 / 24.31 ms |           9.29 / 10.09 ms |                 23.0 MiB |
| Isolated smcli checkout                |             71.6 ms |             1.41 / 6.48 ms |  `FileSearch`: 2.01 ms |                   0.23 ms |                  5.4 MiB |

For repetitive fixtures, every file shared most content; for varied fixtures, one hundred deterministic identifier templates changed bigram distribution. These deliberately bracket content selectivity. They are not universal latency guarantees. Checkout includes its copied `.git` and the release artifacts subject to the walker's ignore rules; the benchmark does not claim a hand-counted tracked-file-only corpus.

At 50k repetitive files, ten rare searches consumed **44.62 CPU-seconds in 7.78 wall-seconds**, about **573% CPU** (100% means one core). Index creation/settling consumed about **3.11 CPU-seconds**. Eight concurrent expensive requests completed in about 1.02 seconds, but individual latency reached 1.02 seconds and budgeted work may be partial. Idle usage was about **0.0044 CPU-seconds over three seconds** (~0.15% of one core). The pinned library uses a six-thread search pool on this Mac and a four-thread background pool; parallel throughput can still be an expensive foreground battery/CPU burst.

Whole-service sampled RSS peaks were 99.4 MiB (10k repetitive), 120.6 MiB (50k repetitive), and 173.6 MiB (50k varied). Startup baselines varied from 87.7 to 142.5 MiB, so absolute values include model/runtime/library residency and must not be attributed entirely to fff. The before/after deltas within each service are more useful than subtracting different runs.

CPU values use `proc_pid_rusage` with Mach clock conversion `125/3`, calibrated against `ps`: a busy subprocess measured 1.725 CPU-seconds versus `ps` 1.73 seconds. Raw data, footprint values, CPU totals, storage snapshots, and source/binary identities are in [measurements.json](evidence/fff-2026-10-30/measurements.json). The reusable macOS harness is [profile-macos.py](evidence/fff-2026-10-30/profile-macos.py); run it with an absolute `smcli-service` path. It recreates the repetitive 10k/50k cases. The varied, dense, medium-file, and pool scenarios are described above and retained as raw measurements; they are not all generated by that small harness. The varied-fixture rare marker was checked to return exactly one hit; an initial truncated-marker fixture was replaced before finalizing the table. The direct API diagnostic is in [upstream-probe.txt](evidence/fff-2026-10-30/upstream-probe.txt), and the temporary actual-function test is in [ranking-probe.rs](evidence/fff-2026-10-30/ranking-probe.rs).

### Additional findings — continue numbering from Scope A

#### 12. **Scope constraints are applied after the result limit — confirmed defect** (`crates/smcli-service/src/file_search.rs:196-221,259-267`).

- **What this is:** A ready parent index is reused for a subfolder, and grep can request one specific file.
- **Problem:** fff searches/limits the broader set before smcli removes other paths. A fixture with thirty older inside matches and eighty newer outside matches returned three hits from a child-only index, but zero after warming the parent, for both files and grep. Single-file grep also returned zero while its target contained the text.
- **Fix:** Apply the scope/file constraint inside fff before ranking/pagination, or use a scope-specific index. Do not fix this by increasing an arbitrary overfetch multiplier.
- **If we skip it:** Valid matches disappear depending on index warm-up history and unrelated files.

#### 13. **Grep result limits do not bound output or memory — confirmed defect** (`crates/smcli-service/src/file_search.rs:213-222,379-381`).

- **What this is:** Interactive requests normally ask for 100 matches.
- **Problem:** The wrapper uses unlimited matches per file, while fff's page limit finishes the current file. One 1.3 MB source file returned **100,000 matches for limit=100**, about **14.39 MiB JSON**, taking **1.42 seconds**. Service RSS rose from **146.5 to 202.0 MiB**. Bigger allowed files can exceed the client's 64 MiB response allowance.
- **Fix:** Set a finite per-file bound, enforce a hard response-count bound, and mark truncation clearly. Apply equivalent bounds to identifier/test-discovery probes using unlimited per-file matches.
- **If we skip it:** A single generated source file can cause large allocations, delayed UI results, and rejected oversized replies.

#### 14. **Scoped picks are stored under the wrong root — confirmed defect** (`crates/smcli/src/cli.rs:1145-1152`; `file_search.rs:358-365`).

- **What this is:** Scoped hits are relative to the searched subfolder.
- **Problem:** The picker omits `scope=true` when saving its choice. A `needle.rs` choice from `repo/sub` was stored as `(repo, needle.rs)`, the identity of the top-level file, instead of `(repo/sub, needle.rs)`.
- **Fix:** Preserve the scope flag/identity from search through selection.
- **If we skip it:** Ranking history boosts the wrong file and fails to remember the selected one.

#### 15. **Blank file ranking ignores recorded pick frequency — confirmed integration gap** (`crates/smcli-service/src/file_search.rs:223-252,94`; `service.rs:96-100`).

- **What this is:** The initial file list is presented as frequently/recently useful files.
- **Problem:** The blank branch neither applies SQLite selection scores nor retains `picked` provenance; the fff access tracker is absent. Four repeated choices changed neither blank ordering nor its picked markers in the fixture. External editor file-open history is not collected here.
- **Fix:** Use the existing SQLite pick scores for blank ranking and describe its actual signals. No second LMDB history store is needed.
- **If we skip it:** The initial list does not learn from repeated smcli file choices as its surrounding descriptions imply.

#### 16. **Picker sorting can undo exact-first file ranking — confirmed function-level defect** (`crates/smcli/src/picker.rs:890-896`).

- **What this is:** The service protects exact matches, then the picker performs another sort.
- **Problem:** Service output placed exact `needle.rs` before picked `xneedle.rs`; calling the actual `suggested_first` function reversed them for query `needle.rs`. A temporary test in the isolated snapshot passed with that reversal. The live-picker attempt did not establish this case end-to-end, so the claim is limited to the connected sorting function.
- **Fix:** Preserve service order or carry enough score information to keep exact matches first in the UI sort.
- **If we skip it:** The user-visible order can contradict the service's documented exact-match protection.

### Historical notes that are now stale

`agents/plans/fgrep-notes.md` is explicitly dated 2026-10-22 and describes the older C wrapper. Its claims that only filenames are indexed, zero-match budgets cannot be enforced, and there is no debounce no longer describe this implementation. The current pinned dependency has a content bigram filter; the wrapper enables `enforce_time_budget`; the picker debounces for 100 ms. Its quoted historical latency numbers must not be presented as current measurements. This audit records the correction here without silently rewriting that historical document.

### Pinned upstream implementation references

All external implementation claims were checked against the local Cargo checkout at the pinned SHA, not the current upstream branch:

- [Picker options, index ownership, and defaults](https://github.com/dmtrKovalenko/fff/blob/95fd777c2529fc7b4d7572dabff64cc07268f2c5/crates/fff-core/src/file_picker.rs).
- [Content cache budgets](https://github.com/dmtrKovalenko/fff/blob/95fd777c2529fc7b4d7572dabff64cc07268f2c5/crates/fff-core/src/types.rs).
- [Bigram construction/compression](https://github.com/dmtrKovalenko/fff/blob/95fd777c2529fc7b4d7572dabff64cc07268f2c5/crates/fff-core/src/index/bigram_filter.rs).
- [Filename scoring](https://github.com/dmtrKovalenko/fff/blob/95fd777c2529fc7b4d7572dabff64cc07268f2c5/crates/fff-core/src/score.rs).
- [Grep options and soft pagination](https://github.com/dmtrKovalenko/fff/blob/95fd777c2529fc7b4d7572dabff64cc07268f2c5/crates/fff-core/src/grep/types.rs).
- [CPU thread pools](https://github.com/dmtrKovalenko/fff/blob/95fd777c2529fc7b4d7572dabff64cc07268f2c5/crates/fff-core/src/parallelism.rs).

## Verdict and limits

**Verdict:** Keeping fff inside the service is sound, and warm filename lookup is fast. Fix constraints-before-pagination and the unbounded per-file grep response before tuning small performance details. The principal resource risks are content-dependent CPU bursts, per-index mapping budgets that are not a global cap, and result allocation.

**Not checked:** Linux runtime (Docker unavailable in the earlier audit), cold-disk/network-filesystem behavior, multi-hour leak/battery/energy measurements, allocator behavior across hardware, or every upstream implementation path. Synthetic file contents, untracked temporary Git fixtures, OS cache state, and concurrent machine activity affect these numbers. No dependency-vulnerability or license re-audit was performed in Scope B. No fixes were applied.
