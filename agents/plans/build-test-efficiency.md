# Build and test efficiency plan

## Scope

Make the edit/test loop faster and the existing targets correct, using the current Makefile, Cargo selection and existing harnesses. No new crates, test framework, packaging system or compiler tuning. `make test` stays the complete gate; `make fresh` stays the user-acceptance build (AGENTS.md).

Review note (2026-10-24): the original draft proposed eleven new targets, pair-atomic publishing, provenance records and a multi-sample benchmark protocol. Cut to what fixes a real hazard or saves real time. Claims below were spot-checked against the Makefile, `scripts/test.sh` and `ranking.rs`.

## Work items, in order

### 1. Test isolation

`scripts/test.sh` sets only `SMCLI_DB` and `SMCLI_RUNTIME_DIR` globally; `HOME`/`HISTFILE` are per-invocation. An inherited `SMCLI_*` or `XDG_*` variable can change results, and `SMCLI_RANKING_OFF` silently disables the ranking floors (`crates/smcli-search/tests/ranking.rs:332`).

- At the top of `scripts/test.sh`, unset every `SMCLI_*` and `XDG_CONFIG_HOME`/`XDG_DATA_HOME`/`XDG_STATE_HOME` before anything runs, so Cargo tests and every harness inherit a clean environment. Leave `HOME`, `PATH`, `CARGO_*`, `RUSTUP_*` alone.
- Do the same in `scripts/test-service.py` and `scripts/test-install.sh` so they are safe when run on their own (item 4).
- Check: `SMCLI_RANKING_OFF=project SMCLI_CONFIG=/nonexistent make test` still passes and the ranking floors run.

### 2. Linux recipe

- Add `sqlite3` and `git` to the apt install (`scripts/test-install.sh` needs the `sqlite3` executable).
- Add a volume for `/usr/local/cargo/git` (fff is a git dependency).
- Pin the image to a specific `rust:1.NN` tag instead of `rust:latest`.

### 3. Release and baseline use the real target directory

`release` copies from `target/release/` even when `CARGO_TARGET_DIR` points elsewhere, which can publish stale binaries.

- Use `$${CARGO_TARGET_DIR:-target}/release` in `release`; `cp` already fails if a file is missing.
- `baseline` should depend on `cargo build --release` and run against that target path, not republish `dist/`.

### 4. Focused test targets

Expose the existing harnesses so one can be run without the whole gate. Each assumes `make build` output.

| Target         | Runs                                                                                                            |
| -------------- | --------------------------------------------------------------------------------------------------------------- |
| `test-rust`    | `cargo test $(if $(PKG),-p $(PKG),--workspace) $(FILTER)` (e.g. `make test-rust PKG=smcli-core FILTER=history`) |
| `test-shells`  | `build` + `scripts/test-shells.py` (honors existing `SMCLI_TEST_ONLY`)                                          |
| `test-service` | `build` + `scripts/test-service.py`                                                                             |
| `test-install` | `build` + `scripts/test-install.sh`                                                                             |

Move the inline CLI checks in `scripts/test.sh` only if a harness needs the shared setup; otherwise leave them. `test` keeps running everything once.

### 5. Measure, then optimize only what is slow

One warm run of each stage (2026-10-24, Apple M-series macOS, Rust 1.99.1; the full `make test` ran immediately before, so caches were warm):

| Stage                         |  Time |
| ----------------------------- |------:|
| `cargo test --workspace`      |   12s |
| `cargo build --workspace`     |    0s |
| `scripts/test-shells.py`      |  150s |
| `scripts/test-service.py`     |   81s |
| `scripts/test-file-picker.py` |   43s |
| `scripts/test-install.sh`     |   22s |
| `make test` total             | 5m14s |

The inline CLI checks and the 10-row baseline sample in `scripts/test.sh` account for the ~7s remainder. No change justified: `test-shells` is inherent PTY interaction across many shell variants (its sleeps are all sub-second waits), the 50,000-file scenario is a minority of `test-service`'s 81s and `make test` would still run an opt-in `test-scale`, and the remaining sleeps test idle expiry and timing, not readiness. Revisit only with a measured reason.

### 6. Docs

Update AGENTS.md Building section with the new targets (through iwe, then `make docs`). Fix the stale `rs-ranking` mention in the `ranking.rs` header and obsolete targets in `agents/reference/baseline.md`.

## Out of scope

Pair-atomic dist publishing, dirty-tree provenance, toolchain pin file, `--locked`, LTO/linker/cache changes, logical slices (`test-sqlite`, `test-index`, `test-model`, `test-ui`) — `make test-rust PKG=… FILTER=…` covers those. Revisit only with a measured reason.

## Acceptance

- `make test` passes, with a poisoned environment too (item 1 check).
- `make linux` passes.
- `CARGO_TARGET_DIR=/tmp/x make release` publishes the binaries built there.
- Each new target runs a nonzero set of tests and fails on failure.
- Timing table from item 5 recorded.
