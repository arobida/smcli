# smcli-search

## Overview

Hybrid search over a user’s shell history: captured SQLite statistics and raw native history are merged, retrieved through a TalaDB index, then reranked by usage and request-scoped project evidence. The library also provides offline tldr-page loading, ranking, placeholder conversion, and a persistent command-example index for natural-language command help. `smcli` and `smcli-service` own the processes; `fff` is never linked.

## Interface

- `src`: [./src/README.md](src/README.md) — production modules, including history corpus/index and Potion embedding.
- `tldr` module (`src/tldr.rs`): `bundled_dir` locates bundled pages; `load` selects common and current-platform entries; `to_placeholders` converts tldr template fields to smcli placeholders; `installed` filters programs against PATH; `rank` and `rank_with_vectors` order entries by embedding similarity, preserving corpus order for ties and applying a small command-prefix bonus. `TldrSource` implements the shared `CommandSource` contract.
- `cheat` module (`src/cheat.rs`): `CheatSource` loads bundled `cheat/cheat.jsonl` (generated offline from a pinned cheat/cheatsheets commit by `scripts/gen-cheat.py`, CC0). It is the fallback source: a program's examples come only from the highest-priority source that has any, tldr first, cheat second.
- `commands::bundled_sources` builds the ordered source list (tldr required, cheat optional) used by both the CLI and the service, so both agree on the sources and the index name.
- `Cargo.toml`: package `smcli-search` (edition 2024). `smcli-core` supplies shared types; `taladb` the history index; `model2vec-rs` the static Potion model.

## tldr corpus and command index

The bundled `tldr/tldr.jsonl` is read-only data. Its first JSONL record provides the data version and source commit; subsequent records contain program, platform, description, and command template. The loader retains `common` entries plus the requested platform. No network access or command execution is involved.

`commands::CommandIndex` stores examples and embeddings in a TalaDB file beside the history index, named by model identity, source versions and dimension. smcli selects OS-shipped and history-seen programs that resolve on PATH; the bundled sources supply examples for those programs, each program from the first source in priority order that has any. Install/reindex builds the index, and the service syncs it after captures when an unknown program or changed PATH requires it. `@@` reads the index and asks the service to prepare it when missing; without the service it uses word ranking on existing examples. Old `tldr-*.vec` files are legacy data removed by `smcli indexes --prune`.

See the [command-completion specification](../../openspec/specs/command-completion-index/spec.md) for the program-selection contract.

## Invariants

- Native shell history is input only; the crate never writes those paths.
- Search indexes and tldr vectors are derived, rebuildable caches.
- History ranking behavior and its filters remain separate from tldr’s vector-only ranking.
- Model embeddings are computed locally; there is no network access or subprocess execution.

## Side Effects

- History and command-example index files are read/written under `<history-db-path>.search/`.
- Reads native history files, model files, the history database/config, PATH, and candidate input paths.
- No subprocesses or network I/O.

## Verification

- `make test-rust PKG=smcli-search`
- `make ranking`
