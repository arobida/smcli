# fgrep: content search on large trees

**Naming:** "fgrep" means smcli's fff content search: the service's `grep` op, `FFFIndex.grep` and the picker's
grep mode (Ctrl-T). It is not the `grep`/`fgrep` command-line tools. Renaming the code identifiers is optional and
has not been done.

## How it works today

- `Picker` (`Sources/smcli/Picker.swift:351`) sends a `grep` request on **every keystroke** in fgrep mode, then
  waits for it before drawing (`timeout: 15`). There is no debounce and no cancellation.
- `FileSearch.grep` (`Sources/smcli-service/FileSearch.swift:26`) calls `FFFIndex.grep` with a 1000 ms budget.
- `FFFIndex.grep` (`Sources/SmcliFFF/FFFIndex.swift:93`) holds the index lock and calls
  `fff_live_grep(handle, query, mode 0 (plain text), …, file_offset 0, page_limit, budget 1000, …)`.
  fff's grep reads file contents on every call. Only the file list is indexed, not the contents.

## Measured (`scripts/bench-baseline.py`, one rare token, warm median)

| Tracked files | fgrep cold | fgrep warm |
|---|---:|---:|
| 10,000 | 100 ms | 102 ms |
| 50,000 | 1,020 ms | 1,488 ms |

The time grows roughly linearly with file count, because every query reads every eligible file.

## Why it is over budget, and what "partial" means

1. **The budget doesn't stop a search that has no matches yet.** `fff_live_grep` only enforces `time_budget_ms`
   once it has matches. The header says `fff_live_grep_ex(…, enforce_time_budget: true)` is the variant that "also
   bounds zero-match searches". A rare query (like the benchmark's) therefore always scans the whole tree. That
   explains 1.5 s at 50k files against a 1 s budget.
2. **When the budget does fire, the result is cut off silently.** fff returns `next_file_offset` (non-zero means
   unsearched files remain), `total_files_searched` and `filtered_file_count`. smcli reads none of them. It doesn't
   resume from the offset, and the picker can't show "partial".
3. **The picker freezes while it types.** Each keystroke blocks on a full scan. Per-index lock contention also
   queues concurrent fgreps behind each other. At 50k files, typing a 6-character query can freeze the picker for
   several seconds.

Correctness is unaffected (right file, line and column; `scripts/test-service.py` checks it at 50k files).

## Options, cheapest first

1. **Report partial results:** read `next_file_offset` and `total_files_searched` and return
   `partial: true` plus a searched/total count. The picker shows "searched 31k / 50k files".
2. **Enforce the budget:** switch to `fff_live_grep_ex(..., enforce_time_budget: true)`, so no query takes longer than the
   budget. Combined with option 1, the user sees results quickly, with an honest "partial" label.
3. **Fix the picker:** debounce keystrokes (~120 ms), cancel or skip stale requests, and keep fgrep off the
   draw path.
4. **Continue in the background:** after a partial page, keep searching from `next_file_offset` and stream or
   append results while the picker stays responsive.
5. **Tune the budget for interactive use** (for example 150–250 ms for each keystroke, 1 s for Enter or an explicit
   query). This is the budget decision the after-action report left open.
6. **Long term: a content index** (trigram or similar). This is a large change, and the work may belong in fff.

Recommendation: 1, 2 and 3 together. Each is small, and together they fix both the freeze and the silent truncation.
Then measure again with `make baseline` at 50k and 100k files.

## Open decision

How large a fraction of a tree can be skipped silently? With options 1 and 2, nothing is silent. The remaining
choice is the interactive budget in option 5.
