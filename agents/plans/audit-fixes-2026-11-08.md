---
title: Audit fixes — 2026-11-08
---

# Audit fixes — 2026-11-08

Source: [Codebase review — 2026-11-08](../reviews/codebase-review-2026-11-08.md). Numbers such as #21 are that report's finding numbers. Line numbers in the report predate `cargo fmt`, so find code by symbol name.

This plan fixes the 29 findings on macOS in three waves. Linux is out of scope: do not run `make linux`, and add any Linux-only problem to the [roadmap](roadmap.md). Four egm reviewers (readiness, code accuracy, design, scope) checked the first draft, and this version includes their corrections.

## Decisions already made

These choices are final for this work, so nothing stops for questions.

| Topic             | Decision                                                                                                                                                                                                                                                | Why, and the limit we accept                                                                                                                                                                                      |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| #1 Git status     | Always pass `-c core.fsmonitor=false` and `--ignore-submodules=all`. Skip `git status` when `git config --show-scope --includes --list` shows a `filter.*` key at `local` or `worktree` scope.                                                          | Follows includes, which `--local` alone does not. Global filters such as Git LFS stay allowed. Repos with their own filters show no Git status, and dirty-submodule counts are lost.                              |
| #21 no context    | `smcli complete` prints nothing when `SMCLI_LBUFFER` is not insertable. If the variable is missing, it also prints the existing "reload smcli's shell-init" hint. An empty value is allowed.                                                            | Matches the picker. Shells opened before the upgrade lose `@` completion until reloaded.                                                                                                                          |
| #22 old database  | Checkpoint the old database, then rename it and its `-wal` and `-shm` files to `history.db.pre-xdg`, `-wal` and `-shm` in the new state folder. Skip a rename if the target exists. If the rename crosses volumes, leave the files where they are.      | A terminal that already has the file open writes into the kept copy. Those writes are kept but never merged. A terminal that opens the old path later recreates it, and `stranded_legacy` correctly reports that. |
| #23 export gap    | Keep the current order and say in the uninstall message that commands run during uninstall may be missing.                                                                                                                                              | Moving the stop first can trigger the database move and skip the export. With live hooks the service would restart anyway.                                                                                        |
| #11 transport     | `smcli record --stdin` reads one line for the expansion, which may be empty, then the command. All shells send it with builtin `printf '%s\n%s\n' ... \|`.                                                                                              | Here-strings can use a temporary file. Text passed to `smcli pick` is still visible in process arguments; that is out of scope.                                                                                   |
| #6 replacement    | Same binary path: replace as today. Different paths: replace only when the service's build time is older. A protocol mismatch always replaces.                                                                                                          | An in-place restore with an old time still works. Times mean "copied at", so a `dist/` build older than the last install uses the installed service.                                                              |
| #25 project files | Hide nothing. A command already in your history is boosted when the project has a file relevant to its program: for example, any `make ...` run when the project has a Makefile. Multiply its score by 1.25, below the 1.5 for an exact project script. | Applies only to commands you have run. A boosted `make docs` can still fail from a subfolder.                                                                                                                     |
| Performance       | Fix #15 and #17 directly. Measure #18 and #28 first, because the report says to.                                                                                                                                                                        | #28's limit is 2 seconds, the existing per-query embedding bound. #18's 5 ms limit is a planning budget.                                                                                                          |

## How the work runs

- **Branch:** `macos-review-fixes`. Do not commit or push unless asked. Run `cargo fmt --all` only at commit time.
- **Writing code:** the `coder` agent, one slice at a time. It also edits test scripts named in its slice, and runs no tests.
- **Review:** the `code-reviewer` agent reviews each slice diff before its tests run.
- **Running tests:** only `kik3`, started by the Orchestrator. This overrides the once-only full-suite rule in `AGENTS.md` for this plan.
- **Logging:** the main agent adds each result to the [test log](#test-log), then runs `make docs`.

### Test rules

1. Run a test only when a slice changes behaviour. Documentation and deletion need only `make build`.
2. Run only the named unit tests: `make test-rust PKG=<crate> FILTER=<name>`. `FILTER` matches part of a test name.
3. Run `make test` once after a wave's slices are all reviewed and passing, and not before. Changes to shell hooks and test scripts are exercised there.
4. **On pass:** log it and move on.
5. **On failure:** log it, send the failing output and the code under test to `code-reviewer`, fix from that review, then have `kik3` rerun only the failing test. If `make test` failed, run it again only after those reruns pass.

## Wave 1: code execution and private data

Findings: #1, #21, #11, #2, #24, #22, #3, #23.

### Slice 1A: untrusted repo Git status (#1)

File: `crates/smcli-core/src/context.rs`, `GitStatus::read`.

- Apply the decision above. A failed config read also returns `None`, so status is skipped.
- Tests, `PKG=smcli-core`, each with a marker-writing script:
  - `FILTER=git_status_ignores_repo_fsmonitor`.
  - `FILTER=git_status_skips_repo_filters`. Commit a tracked file, rewrite it with the same size so its stat data is dirty, and define the filter through `include.path`.

### Slice 1B: shell hooks (#21, #11)

Files: `crates/smcli/src/shell_integration.rs`, `crates/smcli/src/cli/mod.rs`, `crates/smcli/src/cli/history.rs`, `scripts/test-cli.sh`, `scripts/test-shells.py`.

- #21: add `complete_allowed(lbuffer: Option<&str>, shell: &str) -> bool` and call it first in `Command::Complete`. Hooks set `SMCLI_LBUFFER` from: Bash 4 `$before`, Bash 3 `${COMP_LINE:0:COMP_POINT}`, Zsh `$LBUFFER`, Fish `$before`, moved above the call. On Bash 3, Readline's default completion can still run after smcli declines.
- #11:
  - Add `--stdin` to `Record`, conflicting with `--expanded`. It needs the 8- or 9-field form with `-` as the last field.
  - Add `split_record_stdin(&str) -> Option<(Option<&str>, &str)>`. An empty first line means no expansion. Strip exactly one trailing newline. Missing stdin or no newline makes `record` fail.
  - Replace the `-` field before `record` reads it.
  - Hooks send an empty first line unless `_smcli_safe` passes. Quote the value in Fish. Change both the sync and background branches.
- Scripts: set `SMCLI_LBUFFER` on all three `complete` calls in `test-cli.sh`. Make the fake binary in `test-shells.py` log stdin and decode `--stdin`, so its alias-capture checks still work.
- Tests, `PKG=smcli`: `FILTER=record_stdin`, `FILTER=complete_allowed`, and a `shell_integration` assertion on hook text, `FILTER=hooks_send_record_on_stdin`.

### Slice 1C: filters on import and fallback (#2, #24)

Files: `crates/smcli-core/src/import.rs`, `crates/smcli/src/complete.rs`, `crates/smcli/src/cli/mod.rs`.

- #2: call `filter.allows_run(&command, None, &cwd)` before the `sessions` insert. A rejected row with `deleted_at` still runs `UPDATE commands SET deleted_at=? WHERE uuid=?`. Replace the comment saying filters are not applied.
- #24: add an `Option<&FileIgnore>` argument to `complete_local`, and update its test calls. In the fallback arm only, load with `FileIgnore::load(&config_path, &home)?`. Apply it during the walk, before results are cut.
- Tests: `PKG=smcli-core FILTER=atuin_import_applies_filters` and `PKG=smcli FILTER=complete_local_honors_ignore`.

### Slice 1D: database move and uninstall (#22, #3, #23)

Files: `crates/smcli-core/src/history.rs`, `crates/smcli-core/src/prune.rs`, `crates/smcli/src/cli/history.rs`, `scripts/uninstall.sh`, `scripts/test-install.sh`.

- #22: on the `VACUUM INTO` connection, run `PRAGMA wal_checkpoint(TRUNCATE)`. After the new database is in place, rename the main file first, then `-wal` and `-shm`, using the decision above. Change the assertions that require the old file or folder to be gone, and add ones for the kept files.
- Prune: change `PruneReport.backup` and `PruneError.backup` to a list holding `.v7.bak` and `history.db.pre-xdg`. Warn about each.
- #3 and #23, in `uninstall.sh`:
  - Touch both locations only when `db` is the state or legacy default. A custom `SMCLI_DB` never touches them.
  - Before the CLI export runs, back up the extra databases with `sqlite3 .backup` to `history-legacy.db` and `history-pre-xdg.db`. Skip any that is the primary. Never export `-wal` or `-shm`. Honour `--no-export` and `--keep-data`.
  - In step 4, delete both families with their `-wal`, `-shm`, `.search`, `.v7.bak`, `.migration.lock` and `.pre-xdg*` files. Add them to the directory safety check.
  - Delete `.legacy-move.lock`, `kept-keys-bash`, `kept-keys-zsh`, `kept-keys-fish` and `.bash3-warned` from the state folder, then remove both folders if empty.
  - Add the warning message from the #23 decision.
  - In `test-install.sh`, add cases for legacy only, both databases, and a custom `SMCLI_DB` that leaves the default files alone. Assert the export contains the rows and both locations end up empty.
- Test: `PKG=smcli-core FILTER=legacy` and `PKG=smcli-core FILTER=prune_reports`.

### Wave 1 gate

`kik3` runs `make test` once.

## Wave 2: wrong results

Findings: #5, #9, #12, #6, #8, #16, #10, #14, #25.

### Slice 2A: history database (#5, #9, #12)

File: `crates/smcli-core/src/history.rs`.

- #5: run the `journal_mode = WAL` query on every write connection, before the `synchronous` line. Ignore an error, and set `synchronous = NORMAL` only if the returned mode is `wal`. Update the stale comment near the WAL test.
- #9: in `recent_distinct`, use `GROUP BY` on the command key and `ORDER BY max(c.started_at) DESC, max(c.id) DESC`.
- #12: add `AND c.deleted_at IS NULL` to `file_kind_rows` and `last_directory`.
- Tests, `PKG=smcli-core`: `FILTER=write_connection_enables_wal`, `FILTER=recent_distinct_orders_by_latest_run`.

### Slice 2B: service replacement (#6)

Files: `crates/smcli-core/src/service_location.rs`, `crates/smcli-core/src/service_client.rs`.

- Add `build_parts(&str) -> Option<(&str, (i64, i64))>` beside `build`, using `rsplitn(3, '|')` and integer seconds and nanoseconds, which are not zero-padded. An unparsable value still replaces.
- Test: `PKG=smcli-core FILTER=service_replacement`.

### Slice 2C: paths and imports (#8, #16)

Files: `crates/smcli-core/src/paths.rs`, `crates/smcli-core/src/import.rs`.

- #8: in `relative_insert`, canonicalize the parent and add the file name back. Use today's path when the last component is `..`.
- #16: in `validate_source`, report a missing source first.
- Test: `PKG=smcli-core FILTER=relative_insert_keeps_final_symlink`.

### Slice 2D: command line (#10, #14)

Files: `crates/smcli/src/cli/mod.rs`, `crates/smcli/src/initialize.rs`, `crates/smcli/src/usage.txt`, `scripts/test-cli.sh`.

- #10: call `e.exit()` for help and version. For other errors, print clap's message and exit 2. Keep the general usage for no arguments and top-level `--help`.
- #14: delete `--model` and `--builtin`, their `Init` fields, the `builtin` entry in `conflicts_with_all`, their uses in `has_model`, the bail in `run`, and the help text about Ollama and Apple's model. Update the test helper `init()`. Keep clearing old config fields.
- Scripts: add `--version` and an error-message check to `test-cli.sh`.
- Test: `PKG=smcli FILTER=parse_flags_and_times`. Assert that `--version` gives `ErrorKind::DisplayVersion`, and that `init --model x` and `init --builtin` fail.

### Slice 2E: boost history commands with a relevant project file (#25)

Files: `crates/smcli-core/src/ranking.rs`, `crates/smcli-search/src/corpus.rs`.

- In `RankingContext::new`, build `project_programs` from the kinds in `project.projects`, which covers the nearest project and, inside a repo, each project up to the root:

| Project kind        | Programs                                             |
| ------------------- | ---------------------------------------------------- |
| `make`              | `make`                                               |
| `node`              | `npm`, `npx`, `pnpm`, `yarn`, `bun`                  |
| `rust`              | `cargo`                                              |
| `python`            | `python`, `python3`, `pip`, `pytest`, `uv`, `poetry` |
| `go`                | `go`                                                 |
| `swift`             | `swift`                                              |
| `deno`              | `deno`                                               |
| `ruby`, `rake`      | `bundle`, `rake`, `ruby`                             |
| `php`               | `composer`, `php`                                    |
| `maven`             | `mvn`, `mvnw`                                        |
| `gradle`            | `gradle`, `gradlew`                                  |
| `elixir`            | `mix`                                                |
| `dart`              | `dart`, `flutter`                                    |
| `cmake`             | `cmake`                                              |
| `just`              | `just`                                               |
| `task`              | `task`                                               |
| `compose`, `docker` | `docker`, `docker-compose`                           |

- Add one shared helper that tests a command's program against that set, using `parse::program_of`.
- In `apply_ranking_signals`, before project scripts are added, multiply the score of each history command that matches by 1.25. Skip exact project scripts, which already get 1.5. Add a reason such as `Makefile in project`.
- Apply the same helper in the search rerank in `corpus.rs`, so suggestions and search agree.
- Respect the existing `script` switch in `SMCLI_RANKING_OFF`.
- Tests, `PKG=smcli-core`: `FILTER=history_commands_boosted_by_project_file`. Assert that `make -j4 test` scores higher with a Makefile than without, that a command never run is not added, and that an exact project script is not boosted twice. `PKG=smcli-search FILTER=search_boosts_history_with_project_file`.

### Wave 2 gate

`kik3` runs `make test` once.

## Wave 3: speed, ranking details and cleanup

Findings: #4, #7, #13, #15, #17, #18, #28, #26, #27, #19, #20, #29.

### Slice 3A: background work (#4, #7, #13)

Files: `crates/smcli-service/src/service.rs`, `crates/smcli-service/src/context_cache.rs`, `crates/smcli-core/src/suggestion_index.rs`.

- #4: after a sync, add every history program to `known`. Keep `recorded_program`. Inside it, take `expanded.unwrap_or(command)` and call `parse::program_of` with `CommandBreakdown::parse` on that text.
- #7: make `invalidate` only mark the entry dirty, delete the restart after a refresh, and update the comment describing refreshes. `filesystem_events_refresh_status` must call `context()` in its wait loop.
- #13: delete `history`, including the read in the snapshot test. Return `false` from `add` when the base time is too old. Rewrite `rebases_and_keeps_latest_fields` to seed one old run, then assert `add` returns `false` for a run past the limit.
- Tests: `PKG=smcli-service FILTER=sync_remembers_history_programs` (also asserts `FOO=1 make` gives `make`), `PKG=smcli-service FILTER=context_cache`, `PKG=smcli-core FILTER=rebases_and_keeps_latest_fields`.

### Slice 3B: direct fixes and measurements (#15, #17, #18, #28)

Files: `crates/smcli-search/src/commands.rs`, `crates/smcli-service/src/file_search.rs`, `crates/smcli-core/src/suggestion_index.rs`, `crates/smcli-search/src/index.rs`.

- #15: read only the `program` field with `Stage::Project`. Test: `PKG=smcli-search FILTER=programs`.
- #17: collect evicted entries and drop them after unlocking, in `index_with_content` and `evict_idle`. Compiler only.
- #18 and #28: `coder` adds one `#[ignore]` timing test each, using 100,000 synthetic commands and printing the time. Never use real history.
  - `snapshot_timing` (`PKG=smcli-core`) times `SuggestionIndex::snapshot`. Over 5 ms: filter while copying, and copy one session's data.
  - `blocked_search_timing` (`PKG=smcli-search`) times a natural-language search with no index lock. Over 2 seconds: use the existing keyword fallback.
  - Run with `make test-rust PKG=<crate> FILTER="--release --run-ignored only <name>"`. Log the number. Under the limit, close the finding with that number.

### Slice 3C: ranking details (#26, #27)

Files: `crates/smcli-core/src/tests_discovery.rs`, `crates/smcli-service/src/test_candidates.rs`, `crates/smcli-service/src/evidence.rs`.

- #26: accept `async fn`, and add `it("` and `test("` to both the search patterns and the parser.
- #27: add `declares_name(line: &str, name: &str) -> bool`, true when the word before the name is one of `fn`, `def`, `func`, `function`, `class`, `struct`, `enum`, `trait`, `type`, `interface`, `impl`, `const`, `let`, `var`, `mod` or `module`. Mark a definition only when `hit.is_definition && declares_name(...)`.
- Tests: `PKG=smcli-core FILTER=discovers_language_test_names`, `PKG=smcli-service FILTER=double_quoted_test_candidates`, `PKG=smcli-service FILTER=declares_name`.

### Slice 3D: cleanup (#19, #20, #29)

- #19: delete the second delete loop in `prune.rs` and the `DISCARDED_USAGE_VERSION` exception in the newer-version check.
- #20: delete `files_typed`, `grep_typed` and `complete_typed`, and call `search` and `complete_query` from the service.
- #29: set the status of `semantic-search-accuracy.md` to `Shipped (vector-first)`. Mark [double-quoted-js-tests.md](double-quoted-js-tests.md) as replaced by Slice 3C. Use iwe, then `make docs`.
- Check: `make build`, then `make unused`, with no new hits from these files.

### Wave 3 gate

`kik3` runs `make test` once.

## Done when

- Each finding is fixed, or closed in the test log with its measurement.
- Each wave has a passing `make test` row in the test log.

## Outcome

All 29 findings from the 2026-11-08 review are fixed on macOS, or closed with a measurement.

## Acceptance checks

- Wave 1 passes the full suite — integration: make test `#wave-1-gate`
- Wave 2 passes the full suite — integration: make test `#wave-2-gate`
- Wave 3 passes the full suite — integration: make test `#wave-3-gate`

## Decisions

- The old database files move as one unit: if any `history.db.pre-xdg` target exists, none are renamed.
- Uninstall converts exported databases out of WAL mode so they are self-contained.
- `run_git` sets `GIT_NO_LAZY_FETCH=1` (the person approved this on 2026-11-09).
- Uninstall also removes `.bash3-warned` beside the database and `.moving`, and removes the binaries before the data (the person approved this on 2026-11-09).
- Search adds the project-file boost, at half the script weight times query coverage, because a 1.25 multiple could outrank an exact project script. Suggestions keep 1.25.
- `npm run x` gets no project-file boost when the project lists its scripts and x is not one of them, so the ranking floor holds.
- A `dist/` build that keeps a newer installed service can differ from it in bundled model or tldr data, which degrades `@@` ranking until they match.
- #18 keeps the planned optimization and is closed over its 5 ms planning budget: 9.4 ms with a prefix and 41.2 ms with an empty prefix at 100,000 distinct commands.
- #15 uses the planned projection, which saves only part of the cost because TalaDB decodes the document first.
- Wave 3 was written and reviewed by oteam (the person asked on 2026-11-09), with kik3 still running tests.

## Tasks

### Build

- [x] Slice 1A: untrusted repo Git status `#slice-1a`
- [x] Slice 1B: shell hooks `#slice-1b`
- [x] Slice 1C: filters on import and fallback `#slice-1c`
- [x] Slice 1D: database move and uninstall `#slice-1d`
- [x] Slice 1E: lazy-fetch hardening and uninstall follow-ups `#slice-1e`
- [x] Wave 2 slices 2A to 2E `#wave-2`
- [x] Wave 3 slices 3A to 3D `#wave-3`

### Test

- [x] Wave 1 gate `#wave-1-test`
- [x] Wave 2 gate `#wave-2-test`
- [x] Wave 3 gate `#wave-3-test`

### Review

- [ ] Person reviews the audit fixes `#person-review`

## Test log

| Date       | Wave or slice   | Command                                                                           | Result                                     | Notes                                                                                               |
| ---------- | --------------- | --------------------------------------------------------------------------------- | ------------------------------------------ | --------------------------------------------------------------------------------------------------- |
| 2026-11-09 | Slice 1A        | `make test-rust PKG=smcli-core FILTER=git_status_ignores_repo_fsmonitor`          | Pass (1 test)                              | Run by `kik3` after review fixes                                                                    |
| 2026-11-09 | Slice 1A        | `make test-rust PKG=smcli-core FILTER=git_status_skips_repo_filters`              | Pass (1 test)                              | Run by `kik3` after review fixes                                                                    |
| 2026-11-09 | Slice 1B        | `make test-rust PKG=smcli FILTER=record_stdin`                                    | Pass (1 test)                              | Run by `kik3` after review fixes                                                                    |
| 2026-11-09 | Slice 1B        | `make test-rust PKG=smcli FILTER=complete_allowed`                                | Pass (1 test)                              | Run by `kik3` after review fixes                                                                    |
| 2026-11-09 | Slice 1B        | `make test-rust PKG=smcli FILTER=hooks_send_record_on_stdin`                      | Pass (1 test)                              | Run by `kik3` after review fixes                                                                    |
| 2026-11-09 | Slice 1C        | `make test-rust PKG=smcli-core FILTER=atuin_import_applies_filters`               | Pass (2 tests)                             | Run by `kik3` after review fixes                                                                    |
| 2026-11-09 | Slice 1C        | `make test-rust PKG=smcli FILTER=complete_local_honors_ignore`                    | Pass (1 test)                              | Run by `kik3` after review fixes                                                                    |
| 2026-11-09 | Slice 1D        | `make test-rust PKG=smcli-core FILTER=legacy`                                     | Fail (6 of 7)                              | `legacy_existing_pre_xdg_blocks_all_renames` failed at `legacy.exists() && ...-wal.exists()`        |
| 2026-11-09 | Slice 1D        | `make test-rust PKG=smcli-core FILTER=prune_reports`                              | Pass (1 test)                              | Run by `kik3` after review fixes                                                                    |
| 2026-11-09 | Slice 1D rerun  | `make test-rust PKG=smcli-core FILTER=legacy_existing_pre_xdg_blocks_all_renames` | Pass (1 test)                              | Test bug fixed; migration unchanged                                                                 |
| 2026-11-09 | Wave 1 gate     | `make test`                                                                       | Fail                                       | nextest 379 passed; install section: `legacy only: a location was left behind`                      |
| 2026-11-09 | Wave 1 gate fix | `make test-install`                                                               | Fail                                       | Case `both`: export refused `history.db` permissions; `make test` not run                           |
| 2026-11-09 | Wave 1 gate fix | `make test-install`                                                               | Fail                                       | Case `both`: `-wal or -shm was exported` after fixture `chmod 600`; `make test` not run             |
| 2026-11-09 | Wave 1 gate fix | `make test-install`                                                               | Pass                                       | Exports converted out of WAL mode                                                                   |
| 2026-11-09 | Wave 1 gate     | `make test`                                                                       | Pass                                       | nextest 379 passed, 9 skipped; all black-box sections passed                                        |
| 2026-11-09 | Slice 1E        | `make test-install`                                                               | Pass                                       | Lazy-fetch hardening and uninstall follow-ups                                                       |
| 2026-11-09 | Wave 1 gate     | `make test`                                                                       | Pass                                       | Final tree: nextest 379 passed, 9 skipped; all black-box sections passed                            |
| 2026-11-09 | Slice 2A        | `make test-rust PKG=smcli-core FILTER=write_connection_enables_wal`               | Pass (1 test)                              | Run by `kik3` after review                                                                          |
| 2026-11-09 | Slice 2A        | `make test-rust PKG=smcli-core FILTER=recent_distinct_orders_by_latest_run`       | Pass (1 test)                              | Run by `kik3` after review                                                                          |
| 2026-11-09 | Slice 2B        | `make test-rust PKG=smcli-core FILTER=service_replacement`                        | Pass (1 test)                              | Run by `kik3` after review fixes                                                                    |
| 2026-11-09 | Slice 2C        | `make test-rust PKG=smcli-core FILTER=relative_insert_keeps_final_symlink`        | Pass (1 test)                              | Run by `kik3` after review fixes                                                                    |
| 2026-11-09 | Slice 2C        | `make test-rust PKG=smcli-core FILTER=import_reports_missing_source`              | Pass (1 test)                              | Run by `kik3` after review fixes                                                                    |
| 2026-11-09 | Slice 2D        | `make test-rust PKG=smcli FILTER=parse_flags_and_times`                           | Pass (1 test)                              | Run by `kik3` after review fixes                                                                    |
| 2026-11-09 | Slice 2E        | `make test-rust PKG=smcli-core FILTER=history_commands_boosted_by_project_file`   | Pass (1 test)                              | Run by `kik3` after review fixes                                                                    |
| 2026-11-09 | Slice 2E        | `make test-rust PKG=smcli-search FILTER=search_boosts_history_with_project_file`  | Fail                                       | Order assertion at `corpus.rs:1392`: `make -j4 test` first without the project file                 |
| 2026-11-09 | Slice 2E rerun  | `make test-rust PKG=smcli-search FILTER=search_boosts_history_with_project_file`  | Pass (1 test)                              | Test isolated the boost                                                                             |
| 2026-11-09 | Slice 2E        | `make ranking`                                                                    | Fail (3 of 5)                              | Suggest MRR 0.972 below floor 1 (`npm run `); NL `npm run sto` rank 5; `make test` not run          |
| 2026-11-09 | Slice 2E        | `make test-rust PKG=smcli-core FILTER=project_file`                               | Pass (2 tests)                             | Includes the unlisted `npm run` guard                                                               |
| 2026-11-09 | Slice 2E        | `make ranking`                                                                    | Pass (5 of 5)                              | Suggest MRR back to 1.000                                                                           |
| 2026-11-09 | Wave 2 gate     | `make test`                                                                       | Pass                                       | nextest 387 passed, 9 skipped; all black-box sections passed                                        |
| 2026-11-09 | Slice 3A        | `make test-rust PKG=smcli-service FILTER=sync_remembers_history_programs`         | Pass (2 tests)                             | Written and reviewed by oteam, run by `kik3`                                                        |
| 2026-11-09 | Slice 3A        | `make test-rust PKG=smcli-service FILTER=context_cache`                           | Pass (4 tests)                             | Run by `kik3`                                                                                       |
| 2026-11-09 | Slice 3A        | `make test-rust PKG=smcli-core FILTER=rebases_and_keeps_latest_fields`            | Pass (1 test)                              | Run by `kik3`                                                                                       |
| 2026-11-09 | Slice 3B        | `make test-rust PKG=smcli-search FILTER=programs`                                 | Pass (4 tests)                             | #15 saves only part of the cost: TalaDB decodes the document before projecting                      |
| 2026-11-09 | Slice 3B        | `snapshot_timing` (release, 100,000 synthetic commands)                           | 39.9 ms, over the 5 ms limit               | #18 needs the planned optimization                                                                  |
| 2026-11-09 | Slice 3B        | `blocked_search_timing` (release, 100,000 synthetic commands)                     | 1.04 s, under the 2 s limit                | Closes #28                                                                                          |
| 2026-11-09 | Slice 3B        | `make test-rust PKG=smcli-core FILTER=snapshot`                                   | Pass (2 tests)                             | Optimized snapshot keeps results                                                                    |
| 2026-11-09 | Slice 3B        | `snapshot_timing` after the #18 optimization                                      | 9.4 ms with a prefix, 41.2 ms empty prefix | Planned fix applied; still over the 5 ms planning budget at 100,000 distinct commands               |
| 2026-11-09 | Slice 3C        | `make test-rust PKG=smcli-core FILTER=discovers_language_test_names`              | Pass (1 test)                              | Written and reviewed by oteam, run by `kik3`                                                        |
| 2026-11-09 | Slice 3C        | `make test-rust PKG=smcli-service FILTER=double_quoted_test_candidates`           | Pass (1 test)                              | Run by `kik3`                                                                                       |
| 2026-11-09 | Slice 3C        | `make test-rust PKG=smcli-service FILTER=declares_name`                           | Pass (2 tests)                             | Added `object` and a generic-group skip after review                                                |
| 2026-11-09 | Wave 3          | `make ranking`                                                                    | Pass (5 of 5)                              | Suggest MRR 1.000                                                                                   |
| 2026-11-09 | Wave 3 gate     | `make test`                                                                       | Fail                                       | nextest 391 passed, 11 skipped; shells: `run_at_completion(zsh)` timed out waiting for a new prompt |
| 2026-11-09 | Wave 3 gate fix | `make test-shells`                                                                | Pass                                       | `run_at_completion` waits for the picker to close before the second Enter                           |
| 2026-11-09 | Wave 3 gate     | `make test`                                                                       | Pass                                       | nextest 391 passed, 11 skipped; all black-box sections passed                                       |
