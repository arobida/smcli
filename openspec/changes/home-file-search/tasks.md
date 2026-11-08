# Tasks

## 1. Service: index the home directory

- [x] 1.1 Set `enable_home_dir_scanning: true` in the `FilePickerOptions` built in `crates/smcli-service/src/file_search.rs` and verify it compiles with `make test-rust PKG=smcli-service FILTER=file_search` (run by kteam)
- [x] 1.2 Add a service unit test: temp `HOME` with `.config/` holding many files and `Documents/report.txt`; a scoped files search for `report` from `HOME` returns `Documents/report.txt`. Verify with `make test-rust PKG=smcli-service FILTER=<test name>` (run by kteam)

## 2. CLI: breadth-first fallback walker

- [x] 2.1 Rewrite `LocalFileSearch::walk` in `crates/smcli/src/cli/pick.rs` as a breadth-first queue, keeping the budget, depth cap of 32, `.git` skip and no symlink following; verify the existing `fallback_walker_*` tests pass with `make test-rust PKG=smcli FILTER=fallback_walker` (run by kteam)
- [x] 2.2 Add unit tests: (a) scope with `a/` holding a few children over a deep subtree past the budget, and `b/target.txt`: the walk reaches `b/target.txt`; (b) every entry at depth n is visited before any entry at depth n+1. Verify with `make test-rust PKG=smcli FILTER=fallback_walker` (run by kteam)

## 3. Frecency across roots

- [x] 3.1 In `crates/smcli-core/src/history.rs`, add scope-wide selection scores (all roots under the scope, rebased) and directory frecency from `commands`, skipping missing dirs, using the literal range predicate from design.md (no `LIKE`); unit-test both with a temp DB, including scopes with `_` and `%` in the name, a case-distinct sibling (`Project_One`), the scope itself, and `/`, via `make test-rust PKG=smcli-core FILTER=history` (run by kteam)
- [x] 3.2 Have the service's `scores` callback merge both into the `picked` map, and add frecent entries missing from fff's first page on a blank query; add a service test where a pick made under a project root lifts that file at scope `~`. Verify with `make test-rust PKG=smcli-service FILTER=file_search` (run by kteam)
- [x] 3.3 Run `make ranking` and `make baseline` and confirm no regression against the previous results (run by kteam)

## 4. Ctrl+Enter / Alt+Enter jump

- [x] 4.1 In `crates/smcli/src/picker.rs`, always push keyboard enhancement flags (popped on every exit, including panic), and on Ctrl+Enter or Alt+Enter with a directory selected return a new jump choice; a file selection is ignored. Unit-test the key handling via `make test-rust PKG=smcli FILTER=picker` (run by kteam)
- [x] 4.2 In `crates/smcli/src/cli/pick.rs`, print the absolute directory and exit 12 for the jump choice, recording the pick like Enter does; verify with `make test-rust PKG=smcli FILTER=pick` (run by kteam)
- [x] 4.3 In `crates/smcli/src/shell_integration.rs`, handle exit 12 in bash, zsh and fish with `cd --` and a prompt redraw, leaving the line unchanged; extend the shell-init text tests and verify with `make test-shells` (run by kteam)
- [x] 4.4 Document Enter vs Ctrl+Enter/Alt+Enter in the picker help or docs through iwe, then `make docs`

## 5. Integration

- [ ] 5.1 After all slices pass, run `make test-picker` and then the full `make test` and `make linux` once (run by kteam)
- [ ] 5.2 `make fresh`, open the picker in `~`: confirm results outside `~/.config`, frecent dirs first on a blank query, and Alt+Enter changes into a directory in zsh

## Workflow follow-up

- Archive the change after review.
