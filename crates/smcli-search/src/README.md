# smcli-search/src

## Overview

Every production module of `smcli-search`. One request runs three stages: `setup::corpus` builds a `HashMap<String, SearchEntry>` from captured statistics plus native history lines (`corpus::search_corpus`), with `corpus::CorpusState::add` as the incremental form; `index::HistorySearch::search_with_evidence` parses the query, syncs the TalaDB index and returns a bounded candidate pool, or falls back to word matching when another process holds the index; `corpus::rerank_with_evidence` applies usage and request-scoped evidence to produce the final order and each entry's `why` list. Shell text for both retrieval legs comes from `tokenizer::ShellTokenizer`; the embedding leg is `potion::Potion`.

No subdirectories: all modules sit directly in this path.

## Interface

- `lib.rs`: `pub mod corpus`, `evidence`, `identifiers`, `index`, `native`, `setup`; private `filesystem`, `potion`, `tokenizer`. Re-exports `ProjectEvidence`, `HistorySearch`, `Potion`, `ShellTokenizer`. `command_file_presence(commands, directory, evidence) -> Vec<bool>` and `command_git_activity(...) -> Vec<f64>` return per-command evidence vectors for an external caller.
- `corpus.rs`: constants `NATIVE_HALF_LIFE`, `LEXICAL_LIMIT`; `search_corpus`; `CorpusState` (`new`, `add`); `search_document`; `is_literal_query`, `program_arguments`; `query_filter`, `parse_markers`; `SearchSignals` (`additions`); `type Hit`; `entry_coverage`, `coverage`; `rerank`, `rerank_with_evidence`. Crate-internal: `embedding_text`, `lexical_text`.
- `index.rs`: `IndexedCommand` (`new`) is the TalaDB row (stored fields `doc`, `command`, `terms`, `embedding`, `program`, `subcommand`, `flags`, `author`, `dirs`, `lastRun`, `runs`, `meta` (the Rust field is `last_run`)); `index_filter(&SearchFilter)`; `HistorySearch` (`new`, `index_name`, `search`, `search_with_evidence`, `reindex`). Private: `VERSION`, `sync`, `open_index`, `setup_index`, `open_cached`, `files`, `remove_files`, `remove_old_indexes`, `lock_index`, `prepare_directory`, `text_fallback_with_evidence`, `addition_filter`, `has_constraint`, `index_cache_unusable`, `expansion_terms`, `merge_expansion`.
- `potion.rs`: `Potion` (`NAME`, `bundled`, `from_dir`, `dimension`, `embed`); private `bundled_dir`, `model_identity`.
- `tokenizer.rs`: `ShellTokenizer` (`LABELS`, `tokenize`, `encoded`).
- `setup.rs`: `corpus`, `search_signals`, `working_directory`, `search_engine`. The engine uses configured `potion_path` or bundled Potion; retained legacy Ollama/Apple settings do not select those backends.
- `native.rs`: `Shell`, `HistoryFile` (`read`), `history_files`.
- `evidence.rs`: `IdentifierEvidence`, `command_flags`, trait `ProjectEvidence` (`exists`, `git_activity`, `find_identifiers`, `grep_terms`, the last three defaulting to empty results).
- `identifiers.rs`: `identifiers`, private `CAP`.

## Invariants

- Corpus keys are `smcli_core::entry::command_key`; `SearchEntry::primary()` is the executable text (alias expansion when aliases are on) and `secondary()` the typed alias. Document identity is full command plus intent, the embedded text is a 1,500-character cut and the lexical text is capped at 8,192 (`crates/smcli-search/src/corpus.rs:search_document`, `embedding_text`, `lexical_text`). Two commands sharing a long prefix stay distinct rows while embedding identically.
- Each native non-empty non-smcli line is one entry and decays by position, halving every 1,000 lines from the end; a captured command keeps the larger of captured and native usage (`CorpusState::new`). `CorpusState::add` must match what a rebuild from `command_stats` would produce after the same runs (`CorpusState::add`).
- Tokenization keeps each whole shell word plus its alphanumeric and camel/Pascal parts, drops label lines after the first, lowercases everything, and encodes terms as `t` + hex of their UTF-8 bytes so TalaDB's splitter keeps them whole (`crates/smcli-search/src/tokenizer.rs:ShellTokenizer::tokenize`, `encoded`).
- Literal queries weight keywords 0.7 / dense 0.3, each normalized by its own best; other queries use TalaDB rank fusion (`crates/smcli-search/src/index.rs:HistorySearch::search_with_evidence`). `$word` markers are recognized only as whole tokens with plain bodies, and a resolved marker is strict — no fallback to the explicit filter. Automatic filters require a unique program, a subcommand from that program's own arguments, and a unique directory basename (`crates/smcli-search/src/corpus.rs:parse_markers`, `query_filter`; `index.rs:addition_filter`, `has_constraint`).
- `SearchFilter::allows` is the final check on every hit even after the index filter narrowed candidates, and explicit filter values win over parsed ones (`crates/smcli-search/src/index.rs:index_filter`; merge in `smcli-core`).
- Sync is identity-based: rows whose `doc` changed or vanished are deleted, rows with unchanged `doc` and a changed `meta` fingerprint get a metadata-only update, and only new identities are embedded, 256 at a time (`crates/smcli-search/src/index.rs:HistorySearch::sync`). `dirs` holds the last directory and every ancestor to `/`, so a directory filter is a membership test (`IndexedCommand::new`).
- Retrieval pool is `(limit * 4).max(40)`. Only a corrupt or incompatible cache is discarded; any other error is propagated with the cache kept (`open_cached`, `index_cache_unusable`).
- Filesystem evidence is request-scoped and positive-only: index-backed file/Git evidence covers the whole hit pool, with up to 4 candidate paths per hit. Fallback disk probing covers at most 40 hits with one probe thread and an 8 ms receive timeout; acquiring the probe slot can loop through timed waits, so 8 ms is not an end-to-end bound. Anything unprobed or unparsable is neutral, never negative (`crates/smcli-search/src/filesystem.rs:POOL`, `PATHS`, `WAIT`, `PROBES`, `evidence_with_index`). A program is available only for a plain name, in a PATH snapshot of at most 8,192 bytes and 64 absolute entries, on a local filesystem, and only via an executable that is either not a shebang shim or a shebang with a payload (`available_in`, `script_body`, `builtin`, `local`).
- Identifier evidence is one bounded lookup per request: at most 64 plain identifiers, program and leading assignments skipped, flags/paths/numbers/stop words/shell syntax dropped, 20 ms budget; names the budget missed are absent, which is neutral (`crates/smcli-search/src/identifiers.rs:identifiers`, `CAP`; `corpus.rs:identifier_hits`). `ProjectEvidence` describes one request: missing facts are unknown, never negative (`crates/smcli-search/src/evidence.rs:ProjectEvidence`).
- Every ranking signal is gated by `smcli_core::ranking::uses` and may only raise a score except the success-rate multiplier, which demotes (`crates/smcli-search/src/corpus.rs:rerank_with_evidence`). A pick floors relevance at `(0.5 + 0.25 * weight).min(1.0)`, and project hints/scripts/tests count only by query-word coverage (`SearchSignals::pick_relevance`, `additions`, `entry_coverage`).
- Expansion terms come from `ProjectEvidence::grep_terms` under a 20 ms budget, only when no hit already covers 75 % of the query, filtered to 3–128 ASCII identifier characters, deduplicated, limited to 16, and worth at most `0.15 * score` (`crates/smcli-search/src/index.rs:expansion_terms`, `merge_expansion`).
- Empty query with a non-empty filter lists matches by most recent run; an empty query with no filter is an error (`HistorySearch::search_with_evidence`).
- The index name is `rs-commands-v3-potion-shell-16m-<identity>-d<dimension>` (plus `-typed` when aliases are off). A model identity is a 32-character path slug plus a 16-hex-digit hash of the canonical path, keeping file names within 255 bytes (`crates/smcli-search/src/index.rs:VERSION`, `HistorySearch::index_name`; `potion.rs:model_identity`). Text with no known tokens embeds to a fixed `1/sqrt(len)` vector, leaving its ranking to the keyword leg (`Potion::embed`).
- `Potion` is documented as loaded once per process and embedded in batches (`crates/smcli-search/src/potion.rs:Potion`); nothing in the type prevents loading a second model, so "one model per process" is a caller convention.
- `working_directory` prefers the shell's `$PWD`, kept as typed, and falls back to the process directory (`crates/smcli-search/src/setup.rs:working_directory`).

## Side Effects

- Index writes under `<history-db-path>.search/`: `<name>.taladb` and engine sidecars, plus a `lock` file (`crates/smcli-search/src/index.rs:HistorySearch::files`, `lock_index`). `prepare_directory` creates the directory with mode `0700` and refuses anything that is not a real directory owned by the current uid without group/other bits.
- Index deletion: `reindex` removes the current name's files first, `open_cached` removes them when the cache is judged corrupt, and `remove_old_indexes` deletes `rs-commands-v*` generations that are not the current `VERSION`.
- Locking: `lock_index(attempts)` tries `File::try_lock` every 50 ms; `search_with_evidence` passes 20 and falls back to text-only search, `reindex` passes `u32::MAX` and fails instead.
- Probe reads inside the bounded worker: `statfs`, metadata, executable-permission and short-file reads of PATH candidates and input paths (`crates/smcli-search/src/filesystem.rs:available_in`, `present_in`, `local`). Results are cached per request.
- Shared state: the probe gate is a process-wide `Mutex`/`Condvar` pair (`filesystem.rs:GATE`, `GATE_CV`); an overrunning probe marks it hung so later requests return neutral evidence until it returns.
- Reads: native history files including session directories and `$HISTFILE` (`crates/smcli-search/src/native.rs:history_files`), the model directory (`potion.rs:bundled_dir`), and the history database and `config.json` (`crates/smcli-search/src/setup.rs:corpus`, `search_signals`, `search_engine`). `search_signals` opens the database only when the file exists and reads at most 5,000 picks.
- No network I/O and no subprocess execution anywhere in the crate.

## Verification

- Run: `make test-rust PKG=smcli-search`
- Run: `make ranking` (scored, floor-checked ranking benchmark)
