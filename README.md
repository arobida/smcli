# `smcli`

Read your saved Bash and Zsh commands, and — with an opt-in shell hook — record
metadata native history doesn't reliably keep: the working directory, exit code,
start time, duration, user, host, and terminal session. Search commands with keyword
and semantic matching, or rank captured commands for your current directory.
Search uses the potion-code-16M-v2 static model, built into the binary (about 33 MB, MIT licensed,
see `Models/`). Search works on macOS and Linux.

> [!NOTE]
> Native shell history stores the command line and little else. smcli never rewrites
> that file; it keeps its own private SQLite log alongside.

## Quickstart

```sh
$ make release
$ dist/smcli history
==> /Users/you/.zsh_history <==
git status
git push origin main
# ^^ native history, from both shells regardless of the current one

$ dist/smcli search undo last commit
git reset --soft HEAD~1
    runs=4  native history
# ^^ illustrative output; native history has no captured time, exit code, or directory
# ^^ the first search embeds distinct commands; later searches index new commands
```

## Remember more than your shell does

```sh
$ dist/smcli suggest
# ^^ no output if you haven't captured or imported any commands

$ eval "$("$PWD/dist/smcli" shell-init zsh)"

$ git status
$ ./run-tests.sh
$ git commit -am 'fix login'

$ dist/smcli suggest
git commit -am 'fix login'
./run-tests.sh
git status
# ^^ illustrative order, assuming successful runs and no earlier captured history
# ^^ ranked for this user and directory: runs here count 4x, ancestor/descendant 2x, failures 0.25x
```

Nothing installs itself: `shell-init` prints the hooks, you `eval` them. Add the line
to `~/.zshrc` (or `~/.bashrc`) with the binary's absolute path to keep capturing in new
terminals. Use `shell-init bash` for Bash. Finished interactive command lines are
saved in SQLite at `~/.local/share/smcli/history.db`; recording normally happens in
the background, so the newest command may not appear immediately. Set `SMCLI_SYNC=1`
in the shell to make recording synchronous.

`shell-init` binds **Up** and **Ctrl-R** to history search. On an empty prompt in Bash and Zsh, **Left** opens that same search and **Right** opens directory suggestions, when you chose those keys in `smcli init`; a line that already has text keeps normal arrow movement. Start typing to filter every captured and native command
(all words must match), press Tab for semantic search, Enter to put the command on your
prompt, Esc to cancel. Text already on the prompt becomes the initial query. Set
`SMCLI_NO_KEYS=1` before the `eval` to keep your shell's own keys, or `SMCLI_EXTRA_KEYS=1` to add Left and Right on an empty line, Ctrl-Right and Alt-;. Bash needs 4.0+. Fish keeps Right for autosuggestions.

## Install

See **[docs/install.md](docs/install.md)** to install a release or build from source, set up Zsh, Bash or Fish,
choose the picker keys, and upgrade or uninstall. In short:

```sh
scripts/install.sh       # build (needs Rust and a C toolchain), install, hook your shell, start the service
smcli-uninstall          # export your history, then remove everything
```

Linux builds include search. Install a C toolchain, pkg-config, and SQLite development
headers (e.g. `build-essential pkg-config libsqlite3-dev` on Debian/Ubuntu).

## Usage

```sh
smcli history                 # native history by file, then captured records
smcli history --json          # captured records only, as JSON
smcli search [query...]       # --limit N, --json, filters below
smcli suggest [prefix...]     # --dir PATH, --limit N, --json
smcli shell-init bash|zsh|fish  # print opt-in hooks; eval (or `| source` in fish)
smcli pick [query...]         # interactive picker; prints the chosen command
smcli init                    # configure picker keys
smcli init --keys [LIST]      # picker keys only: ask, or ctrl-r,left,right | ctrl-r | left | right | none
smcli init --potion [DIR]     # use another potion-code-16M-v2 copy (no DIR: the built-in one)
smcli init --default          # back to the default: the built-in Potion model
smcli reindex                 # discard the search index and re-embed every command
smcli health [--json]         # index coverage, size, and embedding model check (exit 1 if broken)
smcli import legacy.jsonl     # import older smcli JSONL logs (not native history)
smcli import --atuin [db]     # import Atuin history (default ~/.local/share/atuin/history.db)
smcli import --atuin --replace [db]  # discard existing smcli history, then import
smcli prune [--dry-run]       # delete stored commands that current filters reject
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
- Native history is decoded for display: Bash timestamp lines and Zsh extended-history
  prefixes are removed, and Zsh escaped bytes are decoded. The source files aren't changed.
- Search combines captured commands and native history, treating each native line as an
  entry (not reconstructing multiline commands). It ranks distinct commands by fusing
  BM25 keywords (whole words like `--soft` and their pieces) with potion-code-16M-v2
  embeddings. Semantic matches aren't guaranteed.
- Literal queries (flags, paths, revisions like `HEAD~1`, or a short query led by a program in
  your history) weight keywords more; plain-language requests fuse both legs by rank.
- A bounded pool of matches is reranked with usage aggregates computed in the same SQL
  query as the stats: age-weighted runs (a run two weeks old counts half), runs in the
  current directory or below, and success rate (Ctrl-C, and exit 1 from `grep`/`diff`-style
  programs, are not failures). Usage scales with relevance, so habits lift strong matches
  without surfacing weak ones; failed commands are demoted, not hidden. Text
  results show why a command ranked (`exact words`, `used here`, `recent`, …).
- potion-code-16M-v2 runs natively (WordPiece tokens, mean of float16 rows, normalize;
  no network pass). It is embedded in the binary,
  so there is nothing to download; `init --potion DIR` points at another copy. Being a static model, it embeds only the command and its intent,
  not the labels below; word order is lost (`cp a b` equals `cp b a`), so keywords matter.
  For commands with captured metadata, run counts come only from captured records;
  otherwise they count occurrences across native files. Native lines have no times, so
  their usage decays by position (halving every 1,000 lines from the end); a captured
  command keeps its native usage when that is the stronger evidence.
- Each command is embedded with labels — `command:`, `subcommand:`, `flags:`,
  `author:` and `intent:` (from the last captured run) — so queries naming a
  tool or option rank closer. Directories are not embedded: use `--dir PATH`, and
  commands run in the current directory get a "used here" boost. Time and frequency are filters, not labels.
  Captured commands store their option names in a `flags` column (filled in for existing rows on upgrade).
- Hard filters drop non-matching results: `--command git`, `--subcommand commit`,
  `--since 7d`, `--before 2026-10-01`, `--min-runs 5`, `--author NAME`, `--dir PATH`
  (last run in PATH or below), `--flag NAME` (exact option name, e.g. `--flag --amend`). Times are `30m`/`12h`/`7d`/`2w`,
  `YYYY-MM-DD`, or ISO 8601, compared with the command's last run, so time filters exclude
  native-only history. With filters the query may be empty: matches list by last run.
- The index lives next to the database at `history.db.search/` and is a rebuildable
  cache — delete it to reclaim space. Separate model indexes can be reused when switching
  back to a previously selected model.
- `suggest` reads captured/imported history only and filters to the current user;
  each run is scored by recency (halves every 14 days), directory, and exit status.
  An optional prefix matches the start of the command literally. `--dir` requires an
  absolute path. Search and suggestions exclude commands recognized as invoking smcli.

## Caveats

> [!WARNING]
> History can contain passwords, tokens, and other secrets. smcli does not redact
> captured commands — don't share the database or the search index. Search embeddings
> are computed locally with the bundled Potion model.

> [!NOTE]
> Capture is best-effort, not an audit. It records finished interactive commands;
> commands inside scripts/functions aren't logged individually, and `exec`, force-killed
> shells, and terminal crashes can escape it. Bash hooks refuse installation if DEBUG/EXIT
> traps, functrace, or extdebug are already enabled. Older Bash versions time commands
> with whole-second precision.

- Keep `SMCLI_DB` separate from your native `HISTFILE` — the CLI rejects the same file,
  symlink, or hard link.
- `SMCLI_DB` overrides the database path. Search model settings are stored in
  `~/.config/smcli/config.json` (or `$XDG_CONFIG_HOME/smcli/config.json`);
  `SMCLI_CONFIG` overrides that file's path. `init` configures search, not capture,
  and is not required for the built-in model.
- `import` of legacy JSONL is transactional but does not deduplicate: importing the same
  file again adds duplicate records. Run each migration only once.
- Like Atuin, capture skips commands typed with a leading space, commands containing likely
  secrets (AWS, GitHub, GitLab, Slack, Stripe, Google, npm, Anthropic/OpenAI keys), and any
  matching regexes in the config file: `"historyFilter": ["^vault "]` for commands,
  `"cwdFilter": ["^/Users/me/secret"]` for directories. Set `"secretsFilter": false` to turn
  off the secret patterns. `prune` applies the current filters to already-stored history
  and compacts the database so deleted text is not left on disk.
- `import --atuin` keeps Atuin's record UUIDs, so re-running it only adds new commands.
  It also carries over soft deletions (hidden from history, search, and suggestions) and
  Atuin's optional `author`/`intent` fields. `ATUIN_DB_PATH` or `XDG_DATA_HOME` override
  the default source path. Existing smcli databases are upgraded to the new schema on first use.

## Test

```sh
$ make test
```

## Backend baseline and fff experiment

Before changing shell transports or search ownership:

```sh
make baseline       # release CLI; synthetic 1k/10k/100k histories, raw JSON in dist/baseline.json
make fff-smoke      # pinned fff-search crate smoke test
```

Normal builds link the pinned `fff-search` crate into `smcli-service` only;
`smcli` does not link fff. Builds require Cargo and a C toolchain. See
[the baseline report](agents/reference/baseline.md) for the capture-to-picker trace,
measurements, dependency/API verification, limitations, and the next gate.
