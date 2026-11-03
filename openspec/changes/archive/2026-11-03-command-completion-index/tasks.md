# Tasks

## 1. Source abstraction

- [x] 1.1 Add `CommandSource` trait + `CommandExample` to `smcli-core`; doc-test or unit test with a stub source (`make test-rust PKG=smcli-core`).
- [x] 1.2 Port tldr to a `CommandSource` in `smcli-search` (platform filter kept inside it); existing `crates/smcli-search/tests/tldr.rs` eval passes unchanged.

## 2. Program set + index

- [x] 2.1 Enumerate OS-shipped programs from system dirs intersected with PATH; unit test with temp dirs standing in for system and non-system dirs.
- [x] 2.2 Build the command-completion index from that set via all sources, named by model + source versions; test that a program with no examples is absent and a foreign-platform tldr page is excluded (`make test-rust PKG=smcli-search`).
- [x] 2.3 Switch `pick_tldr` single and all views to the index; results show source name; `make test-picker` passes.
- [x] 2.4 Remove `.vec` cache code; `smcli indexes` lists the index, `--prune` removes old `tldr-*.vec` (`make test-cli`).

## 3. Discovery

- [x] 3.1 In service `record`, add pages for an unindexed program on the request's PATH, off the reply path; test: record `jq ...` with fake `jq` on PATH → bare `@@` returns jq examples (`make test-rust PKG=smcli-service`).
- [x] 3.2 Remember programs with no examples so repeat captures skip lookup; test asserts no index write.
- [x] 3.3 In-process fallback when no service; protocol bump + fixture if a new request is needed (`make test-rust PKG=smcli-core`).

## 4. Service-owned sync

- [x] 4.1 Add `CommandIndex::remove(program)` and a `sync(index, sources, platform, wanted)` that adds missing and deletes unwanted programs; unit test add + delete (`make test-rust PKG=smcli-search`).
- [x] 4.2 Service: replace per-program discovery with a background sync triggered on unknown program or changed PATH, one at a time; test uninstall (fake exe removed) deletes examples (`make test-rust PKG=smcli-service`).
- [x] 4.3 `pick_tldr`: remove in-process `discover` and the on-demand page add; only build when the index is missing; `make test-picker`.

## 5. Verify

- [x] 5.1 `make test` and `make test-shells` pass; `make fresh`, then `@@` on a built-in (`tar`) and on a newly installed program after running it once.
