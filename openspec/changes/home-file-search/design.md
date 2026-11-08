# Design

## Context

See proposal.md - Why. The service builds one fff `FilePicker` per scope root (`FileSearch::index_with_content`). fff's `FilePicker::new` returns `Error::FilesystemRoot` when `base_path` equals `dirs::home_dir()` and `enable_home_dir_scanning` is false. On that error the CLI uses `LocalFileSearch` (`crates/smcli/src/cli/pick.rs`), a recursive depth-first walk with a shared 10,000-entry budget and a depth cap of 32.

## Goals / Non-Goals

**Goals:**

- A home-scoped search works through fff on macOS and Linux.
- The fallback walker spreads its budget across the scope's top level first.

**Non-Goals:**

- Filesystem root (`/`) scanning stays off.
- No home-specific exclude list beyond fff's ignore rules.
- No change to scan-wait time, pool size or idle eviction.
- No new backend (fsearch was considered and rejected: macOS only).
- No new storage: directory frecency comes from `commands`, file frecency from `file_selections`.
- Ctrl+Enter on a file has no action yet.

## Decisions

- **Set `enable_home_dir_scanning: true` on every picker.** fff only checks it when the base path is the home directory, so other roots are unaffected. A per-call flag set only when the scope is `~` was rejected: same effect, more code.
- **Breadth-first walk with a queue.** Replace the recursion in `walk` with a `VecDeque` of `(dir, depth)`. The contract is level order: a full level before the next, and the budget only cuts deeper levels. Fair interleaving of directory iterators (round-robin) was rejected: it guarantees more for a single very wide folder, but needs many open directory handles and more code for a degraded mode. Same budget, same depth cap, same `.git` skip, same `file_type()` check (no symlink following). The callback order changes, and callers already sort the results, so their output order stays the same.
- **Test with a temp `HOME`.** fff compares against `dirs::home_dir()`, which reads `$HOME`. The service test that covers the home case has to run with `HOME` pointed at a temp tree. nextest runs each test in its own process, so setting the variable is safe there.
- **Scope-wide pick scores.** `selection_scores` filters on `root = ?`. Add a query over `file_selections` where `root` is in the scope (see the scope predicate below), joining `root` and `path` and rebasing the result relative to the scope. One query, same `smcli_decay`. Keep the existing per-root form for project scopes; the scope-wide one is a superset, so it can replace it if the results match.
- **Directory frecency from `commands`.** `SELECT directory, sum(smcli_decay(now - started_at)) FROM commands WHERE deleted_at IS NULL AND <in scope> GROUP BY directory`, rebased to the scope with a trailing `/`. It feeds the service's `visited` map, not `picked`, so a folder the user only ran commands in gets provenance `visited`. `boost()` gives it `VISIT_WEIGHT * v / (1 + v)` with `VISIT_WEIGHT = 0.5`: slightly above an unvisited folder, never above a single fresh pick, and no ✨ marker. `directory_scores` orders and caps to 200 in SQL (`commands_by_directory` covers the lookup) with no per-folder stat. The ancestors of a frecent dir are not boosted: z doesn't do it either, and it would flood `~` with `Dev/`.
- **Scope predicate, no `LIKE`.** `LIKE` treats `%` and `_` as wildcards and folds ASCII case, so `project_one` would match `projectXone` and `Project_One`. Use a literal, case-sensitive range instead: `col = :scope OR (col >= :scope || '/' AND col < :scope || '0')`. `'0'` is the byte after `'/'`, so the range is exactly the paths under `scope/`; with the default BINARY collation it is case-sensitive and can use `commands_by_directory`. Scope `/` is special-cased to match everything. The same predicate is used for `file_selections.root`, and a pick whose root is above the scope counts only when `root/path` is in the scope.
- **Blank query.** It reorders fff's list by `boost()`, which reads picks from the `picked` map and squashed visits from the `visited` map, so frecent dirs rise with no new code. A visit scores `VISIT_WEIGHT * v / (1 + v)` and never ranks above one fresh pick. Frecent entries that fff's first page did not return are added only if fff's index already holds them: files by `get_file_by_path`, directories by binary search over fff's sorted non-overflow dirs. A directory the watcher added since fff last scanned is missed until the next rescan.
- **Jump is a new exit code (12), stdout = the directory's absolute path.** The hooks already branch on 0, 10 and 11. Each runs `cd -- "$dir"` in the widget's own shell (bash `bind -x`, a zsh widget, a fish function all run in the interactive shell), then redraws: `zle reset-prompt` in zsh, `commandline -f repaint` in fish; bash readline redraws on return. The cd is not added to history, like z.
- **Ctrl+Enter detection.** crossterm only reports Ctrl+Enter when the terminal's kitty keyboard protocol is on. Always push `KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES` and pop it on every exit path, including panics. Probing with `supports_keyboard_enhancement()` was dropped: it waited up to 2 s on the terminal and made the first draw take about 2 s. Terminals without the protocol ignore the request. Alt+Enter (`ESC CR`) works without it, so it is the guaranteed key.

## Risks / Trade-offs

- [A large home makes the first scan slow and memory-heavy] → The existing 10s wait returns partial results and the scan continues in the background. Content indexing stays off at `~` (`content_indexing_enabled`). The idle eviction frees the instance.
- [Keyboard enhancement left on after a crash garbles the shell's keys] → Pop it in the same guard that restores raw mode.
- [macOS volumes are usually case-insensitive, so the case-sensitive match can miss history recorded under another spelling of a path] → Recorded paths and the scope are both canonical real paths, so in practice the spelling matches.
- [Directory history includes dirs that no longer exist] → `directory_scores` does not stat them. The blank query adds one only if fff's index holds it and does not mark it deleted.
- [Command history can be large] → The query is indexed on `directory`. A per-scope cache is unnecessary unless `make baseline` shows a regression.
- [`@` Tab completion at `~` now uses fff's fuzzy search, so it can open the picker where it used to complete inline] → Accepted: it is the point of this change. The shell test's Enter at that step sends CR, as a real terminal does.
- [A very old terminal may print the keyboard-protocol request as stray characters] → Accepted; modern terminals ignore unknown CSI sequences.
- [The fallback walker still stops at 10,000 entries] → Breadth-first means the cap trims depth, not whole top-level folders. A single folder with more direct children than the budget can still hide its later siblings' contents. The fallback remains a degraded mode.
