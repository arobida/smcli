# `smcli`

Read your saved Bash and Zsh history as-is, and — with an opt-in shell hook — record
what plain history drops: the working directory, exit code, wall-clock time, and
terminal session. Then search and rank it all, entirely on-device.

> [!NOTE]
> Native shell history stores the command line and little else. smcli never rewrites
> that file; it keeps its own private SQLite log alongside.

## Quickstart

```sh
$ swift build -c release
$ .build/release/smcli history
==> /Users/you/.zsh_history <==
git status
git push origin main
# ^^ native history, from both shells regardless of the current one

$ .build/release/smcli search undo last commit
git reset --soft HEAD~1
    runs=4  last=2026-10-12T09:14:03Z  exit=0  /Users/you/project
# ^^ keyword + semantic; the first search embeds history (~20s per 5,000 commands), later ones are fast
```

## Remember more than your shell does

```sh
$ .build/release/smcli suggest
# ^^ empty — native history has no user or directory to rank on

$ eval "$("$PWD/.build/release/smcli" shell-init zsh)"

$ git status
$ ./run-tests.sh
$ git commit -am 'fix login'

$ .build/release/smcli suggest
./run-tests.sh
git commit -am 'fix login'
git status
# ^^ ranked for this user and directory: runs here count 4x, parent/subdir 2x, failures 0.25x
```

Nothing installs itself: `shell-init` prints the hooks, you `eval` them. Add the line
to `~/.zshrc` (or `~/.bashrc`) with the binary's absolute path to keep capturing in new
terminals. Each finished command becomes one row in SQLite at
`~/.local/share/smcli/history.db`.

## Install

Requires Swift 6.0+. Search needs macOS 14+ (Apple's on-device embeddings) and is
unavailable on Linux.

```sh
$ swift build -c release
```

The binary is `.build/release/smcli`. There's no installer yet.

## Usage

```sh
smcli history                 # native history, all files
smcli history --json          # captured records only, as JSON
smcli search <query...>       # --limit N, --json
smcli suggest [prefix...]     # --dir PATH, --limit N, --json
smcli init                    # pick an Ollama embedding model for search
smcli import legacy.jsonl     # one-time migration from older JSONL logs
```

Run `smcli` with no arguments in a terminal for an interactive menu. For the rest:

```sh
$ smcli --help
```

## How it works

- Native history is read from `~/.bash_history`, `~/.zsh_history`, and the
  `.history`/`.historynew` files in `~/.bash_sessions`/`~/.zsh_sessions`, plus an
  exported `HISTFILE`. It's grouped by file, not merged chronologically — most native
  files carry no timestamps.
- Search ranks distinct commands by fusing BM25 keywords (whole words like `--soft` and
  their pieces) with Apple's sentence embeddings, so wording that isn't in the command
  can still match (`extract archive` → `tar -xzf …`). The index lives next to the
  database at `history.db.search/` and is a rebuildable cache — delete it to reclaim space.
- `suggest` reads captured history only; each run is scored by recency (halves every 14
  days), directory, and exit status.

## Caveats

> [!WARNING]
> History contains passwords, tokens, and other secrets. smcli never uploads anything
> and never redacts captured commands — don't share the database or the search index.

> [!NOTE]
> Capture is best-effort, not an audit. It records finished interactive commands;
> scripts, functions, `exec`, force-killed shells, and terminal crashes can escape it.

- Keep `SMCLI_DB` separate from your native `HISTFILE` — the CLI rejects the same file,
  symlink, or hard link.
- The first search embeds every distinct command. Switch embedding models
  (`smcli init --model embeddinggemma`) and it re-embeds once; return to the built-in
  model with `smcli init --builtin`.

## Test

```sh
$ bash scripts/test.sh
```
