# Dependency upgrade — 2026-11-12

Findings from a trial upgrade on 2026-11-12, run on a scratch copy of the repo at `d8582dc`. Nothing in the repo was changed. Result: **every upgrade compiles and behaves the same; no reindex is needed.**

## Toolchain

| Item                                        | Current             | Status                                 |
| ------------------------------------------- | ------------------- | -------------------------------------- |
| rustc (Homebrew, `/opt/homebrew/bin/rustc`) | 1.99.1 (2026-10-13) | Latest stable                          |
| rustup stable                               | 1.99.1              | Up to date                             |
| rustup itself                               | 1.28.2              | 1.29.3 available: `rustup self update` |

## What changes

**Semver-compatible (lockfile only, `cargo update`):** 39 crates, all patch or minor. Notable: `redb` 4.4.0 → 4.5.0, `uuid` 1.28.0 → 1.29.2, `notify` 8.2.0 → 8.2.2 and 9.0.0-rc.6 → rc.8, `syn` 3.0.9 → 3.0.10. Two crates are replaced by forks: `serde_yaml_ng` → `serde_norway`, and `unsafe-libyaml` → `unsafe-libyaml-norway`.

**Breaking versions (need `Cargo.toml` edits):**

| Crate                                               | From   | To     | Where declared                                                     |
| --------------------------------------------------- | ------ | ------ | ------------------------------------------------------------------ |
| `rusqlite` (pulls `libsqlite3-sys` 0.38.2 → 0.40.0) | 0.40.2 | 0.42.0 | `crates/smcli-core`, `crates/smcli-service`, `crates/smcli-search` |
| `taladb`                                            | 0.12.x | 0.13.0 | `crates/smcli-search`                                              |
| `model2vec-rs`                                      | 0.3.0  | 0.4.0  | `crates/smcli-search`                                              |

## Checks run

| Check                                                                                                                                                  | Result                                                                                                                                                    |
| ------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cargo check --workspace --all-targets` after `cargo update`                                                                                           | Clean, no errors or warnings                                                                                                                              |
| Same, with each breaking bump alone, then all three together                                                                                           | Clean every time: no code changes needed                                                                                                                  |
| `model2vec-rs` 0.3 vs 0.4 embeddings with `Models/potion-shell-16m`, on 10 inputs (shell commands, unicode, an empty string, a 4,000-character string) | Bit-identical (max difference 0). The new fast WordPiece/Unigram tokenizer changes nothing for this model                                                 |
| `taladb` on-disk format, 0.12.1 vs 0.13.0 source                                                                                                       | `CURRENT_SCHEMA_VERSION` still 2, index type tags and key encoding unchanged. 0.13 adds a sorted-index query path and NUL-escaping docs, not a new format |
| Cross-version run: old binaries build the index and `history.db` from about 400 fake zsh history lines; new binaries then run 4 searches               | Identical results. The new service reused `rs-commands-v4-potion-shell-16m-…-d256.taladb` without rebuilding it, and read the old `history.db` fine       |

## Not covered

- The test suite was not run (kteam runs tests).
- Real user history; only synthetic history was used.
- `notify` (file watching) at runtime.
- YAML parsing after the `serde_norway` swap. It compiles, but nothing above exercised it.
- Changelogs: none of the three breaking crates ships one, so the review was a source diff plus the runtime checks above.

## Upgrade steps

1. `wt switch --create deps-2026-11`.
2. In `crates/smcli-core/Cargo.toml`, `crates/smcli-service/Cargo.toml` and `crates/smcli-search/Cargo.toml`, set `rusqlite` to `0.42.0`. In `crates/smcli-search/Cargo.toml`, set `taladb = "0.13"` and `model2vec-rs = "0.4.0"`.
3. `cargo update`, then `make notices` (licenses for the new forks).
4. kteam runs the focused slices: `make test-rust PKG=smcli-search`, `make test-service`, `make test-picker`. Then `make test` once.
5. `make ranking` should match the current scores, since the embeddings are identical.
6. `make fresh`, try search and the picker on real history, then commit through `wt step commit` and `wt merge`.
