# Accurate natural-language history search

- **Author:** Andrew Robida
- **Status:** Draft
- **Date:** 2026-11-01

------------------------------------------------------------------------

## Overview

Users should be able to describe a task and find the command they need without remembering its spelling or flags. Today, natural-language history search often favors a shared word over the intended command, even when project targets and local usage support that command.

This work improves retrieval and ranking for both Tab search in the history picker and `smcli search`. It starts with query interpretation and candidate coverage, then changes how project evidence combines with relevance. Optional Tab re-scoring with in-memory embeddings of context text remains Stage 3, only if Stages 1–2 miss the targets. The existing model stays in place, context raises rank without filtering results, and shared improvements must work without the service.

## The Problem

The installed build was tested on 2026-11-01 using `smcli search` from the smcli repository. These nine queries show both failures and behavior to preserve:

| Query                         | Wanted                                         | Observed result                                                                                |
|:------------------------------|:-----------------------------------------------|:-----------------------------------------------------------------------------------------------|
| build and insall locally      | `make install`                                 | Absent from top 8; all results contain literal “build”.                                        |
| build and install locally     | `make install`                                 | Rank 3, below one-run `ll swift-pass.build/` despite “used here 4x, Makefile target” evidence. |
| install smcli on this machine | `make install`                                 | Absent; long pasted `SMCLI_NEW=...` scripts win.                                               |
| run the tests                 | `make test`                                    | Absent; `deno run` and `bun run` win on “run”.                                                 |
| format the docs               | `make docs`                                    | Absent; `docs`, `la docs`, and `mv docs` win on “docs”.                                        |
| rebuild a clean release       | `make fresh`                                   | Absent; stale `swift build -c release` comes first.                                            |
| open pi with gpt 6            | `pi --model openai/gpt-6.1-sol --thinking low` | Absent from top 8; `oc` and `codex` commands with `gpt-6.1-sol` win.                           |
| start pi with sol             | `pi --model openai/gpt-6.1-sol --thinking low` | Absent from top 8; `pi` subcommands fill those positions.                                      |
| commit and push               | `git commit` / `git push`                      | Reasonable results; preserve this behavior.                                                    |

There are two distinct failures. Retrieval can exclude a useful command before ranking starts: existing Makefile targets face a full-coverage gate that natural-language filler and misspellings cannot pass. Ranking can also place a retrieved command below incidental word matches. Short commands sharing “run” or “docs” win over project tasks; other programs sharing a model name win over the requested `pi` invocation. The prefix match between “open” and “openai” adds a false lead.

Project evidence alone does not resolve this. The current combination, `score = relevance * boost^relevance` at `crates/smcli-search/src/corpus.rs:530`, damps evidence when relevance is low. Stage 2 must change that combination, not merely add another boost under the same damping. It applies to every search through `rerank_with_evidence`, including Tab and `smcli search`, and is gated by the natural-language ranking set. Suggestions in `suggestion_index.rs` and picker-local filtering are unaffected.

The solution must distinguish missing candidates from misordered candidates. Otherwise, tuning for these examples could conceal lost recall or regress other searches.

## Goals

- Put each wanted command for the first eight reproduction queries in the top 3 through the service, in both Tab search and `smcli search`, under the reproduction conditions. For “commit and push”, keep relevant `git commit` and `git push` results in the top 3 rather than replacing them with incidental matches.
- Establish a natural-language ranking set covering all nine reproductions plus about 20 additional queries before tuning. Record candidate-pool recall separately from final rank, along with MRR, top-1, and top-3 results.
- Cause no existing case in `crates/smcli-search/tests/ranking.rs` to worsen, including `rank_alias_eval` and `rank_record_time_aliases`. Gate Stage 2 changes on this set and the natural-language set.
- Recover useful candidates despite filler words and misspellings without treating a partial or corrected match as a full exact match. Query-side correction uses a small in-house edit-distance check, only for words with no exact or prefix match, only toward index-vocabulary words, and with less weight than exact matches.
- Let project evidence improve order without removing otherwise eligible results. Preserve the distinction between the requested program and other programs that share its arguments.
- Record p50 and p95 latency before and after each stage for service and in-process search, including Tab where applicable. Set a numerical p95 budget before optional Stage 3 ships; Stage 3 must meet it and improve the target metrics.
- Keep service and in-process fallback operational. Shared retrieval and ranking improvements must benefit both; service-only evidence need not produce identical ranks. Keep protocol version 3 and frozen fixtures unchanged, and never write native shell history files.

## Non-Goals

- Natural-language command completion after a program such as `bun`, including looking up its help or tldr content. This belongs in a separate brainstorm.
- A larger model, an LLM reranker, or retraining.
- Index-side typo tolerance or trigram indexing that requires reindexing. TalaDB 0.12 BM25 matches exact tokens and has no fuzzy option; query correction will not add a dependency such as neo_frizbee.
- Changing suggestions or picker-local filtering, or redesigning the picker UI.
- Re-embedding all history on each query or storing per-query context embeddings.
- Making optional Stage 3 a prerequisite for the retrieval and ranking fixes. Context-text embedding re-scoring remains Tab-only and conditional on Stages 1–2 missing the goals; it does not narrow Stage 2 to Tab.
- Injecting never-run commands from README or CI content. New command sources require a separate evidence-based decision.
- Exact ranking parity between service and fallback, broad weight changes without ranking evidence, or rules tailored to individual reproduction strings.

## Open Questions

- Which words belong in the shared stopword list? In particular, should “locally” be ignored or retained as intent, and how should unmatched filler affect coverage?
- What minimum coverage admits a partial candidate without flooding results with commands that match only one incidental word?
- What counts as the head command when a command uses `sudo`, `env X=1`, or an executable path?
- How should “open” stop prefix-matching “openai” without losing useful prefix matches elsewhere?
- Which evidence–relevance combination meets the ranking targets without allowing strong local usage to promote unrelated commands?
- What numerical p95 latency budget is acceptable on the target machine, including warm service operation and cold fallback model loading?
- If Stages 1–2 miss, what final-score rule, context-text contents, deduplicated pool cap, cancellation behavior, and gather/embedding budgets would make optional Stage 3 worth its cost?

## Critical User Journeys (CUJs)

### CUJ 1: Find a local installation command despite a typo

- **Given:** The user is in the smcli repository, whose Makefile exposes `install`, and history includes `make install` used in this directory.
- **When:** The user searches for “build and insall locally” through Tab or `smcli search`.
- **Then:** `make install` enters the candidate pool and appears in the top 3. The corrected spelling also stays in the top 3. Correction is a lower-weight fallback, not a replacement for available exact or prefix matches; neither search executes the command.

### CUJ 2: Find project tasks from their purpose

- **Given:** The current project exposes `install`, `test`, `docs`, and `fresh` targets, with the reproduction history available.
- **When:** The user searches for “install smcli on this machine”, “run the tests”, “format the docs”, or “rebuild a clean release”.
- **Then:** `make install`, `make test`, `make docs`, or `make fresh`, respectively, appears in the top 3. Project evidence helps the intended task outrank incidental word matches without filtering other eligible commands out of search.

### CUJ 3: Recover a program invocation with its intended model

- **Given:** History contains the wanted `pi` invocation alongside bare `pi`, its subcommands, and other programs using the same model name.
- **When:** The user searches for “open pi with gpt 6” or “start pi with sol” through Tab or `smcli search`.
- **Then:** `pi --model openai/gpt-6.1-sol --thinking low` appears in the top 3 for both queries. Shared model text and the incidental “open”/“openai” match do not displace the requested program and arguments.

### CUJ 4: Preserve useful search behavior across execution paths

- **Given:** The reproduction history and ranking fixtures are available; search can run with the service or through the in-process fallback.
- **When:** The user searches for “commit and push”, and the ranking set is evaluated before and after each stage on the supported paths.
- **Then:** Relevant `git commit` and `git push` results remain in the top 3 for the reproduction, no existing ranking case worsens, and both paths remain usable. Candidate recall, rank metrics, and p95 latency are recorded separately so improvements are not inferred from rank alone. Suggestions and picker-local filtering retain their behavior, and native history remains unchanged.

## Technical Plan (Architecture)

### Shared search flow and boundaries

Keep the changes in the existing search pipeline rather than add another retrieval system. `HistorySearch::search` delegates to the evidence-aware path with no project evidence (`crates/smcli-search/src/index.rs:115`). The service calls the same engine with its available file-index evidence (`crates/smcli-service/src/service.rs:515-519`). In-process search can still use both BM25 and embeddings when it owns the index; the lock-contention text fallback is a different path, with coverage-based retrieval and no embedding leg (`crates/smcli-search/src/index.rs:532-568`).

``` text
Tab / smcli search query
          |
          v
[query interpretation + correction] <--- Stage 1
          |
          +-------------------------+
          v                         v
[BM25 leg, corrected terms]   [embedding leg, original intent]
          +------------+------------+
                       v
              [fused candidate pool]
                       |
       [existing evidence expansion] <--- fff evidence: service-only
                       |
                       v
             [deduplicated additions] <--- Stage 1: fractional gate
                       |                   and relevance
                       v
              [shared rerank] <------------ Stage 1: coverage callers
                       |                   Stage 2: head match, prefix rule,
                       |                   evidence/relevance combination
                       v
                    [results]

Stage 0 observes each box and pool recall in the in-process harness; actual service/CLI/Tab checks report final rank and total elapsed time only.
Lock-contention fallback: correction -> text coverage -> additions -> rerank.
Stage 3, optional Tab mode only: in-memory re-scoring before results;
shared scoring can run in-process; fff gathering remains service-only.
```

The indexed path currently encodes the query, embeds it, and retrieves a bounded pool (`crates/smcli-search/src/index.rs:186-222`), then appends missing candidates and reranks before truncation (`crates/smcli-search/src/index.rs:274-276`). Preserve explicit filters, resolved markers, alias handling, selection additions, and existing deduplication. Context must not become a new filter. Suggestions and picker-local filtering are outside this pipeline.

### Stage 0: establish measurements before tuning

Extend the deterministic temporary-project ranking fixture in `crates/smcli-search/tests/ranking.rs` with all nine reproduction queries and about 20 other natural-language queries. Its existing cases distinguish search from suggestions and include adversarial and held-out cases; keep these and the alias evaluations unchanged as regression checks. Use the bundled model and synthetic histories, not the user's history.

Report whether the wanted command appears in each retrieval leg, after fusion, after evidence expansion, and after additions, before rerank or final truncation. Report candidate-pool recall separately from final position, MRR, top-1, and top-3. A missing candidate is a retrieval failure; a present candidate outside the top 3 is a ranking failure. Preserve per-case positions as well as aggregates so one improvement cannot hide another regression.

Add opt-in diagnostic numbers alongside the existing reason strings: raw and normalized BM25 and embedding scores, source memberships and counts before and after deduplication, additions coverage and relevance, each boost contribution, failure multiplier, evidence increments, and final score. Include head-match and correction weights when those stages exist. This is internal diagnostic output, not a change to protocol fixtures or normal result ordering.

Measure black-box end-to-end p50/p95 and ranks for actual warm service, CLI and Tab paths. Detailed per-leg traces and setup/stage timings come only from the in-process harness, including cold/warm indexed search and lock-contention fallback, with equivalent synthetic evidence supplied explicitly; they are not internal service measurements. Distinguish picker Tab from command-line search and include corpus/index synchronization and model loading in the appropriate totals. Benchmark batch embedding of 1,000 and 5,000 short context texts without assuming a latency result. Record the baseline and choose a numerical Tab p95 budget on the target machine before considering Stage 3. Stage 0 exits when these measurements are recorded, not when either reproduction happens to improve.

### Stage 1: query correction and fractional coverage

Build a query-local interpretation once and use it in retrieval, additions, and rerank coverage. Reuse `is_stopword` (`crates/smcli-search/src/corpus.rs:228-230`) for the coverage denominator and correction eligibility; do not create a second list. Decide additions such as “locally” from the ranking set. Keep unmatched non-stopwords in the denominator rather than silently dropping every inconvenient word. An empty meaningful-term set has zero coverage and cannot admit every project command. Keep the original intent text for embeddings and selection-query matching.

Correction uses a bounded in-house edit-distance check, initially considering distance-one vocabulary neighbors. Only consider a plain query word if it has no exact or permitted prefix match anywhere in the vocabulary, not merely no match in the candidate being scored. Correct only toward vocabulary members, retain the original spelling, and weight correction below exact and prefix evidence. Ambiguous neighbors must not all become full-strength matches. Protect flags, paths, assignments, revisions, markers, model identifiers, and short program names from speculative rewriting. No dependency, tokenizer change, stored-document change, or reindex is required.

The vocabulary is a derived in-memory map of decoded `ShellTokenizer` terms and source-word provenance (as specified in Stage 1), not a flattened string set; it comes from the same `lexical_text` documents used to populate the index, not a dictionary or the program-to-next-argument map. The current synchronization constructs lexical text from corpus entries (`crates/smcli-search/src/index.rs:297-303`); lexical rendering includes executable and alias surfaces plus intent (`crates/smcli-search/src/corpus.rs:163-180`), and tokenization supplies shell words and identifier pieces (`crates/smcli-search/src/tokenizer.rs:10-44`). Both service and in-process indexed search have this corpus. Derive the same vocabulary from that corpus for lock-contention fallback without opening TalaDB. Include tokenized existing project candidates as the pending lexical surface for additions, including the empty-history path, so an unrun `make install` can still supply “install”; this is not permission to correct toward arbitrary filesystem words. A service cache is an optimization only and must track corpus and project-candidate changes.

Use original lexical retrieval plus a lower-weight corrected BM25 retrieval, merging duplicate commands without promoting corrected-only hits to full exact relevance. TalaDB's current query encoding has no term-weight parameter here, so do not describe an unweighted replacement query as lower-weight correction. Keep the embedding query's intent intact. Text fallback uses the same weighted matching without pretending it has an embedding leg.

Replace the all-words gate and flat relevance in `SearchSignals::additions` (`crates/smcli-search/src/corpus.rs:394-405`) with weighted fractional coverage: each meaningful query term contributes at most one match weight, exact/prefix contributes full weight, and correction contributes less. Admit a project candidate only at or above a named coverage floor chosen against the ranking set, then use its coverage as relevance. Partial or corrected-only coverage must stay below 1.0. Leave selection-derived relevance and explicit filtering intact. Makefile and justfile already supply Script candidates (`crates/smcli-core/src/ranking.rs:160-170`); package.json scripts also already exist (`crates/smcli-core/src/ranking.rs:147-158`). Do not rediscover them through fff.

This changes shared coverage semantics, not just additions. `entry_coverage` takes the maximum across primary and secondary command surfaces (`crates/smcli-search/src/corpus.rs:413-415`). Preserve that alias behavior while passing the same prepared terms and correction weights through every caller: project Hint/Script/Test boost scaling (`crates/smcli-search/src/corpus.rs:497-511`), filtered and unfiltered text-fallback retrieval (`crates/smcli-search/src/index.rs:532-548`), and fallback expansion scoring (`crates/smcli-search/src/index.rs:560-564`). Keep evidence-expansion terms distinct from user typo correction. Test exact-match precedence, prefixes, correction-only matches, ambiguous neighbors, all-stopword input, the coverage floor, empty history, aliases, and strict markers. Stage 1 must recover the typo target in the pool without worsening existing cases.

### Stage 2: program identity and the final scoring rule

Add a head-command signal in shared rerank. Match a query program as a whole word against the parsed executable, not a substring or an arbitrary argument: “pi” must not boost “pip” or an `oc` command mentioning pi. Define and test wrapper behavior before tuning: resolve `sudo` and `env X=1` to the executable they wrap, compare executable basenames for paths, and avoid inventing a head when parsing is ambiguous. Retain primary/secondary alias semantics. This signal helps program identity; it cannot by itself distinguish bare `pi` from the requested model-and-flags invocation, so inspect the full invocation's retrieval and evidence numbers.

Apply the prefix policy consistently in correction eligibility and coverage. The current rule accepts any prefix of at least two characters (`crates/smcli-search/src/corpus.rs:424`), allowing “open” to support “openai”. Options are treating “open” as an intent stopword while preserving explicit literal `open` commands, or restricting natural-language prefix eligibility more generally. A longer minimum is only a candidate policy if it actually excludes this four-character false lead. The owner chooses the policy using prefix and literal-command regression cases; do not special-case either reproduction query. Keep exact one-character tokens such as “6” distinct from the two-character minimum for prefixes.

Change the evidence–relevance combination for every search through `rerank_with_evidence`, not only Tab. Merely adding the head boost under the existing `relevance * boost^relevance` (`crates/smcli-search/src/corpus.rs:530`) does not satisfy this stage. Candidate formulas, for owner selection after evaluation, include a bounded product `r * (1 + a * min(e, cap))`, or an additive form `r + a * g(r) * min(e, cap)`, where `r` is normalized relevance, `e` is positive contextual evidence, and `g` is a bounded relevance gate with `g(0) = 0`. These are options, not a selected formula or weights. Define treatment of failure penalties, selection relevance, head-command evidence, and the existing post-combination file/identifier increments in the same final-score rule; avoid double-counting or leaving some evidence under accidental old damping. Preserve stable tie ordering and never drop a candidate because evidence is absent.

Choose named constants in one place only after comparing per-signal diagnostics and per-case ranks. Gate the owner-selected formula and prefix policy on both ranking sets with no existing case worsening, including alias cases. The service must meet the approved top-3 reproduction goals for both Tab and `smcli search`; shared fallback search gets the same rule with the evidence it has. Run `make ranking` and `make test-rust PKG=smcli-search` for implementation changes. No protocol version or fixture changes belong to this stage.

### Stage 3: optional Tab-only in-memory re-scoring prerequisites

Do not implement this stage unless Stages 1–2 miss the goals. Its prerequisites are:

- An explicit picker-only value in the existing optional `ServiceRequest.mode` (`crates/smcli-core/src/service_protocol.rs:99-100`), carried through local fallback too. Ordinary `smcli search` and absent mode retain Stage 2 behavior. Keep version 3 and frozen fixtures unchanged.
- One defined final-score rule consuming the query and deduplicated pool, including stored relevance and new context similarity, rather than two competing rerankers.
- A hard named pool cap after deduplication, gather and embedding time budgets, cancellation on new picker input, and a measured p95 ceiling that includes cold fallback costs and service contention.
- Defined bounded context-text fields, source-to-command templates, and duplicate merging rules. Reuse existing manifest candidates; any package.json context work must add information rather than rediscover scripts. New README or CI commands remain outside scope.
- Reuse of stored history vectors and embedding only new context text, with model identity, dimensions, and text-format compatibility checked. No full-history re-embedding and no persistence of per-query vectors. Define behavior when stored vectors are unavailable, including text fallback, before enabling this path.
- A bounded evidence interface for any new fff context gathering and neutral behavior when evidence is absent. fff remains service-only and never links into the CLI or core. Pure in-memory scoring can be shared with in-process Tab fallback using available context; it does not promise service rank parity.
- A latency assertion and demonstrated ranking improvement against Stage 2 under the Stage 0 budget before shipping.

## Alternatives Considered

Only options that competed with the chosen plan are listed. Options dropped early (running `--help`, man pages, a bigger model, re-embedding all history, grepping the index file) are recorded in `concepts_brief.md`.

### Alternative 1: Typo tolerance through `neo_frizbee`

- **Pros**: fff's typo-tolerant matcher, already compiled into the workspace (0.13.3 via fff-search); no matching code of our own to write.
- **Cons**: Becomes a direct dependency of `smcli-search` and the CLI binary; tuned for file and identifier names, so its scores on short English words are unproven.
- **Reason for Rejection**: Owner decision. A small in-house edit-distance check limited to unmatched words and index vocabulary covers the need without a new direct dependency.

### Alternative 2: Typo tolerance in the index (tokenizer or trigrams)

- **Pros**: Corrects at retrieval time for every query, with no query-side step.
- **Cons**: TalaDB 0.12 looks up exact tokens only, so this means indexing extra tokens: a forced reindex and a larger index.
- **Reason for Rejection**: Query-side correction gets the same recall without touching stored data.

### Alternative 3: Keep the score formula and raise evidence weights

- **Pros**: Smallest change: named constants only.
- **Cons**: `relevance * boost^relevance` (`corpus.rs:530`) shrinks any boost toward 1 when relevance is low, so larger weights barely move weakly matched candidates like `make install`, and they push strongly matched ones further.
- **Reason for Rejection**: Does not reach the failure the reproduction showed. The formula itself changes in Stage 2.

### Alternative 4: Change scoring for Tab only, through `ServiceRequest.mode`

- **Pros**: `smcli search` keeps today's ranking; any regression is confined to the picker.
- **Cons**: The same query ranks differently in two places; two scoring rules to maintain and test.
- **Reason for Rejection**: Owner decision. One rule for every search, gated by the ranking set.

### Alternative 5: Widen retrieval (larger k on the BM25 and embedding legs)

- **Pros**: One constant; cheap to try.
- **Cons**: The measured misses come from the additions gate and from ranking, which a larger pool does not fix; more candidates also means more noise for the reranker.
- **Reason for Rejection**: Not supported by the reproduction. Revisit if Stage 0 shows the legs themselves miss the wanted command.

### Alternative 6: Build the Tab re-scoring pass first (Stage 3 now)

- **Pros**: Uses the fast embedding model with context text, which matches the original intent of pairing the model with fff evidence.
- **Cons**: Needs a final-score rule, pool cap, time budgets and cancellation before it can be measured; it would compete with the existing reranker.
- **Reason for Rejection**: Deferred, not dropped. Stages 1-2 fix the observed failures with shared code; Stage 3 follows only if they miss.

### Open choices (decided after Stage 0, not rejected)

- **Score formula**: bounded product `r * (1 + a * min(e, cap))` versus gated additive `r + a * g(r) * min(e, cap)`.
- **"open" / "openai" prefix lead**: treat "open" as a natural-language stopword versus restricting prefix matching for natural-language queries in general.

------------------------------------------------------------------------

## Non-Functional Requirements

- **Latency:** Stage 0 records current p50/p95 and raw samples on the target machine before tuning. Freeze the ordered query list, synthetic seed, evidence fixture and sampling order before baseline collection. At 1,000, 10,000 and 100,000 synthetic history rows, run 5 warmups then 100 measured requests per query for each warm configuration, in fixed round-robin query order. For each cold case run 20 fresh processes, each measuring one query against an existing index; report fresh-index construction separately. A comparison key is query, row count, path (service/indexed-local/held-lock), surface (CLI/Tab/harness), evidence fixture and lifecycle (warm-loaded/process-cold/model-cold/fresh-index). Cold and warm are separate cases, never paired or pooled. Use nearest-rank p95 per key, not a pooled query percentile; every matched key must pass the regression gate. Keep machine, model and workload metadata fixed and recorded. Report initial index construction separately from a cold process opening an existing index. Detailed component timings belong only to the in-process harness: time model construction and corpus reads around existing setup APIs, then index sync, interpretation, retrieval legs, expansion, additions and rerank inside the engine. Service and actual CLI/Tab paths report black-box totals and ranks only; do not infer their internal component times from local measurements or add service/CLI instrumentation. Measure Tab from query submission to displayed results, not picker first draw. For Stages 1–2, require p95 no greater than baseline plus the larger of 10 ms or 10% in each matched configuration. Before Stage 3, the owner signs off explicit millisecond ceilings for warm-service Tab, warm-local Tab, cold-local Tab and contention fallback, using these measurements; no latency claim or Stage 3 rollout precedes that sign-off. Batch-embedding measurements at 1,000 and 5,000 texts are evidence, not assumed interactive budgets.
- **Storage and compatibility:** No tokenizer, indexed document, embedding text, model identity, database schema or index-name version changes in Stages 0–2; existing indexes open without a forced reindex. Rollback is the previous binary with the same index. Query vocabulary is derived in memory, not persisted. Ordinary corpus synchronization still indexes genuinely new commands.
- **Privacy and safety:** Native shell history is read-only. Tests and benchmarks use a temporary HOME, database, runtime directory and synthetic native files; verify native files byte-for-byte afterward. Never run retrieved commands or export user history. Diagnostics are opt-in, local and excluded from normal stdout and wire responses; warn that diagnostic command text is sensitive. No network access, help/man subprocesses or new dependency.
- **Fallback limits:** Indexed service and indexed local search share interpretation and scoring. Held-lock fallback shares weighted coverage, additions and scoring, but has no dense retrieval. fff evidence remains service-only; neither identical pools nor identical ranks across evidence availability are promised. Compare service/local with identical supplied evidence in unit tests, and report actual-path metrics separately. Missing evidence is neutral, not a filter.
- **Protocol:** `crates/smcli-core/tests/fixtures/service_protocol` and protocol `VERSION = 3` in `crates/smcli-core/src/service_protocol.rs` remain unchanged, including conditional Stage 3. No diagnostics in `SearchEntry` serialization. Stage 3 may use the existing optional mode string without adding a wire field.

### Future Improvements

- **Natural-language command completion:** A separate brainstorm covers completing a command after a program such as `bun`, and whether help/tldr data is useful. This plan only searches history and existing project candidates.
- **Stage 3 re-scoring:** Only reopen after the Stage 2 acceptance report shows unmet goals. Write and approve a separate bounded Tab-only implementation specification satisfying the prerequisites above before changing its conditional files. Do not add its request plumbing, vector cache or context gathering speculatively.
- **whatis descriptions at index time:** Explore local program descriptions as indexed intent in a separate experiment. Measure relevance, freshness, collection cost and privacy first. This would change stored text and may require reindexing, so it is not part of the no-reindex rollout here.

## Detailed Implementation Plan

### Target Files to Create/Modify

This is the complete planned file set for Stages 0–2. Repeated paths indicate further edits in that stage, not duplicate implementations. No files are deleted. New paths are explicitly marked Create; all other paths exist in the current checkout. Line citations refer to the pre-implementation source, not expected future line numbers. Keep existing ranking cases and fixtures intact rather than replacing their expectations to pass a regression. All Markdown changes use iwe and are followed by `make docs`.

#### Stage 0: measurements, diagnostics and decision gates

1. **`crates/smcli-search/src/index.rs` — Modify**
   - **Type / Scope:** Shared indexed and text-fallback retrieval; internal diagnostics.
   - **Proposed Changes:** For the in-process harness only, add `SearchTrace` and `CandidateTrace` structs and `search_with_trace(...existing search arguments..., trace: Option<&mut SearchTrace>) -> Result<Vec<SearchEntry>>`. Existing public entry points delegate with no trace, preserving their signatures and behavior. Trace stage durations, candidate keys, source memberships, raw scores, normalization denominators and normalized scores at original BM25, dense, fusion, evidence expansion, additions and final ranking boundaries. Distinguish filtered retrieval from its safety-net retrieval. Record counts before/after deduplication and the untruncated pool. The literal path already exposes both legs; the intent path uses TalaDB hybrid search. When tracing intent search, issue diagnostic-only separate raw-leg queries with identical text, vector, k and filters and label them observations, not a replacement fusion. Preserve actual hybrid scores and ordering; time those extra diagnostic queries separately and exclude traced runs from end-to-end latency samples. If TalaDB cannot expose a raw component of a fused hit, mark it unavailable rather than manufacture it. Add a trace/no-trace order-equivalence test. Retrieval boundaries are currently in `index.rs:186-276`; held-lock expansion and additions are in `index.rs:559-568`.
2. **`crates/smcli-search/src/corpus.rs` — Modify**
   - **Type / Scope:** Rerank diagnostic collection.
   - **Proposed Changes:** Add `rerank_with_trace(...existing rerank arguments..., trace: Option<&mut SearchTrace>) -> Vec<SearchEntry>`, keeping existing wrappers. Record effective relevance after primary-alias sharing and selection relevance, every positive evidence contribution, failure multiplier, current final score and stable tie offset. Later stages fill correction and head fields in the same record. Do not add numeric fields to returned wire entries or change normal reason strings just to expose diagnostics. The existing score and post-score increments are in `corpus.rs:522-556`.
3. **`crates/smcli-search/tests/ranking.rs` — Modify**
   - **Type / Scope:** Deterministic ranking and test-only policy evaluation.
   - **Proposed Changes:** Add `NaturalLanguageCase { query, expected_commands, cohort, expected_top_k }` and `rank_natural_language_eval()`. Use all nine reproduction queries, checking both git commands for the ninth, plus exactly 20 fixed cases: project install/test/docs/fresh paraphrases, justfile and package scripts, explicit program invocations, literal `open`, useful prefixes, ambiguous typos, aliases, unrelated high-usage distractors and empty meaningful input. Freeze labels and split the additional cases into 10 tuning and 10 held-out cases before scoring. Seed the real temporary manifests and competing pi/oc/codex invocations; include empty-history project candidates. Normal `make ranking` prints aggregate metrics and existing concise case results, not detailed command traces. Enable detailed collection only with a separate `SMCLI_RANK_TRACE=/absolute/private/file.json` opt-in. Before writing, warn on stderr that command text is sensitive; create the parent directory with mode 0700 and the new file exclusively with mode 0600, refusing an existing destination or symlink. Write per-command leg membership, pool recall, rank and final score there, not stdout. Detailed path labels cover only the in-process indexed and held-lock harness with explicitly supplied synthetic evidence; service-equivalent means equivalent supplied evidence, not a traced service request. Actual service/CLI/Tab ranks and total timings come from black-box checks without wire diagnostics. A miss has reciprocal rank zero. Pool recall means wanted commands present before truncation divided by wanted commands; report it at each boundary, never infer it from final rank. Preserve the existing alias evaluations, suggestions and adversarial/held-out labels.
   - **Proposed Changes:** Add test-only `ScoreFormula::{BoundedProduct, GatedAdditive}` and `IntentPrefixPolicy::{IntentOpenStopword, TokenStartOnly}` in the evaluation harness, not runtime settings. Provide a temporary test-only injection point covering prefix policy, stopwords, coverage floor, correction weight, head weight and formula constants through preparation, retrieval, additions and rerank. Use a `cfg(test)` unit-test harness inside the owning crate, since an integration test cannot access private or `cfg(test)` library items; keep the fixed case data in `tests/ranking.rs` reusable by that harness. Production wrappers retain current defaults until selection. Run each retrieval-affecting candidate fresh through this seam before owner approval; install only the approved constants/policy and remove the temporary alternatives, without a runtime switch. Replay captured per-signal traces with `evaluate_score(formula, relevance, evidence, failure) -> f64`. Test product `r * (1 + a * min(e, cap))` against additive `r + a * min(1, r / gate) * min(e, cap)`. Evaluate the cross-product of a = {0.1, 0.25, 0.5, 1}, cap = {1, 2, 4}, gate = {0.25, 0.5, 0.75} for additive, and head boost = {0.2, 0.5, 1}. Baseline traces can replay scores; prefix/coverage changes require fresh retrieval runs, not replay alone.
   - **Proposed Changes:** Compare the two prefix policies on the same fixed queries. IntentOpenStopword removes `open` only for intent queries through the shared stopword predicate; literal executable `open` remains exact. TokenStartOnly allows intent prefixes only at whole shell-word starts of standalone alphanumeric words, not identifier pieces after `/`, `-`, `.`, `_` or inside flags; exact piece matches such as `6` remain valid. Thus `sto` can match `storybook`, but `open` cannot match the identifier component of `openai/gpt-6.1-sol`. Literal-prefix behavior is unchanged. Evaluate `locally` retained versus added to the shared intent stopwords, coverage floor = {0.2, 0.25, 1/3, 0.5}, and correction weight = {0.5, 0.75}; maximum edit distance stays one.
   - **Proposed Changes:** Stage 0 records the unmodified baseline and evaluation protocol first. The test-only candidates become runnable as Stage 1/2 helpers are added; do not implement production alternatives in Stage 0 just to get a decision early. Reject a configuration if any existing expected-command rank worsens, any forbidden result violates its check, any existing alias case worsens, or any natural-language held-out wanted-command pool membership/rank worsens. Among survivors, prefer all reproduction goals met, then higher held-out pool recall, top-3, MRR and top-1 in that order; then lower measured p95. Exact ties prefer fewer changed stopwords and bounded product. If none survives, stop and report the failing cases instead of weakening gates. The owner signs off the per-case comparison, selected formula/prefix policy, stopword change and constants before production selection; no implicit winner or runtime policy flag.
4. **`crates/smcli-search/tests/search_latency.rs` — Create**
   - **Type / Scope:** Ignored release-mode benchmark test, including embedding microbenchmark.
   - **Proposed Changes:** Add ignored `search_latency()` using `Instant`, synthetic corpora, bundled Potion and existing search APIs. Measure repeated requests on one loaded local engine, fresh engine/model loading with existing indexes, and held-lock fallback. Add `context_embedding_batches()` for 1,000/5,000 short synthetic texts using `Potion::embed`, recording batch time, dimensions and RSS where available. No production change to `potion.rs` is needed. Keep normal test runs deterministic: no wall-clock pass/fail threshold in ordinary CI. Use nearest-rank p95 and retain raw samples. Time model construction and corpus reads around existing `Potion`/`setup` calls in this harness, with no production setup instrumentation; trace only engine-owned stages inside search. Keep traced samples separate from untraced totals. Service and actual CLI/Tab requests have black-box totals and ranks only. Both ignored tests write distinct artifacts in a unique per-invocation directory under `dist/search-latency/`; record those paths so later service/local target invocations cannot overwrite either test’s results.
5. **`scripts/bench-baseline.py` — Modify**
   - **Type / Scope:** Existing isolated CLI/service/PTY benchmark.
   - **Proposed Changes:** Add opt-in `--natural-language` workload and `--self-test` for new percentile/report aggregation helpers. Make `--binary` optional at argument parsing; dispatch self-test immediately after parsing, before binary resolution or workload setup. For normal workloads require `--binary` via a parser error and retain strict path validation. Self-test runs without a binary or model. Reuse synthetic seeding and cleanup. Measure CLI search with service on/off and PTY Tab query-to-results, including repeated queries in one live picker for warm local engine measurements. Label process-cold, model-cold and fresh-index costs separately; first draw is not Tab latency. Hold the index advisory lock from a helper process for contention samples; release it in `finally`. Record path, row count, wanted-command positions and raw times; do not turn JSON diagnostics into a new protocol response. Existing baseline arguments and default workloads remain supported.
6. **`Makefile` — Modify**
   - **Type / Scope:** Benchmark entry point.
   - **Proposed Changes:** Add phony `search-latency`: run `cargo test --release -p smcli-search --test search_latency -- --ignored --nocapture` with no function-name filter, so both ignored `search_latency()` and `context_embedding_batches()` run, then the extended Python benchmark against release binaries, using `BENCH_ARGS`. Build through the existing release/build recipes, not a second build script. Reuse `ranking` unchanged; no new ranking target is needed. Tests generate latency artifacts under `dist/`, not committed fixtures.
7. **`AGENTS.md` — Modify**
   - **Type / Scope:** Agent command reference, outside the protected team block.
   - **Proposed Changes:** Document `make search-latency`, its synthetic-only behavior, `BENCH_ARGS`, the ignored-test distinction and Stage 0 baseline/sign-off requirement because a new command is added.
8. **`agents/reference/baseline.md` — Modify**
   - **Type / Scope:** Benchmark method and measurement record.
   - **Proposed Changes:** Append current Rust measurements, machine/toolchain/model/binary identity, raw artifact locations, per-path p50/p95 and signed-off numeric budgets. Do not replace the historical Swift measurements or reuse them as the baseline. Record the owner’s policy comparison/sign-off here as results become available.
9. **`crates/smcli-search/README.md` and `crates/smcli-search/src/README.md` — Modify**
   - **Type / Scope:** Ranking and verification documentation.
   - **Proposed Changes:** Explain opt-in diagnostic boundaries, natural-language cohorts/metrics and latency command. State that normal API/protocol output is unchanged and that held-lock fallback lacks dense retrieval. Update the source-module README’s API list for trace helpers and its harness-only measurement/privacy boundaries.

Stage 0 adds no production scoring change. Generated latency reports are runtime outputs, not source files to create or commit; the new cases live inline in the ranking test, so no new fixture files are needed.

#### Stage 1: shared interpretation and weighted coverage

1. **`crates/smcli-search/src/corpus.rs` — Modify**
   - **Type / Scope:** Query preparation, correction, additions and coverage; colocated unit tests.
   - **Proposed Changes:** Define `PreparedQuery { original_intent, terms: Vec<QueryTerm> }` and `QueryTerm { original, correction: Option<String>, correction_weight }`. Add `prepare_query(query: &str, literal: bool, vocabulary: &QueryVocabulary) -> PreparedQuery`, `edit_distance_at_most_one(a: &str, b: &str) -> bool`, `coverage_prepared(command: &str, query: &PreparedQuery) -> f64`, and `entry_coverage_prepared(entry: &SearchEntry, query: &PreparedQuery) -> f64`. Keep existing public `coverage`/`entry_coverage` wrappers for callers that lack a corpus; they build an uncorrected prepared query rather than a candidate-local correction vocabulary. Thread prepared values into new internal additions/rerank helpers while preserving public wrappers. Reuse `is_stopword` rather than a new list. Preserve alias maximum across surfaces, not a union that can fabricate full coverage.
   - **Proposed Changes:** Define constants here only: `MAX_QUERY_EDIT_DISTANCE = 1`, `CORRECTION_WEIGHT`, `ADDITIONS_COVERAGE_FLOOR`. During the sweep pass the last two through the temporary test-only policy seam; production retains baseline behavior. After evaluation and owner approval, install the selected values as constants, not unmeasured defaults. Apply the same sequence to prefix, stopword, head and final-score choices in Stage 2. Correct only ASCII alphabetic words of at least four characters with no global exact/permitted-prefix match; protect shell punctuation, flags, assignments, paths, markers, revisions, model identifiers and shorter program names. Use insertion/deletion/substitution, not transposition, for distance one. If more than one equally close neighbor exists, do not correct. Deduplicate query terms before coverage. Exact/prefix contributes 1, correction contributes `CORRECTION_WEIGHT`, each term at most once. Empty meaningful terms score zero; unmatched terms remain in the denominator. Additions at or above the floor use coverage as relevance; retain selection relevance independently and the existing filter/dedup logic.
   - **Proposed Changes:** Add unit tests for global exact precedence, literal and intent prefixes, correction-only/partial coverage below one, ambiguity, protected tokens, all-stopwords, threshold boundaries, aliases and selection additions. Preserve original query text for pick similarity.
2. **`crates/smcli-search/src/index.rs` — Modify**
   - **Type / Scope:** Vocabulary construction, corrected retrieval and all coverage callers; unit tests.
   - **Proposed Changes:** Add `query_vocabulary(corpus: &HashMap<String, SearchEntry>, signals: &SearchSignals) -> QueryVocabulary` from the same lexical documents plus existing Hint/Script/Test candidate lexical surfaces. Use an in-memory map from normalized term to provenance flags (whole shell word, standalone alphanumeric word, identifier piece), not a bare string set. Derive provenance from the source words while following the existing tokenizer’s line skipping, lowercase and piece rules, including camel-case pieces; do not try to recover provenance from its flattened token output. Union flags when a term occurs in multiple contexts: a standalone occurrence makes that vocabulary term eligible for a global standalone prefix, but never grants standalone provenance to another command’s identifier piece. Build command-surface terms with the same provenance helper and apply the same prefix predicate to each occurrence in correction eligibility and coverage. Exact piece matches remain valid. Test an identifier-only `openai/gpt-6.1-sol`, standalone `openai`, and both together: global eligibility may change when the standalone word exists, but coverage for the identifier-only command must not gain an `open` prefix. Keep primary/secondary surfaces separate. Build once per request after marker parsing and before empty-history/held-lock branches; do not add a persistent cache. Original marker constraints and literal classification remain authoritative. Pass the same prepared query to additions, project-boost scaling, filtered/unfiltered fallback coverage, fallback expansion scoring and the confidence gate inside `expansion_terms` (`index.rs:572-582`). Prepare evidence-expansion words separately with corrections disabled, so grep evidence is not a user typo.
   - **Proposed Changes:** Keep original hybrid retrieval intact. Issue one extra BM25 query only when unambiguous corrections exist. Use corrected spellings for corrected terms, retain other lexical terms and original embedding intent. Normalize corrected BM25 by its own positive best score, multiply by `CORRECTION_WEIGHT`, and merge duplicate keys by maximum relevance, never sum. Do not renormalize corrected-only scores back to one. Apply the existing parsed-filter safety-net scale and strict-marker rules to this leg too. Record original/corrected membership separately. Tests cover indexed/fallback/empty-history typo recovery, unchanged filters, strict/unresolved markers, expansion confidence and an existing-index query without rebuilding documents.
3. **`crates/smcli-search/tests/ranking.rs` — Modify**
   - **Type / Scope:** Retrieval acceptance tests and candidate sweeps.
   - **Proposed Changes:** Run prepared-query coverage and correction candidates through the frozen Stage 0 set. Require the typo install command in the post-additions pool with no existing case worsening. Report pre-rerank recall separately to avoid crediting a scoring change for retrieval. Commit approved constants only after the owner reviews the comparison; keep policy exploration test-only.
4. **`crates/smcli-search/README.md` and `crates/smcli-search/src/README.md` — Modify**
   - **Type / Scope:** Ranking behavior documentation.
   - **Proposed Changes:** Describe query-local vocabulary, correction safeguards, fractional additions relevance, aliases and the lack of reindexing. Document the chosen floor/weight and shared fallback semantics. Update the source-module API list for prepared-query and provenance helpers and its coverage/correction invariants.
5. **`agents/reference/baseline.md` — Modify**
   - **Type / Scope:** Stage 1 results.
   - **Proposed Changes:** Append per-path before/after latency, pool recall, case regressions and owner-approved correction/coverage decisions.

#### Stage 2: program identity, prefix policy and one final score

1. **`crates/smcli-search/src/corpus.rs` — Modify**
   - **Type / Scope:** Shared head parsing, prefix semantics, final scoring and unit tests.
   - **Proposed Changes:** Add `head_command(command: &str) -> Option<String>` using existing `smcli_core::parse::shell_words`; strip leading assignments, unwrap plain `sudo` (and `sudo --`) and plain `env` assignments (and `env --`), then compare executable basenames. Return None for unsupported wrapper options, shell operators, substitutions or malformed parsing instead of guessing. Test repeated supported wrappers, absolute executable paths, quoted arguments, pipelines, `sudo -u`, `env -u`, pi versus pip, and pi appearing only as another program’s argument. Add `head_match(entry: &SearchEntry, query: &PreparedQuery) -> bool`, exact whole-query-term matching on either alias surface; never use correction/prefix for program identity. Add `HEAD_COMMAND_BOOST` from the approved sweep.
   - **Proposed Changes:** Implement only the signed-off prefix policy through `prefix_matches(term, surface_token_with_provenance, literal) -> bool`, used both by global correction eligibility and coverage. Preserve literal `open` and exact numeric terms. Keep policy alternatives only in evaluation tests; do not ship a runtime enum/config switch. If the stopword option wins, extend the shared predicate with intent/literal context rather than adding an independent stopword list.
   - **Proposed Changes:** Add `final_score(relevance: f64, positive_evidence: f64, failure: f64) -> f64` with the owner-selected formula. Clamp relevance to [0,1]; selection relevance is applied first as today, and alias-primary relevance sharing is retained. Positive evidence is the sum of current boost contributions above the neutral 1, plus head, PATH 0.1, file presence 0.08, git 0.04 times activity, and identifier 0.06 defined or 0.03 seen (never both). Move those existing post-score increments into this evidence sum exactly once. Preserve existing signal-disable behavior. Apply `failure = 0.75 + 0.25 * success_rate` to the whole combined score for captured entries with runs, otherwise one. No evidence produces relevance times failure; zero relevance stays zero. Keep all candidates and original-offset tie breaking. Define `EVIDENCE_SCALE`, `EVIDENCE_CAP` and, only for additive, `RELEVANCE_GATE` beside the coverage/head constants. Remove the old exponent and obsolete comments. Test zero/no evidence, saturation, failure placement, selection, head-only evidence, no double counting, disabled signals, stable ties and diagnostics equality.
2. **`crates/smcli-search/src/index.rs` — Modify**
   - **Type / Scope:** Consistent selected prefix interpretation and traces.
   - **Proposed Changes:** Carry literal/intent context and token provenance needed by the selected prefix policy into vocabulary matching, without altering stored tokenizer output. Verify every indexed and fallback query uses the same policy; extend tests for `open`/`openai`, useful standalone prefixes and exact `6`.
3. **`crates/smcli-search/tests/ranking.rs` — Modify**
   - **Type / Scope:** Final ranking acceptance and test-only formula comparison.
   - **Proposed Changes:** Complete the cross-policy evaluation and record effective relevance, head evidence and all final-score components for the requested full pi invocation as well as bare pi. Gate production selection on every old expected-command rank and alias result, not only aggregate floors. Require the first eight wanted commands top-3 and both git commands top-3 for the ninth with service evidence; require no held-out regression. Report separate indexed-local and held-lock results, with known evidence limits, and verify score parity with identical synthetic evidence.
4. **`scripts/test-service.py` — Modify**
   - **Type / Scope:** Service integration acceptance.
   - **Proposed Changes:** Seed a temporary project/history with the nine reproduction cases and distractors. Exercise version-3 search requests and CLI search, assert intended top-3 results, unchanged default/absent mode and neutral behavior when fff is unavailable. No frozen fixture edits. Keep assertions isolated from the user’s database and service.
5. **`scripts/test-cli.sh` — Modify**
   - **Type / Scope:** CLI fallback smoke and native-history invariants.
   - **Proposed Changes:** Add typo install/project-task smoke in an isolated manifest project with `SMCLI_SERVICE=0`; verify strict filters and output shape. Keep existing byte comparisons for bash/zsh native files. Exact service-only rank expectations do not belong in fallback smoke.
6. **`scripts/bench-baseline.py` — Modify**
   - **Type / Scope:** PTY Tab acceptance within the Stage 0 workload.
   - **Proposed Changes:** Assert wanted-command positions for the nine Tab queries using the isolated reproduction fixture, independently from CLI output. Record rank failures with timing samples; no changes to picker-local filtering or production UI.
7. **`crates/smcli-search/README.md` and `crates/smcli-search/src/README.md` — Modify**
   - **Type / Scope:** Final ranking contract.
   - **Proposed Changes:** Replace the old combination description with the selected formula, constants, head/wrapper rules, prefix behavior and failure handling; retain no-filter and fallback-limit invariants. Update the source-module API list and ranking/prefix invariants too.
8. **`agents/reference/baseline.md` — Modify**
   - **Type / Scope:** Final selection and rollout evidence.
   - **Proposed Changes:** Record the chosen policy/formula, signed-off constant values, all per-case deltas, both ranking sets and latency comparison. State whether Stage 3 is justified; a remaining miss is not permission to enable it automatically.

No Stage 1/2 changes are planned in manifests, lockfiles, tokenizer, native-history readers, core ranking/context candidate discovery, shell hooks, CLI request plumbing, picker UI or protocol definitions. Existing Makefile test commands suffice after Stage 0. No files are deleted in any committed stage.

#### Stage 3: conditional file list only

Only if Stages 1–2 miss the goals and a separate specification is approved, the following existing files form the conditional modification set. This is not authorization to implement Stage 3 or a choice of unmeasured budgets.

- **`crates/smcli/src/cli.rs` — Modify. Type / Scope:** Tab request and local fallback plumbing. **Proposed Changes:** Carry a picker-only mode through both paths, retaining ordinary CLI defaults.
- **`crates/smcli/src/picker.rs` — Modify. Type / Scope:** Query lifecycle. **Proposed Changes:** Cancel obsolete requests and reject stale results without changing local filtering.
- **`crates/smcli-service/src/service.rs` — Modify. Type / Scope:** Mode dispatch. **Proposed Changes:** Route the optional picker mode to bounded re-scoring; absent/ordinary mode keeps Stage 2.
- **`crates/smcli-service/src/evidence.rs` — Modify. Type / Scope:** fff evidence adapter. **Proposed Changes:** Gather only approved bounded context; missing/late evidence is neutral.
- **`crates/smcli-search/src/evidence.rs` — Modify. Type / Scope:** Shared evidence contract. **Proposed Changes:** Add bounded context access only if the separate specification requires it; keep fff out of this crate.
- **`crates/smcli-search/src/index.rs` — Modify. Type / Scope:** Deduplicated pool and vector access. **Proposed Changes:** Enforce the approved cap/time budgets and compatible stored-vector reuse; skip re-scoring when unavailable.
- **`crates/smcli-search/src/corpus.rs` — Modify. Type / Scope:** Shared final score. **Proposed Changes:** Incorporate approved context similarity in the single score, not a second competing reranker.
- **`crates/smcli-search/tests/ranking.rs` and `crates/smcli-search/tests/search_latency.rs` — Modify. Type / Scope:** Quality and latency gates. **Proposed Changes:** Compare Stage 3 with Stage 2, including cancellation, cold fallback and contention.
- **`scripts/test-service.py` and `scripts/bench-baseline.py` — Modify. Type / Scope:** Mode/default compatibility and Tab integration. **Proposed Changes:** Test picker-only behavior, stale-query handling and measured ceilings.
- **`crates/smcli-search/README.md` and `agents/reference/baseline.md` — Modify. Type / Scope:** Behavior and evidence documentation. **Proposed Changes:** Record exact mode scope, limits and signed-off results.

No Stage 3 file creation/deletion is authorized by this list. Protocol VERSION and `crates/smcli-core/tests/fixtures/service_protocol` remain unchanged; the existing optional mode field needs no schema edit. A newly required file or command must be listed in the separate specification before work begins.

### Verification Plan

- **Build and focused tests:** Run `make build`, then `make test-rust PKG=smcli-search`. New nontrivial helpers leave small unit tests in their owning source files. Run `make ranking` after every retrieval/scoring change; compare per-case expected positions against the Stage 0 artifact, including `rank_alias_eval` and `rank_record_time_aliases`, not just the existing aggregate floors. Run `python3 scripts/bench-baseline.py --self-test` for the new aggregation logic.
- **Natural-language metrics:** Run normal `make ranking` without detailed traces, then opt in separately with `SMCLI_RANK_TRACE` pointing to a new private file as specified above; heed the sensitivity warning and do not redirect command-level traces to stdout. Report each wanted command’s membership in original BM25, corrected BM25, dense, fused, expanded and post-additions pools; raw/deduplicated counts; final position; pool recall; MRR; top-1; top-3. Separate reproduction, tuning and held-out cohorts, in-process indexed synthetic service-equivalent evidence, indexed local and text fallback. Detailed traces never describe an actual service request; verify actual service/CLI/Tab ranks separately through black-box checks. Missing candidates and misranking are distinct failures. Require all reproduction service/Tab top-3 goals, no existing expected-command rank regression or forbidden-result violation, no alias regression and no held-out regression. Stage 1 specifically requires typo target pool recovery.
- **Latency command:** After Stage 0 adds the target, run `make search-latency BENCH_ARGS="--natural-language --samples 100 --rows 1000 10000 100000 --service on --repo-files --output dist/search-latency-service.json"`, then the same command with `--service off --output dist/search-latency-local.json`. Empty `--repo-files` skips unrelated file/grep repository workloads. The target runs the ignored local/held-lock and 1,000/5,000-text embedding tests as well as the external workload. Record fresh-process counts separately; do not call one-shot CLI repetitions a warm loaded local engine. Confirm the unfiltered Cargo command runs both ignored tests and retain both artifacts from each invocation. Compare untraced per-query/key p95s to matched baselines and per-path budgets, retain raw samples and host/model metadata, and get owner sign-off before Stage 3. Timing assertions apply on the calibrated target machine, not noisy ordinary CI.
- **Integration:** Run `make test-service` for changed service checks, `make test-cli` for CLI/fallback shape and native-file byte checks, then `make test` as the full gate. Verify Tab rank assertions in the extended PTY benchmark separately from ordinary CLI results. If Stage 3 later changes picker code, additionally run `make test-picker` and `make test-rust PKG=smcli`.
- **Compatibility:** Open an index made by the baseline binary and confirm no format-driven rebuild, compare trace/no-trace ordering, exercise strict markers and alias-disabled operation, and verify all frozen protocol fixtures and VERSION remain untouched. With identical available evidence, shared scoring must agree; do not assert dense-service and held-lock candidate equality. Check no new dependency or fff link enters CLI/core.
- **Documentation and scope:** Run `make docs` after Markdown changes, inspect the diff and reject unrelated rewrites. This document records the original design; accepted results are summarized in `agents/reviews/codebase-review-2026-11-08.md`. Future implementation must pass the gates above before completion.
