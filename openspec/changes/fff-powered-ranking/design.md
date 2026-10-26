# Design

## Context

See proposal.md for the motivation. Current state:

- `crates/smcli-service/src/file_search.rs` keeps a pool of fff `FilePicker`s, one per project root (`index()`), with watchers on. It builds them with `FilePickerOptions::default()`, so `enable_content_indexing` is off, and grep uses `grep_options()` without `classify_definitions`.
- File presence (`crates/smcli-search/src/filesystem.rs`) `stat`s at most 4 paths for at most 40 candidates on one probe thread with an 8 ms wait. It runs in both the service and the CLI.
- Manifest scripts are already suggested by `smcli_core::ranking::candidates` (`CandidateKind::Script`).
- Rules from existing specs: unknown is neutral (no demotion), every signal has an ablation switch, service-only signals are allowed when documented, `make ranking` floors hold, and smcli-core and the CLI never link fff.

## Goals / Non-Goals

**Goals:**

- Feed fff evidence into ranking through one narrow interface that smcli-search can use without linking fff.
- Keep the in-process path unchanged; it only lacks the index signals.

**Non-Goals:**

- fff's LMDB frecency and query-tracker stores (smcli's SQLite owns frecency and picks).
- Embedding file contents with Potion.
- Building an index on demand for a cold root during search; only an existing ready instance is used.

## Decisions

1. **An evidence trait in smcli-search, implemented in the service.** smcli-search defines `ProjectEvidence` with `exists(paths)`, `git_activity(paths)`, `find_identifiers(names, budget)` and `grep_terms(terms, budget)`, all returning plain data. The service implements it over a read guard of the cwd's instance; the CLI passes `None`. This puts fff behind the boundary we already have. Alternative: a callback per signal, rejected as more plumbing for the same effect.
2. **Only use warm instances.** Search asks the pool for an instance without building one (a new `FileSearch::ready(root)`). Waiting for a 10 s scan inside suggest isn't acceptable. The first `files` call or a background warm-up on `cd` builds it.
3. **Index lookup replaces the probe only when present.** `filesystem.rs` keeps its command set and output-path rules; only the existence check is swapped, so both paths decide the same set and the boost size is shared.
4. **Git activity from fff's own data.** Per-file `git_status` plus the git-recency bonus (last 10 commits, default `GitRecencyConfig`). No `git` subprocess.
5. **One `multi_grep` per request for evidence.** Identifiers come from candidate arguments that are plain identifiers (not paths, flags or numbers), deduplicated, capped at about 64, with a 20 ms `time_budget_ms`. `classify_definitions` gives the definition bonus. Commands not checked within the budget stay neutral.
6. **Test-name candidates through grep, not a parser.** Per-language definition patterns (Rust `#[test]`/`fn`, Python `def test_`, JS `it(`/`test(`), validated with the same `valid` identifier rule `candidates()` uses, emitted as a new `CandidateKind::Test`. Cached per root and invalidated by the watcher's change signal.
7. **`@` completion as a new service operation.** `Operation::Complete { word, command }` returns paths. `fuzzy_search_directories` for directory commands, `glob` when the word has glob characters, `fuzzy_search_mixed` otherwise, re-ranked with the existing `picked` scores. Choosing a path records it through the existing `select` path. The in-process fallback walks the cwd with a fixed depth and file limit. `@` was chosen over `+`, which already starts words (`cargo +nightly`, `chmod +x`). `*` is ruled out because it is a live shell glob.
8. **Expansion as a second pass.** It runs when the top history hit is below a fixed score threshold, picked from the ranking fixtures. One `grep_terms` call yields file stems and definition names, which are added as low-weight BM25 terms. The second pass's results are merged below strong direct hits.
9. **Content indexing on.** `enable_content_indexing: true` for every instance. Memory is about 360 B per indexed file, and the pool is already capped.

## Risks / Trade-offs

- [Service and CLI rank differently] → Signals are documented as service-only, each can be switched off, and ranking tests run both ways with the signals disabled.
- [Evidence latency on huge repos] → Hard time budget, warm instances only, abort signal on client disconnect.
- [Common identifiers boost everything] → Skip short tokens (< 4 chars) and a stop list (`main`, `test`, `build`, …), and boost by definition more than by any occurrence.
- [`@` collides with real arguments] → Completion only acts when invoked; typed text is never rewritten.
- [Memory growth from the content index] → Pool capacity and the idle eviction are unchanged. `smcli indexes` reports per-instance memory.

## Migration Plan

Each signal ships behind its switch and lands with its fixture cases. Roll back by setting `SMCLI_RANKING_OFF`, or by reverting the one commit per signal. Adding the `Complete` operation needs new frozen protocol fixtures. Older services answer with a version mismatch, and the CLI falls back.

## Open Questions

- The exact weak-hit threshold for expansion, tuned against `make ranking`.
- Whether to warm an instance in the background on `cd` into a new project or only on first `files`, decided by measuring memory with `make baseline`.
