# Architecture remediation

## Scope

This plan fixes every finding from the 2026-11-04 architecture audit (H1–H3, M4–M8, L9–L12; summarized in [Finding → work package](#finding--work-package)). The crate layout and the client/service split stay as they are. The work is at the lifecycle edges:

- who loads the model;
- what names a model;
- who writes storage;
- what environment the service acts on;
- how requests are validated;
- housekeeping.

It absorbs the earlier service-only model ownership plan (formerly `service-only-model-ownership.md`) as [Phase A](#phase-a--service-only-model-ownership-h1), unchanged except for renumbering and the protocol additions noted in A2.

Baseline: `6f0e7b0` (`commands::ensure` heals a missing `@@` program) is in place; A1 moves that logic into `commands::prepare` without rewriting it.

Phases land in order (A, B, C, D, E, F). Each phase is one or more commits that pass their own gate. `cli.rs` is edited in A4, B1, C3 and D1, and is split last (F2), so the split moves settled code once.

## Locked decisions

1. **`smcli-service` is the only runtime owner of Potion.** The CLI never builds a model (`StaticModel`) for search, the history picker, `@@`, `indexes` or `health`. It may read model files to compute their identity (header and content hash), but it never loads them into a model. `smcli init` (only when the model changes) and `smcli reindex` load the model, but only after stopping the service and taking the **model lock**, and they refuse if the lock stays held.
2. **No dedicated embed daemon.** `@@` query embedding is a protocol v4 `embed` op on the existing service.
3. **When the service is unusable**, search degrades to keyword matching (`lexical_search`) and `@@` to word ranking (`tldr::rank_words`). The CLI never embeds in-process. "Unusable" means down, failed to start, `SMCLI_SERVICE=0`, `connect` with no service, `busy`, timeout, `ok:false` or another protocol version.
4. **One protocol bump (v3 → v4), in A2.** It carries every wire addition this plan needs: the `embed` op, the `embedding` and `model` response fields, and the `home` request field. Later phases (caller context, typed requests, failure codes) change no wire format.
5. **Model identity is a content hash** of weights, tokenizer and config, bundled included. Index names change once, and legacy indexes are cleaned up automatically.

## Finding → work package

| Finding | Summary                                                                                                                              | Work packages                    |
| ------- | ------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------- |
| **H1**  | CLI fallback, `indexes`, `health` and `init` load a second model while the service holds one                                         | A1–A4                            |
| **H2**  | Identity is `"bundled"` or a `DefaultHasher` path hash, not contents; retrained models mix vectors; a toolchain bump orphans indexes | B1                               |
| **H3**  | CLI runs raw SQL on history tables and opens and deletes TalaDB index files; `smcli` links `taladb` and `rusqlite`                   | C1–C3                            |
| **M4**  | Service acts on the launching shell's environment for life; per-request overrides are piecemeal, so service and fallback diverge     | D1–D2                            |
| **M5**  | Service build identity ignores the model folder; a model-only reinstall stays stale                                                  | B2                               |
| **M6**  | Engine cache is keyed on the whole `SmcliConfig`, so a capture-filter edit reloads the model                                         | B2                               |
| **M7**  | Flat `ServiceRequest` bag; validation spread across handlers; failure codes are bare strings                                         | E1                               |
| **M8**  | Dead older-client branches (`config_path` fallback) unreachable under exact-version matching                                         | E1 (with D1's required `config`) |
| **L9**  | Fixture README stale (op count, v3 file); no end-to-end test that old versions are rejected                                          | A2 (README), E1 (rejection test) |
| **L10** | `relative_insert` in `service_location`; `notify` and `ContextCache` in core although service-only; Swift remnants                   | F1                               |
| **L11** | `cli.rs` is a 2,168-line god module                                                                                                  | F2 (after A–D settle it)         |
| **L12** | Service holds the launching shell's whole environment, secrets included                                                              | D2                               |

## New and changed invariants

- **Model ownership (H1):** the CLI reads a model's identity (safetensors header and content hash) and never builds the model, except in `smcli init` when switching models and in `smcli reindex`, both after stopping the service and taking the model lock. Every runtime embedding happens in `smcli-service`. Two checks enforce this:
  - **Static:** all CLI model loads live in `crates/smcli/src/initialize.rs`. A unit test in the `smcli` crate fails if production code (before any `#[cfg(test)]` module) in any other `crates/smcli/src/**/*.rs` names `search_engine(`, `Potion::bundled(`, `Potion::from_dir(` or `.embed(`. Test setup may build with `search_engine`; the guard's own search literals live in its test module.
  - **Runtime:** the model lock (`<db>.search/model.lock`, `flock`) is held by whoever has weights resident: the service from its first `load_engine` until exit, and `init`/`reindex` while they load.
- **Model identity (H2):** an index is named by the BLAKE3 content hash of the model's config, tokenizer and weights. Two copies of the same model share an index; a changed file gives a new name.
- **Storage ownership (H3):** only `smcli-core` runs SQL on the history database, and only `smcli-search` opens, inspects or deletes index files. `smcli` has no *direct* dependency on `taladb` or `rusqlite` and no production code that uses them. They still reach it transitively through `smcli-core` and `smcli-search`, which is the intended path.
- **Caller context (M4, L12):** request behavior depends only on the request's caller context. The service keeps only `LaunchSettings` (idle timeout, fff limits, default home) from its launch environment, and is launched with an allowlisted environment.

## Current call sites and anchors

Mapped with semble and ripgrep over `crates scripts`. Line numbers are at `6f0e7b0`.

### Model loads (H1)

| Site                                                   | What loads or embeds                                                                      | Under this plan                                                   |
| ------------------------------------------------------ | ----------------------------------------------------------------------------------------- | ----------------------------------------------------------------- |
| `smcli/src/cli.rs:315` `Command::Search`               | Service miss leads to `search_engine`, then hybrid search that syncs and embeds the index | `lexical_search`, no index access                                 |
| `smcli/src/cli.rs:1216-1226` `pick` search             | Ctrl-R fallback: `search_engine` in a `OnceCell`                                          | `lexical_search`                                                  |
| `smcli/src/cli.rs:1021` `pick_tldr`                    | `search_engine` for the command index                                                     | `configured_model`, read-only `CommandIndex`                      |
| `smcli/src/cli.rs:1075` `pick_tldr` closure            | `potion.embed(query)` on every keystroke                                                  | `embed` op, otherwise `rank_words`                                |
| `smcli/src/cli.rs:1093-1112` `command_index`           | Builds and `ensure`s with the CLI's `Potion`                                              | `commands::prepare`; service, or `reindex` only                   |
| `smcli/src/cli.rs:386-391` `Command::Reindex`          | `stop_service`, then load and rebuild                                                     | Allowed; moves to `initialize.rs`, takes the model lock           |
| `smcli/src/cli.rs:1482-1488` `indexes`                 | Loads only for the index names                                                            | `configured_model`                                                |
| `smcli/src/cli.rs:1552-1565` `index_health`            | Loads plus a probe embed                                                                  | `configured_model`, no probe                                      |
| `smcli/src/initialize.rs:69-79`                        | Loads **even when the model is unchanged and the service is running**                     | Unchanged model: `describe` only; changed model: stop, lock, load |
| `smcli-service/src/service.rs:545-566` `load_engine`   | The runtime owner; keyed on the whole `SmcliConfig`                                       | Keyed on `ModelId` (B2); takes the model lock (A3)                |
| `smcli-service/src/service.rs:434-449` `command_index` | Cached forever, holding a `Potion` clone                                                  | Keyed by index name, cleared on model change (A3)                 |

### Identity (H2, M5)

`potion.rs:25` uses the literal identity `"bundled"`. `potion.rs:88-96` hashes the path with `DefaultHasher`. `index.rs:98,115` names the index from it. `index.rs:376` reuses stored vectors when the identity and embedding text are unchanged. `service_location.rs:72` `build()` covers only the executable.

### Storage (H3)

- **SQL in `cli.rs`:**
  - `records` (`:599`)
  - `prune` and `finish_prune` (`:660-790`; deletes `rs-commands-v*` by prefix at `:774`)
  - `import`, Atuin and legacy (`:792-933`)
  - `files_kind` (`:938`)
  - `command_directory` (`:965`)
  - `fallback_picker_recents` (`:981`)
- **TalaDB in `cli.rs`:** health opens TalaDB (`:1634-1650`); `indexes` scans files (`:1491-1519`).
- **Dependencies:** `crates/smcli/Cargo.toml` lists `rusqlite` and `taladb`.

### Environment (M4, L12)

- `service_client.rs:169-172` passes the full client environment to `posix_spawn`.
- `service_client.rs:112` already fills `config` on every request, so D1 making it required changes nothing a v4 client sends.
- `smcli-service/src/main.rs:22` collects it.
- `service.rs:66` keeps it for life. It is read at `:108-110` (fff limits, `HOME`), `:303-307` (ranking context), `:530-538` (home, config fallback), `:558` (engine env) and `:639-644` (native history files).
- Per-request overrides today: `config`, `histfile`, `loginShell`, `shellPath` (search only) and `rankingOff` (search, suggest, pick).
- `context_cache.rs:99` runs `git` through `/usr/bin/env`, so it needs a `PATH`.

### Protocol (M7, M8, L9)

- `service_protocol.rs`: 11 ops, 27 request fields (`v`, `op` and 25 optional; A2's `home` makes 26 optional), `Failure.code: String`.
- Validation is spread through `service.rs:295,336,457,461,498-504,583,587` and `file_search.rs:171-175,302-306,383-387`.
- `service_client.rs:142` matches the string `"version"`.
- The dead branch is `service.rs:533-538` (`config_path` "requests from older clients").
- The fixture README says "all ten operations" and does not list `requests-v3.jsonl`.

### Housekeeping (L10, L11)

- `relative_insert` lives at `service_location.rs:85` (unused `_home`) and is used by `cli.rs:1436` and `file_search.rs:221,364`.
- `context_cache.rs` and the `notify` dependency are in `smcli-core` but used only by `smcli-service`.
- Swift remnants appear in comments at `cli.rs:22`, `picker.rs:1121,1126`, `index.rs:97,479`, `service_protocol.rs:1,317,499` and `tests/potion.rs:3`.

## Phase A — service-only model ownership (H1)

### A design

#### Model identity without building the model (`smcli-search`)

- `pub struct ModelId { pub identity: String, pub dimension: usize }`.
- `Potion::describe(potion_path: Option<&Path>) -> Result<ModelId>` resolves the directory the same way `bundled()` and `from_dir()` do. It checks the layout with the same rules as `load`, factored into `fn layout(dir) -> Result<(config, model_dir)>`. It reads the dimension from the safetensors header: 8-byte length, at most 1 MiB of JSON, and the `cols` of the `embeddings`, `0` or `embedding.weight` tensor, which is the order model2vec-rs 0.3 uses. Phase A keeps today's identity strings; B1 replaces them with the content hash.
- `setup::configured_model(home, env) -> Result<ModelId>` is the model-free twin of `search_engine`.
- Index names stop needing a loaded `Potion`: `index::index_name(&ModelId)`, which `HistorySearch::index_name` calls, and `CommandIndex` naming through `ModelId`.
- `CommandIndex` holds `model: ModelId` and `potion: Option<Potion>`. `CommandIndex::new(dir, potion, sources)` keeps its signature. The new `CommandIndex::reader(dir, ModelId, sources)` has `potion: None`. `write` and `remove` (and so `add`, `add_all`, `build`, `sync`, `ensure`) bail with "command index is read-only here" when it is `None`; `sync` calls `remove` directly, so guarding `write` alone is not enough.

#### Model lock

- `setup::model_lock(captured, wait) -> Result<Option<File>>` opens `<index dir>/model.lock` (`0600`, `O_NOFOLLOW | O_CLOEXEC`, private directory checked by `prepare_directory`) and retries `File::try_lock` until `wait` runs out.
- **Service:** `load_engine` takes `model_lock(.., ZERO)` before it builds, and stores the lock in `State.model_lock` until exit. If the lock is unavailable it returns `the model is in use by smcli reindex or smcli init`; search and `embed` reply `failed`, and the CLI degrades. `warm_search` ignores the error, and the next search retries.
- **CLI (`initialize.rs`):** after `stop_service`, `reindex` and a model-changing `init` take `model_lock(.., 5 s)`. If the lock is still held they **refuse** with `smcli-service still holds the model; run \`smcli service stop` and retry.` They hold the lock until the load and rebuild finish.

#### Keyword search and word rank without the model (`smcli-search`)

- `pub fn lexical_search(query, corpus, limit, filter, signals, now) -> Result<Vec<SearchEntry>>` shares a private `plan_query` preamble with `search_with_evidence_using_lock`: typed-only alias handling, the empty-query filter listing, `parse_markers`/`query_filter` and `is_literal_query`. Its body is either `signals.additions` with a rerank (empty corpus) or `text_fallback_with_evidence`. It touches neither TalaDB nor `potion`.
- `tldr::rank_words(query, entries, prefix_words)` scores lowercase word overlap with the description and template, plus the same 0.05 prefix bonus as `rank_with_vectors`, with stable order on ties.

#### `commands::prepare` (`smcli-search`)

This moves the CLI's `command_index` body (rebuild removes the file; a missing file means build from `os_programs ∪ installed(captured)` on `path`; the requested program, when installed, gets `ensure`) into `smcli_search::commands::prepare(index, refs, platform, path, captured_programs, rebuild, program)`. The CLI computes captured programs with `captured_programs`; the service with `recorded_program` over `history.runs`.

#### Protocol v4 (`smcli-core`)

- `VERSION = 4`.
- `Operation::Embed` (`"embed"`). Request fields: `query` (required; `""` means prepare only), optional `mode: "commands"` (prepare the `@@` index first) and `command` (program to make sure is indexed). `prepare_request` also fills `shellPath` for `Embed`.
- New response fields: `embedding: Option<Vec<f32>>` (finite-only serializer) and `model: Option<String>` (the command index name the vector matches).
- **`home: Option<String>` request field.** It is added now so D1 needs no second bump. Clients send it from A2 on; the service ignores it until D1.
- `repeatable(Embed)` is true.

#### Service

- `embed` takes `searching` (blocking), then `current_engine`.
  - With `mode == "commands"`, it calls `command_index_locked(&engine, refs)` and then `commands::prepare(.., rebuild=false, request.command)`. The locked variant does not take `searching` again: `Mutex` is not reentrant, and the existing `command_index` would deadlock.
  - A non-empty `query` sets `embedding` and `model = index.index_name()`. A missing `query` returns `bad_request`.
- **Background sync holds `searching`.** Today `finish_sync` (`service.rs:405`) gets the index through `command_index`, which releases `searching` before `commands::sync` embeds, so a sync thread keeps the old `Potion` clone resident while `load_engine` builds a new model (two models at once). `finish_sync` instead takes `searching.try_lock()` for its whole body (busy: return, the next capture retries, as today) and calls `command_index_locked`; the try-lock `command_index` is deleted.
- **Model lock is taken once per process.** `load_engine` calls `model_lock` only while `State.model_lock` is `None`; a second `flock` on a new descriptor in the same process would conflict with its own lock.
- **Accepted cost:** a 600 s `@@` prepare holds `searching`, so a concurrent search waits until its client timeout and degrades to keyword matching.
- **Cached command index lifecycle:** `load_engine` clears `command_sync.index`, `known` and `last_path` at the same time as it drops `state.engine`. Otherwise an old `Potion` clone stays resident and pairs old vectors with new queries. `command_index_locked` re-keys the cache by `index_name()`.

#### Behavior matrix

| Surface                                  | Service OK                                                                                                                                                                                                                                                                                            | Service unusable                                                                                                                                                            | Other protocol version                                                          |
| ---------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------- |
| `smcli search`                           | Service hybrid search                                                                                                                                                                                                                                                                                 | `lexical_search`, plus one stderr line: `smcli: search service unavailable; showing keyword matches only.`                                                                  | Spawn mode replaces the service and retries; connect and off modes: as unusable |
| Ctrl-R picker search                     | Service                                                                                                                                                                                                                                                                                               | `lexical_search`, no stderr                                                                                                                                                 | As above                                                                        |
| `@@` picker                              | Prepare only when needed: index file missing, `embed{mode:commands, command, query:""}` with 600 s; program missing, the same request with 10 s; otherwise none, and the picker opens at once. Per keystroke `embed{query}` (2 s), then `rank_with_vectors` when `model` equals the CLI's reader name | `rank_words`. **The first unusable reply latches word ranking for the session.** A missing index gives the status `command examples need smcli-service or \`smcli reindex`` | As above                                                                        |
| `smcli indexes [--prune]`                | Never asks the service; `configured_model`                                                                                                                                                                                                                                                            | Same                                                                                                                                                                        | Same                                                                            |
| `smcli health`                           | Never asks the service; `configured_model`, no embed probe                                                                                                                                                                                                                                            | Same                                                                                                                                                                        | Same                                                                            |
| `smcli reindex`                          | `stop_service`, model lock (5 s), load, rebuild                                                                                                                                                                                                                                                       | Same; refuses if the lock stays held                                                                                                                                        | Same                                                                            |
| `smcli init`                             | Unchanged model: `describe` only. Changed model: stop, lock, load, embed check                                                                                                                                                                                                                        | Same                                                                                                                                                                        | Same                                                                            |
| Service while the lock is held elsewhere | `search` and `embed` reply `failed`; the CLI degrades                                                                                                                                                                                                                                                 | n/a                                                                                                                                                                         | n/a                                                                             |
| suggest, picker first draw, files, grep  | Unchanged                                                                                                                                                                                                                                                                                             | Unchanged                                                                                                                                                                   | Unchanged                                                                       |

### A1: `smcli-search` model-free pieces

Files owned: `crates/smcli-search/src/{potion.rs,index.rs,commands.rs,tldr.rs,setup.rs,lib.rs,README.md}`.

Changes:

- `ModelId`, `Potion::describe`, `layout`
- `configured_model`, `model_lock`
- `index::index_name`
- `CommandIndex::reader` and its read-only guard
- `plan_query`, `lexical_search`, `rank_words`
- `commands::prepare`
- re-exports in `lib.rs`

Tests:

- `describe_matches_loaded_model`: same identity, dimension and index names as the loaded model.
- `describe_reads_header_without_weights`: the real config and tokenizer with only the safetensors header bytes. `describe` succeeds; `from_dir` fails.
- `describe_rejects_oversized_or_missing_header`.
- `lexical_search_matches_text_fallback`: literal, intent, `$marker`, empty query with a filter, empty corpus with project candidates.
- `command_index_reader_reads_and_refuses_writes`: also a reader `sync` with an empty wanted set refuses and keeps its rows.
- `rank_words_orders_by_overlap_and_prefix`.
- `prepare_builds_missing_index_and_ensures_program`, which replaces `ensure_adds_a_missing_program_once`.
- `model_lock_is_exclusive_and_waits`.

### A2: protocol v4

Files owned: `crates/smcli-core/src/{service_protocol.rs,service_client.rs}`, `crates/smcli-core/tests/service_protocol.rs`, `crates/smcli-core/tests/fixtures/service_protocol/{requests-v4.jsonl,responses.jsonl,README.md}`.

Changes: as in [Protocol v4](#protocol-v4-smcli-core). The fixture README is fixed in the same edit (**L9**): it gets the twelve operations, a line for `requests-v3.jsonl` and `requests-v4.jsonl`, and a note that v1–v3 files prove decoding only, while rejection is tested in E1.

Tests:

- `requests-v4.jsonl` round-trips: embed (query only; `mode:"commands"` with `command` and `shellPath`) and a request with `home`.
- `responses.jsonl` gains a line with `embedding` and `model`.
- `VERSION == 4`; the literal v3 lines in `tests/service_protocol.rs` are updated.
- A non-finite embedding fails encoding.
- `modes_and_repeatable_operations` covers `Embed`.

### A3: service `embed`, model lock, cache lifecycle

Files owned: `crates/smcli-service/src/service.rs`, `crates/smcli-service/tests/client.rs`, `crates/smcli-service/README.md`.

Tests:

- `embed_returns_vector_and_command_index_name`.
- `embed_commands_mode_builds_then_ensures_program`.
- `embed_without_query_is_bad_request`.
- `embed_does_not_deadlock_with_background_sync`.
- `background_sync_skips_while_searching_held`: with `searching` held, `finish_sync` returns without touching the index and leaves `running` false.
- `load_engine_refuses_while_model_lock_held`.
- `model_switch_drops_cached_command_index`: A→B→A config switch in one service. `model` names the current index each time, and no index for the old identity stays cached.
- `tests/client.rs`: embed over the real socket.

### A4: CLI without the model

Files owned: `crates/smcli/src/{cli.rs,initialize.rs,usage.txt,README.md}`, `scripts/test-service.py`, `scripts/bench-baseline.py`.

Changes:

- `Command::Search` and the `pick` closure call `lexical_search`; `search` prints the stderr line.
- `pick_tldr` uses `configured_model` and `CommandIndex::reader`, with conditional prepare.
- `fn tldr_ranker(service, expected_model, entries, vectors, prefix)` tries `embed` (2 s), falls back to `rank_words`, and latches after the first unusable reply.
- `indexes` and `index_health` use `configured_model` and `lock_index_directory`.
- `Command::Reindex` moves to `initialize::reindex` (`commands::prepare(rebuild=true)`, model lock).
- `init` with an unchanged model only calls `describe`.
- `usage.txt` describes the `SMCLI_SERVICE=0` degradation.
- Python scripts use a `V = 4` constant.

Tests:

- **`cli.rs` unit tests:**
  - `only_initialize_loads_the_model` (source scan).
  - `tldr_ranker_falls_back_to_words_without_service`.
  - `tldr_ranker_rejects_vectors_for_another_model` (fake `UnixListener`).
  - `tldr_ranker_latches_after_stalled_service`: the first call takes at most about 2 s; the next 5 take under 50 ms each with no new connection.
  - `pick_tldr_skips_prepare_when_index_has_program`.
  - Update `health_checks_coverage_without_rebuilding` and `indexes_prune_respects_lock_and_keeps_current_typed`: setup may build with `search_engine`, but `health` and `indexes` stay model-free.
- **`initialize.rs`:**
  - `init_unchanged_model_does_not_load`, using a header-only model directory.
  - `reindex_refuses_while_model_lock_held`.
- **`test-service.py`:**
  - Lines 477-489, "served equals fallback": compare the PATH and file reasons per command, not order; the stderr line is present.
  - Line 548, "in-process rebuild": without the service, keyword search answers and the emptied index stays broken until a served search heals it.
  - Line 292, v1 socket: the assertion is unchanged.
  - New: a **no in-process load** block. A header-only model as `potionPath`: `search`, `indexes --json` and `health` succeed with `SMCLI_SERVICE=0`, while `init --potion <dir>` fails.
  - New: **served `@@` prepare** over a raw socket.
  - New: **model lock**. Python `flock` held, then the service answers search `ok:false`, `smcli search` degrades, and `smcli reindex` refuses. After release, search succeeds with one service.

### A gate

- `make test-rust PKG=smcli-search`, then `make ranking` (shared preamble refactor; scores must not move)
- `make test-rust PKG=smcli-core`
- `make test-rust PKG=smcli-service`
- `make test-rust PKG=smcli`, `make test-service`, `make test-cli`, `make test-picker`

## Phase B — content identity and engine keying (H2, M5, M6)

### B1: content-hash `ModelId` (H2)

Files owned: `crates/smcli-search/src/{potion.rs,index.rs,commands.rs,setup.rs,Cargo.toml}`, `crates/smcli/src/cli.rs` (the `indexes`/`health` filename filter at `:1500`, which stays there until C2), `scripts/test-service.py` (index-name assertions).

Changes:

- **Identity.** `identity = "b3" + first 16 hex digits of BLAKE3` over `len ‖ bytes` of the resolved config file, `tokenizer.json` and `model.safetensors`, in that order. The same rule applies to the bundled model and to `from_dir`. `model_identity` and `DefaultHasher` are deleted. `blake3` is already in `Cargo.lock` as a transitive dependency; make it a direct dependency of `smcli-search` and run `make notices`.
- **Hashing.** `describe` streams the three files through the hasher with a 1 MiB buffer and never allocates the matrix. `Potion::load` derives its `ModelId` from the same bytes it parses, so a loaded engine's identity is exactly the content it embeds with.
- **Index names change once.**
  - History: `VERSION` goes from `rs-commands-v3` to `rs-commands-v4`. The existing `remove_old_indexes` deletes v3 files on the next build.
  - Command index: the name becomes `rs-examples-v1-potion-shell-16m-{identity}-{sources}-d{dim}`. A new `index::remove_legacy_indexes(dir)` deletes unversioned `rs-commands-potion-shell-16m-*` command indexes. It runs wherever `remove_old_indexes` runs, from `commands::prepare`, and from `indexes --prune`.
  - `indexes`, `health` and the prune cleanup recognize the `rs-examples-` prefix. History prune keeps leaving the command index alone, as today.
- **Install.** `install.sh` already runs `smcli reindex`, so an upgrade rebuilds under the new names at install.

Tests:

- `identity_changes_with_any_model_file`: flip one byte in each of the three files of a temp copy, and get three new identities.
- `identity_is_path_independent`: the bundled model and a copy at another path give the same identity.
- `identity_is_stable`: the bundled model's identity equals a constant pinned in the test (catches hasher or input-order drift).
- `loaded_identity_matches_describe`.
- `legacy_indexes_removed_on_build`: v3 history files and unversioned command index files are gone after `prepare` and after a history build. Other models' v4 indexes are kept.
- `describe_timing` (`#[ignore]`, run once in release): record the bundled `describe` time in the commit message. If it is over 25 ms, raise it at the gate (see omissions).
- `test-service.py`: the `-bundled-tldr@` and `rs-commands-v3-potion-othermodel` fixtures become their v4 and `rs-examples-` equivalents. The "stale index from an older version is deleted" case now also covers the legacy command index.

### B2: service engine keyed on model identity (M5, M6)

Files owned: `crates/smcli-service/src/service.rs`.

Changes:

- `State.engine: Option<(ModelId, Arc<HistorySearch>)>`, keyed on the model only.
- `load_engine` resolves the caller's config to a model directory and asks a stat-keyed memo, `model_ids: Mutex<HashMap<PathBuf, (FileStats, ModelId)>>`, where `FileStats` is the (dev, ino, size, mtime_ns, **ctime_ns**) of the three files. `ctime` changes on every write and cannot be set back by user tools, so a same-length rewrite that restores `mtime` still misses the memo. The memo re-hashes only when a stat changes, and it stats again after hashing; if the stats moved during the hash, the result is not stored. If the `ModelId` differs from the cached engine's, the service drops the engine and the command index cache, then builds under the lock it already holds.
- This makes a model-only reinstall take effect on the next search (**M5**). A config edit that leaves `potionPath` and the files alone reuses the engine (**M6**).
- A built engine whose loaded `ModelId` differs from the key it was built for (the files changed mid-load) is not cached.
- `build()` (the binary identity) is unchanged. Hashing the model on every client send was rejected; see omissions.

Tests:

- `capture_filter_edit_keeps_engine`: change `historyFilter`, then search, and get the same `Arc` and no reload.
- `model_file_change_reloads_engine`: a temp model copy; touch and modify `tokenizer.json`, then search. A new identity is loaded, and the old engine and command index cache are dropped.
- `same_content_new_path_keeps_engine`.
- `same_length_rewrite_with_restored_mtime_reloads`: rewrite `tokenizer.json` in place with different same-length bytes and restore its `mtime` (`filetime`/`utimensat`). The next search loads the new identity, the command index cache is dropped, and `embed`'s `model` matches the CLI's `configured_model`.

### B gate

`make test-rust PKG=smcli-search`, `make test-rust PKG=smcli-service`, `make test-service`, `make ranking`, `make notices` (the diff should be empty or limited to `blake3`), `make test-install` (upgrade path: `reindex` at install leaves no legacy index).

## Phase C — storage ownership (H3)

### C1: history SQL into `smcli-core`

Files owned: `crates/smcli-core/src/{history.rs,import.rs (new),prune.rs (new),lib.rs,README.md}`.

These functions move from `cli.rs`, behavior unchanged. Their SQL moves verbatim into `impl CapturedHistory` blocks:

- `export_rows()`, from `records`.
- `prune(filter, dry, cleanup: impl FnOnce() -> Result<()>) -> PruneReport`, from `prune` and `finish_prune`. The core does the deletes, then calls `cleanup` for index files, then runs checkpoint and `VACUUM`. It keeps the current combined error messages and the `.v7.bak` notice in the report.
- `import_atuin(path, filter) -> usize` and `import_legacy(path, filter) -> usize`, from `import`.
- `file_kind_rows(user)`, from `files_kind`. The majority decision stays in the CLI.
- `last_directory(command_key, user)`, from `command_directory`.
- `recent_distinct(user)`, from `fallback_picker_recents`.

Tests: the existing `cli.rs` tests for these move to core and keep their names:

- `atuin_import_deduplicates_and_replace_is_atomic`
- `prune_cleanup_runs_when_database_compaction_fails`
- `prune_keeps_clean_selections_of_allowed_commands`
- `prune_removes_blank_query_file_selections_in_excluded_directories`
- `legacy_import_applies_filters_without_expanded_command`
- `alias_export_import_and_prune_keep_variant_identity`
- `legacy_import_is_idempotent`
- `imports_are_atomic_and_pruning_honors_dry_run`
- `fallback_history_picker_includes_oldest_distinct_capture`

### C2: index files into `smcli-search`

Files owned: `crates/smcli-search/src/{index.rs,commands.rs,README.md}`.

These move from `cli.rs`:

- `index::inspect(directory, &ModelId, corpus) -> IndexReport`. It takes the TalaDB open and aggregate (keeping the "never `open_index`, it deletes" rule), the private directory and owner checks, `other_indexes`, bytes, `last_updated`, and the try-lock.
- `index::list_indexes(directory, current_names, prune) -> Vec<IndexInfo>`.
- `index::remove_history_indexes(directory)`, which is the prune cleanup.

`IndexHealth` serialization stays in the CLI.

Tests: `health_checks_coverage_without_rebuilding`, `indexes_prune_respects_lock_and_keeps_current_typed` and `prune_removes_index_directories_without_loading_model` move or split, so the search side asserts the reports and the CLI asserts output.

### C3: CLI drops storage dependencies

Files owned: `crates/smcli/src/cli.rs`, `crates/smcli/Cargo.toml`.

Changes: call the C1 and C2 APIs. Remove `taladb` from `[dependencies]` and move `rusqlite` to `[dev-dependencies]`; it remains only for test fixtures that build an Atuin database.

Tests:

- `smcli_has_no_storage_dependencies` reads `Cargo.toml` and fails if `taladb` or `rusqlite` is under `[dependencies]`.
- The A4 source-scan guard also fails on `rusqlite::` or `taladb::` in non-test code under `crates/smcli/src/**`, which is code before any `#[cfg(test)]` module.

The transitive dependency `smcli → smcli-search → taladb` stays by design, so the gate does not run `cargo tree` absence checks for these.

### C gate

`make test-rust PKG=smcli-core`, `make test-rust PKG=smcli-search`, `make test-rust PKG=smcli`, `make test-cli` (import, prune, history), `make test-service`.

## Phase D — caller context and launch environment (M4, L12)

### D1: explicit caller context

Files owned: `crates/smcli-core/src/{caller.rs (new),service_client.rs,ranking.rs,config.rs,lib.rs}`, `crates/smcli-service/src/{service.rs,main.rs,file_search.rs}`, `crates/smcli/src/cli.rs` (construction only), `scripts/test-service.py`.

Changes:

- `CallerContext { home, config, cwd, session, histfile, shell, path, ranking_off }`. `path` travels as the existing `shellPath` wire field (the `path` field belongs to file ops).
  - `CallerContext::from_env(env)` is used by the CLI for both the request and the in-process fallback, so both paths see identical inputs.
  - `apply(&mut ServiceRequest)` uses one table, `Operation::caller_fields()`, to decide which fields each op carries. This replaces the per-op `if`s in `prepare_request`.
  - `from_request(&ServiceRequest, &LaunchSettings)` is used by the service.
  - Size bounds stay where they are today (`shell_path_snapshot`, 256-byte `rankingOff`).
- **`config` becomes required** for every op that reads config (record, select, search, suggest, pick, embed); a missing value is `bad_request`. This deletes the `config_path` older-client fallback (**M8**). `test-service.py`'s raw-message helper sends `config`.
- `RankingContext::new` takes `ranking_off: Option<&str>` instead of an environment map.
- Native history files resolve from the caller's `home`, `histfile` and `shell`.
- File-content indexing (`file_search.rs:142`) decides from the caller's `home`, not the launch home. The file index cache (`file_search.rs:76`) is keyed on root plus that content-indexing policy, so callers with different homes never share an incompatible index.
- `Service.environment` is replaced by `LaunchSettings { home, idle, fff_max, fff_idle }`, parsed once in `main.rs`. No request handler reads the launch environment.

Tests:

- `caller_fields_table_is_applied`: for each op, `apply` sets exactly the fields `Operation::caller_fields()` lists (a handler's reads cannot be introspected; E1's typed structs make that check structural).
- `service_uses_caller_home_and_histfile`: a service started with `HOME=A` answers search with native history from the caller's `HOME=B` and `HISTFILE`.
- `missing_config_is_bad_request`.
- `file_index_follows_caller_home`: two caller homes against the same non-repository root get the policy for their own home, and neither reuses the other's index.
- `fallback_and_service_build_identical_context`: the same `CallerContext` from both sides for one environment.
- The `test-service.py` signals parity block (lines 468-496) runs with a service launched from a different `PATH`, `HOME` and `SMCLI_RANKING_OFF` than the caller.

### D2: scrubbed service launch (L12)

Files owned: `crates/smcli-core/src/service_client.rs`.

Changes: `launch` passes `launch_environment(env)`. It is an allowlist:

- `HOME`, `USER`, `LOGNAME`, `PATH`, `TMPDIR`, `LANG`, `LC_*`
- `XDG_RUNTIME_DIR`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`
- `HISTFILE` (for `check_separate`)
- `SMCLI_DB`, `SMCLI_CONFIG`, `SMCLI_RUNTIME_DIR`, `SMCLI_SERVICE_IDLE`, `SMCLI_FFF_MAX`, `SMCLI_FFF_IDLE`

`PATH` is kept because `ContextCache` runs `git` through `/usr/bin/env`. Everything else (tokens, `SMCLI_RANKING_OFF`, `SHELL`) is dropped.

Tests:

- `launch_environment_is_allowlisted`: secrets, `SHELL` and `SMCLI_RANKING_OFF` are dropped; every listed key is kept.
- `test-service.py` on Linux (`make linux`): start the service with `SMCLI_TEST_SECRET=x`. `/proc/<pid>/environ` lacks it. Skipped on macOS, where the unit test is the check.

### D gate

`make test-rust PKG=smcli-core`, `make test-rust PKG=smcli-service`, `make test-service`, `make test-shells` (hooks send records through the new `apply`), `make baseline`: record and suggest latency must not regress by more than 10%.

## Phase E — typed requests, failure codes, dead branches (M7, M8, L9)

### E1: typed validation over the same wire

Files owned: `crates/smcli-core/src/{service_protocol.rs,request.rs (new)}`, `crates/smcli-core/tests/service_protocol.rs`, `crates/smcli-service/src/{service.rs,file_search.rs}`, `crates/smcli-core/src/service_client.rs`.

Changes:

- `pub enum Request { Ping, Shutdown, Record(Record), Suggest(Ranked), Pick(Ranked), Search(Search), Context(Context), Files(FileQuery), Grep(FileQuery), Complete(Complete), Select(Select), Embed(Embed) }`, with `TryFrom<&ServiceRequest>` returning `(FailureCode::BadRequest, message)`. All validation from `service.rs` and `file_search.rs` moves here with its messages verbatim: absolute directories, limit ranges (1–1000 and 1–500), required fields, select modes, expansion safety, and the embed query.
- `FailureCode { Version, BadRequest, Busy, Failed, Timeout, TooLarge }` with `as_str()`, plus `ServiceResponse::fail(FailureCode, msg)` and `Failure::code() -> Option<FailureCode>`. On the wire `code` stays a string, so fixtures and the "open-ended failure codes" decoding rule are unchanged. The client matches `FailureCode::Version`.
- `dispatch` matches on `Request`, and handlers take their typed struct. Remaining `request.<field>` reads in handlers are removed.
- **M8:** with D1's required `config`, delete `config_path`'s fallback and its comment. Search the tree for any other branch handling "older clients" under the same version, and remove it. Live compatibility stays: replacing an older-protocol service (`service_client.rs:84-104`) and the `build` stamp.

Tests:

- `every_v4_fixture_request_converts`: each `requests-v4.jsonl` line becomes a `Request`.
- `validation_table`: one case per rejection, each with the old message.
- `failure_codes_round_trip_as_strings`.
- `older_versions_rejected_end_to_end` (**L9**): every line of `requests.jsonl`, `requests-v2.jsonl` and `requests-v3.jsonl` dispatched to a test service returns a `version` failure.

### E gate

`make test-rust PKG=smcli-core`, `make test-rust PKG=smcli-service`, `make test-service`. No fixture files change.

## Phase F — housekeeping (L10, L11)

### F1: misplaced code and Swift remnants (L10)

Files owned:

- `crates/smcli-core/src/{service_location.rs,paths.rs (new),context.rs,lib.rs,Cargo.toml}`
- `crates/smcli-service/src/{context_cache.rs (moved),service.rs,file_search.rs,Cargo.toml}`
- comment-only edits in `crates/smcli/src/{cli.rs,picker.rs}`, `crates/smcli-core/src/service_protocol.rs`, `crates/smcli-search/src/index.rs` and `crates/smcli-search/tests/potion.rs`

Changes:

- `relative_insert(path, root, cwd)` moves to `smcli_core::paths` and drops the unused `_home`; its tests move with it. The CLI and `file_search` import it from there.
- `context_cache.rs` moves to `smcli-service` together with the `notify` dependency. `smcli-core` keeps `context.rs`, `ProjectContext::discover` and the `GitStatus` parsing, which the CLI uses.
- Swift-remnant comments are reworded to state the behavior. Fixture provenance stays in the fixture README. Command strings like `swift build` in tests stay: they are data.

Tests: the moved tests pass. `cargo tree -p smcli -e normal -i notify` reports the package absent (gate step).

### F2: split `cli.rs` (L11)

Files owned: `crates/smcli/src/cli.rs`, which becomes `crates/smcli/src/cli/{mod.rs,search.rs,pick.rs,tldr.rs,history.rs,index.rs}`.

The split is a pure move after A–D, with no behavior change:

- `mod.rs`: clap types, `run` dispatch, shared helpers (`print_json`, `time`, `directory`, `stop_service`).
- `search.rs`: search and suggest.
- `pick.rs`: `pick`, `LocalFileSearch`, file kind, selections.
- `tldr.rs`: `pick_tldr`, `tldr_context`, `tldr_ranker`.
- `history.rs`: record, history, import and prune wrappers.
- `index.rs`: `indexes`, `health` output.

Each test moves with its function. The A4 guard test scans `src/**`.

### F gate

`make test` (full), `make test-picker`, `make test-shells`, `make linux`, then `make fresh` for the user to try `@@`, Ctrl-R and `smcli search` with and without the service (`SMCLI_SERVICE=0`).

## AGENTS.md Architecture text (scribe, after F)

- **Executables paragraph.** Replace "Either can load it, so each can search alone:" with:

  > Only `smcli-service` loads it at runtime. The CLI reads the model's files for its identity (never building the model), and loads it only in `smcli init` when switching models and in `smcli reindex`, after stopping the service and taking the model lock (all in `crates/smcli/src/initialize.rs`):

- **Libraries.** Replace `smcli-core` (SQLite captured history at …) with:

  > `smcli-core` (sole owner of SQL on the history database at `~/.local/share/smcli/history.db`: capture, import, prune, export; capture filters, ranking/suggestions, config, socket client, protocol and caller context)

  Then add after the `smcli-search` sentence:

  > Only `smcli-search` opens, inspects or deletes index files; the `smcli` crate depends on neither `taladb` nor `rusqlite`.

- **Protocol.** Replace "On a version mismatch or no service, history and search fall back in-process; service-only file-search ranking evidence is unavailable." with:

  > Each request carries its caller context (home, config, cwd, session, HISTFILE, SHELL, PATH, ranking switches); the service acts only on that and on launch settings, and is started with an allowlisted environment. On a version mismatch or no service, history, suggestions and files fall back in-process. Search falls back to keyword matching and `@@` to word ranking, never to the embedding model. Service-only file-search ranking evidence is unavailable.

- **Models paragraph.** Replace "A model's index is named by its identity" with:

  > A model's index is named by its identity, a BLAKE3 hash of its config, tokenizer and weights, so a changed model file gets a new index and the service reloads on the next search.

  Then replace "To avoid holding two models at once, init (when changing models) and `smcli reindex` attempt to stop the service before loading a model; they can proceed if shutdown fails." with:

  > To avoid holding two models at once, whoever has weights resident holds the model lock (`<db>.search/model.lock`): the service from its first model load until exit, and init (when changing models) and `smcli reindex` while they load. Those two stop the service first and refuse if the lock stays held. These are the only CLI paths that load a model; a unit test in `crates/smcli` enforces it.

Also update `crates/smcli/src/README.md`, `crates/smcli-core/src/README.md`, `crates/smcli-search/src/README.md`, `crates/smcli-service/README.md` and `crates/README.md` for the moved modules, then run `make docs`.

## Deliberate omissions

- **A dedicated embed daemon.** `embed` is one op on the existing service.
- **Async `@@` preparation behind an open picker.** The waits are bounded instead: prepare only when needed, and latch word ranking.
- **Ollama and Apple `builtin` embedders.** Still unported.
- **Feature-gating `model2vec-rs` out of the CLI binary.** `init` and `reindex` need it; the guard test and the model lock enforce the rule instead.
- **A degraded-mode indicator in the history picker.** Only `smcli search` prints the stderr line.
- **Model fingerprint in the service `build` string (the other M5 option).** The client would hash about 32 MB on every hook call. B2's stat-keyed reload covers the same case inside the service.
- **A stat-keyed fingerprint cache in the CLI.** Left out unless `describe_timing` exceeds 25 ms. The CLI hashes only for `@@`, `indexes`, `health` and `init`.
- **Per-request `PATH` for the service's `git` subprocesses.** `ContextCache` keeps the scrubbed launch `PATH`; caller `PATH` drives ranking only.
- **A nested caller object on the wire.** The flat fields stay; typing happens in Rust (no extra wire break).
- **Removing the v1–v3 fixture files.** They stay as decode tests, and E1 adds the rejection test.
- **A model lock across databases.** It is per `SMCLI_DB`, as today.

## Adversarial review

### Round 1 (service-only slice, 2026-11-04)

Codex (`gpt-6.1-sol`) returned **needs-attention** with three findings. All held, and Phase A carries the fixes.

- **[high] The ownership invariant excluded the reindex respawn race.** Accepted: the model lock, with refusal instead of "proceed if shutdown fails".
- **[high] The cached command index was not invalidated on a model switch.** Accepted: cleared in `load_engine` and re-keyed by name; A→B→A test.
- **[medium] The 600 s prepare blocked the picker from opening.** Accepted in part: conditional prepare (600 s only for a missing index, 10 s for a missing program) and the latch. Rejected: async preparation (UI change).

### Round 2 (full plan, 2026-11-04)

Codex (`gpt-6.1-sol`) returned **needs-attention** with two findings. Both held, and the plan was revised for each.

- **[medium] The B2 stat memo could keep an obsolete model** after a same-length in-place rewrite that restores `mtime`. The CLI would then hash a new identity and reject the service's vectors. Accepted: `ctime_ns` joins the memo key, stats are re-checked after hashing, and the regression `same_length_rewrite_with_restored_mtime_reloads` was added.
- **[medium] The C3 `cargo tree` absence gate could not pass**, because `smcli → smcli-search → taladb` stays (and `smcli-core → rusqlite`). Accepted: the storage invariant is now about direct dependencies and production code. The gate is the manifest test plus the source-scan guard extended to `rusqlite::`/`taladb::`. The F1 `notify` absence check stays, because after the move no crate `smcli` depends on links `notify`.

Rejected findings: none.

### Round 3 (verification against `99fceb0`, 2026-11-04)

Anchors, test names, `make` targets, `test-service.py` lines, `model2vec-rs` 0.3 tensor order (`model.rs:181-183`), `StaticModel::from_bytes` and `blake3` in `Cargo.lock` (via `fff-search`) all checked out. HEAD differs from the baseline only in docs. Findings:

- **[high] Background sync kept a second model resident.** `finish_sync` embeds without `searching`, so a model switch could overlap it. Accepted: sync holds `searching` via `try_lock`; new test.
- **[medium] Same-process lock re-acquire.** Accepted: take the model lock only while `State.model_lock` is `None`.
- **[medium] B1 renames command indexes but did not own `cli.rs`,** where the `indexes`/`health` filter lives until C2; the B gate would fail. Accepted: B1 owns that filter. The scope line wrongly said B2 edits `cli.rs`; fixed to B1/D1.
- **[low] Counts and anchors.** "27 optional fields" is 27 fields, 25 optional; env spawn is `service_client.rs:169-172`; Swift remnants also at `service_protocol.rs:317,499`, and F1 did not own `cli.rs` or `service_protocol.rs` for them. Accepted, fixed.
- **[low] `caller_fields_table_matches_handlers` cannot be written** (handler reads are not introspectable before E1). Accepted: replaced with `caller_fields_table_is_applied`.
- **[low] A 600 s prepare blocks searches.** Accepted as a stated cost; holding `searching` is what keeps one model resident.
- Rejected: making `config` required in D1 needs a protocol bump. It does not; `prepare_request` already always sends it.
- Not cut, left to the user: E1 (typed `Request`) and F2 (`cli.rs` split) are large refactors for medium/low findings.

### Round 4 (Codex on the revised plan, 2026-11-04)

Codex (`codex-rescue`) returned **needs-attention** with three findings, from code reading. All held.

- **[medium] The read-only `CommandIndex` could still delete:** `sync` calls `remove` (`commands.rs:171,260`) without `write`. Accepted: `remove` is guarded too, and the A1 reader test covers a sync.
- **[medium] D1 left file-content indexing on the launch home** (`file_search.rs:142`), and the file index cache is keyed on root alone (`file_search.rs:76`). Accepted: the caller's home drives the policy, the cache key includes it, and `file_index_follows_caller_home` was added.
- **[low] The model-load source guard contradicted A4's test setup** (`cli.rs:1869`). Accepted: the guard scans production code only.

Rejected findings: none.
