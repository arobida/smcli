# Rust workspace

## Overview

The workspace manifest (`../Cargo.toml:workspace.members`) includes the four crates below. The CLI and service share core history/protocol types and the search library; only the service links fff-search.

## Interface

- [smcli](smcli/README.md) — user-facing executable, picker/menu, shell hooks and local fallbacks (`smcli/Cargo.toml`).
- [smcli-core](smcli-core/README.md) — captured history, config, parsing, ranking and socket protocol (`smcli-core/src/lib.rs`).
- [smcli-search](smcli-search/README.md) — corpus, hybrid history search and local Potion embeddings (`smcli-search/src/lib.rs`).
- [smcli-service](smcli-service/README.md) — persistent history/search state and fff file/content search (`smcli-service/src/main.rs`, `smcli-service/Cargo.toml`).

## Invariants

- Keep fff-search out of CLI/core dependencies; its owner is the service (`smcli-service/Cargo.toml`).
- Native history stays read-only (`smcli-search/src/native.rs:HistoryFile::read`); captured history is separate (`smcli-core/src/history.rs:CapturedHistory::check_separate`).
- Transport uses versioned newline-delimited JSON; dispatch checks the version, not the codec (`smcli-core/src/service_protocol.rs:VERSION`, `encode_line`, `decode_line`; `smcli-service/src/service.rs:Service::dispatch`).

## Side Effects

- Captured-history/config writes, migrations and service launch (`smcli-core/src/history.rs:CapturedHistory`, `config.rs:SmcliConfig::save`, `service_client.rs:ServiceClient`).
- Rebuildable search-index writes and local model reads (`smcli-search/src/index.rs:HistorySearch`, `potion.rs:Potion`).
- Terminal state changes and printed shell hooks (`smcli/src/picker.rs:TerminalSession`, `shell_integration.rs:render_config`); service sockets, fff watchers and bounded index pools (`smcli-service/src/service.rs:Service::run`, `file_search.rs:FileSearch`).

## Verification

Run `make test` from the repository root. Use `make test-rust PKG=<crate>` for a focused crate gate; see child contracts for black-box slices.
