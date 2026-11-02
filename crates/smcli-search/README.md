# smcli-search

## Overview

Hybrid search over one user's shell history: captured SQLite statistics and raw native history merge into one corpus of distinct commands, retrieved through a TalaDB index (BM25 over shell terms plus a static embedding) and reranked by usage and request-scoped project evidence. Library only — `smcli` and `smcli-service` own the processes. `smcli-core` supplies the entry/filter/ranking types; `fff` is never linked.

## Interface

- `src`: [./src/README.md](src/README.md) — every production module (corpus merge and reranking, hybrid index retrieval, tokenizer, native history decoding, embedding model).
- `Cargo.toml`: package `smcli-search` (edition 2024). `smcli-core` supplies entry/filter/ranking types, `taladb` the command index (`src/index.rs`), `model2vec-rs` the static model (`src/potion.rs`), `libc` the filesystem and lock probes (`src/filesystem.rs`, `src/index.rs`).

## Invariants

- Native shell history is input only: `crates/smcli-search/src/native.rs:HistoryFile::read` decodes in memory and nothing in the crate writes those paths.
- The index is a derived, rebuildable cache, not a source of truth. `crates/smcli-search/src/index.rs:HistorySearch::index_name` pins document format, model name, model identity and vector dimension, so a model switch cannot silently reuse old vectors.
- The index holds command text, so it gets the database's privacy: a private directory owned by the current user, mode `0700` (`crates/smcli-search/src/index.rs:HistorySearch::prepare_directory`).
- Ranked order comes from `crates/smcli-search/src/corpus.rs:rerank_with_evidence` except for an empty query with a nonempty filter: that listing bypasses index sync and reranking and sorts by last run, runs, then command. The text path used when the index lock is held reuses the same marker parsing and literal/intent classification, but it matches words instead of embedding, so its candidate set and order are not guaranteed identical to the indexed path.
- Extra evidence may raise a score. The one demotion is the success-rate multiplier on captured commands, and failures are demoted rather than removed (`rerank_with_evidence`).

## Side Effects

- Index directory `<history-db-path>.search/`: the `.taladb` file and engine sidecars, plus a `lock` file (`crates/smcli-search/src/index.rs:HistorySearch::files`, `lock_index`); stale `rs-commands-v*` generations are deleted on open (`remove_old_indexes`).
- Reads: native history files (`crates/smcli-search/src/native.rs:history_files`), the model directory (`crates/smcli-search/src/potion.rs:Potion::bundled_dir`, `Potion::from_dir`), the history database and config (`crates/smcli-search/src/setup.rs:corpus`, `search_signals`, `search_engine`), and PATH plus candidate input paths (`crates/smcli-search/src/filesystem.rs:available_in`, `present_in`).
- No network access — embeddings are computed locally — and no subprocesses. Filesystem probes run on one named worker thread `ranking-fs`, serialized by a process-wide gate (`crates/smcli-search/src/filesystem.rs:GATE`).
- An advisory `flock` on `<index dir>/lock` serializes index writers; a contender falls back to text-only search (`crates/smcli-search/src/index.rs:HistorySearch::lock_index`).

## Verification

- Run: `make test-rust PKG=smcli-search`
- Run: `make ranking` (scored, floor-checked query set)

## Ranking

Natural-language intent uses vector-only top-k retrieval. Unambiguous query-side spelling corrections are applied before embedding only to words absent from the loaded model’s whole-word tokenizer vocabulary (paths without a loaded model retain their existing behavior); the original corpus/model/index format is unchanged. Current project Hint/Script/Test candidates are batch-embedded at query time and join with the same cosine. BM25 fusion, grep expansion and the old usage exponent are not used for intent. Only explicit filters and resolved markers constrain this pool.

Intent score is cosine plus bounded additive context: runs here (0.01 × log1p, cap 0.04), project runs (0.01 × log1p, cap 0.02), frecency (0.005 × log1p, cap 0.03), exact parsed executable (0.02), lexical coverage (0.005), project candidate membership (0.03), PATH (0.03) and existing input files (0.01). The combined context cap is 0.10. Project-config keyword evidence adds a separate boost capped at 0.48, so the combined positive contributions cannot reverse a cosine gap greater than 0.58. Signal-disable behavior is preserved. With a held index lock, intent embeds the filtered in-memory corpus instead of using text-only coverage; this may cost more than indexed retrieval.

Literal queries retain the existing hybrid/usage pipeline, including strict filters, alias behavior and fallback. Exact typed alias invocations retain their alias-aware legacy path; standalone uppercase identifier queries remain lexical. Native history and stored index documents are unchanged.

`make ranking` prints the fixed natural-language cohorts and legacy cases. `make test-rust PKG=smcli-search FILTER="--release vector_first_comparison -- --nocapture"` prints the synthetic old/new comparison and asserts no literal-rank regressions. Natural-language trade-offs are reported separately; the existing suggestion and literal-search floors remain enforced. Timing and pool-tracing instrumentation are not part of this harness.

Project Script candidates also carry up to 400 characters of local config description text: contiguous Make/just comments and recipes, package.json command strings, or Swift target/product names and kinds. Nothing is executed. Corrected query tokens match these descriptions case-insensitively (prefixes need four characters), with stopwords removed and each match weighted by rarity across that file's candidates. Shared boilerplate contributes zero. Candidates already enter independently of the stored-vector pool cutoff, so the keyword signal needs no BM25 retrieval or unbounded relevance promotion.

`make test-rust PKG=smcli-search FILTER="--release project_config_comparison -- --nocapture"` compares this bounded keyword signal with vector-first alone and asserts no worsened fixed-search or NL ranks. Embedding command+detail was tested and rejected after a held-out regression; stored vectors and query-time command embeddings are unchanged.
