---
title: Codebase review — 2026-11-02
---

# Codebase review — 2026-11-02

What this repo does: smcli captures shell commands into SQLite, ranks history, and provides command, file and content search through a local background service. Expected load: one local user with several concurrent shells and up to 100,000 history runs.

## Final scope after the semantic-accuracy merge

Reviewed current main at `16477b7c804c8877e1fc9cf73c467eb9f6e2774c` (`docs(agents): add semantic search accuracy plan and results`), including merged source commit `5a410b9c6e2bbf079c3d9210121db69170b07909` (`feat(search): vector-first intent ranking`). The later commit adds documentation only. Findings 9–11 apply to current main, not an unmerged proposal. The main source/build inputs match the previously reviewed branch commit. Its change from the initial audit baseline, `95d755137f483fa682323738bdf70e6ecbf0a983`, touches nine files (+664/-17 lines).

This report consolidates the two chat audits. Findings 1–8 came from the whole-repository pass at `95d7551`. Findings 9–11 came from the semantic-search change review at `5a410b9`. Numbering is stable even where severity groups put finding 9 before finding 8. Findings 1–11 were addressed in the follow-up fix pass after this report. The merge did not change the implementation paths behind findings 1–8; those reproductions were not individually rerun after the merge.

The audit traced capture/filtering, history writes and migrations, imports and pruning, client/service lifecycle and concurrency, corpus/index construction, ranking, native history decoding, completion and picker behavior, build/test configuration, and install/uninstall scripts. It prioritized data loss, privacy and shared-process behavior rather than claiming every source line was read. No implementation fix, commit or push was made.

The [2026-10-30 review](codebase-review-2026-10-30.md) is historical. Its numbering and fixed statuses are independent of this report. The [semantic-search results report](semantic-search-accuracy-results.md) is supporting material, with the configuration mismatch identified in finding 11.

## Must fix — confirmed defects

### 1. Saving settings can overwrite another file

Source: `crates/smcli-core/src/config.rs:87-94`.

- **What this is:** Settings are written through a temporary file before replacing the config.
- **Problem:** The temporary filename is predictable and follows symlinks. A temporary `config.json.tmp` symlink to a separate file caused `init --keys ctrl-r` to overwrite that file with settings JSON and report success.
- **Fix:** Use the existing `tempfile` dependency to create a unique temporary file beside the destination, then persist it. Add a symlink-target regression test.
- **If we skip it:** A stale or planted symlink can destroy unrelated data accessible to this account. Concurrent saves also share one temporary filename.

Evidence: reproduced with a temporary HOME/config and a victim file containing `DO NOT OVERWRITE`; the victim contained the new key settings afterward. This establishes file clobbering, not a claim of cross-user privilege escalation.

### 2. A prune preview can lose command captures

Source: `crates/smcli/src/cli.rs:668`, `:686`; the preview and real prune both acquire an immediate SQLite transaction.

- **What this is:** `prune --dry-run` previews history cleanup without deleting records.
- **Problem:** It takes a write lock while repeatedly scanning a growing vector for duplicate command keys. During a preview of 60,000 rejected records, concurrent capture failed after 5.19 seconds with `database is locked`.
- **Fix:** Use a read transaction for dry runs and a `HashSet` for command membership. Keep the write transaction for actual deletion.
- **If we skip it:** Merely previewing cleanup can prevent new commands from being saved; actual cleanup also holds the write lock unnecessarily long.

Evidence: temporary SQLite databases with distinct `secret command N` records and `historyFilter: ["^secret"]`. Debug-build preview times were 0.469 seconds for 10,000 records and 6.736 seconds for 40,000. A separate 60,000-record preview overlapped a `record` invocation, which exited 1 after its busy timeout. These are debug measurements, not release performance claims.

### 3. Legacy imports inconsistently bypass privacy filters

Source: `crates/smcli/src/cli.rs:895`.

- **What this is:** Legacy JSONL imports load the configured capture filters.
- **Problem:** The filter is applied only when `expandedCommand` exists. With `historyFilter: ["^secret"]`, an ordinary legacy record containing `secret data` was imported and stored.
- **Fix:** Apply `allows_run` to every legacy record, including those without an expansion. Test both forms against the same filter.
- **If we skip it:** Commands rejected by normal capture can enter persistent history through import.

Evidence: a temporary JSONL record supplied user, shell, directory, timestamp, exitCode and command, without expandedCommand. Import reported one record; direct SQLite inspection found `secret data` in commands. The finding concerns this inconsistent legacy-import branch; it is not a claim that every import format was tested against every filter.

### 4. Prune leaves excluded file selections behind

Source: `crates/smcli/src/cli.rs:694-699`.

- **What this is:** File selections store directory and filename information for ranking.
- **Problem:** Prune checks these rows only when their query is nonempty. After a blank-query selection, adding a matching cwdFilter and pruning reported zero removals and retained the selected filename.
- **Fix:** Apply the filter even when the query is empty. Test a directory exclusion with a blank selection query.
- **If we skip it:** Privacy cleanup leaves records of files selected in excluded directories.

Evidence: an isolated service accepted a version-3 select request with mode `files`, scope true, path `private.txt` and query `""`. A cwdFilter matching both the supplied and canonical directory was added. After prune, SQLite still contained the root/path row. This is a remaining blank-query case, distinct from the older report's already-fixed omission of file-selection cleanup entirely.

### 5. Fish history decoding changes executable commands

Source: `crates/smcli-search/src/native.rs:69-87`.

- **What this is:** Native Fish history is decoded for display and command selection.
- **Problem:** The decoder treats surrounding command quotes as storage syntax. Fish itself saved the command `"/tmp/a b"` verbatim; smcli returned `/tmp/a b`, changing one quoted executable path into separate words.
- **Fix:** Preserve shell quotes and decode only Fish history escapes. Replace the misleading quoted-history fixture with output generated by Fish.
- **If we skip it:** Selecting saved history can put a different command on the prompt.

Evidence: a temporary Fish session used `history append` and `history save`; its file contained `- cmd: "/tmp/a b"`. Reading that record through smcli removed the quotes. Native history was never modified by smcli.

### 6. Fallback completion changes valid filenames

Source: `crates/smcli/src/complete.rs:54`.

- **What this is:** In-process path completion runs when the service is unavailable or disabled.
- **Problem:** It replaces literal backslashes with `/`. A real Unix filename `back\slash.unique` appeared in completion results as `back/slash.unique`.
- **Fix:** Keep the relative filename unchanged; delete the backslash replacement. Add a filename containing a literal backslash to fallback coverage.
- **If we skip it:** Completion inserts a different path from the file the user selected.

Evidence: a temporary directory contained that filename and searches ran with SMCLI_SERVICE=0. The original file existed; the transformed path did not.

### 7. Relative database paths cannot start the service

Sources: `crates/smcli-core/src/history.rs:212-216`, `crates/smcli-core/src/service_client.rs:167-168`, `crates/smcli-service/src/main.rs:18`.

- **What this is:** SMCLI_DB selects the database used by the CLI and service.
- **Problem:** The CLI accepts relative paths and passes them unchanged, while the service requires an absolute path. `SMCLI_DB=relative.db smcli service start` failed in a temporary working directory.
- **Fix:** Resolve relative database paths consistently before service lookup and launch, including the socket identity used by clients. Test service startup with a relative override.
- **If we skip it:** An accepted database setting loses service functionality and repeatedly falls back to local work.

Evidence: the command exited 1 with `smcli-service not running for relative.db`. This is a path-contract mismatch, not a claim that fallback history storage itself fails.

### 9. Intent search no longer penalizes failed commands

Source: `crates/smcli-search/src/corpus.rs:573-604`.

- **What this is:** The merged intent scorer orders natural-language results by embedding similarity plus positive context boosts.
- **Problem:** It ignores success counts and drops failure explanations. With equal usage and timestamps, ten successful `cp b a` runs and ten failed `cp a b` runs, searching `copy files` ranked the successful variant first before this change; the merged scorer put the always-failed variant first without warning.
- **Fix:** Restore the existing success-rate penalty and failure explanation in intent scoring. Add this equal-similarity regression case.
- **If we skip it:** Repeatedly broken commands compete as though they worked.

Evidence: both binaries searched the same temporary SQLite database and index, with no native history or project candidates. Each command had ten runs in the same directory at the same timestamp. The static model gives these word-order variants the same embedding. Baseline ordering was successful then failed, with `always failed` on the latter; new ordering was failed then successful, with no failure explanation. This reproduction isolates loss of failure handling rather than a general preference for the old ranking weights.

## Should fix — test robustness and validation evidence

### 8. The normal test command can wait for human input

Sources: `crates/smcli/src/initialize.rs:159`, `:55-57`; `scripts/test.sh:7`.

- **What this is:** An initialization unit test calls the interactive command handler.
- **Problem:** With terminal stdin, `run(init(None), ...)` enters the key-binding questionnaire. The first audit's make test run waited there until interrupted.
- **Fix:** Keep this unit test out of the interactive branch and run the scripted Rust gate with stdin closed.
- **If we skip it:** Developers can get a stalled gate when running the documented test command in a terminal.

Evidence: the initial Kero terminal run stopped in `init_keeps_custom_model_and_stores_absolute_or_id`; restarting `make test </dev/null` completed. This is a test-robustness defect, separate from expected interactive behavior of the real init command.

### 10. The natural-language evaluation does not enforce quality

Source: `crates/smcli-search/tests/ranking.rs:457-461`, `natural_language_scores`.

- **What this is:** The new evaluation records wanted commands and target ranks.
- **Problem:** `rank_natural_language_eval` only prints results. The current gate passes while `start pi with sol` returns its wanted command fourth against a stated top-three goal, and this test has no assertion against further deterioration.
- **Fix:** Freeze and enforce an accepted natural-language quality floor. Keep acknowledged misses explicit rather than pretending every current top-three goal passes.
- **If we skip it:** Natural-language search can deteriorate while its named evaluation stays green.

Evidence: make ranking printed rank Some(4), goal top 3, and passed. The separate comparison tests do assert non-regression for their respective comparisons, but do not replace an absolute quality floor for the new intent behavior. This is a missing acceptance check, not a claim that every ranking test lacks assertions.

### 11. The results report validates a different configuration

Sources: `agents/reviews/semantic-search-accuracy-results.md:235-248`, `crates/smcli-search/src/corpus.rs:533`, `crates/smcli-search/README.md`.

- **What this is:** The results report records the selected config-description boost and its measured trade-offs.
- **Problem:** It selects 0.18, rejects 0.48 under its no-loss rule, and claims a combined positive cap of 0.28. Current main and the crate README deliberately use 0.48, giving a combined cap of 0.58.
- **Fix:** Update the report to identify the shipped commit, chosen value and accepted regressions. Do not attach the 0.18 validation claims to 0.48 behavior.
- **If we skip it:** Reviewers can approve the shipped behavior using evidence for another configuration.

Evidence: the constant is 0.48 at reviewed HEAD; the commit message and crate README agree. The results table labels 0.18 selected and 0.48 ineligible. This proves documentation/validation drift; it does not by itself prove that 0.48 is the wrong product decision. The private owner-history benchmark was not rerun.

## Validation and limits

- At the initial baseline, `make test </dev/null` passed: 215 Rust tests, three ignored, plus CLI, shell capture, service, file-picker, alias-picker and install/uninstall checks. Log: `/tmp/smcli-audit-test-20261102-noninteractive.log`.
- At merged commit 5a410b9, the equivalent semantic-accuracy worktree passed make build, make release, make test-rust PKG=smcli-core (59 unit + 7 protocol tests, one ignored), make test-rust PKG=smcli-search (66 unit + 1 model-reference + 4 ranking tests, one ignored), and make ranking. Logs: `/tmp/smcli-delta-build.log`, `/tmp/smcli-delta-release.log`, `/tmp/smcli-delta-core-tests.log`, `/tmp/smcli-delta-search-tests.log`, `/tmp/smcli-delta-ranking.log`.
- The current main source/build inputs were compared with that reviewed commit and matched. The full baseline suite was not claimed as a full-suite run of the merged commit. No redundant source/test gate was run solely to write this report.
- Reproductions used temporary homes, databases, configs, native-history fixtures and private services. The user's live history/service was not used by this audit. The pre-existing semantic-search documents and their benchmark claims are external evidence to this audit, not newly performed owner-history validation.
- Linux execution was unavailable because the Docker/OrbStack daemon was stopped. Dependency advisories, every source branch, upstream dependency internals in full, and release-performance measurements were not checked. Debug timing experiments are labeled as such and do not establish a release latency regression.
- Existing documented search trade-offs were not relabeled as defects merely because natural-language ranks changed. No speculative bloat estimate or dependency-removal claim is included.

Verdict: fix file clobbering, capture blocking and privacy-filter gaps first, then restore failure handling in the merged intent scorer. Passing tests do not cover these cases.

Not checked: Linux runtime, dependency advisories, full merged-commit shell/install gate, owner-history benchmark reproduction, release performance, or every source line. Findings 1–8 retain their earlier reproductions with unchanged relevant source paths.
