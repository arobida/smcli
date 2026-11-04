# Proposal

## Why

File and grep picker rows show paths in full and the terminal cuts the end, so in deep trees the part that tells rows apart (the file name) is what disappears: `services/backend/internal/handlers/user_pro` instead of `services/back…/handlers/user_profile.go`.

## What Changes

- Paths in file and grep picker rows that do not fit are shortened in the middle, keeping the file name and as much of the leading directories as fit, joined by `…`.
- In grep mode the path gets a bounded share of the row so the matched line text stays visible.
- Display only: the inserted path, scope, sorting and git/provenance marks are unchanged.

## Capabilities

### New Capabilities

### Modified Capabilities

- `file-picker-scope`: adds how long paths are shown in file and grep rows.

## Impact

- `crates/smcli/src/picker.rs` (file/grep row rendering; the end-cut loop in `history_row` is the existing width-measuring pattern).
- Picker unit tests; `make test-picker` for the interactive check.
