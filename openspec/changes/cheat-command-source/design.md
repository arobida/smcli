# Design

## Context

`CommandSource` (`smcli-core/src/command_source.rs`) is the seam; tldr is the only implementation (`smcli-search/src/tldr.rs`, data `Data/tldr/tldr.jsonl` from `scripts/gen-tldr.py`). `CommandIndex::gather` (`commands.rs:157`) flat-maps every source. `TldrSource` is built directly in five places (`service.rs` ×4, `cli.rs` ×2), and `command_index` in the service takes `&TldrSource`, not the trait.

cheat/cheatsheets format, one file per program: optional YAML front matter (`tags:`), then blocks of `# description` followed by one or more command lines. Placeholders are literal text (`/path/to/foo.tar`), not `{{...}}`.

## Goals / Non-Goals

**Goals:**

- Per-program priority fallback, tldr then cheat.
- Same offline, pinned, generated-data pipeline as tldr.

**Non-Goals:**

- Live cheat.sh, user/local sheets, navi sheets.
- Placeholder conversion for cheat templates (they are inserted as written).
- Per-example merging or dedupe across sources (priority makes it unnecessary).

## Decisions

**First-match `gather`.** `gather` returns the first source's non-empty result instead of the concatenation. One-line change in shared code, so the CLI and service paths agree. Alternative: merge with dedupe by template; rejected by the user's priority rule, and it would bloat `@@` for programs tldr already covers well.

**One ordered source list.** Add a constructor in `smcli-search` that returns the bundled sources in priority order (tldr required as today, cheat when its data loads). Replace the five direct `TldrSource::new` sites with it and make the service's `command_index` take `&[&dyn CommandSource]`. This also keeps the index name (built from source names and versions) identical between CLI and service.

**Data file shape.** `Data/cheat/cheat.jsonl` mirrors tldr: header `{"version": 1, "cheat_commit": "<sha>", "license": "CC0-1.0"}`, then `{"program", "platform": "common", "description", "template"}` lines. One example per command line; consecutive command lines under one `# description` share it. The description is the block's first comment line: further consecutive comment lines before the command are ignored, and comment lines between commands of the same block do not change the description. A command line ending in a backslash continues on the next line: the lines are joined into one template with a single space (the backslash-newline is dropped) and count as one example. Front matter and blank lines are dropped. Only top-level regular files are read (dotfiles and directories skipped). The generator refuses a dirty checkout and caps output size, like `gen-tldr.py`; `scripts/test-gen-cheat.py` holds fixture-based regression tests for the parsing rules.

**Packaging.** Ships as `cheat/` beside `tldr/` (`make release` copy, `install.sh` stage and move, `uninstall.sh` removal). Unlike tldr, a missing `cheat/` is not an install error: the source is optional.

**Notices.** `gen-notices.py` gets a cheat section from the header (commit, license, entry count) and `Data/cheat/LICENSE.txt`.

## Risks / Trade-offs

- [cheat/cheatsheets was last updated August 2024] → it is a fallback only; tldr still wins where it has a page.
- [Literal placeholders like `/path/to/foo` get inserted and must be edited] → accepted for the first version.
- [A cheat sheet for a program tldr covers is never shown] → intended by the priority rule.
- [Index rebuild on first run after upgrade] → existing lifecycle: the index name changes with the source set; `smcli indexes --prune` cleans the old one.
