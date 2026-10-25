# Tasks

## 1. Benchmark

- [x] 1.1 Split `crates/smcli-search/tests/ranking.rs` cases into cohorts (original, adversarial, held-out) and report MRR/top-1/top-3 with denominators per cohort. Verify: `make ranking` prints per-cohort lines and the original cohort scores are unchanged.
- [x] 1.2 Add adversarial cases: flag variants, `HEAD~1`/`HEAD~10`, rare exact vs frequent near-match, quoted paths, >1,500-char shared-prefix tails, camelCase targets, intent paraphrases without command tokens, cold-start project scripts. Verify: cases run, and current failures are recorded in the PR description as the baseline.
- [x] 1.3 Freeze a held-out cohort (not used for tuning) and add correctness asserts for strict filters and forbidden results. Verify: `make ranking` fails when an assert is broken on purpose.

## 2. Long-command recall

- [x] 2.1 Check capture/input limits and decide on a lexical ceiling (record it in design.md). Verify: the decision is written down.
- [x] 2.2 Build lexical terms from the full command plus intent, keep the embedding cap at 1,500, bump the index `VERSION`. Verify: unit test retrieves each shared-prefix command by its tail. The long-tail cohort passes in `make ranking`.
- [x] 2.3 Measure sync time and index size on synthetic long entries before and after. Verify: numbers recorded in the PR.

## 3. Lexical precision

- [x] 3.1 If 1.2 shows flag/revision failures, add a bounded exact whole-token bonus for literal queries in `corpus.rs` (`coverage`/`rerank`), also exercised through `text_fallback`. Verify: exact-literal cohort passes, original cohort meets its floors, and a unit test covers text fallback.
- [x] 3.2 Emit camel/Pascal parts as extra terms in `tokenizer.rs`, using the same split in coverage, and bump `VERSION` if not already bumped. Verify: tokenizer unit tests keep the full token, and the identifier cohort passes, including `FooBar` beating `Foo`/`Bar`.
- [x] 3.3 Run `make test`. Verify: green, and native-history byte-equality checks pass.

## 4. Investigation: aliases

- [x] 4.1 Prototype capture in zsh, bash and fish. Measure hook cost, and write a safe-expansion filter in shell that rejects substitutions, assignments, operators and quotes. Verify: shell tests reject the secret-bearing alias example from the spec.
- [x] 4.2 Inject alias snapshots into ranking fixtures (`gst`→`git status`, `gl`→`git log`, conflicting sessions, removal, no snapshot) with query-side expansion. Verify: on/off ablation deltas written to `investigations/aliases.md`.
- [x] 4.3 Record a go/no-go with privacy review, latency and benchmark evidence in `investigations/aliases.md`. Verify: the file exists with a decision.

## 5. Investigation: PATH availability

- [x] 5.1 Prototype a query-time `shell_path` snapshot and a positive-only availability check (program found and executable; builtins, functions, wrappers and unknowns neutral). Verify: fixture cohort covering present, missing, builtin, wrapper, absolute path and unknown.
- [x] 5.2 Record a go/no-go in `investigations/path.md` with ablation deltas and the cost of sending PATH on each request vs a per-session snapshot. Verify: the file exists with a decision.

## 6. Investigation: file presence and fff

- [x] 6.1 Prototype in-process `stat` of path-like arguments for a small set of commands that read files, relative to the current cwd, bounded to the candidate pool, with no shell expansion. Verify: fixture cohort with two cwd trees, rename/delete, quoted paths, and `mkdir`/output guards that never demote.
- [x] 6.2 Inspect the pinned fff for a non-blocking exact-path lookup and measure warm vs cold lookup latency in the service. Verify: findings plus p50/p95 recorded.
- [x] 6.3 Compare fff evidence (git status, presence) against `stat` alone on the same fixtures. Verify: the delta is recorded.
- [x] 6.4 Record a go/no-go for `stat` and for fff separately in `investigations/files.md`. Verify: the file exists with both decisions.

## 7. Ship PATH and file-presence signals

Decisions: `investigations/path.md` (GO), `investigations/files.md` (stat GO, fff NO-GO).

- [x] 7.1 Add a versioned `shellPath` request field (sent by the CLI from its own environment, never stored), bump the protocol VERSION, and add frozen old/new fixtures. Verify: protocol fixture tests pass, and an old service makes the CLI fall back.
- [x] 7.2 Add the positive-only PATH boost (+0.1, weaker than every usage signal) in shared rerank code used by the service, the CLI fallback and text fallback, behind `SMCLI_RANKING_OFF=path`. Verify: PATH cohort in `make ranking` (present, missing, builtin, wrapper, absolute, unknown, and usage still dominates), plus an ablation-identical test.
- [x] 7.3 Add the stat file-presence boost for a fixed set of commands that read files, applied before truncation, skipped on network/remote mounts, behind `SMCLI_RANKING_OFF=files`. Do not use fff. Verify: files cohort passes, `mkdir`/output guards are never demoted, and `cargo tree -p smcli -i fff-search` stays empty.
- [x] 7.4 Add `why` evidence for both boosts with no full paths. Verify: unit test on `why` output.

## 8. Record-time alias expansion

Decision: approved by the user, with the expansion inserted on pick. Design in `investigations/aliases-record-time.md`.

- [x] 8.1 Apply `CaptureFilter` on the service `record` path as well as the CLI path (it is client-only today), failing closed. Verify: a service record test rejects a configured exclusion.
- [x] 8.2 Add history schema 8: a nullable `commands.expanded_command` column in a transactional migration with no backfill, and extend `NewRun`/`Run`, insert/read/export/prune. Verify: migration test from v7, existing rows stay NULL, and an event retry keeps one row with its original expansion.
- [x] 8.3 Capture in hooks. zsh: save preexec `$1` and `$3` and set the expansion only for a confirmed leading alias. bash: look up the first word (`BASH_ALIASES`, or `alias --` on 3.2) at preexec time, not at the next prompt. fish: only generated single-command alias functions; abbreviations are recorded as typed text. Apply the shell-side safe-expansion gate. Verify: `make test-shells` cases for each shell, including the secret, substitution, operator and quote rejections and the redefined-alias case.
- [x] 8.4 Add a versioned `expandedCommand` record field and an unambiguous hook-to-CLI flag, keep the existing record forms working, apply filters to both texts on both paths, and reject the whole run if either fails. Verify: frozen fixtures (absent, ordinary, unsafe, duplicate, conflicting, oversized) and a fallback test.
- [x] 8.5 Index both forms with the expansion as primary text and typed text as secondary, and key results by `(typed, expanded)` without merging usage counts. Bump the index VERSION and respect the 1,500/8,192 caps. Verify: alias cohort in `make ranking` reaches the investigation's 8/8 top-1 and other cohorts keep their floors. `SMCLI_RANKING_OFF=alias` falls back to typed-only.
- [x] 8.6 Show `typed → expansion` (sanitized) in the picker, insert the expansion on pick, and keep expansions out of `why`, logs and diagnostics. Verify: picker unit test and a black-box insert test.

## 9. Final verification

- [x] 9.1 Run `make test`, `make ranking` and `make test-shells`. Verify: all green, no cohort below its floor, and native history byte-identical.
- [x] 9.2 Run `make fresh` and give the user the build path and hook `eval` line. Verify: the build exists in `dist/`.
