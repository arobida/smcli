# Context-steered semantic search in the smcli picker

**Author:** Andrew Robida
**Date:** 2026-11-01

## Overview

When a user types a natural-language query in the history picker and presses Tab, the results are poor. Two queries show it:

- **Q1:** "build and insall locally" (the typo was not intended) returns only commands containing "build". `make install` is missing from the top 11, even though it has been run many times in this cwd and the Makefile has an `install` target.
- **Q2:** "open pi with gpt 6" puts `oc mini -m openai/gpt-6.1-sol` and codex commands first. The wanted command, `pi --model openai/gpt-6.1-sol --thinking low`, comes 11th, below bare `pi`.

The goal: the potion-shell-16m model handles meaning, project evidence (Makefile targets, scripts, cwd use) steers ranking, and context raises rank but never filters. Constraints: no bigger model, reuse existing infrastructure, keep the service and the in-process fallback both working, never write native history files.

This revision weighs a new plan from the user. Pressing Tab means fast search failed, so extra latency is acceptable. Potion is fast enough to embed a few thousand context texts per query, held in memory and never stored. The plan adds a Tab-only re-scoring pass over a wide pool that includes fff grep hits from command-defining files. No reviewer reproduced Q1 or Q2 or measured embed latency. Code references come from reading the code.

## Analysis

### Findings from the first round that still hold

- **The additions gate.** `SearchSignals::additions` (`crates/smcli-search/src/corpus.rs:374-407`) only admits a project candidate when `coverage(...) == 1.0` (`corpus.rs:394`), and then gives it a flat relevance of 1.0 (`corpus.rs:405`).
- **Coverage matching.** Exact or prefix match of at least 2 characters (`corpus.rs:424`). No typo tolerance anywhere: the tokenizer does exact and camel-case splitting only (`tokenizer.rs:10-44`). For Q1, "and", "insall" and "locally" never match, so `make install` cannot pass the gate.
- **Coverage scales the boosts.** Script and Hint boosts are multiplied by coverage (`corpus.rs:497-511`), and score is `relevance * boost^relevance` (`corpus.rs:530`). Weak semantic hits get almost no help from context.
- **The two paths differ.** Only the service passes evidence (`crates/smcli-service/src/service.rs:368-370`, `:516-519`). The in-process `search()` passes `None` (`crates/smcli-search/src/index.rs:115`), and the text fallback has no embedding leg (`index.rs:532`, `:548`).
- **Overfitting.** Two queries cannot justify weight changes.

### New facts from this round

- **Makefile and justfile targets are already Script candidates.** `make {name}` and `just {name}` are built at `crates/smcli-core/src/ranking.rs:160` and `:170`, discovered at `crates/smcli-core/src/context.rs:22`, and fed into `additions` (`corpus.rs:392`). The open question from round one is answered: Q1's target is in the candidate set and is blocked only by the gate. Grepping Makefiles and justfiles with fff would duplicate this.
- **A stopword list exists.** `is_stopword` (`corpus.rs:228-230`, 17 words, private). Reuse it. "locally" is probably not in it, so Q1 still scores at most about 2 of 4 words unless the list grows or the denominator excludes unmatched filler.
- **Batch embedding exists.** `Potion::embed(&[String])` (`crates/smcli-search/src/potion.rs:57`) encodes a whole slice in one call. `embedding_text` (`corpus.rs:153`) caps input at 1,500 characters. Static Model2Vec embedding is likely tens of milliseconds for a few thousand short texts, but nobody has measured it and no benchmark exists.
- **The fff grep budget is small and returns words.** The plan's citation `file_search.rs:136` is only `grep() -> search(request, true)`. The real budgets are `crates/smcli-service/src/evidence.rs:111` (clamped to 1-20 ms, page limit 32, 2 matches per file) and `crates/smcli-service/src/test_candidates.rs:111` (100 ms, 200 matches). `grep_terms` (`evidence.rs:102-135`) returns up to 16 file stems and identifiers, not lines with context. Turning hits into "a runnable command plus surrounding text" needs a new `ProjectEvidence` method. The trait's default is a no-op (`crates/smcli-search/src/evidence.rs:27`), which is why the fallback gets nothing.
- **Tab already has its own request.** Tab sets `semantic_pending` (`crates/smcli/src/picker.rs:752-754`), and the search thread sends `Operation::Search` with `limit=100` (`crates/smcli/src/cli.rs:1056`). Typing without Tab filters locally. So no protocol change is needed. But `smcli search` on the command line also sends Search (`cli.rs:293`). If it should not pay for re-scoring, the optional `ServiceRequest.mode` field (`crates/smcli-core/src/service_protocol.rs:100`) can tell them apart without a version bump.
- **Q2 has a false lead.** "open" prefix-matches "openai" at `corpus.rs:424`, which lifts every `openai/*` command, including `oc mini`. "6" cannot match at all because of the 2-character minimum. Q2 is a ranking problem: the target is already in the pool at rank 11.

### Problems with the Tab re-scoring pass

- **The final score is undefined.** The plan does not say whether the new cosine replaces the existing rerank (BM25, frecency, run-here, Script/Hint boosts) or feeds into it, or with what weights. Without that, Stage 2 cannot be measured, and two scorers would fight.
- **Most of its gain is already covered.** Makefile and justfile come from `context.rs`. What is new is package.json scripts, CI files, README code blocks and `scripts/`. README and CI commands were never run: often wrong for this machine, sometimes dangerous (`rm`, deploy). That reverses the round-one non-goal on never-run commands.
- **Cost has no ceiling.** The cap is on gathering only. Embedding thousands of texts on the service thread could block other clients, and the fallback may load the model cold. Pool size needs a hard cap after dedup, and typing again should cancel.
- **Re-embedding history contradicts a rejected option.** "Embed the whole pool" re-embeds history commands whose vectors are already stored. Only the new context text (where and how often used) needs fresh vectors. Mixing fresh and stored vectors is safe only if they come from the same model and text format, which is the point of the plan's "one consistent pass" and should be stated that way.
- **Undefined terms.** "Surrounding text" (how many lines?), "context text" for history (cwd, count, exit code?), how a hit maps to a command per file type, and how grep hits that duplicate history commands are merged.

### Problems with the coverage fix

- **Edit-distance-1 matches the wrong words.** "open" to "oven", "with" to "wish", "test" to "text". Accept a typo match only when no exact or prefix match exists, and weight it below an exact match.
- **Fractional coverage needs a floor.** Without a threshold, every Script candidate with one matching word enters the pool. Relevance for partial matches must stay below 1.0.
- **The change spreads.** `entry_coverage` (`corpus.rs:413`) shares the code, so rerank coverage changes too, not only the gate.

### Observability

Neither the `why` strings nor any debug output give per-signal numbers or per-source pool counts. Every reviewer who touched tuning asked for them first.

## Goals vs. Non-Goals

**Goals**

- Q1 and Q2 rank their target in the top 3 through the service.
- No existing case in `crates/smcli-search/tests/ranking.rs` gets worse, including `rank_alias_eval` and `rank_record_time_aliases`.
- Context raises rank and never removes a result.
- No protocol version change. VERSION stays 3 and the frozen fixtures stay unchanged. An optional `mode` value is allowed.
- Shared search code gets the fixes where possible, so the fallback benefits from everything that does not need fff.
- A measured p95 latency budget for the Tab path before any per-query embedding ships.

**Non-Goals**

- A bigger model, an LLM reranker, or retraining.
- Typo tolerance in the index tokenizer (forces a reindex).
- Storing per-query context embeddings.
- Picker UI changes, apart from an optional debug score view.
- Injecting never-run commands from README or CI files, unless Stage 3 data justifies it as a separate decision.
- Rank parity between the service and the fallback.

## Alternatives Weighed

**Retrieval (getting the target into the pool)**

| Option                                                                                                                         | Assessment                                                                                                                                                  |
| ------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Loosen the `additions` gate: stopwords via `is_stopword`, typo match as last resort, relevance = fraction covered with a floor | Smallest change, shared by both paths, and Q1's target is already a Script candidate. Recommended.                                                          |
| Typo tolerance in the BM25 tokenizer                                                                                           | Forces a reindex. Rejected.                                                                                                                                 |
| Wider `k` on the existing legs (`index.rs:188`)                                                                                | Cheap if recall is the problem. Try before a new pool.                                                                                                      |
| fff grep over Makefile and justfile                                                                                            | Duplicates `context.rs:22` discovery. Rejected.                                                                                                             |
| fff grep over package.json, `scripts/`, CI, README code blocks                                                                 | New sources, but needs a new trait method, a larger budget than 20 ms, and brings never-run commands. Deferred to Stage 3, starting with package.json only. |
| New cwd-frequent injection path                                                                                                | Duplicates `additions` and adds noise to every query. Rejected.                                                                                             |

**Ranking (ordering inside the pool)**

| Option                                                                                                                                          | Assessment                                                                                                       |
| ----------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| Head-command boost: a query word equals the program as a whole word (not `pip` for `pi`), placed in `boost` so `boost^relevance` still damps it | Aimed at Q2. Recommended first. Needs a definition for wrappers (`sudo`, `env X=1`) and paths.                   |
| Stop "open" prefix-matching "openai" (stopword or longer prefix minimum)                                                                        | Removes Q2's false lead. Check with the ranking set.                                                             |
| Tab-only in-memory re-scoring of a wide pool                                                                                                    | Plausible once a final-score formula, pool cap and latency budget exist. Deferred until Stages 1-2 are measured. |
| Broad weight rebalance                                                                                                                          | Overfits. Deferred.                                                                                              |
| Lower frecency and run-here so bare `pi` drops                                                                                                  | Likely hurts other queries. Rejected.                                                                            |

**Rejected evidence sources and approaches (user positions, confirmed)**

| Option                             | Reason                                                                                                                                       |
| ---------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| Running `prog --help`              | Executes arbitrary binaries and is slow.                                                                                                     |
| Man pages via fff                  | fff does not search the man database. Low value for the user's own tools. Short `whatis` descriptions at index time may be considered later. |
| Re-embedding all history per query | No gain: vectors are already stored. Applies equally to history commands inside the Tab pool.                                                |
| A bigger model                     | User constraint.                                                                                                                             |

## Key Decisions & Staging for Next Steps

**Decisions to confirm**

1. Fix `coverage` and the `additions` gate first. Q1's target is already a Script candidate, so this is the direct fix.
2. Typo tolerance stays query-side, as a fallback after exact and prefix matches, weighted lower.
3. Tab re-scoring waits until Stages 1-2 are measured. If built, it must define the final score in one place and treat sources as inputs to one pure function of (query, pool).
4. Decide whether `smcli search` gets the Tab path. If not, mark picker requests with `mode`.
5. fff-backed gains are service-only. The fallback still gets coverage, the head boost and per-query embedding.
6. Weights live as named constants in one place. No rules written for specific queries.

**Stages** (each ships and is measured on its own)

- **Stage 0: baseline and observability.**
  - Add Q1, Q2 and about 20 more natural-language queries to `ranking.rs`, using its synthetic layout and `Potion::bundled()` (`ranking.rs:191`). Report pool recall (the BM25 + embedding + additions pool before any re-scoring) separately from rank.
  - Add per-signal scores and per-source pool counts to debug output.
  - Record MRR, top-1, top-3, and p50/p95 latency on both paths. Time `Potion::embed` on 1k and 5k texts.
  - Exit: numbers recorded.
- **Stage 1: retrieval.** Stopwords, last-resort typo match, fractional relevance with a floor. Exit: Q1 in the pool, all groups flat.
- **Stage 2: ranking.** Head-command boost, fix the "open"/"openai" lead, inspect Q2's per-signal scores. Exit: Q1 and Q2 in the top 3, no regressions.
- **Stage 3, only if Stage 2 misses: Tab re-scoring.**
  - Prerequisites: final-score formula, pool cap (named constant, about 1k after dedup), embed and gather time budgets, cancel on new input, command templates per source.
  - Reuse stored history vectors. Embed only new context text and package.json scripts first. Add CI and README only with data.
  - Exit: target metrics improve and p95 stays within the Stage 0 budget.

Gates: `make ranking`, `make test-rust PKG=smcli-search`, plus a latency assertion for Stage 3. Protocol fixtures stay unchanged.

**Risks**

1. **The wide pool mostly duplicates what exists and adds unrun commands.** Makefile and justfile targets already arrive through `context.rs`. The fff budget is 20 ms and returns words. README and CI hits bring commands the user never ran.
2. **Two scorers.** Without a defined final score, re-scoring and the existing rerank will conflict and regressions will be hard to trace.
3. **Wrong matches and unbounded cost.** Typo tolerance and the existing "open" to "openai" prefix match create false leads. Per-Tab embedding with no pool cap could block the service thread, and `smcli search` would pay the same cost.
4. **Overfitting.** Without the wider set and recall reported apart from rank, tuning can hide regressions.

## Reproduction (2026-11-01, `smcli search`, cwd = smcli repo, installed build)

| Query                         | Wanted                                         | Result                                                                                               |
|:------------------------------|:-----------------------------------------------|:-----------------------------------------------------------------------------------------------------|
| build and insall locally      | `make install`                                 | absent from top 8; all literal "build"                                                               |
| build and install locally     | `make install`                                 | rank 3, below 1-run `ll swift-pass.build/` with no evidence, despite "used here 4x, Makefile target" |
| install smcli on this machine | `make install`                                 | absent; long pasted `SMCLI_NEW=...` scripts win                                                      |
| run the tests                 | `make test`                                    | absent; `deno run` / `bun run` win on the word "run"                                                 |
| format the docs               | `make docs`                                    | absent; `docs`, `la docs`, `mv docs` win on the word "docs"                                          |
| rebuild a clean release       | `make fresh`                                   | absent; `swift build -c release` (stale tool) first                                                  |
| open pi with gpt 6            | `pi --model openai/gpt-6.1-sol --thinking low` | absent from top 8; `oc`/`codex` with `gpt-6.1-sol` win                                               |
| start pi with sol             | same                                           | `pi` subcommands fill top 8; wanted absent                                                           |
| commit and push               | `git commit`/`git push`                        | reasonable                                                                                           |

Findings:

1. The typo alone removes `make install` from the pool (question 1 vs 2).
2. Even when retrieved, context evidence ("Makefile target", "used here", run count) loses to a one-word surface match with no evidence. Evidence weights are too low relative to the match score.
3. Makefile targets in the cwd (`test`, `docs`, `fresh`, `install`) are not offered as candidates when the query word differs from the target name.
4. Short commands that contain the query word (`docs`, `build`, `run`) score as "semantic match": the embedding of a short command is close to any query sharing its word, so the embedding half behaves lexically on short commands.
5. For pi, the program name matches but the full command loses to sibling subcommands and to other programs that share `gpt-6.1-sol`; the flags/model value are not weighted as part of the intent.
