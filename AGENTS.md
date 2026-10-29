# AGENTS.md

This file provides guidance to coding agents when working with code in this repository.

<-- Start of do not change this -->

## Agent Team(can be referenced by saying ateam)

By using the kero skill you are allowed to open new sessions with the list of agents I will provide. Whenever you are asked you should efficiently delegate task to the appropriate agent and act as the orchestrator and let them handle the tasks. Do not perform redudant testing or rerun them unecessarily. The list will show two things first an alias followed by the command needed to open the agent in the new sesion.

### Agents List:

pisol=pi --model openai/gpt-6.1-sol --thinking low
piterra=pi --model openai/gpt-6.1-terra --thinking low
piluna=pi --model openai/gpt-6-luna --thinking low
pikcode=pi --model opencode-go/kimi-k2.7-code --thinking high
pidpro=pi --model opencode-go/deepseek-v4-pro --thinking high
pidflash=pi --model opencode-go/deepseek-v4.1-flash --thinking high
pigrok47=pi --model xai/grok-4.7 --thinking high
pigrok46=pi --model xai/grok-4.6 --thinking high
pigrok45=pi --model xai/grok-4.5 --thinking medium

#### Agent Affinities

> Some agents are better at certain tasks than others(try to use free models as much as possible)

pisol: great for complicated reasoning tasks
piterra: great for running test and code review
piluna: great fast general purpose worker
pikcode: great at writing code
pidpro: good general purpose model, very token efficient with prompt caching
pidflash: good fast general purpose model with vision
pigrok47: complicated reasoning tasks
pigrok46: general purpose model
pigrok45: decent model should stick to using with commit messages

<-- End of do not edit -->

### Dispatching to the team

Delegate through `scripts/ateam.sh` instead of hand-rolled `kero new` loops: `bash scripts/ateam.sh <brief> <alias=command>...` (bash only; run it, don't source it from zsh), passing the `alias=command` lines above verbatim.

- Starts every agent in parallel in the repo dir (each in its own kero session), waits for each prompt, then briefs it.
- `<brief>` is one of: empty (start only), inline text (sent to every agent), a task file path, or a task directory; with a directory it briefs each agent from `<dir>/<alias>.task` — write one `.task` file per agent and pass the dir, so each gets `Read <dir>/<alias>.task and do exactly what it says.`
- Prints `<alias>\t<session-id>` per agent; monitor with `kero wait` / `kero read`, `kero close` when done. `ATEAM_SHELL` (default zsh) and `ATEAM_DIR` (default `$PWD`) override.

## Git

Stage, write commit messages, and push using [Stage, commit, and push](agents/reference/commits.md). Only commit or push when asked; follow that runbook for staging, caveman-commit message format, and remote push rules.

## Search

- Code: semble (`semble search "<query>" crates scripts`). Always pass those paths, not the repo root: semble does not read `.git/info/exclude`, so a root search returns stale Swift copies from `.delta/worktrees`.
- Markdown: iwe owns every hand-written `.md` file (`.iwe/config.toml`); its formatting is the house style. Read with `iwe find --lexical "<query>"`, `iwe retrieve <key>`, `iwe tree`. Create, move and change docs through iwe (`iwe new`, `create`, `update`, `rename`, `delete`, `extract`, `inline`) so links stay correct. After any markdown edit, run `make docs`.
- Generated markdown is the exception: `THIRD_PARTY_NOTICES.md` is written by `make notices` (`scripts/gen-notices.py`). Never format or edit it through iwe, and never run a bare `iwe normalize` (it rewrites every doc, generated ones included); `make docs` skips them. A new generated doc goes in `GENERATED_DOCS` in the Makefile.

## Building

Use the Makefile for builds; don't hand-roll `cargo build` commands.

- `make fresh` — clean release build for the user to test. Output: `dist/smcli-<timestamp>-<commit>`, with `dist/smcli` symlinked to the latest. It also prints the `eval` line that loads the zsh hooks from the new build; relay those commands to the user. Run this after changes the user will try out.
- `make install` — release build, then `scripts/install.sh` installs it on this machine (binaries and model in `~/.local/bin`, shell hooks, keys, service; `ARGS="--yes"` etc. pass through).
- `make uninstall` — runs `~/.local/bin/smcli-uninstall $(ARGS)`, falling back to `scripts/uninstall.sh`.
- `make release` — same, without cleaning (faster). `make build` — debug build; `make run ARGS="..."` builds and runs the CLI.
- `make test` — full test suite (`scripts/test.sh`): `cargo test --workspace`, then black-box CLI checks against a temp `HOME`, `SMCLI_DB` and `SMCLI_RUNTIME_DIR`.
- Focused slices of the gate, each assuming `make build` output: `make test-rust` (`PKG=smcli-core FILTER=history`
  narrows crate and test), `make test-shells` (honors `SMCLI_TEST_ONLY`), `make test-service`, `make test-install`.
- `make ranking` — scores suggest/search ranking on the fixed query set (`crates/smcli-search/tests/ranking.rs`); run after ranking changes.
- `make linux` — full test suite on Linux in Docker (needs a running daemon).
- `make baseline` — black-box CLI/hook benchmark (`scripts/bench-baseline.py`, see `agents/reference/baseline.md`); never reads real history.
- `make docs` — formats all hand-written markdown with iwe. `make notices` — regenerates `THIRD_PARTY_NOTICES.md`.
- `make clean` — `cargo clean`, keeping the newest `dist/` CLI and service pair (live shell hooks may point at them).

## Architecture

Cargo workspace (`crates/`), macOS and Linux, search included on both. Two executables sharing one copy of the potion-shell-16m model (fine-tuned potion-code-16M-v2, `Models/potion-shell-16m`), shipped as a `potion-shell-16m/` folder beside them (`make release` copies it into `dist/`, `install.sh` into the prefix; dev builds and tests fall back to the checkout's `Models/`). Either can load it, so each can search alone:

- `smcli` (`crates/smcli`) — clap CLI, ratatui picker and menu, `shell-init` hook generation (`shell_integration.rs`).
- `smcli-service` (`crates/smcli-service`) — long-lived owner of history state and the open search index, serving clients over a Unix-domain socket. Only it depends on `fff-search` (file search); the CLI and `smcli-core` must never link fff.

Libraries: `smcli-core` (SQLite captured history at `~/.local/share/smcli/history.db`, capture filters, ranking/suggestions, config, socket client and protocol), `smcli-search` (hybrid BM25 + embedding search over captured and native history in a TalaDB index; Potion only).

Client/service protocol (`smcli-core/src/service_protocol.rs`, frozen fixtures in `crates/smcli-core/tests/fixtures/service_protocol`): one newline-delimited JSON request and one response per connection, versioned. On a version mismatch or no service, the CLI falls back to doing the work in-process, so every operation must work both ways. The service identifies its binary by path/mtime/size so a rebuilt binary is detected.

Models: `smcli init --potion DIR` switches model and `--default` goes back. A model's index is named by its identity, so a switch rebuilds the index on the next search, and init says so. Never hold two models at once: init and `smcli reindex` stop the service before loading a model, and the service drops its old engine before building a new one (`load_engine`). `smcli indexes [--prune]` lists every index on disk, marks the current model's, and deletes the others.

Native shell history files are read-only: never rewrite them. smcli keeps its own SQLite log.
