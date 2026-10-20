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

Use the Makefile for builds; don't hand-roll `swift build` commands.
- `make fresh` — clean release build for the user to test. Output: `dist/smcli-<timestamp>-<commit>`, with `dist/smcli` symlinked to the latest. It also prints the `eval` line that loads the zsh hooks from the new build; relay those commands to the user. Run this after changes the user will try out.
- `make release` — same, without cleaning (faster).
- `make build` — debug build; `make run ARGS="..."` builds and runs.
- `make test` — full test suite (`scripts/test.sh`): `swift test`, then black-box CLI checks against a temp `HOME`, `SMCLI_DB` and `SMCLI_RUNTIME_DIR`.
- Single test: `make fff` once, then `swift test --filter <TestClass>[/<testMethod>]`.
- `make ranking` — scores suggest/search ranking on the fixed query set (`RankingQualityTests`); run after ranking changes.
- `make fff` builds the pinned Rust `fff` static library (`scripts/build-fff.sh`) that `smcli-service` links; every build target depends on it (needs Rust).
- `make clean` — remove build output.

## Architecture

Swift package, macOS 14+ (Linux builds without search). Two executables, both embedding the potion-code-16M-v2 model via linker sections (`Package.swift`), so each can search alone:
- `smcli` (`Sources/smcli`) — CLI, interactive picker/menu, `shell-init` hook generation (`ShellIntegration.swift`).
- `smcli-service` (`Sources/smcli-service`) — long-lived owner of history state and the open search index, serving clients over a Unix-domain socket. Only it links `SmcliFFF` (file search via fff); the CLI and `SmcliCore` must never link fff.

Libraries: `SmcliCore` (SQLite captured history at `~/.local/share/smcli/history.db`, capture filters, ranking/suggestions, config, socket client and protocol), `SmcliSearch` (hybrid BM25 + embedding search over captured and native history; Potion, Apple embeddings or Ollama).

Client/service protocol (`SmcliCore/ServiceProtocol.swift`): one newline-delimited JSON request and one response per connection, versioned. On a version mismatch or no service, the CLI falls back to doing the work in-process, so every operation must work both ways. The service identifies its binary by path/mtime/size so a rebuilt binary is detected.

Native shell history files are read-only: never rewrite them. smcli keeps its own SQLite log.
