# Tasks

## 1. Helper

- [x] 1.1 Add `truncate_middle(path, room)` in `picker.rs`; verify unit tests: fits unchanged, deep path keeps start and `/file`, last component longer than room is end-cut, double-width characters never exceed room, room 0 and 1.

## 2. Rows

- [x] 2.1 Use it for the path span in file and grep rows, with room computed from the row's other spans and the grep half-row cap; keep LS_COLORS from the full name; verify a render test at a fixed width for a file row and a grep row.
- [x] 2.2 Verify picking a shortened row inserts the full path with `make test-picker`.

## 3. Integration

- [x] 3.1 Run `make test-rust PKG=smcli` and `make test-picker`; verify both pass.

## Workflow follow-up

- Archive the change after review.
