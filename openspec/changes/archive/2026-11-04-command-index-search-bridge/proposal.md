# Proposal

## Why

Natural-language queries ("undo last commit") often miss history rows like `git reset --soft HEAD~1` because they share no words and the small static model does not reliably bridge English to shell. The tldr/cheat command index already maps English descriptions to command templates, but only `@@` uses it.

## What Changes

- Intent queries look up the nearest command-index descriptions and search history again using the matched templates as command-shaped queries (template-as-query), merging those hits into the candidate pool.
- History candidates that share program, subcommand or flags with matched templates get an affinity bonus in reranking, on both intent and literal queries; on literal queries the bonus never lifts a row above an exact keyword hit.
- Matches only recall or rerank rows already in history; tldr templates are never returned as results.
- The bridge is a service-only signal: without the service, with the index lock held, or with no command index built, search behaves as it does today.
- The `semantic-search` intent retrieval requirement is corrected to describe the vector-first path that ships.
- The ranking benchmark gains a natural-language cohort.

## Capabilities

### New Capabilities

### Modified Capabilities

- `semantic-search`: intent retrieval rewritten to match shipped behavior; adds template-as-query recall from the command index.
- `history-search-ranking`: adds the command-index affinity bonus as a documented service-only signal and a natural-language benchmark cohort.

## Impact

- `crates/smcli-search` (`index.rs` intent and literal paths, `commands.rs` description lookup, `tests/ranking.rs`).
- `crates/smcli-service` opens the command index alongside the history index during search.
- No protocol, CLI flag, or index format change; no new dependency.
