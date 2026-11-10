# Review fixes 2026-11-06

Fixes for the 2026-11-06 code review of the uncommitted work on `main` (database move to `~/.local/state`, home file
search, `ignoreFile`, jump keys, fallback walker), plus one problem found while planning (A0).

## Rules

- Agents: only `.claude/agents` (coder, tester, code-reviewer, scribe, semble-search). No orchestrator, no external teams.
- Coder and tester run only the focused tests for their slice (`make test-rust PKG=<crate> FILTER=<test>`, or one slice
  narrowed with `SMCLI_TEST_ONLY`). Only the main agent runs `make test` and `make linux`, once, after every slice is
  done and reviewed.
- Format code only right before a commit, never during the work. Edit only the slice's files.
- Do not commit or touch git state.
- Loop per slice: coder implements + adds tests -> coder runs its focused tests -> code-reviewer reviews the slice diff ->
  coder fixes -> tester reruns only that slice's focused tests -> done.

## Step 0: workspace reformat (resolved)

A wave 1 coder ran `cargo fmt` over the whole workspace (69 files). The user keeps it: the code should be formatted.

- Backup of the live history database (before any work): `~/Desktop/smcli-history-20261106-183816.db`, integrity ok,
  19,604 commands.

## Wave 1: code written, review pending

### Slice A: database move (findings 1, 8, new A0)

Files: `crates/smcli-core/src/history.rs`, `crates/smcli/src/cli/mod.rs`.

- A0: the old hooks exported `SMCLI_DB="${SMCLI_DB:-$HOME/.local/share/smcli/history.db}"`, so shells opened before the
  upgrade (and their children) keep an explicit legacy SMCLI_DB, never migrate, and recreate an empty legacy database
  after a new shell moves it. Fix: `CapturedHistory::location` treats SMCLI_DB equal to `legacy_location(home)` as unset.
- 1 (partial move): `migrate_legacy` keeps the cheap pre-check, the `.legacy-move.lock` and the re-check, stops the
  service, clears a stale `history.db.moving`, runs `VACUUM INTO` the `.moving` file, fsyncs, renames it to `history.db`,
  and only then deletes the old `history.db`/`-wal`/`-shm`. The index folder is renamed after; if that fails it is
  deleted and `reindex` rebuilds it. Only database files move.
- `run()`: if the move fails, warn and use the legacy path for this run; the next run retries.
- 8 (runs on every call): skip the move for `--help`/`-h`, `version`, `shell-init` and usage errors; keep it for
  `record` (skipping it would let record create the new database first and strand the history).
- Tests: success; index-folder clash still moves the database; failure before the rename leaves the old database and no
  new one; stale `.moving` cleaned; SMCLI_DB equal to the legacy path resolves to the default; non-default SMCLI_DB
  never migrates. Focused run: `make test-rust PKG=smcli-core FILTER=legacy`.

### Slice B: database exposure guard (finding 2)

Files: `crates/smcli-service/src/service.rs`, `crates/smcli-core/src/paths.rs`, the refusal check in
`crates/smcli-service/src/file_search.rs`.

- The canonical database path is always passed to `refuse_roots_holding`. A root that holds it is refused unless
  `paths::fff_skips(root, db)`: the root is not in a git work tree and the database's path below the root runs through
  an fff-ignored folder (`.local/state`, macOS `Library/Application Support`), as confirmed in the fff-search source.
- Review must check: the fff version and source lines cited in the comment, and that the home root is still allowed.
- Tests: non-git home allowed; home as a git repo refused; root `~/.local` refused; root equal to the database folder
  refused; database outside home with its parent as root refused. Focused runs:
  `make test-rust PKG=smcli-core FILTER=paths`, `make test-rust PKG=smcli-service FILTER=<guard tests>`.

### Slice C: relative XDG_STATE_HOME in hooks (finding 3)

File: `crates/smcli/src/shell_integration.rs`.

- `shell-init` computes `Xdg::State.dir(home, env)` in Rust (which drops relative values) and writes it into the bash,
  bash 3, zsh and fish hooks as a quoted literal. `${SMCLI_DB:-...}` stays, so an explicit SMCLI_DB still wins.
- Review must check: every former `${XDG_STATE_HOME:-...}` spot is replaced, quoting, and whether install caches the
  hook output (the literal would then be fixed at install time).
- Tests: relative value gives the `~/.local/state` default, absolute value is used, a path with a space or quote is
  quoted, for each shell. Focused runs: `make test-rust PKG=smcli FILTER=shell_init`, then a few basic checks with
  `SMCLI_TEST_ONLY=... make test-shells`.

## Wave 2: after wave 1 is reviewed (D then E; both edit `file_search.rs`)

### Slice D: ignoreFile correctness (findings 4, 5)

Files: `crates/smcli-core/src/config.rs`, `crates/smcli-service/src/file_search.rs`.

- 4: outside home, `FileIgnore::ignores` matches each of `path.ancestors()` (parents as folders), so `node_modules/`
  hides everything under it, as the CLI fallback walker already does. For a symlinked HOME, keep the given and the
  canonical home and map paths under one onto the other before matching; no per-call canonicalize.
- 5: drop ignored items before `.take(limit)` (typed fuzzy query) and before `truncate` (grep). With an ignoreFile set,
  ask fff for `2*limit + 15` candidates, with a `shortcut:` comment that results can still come up short when most
  matches are ignored.
- Tests: outside-home folder pattern; symlinked home; top `limit` fuzzy matches all `*.log` with `*.log` ignored still
  returns `limit` hits; same for grep. Focused runs: `make test-rust PKG=smcli-core FILTER=ignore`,
  `make test-rust PKG=smcli-service FILTER=ignore`.

### Slice E: visited folders and blank-query extras (findings 6, 7)

Files: `crates/smcli-service/src/service.rs`, `crates/smcli-service/src/file_search.rs`, `crates/smcli/src/picker.rs`.

- 6 (decided by the user): folders the user only ran commands in rank slightly higher but are not labeled picked.
  `directory_scores` comes back as its own map, adds a smaller boost than picks, and gives the provenance `visited`.
  `picker.rs` `suggested()` keeps keying on `picked`/`suggested` only. Confirm clients ignore an unknown provenance
  value; add a protocol fixture line only if the frozen fixtures list values. `commands_by_directory` already indexes
  the range query; check with `EXPLAIN QUERY PLAN` and cap the per-folder `stat` to the top 200 by score.
- 7: on a blank query, add a frecent path only if fff's index for this root already holds it (restores fff's gitignore,
  hidden and IGNORED_DIRS filtering) and `ignoreFile` does not hide it; dedupe with a `HashSet`; consider only the top
  N frecent entries.
- Tests: update the existing `picked` assertions in `file_search.rs` to cover real picks only; a cd-only folder gets
  `visited`, ranks above an equal unvisited one, and has no `picked`; a frecent `node_modules/x` or gitignored folder
  stays off a blank query. Touch `scripts/test-service.py` only if its meaning changes. Focused runs:
  `make test-rust PKG=smcli-service FILTER=frecen`, `make test-rust PKG=smcli FILTER=picker`, `make test-picker`.

## Wave 3: docs (scribe, iwe only)

- 9: `crates/smcli-core/src/README.md`, replace `selection_scores` with `scope_selection_scores` and `directory_scores`.
- `openspec/changes/home-file-search/design.md`: the `visited` provenance (6) and the rule that frecent extras must be
  in fff's index (7). Leave tasks 5.1 and 5.2 open.
- `docs/install.md` and `crates/smcli/src/usage.txt`: the move is copy-then-remove, and a legacy SMCLI_DB is redirected.
- Run `make docs`.

## Final gate (main agent only, once)

1. code-reviewer does one pass over the combined diff for cross-slice interactions: A and C agree on the database path;
   B still allows the home root; D and E filter the same way.
2. `make test` (macOS).
3. `make linux`: needs Docker running (OrbStack's socket was missing last time); otherwise reported as not run.
4. `make fresh`, then relay the eval line for the manual check (openspec task 5.2).

On a failure: send the output and the code to code-reviewer, coder fixes, tester reruns that one focused test, then rerun
the gate.
