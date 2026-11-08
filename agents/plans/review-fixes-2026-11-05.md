# Plan: fix the 2026-11-05 review findings (commit 37a7f7b)

Source: Codebase review — 2026-11-05

## Fix per finding

**#1 Uninstall can delete a directory (must fix)** — `scripts/uninstall.sh`

- Confirmed: export only runs when `-f $db`, but line 142 runs `rm -rf "$db" ...`.
- Fix: right after `$db`/`$config` resolve, before prompt or export, `die` if `$db`, `$db-wal`, `$db-shm`, `$db-journal` or `$config` is a directory. Step 4: `rm -f` for files; `rm -rf` only for `"$db.search"`.
- Test: `scripts/test-install.sh` case: `SMCLI_DB=<tmpdir>` exits non-zero, directory and a file inside untouched.
- Run: `make test-install`

**#2 Picker scope change bypasses directory filters (must fix, privacy)** — `crates/smcli-core/src/caller.rs`, `crates/smcli-core/src/request.rs`, `crates/smcli-service/src/file_search.rs`, `crates/smcli-service/src/service.rs`

- Confirmed: `file_selection_request` sends search scope as `directory`; select callback (service.rs:137) filters on that scope.
- Fix:
  - Add `Cwd` to `Select` caller fields in `caller_fields`.
  - Add `cwd` to `Select::File` in request.rs and fill it when parsing (caller field alone does not reach the callback).
  - Pass it through `FileSearch::select_file` to the recording callback as a separate argument.
  - Record only if `filter.allows(query, cwd)` and `filter.allows(query, directory)`. If cwd missing (older client), check `directory` alone.
- No `pick.rs` change: cwd comes from `CallerContext`.
- Fixtures: if encoding or caller-field list changes, append a line to `requests-v4.jsonl`; never edit existing lines. Optional field, no version bump.
- Tests: smcli-service unit test (select from excluded `/r/private`, scope `/r`: nothing recorded) + `scripts/test-service.py` case through the real CLI.
- Run: `make test-service`, `make test-rust PKG=smcli-service`, `make test-picker`

**#3 Relative config path resolves in wrong directory** — `crates/smcli-core/src/config.rs`

- Fix: in `SmcliConfig::location()`, join a relative `SMCLI_CONFIG` onto `PWD` (if absolute), else `current_dir()`. Covers request and in-process fallback (`CallerContext::from_env` calls `location()`).
- Test: unit test `SMCLI_CONFIG=config.json`, `PWD=/a` -> `/a/config.json`.
- Run: `make test-rust PKG=smcli-core FILTER=config`

**#4 Captures never send PATH** — `crates/smcli-core/src/caller.rs:32`

- Fix: add `Path` to `Record` caller fields.
- Test: drop hand-supplied `shell_path` from handler tests, or add a CLI capture test showing discovery runs.
- Run: `make test-rust PKG=smcli-core`, `make test-cli`

**#5 Reindex misreads alias keys** — `crates/smcli/src/cli/tldr.rs:106` (`captured_programs`)

- Fix: use `SearchEntry::from_key(key)` + `.program()`. First check `program()` returns a bare name; else strip path as current code does.
- Test: alias-only history (`j` -> `jq . f.json`) yields `jq`.
- Run: `make test-rust PKG=smcli FILTER=captured_programs`

**#6 Bash binds `\eOA` with keys disabled** — `crates/smcli/src/shell_integration.rs:368`

- Fix: same `(( \#(keyUp) )) &&` guard as `\e[A`.
- Test: shell suite: `--keys none` binds neither; default binds both.
- Run: `make test-shells`

**#7 Metadata updates scan whole index** — `crates/smcli-search/src/index.rs` (`setup_index`)

- Fix: add `"command"` to indexed fields.
- Check first: `setup_index` runs only on new/rebuilt index. Confirm `create_index` is idempotent and runs for existing indexes; if not, call it on open.
- Test: unit test that `list_indexes()` includes `command`. No timing benchmark.
- Run: `make test-rust PKG=smcli-search`, `make ranking`

**#8 Fallback grep unbounded** — `crates/smcli/src/cli/pick.rs` (`LocalFileSearch`, `Mode::Grep`)

- Fix: skip files with metadata len > 10 MiB; read via `File::open(..).take(10 << 20)`. Named constant matching service limit.
- Test: sparse 11 MiB file (`set_len`) gives no hits.
- Run: `make test-rust PKG=smcli`, `make test-picker`

**#9 `make linux` hits macOS-only installer test** — `scripts/test.sh:28`

- Fix: run install suite only when `uname` is `Darwin`; else print `skip install (macOS-only)`.
- Run: `make test`, `make linux` (needs Docker).

**#10 Unused `picker_recents`** — `crates/smcli-core/src/history.rs`

- Fix: delete method, its test, and its line in `crates/smcli-core/src/README.md`. Keep historical mention in `agents/plans/architecture-remediation.md`.
- Run: `make test-rust PKG=smcli-core`, `make docs`

## Work slices (no shared files)

| Slice | Findings   | Files                                                                                   | Agent                          |
| ----- | ---------- | --------------------------------------------------------------------------------------- | ------------------------------ |
| A     | #1         | uninstall.sh, test-install.sh                                                           | `kikcode`                      |
| B     | #2, #3, #4 | caller.rs, request.rs, file_search.rs, service.rs, config.rs, fixtures, test-service.py | `pisol`                        |
| C     | #5, #6     | tldr.rs, shell_integration.rs, shell test script                                        | `piterra`                      |
| D     | #7         | index.rs                                                                                | `kik3-256`                     |
| E     | #8, #10    | pick.rs, history.rs, core README                                                        | `pidflash`                     |
| F     | #9         | test.sh                                                                                 | `piluna` (Orchestrator checks) |

## Order

1. Wave 1 in parallel: A–F.
2. Each slice runs its own `make` slice before handing back.
3. Cross-review: `kik31pro` reviews A+B; `piterra` reviews D+E; `kikcode` reviews C+F; then `code-reviewer` over the whole diff.
4. Final gate: `make test`, `make linux`, `make docs`.

## Commits (agents/reference/commits.md; only when the user asks)

1. `fix(uninstall)`: #1
2. `fix(service)`: #2, #3, #4
3. `fix(search)`: #5, #7
4. `fix(shell)`: #6
5. `fix(picker)`: #8
6. `refactor(core)`: #10
7. `test`: #9

## Deviations from the review

- #7: no performance benchmark; index assertion suffices.
- #2: cwd via caller context, not a new pick.rs field; requests without cwd keep today's check.
- #3: unit test on `location()` instead of two-client test.
- #9: don't fix BSD `stat` flags; suite skipped on Linux.

## Open checks

- #7: does `create_index` run for existing on-disk indexes?
- #5: what does `SearchEntry::program()` return for paths?
- Slice B risk: append to `requests-v4.jsonl`, never rewrite frozen lines.

## Outcome (2026-11-05)

All ten findings fixed; `make test` passed (340 Rust tests, 9 skipped, all black-box suites). `make linux` not run (Docker down).

- #7: `open_cached` runs setup on every open, so existing indexes gain `command`; a test drops it and checks reopen restores it.
- #5: `SearchEntry::program()` already returns the bare name.
- Fixture: two lines appended to `requests-v4.jsonl`; existing lines unchanged.
- Whole-diff review follow-ups: uninstall guard also covers `.v7.bak`/`.migration.lock` and is skipped with `--keep-data`; relative `SMCLI_CONFIG` no longer panics without a cwd; fallback grep skips files that grow past 10 MiB mid-read.
- Not addressed: prune re-checks file picks against their stored root only, not the pick-time cwd.
