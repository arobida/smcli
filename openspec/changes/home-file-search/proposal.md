# Proposal

## Why

A file search scoped to `~` only returns entries from `~/.config`. fff refuses to index the home directory unless `enable_home_dir_scanning` is set, so the service fails and the CLI falls back to its in-process walker. That walker goes depth-first with a 10,000-entry budget, and the first dot-folder uses up the budget before any other folder under `~` is reached.

## What Changes

- The service lets fff index the home directory, so a search scoped to `~` (picker scope, or Shift+Tab widened up to `~`) searches the whole home tree with fff's ignore rules, watcher and ranking. This works on macOS and Linux.
- The in-process fallback walker goes breadth-first, so every top-level folder of the scope is listed before any deep subtree uses up the entry budget.
- Results are ranked like `z`, extended to files: directories you work in (from captured command history) and files you picked anywhere under the scope rank higher, whichever project root they were recorded under. A blank query at `~` lists the directories and files you use most.
- In the file picker, Enter keeps inserting the path. Ctrl+Enter (Alt+Enter where the terminal can't tell Ctrl+Enter apart) on a directory changes the shell into it, like `z`. On a file it does nothing for now.
- No config or dependency changes. The pick output gets one new exit code for "change directory", handled by the bash, zsh and fish hooks.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `file-picker-scope`: adds requirements that a search scoped to the home directory covers the whole home tree, that the fallback search reaches every top-level folder of a large scope, that results are ranked by directory and pick frecency across project roots, and that Ctrl+Enter or Alt+Enter on a directory changes into it.

## Impact

- `crates/smcli-service/src/file_search.rs`: one `FilePickerOptions` field.
- `crates/smcli/src/cli/pick.rs`: `LocalFileSearch::walk` becomes breadth-first; its depth, budget and symlink tests keep passing.
- `crates/smcli-core/src/history.rs`: selection scores for every root under a scope, plus directory frecency from `commands.directory`.
- `crates/smcli-service/src/file_search.rs`: blends both into the existing pick boost.
- `crates/smcli/src/picker.rs`, `crates/smcli/src/cli/pick.rs`: new key and exit code; keyboard enhancement flags pushed so Ctrl+Enter is reported where supported.
- `crates/smcli/src/shell_integration.rs`: bash, zsh and fish handle the new exit code with `cd`.
- First search from `~` scans the whole home tree. It returns partial results after the existing 10s scan wait and fills in from the background. Content indexing at `~` stays off, as it is today.
