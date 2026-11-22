---
title: Codebase review — 2026-11-22
---

# Codebase review — 2026-11-22

**What this repo does:** smcli saves the shell commands you run in a private SQLite database, then searches and ranks them. It adds a terminal picker for history and files, and `@` completion. A background service holds the search model and open indexes, and the CLI talks to it over a private local socket. The assumed load is one person with dozens of open terminals, one service per database, and up to about 100,000 saved commands.

This review covers commit `e8e65ff`. It changed no application code, and project tests were not run (repo rule). Six auditors each read one area: data writes, the client and service, file search and Git, shell hooks and the CLI, the search index, and ranking with install. Every finding below was checked against the code. Findings marked **reproduced** were run against `dist/smcli` (built from `d8582dc`; later commits are docs only) or in a scratch script. The [2026-11-08 review](codebase-review-2026-11-08.md) is fixed except where noted.

Codex (session `01a200d8-d20c-7852-8c55-d71d618c9725`) reviewed this report adversarially. Its verdicts were checked, and the confirmed corrections are folded in below. Its new arithmetic cases are part of finding 1, and its other missed findings are in the smaller list.

## Must fix

### 1. **A file name inside shell arithmetic runs as a command** — `crates/smcli/src/shell_quoting.rs:60-110`

- **What this is:** Before a picked or completed path goes onto your command line, `insertable` checks that the cursor is somewhere a quoted word is safe, and `quote` wraps it in single quotes.
- **Problem:** The checker treats `$((` and `((` as a normal nested command. Inside arithmetic, Bash and Zsh still run `$(...)`, even within single quotes. **Reproduced:** with `SMCLI_LBUFFER='echo $(( 1 + '`, completing a file named `$(touch PWNED)` printed `'$(touch PWNED)'`. Running the line created `PWNED` in Bash 3.2, Bash 5 and Zsh.
- **Also (Codex, reproduced):** `echo $[1 + ` and array subscripts such as `${a[1+(` run it too, in Bash and Zsh, and smcli accepts both prefixes.
- **Fix:** Return `false` for every arithmetic context: `((`, `$((`, `$[` and `${name[`, next to the existing `<<` check at line 72. Add one test per form.
- **If we skip it:** A hostile file name in a cloned repo runs code when you complete or pick it inside arithmetic and press Enter.

### 2. **One non-UTF-8 environment variable crashes every smcli call** — `crates/smcli/src/cli/mod.rs:218,231`; `crates/smcli/src/shell_integration.rs:34`

- **What this is:** Every command starts by copying the environment and arguments into strings.
- **Problem:** `std::env::vars()` and `std::env::args()` panic on bytes that are not valid UTF-8. **Reproduced:** `BAD=$'\xff' dist/smcli shell-init zsh` panicked with `called Result::unwrap() on an Err value: "\xFF"`. The same panic hits `record`, `pick` and `complete`.
- **Fix:** Build the environment map from `vars_os()` and keep the entries where `to_str()` succeeds. Do not drop arguments, because that shifts positions. Parse `args_os()` and reject a bad argument with a clear error.
- **If we skip it:** One odd variable from another tool stops the hooks from loading and stops all recording, with only a Rust panic as the message.

### 3. **The service can freeze when it drops its 17th repo** — `crates/smcli-service/src/context_cache.rs:80-125`

- **What this is:** The service watches up to 16 Git repos for file changes so it can keep their status fresh. When a 17th repo arrives, it drops the oldest one while holding the cache lock.
- **Problem:** Dropping the entry drops its macOS file watcher. That waits for the watcher's thread to go idle (`notify-8.2.0/src/fsevent.rs:329-348`). If a file event is in progress at that moment, its callback calls `invalidate`, which waits for the same cache lock. Each side waits for the other forever, and every later suggest or picker request that needs project context hangs. Found by reading this code and notify's source. The same "drop after unlock" fix was made for `file_search.rs` in the last round.
- **Fix:** Have `make_entry` return the evicted entry, and drop it after the lock is released.
- **If we skip it:** A heavy user of many repos eventually gets a service that hangs every terminal until it is killed.

### 4. **Install and uninstall delete other tools named `tldr` or `cheat`** — `scripts/install.sh:90-93`; `scripts/uninstall.sh:238-239`

- **What this is:** smcli ships its command-example data as folders named `tldr` and `cheat` in the install folder, `~/.local/bin` by default.
- **Problem:** Both scripts run `rm -rf "$prefix/tldr"` and `rm -rf "$prefix/cheat"`. Those are the names of popular CLIs, and `pipx install tldr` puts exactly `~/.local/bin/tldr` there. Install deletes that program and puts a folder in its place, and uninstall deletes anything installed under those names later.
- **Fix:** Rename the data folders to something smcli owns, for example `smcli-data/tldr` and `smcli-data/cheat`, wherever the binaries look for them.
- **If we skip it:** Installing smcli silently removes another tool the user had.

## Should fix

### 5. **After a pick, the service can stop noticing prunes and imports** — `crates/smcli-service/src/service.rs:766-777`

- **What this is:** The service rebuilds its in-memory suggestions when SQLite's `data_version` changes, which signals a write by another process, such as `smcli prune`.
- **Problem:** After saving a pick, line 774 reads `data_version` again and stores it. A connection's own writes never change that number, so the re-read only matters when another process commits between `refresh` (line 769) and line 774, and then it hides that write. **Reproduced** in a scratch script with SQLite: an outside commit followed by our own commit moves the number, and the stored value swallows it.
- **Fix:** Delete line 774.
- **If we skip it:** Commands you pruned keep appearing in suggestions until some unrelated write.

### 6. **A custom `IFS` stops pipelines from being recorded** — `crates/smcli/src/shell_integration.rs:193,229,622`; `crates/smcli-core/src/history.rs:236-246`

- **What this is:** The hooks send each pipe's exit codes as one word list, for example `0 1 0`.
- **Problem:** `"${PIPESTATUS[*]}"` and `"${pipestatus[*]}"` join with the first character of `IFS`. With `IFS=,`, `true | false | true` sends `0,1,0`, and `parse_pipestatus` rejects it. **Reproduced:** `record` printed `Invalid pipestatus: 0,1,0` and saved nothing. With an empty `IFS` it saves `01` as one wrong code.
- **Fix:** First copy the status array (`local -a codes=("${PIPESTATUS[@]}")`), then join it under a local `IFS=' '`. Setting `IFS` first would run a command and reset the status. This covers lines 193, 229, 233, 622 and 626.
- **If we skip it:** Users with a changed `IFS` lose every pipeline from history.

### 7. **Multi-line Zsh commands are split into fragments** — `crates/smcli-search/src/native.rs:50-66`; `crates/smcli-search/src/setup.rs:22-26`

- **What this is:** Search also reads your existing `~/.zsh_history`, where a multi-line command is stored as lines ending in `\`.
- **Problem:** `HistoryFile::read` joins those lines with `\n`, then `setup::corpus` splits the text on `\n` again. A `for f in *; do` / `echo $f` / `done` entry becomes three commands, so fragments like `done` and `EOF` show up in search.
- **Fix:** Have `read` return one string per command, and drop the join and split in both callers: `setup.rs` and the service's own copy at `service.rs:1033-1037`.
- **If we skip it:** Search shows broken pieces of multi-line commands.

### 8. **A misspelled config key silently turns off privacy filters** — `crates/smcli-core/src/config.rs:22-24`

- **What this is:** `historyFilter` and the other filters in `config.json` decide what is never saved.
- **Problem:** Unknown keys are ignored. `"historyfilter"` or `"history_filter"` loads with no error and no filter, so commands you meant to hide are saved. `smcli init` then drops the misspelled key when it saves.
- **Fix:** Add `#[serde(deny_unknown_fields)]`. The legacy keys are already declared, so old configs still load.
- **If we skip it:** One typo saves the secrets the user tried to block.

### 9. **When the service cannot start, every call waits 2 seconds** — `crates/smcli-core/src/service_client.rs:205-221`

- **What this is:** If no service answers, the client starts one and waits up to 2 seconds for its socket.
- **Problem:** If the service exits at startup, for example on a locked database or a refused database path, the client still waits the full 2 seconds. There is no memory of the failure, so every suggest and record in every shell pays 2 seconds plus a process start.
- **Fix:** Keep the child's pid from `posix_spawn`, and stop waiting when `waitpid(pid, WNOHANG)` shows a non-zero exit. Exit 0 is a normal lost race, so keep waiting then.
- **If we skip it:** A broken service turns every prompt into a 2-second stall instead of a quick fallback.

### 10. **Every search re-checks the whole index** — `crates/smcli-search/src/index.rs:496,865-943`

- **What this is:** `sync` brings the search index up to date with the history corpus.
- **Problem:** It runs on every search, even when nothing changed. Each run builds index rows and texts for every entry (up to 100,000) and reads every stored row's fingerprint, while holding the service's search lock. Other searches wait on that lock. The time was not measured, so measure before fixing.
- **Fix:** Remember the corpus version that was last synced, and skip `sync` when it is unchanged. The key must cover more than the service's `(data_version, history_revision)`: native history files, the model identity, and the typed versus expanded index.
- **If we skip it:** `@@` search slows down as history grows, and parallel searches wait behind it.

### 11. **Shells with different PATHs rerun the command-example sync, holding the main lock** — `crates/smcli-service/src/service.rs:536-544,581,638`

- **What this is:** After a command is saved, the service may sync its examples index for programs on that shell's `PATH`.
- **Problem:** It remembers only the last `PATH`. Two terminals with different `PATH`s (a virtualenv and a plain shell) make nearly every command start a sync. Each sync reads all runs with `self.history.runs(&self.user, &self.lock().db)`, which holds the main state lock for the whole read, so records and suggests from every shell wait.
- **Fix:** Remember what each sync covered (the `PATH` and the set of programs seen), and resync only when a new program or `PATH` entry appears. A set of `PATH` strings alone would miss newly installed programs. Read runs on a separate connection instead of under the state lock.
- **If we skip it:** Normal multi-terminal use causes repeated full-history reads that block every shell.

### 12. **Every suggest parses every saved command** — `crates/smcli-core/src/ranking.rs:145-165,600-610`

- **What this is:** In a project folder, suggestions get a boost when their program matches a project file, such as `make` with a `Makefile`.
- **Problem:** `project_file_reason` runs the full shell parser on every scored candidate for every request. It is skipped when script ranking is off or no project file matches. With a short prefix, that is every distinct command, which is fewer than the number of runs. The auditor measured about 144 ms per 100,000 commands in a release build. I did not re-measure.
- **Fix:** Store each command's program once, when it is added to the index, and look it up here.
- **If we skip it:** Up-arrow and suggest slow down in every project folder as history grows.

### 13. **A failed Git status is retried on every request** — `crates/smcli-service/src/context_cache.rs:100-106,158-162`; `crates/smcli-core/src/context.rs:670-681`

- **What this is:** Git status is cached and refreshed when it is older than its maximum age.
- **Problem:** A failed or timed-out read keeps no timestamp, so the entry stays stale. In a repo whose status takes over 10 seconds, or one that is skipped on purpose because it has local `filter.*` config, this repeats. A quickly failing read (the filtered repo) starts Git again on about every request a second apart. A slow read stays "refreshing" until it times out, then starts again on the next request.
- **Fix:** On a failed read, record the attempt time so the normal maximum age applies.
- **If we skip it:** Large or filtered repos run Git about once a second while you type.

### 14. **`ignoreFile` is not applied to test suggestions and ranking evidence** — `crates/smcli-service/src/test_candidates.rs:157-240`; `crates/smcli-service/src/evidence.rs:225-330`

- **What this is:** `ignoreFile` hides paths, such as `vendor/` or `private/`, from file search and completion.
- **Problem:** Test discovery and the "defined in project" and Git-activity evidence search the file index without it. A `#[test] fn leaked_case` under an ignored `vendor/` is still offered as `cargo test leaked_case`.
- **Fix:** Pass the caller's `FileIgnore` into `ranked`, and drop ignored paths in these three places when reading results. The test cache is shared per root, so filter after reading it, not when filling it, or one caller's rules leak into another's.
- **If we skip it:** Hidden folders still shape suggestions and can show names from them.

### 15. **An old terminal can replace a newer service** — `crates/smcli-core/src/service_client.rs:91-95,118-143`

- **What this is:** A client replaces a service from an older build. The last round added an age check for this (old #6).
- **Problem:** A protocol-version mismatch skips that age check and always replaces. After a protocol bump, a terminal still running the old binary shuts down the new service and starts its own, and new terminals switch it back. Each switch reloads the model and drops requests in flight.
- **Fix:** On a mismatch, replace only when `should_replace` says the running build is older.
- **If we skip it:** After an upgrade, open terminals keep restarting the service until they are closed.

### 16. **Old fix #25 is incomplete: subfolders still get commands that fail** — `crates/smcli-core/src/ranking.rs:226-376`

- **What this is:** The last round meant to stop `make docs`, `./mvnw` and `./gradlew` from being suggested in a subfolder, where they fail.
- **Problem:** Commit `bc0b221` only removed a ranking boost. `candidates()` still offers those commands from a subfolder, though `project_relative` is available.
- **Fix:** Restrict or qualify only the commands that depend on the folder: Make targets (`make -C <root> docs`) and `./mvnw` or `./gradlew`. Cargo and npm commands work from subfolders and should stay.
- **If we skip it:** The original bug remains: suggested commands fail with "No rule to make target".

## Nice to have

### 17. **Pruned text stays in the database file until the next write** — `crates/smcli-core/src/prune.rs:176`; `crates/smcli-core/src/import.rs:107`

- **What this is:** After a prune, the code checkpoints the write-ahead log (WAL, the side file that holds new writes) into the database, then compacts it with `VACUUM`.
- **Problem:** In WAL mode, `VACUUM` writes its clean copy to the WAL, so the old pages, deleted text included, stay in the main file until the next checkpoint. **Reproduced** in one SQLite layout. The text was gone after the next write or close, and was not present with longer rows. So this is a short window, not a lasting leak. The checkpoint can also report busy, and its result is not checked.
- **Fix:** Swap the order to `VACUUM; PRAGMA wal_checkpoint(TRUNCATE);` in both places, and check that the checkpoint was not busy.
- **If we skip it:** A backup taken right after a prune can still hold the removed secret.

### 18. **Glob matching can take exponential time** — `crates/smcli-core/src/parse.rs:585-641`

- **What this is:** `glob_match` matches `@` completion globs against paths, both in the CLI and in the service.
- **Problem:** Each `*` retries the whole rest of the pattern at every position. A pattern that fails at the end forces this: the auditor timed eleven `*a` followed by `b` against 40 `a`s past 60 seconds. The work continues in the service after the client gives up. One such pattern ties up the service every terminal shares. The input is the user's own, so this is unlikely.
- **Fix:** Use the `globset` matcher that ships with the `ignore` crate, which is already a dependency, or switch to the standard iterative matcher.
- **If we skip it:** A pathological glob can hang completion.

### 19. **On macOS Bash 3.2, the hooks replace existing completions** — `crates/smcli/src/shell_integration.rs:473,521-545`

- **What this is:** On Bash older than 4, the hook installs its own `@` completion with `complete -F _smcli_complete printf echo cd pushd z cat npm true`.
- **Problem:** `complete -F` replaces any completion already set for those commands. A Bash 3.2 user (the macOS default) with bash-completion loses its `cd` and `npm` completion as soon as the hooks load. Found by reading.
- **Fix:** Skip a command when `complete -p <cmd>` already shows a completion, or call the old one from `_smcli_complete` when the word does not start with `@`.
- **If we skip it:** Bash 3.2 users lose completions they had before installing smcli.

### 20. **Dead hook function** — `crates/smcli/src/shell_integration.rs:164-173`

- **What this is:** `_smcli_histignore` is defined in every Bash shell.
- **Problem:** Nothing calls it. A search of the whole tree, scripts included, finds only the definition.
- **Fix:** Delete it.
- **If we skip it:** Ten lines in every Bash session that do nothing.

## Smaller findings left out

Twenty-five smaller findings, each checked by reading, were left out of the ranked list:

- Config save replaces a symlinked `config.json` with a plain file (`config.rs:179-191`).
- Search counts other users' rows, while suggestions and the picker do not (`history.rs:683-700`).
- Prune deletes picks of a command that is still allowed in another folder (`prune.rs:116`).
- `smcli reindex` leaves the `-typed` index alone (`index.rs:995-1005`).
- One run dated decades ahead turns suggestion scores into NaN (`suggestion_index.rs:107`).
- The menu's "Search history" treats words starting with `-` as options, so `ls -la` fails (`menu.rs:113`).
- `indexes --prune` deletes the commands index when the bundled data is missing (`cli/index.rs:54-60`).
- The service keeps the starting shell's folder as its working folder for its whole life (`service_client.rs:258-313`).
- A panic in `finish_sync` would leave `running` set, so no sync ever runs again; no input that panics was found (`service.rs:562-598`).
- Bash under `set -u` errors on an unset `HISTCONTROL` (`shell_integration.rs:186,211`).
- Finding a repo root also parses every project manifest, three times per suggest (`file_search.rs:127-129`; `service.rs:678-689`).
- Tab completion in a non-repo home folder builds a content index that the picker avoids (`file_search.rs:147,655`).
- `scoped_index` builds a second index while the parent is still scanning (`file_search.rs:299-331`).
- Test discovery re-reads each hit file with no size cap (`test_candidates.rs:215-240`).
- A `.git` that is a symlink to a folder is not recognized (`context.rs:224-228`).
- `function foo-bar` is seen as a mention of `foo-bar` but never as its definition, because `declares_name` and `contains_ident` disagree on `-` (`evidence.rs:163-204`).
- Uninstall computes the wrong socket name for a non-ASCII database path under Bash 3.2 (`uninstall.sh:215-217`).
- Install adds a blank line on every rerun, and an install folder containing `'` breaks the hook line (`install.sh:103,162-165`).
- A command with bytes that aren't valid UTF-8 is not recorded (`cli/mod.rs:294`); `from_utf8_lossy` would save a different file name, so store raw bytes or keep a clear rejection.
- Zsh starts a subshell on every prompt to read `bindkey` (`shell_integration.rs:825-832`).
- Atuin import keeps exit `-1` and relative folders that `append` rejects (`import.rs:76-102`).
- JSONL import deduplicates only against its own synthetic `imported-<user>-<shell>` session, so importing captured rows duplicates them. Two distinct runs with the same command, folder, exit code and millisecond merge into one, and duration, pipe statuses and IDs are dropped (`import.rs:128-161`).
- `smcli history --json` writes a JSON array, while `smcli import` reads one record per line, so the export cannot be imported directly (`cli/history.rs:91-93`; `import.rs:118-126`).
- Automatic Git runs `/usr/bin/env git`, so with `.` in `PATH` the service runs a repo's own `git` file (`context.rs:698-714`). That `PATH` already runs it whenever you type `git`.
- Any outside write rebuilds the whole suggestion index while holding the state lock (`service.rs:420-437`).

## Checked and correct

- The fixes from the last round for Git `fsmonitor`, Atuin filters, stdin recording, eviction in `file_search.rs`, completion `ignoreFile`, WAL on every connection, and index compaction. Old #25 is only partly fixed (finding 16).
- Socket in an owner-checked `0700` folder, with the peer's uid checked on both ends. A 1 MiB request limit and a total read deadline. Stale sockets are removed only under the service lock.
- The service's environment allowlist and its idle shutdown. The model lock and model identity. Native history files are only ever read.
- Every user-facing history query filters deleted runs. Every caller of a write applies the capture filter (`append` itself does not). Migrations run in one transaction under a lock.

**Verdict:** Fix 1 and 2 first (code execution, and a crash that stops all recording), then 3 and 4.

**Lean:** -10 lines possible (finding 20). Findings 5, 17 and 18 also shrink or delete code.

**Not checked:**

- `picker.rs` beyond about 600 of its 2,581 lines.
- `corpus.rs` from line 262 on, `filesystem.rs`, and `cheat.rs`.
- Test bodies in general.
- The internals of fff-search and TalaDB, and Linux-only code.
- No live pty session in Fish or Bash 3.2.
- No timing under real 100,000-row load, except the parse cost in finding 12.
