# Tasks

## 1. Priority order

- [x] 1.1 Make `CommandIndex::gather` return only the first source with examples; update the `commands.rs` tests so two sources for one program index only the first, and a program missing from the first falls back; verify `make test-rust PKG=smcli-search FILTER=commands`.
- [x] 1.2 Add a shared constructor for the bundled sources in priority order and use it at the five `TldrSource::new` sites in `smcli-service/src/service.rs` and `smcli/src/cli.rs`; make the service's `command_index` take `&[&dyn CommandSource]`; verify `cargo test --workspace` and that CLI and service report the same command index name in `smcli indexes`.

## 2. Cheat source and data

- [x] 2.1 Write `scripts/gen-cheat.py` (pinned clean checkout, front matter stripped, one example per command line, size cap) and a `make cheat CHEAT=<checkout>` target; verify it regenerates `Data/cheat/cheat.jsonl` byte-identically on a second run.
- [x] 2.2 Generate `Data/cheat/cheat.jsonl` from a pinned cheat/cheatsheets commit and add `Data/cheat/LICENSE.txt` (CC0); verify the header names the commit and license.
- [x] 2.3 Add the cheat source in `smcli-search` (loads the JSONL, `name` `cheat`, `version` the commit, platform `common`), optional when data is missing; verify unit tests for loading, a program only in cheat, and missing data.

## 3. Packaging and notices

- [x] 3.1 Copy `Data/cheat` into `dist/` in `make release`, stage and install it in `scripts/install.sh` (optional, no error when absent), remove it in `scripts/uninstall.sh`; verify `make test-install`.
- [x] 3.2 Add the cheat section to `scripts/gen-notices.py` and run `make notices`; verify `THIRD_PARTY_NOTICES.md` names cheat/cheatsheets, its commit and CC0.

## 4. Integration

- [x] 4.1 Verify with `make fresh` that `@@` for a program with only a cheat sheet shows cheat examples, `@@ tar` shows only tldr examples, and `make tldr-ranking` scores are unchanged.
- [x] 4.2 Update `crates/smcli-search/README.md` and the "later cheat.sh" note in `command_source.rs` to describe the priority order; run `make docs`.
- [x] 4.3 Run `make test`; verify it passes.

## Workflow follow-up

- Archive the change after review.
