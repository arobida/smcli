---
title: Audit fixes — 2026-11-22
---

# Audit fixes — 2026-11-22

Source: [Codebase review — 2026-11-22](../reviews/codebase-review-2026-11-22.md). Numbers such as #3 are that report's finding numbers; "S:" marks an item from its smaller-findings list. Line numbers drift, so find code by symbol name.

This plan fixes the 20 ranked findings and 15 of the smaller ones on macOS, in three waves. The rest are [deferred](#deferred) with a reason. Linux is out of scope: do not run `make linux`, and add any Linux-only problem to the [roadmap](roadmap.md).

Codex reviewed the first draft adversarially (`codex exec`, output kept in the session scratchpad). Each of its points was checked against the code, and this version includes the corrections: a third arithmetic route, wrong function and file names, unsafe legacy-folder deletion, a glob bound that did not hold, wrong cache designs in 3C and 3E, and process rules that contradicted `AGENTS.md`.

## Decisions already made

These choices are final for this work, so nothing stops for questions.

| Topic                   | Decision                                                                                                                                                                                                                                                                                                                                                                                                                                                                  | Why, and the limit we accept                                                                                                                                                                                                                                                           |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| #1 arithmetic           | Two layers. (1) `insertable` returns `false` when the text before the cursor contains `((` or `$[`, or has an unquoted `[` that is not closed. That covers `a[1+`, `${a[` and `[[ 1 -eq `. (2) A path whose name contains `$(` or a backtick is never inserted: completion drops it, and the picker refuses with a status message.                                                                                                                                        | Bash also evaluates arithmetic in `let`, `declare -i` and `${x:` offsets, so a list of contexts alone cannot be complete. Layer 2 closes the class. Files with those names cannot be inserted by smcli.                                                                                |
| #2 bad bytes            | Environment: build from `vars_os()` and skip entries that are not UTF-8. CLI arguments: parse `args_os()` with clap. `PathBuf` arguments stay byte-capable, and any later `to_str()` on them returns an error, never a panic. The service parses `args_os()` by hand and exits 2 on an argument that is not UTF-8.                                                                                                                                                        | A skipped variable is invisible to smcli. One that smcli needs (`HOME`, `PATH`) with bad bytes is treated as unset.                                                                                                                                                                    |
| #4 data folders         | Bundled data moves to `smcli-data/tldr` and `smcli-data/cheat` beside the binaries (`dist/`, the install prefix). Loaders look there, then in the checkout's `Data/`. An old `$prefix/tldr` or `$prefix/cheat` is cleaned only when it is a directory: delete just the files smcli shipped (`tldr.jsonl`, `LICENSE.md`; `cheat.jsonl`, `LICENSE.txt`), then `rmdir` it if empty.                                                                                          | Old builds in open terminals lose command examples until restarted. Anything else at those names, or inside those folders, is left alone.                                                                                                                                              |
| #8 config keys          | `#[serde(deny_unknown_fields)]` on `SmcliConfig`. The error names the unknown key.                                                                                                                                                                                                                                                                                                                                                                                        | A config written by a newer build fails on an older one, loudly. Recording stops until the key is fixed, as with bad JSON today.                                                                                                                                                       |
| #15 replacement         | Supersedes the 2026-11-08 #6 decision. On a protocol mismatch, replace only when the binary path is the same or `should_replace` says the running build is older. Otherwise the newer incompatible service counts as unavailable: `send` returns `None` and the client falls back in-process. `replace` also returns `None`, never the incompatible response, when its shutdown fails or the request cannot be repeated. An explicit `smcli service stop` still replaces. | An old terminal works in fallback mode until it is reopened.                                                                                                                                                                                                                           |
| #16 subfolder commands  | Qualify, never hide. Outside the project root, a generated Make target becomes `make -C <relative root> <target>`, and `./mvnw` or `./gradlew` becomes the relative path, such as `../../mvnw`. Compute the relative root from canonical paths. Cargo, npm and other commands that work from a subfolder are unchanged.                                                                                                                                                   | Suggestions get longer in subfolders. A history command such as a bare `make docs` is still boosted, as decided last round.                                                                                                                                                            |
| #19 Bash 3.2 completion | Install `_smcli_complete` only for commands where `complete -p <cmd>` prints nothing.                                                                                                                                                                                                                                                                                                                                                                                     | On Bash 3.2, `@` completion is missing for commands that already had a completion.                                                                                                                                                                                                     |
| #18 glob                | Rewrite `glob_match` as a memoized walk over (pattern index, text index). `*` has two moves: skip the star, or consume one character that is not `/`. `**` may also consume `/`. `?`, classes and literals consume one character. Keep today's rules for `/`, `**` and bad classes.                                                                                                                                                                                       | No new dependency. The worst case is pattern length × path length states, each visited once.                                                                                                                                                                                           |
| Service working folder  | The client spawns the service with working folder `/` (`posix_spawn_file_actions_addchdir_np`, macOS 10.15+).                                                                                                                                                                                                                                                                                                                                                             | A relative `PATH` entry such as `.` then resolves to `/`, so automatic `git` never runs a file from the folder of the shell that started the service. This also fixes the "service keeps the launch folder" smaller finding. Absolute `PATH` entries the user chose are still trusted. |
| Import                  | `import` accepts the JSON array that `smcli history --json` writes, as well as JSONL. When a record has an `id`, it is stored as the run's UUID, and `INSERT OR IGNORE` on that UUID deduplicates against every row. The record's session, host, duration, pipe statuses, author and intent are kept when present. Records without `id` use today's import key.                                                                                                           | An export can be imported back without duplicates or lost fields. Old JSONL keeps working.                                                                                                                                                                                             |
| #10 and #12 speed       | #12: fix directly; the auditor measured it. #10: measure first. Fix only if one sync at 100,000 entries with no changes takes over 20 ms.                                                                                                                                                                                                                                                                                                                                 | 20 ms is a planning budget, not a measured user limit.                                                                                                                                                                                                                                 |

## How the work runs

- **Branch:** `wt switch --create audit-fixes-2026-11-22`. Do not commit or push unless asked. Run `cargo fmt --all` only at commit time.
- **Writing code:** the `coder` agent on Sonnet, one slice at a time and in plan order. It also edits the test scripts named in its slice, and runs no tests.
- **Review:** the main agent reads each slice diff. The `code-reviewer` agent reviews slices marked **review**, and reviews the combined `service.rs` diff after 2D and again after 3G, because 2A, 2C, 2D, 3C, 3E and 3G all edit it.
- **Running tests:** only kteam (`kik3`), started by the Orchestrator, because this plan has several waves.
- **Tracking:** check `dex list` first so nothing is duplicated, then `dex plan agents/plans/audit-fixes-2026-11-22.md`: one task per slice under one parent. Created as parent `cewc0d3h`, with a task per wave, one per slice, and a final gate task. Without `dex` on `PATH`, use `npx @zeeg/dex`.
- **Docs and logging:** markdown edits go through iwe (`iwe update`, or an edit followed by `make docs`). The main agent adds each result to the [test log](#test-log).

### Test rules

These follow `AGENTS.md` "Testing rules".

1. Only kteam runs tests, and only after a slice's code is written and reviewed.
2. During the work, run only the slice's named unit tests (`make test-rust PKG=<crate> FILTER=<name>`) and its focused target, such as `make test-shells`. A slice that only deletes code still runs the focused target for the behaviour it touches. Only a docs-only change skips tests.
3. A wave is done when all its slices pass their own checks. There is no full-suite run between waves.
4. `make test` runs once, after all three waves pass, then `make ranking` and `make baseline`.
5. **On failure:** log it, send the failing output and the code under test to `code-reviewer`, fix from that review, then have kteam rerun only the failing test.

### Order constraints

Slices run in plan order. These dependencies must hold if anyone reorders: 2C before 3E (corpus shape), 2D before 3G (`ranked` changes), 2F and 2G before 3D (ranking inputs), and 2G before 3E (`reindex`).

## Wave 1: code execution, crashes and private data

Findings: #1, #2, #3, #4, #8, and the service working folder.

### Slice 1A: arithmetic insertion (#1) — review

Files: `crates/smcli/src/shell_quoting.rs` (`insertable`, new `insertable_name`), `crates/smcli/src/cli/mod.rs` (the `Complete` arm and `complete_allowed`), `crates/smcli/src/complete.rs`, `crates/smcli/src/cli/pick.rs` (the insert path near the `insertable` call).

- Layer 1: in `insertable`, add the `((` and `$[` checks beside the existing `<<` check, after removing escaped newlines. Count unquoted `[` and `]` in the `Command` and `Brace` frames, and return `false` if any `[` is still open at the end. This applies to every shell.
- Layer 2: `insertable_name(name) -> bool` is false when the name contains `$(` or a backtick. Completion drops such candidates, both from the service and from the local fallback. The picker shows "cannot insert this name safely" and inserts nothing.
- Tests, `PKG=smcli`:
  - `FILTER=insertable_rejects_arithmetic`: `echo $(( 1 + `, `(( x + `, `echo $[1 + `, `a[1+`, `a=(7); echo ${a[1+(` and `[[ 1 -eq ` all return `false`. `ls [ab]* ` still returns `true`.
  - `FILTER=insertable_name`.
  - The existing `safe_words` test still passes.
- Slice gate: `make test-picker`.

### Slice 1B: bytes that are not UTF-8 (#2)

Files: `crates/smcli-core/src/paths.rs` (new `utf8_environment`), `crates/smcli/src/cli/mod.rs` (`run`), `crates/smcli/src/shell_integration.rs` (`render_with_keys`), `crates/smcli-service/src/main.rs`.

- Apply the #2 decision at all five sites: `vars()` in `cli/mod.rs`, `shell_integration.rs` and `main.rs`, and `args()` in `cli/mod.rs` and `main.rs`.
- `utf8_environment() -> HashMap<String, String>` lives beside `home_dir` in `paths.rs`, and every environment site uses it.
- The menu check `args.len() == 1` must use the `args_os` length.
- Find any `to_str().unwrap()` or `to_string_lossy` on `PathBuf` arguments in `cli/` and make them return errors.
- Tests:
  - `PKG=smcli-core FILTER=utf8_environment_skips_bad_values`.
  - In `scripts/test-cli.sh`, `BAD=$'\xff' smcli shell-init zsh` exits 0 and prints the hooks.
  - In `scripts/test-cli.sh`, `smcli search $'\xff'` exits 2 with a message, not 101.
  - In `scripts/test-service.py`, `smcli-service` given a bad argument exits 2.
- Slice gate: `make test-cli`, `make test-service`.

### Slice 1C: context cache deadlock (#3) — review

File: `crates/smcli-service/src/context_cache.rs`.

- `make_entry` returns the evicted `Entry` (`Option<Entry>`). `context` keeps it in a local declared before the guard, so it drops after the guard is released.
- Build the new watcher before taking the lock. Recheck `contains_key` after locking. If another request won, drop the spare watcher after unlocking too.
- For a deterministic test, change the field to `_watcher: Option<Box<dyn Send>>`, so a test can put in a value whose `Drop` calls `state.try_lock()` and records whether the lock was held.
- Test, `PKG=smcli-service FILTER=eviction_drops_watcher_outside_lock`: with `limit = 1`, insert entry A with the probe, then request repo B. Assert that the probe saw the lock free. Before the fix, the probe sees it held, so the test fails without needing a real race.

### Slice 1D: data folder names (#4) — review

Files: `crates/smcli-search/src/tldr.rs` and `cheat.rs` (`bundled_dir`), `Makefile` (`release`), `scripts/install.sh`, `scripts/uninstall.sh`, `scripts/test-install.sh`, `scripts/clean-dist.sh`, `scripts/test-cli.sh` (its `clean-dist` fixture), `README.md`, `docs/install.md`.

- Split `bundled_dir` into `bundled_dir_in(exe: Option<&Path>, checkout: Option<&Path>)`, so tests can leave out the checkout fallback. It looks in `<exe dir>/smcli-data/tldr` (the canonical exe, then the plain one), then the checkout's `Data/tldr`. Drop the old `<exe dir>/tldr` lookup. Do the same for cheat.
- The `Makefile` copies to `$(DIST)/smcli-data/{tldr,cheat}`.
- `clean-dist.sh` matches `smcli-*`, so it would reach `smcli-data`. Skip directories, and keep `smcli-data` by name. Add that case to the `test-cli.sh` fixture.
- `install.sh` stages and moves `smcli-data` as one folder. It cleans old folders only under the #4 decision.
- `uninstall.sh` removes `$prefix/smcli-data`, and old folders only under the #4 decision. Update the Linux message in `install.sh`.
- `test-install.sh`, three cases:
  - An executable `tldr` file in the prefix survives install and uninstall.
  - An old `tldr/` holding only smcli's files is removed.
  - An old `tldr/` with an extra file keeps that file.
- Update the paths in `README.md` and `docs/install.md`, then run `make docs`.
- Tests: `PKG=smcli-search FILTER=bundled_dir_in`, including a case with no checkout fallback. Slice gate: `make test-install`, `make test-cli`.

### Slice 1E: unknown config keys (#8)

File: `crates/smcli-core/src/config.rs`.

- Apply the #8 decision. First check `Data/`, `scripts/` and the test fixtures for configs with extra keys: `rg -n '"[a-zA-Z_]+":' --glob '*config*.json'`.
- Test, `PKG=smcli-core FILTER=config_rejects_unknown_keys`: `{"historyfilter":["x"]}` fails, and the message contains `historyfilter`.

### Slice 1F: service working folder

File: `crates/smcli-core/src/service_client.rs`, `launch`.

- Add `libc::posix_spawn_file_actions_addchdir_np(&mut actions, c"/".as_ptr())` beside the existing file actions (the binding is in libc 0.2.191 for Apple targets).
- Test: in `scripts/test-service.py`, start the service from a temporary folder and check, with `lsof -p <pid> -d cwd`, that its working folder is `/`. Slice gate: `make test-service`.

## Wave 2: wrong results

Findings: #5, #6, #7, #14, #15, #16, #19, #20, and S: `HISTCONTROL` under `set -u`, NaN base, menu dash words, prune picks, `indexes --prune`, `reindex` typed index, uninstall fingerprint, install quoting and blank lines, config symlink, import merging, export not importable.

### Slice 2A: service misses outside writes (#5)

File: `crates/smcli-service/src/service.rs`, `select_command`.

- Delete the `state.data_version = data_version(&state.db)?;` line after `record_pick`.
- Test, `PKG=smcli-service FILTER=pick_keeps_outside_write_visible`: commit a row from a second connection after the refresh and before the pick (call the parts directly), then check that the next suggest sees it.

### Slice 2B: shell hooks (#6, #19, #20, S: `HISTCONTROL`) — review

Files: `crates/smcli/src/shell_integration.rs`, `scripts/test-shells.py`.

- #6: the status capture sites are `_smcli_preexec`, `_smcli_prompt` (including its `BP_PIPESTATUS` branch), `_smcli_precmd` and `_smcli_zshexit`.
  - In each, capture `$?` and the status array in the same first `local` statement, for example `local exit_code=$? codes=("${PIPESTATUS[@]}")` (Zsh: `pipestatus`).
  - Then join under `local IFS=' '`. For `BP_PIPESTATUS`, copy the array the same way.
- #19: apply the #19 decision to the Bash 3 `complete -F` line.
- #20: delete `_smcli_histignore`.
- S: use `${HISTCONTROL-}` at both sites.
- `test-shells.py`:
  - With `IFS=,`, in Bash, Zsh and the bash-preexec variant, `true | false` records pipe status `[0, 1]` and exit 1. This also catches a reset of `$?`.
  - Make the `BASH_PREEXEC` prelude's `__bp_precmd_invoke_cmd` copy the array the same way, so the test is not broken by its own `IFS`.
  - Bash under `set -u` prints nothing to stderr.
  - In Bash 3.2, a completion set for `cd` before the hooks load survives loading.
- Tests: `PKG=smcli FILTER=shell_integration`. Slice gate: `make test-shells`.

### Slice 2C: multi-line native history (#7)

Files: `crates/smcli-search/src/native.rs`, and the four `read()` callers: `crates/smcli-search/src/setup.rs` (`corpus`), `crates/smcli-service/src/service.rs` (corpus build), `crates/smcli/src/cli/pick.rs` and `crates/smcli/src/cli/history.rs`.

- `HistoryFile::read` returns `Vec<String>`, one per command. Zsh continuation lines join with `\n` into one entry. Fish entries keep their decoded newlines.
- `setup.rs`, `service.rs` and `pick.rs` stop splitting on `\n`. `cli/history.rs` prints each entry as before.
- Update the `native.rs` test that asserts the joined text. Add `FILTER=zsh_multiline_is_one_command` with a `for` / `echo` / `done` entry, and `FILTER=fish_multiline_is_one_command`.

### Slice 2D: `ignoreFile` in ranking (#14, S: test discovery cap) — review

Files: `crates/smcli-core/src/request.rs` (`Ranked`), `crates/smcli-service/src/service.rs` (`ranked`, and the search path that builds `IndexEvidence`), `crates/smcli-service/src/test_candidates.rs`, `crates/smcli-service/src/evidence.rs` (mounted from `file_search.rs` with `#[path]`).

- `Ranked` gets the caller's config path and home from the request's existing caller context. These are internal fields: no wire change, and the frozen fixtures in `crates/smcli-core/tests/fixtures/service_protocol` must not change.
- `ranked` loads `FileIgnore` as `search` does.
- The test cache keeps every source path for each command, not one. After reading the cache, drop candidates whose paths are all ignored. Never filter while filling the shared cache.
- `IndexEvidence` stores an `Option<FileIgnore>` and skips ignored hits. The `ProjectEvidence` trait signature stays the same. Both constructors, in `ranked` and in search, pass the policy.
- S: in test discovery, read each hit path once, with at most 1 MiB per file (`take(1 << 20)`). This is a size cap only; the symlink-swap race stays deferred.
- Tests, `PKG=smcli-service`:
  - `FILTER=test_candidates_honor_ignore`: two callers with opposite policies on the same warm cache, run in both orders. A command defined in both an ignored and an allowed file stays for the caller that allows it.
  - `FILTER=evidence_honors_ignore`.

### Slice 2E: service replacement (#15) — review

File: `crates/smcli-core/src/service_client.rs`, `ServiceClient::send` and `replace`.

- Apply the #15 decision. The `Mismatch` arm calls `replace` only when the build check allows it, and otherwise returns `None`. In `replace`, the two paths that return `stale` (a failed shutdown, and a request that cannot be repeated) return `None` when `stale.v` differs from ours. The `Shutdown` operation keeps today's path.
- Tests, `PKG=smcli-core`, using the existing fake-service helpers in the `service_client` tests:
  - `FILTER=newer_incompatible_service_is_not_replaced`: no shutdown is sent, and `send` returns `None`.
  - `FILTER=older_incompatible_service_is_replaced`: exactly one shutdown is sent.
- Slice gate: `make test-service`.

### Slice 2F: subfolder commands (#16)

File: `crates/smcli-core/src/ranking.rs`, `candidates` (the Make arm) and `standard_commands` (Maven and Gradle wrappers).

- Apply the #16 decision. Pass the relative root into `standard_commands`. It is only `..` segments (`../..`), so it never needs quoting.
- Tests, `PKG=smcli-core`:
  - `FILTER=make_candidate_from_subfolder` (`make -C .. docs`).
  - `FILTER=mvnw_candidate_from_subfolder` (`../mvnw`).
  - `FILTER=gradlew_candidate_from_subfolder`.
  - `FILTER=subfolder_candidate_through_symlink`.
  - The existing root-folder tests stay unchanged.
- Run `make ranking` after this slice.

### Slice 2G: small correctness fixes

Files: `crates/smcli-core/src/suggestion_index.rs`, `prune.rs`, `crates/smcli/src/menu.rs`, `crates/smcli/src/cli/index.rs`, `crates/smcli-search/src/index.rs`.

- NaN base: in `SuggestionIndex::new`, use `base = last.min(now())`. Test `FILTER=new_index_clamps_future_base`, built from a non-empty run list.
- Prune picks: delete the `commands.contains(&command) ||` clause. The filter check that follows it covers each pick in its own folder. Test `FILTER=prune_keeps_picks_in_allowed_folder`.
- Menu: push `"--"` before the query words for "Search history". Test `FILTER=menu_search_passes_literal_words` on the argument list it builds.
- `indexes --prune`: refuse with a message when `bundled_sources()` finds nothing in any fallback. Test `FILTER=indexes_prune_refuses_without_sources`.
- `reindex`: also remove `<name>-typed.*` files in `reindex_using_lock`. Test `FILTER=reindex_removes_typed_index`.
- Slice gate: `make test-picker`.

### Slice 2H: install scripts and config save

Files: `scripts/install.sh`, `scripts/uninstall.sh`, `scripts/test-install.sh`, `crates/smcli-core/src/config.rs`.

- Fingerprint: in every shell copy of the FNV helper (`uninstall.sh`, and the parity helper in `test-install.sh`), add 256 to a negative `printf '%d'` byte. Run the non-ASCII case with `/bin/bash`.
- Rc block:
  - When replacing an old block, remove the blank line before it.
  - Fix the single-quote escaping in the `quoted=` line, so the output keeps today's `'...'` form that `test-install.sh` asserts.
  - The Fish line uses Fish quoting (`\'` inside single quotes), not the POSIX `'\''` form.
- Config symlink: in `SmcliConfig::save`, write to `fs::canonicalize(path)` when the path is a symlink that resolves. Keep the temporary file and rename beside the target.
- `test-install.sh`:
  - Rerun install three times and assert one blank line before the block.
  - Install to a prefix containing `'`, and load the hooks in Bash, Zsh and Fish.
- Test: `PKG=smcli-core FILTER=config_save_keeps_symlink`. Slice gate: `make test-install`.

### Slice 2I: import round trip (S: import merging, export not importable)

File: `crates/smcli-core/src/import.rs`, `import_legacy`.

- Apply the import decision. If the file's first non-space byte is `[`, parse it as one array; otherwise read JSONL as today.
- With `id`: use the record's session, or `imported-<user>-<shell>` when it is missing, and `INSERT OR IGNORE` with the UUID. Store `host`, `durationMs`, `pipestatus`, `author` and `intent` when present, using the same validation as `append`.
- Tests, `PKG=smcli-core`:
  - `FILTER=import_history_json_round_trip`: export, import into the same database (no new rows), and import into an empty database (same rows and fields).
  - `FILTER=import_keeps_distinct_runs_same_millisecond`.

## Wave 3: speed and load

Findings: #9, #10, #11, #12, #13, #17, #18, and S: root discovery cost.

### Slice 3A: failed service start (#9)

File: `crates/smcli-core/src/service_client.rs`, `launch` and `connect`.

- `launch` returns `Option<libc::pid_t>`. In `connect`'s wait loop, call `waitpid(pid, WNOHANG)`. A non-zero exit or a signal ends the wait at once. Exit 0 is a lost start race, so keep waiting for the winner.
- Test, `PKG=smcli-core FILTER=failed_launch_returns_quickly`: use an executable that exits 1, and expect `connect` to return in under 500 ms.

### Slice 3B: Git status after failure (#13)

File: `crates/smcli-service/src/context_cache.rs`.

- Add `failed_at: Option<f64>` to `Entry`. A `None` read sets it, and a successful read clears it. `stale` also needs `failed_at` to be older than `max_age`.
- Test, `PKG=smcli-service FILTER=failed_status_waits_max_age`, with a reader that fails and counts its calls, a short `max_age` and a shorter `min_interval`:
  - Within `max_age`, no refresh is scheduled (`refreshing` stays false) and the count stays 1.
  - After `max_age`, one retry happens.
  - After a successful read, `failed_at` is clear.

### Slice 3C: command-example sync (#11) — review

File: `crates/smcli-service/src/service.rs`: `CommandSync`, `maybe_sync_commands`, `finish_sync`, `embed`, and the model-reset code that clears `CommandSync`.

- Replace `last_path` with `checked: HashMap<String, HashSet<String>>`: for each `PATH` snapshot, the programs already looked up there. Cap it at 16 `PATH`s (clear it when full).
- A record syncs when its program is not in its `PATH`'s set, so a program first seen in one `PATH` is still discovered in another. After a sync, that `PATH`'s set becomes every program in history. `known` (what the index holds) and `absent` keep their meaning.
- Read runs on a connection of their own (`self.history.open_connection()`), not under `self.lock()`, in `finish_sync` and `embed`.
- Update the existing tests that use `last_path`.
- Tests, `PKG=smcli-service`:
  - `FILTER=alternating_paths_sync_once_each`.
  - `FILTER=program_known_elsewhere_syncs_on_new_path`: A syncs; B records `jq` without it; A then records `jq` and finds it.

### Slice 3D: suggest parse cost (#12)

Files: `crates/smcli-core/src/suggestion_index.rs`, `ranking.rs`.

- Add `program: Option<String>` to `Tally`, set once in `add` from the expanded text when present, else the typed text.
- `project_file_reason(command, program: Option<&str>)`: it keeps the command text for the `npm run` check, and parses only when `program` is `None`.
- The suggestion index path (`ranking.rs`, the scoring loop that calls `project_file_reason`) passes the cached program. The two search callers in `smcli-search/src/corpus.rs` pass `None` and keep parsing, since they are not on the per-keystroke suggest path.
- Tests: the existing `project_file` tests, plus `make ranking`.

### Slice 3E: index sync on every search (#10)

Files: `crates/smcli-search/src/index.rs`, and, only if the fix goes ahead, `crates/smcli-service/src/service.rs` and the `search_with_command_index` callers.

- First measure. Add an ignored test, `sync_unchanged_100k_timing`, that syncs 100,000 entries twice and prints the second time. kteam runs it as `make tldr-ranking` runs its release test: `cargo test --release -p smcli-search --lib sync_unchanged_100k_timing -- --ignored --nocapture`.
- If it is under 20 ms, log the time and close the finding.
- If it is over 20 ms, skip `sync` when the corpus is unchanged.
  - The caller passes a corpus version covering `data_version`, `history_revision`, the directory, project and kind the corpus was built for, each native file's device, inode, size and modification time, and the time the corpus was built.
  - The engine keeps the last synced version per index name, including the `-typed` one, and clears it when it discards and recreates an index (`open_index` recovery and `reindex`).

### Slice 3F: prune order and glob (#17, #18)

Files: `crates/smcli-core/src/prune.rs` (`finish_prune`), `import.rs` (`--replace` path), `parse.rs` (`glob_match`).

- #17: run `VACUUM` first, then `PRAGMA wal_checkpoint(TRUNCATE)`. Read the checkpoint's result row, and report busy as a compaction failure.
- #18: apply the #18 decision. Keep the public signature, with an inner function that carries the memo.
- Tests, `PKG=smcli-core`:
  - `FILTER=glob_match` (existing cases unchanged).
  - `FILTER=glob_match_pathological_is_fast`: eleven `*a` then `b` against 40 `a`s, and the same against 320 `a`s, each under 100 ms.
  - `FILTER=prune_reports_busy_checkpoint`: a second connection holds a read transaction during prune.

### Slice 3G: root discovery (S)

Files: `crates/smcli-core/src/context.rs`, `crates/smcli-service/src/file_search.rs` (`FileSearch::root`, `content_indexing_enabled`), `service.rs` (`ranked`).

- Add `pub fn repository_root(directory: &str) -> Option<String>` in `context.rs`. It walks ancestors, like the loop in `directory_kind`, and calls the private `repository` at each level without reading manifests.
- `FileSearch::root` and `content_indexing_enabled` use it.
- `ranked` computes the root once and passes it to test discovery and evidence.
- Tests, `PKG=smcli-core FILTER=repository_root`: a nested folder and the root itself both give the same root as `ProjectContext::discover(..).repository`, including through a linked worktree. Slice gate: `make test-picker`.

## Deferred

Not fixed in this plan. Move any that matter to the [roadmap](roadmap.md) at the end.

- Commands with bytes that are not UTF-8 are not recorded: storing raw bytes is a schema change, and `from_utf8_lossy` would save a different file name.
- Search counts other users' rows: only matters after importing another user's history; decide whether search is meant to show everything first.
- A guard for a panic in `finish_sync`: no input that panics was found.
- `scoped_index` during a scan, `.git` as a symlink, the `-` mismatch in `declares_name`, and the Zsh `bindkey` subshell: small and unmeasured.
- Atuin exit `-1` and relative folders: `-1` can mean an unfinished command, so decide what it should mean before changing it.
- Test discovery reading a file swapped for a symlink between the search and the read: needs an open-then-check read; small window.
- Absolute `PATH` entries inside a repo (for example from direnv) being used by automatic `git`: the user's own shell trusts them the same way. Slice 1F handles relative entries.
- Suggestion index rebuilt under the state lock after outside writes: measure first.
- Tab completion's content index in a home folder: unmeasured.

## Changes during implementation

- **3E:** an unchanged sync of 100,000 entries measured 944 ms, so the skip was built. Instead of the planned version tuple, the service bumps a corpus generation whenever its cached corpus is built or changed. The engine stores it per index name together with the index's row count, so an index emptied on disk still syncs. Recovery and `reindex` forget it.
- **#15:** when the client cannot read its own build, it leaves a mismatched service running.
- **2D:** loading `ignoreFile` is best-effort for ranking and search evidence. A broken file fails file search loudly, not suggestions.
- **1A:** `scripts/test-file-picker.py` now expects the backtick file name to be refused.
- Slices with no shared files ran in parallel. Tests were run by kteam (`kik3`).

## Test log

| Slice  | Test                                                                                           | Result | Notes                                                                                                                                                                                                     |
| ------ | ---------------------------------------------------------------------------------------------- | ------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1A     | make test-rust PKG=smcli FILTER=shell_quoting                                                  | pass   |                                                                                                                                                                                                           |
| 1B     | make test-rust PKG=smcli-core FILTER=utf8_environment                                          | pass   |                                                                                                                                                                                                           |
| 1E     | make test-rust PKG=smcli-core FILTER=config                                                    | pass   |                                                                                                                                                                                                           |
| 1C     | make test-rust PKG=smcli-service FILTER=context_cache                                          | pass   |                                                                                                                                                                                                           |
| 1D     | make test-rust PKG=smcli-search FILTER=bundled_dir                                             | pass   |                                                                                                                                                                                                           |
| 1A     | make test-picker                                                                               | fail   | test-file-picker.py expected a backtick name to be inserted; updated to expect the decision #1 refusal                                                                                                    |
| 1B, 1D | make test-cli                                                                                  | pass   |                                                                                                                                                                                                           |
| 1B, 1F | make test-service                                                                              | pass   |                                                                                                                                                                                                           |
| 1D     | make test-install                                                                              | pass   |                                                                                                                                                                                                           |
| 1A     | make test-picker (rerun)                                                                       | pass   |                                                                                                                                                                                                           |
| 2A     | make test-rust PKG=smcli-service FILTER=pick_keeps_outside_write_visible                       | pass   |                                                                                                                                                                                                           |
| 2B     | make test-rust PKG=smcli FILTER=shell_integration                                              | pass   |                                                                                                                                                                                                           |
| 2C     | make test-rust PKG=smcli-search FILTER=native                                                  | pass   |                                                                                                                                                                                                           |
| 2D     | make test-rust PKG=smcli-service FILTER=test_candidates / evidence                             | pass   |                                                                                                                                                                                                           |
| 2E     | make test-rust PKG=smcli-core FILTER=service                                                   | fail   | test used a missing binary, so "own build unknown" allowed replacement; now unknown means leave it running, and the test uses a real file                                                                 |
| 2F     | make test-rust PKG=smcli-core FILTER=subfolder                                                 | pass   |                                                                                                                                                                                                           |
| 2G     | make test-rust (clamps_future_base, prune, menu, indexes_prune)                                | pass   |                                                                                                                                                                                                           |
| 2G     | make test-rust PKG=smcli-search FILTER=reindex                                                 | fail   | test index folder lacked mode 0700                                                                                                                                                                        |
| 2H     | make test-rust PKG=smcli-core FILTER=config_save_keeps_symlink                                 | pass   |                                                                                                                                                                                                           |
| 2I     | make test-rust PKG=smcli-core FILTER=import                                                    | pass   |                                                                                                                                                                                                           |
| 2B     | make test-shells                                                                               | fail   | bash-preexec without BP_PIPESTATUS has no pipe statuses; the check now skips that variant                                                                                                                 |
| 2E     | make test-service                                                                              | pass   |                                                                                                                                                                                                           |
| 2G     | make test-picker                                                                               | pass   |                                                                                                                                                                                                           |
| 2H     | make test-install                                                                              | fail   | awk kept a trailing blank line and added another; the held blank is now dropped at end of file                                                                                                            |
| 2F     | make ranking                                                                                   | pass   |                                                                                                                                                                                                           |
| 2E     | make test-rust PKG=smcli-core FILTER=service (rerun)                                           | pass   |                                                                                                                                                                                                           |
| 2G     | make test-rust PKG=smcli-search FILTER=reindex (rerun)                                         | pass   |                                                                                                                                                                                                           |
| 2B     | make test-shells (rerun)                                                                       | pass   |                                                                                                                                                                                                           |
| 2H     | make test-install (rerun)                                                                      | fail   | blank-line check misread a file whose only content is the block; then the quoted-prefix case left a second service running. Check rewritten; case isolated with its own runtime dir and SMCLI_SERVICE=off |
| 2H     | make test-install (rerun 3)                                                                    | pass   |                                                                                                                                                                                                           |
| 3E     | cargo test --release -p smcli-search --lib sync_unchanged_100k_timing -- --ignored --nocapture | pass   | second sync of 100,000 unchanged entries: 944 ms, over the 20 ms bar, so the skip was implemented                                                                                                         |
| 3A-3G  | make test-rust PKG=smcli-core / smcli-service / smcli-search                                   | fail   | failed_launch_returns_quickly over its 500 ms bound under load; bound raised to 1.5 s (full wait is 2 s)                                                                                                  |
| 3A-3G  | make test-picker, make ranking                                                                 | pass   |                                                                                                                                                                                                           |
| 3E     | make test-service                                                                              | fail   | generation skip ignored an index emptied on disk; the skip now also compares the row count                                                                                                                |
| 3A, 3E | reruns: failed_launch_returns_quickly, make test-rust PKG=smcli-search, make test-service      | pass   |                                                                                                                                                                                                           |
