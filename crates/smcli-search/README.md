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
