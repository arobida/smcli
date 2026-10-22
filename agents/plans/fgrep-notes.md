# fgrep: content search on large trees

**Naming:** "fgrep" means smcli's fff content search: the service's `grep` op, `FilePicker::grep` in
`fff-search`, and the picker's grep mode (Ctrl-T). It is not the `grep`/`fgrep` command-line tools.
Renaming the code identifiers is optional and has not been done.

**Status (2026-10-22):** the Rust port calls `fff-search` directly. Partial reporting is implemented: the
service sets `partial` from `next_file_offset`, and the picker shows "partial grep results · type more to
narrow". Still open: enforce the budget on zero-match searches, debounce or cancel keystrokes, resume from
the offset, and pick an interactive budget. The measurements below were taken with the former C wrapper
before the Rust port; the fff engine and the scaling behavior are the same, but the numbers were not
re-measured.

## How it works today

- The picker (`crates/smcli/src/picker.rs`) sends a `grep` request when the query changes and no request
  is in flight. Replies are checked against the current query, so a stale reply is discarded, but there is
  no debounce: each settled keystroke starts a full scan.
- `FileSearch::search` (`crates/smcli-service/src/file_search.rs`) calls `FilePicker::grep` with a 1000 ms
  budget and reads `next_file_offset` to mark the response partial.
- fff's grep reads file contents on every call. Only the file list is indexed, not the contents. The
  service holds the index read guard for the call.

## Measured (`scripts/bench-baseline.py`, one rare token, warm median)

| Tracked files | fgrep cold | fgrep warm |
|---|---:|---:|
| 10,000 | 100 ms | 102 ms |
| 50,000 | 1,020 ms | 1,488 ms |

The time grows roughly linearly with file count, because every query reads every eligible file.

## Why it is over budget, and what "partial" means

1. **The budget doesn't stop a search that has no matches yet.** fff only enforces `time_budget_ms` once
   it has matches. A rare query therefore scans the whole tree. That explains 1.5 s at 50k files against
   a 1 s budget.
2. **When the budget does fire, the result is cut off.** fff returns `next_file_offset` (non-zero means
   unsearched files remain), `total_files_searched` and `filtered_file_count`. The service now reports
   `partial`, but it does not resume from the offset, and the picker only shows a generic partial label.
3. **Typing still starts one full scan per settled keystroke.** The worker keeps the draw loop responsive,
   but there is no debounce and no cancellation, so results lag behind typing and scans queue up. At 50k
   files, a 6-character query can keep the picker busy for several seconds.

Correctness is unaffected (right file, line and column; `scripts/test-service.py` checks it at 50k files).

## Options, cheapest first

1. **Report partial results — done.** The service returns `partial`, and the picker shows a partial label.
   A searched/total count would still be more useful.
2. **Enforce the budget:** make zero-match searches respect `time_budget_ms` too, so no query takes longer
   than the budget. The current `fff-search` call has no explicit switch for this; check the crate API or
   upstream before adding one.
3. **Debounce the picker:** wait about 120 ms after the last keystroke, and skip requests whose query is
   already stale. Keep fgrep off the draw path.
4. **Continue in the background:** after a partial page, keep searching from `next_file_offset` and append
   results while the picker stays responsive.
5. **Tune the budget for interactive use** (for example 150–250 ms per keystroke, 1 s for Enter or an
   explicit query). This is the budget decision left open.
6. **Long term: a content index** (trigram or similar). This is a large change, and the work may belong in
   fff.

Recommendation: 2, 3 and 4 together, after 1. Then measure again with `make baseline` at 50k and 100k
files.

## Open decision

How large a fraction of a tree can be skipped silently? With options 1 and 2, nothing is silent. The
remaining choice is the interactive budget in option 5.
