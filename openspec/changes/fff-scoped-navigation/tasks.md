# Tasks

## 1. Protocol

- [x] 1.1 Add optional `scope` (bool) and `path` (string) to the files/grep request in `service_protocol.rs`, bump version, add frozen fixtures; verify `make test-rust PKG=smcli-core FILTER=service_protocol`.

## 2. Service

- [x] 2.1 `FileSearch::search`: when `scope` is set use `directory` as the root (prefix-filter an already-indexed ancestor root instead of a new instance); verify with a unit test searching a subdirectory and a parent of the repo.
- [x] 2.2 Grep `path` filter restricting hits to one file; unit test.
- [x] 2.3 Disable content indexing for roots at or above `~`'s direct children outside a repo; unit test on the options chosen.
- [x] 2.4 `relative_insert` relative to cwd, then `~/`, then absolute; unit test covering all three.

## 3. Picker

- [x] 3.1 Replace `global_files`/`local_files` with a `scope` path initialised to the project root; remove `^A` and its hint; update/remove `grep_is_only_available_outside_global_search`; verify `make test-rust PKG=smcli`.
- [x] 3.2 `Tab` on a directory: scope = that dir, clear query; `Tab` on a file: grep mode with `path`; test key handling.
- [x] 3.3 `BackTab`: scope = parent, stop at `~`; from single-file grep, return to saved file scope; test including the `~` stop and a scope outside `~`.
- [x] 3.4 Header shows cwd and scope (`~/` abbreviated); hint line shows `tab narrow`/`⇧tab up`; snapshot/render test.

## 4. Fallback and integration

- [x] 4.1 In-process fallback in `main.rs` honors scope and single-file grep; verify with the service stopped in `make test-shells`.
- [x] 4.2 Full gate: `make test` and `make ranking` pass; `make fresh` and relay the eval line for manual try-out.
