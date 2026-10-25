# Design

## Context

Verified baseline (details in the research notes):

- The corpus merges captured and native history (`smcli-search/src/corpus.rs:14-81`). Embedding text is capped at 1,500 characters (`corpus.rs:141-177`), and lexical terms are built from that same capped text (`index.rs:252-300`).
- The tokenizer (`tokenizer.rs`) keeps whole shell words plus their alphanumeric pieces. Lowercasing happens early, so camelCase boundaries are lost.
- Literal queries fuse scores 70/30 lexical/dense on top-score-normalized values. Intent queries use TalaDB `HybridQuery` defaults. Retrieval fetches `max(4*limit, 40)` candidates and scans the vector index exactly.
- The reranker (`corpus.rs:414-493`) already scores habit, cwd/project, recency, picks, transitions and failures. `smcli-core/src/context.rs` already detects manifests and scripts.
- The CLI links `smcli-search` and falls back to in-process search (`cli.rs:258-275`). Only fff is service-only (`smcli-service/src/file_search.rs`). fff's `index()` may block up to ~20s on a cold root.
- Hooks send history, cwd and session metadata today. They send no alias or PATH data. `ServiceRequest.path` already means a selected file path.

## Goals / Non-Goals

**Goals:**

- Make ranking changes measurable before tuning anything.
- Fix the long-command recall gap and add cheap lexical precision.
- Answer the alias, PATH and fff questions with evidence, then ship only what earns its place.

**Non-Goals:**

- No new model, neural reranker, chunking, RRF for literal queries, or test/docs path penalties. These came from semble and were judged a poor fit.
- No reading of rc files, no alias-table snapshots, no persisted PATH. The only schema change is schema 8's `expanded_command` column (see decision 7).
- No fff in ranking (spike NO-GO).
- No user-facing tuning knobs.

## Decisions

1. **Benchmark first (P0).** Add cohorts to `tests/ranking.rs`: the original cases, adversarial cases (flags, revisions, rare vs frequent, quoting, long tails, camelCase) and a frozen held-out set. Report each cohort separately. Alternative considered: tune against the existing 16 cases. Rejected, because that would overfit.
2. **Separate lexical text from embedding text.** Lexical terms come from the full command plus intent. The embedding keeps its 1,500-character cap. Bump the index `VERSION`. If capture limits allow very large commands, a separate, documented lexical ceiling applies.
3. **Exact-token bonus is conditional.** Add it to `coverage`/`rerank` only if P0 cases fail. Parse with the existing `shell_words` usage. It is a bounded preference, never a filter.
4. **Identifier parts.** Keep the original-case token long enough to split camel/Pascal parts. Emit the parts in addition to the full token, and reuse the same split in coverage so retrieval and rerank agree.
5. **Signals investigated as spikes with written go/no-go decisions.** Each spike writes `openspec/changes/improve-search-accuracy/investigations/<signal>.md` covering method, benchmark deltas with ablation, latency, privacy review and the decision. Implementation tasks for a signal run only after a go decision.
   - **Aliases:** capture a bounded snapshot from the live shell (zsh `alias -L`/`${aliases}`, bash `alias -p`, fish `abbr`/`alias`). Filter to simple expansions in the shell before sending. Send on session start and on change, never every prompt. Keep it in service memory per session, and have the CLI fallback read it from the request. Add a new protocol field with a distinct name, and add fixtures for old and new versions. The query is expanded as an additional query form. Documents are not re-embedded.
   - **PATH:** a positive-only availability boost computed at query time from a snapshot named `shell_path`. Never use the service's own PATH. Builtins, functions, wrappers and unknown programs are neutral.
   - **File presence:** start with in-process `stat` of path-like arguments for a small set of commands that read files, relative to the current cwd and the bounded candidate pool. Never expand shell syntax. Then measure whether warm-fff evidence (a non-blocking, non-building lookup) adds anything beyond `stat`.
6. **Spike outcomes.** Query-time aliases NO-GO (`investigations/aliases.md`). Record-time alias expansion approved by the user (`investigations/aliases-record-time.md`): nullable `commands.expanded_command` (schema 8), the expansion as primary search text, `typed → expansion` display, the expansion inserted on pick, usage counts not merged, and filters applied to both texts on both record paths. PATH GO: positive-only +0.1 from the client's `shellPath`. stat GO, fff NO-GO (`investigations/files.md`).
7. **Ablation names.** Each new signal joins `SMCLI_RANKING_OFF` (`alias`, `path`, `files`).

## Lexical ceiling

Captured commands are stored as SQLite `TEXT`, which has no practical length limit. To keep the keyword index bounded while still indexing typical pasted scripts and heredocs, lexical terms are capped at **8,192 characters**. The embedding text stays capped at 1,500 characters. The identity (`doc`) and the history database still keep the full command verbatim.

## Risks / Trade-offs

- [Benchmark overfit] → frozen held-out cohort, with per-cohort floors.
- [Larger lexical index] → measure sync time and size on synthetic long entries. Add a ceiling if needed.
- [Index rebuild after VERSION bump] → one-time cost. Init already reports rebuilds.
- [Alias capture leaking secrets] → filter in the shell with an allowlist. Snapshots stay in memory. `why` never shows expansion bodies.
- [Typing-path latency from stat/fff] → bounded candidate count, no waits, unknown treated as neutral. Measure p50/p95.
- [Protocol drift] → version bump plus frozen fixtures. An old service without the new fields falls back.

## Migration Plan

The index format bump triggers a rebuild on the next search. Rollback means reverting the commit: the old binary sees a newer index version and rebuilds. Schema 8 is not backward compatible: a schema 7 binary refuses a migrated history.db, so take a backup before migrating and document a restore path. Protocol fields are additive and versioned, so an older CLI falls back to in-process search.

## Open Questions

- What size ceiling should the lexical text have, if any? Depends on capture limits. Answered in task 2.1: **8,192 characters**.
- Does the pinned fff expose a non-blocking exact-path lookup? Answered in the fff spike. If not, the spike decides between `stat` alone and no-go.
