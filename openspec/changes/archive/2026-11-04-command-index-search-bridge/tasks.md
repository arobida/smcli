# Tasks

## 1. Spike

- [x] 1.1 Confirm the service can hold the history index and the command index open in one search request and measure added latency for 3 template queries; record results in design.md

## 2. Benchmark cohort

- [x] 2.1 Add a natural-language cohort (queries sharing no word with the expected command) to `crates/smcli-search/tests/ranking.rs` and verify `make ranking` prints its MRR/top-1/top-3 with today's baseline

## 3. Template-as-query recall

- [x] 3.1 Add a description lookup on `CommandIndex` returning top-k examples above a floor, with placeholders stripped from templates; verify with a unit test on a small fixture index
- [x] 3.2 Merge template-recalled history hits into the vector-first intent path with the damped max rule; verify `undo last commit` returns `git reset --soft HEAD~1` in a test and NL cohort improves in `make ranking`
- [x] 3.3 Skip recall silently when the command index is missing, building, locked or the CLI answers in-process; verify with tests for each case

## 4. Affinity bonus

- [x] 4.1 Add program/subcommand/flag affinity bonus in rerank for intent and literal queries, capped below an exact literal hit; verify the spec scenarios as tests and `make ranking` keeps original-cohort floors
- [x] 4.2 Tune k, floor and weights against `make ranking` and record chosen values in design.md

## 5. Docs and integration

- [x] 5.1 Document the bridge as a service-only signal in the architecture/search docs via iwe and run `make docs`
- [x] 5.2 Run `make test` and `make baseline`; verify both pass and latency stays within the baseline budget
