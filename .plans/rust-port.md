# Rust port plan

Port smcli from Swift to Rust in four phases. Each phase ends with something that builds and passes its tests.

## Why port at all

The Swift binaries do everything the plan below does. The one payoff the Rust port adds is a Linux build with search included. If Linux is not a real need, stop here and don't port. If it is, the Linux proof moves to Phase 1 (below) so the port can't fail on its only payoff at the end.

## Decisions

- **Embeddings:** `model2vec-rs` for Potion. No ONNX, Candle or fastembed (Potion is a static lookup table). Potion only; Ollama and Apple embedders are not ported until someone asks. One embedder means one struct, no `TextEmbedder` trait; extract the trait when the second embedder arrives.
- **Model files:** `include_bytes!` the three potion files and write them to the runtime dir on first run (one `fs::write` each), then load from that path. `model2vec-rs` loads from a directory; don't spend time checking for a from-bytes API.
- **Index:** keep TalaDB, called natively from Rust (`taladb` crate, 0.12.x). No SQLite FTS5, no `sqlite-vec`. The Rust crate and taladb-swift share the same engine, but the Swift wrapper is 0.1.x and the crate is 0.12 with a `legacy-migration` feature for pre-0.11 files. Treat the index as a cache: on open failure or format mismatch, delete and rebuild from `history.db`. No migration code.
- **Ranking:** TalaDB handles retrieval and filtering (`indexFilter`, `hybridSearch` RRF). `rerank`, frecency, `SearchSignals`, `isLiteralQuery` and the 0.7/0.3 literal-query fusion stay in Rust code.
- **Picker:** `ratatui` (uses its re-exported `ratatui::crossterm`, so no separate crossterm dependency) plus `nucleo-matcher`.
- **No Tokio:** std `UnixListener` with threads.
- **Errors:** `anyhow` everywhere. No `thiserror`: there is no external library consumer.
- **Paths:** `$HOME/.local/share/smcli` via `format!`. No `etcetera`, no `directories` (on macOS it would move the DB to `~/Library`).
- **Peer uid:** `UnixStream::peer_cred()` from std. No `nix`.
- **Native shell history stays read-only.** `history.db` keeps its schema and WAL mode.

## Crates

| Need | Crate |
|---|---|
| Potion embeddings | `model2vec-rs` |
| Index | `taladb` (Rust engine) |
| History DB | `rusqlite` (`bundled`) |
| CLI | `clap` 4 (derive) |
| Picker | `ratatui`, `nucleo-matcher` |
| Protocol / config | `serde`, `serde_json` |
| flock, chmod, peer uid | std (`File::lock`, `PermissionsExt`, `UnixStream::peer_cred`) |
| File watching | `notify` (only if the service still watches native history) |
| File search | `fff-search`, pinned at the same v0.11.0 revision `scripts/build-fff.sh` uses; default features only (`ripgrep`) |
| Errors | `anyhow` |
| CLI tests | `assert_cmd`, `tempfile` |

Dropped from the first draft: `ureq`, `objc2-natural-language`, `etcetera`, `nix`, `thiserror`. Add any of them back only when a feature needs it.

Add every crate with `cargo add` to get current versions.

## Notes from the dependency wikis

**fff (`fff-search`)**
- Entry points: `FilePicker` (indexing, background watcher, fuzzy + frecency search, `file_search`) and `SharedFilePicker` (`Arc<RwLock<Option<FilePicker>>>`) for cross-thread use. The service owns one `FilePicker`; `SharedFilePicker` is unnecessary with a single owner thread, use it only if connection threads call search directly.
- Grep lives in the `grep` module (regex, plain and fuzzy modes, constraint filtering).
- Features: `default = ["ripgrep"]` (pure-Rust `ignore` + `globset`). Do not enable `zlob` (needs Zig), `ffi` (the C ABI we're deleting), `mimalloc-collect` or `definitions`.
- Native deps: `git2` with `vendored-libgit2`, so no system libgit2, but it builds libgit2 from source on first compile. Expect a slow cold build; that is the "build time" risk. zlib comes via libgit2.
- Linux is in its CI. Good for the Linux payoff.

**TalaDB (`taladb`)**
- API: `taladb::open(path)`, `db.typed::<T>("name")` for serde structs (`IndexedCommand`) or `db.collection("name")` for `taladb::Value`; `create_vector_index(field, dims, None, None)`, `create_fts_index(field)`; `hybrid_search(HybridQuery { text field, vector field, query, filter })` on `.raw()`; filters are `serde_json::json!` objects, same shape as the Swift `indexFilter`.
- `Database` is cheap to clone and thread-safe in-process. No read-only or multi-process open is documented. So the Phase 2 lock decision is already made: when the service holds the index, the in-process fallback skips semantic search and does BM25-less text matching over `history.db` only. Do not build a second index reader.
- Builds on Linux (ships `libtaladb_ffi.so`).

**semble (reference for Potion usage)**
- Loads `potion-code-16M-v2` once and caches the loaded model; encode in one batched call, `float32`, no multiprocessing. Mirror that: load once per process, embed in batches at index-sync time.
- Normalizes vectors before cosine dot products. Store normalized vectors in TalaDB so cosine and dot product agree.
- Hybrid = RRF with `k = 60` over semantic and BM25 rankings, then an `alpha_weight` blend chosen by query type, then code-tuned reranking (identifier boosts). This is the same shape as our `isLiteralQuery` → 0.7/0.3 fusion → `rerank`, so keep ours; it is already the proven pattern.
- Persists the index with a `metadata.json` holding the model path, and reuses vectors for unchanged inputs. Our fingerprints already do this; add the model name/version to the index metadata so a model change triggers a rebuild.

---

## Phase 1: Workspace, embeddings, Linux proof

**Goal:** Rust vectors match the Swift vectors, and the two risky crates link on Linux.

1. Create a Cargo workspace beside the Swift package with crates `smcli-core`, `smcli-search`, `smcli` (binary) and `smcli-service` (binary).
2. Add `make` targets for the Rust build (`make rs-build`, `make rs-test`). Keep the existing targets working.
3. Embed the potion-code-16M-v2 files with `include_bytes!`, write them to the runtime dir on first use, load with `model2vec-rs`.
4. Potion embedder struct (no trait).
5. Port the `reference.json` tests. **Exit criterion:** the vectors match within tolerance.
6. Port `ShellTokenizer` with its tests.
7. Add `taladb` and `fff-search` as dependencies of a stub `smcli-service` and `cargo build` on Linux (container or CI). **Exit criterion:** both link. If they don't, the port's payoff is gone; stop and reassess before Phase 2.

## Phase 2: Core data and search

**Goal:** `search` and `suggest` run in-process, with ranking parity.

1. Port the `history.db` schema, capture filters and config loading with `rusqlite` and `serde_json`. Same paths, same schema.
2. Port the read-only native-history readers.
3. Open TalaDB natively. Port `IndexedCommand`, the index sync and fingerprints, and `SearchFilter::index_filter`. Record the model name in index metadata; rebuild on mismatch or open failure.
4. Port the search path from `HistorySearch.swift:495`: `hybridSearch` for normal queries; `searchText` + `findNearest` with the 0.7/0.3 weighted sum for literal queries; then `signals.additions` and `rerank`.
5. Port `RankingQualityTests` and run them against the fixed query set. **Exit criterion:** scores at or above the Swift baseline (`make ranking`).
6. When the service holds the TalaDB lock, the fallback skips semantic search (decided above).

## Phase 3: Service and protocol

**Goal:** the Rust service is a drop-in replacement for the Swift service. This is also the cheapest exit: if Phase 3 passes, the Rust service can ship with the Swift CLI and Phase 4 can wait.

1. Port `ServiceProtocol` with serde: newline-delimited JSON, one request per connection, versioned. Wire-compatible with the Swift client.
2. Build `smcli-service` on std `UnixListener` with a thread per connection. Port the runtime-dir setup (chmod 700, `peer_cred()` uid check, flock).
3. Port the binary identity check (path/mtime/size) so a rebuilt binary is detected.
4. Link `fff-search` in `smcli-service` only, and port `FileSearch` onto `FilePicker` and the `grep` module. Delete the `SmcliFFF` C wrapper and `scripts/build-fff.sh`.
5. Port `FileWatcher` with `notify`, if it's still needed.
6. Test: the Swift CLI against the Rust service, then the Rust client against the Rust service. **Exit criterion:** `scripts/test-service.py` passes.

## Phase 4: CLI, picker and cutover

**Goal:** the Rust binaries replace the Swift ones.

1. Port the `clap` subcommands one to one with the Swift CLI, including the in-process fallback for each operation.
2. Port `shell-init` hook generation (`ShellIntegration.swift`). Add a `diff <(swift shell-init) <(rust shell-init)` check to `scripts/test.sh`; it must be empty for zsh.
3. Build the picker with `ratatui` (inline viewport) and `nucleo-matcher`: history, file, grep and directory modes; key bindings (Left, Up, Ctrl-R).
4. Point `scripts/test.sh` at the Rust binaries (temp `HOME`, `SMCLI_DB`, `SMCLI_RUNTIME_DIR`). Port to `assert_cmd` only if the shell script becomes a problem.
5. Point `make fresh` / `make release` at the Rust build, output to `dist/smcli-<timestamp>-<commit>`, and print the `eval` line.
6. Linux: full build and `scripts/test.sh` (search included now; no Apple embedder).
7. Remove the Swift sources, `Package.swift` and the TalaDB xcframework. Update `AGENTS.md`. Run `graphify update .`.

---

## Risks to check early

- `taladb` and `fff-search` linking on Linux (Phase 1, step 7). Resolved by the wikis as "should", not yet proven here.
- `fff-search` cold build time (vendored libgit2).
- `taladb` 0.12 API vs. the taladb-swift 0.1.x surface we read; the wiki confirms the shape (`hybrid_search`, JSON filters) but not every method name.
