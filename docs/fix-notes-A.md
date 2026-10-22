# Batch A fix notes

Agent: pikcode (fix-A). Scope: `crates/smcli` plus one-line/shared changes in `smcli-search`, `smcli-core/config.rs` and `smcli-core/service_client.rs`.

All changes followed the batch rules: graphify query first, smallest complete change, no `make`/workspace build, one `cargo check -p smcli --tests` at the end. `graphify update .` was run after edits.

## Changes

### #2 — folder pick in cwd inserts `./`
- `crates/smcli/src/picker.rs:654` `filter_kind`: fallback to `"./".to_string()` when the matched file's `insert` has no `/`.
- `crates/smcli/src/picker.rs:784` added unit test: `filter_kind([hit("src/a.rs", "a.rs")], 1)` produces `[("src/", "./")]`.

### #6 — shell hooks pass `--` before the command line
- `crates/smcli/src/shell_integration.rs:212` bash `_smcli_pick`: `pick "$@" -- "$READLINE_LINE"`.
- `crates/smcli/src/shell_integration.rs:340` zsh `_smcli_pick`: `pick "$@" -- "$BUFFER"`.
- `crates/smcli/src/shell_integration.rs:459` fish `_smcli_pick`: `pick $argv -- (commandline | string collect)`.

### #8 — prune rebuilds the search index after removing records
- `crates/smcli/src/cli.rs:593` `prune` signature now takes `home: &Path` and `env: &HashMap<String, String>`.
- `crates/smcli/src/cli.rs:419` call site updated to pass `&home, &env`.
- `crates/smcli/src/cli.rs:670-675` after a non-dry prune that removed `> 0` records, call `search_engine(c, home, env)?.reindex(&corpus(c, home, env, None, now())?)`.
- `crates/smcli/src/cli.rs:1256,1258` updated existing test calls to supply `&dir, &HashMap::new()`.

### #13 — health check uses `HistorySearch::index_name()`
- `crates/smcli-search/src/index.rs:105` `index_name` is now `pub`.
- `crates/smcli/src/cli.rs:1051` `index_health` builds the index path with `engine.index_name()`.
- `crates/smcli/src/cli.rs:1069` loop uses `let name = engine.index_name();` instead of the duplicated format string.
- Deleted the duplicated format string and its "must match" comment.

### #15 — shared `home_dir` helper
- `crates/smcli-core/src/config.rs:11` added `pub fn home_dir(environment: &HashMap<String, String>) -> PathBuf` (moved from `service_client.rs`).
- `crates/smcli/src/cli.rs:184` `run()` now uses `home_dir(&env)` instead of `HOME.unwrap_or("/")`.
- `crates/smcli-core/src/service_client.rs:34` `ServiceClient::new` now uses `home_dir(environment)`.

### #12 — glibc service launch closes inherited fds
- `crates/smcli-core/src/service_client.rs:114` doc comment now notes macOS/glibc close inherited descriptors above stdio while musl keeps them.
- `crates/smcli-core/src/service_client.rs:143-146` on `#[cfg(all(target_os = "linux", target_env = "gnu"))]` call `posix_spawn_file_actions_addclosefrom_np(&mut actions, 3)` and fold its status into `status`.

### #17 — `shell-init` reports a broken config instead of silently defaulting
- `crates/smcli/src/cli.rs:208-215` on `SmcliConfig::load` error, `eprintln!("smcli: {e:#}; using default keys")` and continue with `SmcliConfig::default()`.

### #18 — picker leaves alternate screen on setup error; drop unused `height`
- `crates/smcli/src/picker.rs:736` `TerminalSession::new` no longer takes `height`.
- `crates/smcli/src/picker.rs:744` keeps a cloned `display` handle; on error it executes `LeaveAlternateScreen` on the original `tty` before `disable_raw_mode`.
- `crates/smcli/src/picker.rs:260` and `crates/smcli/src/menu.rs:135` call sites updated to `TerminalSession::new()`.

## Tests

- Added one unit test in `picker.rs` for the cwd folder fallback.
- Updated existing `cli.rs` prune tests for the new signature.
- `cargo check -p smcli --tests` passed.

## Not done / out of scope

- Batch B and C findings were not touched.
- No full `make test`, `make ranking`, or `make fresh` runs (orchestrator per fix-plan).
- No commit.
