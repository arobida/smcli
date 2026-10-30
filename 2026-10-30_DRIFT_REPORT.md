# EGM Drift Report — 2026-10-30

## README.md

- **Medium — Keyword-label claim is incorrect.** “Keyword documents retain contextual labels” contradicts `crates/smcli-search/src/corpus.rs:lexical_text` (lines 165–166), which calls `render_document(entry, false, LEXICAL_LIMIT)`. The unlabeled branch (lines 176–178) includes executable text, the typed alias when present, and intent, but not generated `command:`, `subcommand:`, `flags:` or `author:` labels. Describe the actual lexical input rather than the unused labeled rendering.
- **Low — Native-history coverage omits Fish.** The introduction and native-file enumeration describe Bash/Zsh only, but `crates/smcli-search/src/native.rs:history_files` also reads `~/.local/share/fish/fish_history`, and `HistoryFile::read` decodes its `- cmd:` entries. Add Fish to the input-format overview.

## crates/README.md

No drift detected.

## crates/smcli/README.md

- **Low — Side-by-side installation is a default layout, not an unconditional invariant.** The contract says both binaries and the model folder “must” be installed side by side. `crates/smcli-core/src/service_client.rs:ServiceClient::new` accepts an absolute `SMCLI_SERVICE_BIN` override, while `crates/smcli-search/src/setup.rs:search_engine` loads a configured `potionPath`; `potion.rs:Potion::bundled_dir` also supports the checkout fallback. Qualify the requirement as the default bundled installation layout and mention the supported overrides.

## crates/smcli/src/README.md

- **Medium — Local file/grep fallback is incompletely described.** The overview says file/grep search is delegated to the service, and the interface lists bounded `complete_local` but omits `cli.rs:LocalFileSearch`. In `crates/smcli/src/cli.rs:pick` (file-search closure), a failed service request calls `LocalFileSearch::search`, including local content grep. Its `walk` recursively traverses directories, follows `Path::is_dir` results and skips `.git`, without the depth/entry budgets used by `complete.rs`. Document this separate fallback and its filesystem-read/traversal behavior; do not imply all file/content search requires the service or inherits completion's limits.
- **Medium — Model-stop wording overstates the guarantee.** “Model changes stop the service first so two models are never loaded at once” is stronger than `crates/smcli/src/initialize.rs:run`: it attempts shutdown only when `wanted != config.potion_path`, and proceeds to load the model even if `cli.rs:stop_service` returns false. `stop_service` reports the result of a request rather than enforcing successful termination. Describe the conditional/best-effort shutdown, or separately change code if a hard guarantee is required.

## crates/smcli-core/README.md

No drift detected.

## crates/smcli-core/src/README.md

- **High — Shell-validation contract conflates two different grammars.** The invariant says both `shell_words` and `safe_expansion` refuse compound syntax. `crates/smcli-core/src/parse.rs:shell_words` rejects unquoted `|`, `&`, `;` and related syntax, but `safe_expansion` explicitly permits `&&`, `||`, `|` and `;` as inert record-time alias text (lines 169–187). Both avoid execution, but callers must not treat `safe_expansion` as proof that text is a single simple command. Split the contracts and preserve the substitution/redirection/background/assignment exclusions of the alias validator.
- **Low — Command-key exception is missing.** The key invariant only describes JSON encoding when an expanded command exists. `crates/smcli-core/src/entry.rs:command_key` also encodes a command starting with NUL even when `expanded` is `None`, to escape the sentinel and preserve collision freedom. Add that native-input case.
- **Low — One advertised public symbol path is not exported.** `entry::Suggestion::key` is not a public path: `crates/smcli-core/src/entry.rs` re-exports only `SearchEntry` and `SearchFilter`, and implements the method on `crate::service_protocol::Suggestion`. `history.rs` publicly re-exports `Suggestion`. Use `service_protocol::Suggestion::key` or `history::Suggestion::key`, citing `entry.rs` as the implementation location.
- **Medium — Storage and watcher contracts are missing from the leaf boundary.** `crates/smcli-core/src/history.rs:SCHEMA_V1` and `MIGRATION_TO_V2/V5/V6/V7/V8` define `sessions`, `commands`, `file_selections` and `command_selections`, including UUID deduplication, soft deletion, pipeline statuses and captured alias expansion. The contract lists the schema version but not these storage surfaces. Its side-effects section also omits `context_cache.rs:ContextCache::start_refresh` and `FileWatcher`, which start background work and subscribe through `notify`. Add a compact table/behavior boundary and watcher/thread side effects rather than making readers infer them from type names.

## crates/smcli-search/README.md

- **Low — Not every result path is reranked.** The invariant says `rerank_with_evidence` defines order “on every path.” `crates/smcli-search/src/index.rs:HistorySearch::search_with_evidence` returns early for an empty query with a nonempty filter, sorting by last-run time, then runs, then command (lines 131–141). That branch bypasses index synchronization and reranking. State the filtered-listing exception.

## crates/smcli-search/src/README.md

- **Medium — Evidence limits are applied to the wrong scope.** The invariant describes all filesystem evidence as at most 40 hits with an 8 ms wait. In `crates/smcli-search/src/filesystem.rs:evidence_with_index`, the index-backed branch processes `hits.iter()` and resizes results to the entire pool before the `hits.len().min(POOL)` probe branch. The 40-hit/one-thread constants constrain fallback disk probing, not index-backed file/Git evidence. `acquire_probe` also loops over timed condition-variable waits, so `WAIT` is not a guaranteed end-to-end latency bound. Separate index evidence, probe work limits and queue/receive waits.
- **Low — Serialized row field is named incorrectly.** The interface calls the stored fields camelCase but lists `last_run`. `crates/smcli-search/src/index.rs:IndexedCommand` has Rust field `last_run` under `#[serde(rename_all = "camelCase")]`, while `metadata`, `index_filter` and `setup_index` use the persisted field `lastRun`. Distinguish the Rust field name from the stored schema name.
- **Medium — Legacy-model configuration fallback is undocumented.** The interface mentions `setup::search_engine` without its explicit compatibility shortcut. `crates/smcli-search/src/setup.rs:search_engine` uses `potion_path` when present and otherwise bundled Potion, ignoring legacy Ollama/Apple model-selection fields. `crates/smcli-core/src/config.rs:SmcliConfig` retains those fields for round-tripping. Document that keeping those settings does not select those backends in this implementation.

## crates/smcli-service/README.md

- **Medium — The stated 64-thread cap is not enforced before spawning.** The side-effects section says per-connection threads are capped at 64 active. `crates/smcli-service/src/service.rs:Service::run` spawns a thread for each accepted stream (lines 152–157), and `Service::serve` checks `MAX_CONNECTIONS` afterward (lines 176–186). Excess requests get a busy response, but threads are still created to reject them. Describe this as an admission limit, not a hard thread-count bound; a true spawn cap would require a separate code change.

## crates/smcli-service/src/README.md

- **High — File-selection containment is only lexical.** The invariant says selection paths stay “inside the searched root.” `crates/smcli-service/src/file_search.rs:FileSearch::select` (lines 351–369) rejects absolute paths, NUL, empty components and `..`, resolves the root, then passes the relative string to the record callback. It does not canonicalize `root.join(path)`, check symlink resolution or require the target to exist. Document the actual validation and metadata-only recording; do not claim a filesystem-containment guarantee that could be reused as a security boundary.
- **Medium — Thread bound repeats the crate-level overclaim.** The side-effects section likewise calls `MAX_CONNECTIONS == 64` a cap on active per-connection threads. The check is inside `Service::serve`, after `Service::run` spawns the thread. Apply the admission-versus-spawn distinction described in the crate-root finding here too.
- **Medium — Content-index scope and test-cache lifecycle are missing.** `crates/smcli-service/src/file_search.rs:content_indexing_enabled` and `scoped_index` apply a content-indexing policy outside repositories, including disabling content indexing at home/direct-child scopes; they also reuse ready ancestor indexes. `test_candidates.rs:test_candidates`, `Cached::drop` and `prune` maintain a process-wide cache, register invalidation watches, unsubscribe on drop, and expire entries after 30 seconds when needed. The contract names the broad operations but omits these scope/privacy and watcher/cache limits. Add them under invariants/side effects.

## scripts/README.md

- **Medium — Uninstall retention/export branches are not explained.** The interface lists `--keep-data` and `--no-export`, but the side-effects summary unconditionally describes exporting and deleting the database/index/config. `scripts/uninstall.sh` guards both export and data removal on `keep_data`, and skips export on `no_export`. State that `--keep-data` preserves data/settings and `--no-export` deletes without the normal backup; explain that an attempted export failure stops removal.
- **Medium — Runtime cleanup affects more than the selected database.** The uninstall side-effects summary omits `scripts/uninstall.sh`'s runtime-directory loop, which removes every `service-*` entry in the selected/default runtime directories when data removal is enabled. Only the selected database's service is explicitly stopped first. Document the runtime cleanup scope so users with multiple `SMCLI_DB` instances are not led to assume per-database-only cleanup.
