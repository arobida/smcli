# AGENTS.md

This file provides guidance to coding agents when working with code in this repository.

<-- Start of do not change this -->

## Agent Team(can be referenced by saying ateam)

A native Claude agent named the **Orchestrator** runs every team below and handles all Kero commands. When you are asked to delegate, do not open sessions or drive panes yourself: choose the team and agent from the routing table, then have the Orchestrator start it, prompt it, and read the result. The Orchestrator owns `kero new`/`prompt`/`wait`/`read`/`close`; do not run them directly. The list shows two things: an alias, then the command the Orchestrator uses to open the agent.

> Some agents are better at certain tasks than others (try to use free models as much as possible). Pick a team, then take the first row in its routing table that matches the job.

### GPT Team (oteam)

| Task type                                                                                      | Route to  | Why                                                                                                      |
| ---------------------------------------------------------------------------------------------- | --------- | -------------------------------------------------------------------------------------------------------- |
| Planning a change and implementing that plan, long refactors, long agent sessions              | `pisol`   | Best GPT coder; beats Terra by ~13 pts when planning in long sessions, but ~4x the cost and ~3.5x slower |
| Default: reading code, code review, Q&A, small fixes, running tests, docs/writing, screenshots | `piterra` | Within a couple points of Sol on these, ~4x cheaper and much faster; strong vision and writing           |
| Lookups, grep-style questions, extraction, summaries you will check yourself                   | `piluna`  | ~1/10 of Terra's price but 10-15 pts weaker and unreliable one-shot; never for edits that matter         |

```
pisol=pi --model openai/gpt-6.1-sol --thinking low
piterra=pi --model openai/gpt-6.1-terra --thinking low
piluna=pi --model openai/gpt-6-luna --thinking low
```

### Grok Team (gteam)

| Task type                                                                                           | Route to   | Why                                                                                                                 |
| --------------------------------------------------------------------------------------------------- | ---------- | ------------------------------------------------------------------------------------------------------------------- |
| Long multi-step terminal work, multi-file migrations, running test suites, thorough codebase review | `pigrok47` | Same price as 4.6 but ~2x on Terminal-Bench 4.0 and ahead on 16 of 18 benchmarks; token-heavy, weak at front-end/UI |
| Fallback when `pigrok47` is busy or rate-limited                                                    | `pigrok46` | Same price as 4.7 and weaker nearly everywhere; no task where it is the better pick                                 |
| Commit messages                                                                                     | `pigrok45` | Older and cheaper on cache; fine for short structured text, not for coding                                          |

```
pigrok47=pi --model xai/grok-4.7 --thinking high
pigrok46=pi --model xai/grok-4.6 --thinking high
pigrok45=pi --model xai/grok-4.5 --thinking low
```

### Kimi Team (kteam)

| Task type                                                                              | Route to   | Why                                                                                                                                         |
| -------------------------------------------------------------------------------------- | ---------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| Hard bugs, root-cause diagnosis, planning, hard terminal tasks                         | `kik31pro` | Strongest Kimi (Terminal-Bench 4.0 ~66%); better than K3 at diagnosis and planning but slow (~2x the time, ~2x the tokens); pinned to `low` |
| Whole-repo analysis past 256K, multi-agent orchestration, research, front-end/3D/WebGL | `kik3`     | 1M context; strong at orchestration, research and visual front-end work                                                                     |
| Everyday features, routine Q&A, single/small-file edits                                | `kik3-256` | K3 quality within 256K at ~half the quota of `kik3`                                                                                         |
| Implement a scoped feature, review a change, code completion, quick fixes              | `kikcode`  | Close to K3 on coding with much leaner thinking, so cheaper and quicker per task; pinned to `high` (default `max`)                          |

```
kikcode=kimi -m kimi-code/kimi-for-coding --auto
kik3=kimi -m kimi-code/k3 --auto
kik31pro=kimi -m kimi-code/k3.1-pro --auto
kik3-256=kimi -m kimi-code/k3-256k --auto
```

Effort is pinned per model by `[models."<alias>".overrides] default_effort` in `~/.kimi-code/config.toml` (`k3.1-pro` low, the rest high). Do not set global `[thinking].effort` — it overrides the main agent's model default. The Orchestrator opens each as a Kero session and drives it; for a throwaway one-shot it runs `kimi -m <model> -p "<prompt>"` without `--auto`: kimi rejects `-p` combined with `--auto` (`Cannot combine --prompt with --auto`).

### Other

| Task type                                                                           | Route to   | Why                                                                                                                    |
| ----------------------------------------------------------------------------------- | ---------- | ---------------------------------------------------------------------------------------------------------------------- |
| Cheap long sessions, analysis/planning, self-checking its own changes, vision tasks | `pidflash` | Top open-weight coder in one independent long-session test (80%, ~$0.14 a session, the fastest); 1M context and vision |

```
pidflash=pi --model opencode-go/deepseek-v4.1-flash --thinking high
```

<-- End of do not edit -->

## Task tracking (dex)

Every agent, including subagents and the external teams, tracks all work in dex (`dex` CLI, or the `dex` skill). Tasks live in `.dex/` in this repo.

- Every todo, task, follow-up and action item goes into dex: `dex create "name" --description "..."`, with subtasks for multi-step work. Do not keep them anywhere else.
- dex overrides the task tracking of any other skill, plugin or tool (built-in todo lists, OpenSpec `tasks.md`, plan-file checklists, MemPalace tasks). When one of them asks for its own task list, put the tasks in dex. Those tools may still hold the plan or spec text; dex holds the tasks.
- `dex start <id>` when work begins; `dex complete <id> --result "..."` when it is done (add `--commit <sha>` when there is one).
- Turn a plan document into tasks with `dex plan <file>`. Check `dex list` before starting work so you do not duplicate a task.

## Testing rules

1. **Never run the full suite while work is in progress.** `make test` runs once, and only after *all* the work is complete: every slice written, reviewed, and passing its own checks. This applies to everyone, including the main agent. Linux is on the backburner: do not run `make linux` unless asked, and put Linux issues in [the roadmap](agents/plans/roadmap.md) instead of fixing them.
2. **During the work, verify with modular tests only.** Use unit tests (`make test-rust PKG=<crate> FILTER=<test>`) or the one focused slice that covers the change (`make test-picker`, `make test-shells`, and so on; see "Building").
3. **Tests run only after the code is written, and only Kimi Team (kteam) runs them.** Coders, reviewers and other teams do not run tests.
4. **When a test fails:** send the failing output and the code under test for review, fix it from the review, then have kteam rerun only that modular test. The full suite runs again only when all work is fixed and passing.

## Git

Stage, write commit messages, and push using [Stage, commit, and push](agents/reference/commits.md). Only commit or push when asked; follow that runbook for staging, caveman-commit message format, and remote push rules.

Use the Worktrunk `wt` CLI for worktrees and commits: `wt switch --create <branch>` to start a worktree, `wt step commit --stage=none` to commit what you staged (it writes the message), `wt merge` to land it, and `wt remove` to clean up. Do not use raw `git worktree` or hand-write commit messages with `git commit -m`. Preview a message with `wt step commit --dry-run`. The caveman-commit format reaches the LLM through `template-append` in `.config/wt.toml`; change the format there and in the runbook together.

## Search

- Code: semble (`semble search "<query>" crates scripts`). Always pass those paths, not the repo root: semble does not read `.git/info/exclude`, so a root search returns stale Swift copies from `.delta/worktrees`.
- Markdown: iwe owns every hand-written `.md` file (`.iwe/config.toml`); its formatting is the house style. Read with `iwe find --lexical "<query>"`, `iwe retrieve -k <key>`, `iwe tree`. Create, move and change docs through iwe (`iwe new`, `create`, `update`, `rename`, `delete`, `extract`, `inline`) so links stay correct. After any markdown edit, run `make docs`.
- Generated markdown is the exception: `THIRD_PARTY_NOTICES.md` is written by `make notices` (`scripts/gen-notices.py`). Never format or edit it through iwe, and never run a bare `iwe normalize` (it rewrites every doc, generated ones included); `make docs` skips them. A new generated doc goes in `GENERATED_DOCS` in the Makefile.

## Building

Use the Makefile for builds; don't hand-roll `cargo build` commands.

- `make fresh` — clean release build for the user to test. Output: `dist/smcli-<timestamp>-<commit>`, with `dist/smcli` symlinked to the latest. It also prints the `eval` line that loads the zsh hooks from the new build; relay those commands to the user. Run this after changes the user will try out.
- `make install` — release build, then `scripts/install.sh` installs it on this machine (binaries and model in `~/.local/bin`, shell hooks, keys, service; `ARGS="--yes"` etc. pass through).
- `make uninstall` — runs `~/.local/bin/smcli-uninstall $(ARGS)`, falling back to `scripts/uninstall.sh`.
- `make release` — same, without cleaning (faster). `make build` — debug build; `make run ARGS="..."` builds and runs the CLI.
- `make test` — full test suite (`scripts/test.sh`): `cargo nextest run --workspace` (install: `cargo install cargo-nextest --locked`; config in `.config/nextest.toml`), then black-box CLI checks against a temp `HOME`, `SMCLI_DB` and `SMCLI_RUNTIME_DIR`.
- Focused slices of the gate (black-box targets build automatically): `make test-rust` (`PKG=smcli-core FILTER=history`
  narrows crate and test), `make test-cli` (CLI smoke), `make test-picker` (file + alias picker), `make test-shells` (runs checks in parallel; honors `SMCLI_TEST_ONLY`, and `SMCLI_TEST_JOBS=1` runs them one at a time), `make test-service`, `make test-install`. Match the slice to what changed: picker/menu -> `test-picker` + `test-rust PKG=smcli`, hooks -> `test-shells`, service -> `test-service`, install -> `test-install`.
- `make ranking` — scores suggest/search ranking on the fixed query set (`crates/smcli-search/tests/ranking.rs`); run after ranking changes.
- `make linux` — full test suite on Linux in Docker (needs a running daemon). On the backburner; run only when asked.
- `make baseline` — black-box CLI/hook benchmark (`scripts/bench-baseline.py`, see `agents/reference/baseline.md`); never reads real history.
- `make docs` — formats all hand-written markdown with iwe. `make notices` — regenerates `THIRD_PARTY_NOTICES.md`.
- `make unused` — dead-code audit: compiler warnings plus `pub` items nothing uses or only tests use across crates (`scripts/unused-pub.py`). It matches by name, so check each hit, and a shared name such as `new` can hide dead code.
- `make clean` — `cargo clean`, keeping the newest `dist/` CLI and service pair (live shell hooks may point at them).

## Architecture

Cargo workspace (`crates/`), macOS today, Linux support coming soon (its code stays in place). Two executables sharing one copy of the potion-shell-16m model (fine-tuned potion-code-16M-v2, `Models/potion-shell-16m`), shipped as a `potion-shell-16m/` folder beside them (`make release` copies it into `dist/`, `install.sh` into the prefix; dev builds and tests fall back to the checkout's `Models/`). Only `smcli-service` loads it at runtime. The CLI reads the model's files for its identity (never building the model), and loads it only in `smcli init` when switching models and in `smcli reindex`, after stopping the service and taking the model lock (all in `crates/smcli/src/initialize.rs`):

- `smcli` (`crates/smcli`) — clap CLI, ratatui picker and menu, `shell-init` hook generation (`shell_integration.rs`).
- `smcli-service` (`crates/smcli-service`) — long-lived owner of history state and the open search index, serving clients over a Unix-domain socket. Only it depends on `fff-search` (file search); the CLI and `smcli-core` must never link fff.

Libraries: `smcli-core` (sole owner of SQL on the history database at `~/.local/state/smcli/history.db`: capture, import, prune, export; capture filters, ranking/suggestions, config, socket client, protocol and caller context), `smcli-search` (hybrid BM25 + embedding search over captured and native history in a TalaDB index; Potion only). Only `smcli-search` opens, inspects or deletes index files; the `smcli` crate depends on neither `taladb` nor `rusqlite`.

Client/service protocol (`smcli-core/src/service_protocol.rs`, frozen fixtures in `crates/smcli-core/tests/fixtures/service_protocol`): one newline-delimited JSON request and one response per connection, versioned. Each request carries its caller context (home, config, cwd, session, HISTFILE, SHELL, PATH, ranking switches); the service acts only on that and on launch settings, and is started with an allowlisted environment. On a version mismatch or no service, history, suggestions and files fall back in-process. Search falls back to keyword matching and `@@` to word ranking, never to the embedding model. Service-only file-search ranking evidence is unavailable. The service identifies its binary by path/mtime/size so a rebuilt binary is detected.

Models: `smcli init --potion DIR` switches model and `--default` goes back. A model's index is named by its identity, a BLAKE3 hash of its config, tokenizer and weights, so a changed model file gets a new index and the service reloads on the next search. To avoid holding two models at once, whoever has weights resident holds the model lock (`<db>.search/model.lock`): the service from its first model load until exit, and init (when changing models) and `smcli reindex` while they load. Those two stop the service first and refuse if the lock stays held. These are the only CLI paths that load a model; a unit test in `crates/smcli` enforces it. The service drops its old engine before building a new one (`load_engine`). `smcli indexes [--prune]` lists every index on disk, marks the current model's, and deletes the others.

Native shell history files are read-only: never rewrite them. smcli keeps its own SQLite log.
