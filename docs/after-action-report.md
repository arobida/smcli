# After-action report — final audit fixes (2026-10-22)

Branch `rust-port`, base `cfe9b2f`. Audit: `docs/caveman-review.md` ("Final audit"). Plan: `docs/fix-plan.md`. Per-batch notes: `docs/fix-notes-{A,B,C}.md`.

## How the work was split

| Batch | Agent | Scope | Result |
|-------|-------|-------|--------|
| A | pikcode (Kimi K2.7 Code) | `crates/smcli`, `index_name`, `home_dir`, Linux spawn | Done; one follow-up by orchestrator |
| B | pikcode (Kimi K2.7 Code) | `smcli-core` internals, `smcli-service` | Done; two follow-ups by orchestrator |
| C | pisol (GPT-6.1 Sol) | `scripts/`, `docs/install.md` | Done as written |

Each batch owned its own files, so the agents ran in parallel. No agent ran a full build: each ran one `cargo check` on its crate. The orchestrator ran `make test` twice, `make ranking` once and `make fresh` once.

## What was fixed (audit numbers)

**Must fix**
1. Uninstall no longer copies a live database with `cp`. Without `sqlite3` it stops before removing anything.
2. Folder pick in the current directory inserts `./`, not a root-relative path. Unit test added.
3. Uninstall export is private: `umask 077`, export dir `0700`, files `0600` (existing destinations too). `test-install.sh` checks both 1 and 3.
4. A far-future start time is clamped to now in `SuggestionIndex::add`, so service scores stay finite. Test added.
5. A record that arrives out of order for its session makes `SuggestionIndex::add` return `false`; the service drops the index and rebuilds it in time order. Test added.

**Should fix**
6. All three hooks pass `--` before the command line.
7. `warm_search` loads the model outside the `state` lock. Native history and `command_stats` are read outside it during a corpus rebuild.
8. `smcli prune` rebuilds the search index when it removed anything. If the rebuild fails, it prints a warning and tells you to run `smcli reindex`.
9. Non-finite or negative `SMCLI_SERVICE_IDLE` / `SMCLI_FFF_IDLE` fall back to the default instead of panicking.
10. Service stop uses `_exit` after unlinking the socket.
11. The `strerror` call is gone; socket errors use `io::Error`'s own text.
12. glibc Linux: the service spawn closes inherited fds ≥ 3 (`posix_spawn_file_actions_addclosefrom_np`).
13. `smcli health` calls `HistorySearch::index_name()` instead of a copied format string.
14. `new_event_id` returns an error instead of an all-zero id.
15. One `home_dir` helper (`HOME`, then `getpwuid`, then `/`) is shared by the CLI and `ServiceClient`.
16. A panic while building a file index becomes an error, so pool cleanup always runs.

**Nice to have**
17. `shell-init` prints the config error and falls back to default keys.
18. Picker setup leaves the alternate screen on error; the unused `height` parameter is gone.
19. The concurrent-writer check waits on every writer and lists each failure. The context-cache test uses a channel, not a timing race.
20. The service lock exits quietly only on `WouldBlock`; other lock errors are reported.

## Orchestrator follow-ups (changes made after review)

- **Prune (A):** the agent wrote `let _ = …reindex(…)`, which threw away a failed rebuild. That would leave pruned secrets in the index with no message. Changed to a warning that names `smcli reindex`.
- **Corpus (B):** the agent's version re-took the lock and called `state.corpus.as_ref().unwrap()`. A fallback write from another process clears the corpus in between, so that could panic. A record landing mid-rebuild would also be dropped from search for up to 5 minutes. Now `current_corpus` returns copies directly. It stores the new corpus only if `data_version` did not move while it was built; otherwise the copy is used once and rebuilt on the next search.
- **Test (B):** `rebases_and_keeps_latest_fields` added an older run in the same session, which is exactly what #5 now rejects. The older run moved to a second session, so the test still proves an older run doesn't overwrite the last-run fields.

## Verification

- `make test`: **passed**. All Rust suites, bash 3.2/5.3 (all modes), zsh, fish, plugin composition, concurrent append, service, file picker, install/uninstall with export, history.
- `make ranking`: suggest MRR **1.000**; search MRR **0.955**, top-1 **0.91**. Same as the recorded baseline.
- `make fresh`: `dist/smcli-20261022-154328-cfe9b2f`. Smoke checks: the new hooks contain `pick "$@" -- …`, and `shell-init` with a broken config warns and still prints hooks.
- `graphify update .` run.

## New issues found along the way

- **#5 changed an index contract.** `SuggestionIndex::add` used to accept older runs in the same session. Callers now have to handle `false`. Only the service calls it, and it does.
- **Model rebuild still holds the lock in one case.** `current_engine` still builds the model under `state` when the config changes after start, or when warm-up failed (noted by batch B). This is rare, so it was left as is.
- **The corpus rebuild window can repeat a rebuild.** If a record lands during every rebuild, each search rebuilds again. This is correct but costs time under a steady stream of records. Not seen in tests.
- **The audit's first 🔴 was overstated.** The hook bug never replaced the line (pick exits 2); it only stopped the picker opening. The audit's suggested Linux fd fix was also wrong (Apple-only flag). Both corrections are recorded in the review.

## Left over

- **Linux:** `make linux` not run, because the Docker/OrbStack daemon is down. That leaves unverified: the glibc `addclosefrom_np` change (it needs glibc ≥ 2.34 at runtime), the historical Fish prompt timeout, and the Linux build. musl still passes inherited fds to the service.
- **Picker resize:** a recorded `shortcut:` decision, unchanged.
- **Dead code:** `shell_integration::render` removed after this report (only tests used it).
- **Release backlog:** license texts for 10 crates, a root `LICENSE`, the xdiff decision, and a stale `.plans/shipit-checklist.md`.
- **Project-search ranking:** one known miss (`swift test --filter Lib`), unchanged.
- **Not committed by this run.** Commit and push are a separate step.
