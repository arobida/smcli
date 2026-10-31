# Codebase review fixes plan

## Scope

Fix findings 1–16 from [Codebase review and fff investigation (2026-10-30)](../reviews/codebase-review-2026-10-30.md). Nothing else: no fff tuning, no new stores, no Linux runtime work. `agents/plans/fgrep-notes.md` stays as written (the review already records its corrections).

Pushback, settled before coding:

- **#12, #13, #11, #3** — investigated in advance; see [Pre-investigation](#pre-investigation-2026-10-31).
- **#15** — reuse the SQLite pick scores that the nonblank rerank already reads. No fff access tracker or LMDB.

## Pre-investigation (2026-10-31)

Source-read against commit `fd51b76` and the pinned fff checkout (`~/.cargo/git/checkouts/fff-8062f806af4ee2de/95fd777`). Nothing was built or run; the regression tests in WP3, WP2 and WP9 prove each fix.

### #12 — confirmed; fff can filter before the limit

- **Cause:** `scoped_index` (`file_search.rs:142`) reuses a ready ancestor index and returns a `prefix`. `search` then asks fff for `limit` (files), `limit*3` (nonblank) or a `limit`-match page (grep) over the whole ancestor, and only afterwards `scope_relative` (`:194`) drops paths outside `prefix` and paths other than `request.path`. Outside hits use up the page, so inside hits never arrive.
- **fff support:** `FFFQuery.constraints` is a public `Vec<Constraint>` (`fff-query-parser/src/lib.rs:57`, re-exported by `fff_search`). A `Constraint::Glob` is applied before scoring and pagination: files `score.rs:611`, dirs `score.rs:403`, mixed merges both then paginates (`file_picker.rs:1263`), and grep prefilters files before searching (`grep/grep.rs:284`, `grep/prefilter.rs`). The default `ripgrep` feature compiles it with `globset::Glob::new`, anchored to the whole relative path.
- **Fix:** after parsing the query, push `Constraint::Glob` built from the escaped prefix:
  - a scoped search on a reused ancestor uses `"<escaped prefix>**"`;
  - a single-file grep uses `"<escaped prefix><escaped path>"`, the exact relative path.
    Apply it on all three branches: grep, blank mixed and nonblank files. The glob `String` must be declared before the parsed query, because the query borrows it.
- **Escaping:** prefix each of `\ * ? [ ] { }` with `\`. On Unix `globset` honors backslash escapes by default; smcli ships only on macOS and Linux. Do not add `globset` as a dependency for its `escape` function. A pattern that fails to compile matches nothing (`index/constraints.rs:540`), so a bad escape returns no results instead of leaking outside paths.
- **Why not `FilePath` or `PathSegment`:** `FilePath` matches any suffix at a `/` boundary (`x/sub/a.rs` matches `sub/a.rs`), and grep silently drops it when it yields zero files (`prefilter.rs:35`). `PathSegment` matches anywhere in the path. Neither is an anchored scope.
- **Keep the `scope_relative` post-filter** as a safety net. When a constrained grep finds nothing, fff retries the raw query as literal text with only `FilePath` constraints kept (`grep/grep.rs:223-245`), which drops our glob. The post-filter still removes outside hits, so the cost is a possible empty result only when the query also contained operator-like tokens (`!=`, `*.rs`). This is acceptable and should be noted in a one-line comment.
- **Not chosen:** scope-specific indexes (skipping the ancestor reuse) would add a second full scan and more memory per subfolder. The glob costs one path pass per query.
- **Test:** the review's fixture (30 older inside matches, 80 newer outside matches, parent index warmed first) returns the inside hits for files and grep, and single-file grep returns its target's line.

### #13 — confirmed; three call sites

- fff's `max_matches_per_file: 0` means unlimited (`grep/grep.rs:129`), and `page_limit` is soft: fff finishes the current file, so a page can hold `page_limit + max_matches_per_file - 1` matches (`grep/types.rs:254`). Our three call sites all pass `0`.
- **`file_search.rs:381` (picker grep):** set `max_matches_per_file` to `limit`, then `truncate(limit)` the hits and set `partial = Some(true)` whenever anything was cut. That bounds the reply at 500 hits (the protocol's maximum `limit`).
- **`test_candidates.rs:111`:** set `max_matches_per_file: 1`. The loop only needs each matching file once: it re-reads the whole file and runs `test_names` on it for every hit, so this also removes the repeated full-file reads the review noted.
- **`evidence.rs:148` (identifier probe):** set a finite cap such as `32`. It only needs to know whether each name is seen and whether it is defined; `page_limit: 256` already caps the page.

### #11 — confirmed; client-only, one condition

- The service applies `request.ranking_off` for suggest and pick through `ranking_context` (`service.rs:279`, used by `ranked` at `:362`), and it already has a test for it (`service.rs:779`).
- The client fills `ranking_off` only when `request.op == Operation::Search` (`service_client.rs:112`). Move the `ranking_off` line into its own `matches!(request.op, Operation::Search | Operation::Suggest | Operation::Pick)` block and leave `shell_path` as search-only.
- The field already exists in the v3 request fixtures, so there is no version bump or fixture change.

### #3 — confirmed; one closure, two loops

- In `edit_files_header` (`picker.rs:1036` and `:1042`), `is_placeholder` slices `s[i..]` for every byte offset in `0..len()`. Any multibyte character before the end (`echo café @`) panics on a non-boundary slice.
- **Fix:** iterate `line.match_indices('@')` and `text.match_indices('@')`, which only yield char boundaries. The `s[i + 1..]` slice is safe there, because `@` is one byte.
- **Test:** add `echo café @` and `mv é @ @` cases to the existing `files_edit_header` test.

## How agents run

Sessions are opened directly with the kero skill (not the ateam script), one pane per agent, all in the shared checkout. Each agent owns specific files for a wave, so lanes never edit the same file at the same time.

| Role         | Agents                              | Rule                                                                                                                                                        |
| ------------ | ----------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Implementers | pikcode, piluna, pidflash, pigrok46 | Write the fix and its regression test. **Run no tests and no builds**: stop when the diff is ready and report the changed files and the new test names.     |
| Reviewers    | piterra, pisol, pigrok47            | Review diffs against the finding, then run only the slices listed at each gate. A failure goes back to the implementer, and only the failed slice is rerun. |
| Commits      | pigrok45                            | One commit per work package, using `agents/reference/commits.md`. Only when the user asks.                                                                  |

Test economy:

- One `make build` per gate (piterra), then the focused slices below. Nothing runs between gates.
- Slices: `make test-rust PKG=<crate> FILTER=<name>`, `test-cli`, `test-picker`, `test-service`, `test-install`.
- One full `make test` at the very end. `make ranking` is not needed: no work package changes command ranking.

## Wave 1 — four lanes in parallel

| WP  | Findings                                           | Implementer | Files owned                                             | Regression test                                                                                                               |
| --- | -------------------------------------------------- | ----------- | ------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| 1   | #7 timestamp parser                                | pidflash    | `smcli-core/src/time.rs`                                | Feb 31, `+99:99` rejected; `2026-01-01T00:00:00+1é00` returns an error, no panic                                              |
| 2   | #11 ranking switch                                 | pidflash    | `smcli-core/src/service_client.rs`                      | Suggest/pick requests carry `ranking_off`                                                                                     |
| 3   | #3 Unicode header, #16 exact-first                 | piluna      | `smcli/src/picker.rs`                                   | `echo café @` header; exact `needle.rs` stays ahead of picked `xneedle.rs`                                                    |
| 4   | #1 prune, #2 Atuin, #6 fallback history            | pikcode     | `smcli/src/cli.rs` (prune, import, history fallback)    | Rejected file query gone after prune; deleted Atuin UUID hidden on re-import; 52nd-oldest capture found without the service   |
| 5   | #10 dir picks, #15 blank ranking, #14 service half | pigrok46    | `smcli-service/src/file_search.rs`, `service.rs:96-100` | `folder/` selection stored; blank list reorders after picks and keeps `picked`; scoped pick stored as `(repo/sub, needle.rs)` |

Fix notes:

- **WP1:** check ASCII first, then the day for the month (leap years), hours 0–23 and minutes 0–59 for the offset.
- **WP2:** see [#11](#11--confirmed-client-only-one-condition).
- **WP3:** for #3, see [#3](#3--confirmed-one-closure-two-loops). In `suggested_first` (`:890`), exact filename matches keep their place ahead of the picked/suggested move.
- **WP4:** prune filters `file_selections` in the same transaction, before compaction. Import uses `ON CONFLICT(uuid) DO UPDATE` on the deletion state. The fallback calls the same distinct-command query the service uses, not a 50-row load.
- **WP5:** trim a trailing `/` before validation and storage. The blank branch applies the existing pick score and keeps the `picked` provenance.

**Gate 1 (piterra):** review WP1–5. `make build`, then:

- `make test-rust PKG=smcli-core FILTER=time`
- `make test-rust PKG=smcli-core FILTER=service`
- `make test-rust PKG=smcli`
- `make test-rust PKG=smcli-service FILTER=file_search`
- `make test-cli`, `make test-picker`, `make test-service`

pisol also reviews WP5 (ranking semantics).

## Wave 2 — four lanes in parallel

| WP  | Findings                                           | Implementer | Files owned                                                             | Regression test                                                                                                                                                                                    |
| --- | -------------------------------------------------- | ----------- | ----------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 6   | #5 cycle-safe walk, #9 empty dirs, #14 client half | pikcode     | `smcli/src/cli.rs` (file fallback, selection request)                   | Two dir symlinks back to the cwd return within budget; empty folder listed in folder mode; scoped pick sends `scope=true`                                                                          |
| 7   | #4 home-abbreviated insertion                      | piluna      | `smcli/src/shell_quoting.rs`, `smcli-core/src/service_location.rs`      | A `~/repo/file` hit inserts an absolute path that passes `test -f`; display still shows `~`                                                                                                        |
| 8   | #8 uninstall sockets                               | pidflash    | `scripts/uninstall.sh`                                                  | Uninstalling database A leaves running database B's socket (extend `test-install`)                                                                                                                 |
| 9   | #12 scope before limit, #13 grep bounds            | pigrok46    | `smcli-service/src/file_search.rs`, `evidence.rs`, `test_candidates.rs` | 30 inside / 80 outside fixture returns the inside hits with the parent warm, for files and grep; single-file grep finds its target; a 100k-match file returns at most `limit` hits, marked partial |

Fix notes:

- **WP6:** use `DirEntry::file_type()`, which does not follow symlinks, so directory links are skipped. Add a depth and entry budget, and collect directories as entries before the limit, not derived from files.
- **WP7:** keep absolute paths for insertion and abbreviate only the display text. If this needs a new `FileHit` field, the protocol fixtures change and pisol approves the change first.
- **WP8:** derive the selected database's runtime file names the same way `service_location.rs` does, and delete only those.
- **WP9:** follow [#12](#12--confirmed-fff-can-filter-before-the-limit) and [#13](#13--confirmed-three-call-sites) in the pre-investigation.

**Gate 2:** reviews by pisol (WP7, WP9), pigrok47 (second review of WP9 only) and piterra (WP6, WP8). Then piterra runs `make build` and:

- `make test-rust PKG=smcli`
- `make test-rust PKG=smcli-core FILTER=location`
- `make test-rust PKG=smcli-service`
- `make test-picker`, `make test-service`, `make test-install`

## Wave 3 — close out

1. piterra: one full `make test`. Each failure goes to the WP owner, and only that slice is rerun.
2. piterra: `make fresh`, and relay its `eval` line to the user.
3. piluna: mark each finding fixed in the review doc through iwe, then run `make docs`.
4. pigrok45: write commit messages per WP, only when the user asks.
