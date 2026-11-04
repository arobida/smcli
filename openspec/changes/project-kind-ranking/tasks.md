# Tasks

## 1. Baseline before the change

- [x] 1.1 Run `make ranking` and `make baseline` on the unchanged tree and save both reports under `dist/` for comparison; verify both complete and the ranking floors pass.

## 2. Benchmark cases first

- [x] 2.1 Add the `kind` group to `crates/smcli-search/tests/ranking.rs` (fixture projects of two kinds, fresh-project habit cases, own-project-wins, outside-project) in the Adversarial cohort; verify `make ranking` runs and reports the new group (expected to miss before the tier exists).

## 3. Kind lookup and switch

- [x] 3.1 Add `Signal::Kind` (`kind` in `SMCLI_RANKING_OFF`) and `project_kind` on `RankingContext`; verify a unit test that `RankingContext::new` reads the cwd's primary kind and that `kind` parses as a switch.
- [x] 3.2 Add a kind-of-directory lookup (a cheap probe equal to `discover`'s first project kind, starting from the directory's real path) with a per-call memo in the in-process scan and a 60 s-ttl `KindCache` in the service index; verify unit tests for nested directories, monorepo roots, a missing directory (unknown), a directory with no manifest (unknown) and a symlinked directory.

## 4. Scoring

- [x] 4.1 Apply the same-kind multiplier in in-process suggest (`history.rs`); verify a unit test where a same-kind habit beats an other-kind distractor and an in-project command still wins.
- [x] 4.2 Apply the same multiplier in service suggest (`suggestion_index.rs`) with kinds cached when the index is built; verify a test that service and in-process suggest give the same order for the same fixture.
- [x] 4.3 Add `runs_kind` to the search corpus and its boost in `corpus.rs`, below the project boost; verify `cargo test -p smcli-search` and that the existing corpus tests are unchanged.
- [x] 4.4 Add "used in a <kind> project" to `why` output; verify with a `why` unit test, and that `SMCLI_RANKING_OFF=kind` removes both the boost and the `why` line.

## 5. Prove it

- [x] 5.1 Run `make ranking` with the tier on and with `SMCLI_RANKING_OFF=kind`; verify the `kind` group MRR is higher with the tier on and every cohort is at or above its floor. Tune weights only within the spec's tier order.
- [x] 5.2 Run `make baseline` and compare with 1.1; verify warm suggest and hybrid search medians are within noise, and record the first-request difference in `agents/reference/baseline.md` (run `make docs`).
- [x] 5.3 Run `make test`; verify it passes.

## Workflow follow-up

- If 5.1 or 5.2 fails, stop and report the numbers instead of merging.
- Archive the change after review.
