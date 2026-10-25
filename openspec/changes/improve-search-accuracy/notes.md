This file records implementation measurements for the improve-search-accuracy change.

# Baseline ranking scores (task 1.2)

Scores captured after adding cohorts, adversarial cases and held-out assertions, before any ranking fixes.

| Cohort      | Kind    | MRR   | top-1 | top-3 | n   |
| ----------- | ------- | ----- | ----- | ----- | --- |
| original    | suggest | 1.000 | 1.00  | 1.00  | 18  |
| original    | search  | 1.000 | 1.00  | 1.00  | 16  |
| adversarial | suggest | 1.000 | 1.00  | 1.00  | 1   |
| adversarial | search  | 0.593 | 0.44  | 0.78  | 9   |
| held-out    | suggest | 1.000 | 1.00  | 1.00  | 1   |
| held-out    | search  | 1.000 | 1.00  | 1.00  | 2   |

Adversarial failures used as the baseline:

- `flag-variant`: passing (rare `--soft` already ranks first).
- `revision`: passing (`HEAD~1` ranks first).
- `rare-exact`: failing — `git checkout main-branch` is rank 2 behind the 25× `git checkout main`.
- `quoted-path`: passing.
- `long-tail`: failing — commands whose only distinguishing term is after the 1,500-character embedding cap are not retrieved.
- `camelcase`: failing — `bar tests` does not reach `FooBarTests`; `FooBar` reaches `run FooBar` but not robustly above part matches.
- `intent`: failing — paraphrase without command tokens ranks the target at rank 3.
- `cold-start`: passing.

# Lexical ceiling (task 2.1)

Captured commands are stored as SQLite `TEXT`, which has no practical length limit. To keep the keyword index bounded while still indexing typical pasted scripts and heredocs, lexical terms are capped at **8,192 characters**. The embedding text stays capped at 1,500 characters. The identity (`doc`) and the history database still keep the full command verbatim.

# Sync time and index size (task 2.3)

Measured by indexing five synthetic commands of 8,000 unique characters each.

| Version                                   | Sync time | Index size    |
| ----------------------------------------- | --------- | ------------- |
| v1 (terms from 1,500-char embedding text) | 72 ms     | 565,248 bytes |
| v2 (terms from 8,192-char lexical text)   | 90 ms     | 626,688 bytes |

Delta: +18 ms sync, +61,440 bytes (~11% larger) for the five long entries. The shared-prefix long-tail cohort in `make ranking` moves from MRR 0.000 to MRR 1.000 after the change.

# Review and lexical precision (group 3)

Reviewed the inherited diff before completing group 3. Full-command identity, bounded lexical text, capped embedding text and the v2 rebuild are retained. The unfinished exact bonus alone did not fix rare-exact: `main-branch` was classified as intent. Single hyphenated tokens now use literal ranking; exact matching parses whole shell tokens without expansion, retaining a bounded +1.0 literal bonus (+0.2 for intent). Coverage and indexing share the tokenizer's camel/Pascal splitting, including acronym boundaries, while retaining full identifiers. v2 already covers both indexing changes in this uncommitted change.

The version bump also required updating the service cache-cleanup fixture to use a v2 index for the other model. The inherited timing-only unit test is now ignored by default. Its five commands are cyclic 8,000-character strings, not 8,000 unique lexical terms; the task 2.3 numbers above are inherited measurements, not a representative large-vocabulary benchmark or independently reproduced before/after timing.

Verification: `make test-rust PKG=smcli-search`, `make ranking`, and `make test` pass. Added text-fallback cases for rare-exact, flag/revision distinctions and identifier parts. The benchmark now asserts top-1 for the repaired adversarial cases (except intent), rather than merely printing failures. Added explicit `cmp` checks for the temporary Bash and Zsh native history files to the full gate; both pass. Held-out cases were not changed or used for tuning.

# Final per-cohort scores (group 3)

| Cohort      | Kind    | MRR   | top-1 | top-3 | n   |
| ----------- | ------- | ----- | ----- | ----- | --- |
| original    | suggest | 1.000 | 1.00  | 1.00  | 18  |
| original    | search  | 1.000 | 1.00  | 1.00  | 16  |
| adversarial | suggest | 1.000 | 1.00  | 1.00  | 1   |
| adversarial | search  | 0.926 | 0.89  | 1.00  | 9   |
| held-out    | suggest | 1.000 | 1.00  | 1.00  | 1   |
| held-out    | search  | 1.000 | 1.00  | 1.00  | 2   |

All exact-literal cases and both identifier cases are top-1; both long-tail cases remain top-1. The only remaining miss is intent (MRR 0.333, top-1 0.00, top-3 1.00, n=1).

# Intent case: defer

`undo the last commit but keep changes` still ranks `git reset --soft HEAD~1` third, behind `git commit --amend` and `git commit -m wip`. The target is retrieved, so this is not a lexical recall failure. The query contains none of the target's command tokens; exact-token or identifier bonuses cannot supply the missing semantics. A phrase-to-command rule would tune to this one benchmark example, and globally reducing usage evidence could disturb valid habits. No cheap general fix was established; leave semantic model/intent improvements for separate measured work rather than adding a special case. No shell-context signals or group 7 work were started.

# Group 7: PATH and file-presence signals

Implemented cleanly from the approved investigation notes, without spike branch code. No group 8 work or commits.

## Wiring and limits

- Protocol version 2 adds optional `shellPath` and `rankingOff` request fields. The shared client takes PATH from its own inherited environment for each search, capped at 8,192 bytes (oversized or NUL-containing values become unknown). `rankingOff` carries the caller's bounded ablation setting, including an empty value to clear a warm service's startup setting. Old v1 fixtures remain frozen; new v2 fixtures cover present, absent and empty snapshots. A fake v1 service confirms the CLI searches in-process rather than silently losing signals.
- PATH/cwd evidence stays in request-scoped `SearchSignals`, never database rows, index documents, embeddings or hook snapshots. The shared reranker is used by service, CLI fallback and locked-index text fallback. No hook change is needed: the CLI inherits the live shell's PATH.
- PATH adds at most `0.1 * relevance` to the score. Each distinct program is resolved once per query; first executable wins. Executability uses this user's `access(X_OK)`. Builtins, path-qualified programs, shebang wrappers and unknowns get nothing. Snapshots with relative/empty entries or more than 64 directories are treated as unknown, not silently reinterpreted. Missing directories may be skipped; an uncertain/remote lookup stops resolution rather than looking past an unknown shadowing program.
- Files add at most `0.08 * relevance`, with the investigation's fixed reader set and no shell expansion. Pipes, redirects and substitutions are rejected by `shell_words`; variable/glob/tilde arguments are ignored. Only path-like arguments are checked. Two cwd trees, quoted paths, head options, rename/delete, output guards, usage dominance and absent/disabled identity are covered.
- Both signals inspect at most 40 candidates, before final truncation. Files inspect at most four eligible arguments per command; commands over 32,768 bytes get no filesystem evidence. macOS requires `MNT_LOCAL`; Linux uses a conservative local-filesystem allowlist. Unknown or remote mounts do not receive evidence.
- Even `statfs` can hang on a dead mount, so filesystem work runs off the search thread with an 8 ms wait budget and at most one outstanding worker per process. Timeout/contention discards the entire observation; later queries remain neutral while a stalled worker is outstanding. No partial-result ranking, unbounded worker growth, persistent cache, fff lookup or cold scan is introduced.
- `why` contains only `program found in PATH` / `input file exists`, not paths. `SMCLI_RANKING_OFF=path,files` disables both; each switch is also checked independently against a warm service and the CLI fallback.

## Verification

Final `make test` and `make ranking` are green. The full gate includes shell tests, native Bash/Zsh history byte-equality, protocol fixtures, real-service integration, installation and picker tests. Locked-index tests verify both signals at limit 1, proving the boosts run before truncation. Signal-off results (including `why`) are asserted identical to results with the inputs omitted. Service/fallback ordering and ablation evidence are checked end-to-end with a client-only executable directory.

`cargo tree -p smcli -i fff-search` and the smcli-core equivalent report no matching package in those dependency graphs; the service-only query shows fff-search linked only by smcli-service.

The repeated full gates exposed an existing macOS oversized-message test race (`ENOTCONN` instead of `EPIPE` after early rejection). The helper now accepts only expected disconnect errors on oversized payloads; normal-size send errors still fail. One subsequent run had a transient initial service-launch/no-socket failure; an unchanged rerun passed all gates. No retry was added to hide that failure.

## Final cohort scores

| Cohort      | Kind    | MRR   | top-1 | top-3 | n   |
| ----------- | ------- | ----- | ----- | ----- | --- |
| original    | suggest | 1.000 | 1.00  | 1.00  | 18  |
| original    | search  | 1.000 | 1.00  | 1.00  | 16  |
| adversarial | suggest | 1.000 | 1.00  | 1.00  | 1   |
| adversarial | search  | 0.926 | 0.89  | 1.00  | 9   |
| held-out    | suggest | 1.000 | 1.00  | 1.00  | 1   |
| held-out    | search  | 1.000 | 1.00  | 1.00  | 2   |
| PATH on     | search  | 1.000 | 1.00  | 1.00  | 7   |
| PATH off    | search  | 0.929 | 0.86  | 1.00  | 7   |
| files on    | search  | 1.000 | 1.00  | 1.00  | 9   |
| files off   | search  | 0.833 | 0.67  | 1.00  | 9   |

PATH/files cohorts use controlled equal-relevance rerank hits to isolate near-tie behavior from embedding noise; the original/adversarial/held-out cohorts still exercise indexed retrieval. Additional assertions cover cwd changes, rename/delete and the locked-index path outside those denominators. The only existing adversarial miss remains intent, at rank 3.

## Limits not tested on hardware

No Docker/Linux gate, live remote/dead network mount, real-history evaluation or fresh release build was run. Locality rejection and bounded-worker contention have regression coverage, but real remote-mount behavior was not exercised. A hung worker intentionally disables these optional boosts for that process until it returns. A PATH-only snapshot cannot identify an unexported shell function shadowing an installed binary; missing function names are neutral, but such shadowing remains indistinguishable from an available executable (as are compiled wrapper shims). No function-table capture was added.

# Group 8 core implementation (shell handoff pending)

Tasks 8.1, 8.2, 8.4, 8.5 and 8.6 have their focused verification passing. Task 8.3 is owned by alias-hooks, which exclusively edits `shell_integration.rs` and `test-shells.py`; final full/shell gates await the user's handoff notice. Protocol v2 remains unreleased and is reused, not bumped again. Group 9 has not been started.

## Storage, privacy and protocol

- Service record now loads the caller's config fail-closed and applies `CaptureFilter` to both known texts before append or incremental updates. CLI local/service paths share `allows_run`; a filtered expansion rejects the whole run. Acknowledging a filtered record does not trigger an unsafe local retry.
- Schema 8 adds nullable `expanded_command` without backfill. `NewRun`, `Run`, SQL insert/read, JSON export/import and prune preserve it. Event retries retain the original expansion and one row. Unsafe/empty/oversized expansion fields fail with a body-free error; hooks omit unsupported expansions. The receiver independently enforces the conservative single-space ASCII-word vocabulary, rejecting assignments, operators, quoting, substitutions, variables and leading tildes.
- CLI accepts `record --expanded TEXT` before the unchanged 5/8/9 positional forms. `expandedCommand` is a distinct v2 request/result field. Frozen fixtures cover typed-only, ordinary, unsafe, duplicate/conflicting records and alias results. Scripted checks exercise oversized/empty fields and filters on both record paths.
- Aggregates, transitions, picks, corpus entries and derived index rows use a collision-free `(typed, expanded)` identity. Ordinary string keys stay unchanged; variant keys are internal and never displayed. `recentEntries` carries explicit typed/expanded rows across the wire, while the old `recent` field remains typed-only. Direct and aliased runs do not merge counts. Native-only commands stay unexpanded.

## 8.2 review: fallback performance and backup

The temporary all-runs delegation was removed. Suggestions again use prefix-filtered SQL, now matching the typed surface and (when enabled) the expansion, with per-variant tallies. The suggestion-index parity test again compares independent SQL and in-memory implementations and now includes conflicting aliases.

Measured with a 100,000-run synthetic DB (100 prefix matches), 10 debug samples on this Mac, using `make test-rust PKG=smcli-core FILTER='measure_suggestion_fallback_100k -- --ignored --nocapture'`:

| Method                          | p50        | p95        |
| ------------------------------- | ---------- | ---------- |
| Prefix SQL                      | 30.921 ms  | 31.536 ms  |
| Read all runs + construct index | 683.923 ms | 696.070 ms |

The prefix SQL path is retained; no per-suggest all-history index rebuild ships.

Before migrating an existing nonempty pre-v8 database, smcli creates `history.db.v7.bak` once at mode 0600. It uses SQLite `VACUUM INTO`, not a raw copy of the main file, so committed WAL contents are included. A private migration lock coordinates concurrent openers. Existing backups are not overwritten. The backup is taken before the migration transaction; **all ALTERs, including v8, remain inside the same BEGIN IMMEDIATE/COMMIT transaction**. Tests verify a live-WAL v7 backup, mode 0600, old rows staying NULL, unchanged backup on reopen, original retry metadata, and rollback of earlier ALTERs when v8 fails.

Rollback: stop smcli-service and pause capture in all shells first. Preserve/export the v8 database before replacing anything. Move the v8 database and its WAL/SHM sidecars aside together, copy the private `.v7.bak` to `history.db`, chmod it 0600, then use the v7 binary. Do not restore over a live connection or leave v8 sidecars beside the restored file. Restoring the backup loses runs captured after that snapshot; the v7 binary cannot read schema 8. The automatic backup test is synthetic; no user history was migrated.

## Search and picker

Index format v3 keys documents by variant, uses expansion-primary program/subcommand/flag and failure semantics, indexes both lexical surfaces, and embeds primary executable text plus authored intent (typed aliases are not semantic noise). The 1,500/8,192 caps are retained. Equal executable primary text shares retrieval relevance so an extra alias spelling does not impose a BM25 length penalty; usage remains separate. `SMCLI_RANKING_OFF=alias` switches to a separate typed-only index representation, while retaining expansion provenance and default insertion.

Picker labels are sanitized `typed → expansion`; filtering uses text surfaces, not display markup or encoded identities. Selecting emits the captured expansion verbatim, never the arrow label and never execution. Unit and PTY tests cover control escaping, conflicting `gl` variants, service/fallback insertion and a `touch` expansion whose marker must remain absent. Export/import/prune tests preserve and remove the correct variant and its selections.

## Focused scores and verification

`make test-rust`, `make ranking`, `make test-service`, and the isolated alias-picker PTY test pass. Alias cohort on: MRR **1.000**, top-1 **1.00**, top-3 **1.00**, n=8. Typed-only off: MRR **0.641**, top-1 **0.50**, top-3 **0.62**, n=8. The fixture includes competing direct spelling and historical redefinition; additional assertions check strict expansion filters, native unknowns, separate counts, authored intent/caps, incremental rebuild parity and locked-index parity. All existing cohort totals are unchanged from group 7. Final full and shell gate results will be recorded after the hook handoff.

# Group 8 post-handoff gates

After the user confirmed 8.3 landed, ran the requested gates in order:

| Gate               | Result                                                   |
| ------------------ | -------------------------------------------------------- |
| `make build`       | PASS                                                     |
| `make test`        | FAIL: `test-install: .local/share/smcli was not removed` |
| `make ranking`     | PASS (2 tests)                                           |
| `make test-shells` | PASS                                                     |

No hook/`--expanded` integration breakage was found, so no integration code was changed. Alias capture passed for Bash 3.2 DEBUG, Bash 5.3 PS0, DEBUG fallback and bash-preexec, zsh, and fish. The full gate also passed Rust, shell, service, file-picker and alias-picker checks before failing in uninstall verification. The later full-gate native-history `cmp` checks were not reached on this run. The uninstall failure was left unchanged because the user restricted fixes to hook/CLI integration. The queued group-7 review was dropped and reassigned by the user; group 9 remains untouched.

Final ranking totals from this gate:

| Cohort      | Kind    | MRR   | top-1 | top-3 | n   |
| ----------- | ------- | ----- | ----- | ----- | --- |
| alias on    | search  | 1.000 | 1.00  | 1.00  | 8   |
| alias off   | search  | 0.641 | 0.50  | 0.62  | 8   |
| original    | suggest | 1.000 | 1.00  | 1.00  | 18  |
| original    | search  | 1.000 | 1.00  | 1.00  | 16  |
| adversarial | suggest | 1.000 | 1.00  | 1.00  | 1   |
| adversarial | search  | 0.926 | 0.89  | 1.00  | 9   |
| held-out    | suggest | 1.000 | 1.00  | 1.00  | 1   |
| held-out    | search  | 1.000 | 1.00  | 1.00  | 2   |
| PATH on     | search  | 1.000 | 1.00  | 1.00  | 7   |
| PATH off    | search  | 0.929 | 0.86  | 1.00  | 7   |
| files on    | search  | 1.000 | 1.00  | 1.00  | 9   |
| files off   | search  | 0.833 | 0.67  | 1.00  | 9   |

Logs: `/tmp/group8-build.log`, `/tmp/group8-test.log`, `/tmp/group8-ranking.log`, `/tmp/group8-shells.log`.
