# Proposal

## Why

The service already keeps a warm fff index for every project root it serves (path index, git status, watcher, grep). Today only `smcli files`, `grep` and `select` use it. History search and suggestions still judge a command by `stat`-ing a few path arguments under an 8 ms gate, and they can't see the project's code. With fff, ranking can check every candidate's paths and identifiers against the current project for little extra cost.

## What Changes

- Service builds fff instances with content indexing on and grep with definition classification, so grep answers come from the bigram index and mark definitions.
- File-presence evidence comes from the fff index when the service has a warm instance for the cwd's root, covering every candidate in the pool. The existing `stat` probe stays the fallback.
- New boost for commands whose path arguments are modified, staged or untracked in Git, or were touched in recent commits.
- New project-evidence boost: identifiers in a candidate command (test names, symbols, targets) found in the project, more when the match is a definition. One multi-pattern search per request.
- New suggestions from test definitions in the project (for example `cargo test <name>`), alongside the existing manifest script candidates.
- New `@` path completion: a word starting with `@` completes to project files or directories through fff's fuzzy search, or through a glob when the word holds glob characters. `*` and `+` are not triggers: `*` is a live shell glob, and `+` already starts words like `cargo +nightly` and `chmod +x`.
- Semantic query expansion: when history hits are weak, grep the project for the query terms and add the matching file and definition names as extra search terms for a second pass over history.
- Every new signal is service-only, additive, has an ablation switch, and never demotes.

## Capabilities

### New Capabilities

- `project-index-signals`: ranking, suggestion and query-expansion evidence the service draws from its file index of the current project.
- `path-completion`: `@`-triggered completion of project paths on the command line.

### Modified Capabilities

None. The new signals sit within the existing rules in `shell-context-signals` (unknown is neutral, signals can be disabled) and `history-search-ranking` (service-only signals are allowed when documented; benchmark floors hold).

## Impact

- `crates/smcli-service`: `file_search.rs` (instance options, grep options, new lookup/evidence entry points), `service.rs` (passing evidence into search and suggest).
- `crates/smcli-search`: `filesystem.rs` and `corpus.rs` take an optional index-backed evidence source; query expansion in search.
- `crates/smcli-core`: new `Signal` variants and `SMCLI_RANKING_OFF` names; protocol fields if completion goes over the socket (new fixtures).
- `crates/smcli`: `@` completion in `shell_integration.rs` hooks and the picker.
- Memory: content index costs about 360 B per indexed file per root (fff README).
- Ranking: new fixture cases in `crates/smcli-search/tests/ranking.rs`; `make ranking` floors must hold.
