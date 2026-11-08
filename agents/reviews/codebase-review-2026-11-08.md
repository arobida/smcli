---
title: Codebase review — 2026-11-08
---

# Codebase review — 2026-11-08

**What this repo does:** smcli records the shell commands you run in a private SQLite database, then searches and ranks them, with a terminal picker for history and files. A background service holds the search model and open indexes, and talks to the CLI over a private local socket. The assumed load is one person, many open terminals, one service per database, and up to about 100,000 saved commands.

The review covers the working tree on top of commit `a85bb44`, including uncommitted changes. It changed no application code. Project tests were not run. Linux-only issues are out of scope and live in [the roadmap](../plans/roadmap.md).

This version includes a Codex second opinion (job `task-mw7t5rcg-ge65gv`). Each Codex finding was checked against the code before being added. Finding numbers are stable identifiers; within each group, findings are listed by importance.

## Must fix

### 1. **Using the picker in a folder can run code from that folder** — `crates/smcli-core/src/context.rs:661-665`

- **What this is:** smcli runs `git status` in your project to help rank suggestions.
- **Problem:** A repo's `.git/config` can name a program in `core.fsmonitor`, and `git status` runs it. A downloaded archive that includes `.git` could run its program when you open the picker there. This exact git command ran such a program in a test.
- **Fix:** Add `-c core.fsmonitor=false` to the git arguments, with a test. In the same test, the program then did not run. Git "filter" settings are a similar route that was not tested.
- **If we skip it:** Opening the picker in an untrusted repo can run someone else's code as you.

### 21. **Tab completion can turn a file name into a command** — `crates/smcli/src/shell_integration.rs:495-508,805-807,1068-1070`; `crates/smcli/src/shell_quoting.rs:17-18`

- **What this is:** Tab on an `@` word completes a file path. One match is inserted straight away.
- **Problem:** The several-matches path checks whether the cursor is inside an open quote, but the one-match path does not. In `echo " @probe`, a file named `probe$(cmd).txt` becomes `echo " 'probe$(cmd).txt'"`, where the single quotes do not protect anything. Running that line in Bash and Zsh executed the substitution. The full Tab flow was not driven.
- **Fix:** Pass the text before the cursor to `smcli complete` in all three shells, and apply the existing `insertable` check before printing.
- **If we skip it:** A hostile file name in a repo can run code when you complete it inside a quoted string and press Enter.

### 2. **Atuin import ignores your "never save this" rules** — `crates/smcli-core/src/import.rs:26,74-90`

- **What this is:** This imports old history from Atuin, another shell-history tool. The installer offers to run it.
- **Problem:** The filter is passed in but never used (`_filter`), although a comment says this is on purpose. A command matching `historyFilter`, or one that looks like an API key, is still saved and searchable. The JSONL import does apply the filter.
- **Fix:** Skip the row when `filter.allows_run(&command, None, &cwd)` is false, and keep tombstone handling for rows already stored.
- **If we skip it:** Secrets you chose to block stay stored until you run `smcli prune`.

### 3. **Uninstall can leave history behind** — `scripts/uninstall.sh:44-45,153-155`

- **What this is:** The uninstaller exports and then deletes your data.
- **Problem:** Shells from an older build can recreate `~/.local/share/smcli/history.db` beside the new database. Uninstall handles only the new one, so the old file is neither exported nor deleted. `.legacy-move.lock` and the `kept-keys-*` files also stop the state folder from being removed.
- **Fix:** Export and delete the old database too, with its `-wal`, `-shm` and `.search` files. Delete smcli's small state files.
- **If we skip it:** Command history stays on disk after the tool says it was deleted.

## Should fix

### 22. **Moving the old database can lose a command just recorded** — `crates/smcli-core/src/history.rs:334-345`

- **What this is:** On first run, the old database is copied to the new location and then deleted.
- **Problem:** The copy is finished and closed before the old file is deleted. A terminal still running an older build does not take the move lock, so a command it records in that gap is in neither place afterwards. Found by reading; the window is short and happens once.
- **Fix:** Keep the old database instead of deleting it. `stranded_legacy` already reports a leftover file, so its message needs to explain the case.
- **If we skip it:** One-time, small loss of recent commands during an upgrade.

### 23. **Uninstall's export can miss the last commands** — `scripts/uninstall.sh:74,88,153`

- **What this is:** Uninstall exports history, stops the service, then deletes the database.
- **Problem:** Other terminals keep recording during those steps. A command finished after the export is deleted without being exported. Found by reading.
- **Fix:** Stop the service before the export. That narrows the gap but does not close it, because the CLI can write to SQLite directly; say so in the uninstall message.
- **If we skip it:** Commands run while uninstalling can be missing from the export.

### 24. **Completion without the service ignores `ignoreFile`** — `crates/smcli/src/cli/mod.rs:307-330`; `crates/smcli/src/complete.rs:7-13,51`

- **What this is:** When the service is off or fails, `@` completion walks the folder itself.
- **Problem:** That walk never reads `ignoreFile`, so files under an ignored `private/` folder are offered. A missing or invalid ignore file makes the service fail, and the fallback then ignores the rule too. The picker's fallback in `pick.rs` does apply it.
- **Fix:** Load `FileIgnore` before falling back, apply it during the walk, and return its errors.
- **If we skip it:** Paths you chose to hide appear in completion.

### 25. **Project suggestions can fail from a subfolder** — `crates/smcli-core/src/ranking.rs:186,245-249,409,417-418`

- **What this is:** smcli suggests commands from the nearest project file above your folder.
- **Problem:** The command text ignores where that file is. In a temporary repo, the installed build suggested `make docs` from `src/`, and running it failed with "No rule to make target". `./mvnw` and `./gradlew` have the same problem by reading.
- **Fix:** Offer these commands only when the current folder is the project root, or write them with the root, such as `make -C <root> docs`.
- **If we skip it:** Suggestions offer commands that fail.

### 4. **Some commands trigger a full rescan every time** — `crates/smcli-service/src/service.rs:125-130,549,567-575`

- **What this is:** After a command is saved, the service adds examples if the program is new.
- **Problem:** It remembers only programs found on `PATH`. Once the examples index exists, `./run.sh`, shell functions and builtins not on `PATH` always look new, and `FOO=1 make` is read as a program called `FOO=1`. Each run scans system folders and reloads all history while holding the search lock, so a search made then has to wait. The delay was not measured.
- **Fix:** Add `sync.known.extend(history);` after line 574. Replace `recorded_program` with the existing `parse::program_of`.
- **If we skip it:** Normal shell use causes repeated background work that grows with your history.

### 5. **A moved database can stay out of WAL mode forever** — `crates/smcli-core/src/history.rs:336,1235`

- **What this is:** WAL is the SQLite mode that lets terminals read and write at the same time.
- **Problem:** WAL is set only during a schema upgrade. `VACUUM INTO` produced a non-WAL copy in a test, so a moved version 9 database never gets it. The same happens if setting WAL fails just after an upgrade.
- **Fix:** Move the `journal_mode = WAL` line out of the upgrade block, so every write connection runs it.
- **If we skip it:** Saves can fail with lock errors. SQLite also rates `synchronous = NORMAL` as less safe outside WAL mode; no corruption was shown here.

### 6. **Two builds keep restarting each other's service** — `crates/smcli-core/src/service_client.rs:101-107`

- **What this is:** The CLI replaces a service that came from a different build.
- **Problem:** Any difference triggers a replacement, even when the running service is newer. With the installed build in one terminal and a `dist/` build in another, each terminal's commands replace the other's service.
- **Fix:** Replace the service only when its build's modification time is older, so an old client never replaces a newer service. That time is already in the build string. Keep replacing on a protocol mismatch, which is the real compatibility check.
- **If we skip it:** The model and file indexes keep reloading during development.

### 7. **File changes run `git status` nobody asked for** — `crates/smcli-service/src/context_cache.rs:107-116,196-198`

- **What this is:** The service watches up to 16 repos and keeps their Git status fresh.
- **Problem:** Every file event starts a refresh, even if no one is asking about that repo. A build writing to `target/` causes `git status` about once a second, for as long as the service stays alive. CPU cost was not measured.
- **Fix:** Have `invalidate` only mark the entry dirty, and delete the restart at lines 196-198. `context()` already refreshes dirty entries on demand. The test at line 354 must change with it.
- **If we skip it:** Idle projects with busy build folders keep using CPU.

### 8. **Picking a symlink inserts a different file** — `crates/smcli-core/src/paths.rs:92`

- **What this is:** This builds the path placed on your command line after you pick a file.
- **Problem:** It resolves the whole path, including the last part. Picking `CLAUDE.md`, a link to `AGENTS.md`, inserts `AGENTS.md`. Existing tests cover linked folders only.
- **Fix:** Resolve only the parent folder, then add the file name back. Add one test.
- **If we skip it:** Commands act on a different path from the one you chose. For example, `rm` removes the target instead of the link.

### 9. **The picker's "recent" list can be in the wrong order without the service** — `crates/smcli-core/src/history.rs:995-999`

- **What this is:** When the service is unavailable, this query lists distinct commands, newest first.
- **Problem:** `SELECT DISTINCT` with this `ORDER BY` can sort by an older run. In a small copy of the query, a command last run at time 3 came after one last run at time 2. Codex reproduced this with the current schema. The current test checks only that a command is present.
- **Fix:** Use `GROUP BY` with `ORDER BY max(started_at) DESC, max(id) DESC`, and test the order.
- **If we skip it:** A command you just ran can appear far down the list.

### 10. **`--version`, subcommand help and error messages are lost** — `crates/smcli/src/cli/mod.rs:249-252`

- **What this is:** This reads the command-line arguments.
- **Problem:** Every parse result that is not a normal command prints the general usage and exits with 2. Running the installed binary confirmed this for `smcli --version`, `smcli search --help` and `smcli suggest --limit 0`.
- **Fix:** Call `e.exit()` for help and version, and print the error before the usage for real errors.
- **If we skip it:** Scripts cannot read the version, and users never see what was wrong.

### 11. **Commands are briefly visible to other users on the machine** — `crates/smcli/src/shell_integration.rs:158,596,909`

- **What this is:** The shell hooks start `smcli record` with the command as an argument.
- **Problem:** This happens before the secret filter runs. On a shared machine, another user's `ps` could catch `export TOKEN=...`.
- **Fix:** Send the command and its expansion over stdin, and read them in `record`. Cross-user visibility was not exercised.
- **If we skip it:** Secrets typed into shell builtins can be exposed for a short time.

### 12. **Two lookups still read deleted runs** — `crates/smcli-core/src/history.rs:959,982-984`

- **What this is:** These lookups choose the file-or-folder picker mode and the folder used to detect paths in a command.
- **Problem:** Unlike the other main history queries, they do not check `deleted_at`. A run deleted in Atuin can still affect the picker.
- **Fix:** Add `AND c.deleted_at IS NULL` to both queries.
- **If we skip it:** Deleted history can still change results.

### 14. **Init has options that only fail, and help describes features that are gone** — `crates/smcli/src/cli/mod.rs:133-138`; `crates/smcli/src/initialize.rs:69-71`; `crates/smcli/src/usage.txt:1,34`

- **What this is:** `smcli init` chooses the search model.
- **Problem:** `--model` and `--builtin` exist only to print "Rust supports Potion only". The help text still offers Ollama and Apple's model.
- **Fix:** Delete both options and correct the help text.
- **If we skip it:** Users are offered choices that always fail.

## Nice to have

### 13. **The service keeps every run in memory for a case that almost never happens** — `crates/smcli-core/src/suggestion_index.rs:100,171-178`

- **What this is:** The suggestion index stores scores, plus a full copy of every run.
- **Problem:** The copy is used only to recalculate when a run is about 19 years newer than the index's base time. Up to 100,000 commands and folders stay in memory for that branch. The cost was not measured.
- **Fix:** Delete `history` and return `false` in that case. The service already rebuilds from the database when `add` returns `false`. The test `rebases_and_keeps_latest_fields` must change with it.
- **If we skip it:** Memory use stays higher than it needs to be.

### 15. **Listing indexed programs loads every embedding** — `crates/smcli-search/src/commands.rs:263-297`

- **What this is:** `programs()` returns the program names in the examples index.
- **Problem:** It decodes every row, vectors included. `fingerprints` in `index.rs` already avoids this because decoding is slow. The cost here was not measured.
- **Fix:** Use the same `Stage::Project` read for the `program` field.
- **If we skip it:** Each sync does extra work.

### 16. **Wrong error for a missing import file** — `crates/smcli-core/src/import.rs:8-16`

- **What this is:** This check stops a database from being imported into itself.
- **Problem:** On a new install, a mistyped path says "Cannot import history into itself", because both failed lookups compare equal.
- **Fix:** Check that the source exists first.
- **If we skip it:** The error sends the user in the wrong direction.

### 17. **Removing a file index can block all file searches** — `crates/smcli-service/src/file_search.rs:56-63,158,201-210,896-902`

- **What this is:** The pool of file indexes evicts old ones.
- **Problem:** Dropping an index waits for its file watchers while the pool lock is held. Other file requests wait behind it. The length of that wait was not measured.
- **Fix:** Move evicted entries into a list and drop it after unlocking.
- **If we skip it:** The picker can stall during an eviction.

### 18. **Every suggestion copies the whole ranking data** — `crates/smcli-core/src/suggestion_index.rs:124-146`

- **What this is:** Each suggest request takes a snapshot so ranking can run outside the main lock.
- **Problem:** The snapshot copies all commands and directories before filtering by prefix, plus all session followers, though a request needs one session's. Saves wait on the lock meanwhile. No stall was measured.
- **Fix:** Measure first. Then filter while copying, and copy only that session's last command and followers.
- **If we skip it:** Suggest requests do more work under the lock as history grows.

### 19. **Small dead code** — `crates/smcli-core/src/prune.rs:140-142`; `crates/smcli-core/src/history.rs:1143`

- **What this is:** Two bits of code that can never make a difference.
- **Problem:** Prune deletes picks that were already deleted in the same transaction. `version != DISCARDED_USAGE_VERSION` cannot matter because 4 is below `VERSION`.
- **Fix:** Delete both.
- **If we skip it:** Readers keep working through code that does nothing.

### 20. **Pass-through methods** — `crates/smcli-service/src/file_search.rs:274-282`

- **What this is:** Three small methods sit between the service and the file search code.
- **Problem:** `files_typed`, `grep_typed` and `complete_typed` only forward their arguments. No wrong result comes from this.
- **Fix:** Call `search` and `complete_query` directly from the service.
- **If we skip it:** Nine extra lines remain to read.

### 26. **Test suggestions miss common test syntax** — `crates/smcli-core/src/tests_discovery.rs:30-37,59-61`; `crates/smcli-service/src/test_candidates.rs:182`

- **What this is:** smcli finds test names so it can suggest commands that run them.
- **Problem:** `#[tokio::test]` followed by `async fn works()` yields no name, because the parser accepts only `fn `. JavaScript `test("works", ...)` is also missed, because both the search and the parser need single quotes. Found by reading.
- **Fix:** Accept `async fn`, and add double-quote forms in both places.
- **If we skip it:** Those tests are never suggested.

### 27. **"Defined in project" can credit the wrong name** — `crates/smcli-service/src/evidence.rs:271-279`

- **What this is:** Ranking lifts commands that name something defined in the project.
- **Problem:** A name counts as defined if it appears anywhere on a definition line. In `fn live_case() { println!("old_removed_case"); }`, `old_removed_case` is marked as defined, so a stale test command gets the lift. Found by reading.
- **Fix:** Count a definition only for the declared name. Other names on the line count as mentions.
- **If we skip it:** Stale commands rank slightly too high.

### 28. **A blocked search embeds your whole history** — `crates/smcli-search/src/index.rs:459-465,793-804`

- **What this is:** A natural-language search needs the index lock.
- **Problem:** If `prune` or another smcli holds that lock after 20 tries, the search embeds every matching command in memory, while holding the service's one-at-a-time search lock. At 100,000 commands that is a large batch for one query. The 2026-11-02 results recorded this as a trade-off with timing still to be checked, and it was never measured.
- **Fix:** Measure it first. If it is slow, use the keyword fallback that already exists for this case.
- **If we skip it:** Searches made during a long index operation can stall.

### 29. **The semantic search plan still says Draft** — `agents/plans/semantic-search-accuracy.md:4`

- **What this is:** The design plan for natural-language search.
- **Problem:** Its status is "Draft", though vector-first search shipped. The 2026-11-03 removal audit asked for this label to be corrected.
- **Fix:** Mark what shipped, and what the document no longer describes.
- **If we skip it:** Agents can treat finished work as still proposed.

## Older reports

All earlier reports in this folder were checked against the current code, then removed. Their text remains in Git history.

| Report                     | Items | Result                                                                                           |
| -------------------------- |------:| ------------------------------------------------------------------------------------------------ |
| Codebase review 2026-10-30 |    16 | All fixed.                                                                                       |
| Codebase review 2026-11-02 |    11 | All fixed. The search quality floor is now enforced by `rank_natural_language_eval`.             |
| Codebase review 2026-11-05 |    10 | All fixed. The Linux test item was fixed; further Linux work is on the roadmap.                  |
| Removal audit 2026-11-03   |     9 | All done, except the status label in finding 29. Linux acceptance is on the roadmap.             |
| Semantic search results    |     — | One unmeasured trade-off carried over as finding 28. The remaining items are accepted decisions. |

Accepted decisions and measurements worth keeping:

- **Config boost:** `INTENT_CONFIG_WEIGHT` ships at 0.48, not the 0.18 chosen under a no-loss rule. It accepts `run the tests` moving from rank 1 to 2 for a better overall score (private history benchmark: 8 of 13 in the top three, MRR 0.558, against 6 of 13 and 0.406). The benchmark was not rerun afterwards.
- **Known misses:** `start pi with sol` stays fourth, and is marked as an acknowledged miss in the ranking tests.
- **File grep cost (2026-10-30, 50,000 near-identical files):** rare-word grep took about 780 ms and used about 573% of one core. On varied files of the same count it took about 24 ms. Opening resumable results and choosing an interactive budget are on the roadmap.
- **File search memory:** fff's content cache is limited per index, not per process. Four cached indexes held 159.5 MiB of physical memory in that test.

**Verdict:** Fix 1 and 21 first because they can run code, then 2 and 3 for private history.

**Lean:** about -25 lines, -0 dependencies possible.

**Not checked:** Project tests were not run, and Linux is out of scope. Older findings were confirmed fixed by reading the current code, not by rerunning their original reproductions. The concurrent races in 22 and 23 and the full Tab flow in 21 were not reproduced. Performance findings are unmeasured. Codex reviewed the files this audit had skipped with focused reading, not line by line, and neither audit read the internals of `fff-search` or TalaDB. Dependency vulnerabilities were not checked because `cargo audit` is not installed. Clippy reported only style and complexity warnings, which are not listed here.
