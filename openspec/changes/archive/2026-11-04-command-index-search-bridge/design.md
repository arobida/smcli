# Design

## Context

Production intent search takes the vector-first path (`vector_first()` is true outside tests): nearest neighbour over history embeddings of the typo-corrected query, then `intent_search` rerank. Literal queries use BM25 0.7 + dense 0.3. The command index (`CommandIndex`, TalaDB, one row per example with an embedded description and a template) is opened only for `@@`. TalaDB allows one process per index, so only the service can use both indexes during a search.

## Goals / Non-Goals

**Goals:**

- Recall history rows for English queries via command-index templates (B).
- Rerank history by program/subcommand/flag affinity with matched templates (C).

**Non-Goals:**

- Returning templates as results or changing suggestions.
- Query token expansion (rejected: placeholder and common-word drift).
- Any in-process fallback for the bridge.

## Decisions

- **Template-as-query over token expansion.** Templates are commands, the same text kind history vectors were built from, so command-to-command nearest neighbour is more reliable than English-to-command. Token expansion was rejected for drift risk.
- **Placeholders stripped** (`{{path}}` and cheat equivalents) before embedding templates and before parsing flags, using the existing `command_flags` parser.
- **Merge rule.** A history row recalled through template *i* scores `sim(query, description_i) × sim(template_i, row)`; it is merged with the direct hit by max, then reranked normally. Damping by description similarity keeps weak bridges below strong direct hits.
- **Affinity as a rerank term, not retrieval.** Computed from `IndexedCommand` program/subcommand/flags already stored; capped on literal queries below the exact-hit margin.
- **Service-only signal.** Declared as such under history-search-ranking's "Same results with or without the service"; the in-process path skips it.
- **Tuning constants** (k, similarity floor, bonus weights) set by `make ranking` against the new natural-language cohort, not fixed in specs.

## Risks / Trade-offs

- [Wrong tldr match pulls in unrelated rows] → similarity floor, damping by description similarity, NL cohort plus original-cohort floors in `make ranking`.
- [Latency: k extra nearest-neighbour queries per intent search] → small k (start at 3), batch-embed templates; measure with `make baseline`.
- [Opening two TalaDB indexes in one request] → spike first; command index opened read-only and cached by the service.
- [Service vs in-process ordering differs] → documented service-only signal.

### Spike measurements

Both TalaDB indexes opened successfully while holding the history-directory lock. The original six-row fixture measured 0.234 ms median / 0.260 ms p95 for description lookup, batch embedding, and three history nearest-neighbour queries. A 5,000-distinct-command synthetic fixture (never real history), with 107 bundled descriptions across 14 program pages, measured 1.645 ms median / 1.774 ms p95 across 20 warm samples after one warmup, in the debug test profile. This excludes index construction and request overhead; final release baseline verification remains required. For `undo last commit`, the bundled `git reset --soft HEAD~2` match scored 0.7245 and recalled `git reset --soft HEAD~1` at 0.9328: damped score 0.6758, above the incorrect direct leader at 0.4357. The damped-max rule therefore works for this probe without changing it.

## Open Questions

- Starting values for k, floor and bonus weights (settled by benchmark).

### Recall tuning sweep

Measured with full `make ranking`; triples are MRR / top-1 / top-3. All runs use k=3 and description-similarity × history-similarity, max-merged with direct scores.

| Floor  | Existing NL held-out | Bridge NL           | Ranking                    |
| ------ | -------------------- | ------------------- | -------------------------- |
| Before | 0.815 / 0.78 / 0.89  | 0.732 / 0.67 / 0.67 | Pass                       |
| 0.45   | 0.806 / 0.78 / 0.78  | 0.857 / 0.83 / 0.83 | Fail: clean release rank 4 |
| 0.50   | 0.806 / 0.78 / 0.78  | 0.857 / 0.83 / 0.83 | Fail: clean release rank 4 |
| 0.55   | 0.806 / 0.78 / 0.78  | 0.857 / 0.83 / 0.83 | Fail: clean release rank 4 |
| 0.60   | 0.815 / 0.78 / 0.89  | 0.857 / 0.83 / 0.83 | Pass                       |
| 0.65   | 0.815 / 0.78 / 0.89  | 0.857 / 0.83 / 0.83 | Pass                       |
| 0.70   | 0.815 / 0.78 / 0.89  | 0.857 / 0.83 / 0.83 | Pass                       |

Chosen recall settings: k=3, floor=0.60, no direct-score gate, unchanged damped-max merge (weight 1). Smaller k and weak-direct gating were unnecessary because the floor sweep passed. Original search stayed 0.922 / 0.88 / 0.94; adversarial search stayed 0.894 / 0.85 / 0.90; all perfect cohorts stayed perfect. Existing NL reproduction stayed 0.875 / 0.80 / 0.90 and tuning stayed 0.833 / 0.80 / 0.90. The existing 0.954/0.909 literal floor gate is untouched. Affinity weights and final integration verification remain pending. Logs: `/tmp/bridge-floor-<floor>.log`.

Final affinity verification: program match +0.01, matching nonempty subcommand +0.02, shared template-flag fraction ×0.02 (maximum +0.05, best matching template only). The term is added during reranking, after existing signals. Each candidate is capped below the nearest exact literal hit that previously ranked at least as high, including ties; no changes to existing literal floor gates. Metadata is read from indexed history candidates only, so project additions cannot acquire this bonus. Full `make ranking` passes with these weights: adversarial search improves to 0.906 / 0.85 / 0.95; every other cohort retains the recall-only values above. No merge-rule or specification deviation. The lookup fixture is an integration test using a real small TalaDB index rather than a module-local unit test; it covers missing/building/foreign-process-locked indexes, stripped placeholders, template exclusion and the unchanged no-index API. Service dispatch and affinity metadata/capping have focused tests.

#### Final verification

`make test` passed: 336 Rust tests, 9 skipped by the suite configuration, plus every black-box CLI, shell, picker, service, install and tiny-baseline check. `make ranking` passed all five tests; final metrics are listed above. `make fresh` produced the clean release pair. No dependency manifests or protocol fixtures changed; smcli/smcli-core gain no taladb, rusqlite or fff dependency. `git diff --check` passed.

Release `make baseline BENCH_ARGS="--service off --output /tmp/smcli-bridge-baseline-after.json"` passed, compared against `/tmp/smcli-bridge-baseline-before.json` (also service-off, same 1k/10k/100k rows and 20 samples). All 18 operation and 18 hook warm medians are within the established noise budget: increase ≤ max(10% of before median, before p95 minus before median). Search warm medians in ms:

| Rows | Literal before → after | Intent before → after | Intent delta      |
| ---- | ---------------------- | --------------------- | ----------------- |
| 1k   | 12.766 → 13.057        | 12.432 → 12.786       | +0.354 ms (+2.8%) |
| 10k  | 45.429 → 45.372        | 46.357 → 46.087       | −0.270 ms (−0.6%) |
| 100k | 299.200 → 302.022      | 303.287 → 307.772     | +4.485 ms (+1.5%) |

The baseline verifies unchanged in-process performance, not active service-only bridge overhead. A separate release run (`make test-rust PKG=smcli-search FILTER="--release bridge_dual_index_realistic_history --no-capture"`) measured 1.571 ms median / 1.726 ms p95 for the original 5,000-command/107-description dual-index spike, including all three template queries without the selected floor. This is a retrieval-leg measurement, not end-to-end service latency; there is no service-on before baseline for comparison. Linux was not run. No design decision/specification deviations; the small lookup fixture lives in integration tests rather than a module-local unit test. No commits or pushes.
