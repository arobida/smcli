# Proposal

## Why

In a new or rarely used project, suggest and search only know "this directory", "this project" and "everywhere". Habits from other projects of the same kind (another Rust crate's `cargo nextest run`, another Node package's `pnpm test`) rank no higher than unrelated commands. smcli already detects the project kind (`context.rs`, `MANIFESTS`) but ranking ignores it.

## What Changes

- Add a same-kind ranking tier: runs from a directory whose project has the same primary kind as the cwd's project get a boost below "in this project" and above everywhere else, in suggest and search.
- Add a `kind` switch to `SMCLI_RANKING_OFF` so the tier can be measured on and off.
- Add same-kind cases to the ranking benchmark (`crates/smcli-search/tests/ranking.rs`).
- The change only ships if it is proven by benchmark: the ranking benchmark improves on the new cases without lowering any existing floor, and `make baseline` shows no warm latency regression.

## Capabilities

### New Capabilities

### Modified Capabilities

- `history-search-ranking`: adds the same-kind tier, its ablation switch, and the benchmark gate for it.

## Impact

- `smcli-core`: `ranking.rs` (`RankingContext`, `Signal`), `history.rs` (in-process suggest weights), `suggestion_index.rs` (service suggest weights), `context.rs` (kind lookup for past run directories).
- `smcli-search`: `corpus.rs` (search boost, a same-kind run count beside `runs_project`), `tests/ranking.rs` (new cases).
- `smcli-service`: caches kinds of run directories alongside the suggestion index.
- No schema change, no protocol change, no shell hook change.
