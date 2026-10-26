# Tasks

## 1. Index configuration

- [x] 1.1 Build fff instances with `enable_content_indexing: true` in `FileSearch::index` and verify `make test-service` passes and `smcli grep` results are unchanged on the test fixtures
- [x] 1.2 Set `classify_definitions: true` in `grep_options`, expose `is_definition` on grep hits (protocol field + frozen fixture), and verify the protocol fixture test passes

## 2. Evidence interface and index-backed file presence

- [x] 2.1 Add the `ProjectEvidence` trait in smcli-search and an optional evidence argument through search and suggest; verify `make test-rust` passes with `None` everywhere
- [x] 2.2 Add `FileSearch::ready(root)` returning a warm instance without building one; unit test that a cold root returns `None`
- [x] 2.3 Implement `exists` over the fff index in the service and use it in `filesystem.rs` when present; add `index` to `SMCLI_RANKING_OFF`; verify with a test that 200 file-naming candidates all get the boost, and that the ranking matches the probe on the existing cases
- [x] 2.4 Document the `index` switch in the CLI docs and run `make docs`

## 3. Git activity boost

- [x] 3.1 Implement `git_activity` from fff git status and git recency, add a `Signal::Git` boost weaker than usage, and add the "file being edited" and "usage still dominates" cases to `ranking.rs`; verify `make ranking` floors hold
- [x] 3.2 Document the `git` switch and run `make docs`

## 4. Project evidence boost

- [x] 4.1 Add identifier extraction from candidate arguments (stop list, length ≥ 4, cap 64) with unit tests for paths, flags and numbers being excluded
- [x] 4.2 Implement `find_identifiers` with one `multi_grep` and a 20 ms budget; boost occurrences, more for definitions; verify the "test still defined" and "budget exceeded" scenarios in tests and `make ranking`
- [x] 4.3 Document the `evidence` switch and run `make docs`

## 5. Test definition suggestions

- [x] 5.1 Add `CandidateKind::Test` and per-language test-definition patterns, validated by the `candidates()` identifier rule, cached per root and invalidated on watcher change; verify the Rust and hostile-name scenarios with tests
- [x] 5.2 Show the project-test label in the picker and suggest output; document the `tests` switch and run `make docs`

## 6. `@` path completion

- [x] 6.1 Add the `Complete` service operation with frozen protocol fixtures, using directory, glob and mixed fuzzy search re-ranked by picks; verify with service tests for each mode
- [x] 6.2 Add the in-process fallback (bounded cwd walk) and verify `@` completion works with the service stopped
- [x] 6.3 Wire `@` completion into the zsh, bash and fish hooks in `shell_integration.rs`, recording picks via `select`; verify with `make test-shells`, including that `npm i @types/node` runs untouched
- [x] 6.4 Document `@` completion in the CLI docs and run `make docs`

## 7. Query expansion

- [x] 7.1 Implement `grep_terms` and the weak-hit second pass with expansion terms weighted low; add the "concept named in code only" and "strong direct hit" cases; tune the threshold until `make ranking` floors hold
- [x] 7.2 Document the `expand` switch and run `make docs`

## 8. Integration

- [ ] 8.1 Run `make test`, `make ranking` and `make baseline` and compare suggest latency and service RSS against main; record results in the change
