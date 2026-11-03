# Natural Language Command Help (`@@`)

- **Author:** Andrew Robida
- **Status:** Implemented; storage and program selection superseded by the command-completion index.
- **Date:** 2026-11-03

## User flow

In Bash, Zsh and Fish, type `bun @@` and press Tab to open command help for that program. Text after `@@`, such as `bun @@ install globally`, becomes the initial picker query. A bare `@@` searches all examples in the command-completion index. Rows show descriptions, command templates and source attribution.

Only the whole word `@@` triggers command help. Ordinary `@` file completion, scoped package names and words such as `@@x` keep their existing behavior.

Enter inserts the selected template without running it. Each tldr `{{name}}` placeholder becomes a bare `@`; the cursor moves to the first placeholder. For example, selecting the global package-install example produces `bun add --global @`. The placeholder can be a package or other value, so selection does not automatically open the file picker. Esc leaves the prompt unchanged.

Leading wrappers such as `sudo`, `env` and `command` are retained. Subcommand words before `@@`, such as `git rebase`, supply a ranking bonus rather than a hard filter. Insertion replaces the original program/question text, so subcommands are not duplicated.

The picker can switch between the requested program and all indexed programs while retaining the query. A program with no indexed examples shows the missing-page status and allows switching to all programs.

## Current implementation

The [command-completion index specification](../../openspec/specs/command-completion-index/spec.md) owns program selection, source identity and index lifecycle. Its [archived design](../../openspec/changes/archive/2026-11-03-command-completion-index/design.md) replaces the original per-page embedding and `.vec` cache proposal.

- `smcli-core::command_source` defines `CommandSource` and `CommandExample`.
- `smcli-search::tldr::TldrSource` loads read-only bundled JSONL, retaining common and current-platform pages. Data includes its source commit and license attribution.
- `smcli-search::commands::CommandIndex` persists examples and vectors beside the history index. Programs come from OS tools and captured history, intersected with PATH and available source examples.
- Install/reindex builds the index. The service syncs it after capture when an unknown program or changed PATH requires it. `@@` reads the existing index and builds only when it is missing.
- `pick --tldr` reads context from `SMCLI_LBUFFER` and the initial question from `SMCLI_TLDR_QUERY`. It conflicts with the other pick modes and returns the selected command on stdout.
- The picker embeds questions locally with Potion and ranks indexed vectors with a prefix bonus. It does not execute programs or fetch help over the network.
- Old `tldr-*.vec` files are obsolete and handled by `smcli indexes --prune`; their reader/writer has been removed.

## Verification

- `make test-rust PKG=smcli-search` and `make tldr-ranking` cover data loading, placeholders, ranking and the command index.
- `make test-picker` and `make test-shells` cover picker and shell insertion behavior.
- `make test-cli` covers index listing and legacy-cache pruning.

No new latency claim is made by this documentation cleanup.
