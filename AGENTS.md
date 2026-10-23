# AGENTS.md

This file provides guidance to coding agents when working with code in this repository.

<-- Start of do not change this -->

## Agent Team

By using the kero skill you are allowed to open new sessions with the list of agents I will provide. Whenever you are asked you should efficiently delegate task to the appropriate agent and act as the orchestrator and let them handle the tasks. The list will show two things first an alias followed by the command needed to open the agent in the new sesion.

### Agents List:

pisol=pi --model openai/gpt-6.1-sol --thinking low
piluna=pi --model openai/gpt-6-luna --thinking low
pikcode=pi --model opencode-go/kimi-k2.7-code --thinking high
pidpro=pi --model opencode-go/deepseek-v4-pro --thinking max
ocflash-free=oc mini -m opencode/deepseek-v4.1-flash-free
pigrok47=pi --model xai/grok-4.7 --thinking high
pigrok46=pi --model xai/grok-4.6 --thinking high
pigrok45=pi --model xai/grok-4.5 --thinking medium

#### Agent Affinities

> Some agents are better at certain tasks than others(try to use free models as much as possible)
> pikcode: great at writing code
> pisol: complicated reasoning tasks
> piluna: fast general purpose worker
> pidpro: good general purpose model, very token efficient with prompt caching, and uses max reasoning
> ocflash: a fast a free vision model for general purpose tasks
> pigrok47: complicated reasoning tasks(not as good as pisol!)
> pigrok46: general purpose model faster than pigrok47
> pigrok45: decent model should stick to using with commit messages

<-- End of do not edit -->

## Search

- Code: semble (`semble search "<query>" crates scripts`). Always pass those paths, not the repo root: semble does not read `.git/info/exclude`, so a root search returns stale Swift copies from `.delta/worktrees`.
- Markdown: iwe owns every hand-written `.md` file (`.iwe/config.toml`); its formatting is the house style. Read with `iwe find --lexical "<query>"`, `iwe retrieve <key>`, `iwe tree`. Create, move and change docs through iwe (`iwe new`, `create`, `update`, `rename`, `delete`, `extract`, `inline`) so links stay correct. After any markdown edit, run `make docs`.
- Generated markdown is the exception: `THIRD_PARTY_NOTICES.md` is written by `make notices` (`scripts/gen-notices.py`). Never format or edit it through iwe, and never run a bare `iwe normalize` (it rewrites every doc, generated ones included); `make docs` skips them. A new generated doc goes in `GENERATED_DOCS` in the Makefile.

## Building

Use the Makefile for builds; don't hand-roll `cargo build` commands.

- `make fresh` — clean release build for the user to test. Output: `dist/smcli-<timestamp>-<commit>`, with `dist/smcli` symlinked to the latest. It also prints the `eval` line that loads the zsh hooks from the new build; relay those commands to the user. Run this after changes the user will try out.
- `make release` — same, without cleaning (faster). `make build` — debug build; `make run ARGS="..."` builds and runs the CLI.
- `make test` — full test suite (`scripts/test.sh`): `cargo test --workspace`, then black-box CLI checks against a temp `HOME`, `SMCLI_DB` and `SMCLI_RUNTIME_DIR`.
- Single test: `cargo test -p <crate> <test name>`.
- `make ranking` — scores suggest/search ranking on the fixed query set (`crates/smcli-search/tests/ranking.rs`); run after ranking changes.
- `make linux` — full test suite on Linux in Docker (needs a running daemon).
- `make baseline` — black-box CLI/hook benchmark (`scripts/bench-baseline.py`, see `agents/reference/baseline.md`); never reads real history.
- `make docs` — formats all hand-written markdown with iwe. `make notices` — regenerates `THIRD_PARTY_NOTICES.md`.
- `make clean` — `cargo clean`, keeping the newest `dist/` CLI and service pair (live shell hooks may point at them).

## Architecture

Cargo workspace (`crates/`), macOS and Linux, search included on both. Two executables, both embedding the potion-shell-16m model (fine-tuned potion-code-16M-v2, `Models/potion-shell-16m`) (`include_bytes!`, loaded from memory via model2vec-rs), so each can search alone:

- `smcli` (`crates/smcli`) — clap CLI, ratatui picker and menu, `shell-init` hook generation (`shell_integration.rs`).
- `smcli-service` (`crates/smcli-service`) — long-lived owner of history state and the open search index, serving clients over a Unix-domain socket. Only it depends on `fff-search` (file search); the CLI and `smcli-core` must never link fff.

Libraries: `smcli-core` (SQLite captured history at `~/.local/share/smcli/history.db`, capture filters, ranking/suggestions, config, socket client and protocol), `smcli-search` (hybrid BM25 + embedding search over captured and native history in a TalaDB index; Potion only).

Client/service protocol (`smcli-core/src/service_protocol.rs`, frozen fixtures in `crates/smcli-core/tests/fixtures/service_protocol`): one newline-delimited JSON request and one response per connection, versioned. On a version mismatch or no service, the CLI falls back to doing the work in-process, so every operation must work both ways. The service identifies its binary by path/mtime/size so a rebuilt binary is detected.

Native shell history files are read-only: never rewrite them. smcli keeps its own SQLite log.
