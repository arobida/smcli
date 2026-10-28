# Proposal

## Why

The file picker opens on a one-level listing of the cwd, so most project files are out of reach until the user presses `^A`, and that "global" search only covers the project root, not the user's system. Scope should be something the user moves through with the keys they already use, not a toggle between two fixed places.

## What Changes

- The file picker opens searching the whole project (repository root, or the cwd outside a repository) with fff, ranked as today.
- **BREAKING**: `^A` (cwd/global toggle) is removed. The one-level cwd listing mode goes away.
- `Tab` on a selected directory narrows the search scope to that directory.
- `Tab` on a selected file switches to grep restricted to that one file.
- `Shift+Tab` widens the scope to the parent of the current scope, one level per press, stopping at `~`. Scopes outside `~` are never entered by widening.
- The picker header shows both the cwd and the current search scope.
- Paths inserted on the command line stay relative to the cwd when possible, absolute (or `~/`-prefixed) otherwise.

## Capabilities

### New Capabilities

- `file-picker-scope`: how the interactive file/grep picker chooses, narrows, widens and displays its search scope.

### Modified Capabilities

## Impact

- `crates/smcli/src/picker.rs`: remove `^A`/`global_files`/local listing path, add scope state, `Tab`/`Shift+Tab` handling, header and hint lines, tests.
- `crates/smcli/src/main.rs` (files callback): send the scope directory instead of the cwd.
- `crates/smcli-service/src/file_search.rs`: search an explicit scope directory as an fff root (not snapped to the repository root when the user chose it); grep accepts an optional single-file path filter; `relative_insert` relative to the cwd.
- `crates/smcli-core/src/service_protocol.rs` + frozen fixtures: optional `scope` and `path` request fields (version bump).
- In-process fallback must honor the same scope and single-file grep.
- fff indexing `~` is a large scan; bounded by the existing instance pool (capacity/idle eviction).
