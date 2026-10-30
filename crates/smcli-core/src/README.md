# smcli-core/src

## Overview

`smcli-core` shares captured-history SQLite, capture filtering, parsing, ranking, service protocol and socket transport between both executables. Captured-history writes use `history.rs:CapturedHistory`; configuration writes use `config.rs:SmcliConfig::save`. The crate does not link fff-search.

## Interface

- `config::home_dir` — resolves `$HOME`, falling back to `pw_dir` then `/`.
- `config::SmcliConfig` — user settings; `location`, `load`, `save`.
- `config::CaptureFilter` — `standard`, `new`, `allows_run`, `allows`.
- `context::ProjectContext::discover`, `context::real_path`, `context::NODE_MANAGERS`.
- `context_cache::GitStatus::{clean, parse, read, run_git}`; `context_cache::ContextCache::{new, context, invalidate}`.
- `entry::{command_key, command_parts, trim_slash}`; `entry::SearchEntry::{new, from_key, key, primary, secondary, program, subcommand, last_run_epoch}`; `service_protocol::Suggestion::key` (implemented in `entry.rs`); `entry::SearchFilter::{merge_parsed, is_empty, allows}`.
- `history::{USAGE_HALF_LIFE, INTERRUPTED_EXIT_CODES, VERSION, answers_with_exit_code, user_name, parse_pipestatus, is_timestamp, is_event_id}`.
- `history::{CommandStats, Run, NewRun}`; `history::CapturedHistory::{new, location, open_connection, append, check_separate, record_selection, selection_scores, record_pick, picks, picker_recents, previous_command, followers, runs, command_stats, suggestions}`.
- `parse::{CommandFlag, CommandBreakdown}`; `parse::{command_flags, shell_words, safe_expansion, is_smcli_command, program_of, glob_match, path_placeholders, path_arguments, last_command_word}`.
- `ranking::{Signal, CandidateKind, Candidate, Pick, SelectionLog}`; `ranking::RankingContext::{new, disabled, uses, in_project, candidates_of}`; `ranking::{uses, candidates, apply_ranking_signals, has_prefix, suggestion_why}`.
- `service_client::ServiceClient::{new, socket, send}`.
- `service_location::ServiceLocation::{new, fingerprint, prepare}`; `service_location::{build, relative_insert}`.
- `service_protocol::{VERSION, MAX_SHELL_PATH_BYTES, MAX_MESSAGE_BYTES, MAX_RESPONSE_BYTES, IO_TIMEOUT, MAX_CONNECTIONS}`; `service_protocol::{Operation, ServiceRequest, ServiceResponse, Failure, FileHit, FileIndexInfo, SearchFilter, Suggestion, SearchEntry, ProjectContext, Repository, Project, GitStatus}`; `service_protocol::{shell_path_snapshot, encode_line, decode_line}`.
- `suggestion_index::{CommandAffinity, SuggestionIndex}`.
- `time::{now, format_iso8601, parse_iso8601}`.
- `tests_discovery.rs:{test_names, fn_test_command}` — production helpers that discover test names and construct validated runner commands, not a test-only module.
- `unix_socket::{SocketError, SocketConnection, listen_unix, connect_unix}`.

Subdirectories: none (this is the leaf module directory; `../tests/` holds integration tests and the frozen wire fixtures).

## Invariants

- Native shell history is never rewritten; database access belongs to `history.rs:CapturedHistory`; config writes belong to `config.rs:SmcliConfig::save`.
- Database file must be a regular file, owned by the current uid, no group/other permission bits (created `0600`), not a symlink (`history.rs::CapturedHistory::connect`).
- `SMCLI_DB` must not be the shell's native `HISTFILE`, by path or hard link (`history.rs::CapturedHistory::check_separate`).
- Storage includes `sessions` and `commands` (UUID deduplication, soft deletion, pipeline statuses and captured alias expansion), plus explicit `file_selections` and `command_selections` (`history.rs::SCHEMA_V1`, `MIGRATION_TO_V2/V5/V6/V7/V8`). Schema is versioned (`history.rs::VERSION == 8`); a newer `user_version` is rejected, version `4` is discarded to `3`, and a `.v7.bak` backup is taken before the v8 migration (`history.rs::backup_before_v8`).
- Timestamps are finite and in `0.0..=1e11` (`history.rs::is_timestamp`); event IDs must have lowercase hexadecimal UUID shape (the validator does not enforce version bits) (`history.rs::is_event_id`).
- Command keys are the plain command, or a NUL-prefixed JSON pair when an expanded command exists or the command itself starts with NUL (including native input), so variants stay unambiguous and collision-free (`entry.rs::command_key`).
- `parse.rs::shell_words` tokenizes without execution or expansion and refuses unquoted compound shell syntax. `parse.rs::safe_expansion` validates inert record-time alias text and permits `&&`, `||`, `|` and `;`; it rejects substitution, redirection, background execution and assignments, but does not establish that the text is a single simple command.
- The service runtime directory must be a private directory owned by the uid with no group/other access (`service_location.rs::ServiceLocation::prepare`).
- Wire codec: LF-terminated compact JSON; `encode_line`/`decode_line` own only (de)serialization, not size/deadline limits (see `../tests/fixtures/service_protocol/README.md`).
- `path_placeholders` returns `None` when no path was rewritten; offsets refer to the rewritten string (`parse.rs::path_placeholders`).

## Side Effects

- Disk: creates/writes the SQLite database at `SMCLI_DB` or `~/.local/share/smcli/history.db` (WAL mode, `synchronous=NORMAL`); creates the config at `SMCLI_CONFIG` / `$XDG_CONFIG_HOME/smcli/config.json` / `~/.config/smcli/config.json`; creates the runtime directory, socket and lock files; writes a `.migration.lock` and `.v7.bak` during migration.
- Processes: `service_client.rs::ServiceClient::launch` spawns `smcli-service` detached via `posix_spawn` (no terminal, stdio → `/dev/null`, extra descriptors closed); `send`/`replace` can stop and replace a stale/mismatched-version service.
- Background work: `context_cache.rs::ContextCache::start_refresh` starts Git-status refresh threads; `FileWatcher` subscribes to filesystem notifications via `notify` and invalidates cached context.
- `context.rs::ProjectContext::discover` only stats and reads small files — it never runs Git or executes a manifest. `context_cache.rs::GitStatus::read` is the exception: it runs `/usr/bin/env git --no-pager -C <root> status --porcelain=v2 --branch` (no shell, `GIT_OPTIONAL_LOCKS=0`, killed after its timeout).

## Verification

- Run: `make test-rust PKG=smcli-core` (from the repo root), or the full gate `make test`.
