# Service-only model ownership

## Scope

`smcli-service` becomes the only runtime owner of the Potion model. The CLI never opens the weights for search, the history picker, `@@`, `indexes` or `health`. When the service is down, busy, failed, timed out or speaks another protocol, the CLI degrades (keyword search, word-ranked `@@`, an error, or starting the service) and never embeds in-process.

`smcli init` (only when the model changes) and `smcli reindex` still load the model, but only after stopping the service and taking the model lock. This turns the existing "never hold two models at once" rule into an enforced one.

Non-goals: see [Deliberate omissions](#deliberate-omissions).

Prerequisite: the uncommitted `commands::ensure` change in `crates/smcli-search/src/commands.rs` and `crates/smcli/src/cli.rs` (heal a missing `@@` program) lands first. WP1 moves that logic and does not rewrite it.

## Invariant (new rule)

> The CLI reads a model's identity and dimension from its files (directory layout and the safetensors header) and never loads its weights, except in `smcli init` when switching models and in `smcli reindex`, both after stopping the service. Every embedding at runtime happens in `smcli-service`. Without a usable service, search is keyword-only and `@@` is word-ranked.

Enforcement has two layers:

- **Static:** every CLI model load goes through `crates/smcli/src/initialize.rs`. A unit test in the `smcli` crate fails if any other `crates/smcli/src/*.rs` file names `search_engine(`, `Potion::bundled(`, `Potion::from_dir(` or `.embed(`.
- **Runtime:** a cross-process **model lock** (`<db>.search/model.lock`, `flock`). Whoever has weights resident holds it: the service from its first `load_engine` until exit, and `init`/`reindex` for the length of their load. Neither side loads without it. This closes the race where a shell capture respawns the service (which warms its model) while `reindex` holds its own copy.

## Current call sites

Mapped with semble and ripgrep over `crates scripts` (line numbers at `HEAD` plus the in-flight `ensure` diff).

| Site                                                   | What loads or embeds                                                                                   | Under the new rule                                                 |
| ------------------------------------------------------ | ------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------ |
| `smcli/src/cli.rs:315` `Command::Search`               | Service miss leads to `search_engine` and then the hybrid search, which syncs and embeds the index     | Keyword fallback, no index access                                  |
| `smcli/src/cli.rs:1216-1226` `pick` search             | Ctrl-R semantic fallback: `search_engine` held in a `OnceCell`                                         | Keyword fallback                                                   |
| `smcli/src/cli.rs:1021` `pick_tldr`                    | `search_engine` for the command index                                                                  | Header-only `ModelId`, read-only `CommandIndex`                    |
| `smcli/src/cli.rs:1075` `pick_tldr` closure            | `potion.embed(query)` on every keystroke                                                               | `embed` service op, otherwise `tldr::rank_words`                   |
| `smcli/src/cli.rs:1093-1112` `command_index`           | `CommandIndex::new(.., engine.potion)`, `build`, `ensure` (embeds examples)                            | Moves to `commands::prepare`; service, or `reindex` only           |
| `smcli/src/cli.rs:386-391` `Command::Reindex`          | `stop_service`, then `search_engine`, `reindex_using_lock`, `command_index(rebuild)`                   | Allowed; moves to `initialize.rs`                                  |
| `smcli/src/cli.rs:1482-1488` `indexes`                 | `search_engine` only for `index_name()` and the command index name                                     | Header-only `ModelId`                                              |
| `smcli/src/cli.rs:1552-1565` `index_health`            | `search_engine`, plus a probe `embed("smcli health check")`                                            | Header-only `ModelId`, no probe                                    |
| `smcli/src/initialize.rs:69-79`                        | Loads and embeds **even when the model is unchanged and the service is running** (two models resident) | Unchanged model: header check only; changed model: stop, then load |
| `smcli-service/src/service.rs:561` `load_engine`       | The one runtime owner                                                                                  | Unchanged; gains the `embed` op                                    |
| `smcli-service/src/service.rs:434-449` `command_index` | Shares the engine's `Potion` through `try_lock(searching)`                                             | Split into a locked helper that `embed` reuses                     |
| `smcli-search/src/setup.rs:64` `search_engine`         | The loader                                                                                             | Kept for the service, `init`, `reindex` and tests                  |

Callers that already avoid the model: suggest and the picker's first draw (`SuggestionIndex`), `prune` (`prune_removes_index_directories_without_loading_model`), files and grep (`LocalFileSearch`).

## Design

### Model identity without weights (`smcli-search`)

- `pub struct ModelId { pub identity: String, pub dimension: usize }`.
- `Potion::describe(potion_path: Option<&Path>) -> Result<ModelId>` resolves the directory the same way `bundled()` and `from_dir()` do (`bundled_dir`, `model_identity` on the canonical path). It checks the layout with the same rules as `load`, factored into `fn layout(dir) -> Result<(config, model_dir)>`. It then reads only the safetensors header: 8 bytes for the length, at most 1 MiB of JSON, and the `cols` of the `embeddings`, `0` or `embedding.weight` tensor, which is the order model2vec-rs 0.3 uses. `Potion::load` sets `ModelId` from the same header and keeps its encode-based dimension as a debug assertion.
- `setup::configured_model(home, env) -> Result<ModelId>` is the header-only twin of `search_engine`.
- Index names stop requiring a loaded `Potion`: `index::index_name(&ModelId)`, which `HistorySearch::index_name` calls, and `CommandIndex::index_name` through `ModelId`.
- `CommandIndex` holds `model: ModelId` and `potion: Option<Potion>`. `CommandIndex::new(dir, potion, sources)` keeps its signature. The new `CommandIndex::reader(dir, ModelId, sources)` sets `potion: None`; `write` (and so `add`, `add_all`, `build`, `sync`, `ensure`) bails with "command index is read-only here" when it is `None`. `collection()` takes the dimension from `model`.

### Model lock (`smcli-search`, service, CLI)

- `setup::model_lock(captured, wait: Duration) -> Result<Option<File>>` opens `index_directory(captured).join("model.lock")` (`0600`, `O_NOFOLLOW | O_CLOEXEC`, the same flags as `index_health`'s lock open; it creates the private directory through the existing `prepare_directory` rules). It retries `File::try_lock` until `wait` runs out and returns `None` if the lock is still held.
- **Service:** `load_engine` takes `model_lock(.., Duration::ZERO)` before `search_engine` and stores the `File` in `State` (`model_lock: Option<File>`), held until exit. If the lock is unavailable it returns the error `the model is in use by smcli reindex or smcli init`. Search then replies `failed`, and the CLI degrades to keyword search. `warm_search` already ignores the error, and the next search retries once the lock is free.
- **CLI (`initialize.rs`):** after `stop_service`, `reindex` and a model-changing `init` take `model_lock(.., 5 s)`; the service releases it at exit. If the lock is still held they **refuse** with `smcli-service still holds the model; run \`smcli service stop` and retry.` They no longer proceed when shutdown fails, and they hold the lock until the load (and rebuild) finishes.

### Keyword search without the model (`smcli-search`)

- `pub fn lexical_search(query, corpus, limit, filter, signals, now) -> Result<Vec<SearchEntry>>`. It shares the preamble of `search_with_evidence_using_lock` (typed-only alias handling, the empty-query filter listing and its error, `parse_markers`/`query_filter`, `is_literal_query`). The preamble moves into a private `fn plan_query(..) -> QueryPlan` that both call, so the two paths cannot drift. The body is then `corpus.is_empty()` leading to `signals.additions` and `rerank_with_evidence`, or `text_fallback_with_evidence`. Neither touches TalaDB or `self.potion`.
- `tldr::rank_words(query, entries, prefix_words) -> Vec<(usize, f32)>` scores the lowercase word overlap of the query with `description` and `template`, plus the same 0.05 prefix bonus as `rank_with_vectors`, with stable order on ties. An empty query keeps index order plus the bonus.

### Protocol v4: `embed` (`smcli-core`)

The fixture rule says that a protocol change edits fixtures by hand and bumps the version, so `VERSION` goes from 3 to 4.

- `Operation::Embed` (wire `"embed"`). Request fields: `query` (required; may be `""` for prepare-only), `mode: "commands"` (optional: prepare the `@@` command index first), `command` (optional program to make sure is indexed), and `shellPath` (filled by `prepare_request` for `Embed` as it is for `Search`) together with `config`.
- New optional response fields: `embedding: Option<Vec<f32>>`, serialized finite-only like the existing `numbers::finite` (a non-finite value fails encoding, which the service turns into a `failed` reply), and `model: Option<String>`, the command index name that the vector matches.
- `ServiceClient::repeatable(Embed)` is true: prepare and embed are idempotent.

### Service (`smcli-service`)

- `dispatch` routes `Operation::Embed` to `fn embed(&self, request)`. It takes `searching` (blocking, like `search`), then calls `current_engine(request)`.
  - With `mode == "commands"`, it calls `command_index_locked(&engine, refs)` and then `commands::prepare(&index, refs, platform, shell_path, history_programs, rebuild=false, request.command)`. `command_index_locked` is factored out of `command_index` and does not take `searching` again: `Mutex` is not reentrant, and the existing `command_index` would deadlock.
  - A non-empty `query` sets `embedding` and `model = index.index_name()`. `model` is set even when `mode` is absent, using the same `CommandIndex` naming.
  - A missing `query` returns `bad_request`. A missing tldr data source returns `failed`, and `@@` then shows its status line.
- **Cached command index lifecycle:** today `command_sync.index` is cached forever and holds a `Potion` clone. After a model switch it would keep the old weights resident (two models in one process) and pair old vectors with new query embeddings. To fix this:
  - `load_engine` clears `command_sync.index`, `known` and `last_path` at the same time as it sets `state.engine = None`, before it builds the new engine.
  - `command_index_locked` compares the cached `index_name()` with the one for the current engine and sources, and rebuilds the cache (resetting `known` and `last_path`) when they differ.

### `commands::prepare` (`smcli-search`)

This moves the CLI's `command_index` body (rebuild removes the file; a missing file means build from `os_programs ∪ installed(captured)` on `path`; the requested program, when installed, gets `ensure`) into `smcli_search::commands::prepare`. Captured programs are passed in as a `HashSet<String>`: the CLI computes them with `captured_programs`, the service with `recorded_program` over `history.runs`, which `finish_sync` already does.

### Behavior matrix

"Unusable" covers a refused or failed start, `SMCLI_SERVICE=0`, `connect` with no service, connection-limit `busy`, timeout, `ok:false` (including a `failed` model load) and a decode error. `ServiceClient::send` already maps all of these to `None` or `ok:false`.

| Surface                                             | Service OK                                                                                                                                                                                                                                                                                                                                                                                                                                      | Service unusable                                                                                                                                                                                                                                                                                | Other protocol version                                                                     |
| --------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| `smcli search`                                      | Service hybrid search (unchanged)                                                                                                                                                                                                                                                                                                                                                                                                               | `lexical_search`, plus one stderr line: `smcli: search service unavailable; showing keyword matches only.`                                                                                                                                                                                      | Spawn mode replaces the service and retries (existing). Connect and off modes: as unusable |
| Ctrl-R picker search                                | Service                                                                                                                                                                                                                                                                                                                                                                                                                                         | `lexical_search`, no index lock, no stderr                                                                                                                                                                                                                                                      | As above                                                                                   |
| `@@` picker                                         | Prepare only when needed: index file missing, `embed{mode:commands, command, query:""}` with 600 s (nothing to show without it, the same wait as today's in-process build); program missing from an existing index, the same request with 10 s; otherwise no prepare, and the picker opens at once on the existing rows. Per keystroke `embed{query}` (2 s), then `rank_with_vectors` when `model` equals the CLI's `CommandIndex::reader` name | `rank_words`. **The first unusable reply latches word ranking for the rest of the picker session**, so a stalled service costs at most one 2 s wait. An existing index is read as is. A missing index gives empty rows and the status `command examples need smcli-service or \`smcli reindex`` | As above                                                                                   |
| `smcli indexes [--prune]`                           | Never asks the service: `configured_model` names, `lock_index_directory` for prune                                                                                                                                                                                                                                                                                                                                                              | Same                                                                                                                                                                                                                                                                                            | Same                                                                                       |
| `smcli health`                                      | Never asks the service: `configured_model`. The embedder line comes from the header. No embed probe                                                                                                                                                                                                                                                                                                                                             | Same; an unreadable header is a problem ("rerun `smcli init`")                                                                                                                                                                                                                                  | Same                                                                                       |
| `smcli reindex`                                     | `stop_service`, `model_lock` (5 s), then load and rebuild                                                                                                                                                                                                                                                                                                                                                                                       | Same; it refuses if the lock stays held                                                                                                                                                                                                                                                         | Same                                                                                       |
| `smcli init`                                        | Model unchanged: `describe` only, no load, no stop, no lock. Model changed: `stop_service`, `model_lock` (5 s), then load and the embed check                                                                                                                                                                                                                                                                                                   | Same                                                                                                                                                                                                                                                                                            | Same                                                                                       |
| Service while `reindex`/`init` holds the model lock | `search` and `embed` reply `failed`, and the CLI degrades to keyword search and word rank                                                                                                                                                                                                                                                                                                                                                       | n/a                                                                                                                                                                                                                                                                                             | n/a                                                                                        |
| suggest, picker first draw, files, grep             | Unchanged (no model)                                                                                                                                                                                                                                                                                                                                                                                                                            | Unchanged                                                                                                                                                                                                                                                                                       | Unchanged                                                                                  |

`@@` model mismatch (the service answers with another `model` name, for example a config edited mid-session) is handled as unusable for that keystroke, which gives word rank.

## Work packages

Packages run in order: WP2 and WP3 depend on WP1's types, and WP4 depends on all three.

### WP1: `smcli-search` model-free pieces

Files owned: `crates/smcli-search/src/{potion.rs,index.rs,commands.rs,tldr.rs,setup.rs,lib.rs}`, `crates/smcli-search/src/README.md`.

Changes: `ModelId`, `Potion::describe`, `layout`, `setup::configured_model`, `setup::model_lock`, `index::index_name`, `CommandIndex::reader` and the read-only guard, `plan_query`, `lexical_search`, `tldr::rank_words`, `commands::prepare` (absorbing the in-flight `ensure`). Re-export `ModelId` and `lexical_search` from `lib.rs`.

Regression tests (all `#[test]` in the files above):

- `describe_matches_loaded_model`: bundled and `from_dir` give the same `identity` and `dimension` as the loaded `Potion`, and the same `HistorySearch` and `CommandIndex` names.
- `describe_reads_header_without_weights`: a temp model directory with the real `config.json` and `tokenizer.json` plus only the safetensors header bytes. `describe` succeeds; `Potion::from_dir` fails.
- `describe_rejects_oversized_or_missing_header`.
- `lexical_search_matches_text_fallback`: literal, intent, `$marker`, empty query with a filter, empty corpus with project candidates. Same results as `search_with_evidence_using_lock(.., lock: None)` for literal queries; intent queries return keyword matches and never panic without a model.
- `command_index_reader_reads_and_refuses_writes`: rows written by a model-backed index can be read through `reader`; `add` errors.
- `rank_words_orders_by_overlap_and_prefix`.
- `prepare_builds_missing_index_and_ensures_program`, which replaces `ensure_adds_a_missing_program_once` and keeps its assertions.
- `model_lock_is_exclusive_and_waits`: a second `model_lock(.., ZERO)` returns `None` while the first is held; with a wait, it succeeds once the first is dropped.

### WP2: protocol v4

Files owned: `crates/smcli-core/src/{service_protocol.rs,service_client.rs}`, `crates/smcli-core/tests/service_protocol.rs`, `crates/smcli-core/tests/fixtures/service_protocol/{requests-v4.jsonl,responses.jsonl,README.md}`.

Changes: `VERSION = 4`, `Operation::Embed`, response `embedding` (finite `f32` serializer) and `model`, `prepare_request` sets `shell_path` for `Embed`, and `repeatable` includes it.

Regression tests:

- `requests-v4.jsonl` holds an embed request (query only; `mode:"commands"` with `command` and `shellPath`). It is round-tripped like v2 and v3.
- `responses.jsonl` gains a line with `embedding` and `model`.
- `assert_eq!(VERSION, 4)` and update the literal `{"v":3,...}` lines in `tests/service_protocol.rs`.
- A non-finite `embedding` fails `encode_line`.
- `modes_and_repeatable_operations` covers `Embed`.

### WP3: service `embed`

Files owned: `crates/smcli-service/src/service.rs`, `crates/smcli-service/tests/client.rs`, `crates/smcli-service/README.md`.

Changes: dispatch, `embed`, `command_index_locked` (and `command_index` calls it after `try_lock`), `prepare` wiring, the model lock in `load_engine` and `State`, and clearing or keying the cached command index, all as in [Service](#service-smcli-service) and [Model lock](#model-lock-smcli-search-service-cli).

Regression tests:

- `load_engine_refuses_while_model_lock_held`: with the lock held by the test, `search` replies `failed`; after release, the next search loads.
- `model_switch_drops_cached_command_index`: switch the config A, then B, then A within one service. After each `embed{mode:commands}`, `model` names the current model's index, and that index file holds the rows. After the switch, no `CommandIndex` with the old identity is cached (assert through `command_sync.index` name).
- `embed_returns_vector_and_command_index_name`: length equals `configured_model().dimension`, and `model` equals `CommandIndex::reader(..).index_name()`.
- `embed_commands_mode_builds_then_ensures_program`: the index file appears, and the asked-for program is present.
- `embed_without_query_is_bad_request`.
- `embed_does_not_deadlock_with_background_sync`: `maybe_sync_commands` runs concurrently and `embed` returns within a bound.
- `tests/client.rs`: an embed over the real socket from a spawned service.

### WP4: CLI without the model

Files owned: `crates/smcli/src/{cli.rs,initialize.rs,usage.txt,README.md}`, `scripts/test-service.py`, `scripts/bench-baseline.py`.

Changes:

- `Command::Search` fallback calls `lexical_search` and prints the stderr line.
- The `pick` search closure calls `lexical_search` (the `OnceCell` engine is removed).
- `pick_tldr` uses `configured_model` and `CommandIndex::reader`. It prepares through `embed` only when needed (600 s for a missing index, 10 s for a missing program, nothing otherwise). The ranking closure is factored into `fn tldr_ranker(service, expected_model, entries, vectors, prefix) -> impl FnMut(&str, bool)`: it tries `embed` (2 s), falls back to `rank_words`, and latches word ranking after the first unusable reply. It sets the missing-index status.
- `reindex` and model-changing `init` take `model_lock` (5 s) after `stop_service` and refuse if it stays held.
- `indexes` and `index_health` use `configured_model` and `lock_index_directory`, and the probe embed is removed.
- `Command::Reindex` body moves to `initialize::reindex(captured, home, env, service)` and calls `commands::prepare(rebuild=true)`.
- `init` with an unchanged model uses `describe` (no load, no stop). The "Search will use … (N dimensions)" line takes `ModelId.dimension`.
- `usage.txt`: `SMCLI_SERVICE=0` keeps everything in-process, but search is keyword-only and `@@` word-ranked.
- `test-service.py` and `bench-baseline.py`: raw `"v": 3` becomes a `V = 4` constant.

Regression tests:

- `cli.rs` unit tests:
  - `only_initialize_loads_the_model` (source-scan guard; see [Invariant](#invariant-new-rule)).
  - `tldr_ranker_falls_back_to_words_without_service` (`SMCLI_SERVICE=0` client).
  - `tldr_ranker_rejects_vectors_for_another_model`, using a fake socket that answers with a wrong `model`. Reuse the `old_service.py` fixture pattern, or a thread with `UnixListener`.
  - `tldr_ranker_latches_after_stalled_service`: a fake socket accepts and never replies. The first call returns word rank within about 2 s; the next 5 calls return within 50 ms each without connecting (the fake counts connections).
  - `pick_tldr_skips_prepare_when_index_has_program`: no `embed{mode:commands}` is sent (fake socket counts requests).
- `initialize.rs`: `reindex_refuses_while_model_lock_held`. The test holds `model_lock`; `reindex` with an `SMCLI_SERVICE=0` client errors within about 5 s and has not loaded the model (pointing `potionPath` at a header-only model directory proves it: a load attempt would give a different error).
  - Update `health_checks_coverage_without_rebuilding` and `indexes_prune_respects_lock_and_keeps_current_typed` so they build the index with `search_engine` in test setup (allowed in tests) but call `health` and `indexes` header-only.
- `initialize.rs`: `init_unchanged_model_does_not_load` uses a header-only model directory as `potionPath`: unchanged init succeeds, and switching to it fails at load.
- `test-service.py`:
  - **Lines 477-489** (served equals fallback): the order is no longer compared. Assert that the fallback contains `txbuild signal.md` and `cat signal.md` with the same PATH and file reasons as the served answer, and that stderr has the keyword-only line.
  - **Line 548** ("in-process rebuild"): becomes "an emptied index is left alone without a service". `search -n 1` with `SMCLI_SERVICE=0` still answers `git commit…` lexically, `health` still reports the index unreadable, then a served `search` heals it and `health` passes.
  - **Line 292** (v1 socket fallback): unchanged assertion; it is now keyword fallback.
  - **New "no in-process load" block**: point `SMCLI_CONFIG`'s `potionPath` at a header-only model directory. With `SMCLI_SERVICE=0`: `search` succeeds with the stderr line, `indexes --json` lists a current index whose name contains that identity, and `health` succeeds with the embedder line. `smcli init --potion <dir>` fails. Any CLI path that tried to load would fail the same way.
  - **New served `@@` prepare**: a raw `{"v":4,"op":"embed","mode":"commands","query":"compress a folder"}` returns a 256-float vector, and the `-bundled-tldr@` index file exists afterwards.
  - **New model-lock block**: Python `fcntl.flock` holds `<db>.search/model.lock`, then `service start`. A raw `search` replies `ok:false`; `smcli search` answers with the keyword-only stderr line; `smcli reindex` exits non-zero with the "still holds the model" message. After the lock is released, a served `search` succeeds and `service status` shows one service. This is the respawn race the plan used to leave open.

### WP5: docs (scribe, after WP4 passes)

Files: `AGENTS.md` (Architecture text below), `crates/smcli/src/README.md` (line 5: the CLI links `smcli-search` for keyword search, index naming and `init`/`reindex`, not for runtime embedding), `crates/smcli-search/src/README.md` (new exports), `crates/smcli-service/README.md` (`embed` op), and `agents/plans/roadmap.md` if it lists in-process semantic fallback. Run `make docs`.

## Gate

Per WP, then once at the end:

- WP1: `make test-rust PKG=smcli-search`, `make ranking` (the `plan_query` refactor touches the shared search preamble; scores must not move).
- WP2: `make test-rust PKG=smcli-core`.
- WP3: `make test-rust PKG=smcli-service`.
- WP4: `make test-rust PKG=smcli`, `make test-service`, `make test-cli`, `make test-picker`.
- End: `make test`, then `make fresh` for the user to try `@@`, Ctrl-R and `smcli search` with the service stopped (`SMCLI_SERVICE=0`).

`make linux` is optional: no platform-specific code changes.

## AGENTS.md Architecture text to update

Replace:

> Two executables sharing one copy of the potion-shell-16m model (fine-tuned potion-code-16M-v2, `Models/potion-shell-16m`), shipped as a `potion-shell-16m/` folder beside them (`make release` copies it into `dist/`, `install.sh` into the prefix; dev builds and tests fall back to the checkout's `Models/`). Either can load it, so each can search alone:

with:

> Two executables sharing one copy of the potion-shell-16m model (fine-tuned potion-code-16M-v2, `Models/potion-shell-16m`), shipped as a `potion-shell-16m/` folder beside them (`make release` copies it into `dist/`, `install.sh` into the prefix; dev builds and tests fall back to the checkout's `Models/`). Only `smcli-service` loads it at runtime. The CLI reads the model's identity and dimension from its files (never the weights), and loads the weights only in `smcli init` when switching models and in `smcli reindex`, after stopping the service (all in `crates/smcli/src/initialize.rs`):

Replace:

> On a version mismatch or no service, history and search fall back in-process; service-only file-search ranking evidence is unavailable.

with:

> On a version mismatch or no service, history, suggestions and files fall back in-process. Search falls back to keyword matching and `@@` to word ranking, never to the embedding model. Service-only file-search ranking evidence is unavailable.

In the Models paragraph, replace:

> To avoid holding two models at once, init (when changing models) and `smcli reindex` attempt to stop the service before loading a model; they can proceed if shutdown fails.

with:

> To avoid holding two models at once, whoever has weights resident holds the model lock (`<db>.search/model.lock`): the service from its first model load until exit, and init (when changing models) and `smcli reindex` while they load. Those two stop the service first and refuse if the lock stays held. A service that cannot take the lock answers search as failed, and the CLI degrades. These are the only CLI paths that load a model; a unit test in `crates/smcli` enforces it.

## Deliberate omissions

- **Content-hash model identity** (audit finding #2): identity stays path-based (`model_identity`). This is a separate follow-up; `ModelId` is where it would land.
- **Moving CLI raw SQL into `smcli-core`** (finding #3).
- **A dedicated embed daemon**: `embed` is one op on the existing service, not a new process.
- **Protocol typed-request refactor**: `embed` reuses the flat `ServiceRequest` fields (`query`, `mode`, `command`, `shellPath`).
- **Environment scrubbing and a caller-context object**.
- **Feature-gating `model2vec-rs` out of the CLI binary**: the CLI still links it for `init` and `reindex`. The guard test, not the linker, enforces the rule.
- **A degraded-mode indicator in the history picker**: only `smcli search` prints the stderr line; `@@` shows its status only when the index is missing.
- **Async `@@` preparation inside the picker**: preparing is skipped when the index already has the program, and a stall latches word ranking. Running the build behind an open picker would be a UI change and is left out.
- **Model lock across databases**: the lock is per history database (`SMCLI_DB`). Two databases still mean two services and two models, as today.
- **A live embed check in `health`**: health no longer proves the weights load. `init` and the service's first search do.
- **Ollama and Apple `builtin` embedders**: still unported; the config falls back to Potion as before.

## Adversarial review

Codex (`gpt-6.1-sol`, working-tree scope, 2026-11-04) returned **needs-attention** with three findings. All three held against the code, and the plan was revised for each.

- **[high] The ownership invariant excluded the race that violates it.** Accepted. `ServiceClient` spawn mode lets any capture restart the service while `reindex` runs, and `warm_search` then loads a second model. The fix is the [model lock](#model-lock-smcli-search-service-cli): the service refuses to load while `reindex`/`init` hold the lock, and `reindex`/`init` refuse rather than proceed when shutdown fails. It is tested in WP1, WP3, WP4 and `test-service.py`. The AGENTS.md "can proceed if shutdown fails" text changes with it.
- **[high] The cached command index was not invalidated on a model switch.** Accepted. `command_sync.index` is cached forever (`service.rs:434-449`) and holds a `Potion` clone, so after a switch it would keep the old weights resident and mix old vectors with new query embeddings. The fix: `load_engine` clears it with the engine, and `command_index_locked` re-keys it by `index_name()`. The A→B→A test is in WP3.
- **[medium] The 600 s prepare blocked the picker from opening.** Accepted in part. Prepare is skipped when the index already holds the program, uses 10 s when only the program is missing, and keeps 600 s only when the index file is missing (nothing to show without it, and the same wait as today's in-process build). Per-keystroke embedding latches to word ranking after the first unusable reply. The recommended **asynchronous preparation behind an open picker was rejected** as a UI change larger than this plan needs once the two bounds above apply.

Rejected findings: none outright; only the async part of the third finding (reason above).
