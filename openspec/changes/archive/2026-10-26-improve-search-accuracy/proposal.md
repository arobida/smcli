# Proposal

## Why

smcli's ranking benchmark has 34 cases (16 of them search). That's too few to show whether a ranking change helps or hurts. Search also has one confirmed recall gap: the lexical index is built from the same text as the embedding, which is cut at 1,500 characters, so nothing in a long command's tail can be found. The user also wants to know whether their aliases, PATH and smcli's fff file index can improve results. Today none of them feed ranking.

## What Changes

- Expand `make ranking` with adversarial and held-out cases, reported per cohort, so later changes can be measured honestly.
- Index the full eligible command lexically. The embedding text keeps its 1,500-character cap. Bump the index format version.
- Add a bounded bonus for exact whole-shell-token matches in literal queries (`--soft` vs `--hard`, `HEAD~1` vs `HEAD~10`), only if the new cases show failures.
- Emit camelCase/PascalCase parts as extra lexical terms, alongside the full tokens.
- Investigate three shell-context signals, each a time-boxed spike that ends in a written go/no-go with benchmark evidence:
  - live-shell aliases/abbreviations as an extra form of the query
  - PATH/command availability as a weak, positive-only boost
  - fff's warm file index (service only) and plain filesystem checks of a command's path arguments
- Ship only the signals the spikes approve, each behind its own ablation switch.

## Capabilities

### New Capabilities

- `history-search-ranking`: how search retrieves and orders history commands for literal and intent queries, and how ranking quality is measured.
- `shell-context-signals`: which signals from the user's live shell and working directory may influence ranking, and the privacy, freshness and fallback rules they must follow.

### Modified Capabilities

## Impact

- `crates/smcli-search` (`corpus.rs`, `index.rs`, `tokenizer.rs`, `tests/ranking.rs`). The index rebuilds once after the format version bump.
- Only if the spikes approve them: `crates/smcli/src/shell_integration.rs` (hooks for zsh/bash/fish), `crates/smcli-core/src/service_protocol.rs` plus its frozen fixtures, and `crates/smcli-service/src/{service,file_search}.rs`.
- History DB schema 8 adds a nullable `expanded_command` column for record-time alias expansion. Native history files stay read-only. The CLI and smcli-core still never link fff.
