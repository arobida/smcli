# Proposal

## Why

`@@` only knows programs that have a tldr page. cheat/cheatsheets (CC0, about 500 programs) covers some that tldr does not. Adding it as a fallback widens coverage without diluting tldr's examples where tldr already has a page.

## What Changes

- Add cheat/cheatsheets as a second bundled, offline example source, generated from a pinned checkout into `Data/cheat/cheat.jsonl` by a new `scripts/gen-cheat.py` (`make cheat CHEAT=<checkout>`), shipped beside `tldr/` by `make release` and `scripts/install.sh`, removed by `scripts/uninstall.sh`.
- **Sources are tried in priority order per program**: tldr first; cheat/cheatsheets only for programs tldr has no examples for. Today every source's examples are merged.
- One place builds the ordered source list; the CLI and the service stop constructing tldr directly.
- `make notices` attributes cheat/cheatsheets (CC0) in `THIRD_PARTY_NOTICES.md`.
- The command-completion index name already includes source names and versions, so the first run after upgrade rebuilds it once.

## Capabilities

### New Capabilities

### Modified Capabilities

- `command-completion-index`: "Pluggable example sources" changes from merging every source to the first source in priority order that has examples for a program.

## Impact

- `smcli-search`: new cheat source module beside `tldr.rs`; `commands.rs` `gather` becomes first-match; a shared ordered-sources constructor.
- `smcli-service/src/service.rs` and `smcli/src/cli.rs`: the five places that build `TldrSource` use the shared list.
- `scripts/gen-cheat.py` (new), `scripts/gen-notices.py`, `Makefile` (`cheat` target, release copy), `scripts/install.sh`, `scripts/uninstall.sh`.
- `Data/cheat/` (new bundled data, with the CC0 license text).
