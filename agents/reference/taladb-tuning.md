# TalaDB tuning

Research from 2026-11-10 (TalaDB 0.12.0 source, DeepWiki). Decision: keep TalaDB; a Grafeo replacement was weighed and dropped.

## How history search uses TalaDB

`crates/smcli-search/src/index.rs` routes each query one of three ways:

| Query                                                | Example            | TalaDB call                         | Parameters                                                |
| ---------------------------------------------------- | ------------------ | ----------------------------------- | --------------------------------------------------------- |
| Natural language (usual)                             | `undo last commit` | `find_nearest`, exact cosine scan   | `k = max(limit * 4, 40)`, filter from markers             |
| Literal (`-flag`, `/`, `=`, `~`, `HEAD`, `file.ext`) | `git reset --soft` | `search_text_with` + `find_nearest` | BM25 defaults, then 0.7 keyword / 0.3 vector weighted sum |
| Exact typed alias                                    | `gst`              | `hybrid_search`                     | RRF defaults                                              |

Natural-language search is vector-only, so BM25 and RRF settings do not affect it. Its levers are `BRIDGE_MATCHES` (3) and `BRIDGE_FLOOR` (0.60) in `index.rs`, the `INTENT_*` constants in `corpus.rs` (`rerank_intent_with_affinity`), and the model. The picker's service-down fallback uses `lexical_search` (limit 100) and touches neither the index nor the model.

## Query-time knobs (no reindex)

- `Bm25Params { k1: 1.2, b: 0.75, stopwords: true }`: pass instead of `&Default::default()` to `search_text_with`, or set `hybrid.bm25`. Try lower `b` (0.3, 0.5) for short commands. `stopwords` does nothing: terms are hex-encoded by `ShellTokenizer::encoded`.
- `RrfParams { k: 60.0, text_weight: 1.0, vector_weight: 1.0 }` on `hybrid.rrf`; `hybrid.candidates` defaults to `max(top_k * 4, 20)`.
- `search_vectors(field, query, k, filter, &VectorQueryOptions)` in place of `find_nearest`: `group_by`/`group_size` (cap hits per program), `score_threshold`, `offset`, `oversampling`, and execution stats.
- Score each change with `make ranking` and `make tldr-ranking` (kteam runs tests).

## Unused TalaDB features

- Encryption (`encryption` feature): `open_encrypted`, `with_field_encryption`.
- `create_compound_index`, `drop_*_index`.
- `create_vector_index_with_options`: HNSW (`m` 32, `ef_construction` 200), quantization, staged builds, `upgrade_vector_index`, `rebuild_hnsw_indexes`. The index is deliberately exact scan (faster below tens of thousands of commands).
- Vector cache: `set_vector_cache_budget`, `set_vector_cache_adaptive`, `notify_memory_pressure`, `vector_cache_stats`.
- `set_durability`, `compact`, `watch`, snapshots, `open_with_migrations`/`user_version`, audit log.

## Limits

- One process holds the file exclusively; no read-only open was found.
- No custom tokenizer: text is lowercased, split on non-alphanumerics, one-character tokens dropped. Hence the hex encoding.
- Version 0.13.0 is out (smcli pins 0.12); its changes are unreviewed.
