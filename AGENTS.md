# AGENTS.md

This file provides guidance to coding agents when working with code in this repository.

## graphify

This project has a knowledge graph at graphify-out/ with god nodes, community structure, and cross-file relationships.

Rules:
- When graphify-out/graph.json exists, run `graphify query "<question>"` before your first grep, search or source-file read for a codebase question, and put this rule in every subagent prompt that explores code. Read raw files only for lines the graph or an earlier read already located. Use `graphify path "<A>" "<B>"` for relationships and `graphify explain "<concept>"` for focused concepts. These return a scoped subgraph, usually much smaller than GRAPH_REPORT.md or raw grep output.
- If graphify-out/wiki/index.md exists, use it for broad navigation instead of raw source browsing.
- Read graphify-out/GRAPH_REPORT.md only for broad architecture review or when query/path/explain do not surface enough context.
- After modifying code, run `graphify update .` to keep the graph current (AST-only, no API cost).
- A committed graphify-out/graph.json is rewritten whole on every build, so a large diff to it in a pull request is expected. Do not flag it as churn or suggest skipping the graph rebuild or its hooks.

## Building

Use the Makefile for builds; don't hand-roll `cargo build` commands.
- `make fresh` — clean release build for the user to test. Output: `dist/smcli-<timestamp>-<commit>`, with `dist/smcli` symlinked to the latest. It also prints the `eval` line that loads the zsh hooks from the new build; relay those commands to the user. Run this after changes the user will try out.
- `make release` — same, without cleaning (faster). `make build` — debug build; `make run ARGS="..."` builds and runs the CLI.
- `make test` — full test suite (`scripts/test.sh`): `cargo test --workspace`, then black-box CLI checks against a temp `HOME`, `SMCLI_DB` and `SMCLI_RUNTIME_DIR`.
- Single test: `cargo test -p <crate> <test name>`.
- `make ranking` — scores suggest/search ranking on the fixed query set (`crates/smcli-search/tests/ranking.rs`); run after ranking changes.
- `make linux` — full test suite on Linux in Docker (needs a running daemon).
- `make baseline` — black-box CLI/hook benchmark (`scripts/bench-baseline.py`, see `docs/baseline.md`); never reads real history.
- `make clean` — `cargo clean`, keeping the newest `dist/` CLI and service pair (live shell hooks may point at them).

## Architecture

Cargo workspace (`crates/`), macOS and Linux, search included on both. Two executables, both embedding the potion-code-16M-v2 model (`include_bytes!`, written to the runtime dir on first use), so each can search alone:
- `smcli` (`crates/smcli`) — clap CLI, ratatui picker and menu, `shell-init` hook generation (`shell_integration.rs`).
- `smcli-service` (`crates/smcli-service`) — long-lived owner of history state and the open search index, serving clients over a Unix-domain socket. Only it depends on `fff-search` (file search); the CLI and `smcli-core` must never link fff.

Libraries: `smcli-core` (SQLite captured history at `~/.local/share/smcli/history.db`, capture filters, ranking/suggestions, config, socket client and protocol), `smcli-search` (hybrid BM25 + embedding search over captured and native history in a TalaDB index; Potion only).

Client/service protocol (`smcli-core/src/service_protocol.rs`, frozen fixtures in `crates/smcli-core/tests/fixtures/service_protocol`): one newline-delimited JSON request and one response per connection, versioned. On a version mismatch or no service, the CLI falls back to doing the work in-process, so every operation must work both ways. The service identifies its binary by path/mtime/size so a rebuilt binary is detected.

Native shell history files are read-only: never rewrite them. smcli keeps its own SQLite log.
