# smcli-search

## Overview

Hybrid search over a user’s shell history: captured SQLite statistics and raw native history are merged, retrieved through a TalaDB index, then reranked by usage and request-scoped project evidence. The library also provides offline tldr-page loading, ranking, placeholder conversion, and a cached vector corpus for natural-language command help. `smcli` and `smcli-service` own the processes; `fff` is never linked.

## Interface

- `src`: [./src/README.md](src/README.md) — production modules, including history corpus/index and Potion embedding.
- `tldr` module (`src/tldr.rs`): `bundled_dir` locates bundled pages; `load` selects common and current-platform entries; `to_placeholders` converts tldr template fields to smcli placeholders; `installed` filters programs against PATH; `rank` and `rank_with_vectors` order entries by embedding similarity, preserving corpus order for ties and applying a small command-prefix bonus; `corpus_vectors` loads or builds vectors for installed programs.
- `Cargo.toml`: package `smcli-search` (edition 2024). `smcli-core` supplies shared types; `taladb` the history index; `model2vec-rs` the static Potion model.

## tldr corpus and cache

The bundled `tldr/tldr.jsonl` is read-only data. Its first JSONL record provides the data version and source commit; subsequent records contain program, platform, description, and command template. The loader retains `common` entries plus the requested platform. No network access or command execution is involved.

The vector cache is stored in the history search-index directory as `tldr-<commit>-<Potion::NAME>-<identity>-d<dimension>.vec`. A JSON header line lists cached programs, followed by little-endian `f32` vectors in corpus order within each program. Missing installed-program pages are embedded and appended; writes use a private temporary file followed by rename. Commit and model identity/dimension in the name prevent reuse across data or model changes. This is rebuildable cache data, not a source of truth.

## Invariants

- Native shell history is input only; the crate never writes those paths.
- Search indexes and tldr vectors are derived, rebuildable caches.
- History ranking behavior and its filters remain separate from tldr’s vector-only ranking.
- Model embeddings are computed locally; there is no network access or subprocess execution.

## Side Effects

- History index and tldr cache files are read/written under `<history-db-path>.search/`.
- Reads native history files, model files, the history database/config, PATH, and candidate input paths.
- No subprocesses or network I/O.

## Verification

- `make test-rust PKG=smcli-search`
- `make ranking`
