---
title: Codebase review — 2026-11-05
---

# Codebase review — 2026-11-05

**What this repo does:** smcli captures shell history, searches commands locally, and provides file and command pickers. The assumed load is one person, several concurrent terminals, one background service per database, and roughly 100,000 history records.

The review covers commit `37a7f7b`. It is read-only: no application code was changed.

Fix plan: [Review fixes — 2026-11-05](../plans/review-fixes-2026-11-05.md).

## Must fix

### 1. **Uninstall can recursively delete a misconfigured directory** — `scripts/uninstall.sh:59-75,141-144`

- **What this is:** Uninstall exports history, then deletes its database.
- **Problem:** If `SMCLI_DB` names a directory, export silently skips it, but `rm -rf` deletes it. This was confirmed with deletion intercepted.
- **Fix:** Reject directory paths before changing anything. Use `rm -f` for database files; recursive deletion belongs only on validated cache directories.
- **If we skip it:** One wrong environment value can destroy unrelated files without a backup.

### 2. **File-picker scope changes bypass directory filters** — `crates/smcli/src/cli/pick.rs:234-237,258-265`; `crates/smcli-service/src/service.rs:137-145`

- **What this is:** File selections are saved to improve later results.
- **Problem:** From an excluded `/repo/private` directory, switching search scope to `/repo` makes filtering check `/repo`, not the caller's directory.
- **Fix:** Carry the caller's directory separately from search scope. Apply `cwdFilter` to that directory. Add a scope-switch regression test.
- **If we skip it:** File paths and queries from excluded directories can enter the database.

### 3. **Relative config paths resolve in the wrong terminal's directory** — `crates/smcli-core/src/config.rs:68-70`; `crates/smcli-core/src/caller.rs:83-85`

- **What this is:** Clients tell the shared service which config to read.
- **Problem:** `SMCLI_CONFIG=config.json` stays relative. A service started in directory A reads A's file when a client in directory B expects B's file.
- **Fix:** Resolve relative config paths against the client's working directory before sending them. Test two clients sharing one database.
- **If we skip it:** Search uses the wrong model or settings. Service-owned selection writes can use the wrong privacy filters.

### 4. **Normal captures never send PATH to background command discovery** — `crates/smcli-core/src/caller.rs:28-32`; `crates/smcli/src/cli/history.rs:62-75`

- **What this is:** Capturing a newly used program should add its command examples.
- **Problem:** The handler expects `shell_path`, but neither the CLI record builder nor the caller-field table supplies it. Existing tests manually supply the missing field.
- **Fix:** Include `Path` among Record's caller fields. Test discovery through the real CLI.
- **If we skip it:** Newly used programs miss automatic indexing. Repeated captures can keep triggering fruitless background scans.

### 5. **Reindex misreads alias keys as program names** — `crates/smcli/src/cli/tldr.rs:105-108`

- **What this is:** Reindex discovers programs previously used in captured history.
- **Problem:** Alias keys contain NUL-prefixed JSON. For alias `j` expanding to `jq . file.json`, splitting the key produces `\0["j","jq`, not `jq`.
- **Fix:** Decode with `SearchEntry::from_key`, then use its existing `program()` method. Add an alias-only history test.
- **If we skip it:** Reindex misses non-system programs used only through aliases. Bare `@@` lacks their examples.

### 6. **Bash binds one Up sequence even with keys disabled** — `crates/smcli/src/shell_integration.rs:367-369`

- **What this is:** Saved settings control picker shortcuts.
- **Problem:** `\eOA` is bound unconditionally. This was confirmed from the generated Bash template with every key setting disabled.
- **Fix:** Guard this binding with `keyUp`, matching `\e[A`. Test both sequences with `--keys none`.
- **If we skip it:** Terminals sending this sequence still open smcli instead of normal history.

## Should fix

### 7. **Metadata updates repeatedly scan the whole search index** — `crates/smcli-search/src/index.rs:499-502,534-538`

- **What this is:** Search updates usage metadata without rebuilding command embeddings.
- **Problem:** Updates filter on `command`, but that field has no index. TalaDB's implementation scans all documents. Updating thousands of existing commands after an import repeats that scan thousands of times.
- **Fix:** Add `command` to the existing indexed fields. Add a bulk-update performance check.
- **If we skip it:** Large history updates can make the next search disproportionately slow and hold up other searches.

### 8. **Fallback grep reads files without a size limit** — `crates/smcli/src/cli/pick.rs:297-325`

- **What this is:** The picker searches files locally when the service is unavailable.
- **Problem:** `read_to_string` loads each whole file. A multi-gigabyte generated log can consume equivalent memory. The service path already limits files to 10 MiB.
- **Fix:** Apply the same limit before reading, with a bounded read for files that grow. Test an oversized file.
- **If we skip it:** A normal project containing large logs can exhaust picker memory.

### 9. **The Linux test target always reaches a macOS-only installer test** — `Makefile:92-94`; `scripts/test.sh:28`; `scripts/install.sh:37`

- **What this is:** `make linux` runs the full gate inside Linux.
- **Problem:** The gate unconditionally runs installation tests. Their first installation exits with "the installer is macOS-only". The tests also use BSD-specific `stat` flags.
- **Fix:** Skip the macOS installation suite explicitly on Linux. Keep Linux build, shell, service and picker checks running.
- **If we skip it:** The advertised Linux gate cannot pass, hiding meaningful Linux regressions behind a known failure.

## Nice to have

### 10. **Delete the unused bounded-history reader** — `crates/smcli-core/src/history.rs:344-357,1136-1156`

- **What this is:** `picker_recents` reads a bounded list of recent runs.
- **Problem:** Whole-tree reference checks found only its own test and documentation. Production pickers use `recent_distinct` or the service index's `recent`.
- **Fix:** Delete this method and its test. Remove its name from the interface documentation.
- **If we skip it:** An unused database path remains part of the maintenance surface.

## Verification

`make test` passed on macOS: **338 Rust tests passed, 9 skipped**, followed by all black-box suites: CLI, shell integration, service, file picker, alias picker, cheat-data parsing, and installation/uninstallation.

Targeted checks also confirmed:

- Uninstall attempts recursive deletion of a directory supplied as `SMCLI_DB`, without exporting it. A mock `rm` intercepted deletion; no real user data was removed.
- The Bash template still binds `\eOA` when all saved key settings are disabled.
- The normal CLI capture path does not populate `shell_path`.
- Alias-key splitting produces an encoded fragment instead of the expanded program name.
- Simulating Linux makes the installer refuse execution, while the full test gate invokes its suite unconditionally.
- TalaDB 0.12.0 plans unindexed equality filters as full scans; metadata updates use an unindexed `command` filter.
- Whole-tree references to `picker_recents` contain no production callers.

Passing tests do not cover the reported failure cases. Performance implications were traced in code, not measured with a new bulk-update benchmark.

**Verdict:** Fix uninstall safety and privacy-filter routing first. No application code changed.

**Lean: -35 lines, -0 dependencies possible.**

**Not checked:** Full parser, ranking and UI implementations; generators, benchmarks, model/data contents, dependency advisories, Linux runtime, and the nine skipped Rust tests. Third-party implementation review was limited to the TalaDB query-planning and update paths relevant to finding 7.
