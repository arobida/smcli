# Removal audit — 2026-11-03

What this repo does: smcli captures shell history, ranks commands and searches files through a local service. Assumed load: one user with several shells and up to 100,000 history records. This pass audits removal candidates, not runtime correctness.

Baseline: main `2b14ebf`, including the existing uncommitted command-completion work. No source, worktree, branch or existing document was deleted. REMOVE means a candidate for a later cleanup, not an action performed here.

## Cleanup applied — 2026-11-03

User authorized removal of findings 1–6 and 9, and archival of 7–8. The findings below remain as historical evidence; their REMOVE/HOLD labels describe the audit state before this cleanup.

- **1 done:** removed the clean, merged semantic-accuracy worktree through Git. Local and remote branches were left intact.
- **2 done:** deleted `.delta` in full. At the user's explicit follow-up request, also deleted the recovery archive created during cleanup; no recovery copy was retained by this session.
- **3 done:** removed the service's unused direct TalaDB declaration and refreshed Cargo.lock. TalaDB remains a transitive search dependency.
- **4 done:** deleted `design_draft.md` and removed stale draft-only instructions from the semantic-search design.
- **5 done:** deleted `fgrep-notes.md`, retained the remaining resume/cancellation/budget decisions in roadmap, and repaired current index/plan references. Dated audit evidence remains historical.
- **6 done:** deleted both completed path-edit plans. Implementation and tests remain intact.
- **7 archived:** `openspec/changes/archive/2026-11-03-fff-scoped-navigation`; canonical `file-picker-scope` specification synced, including the resolved cwd/directory protocol mapping.
- **8 archived:** `openspec/changes/archive/2026-11-03-command-completion-index`; canonical `command-completion-index` specification synced.
- **9 done:** removed obsolete cache instructions from the search README and replaced the original help implementation plan with current UI/placeholder behavior and links to the command-index contract. Legacy-cache pruning and its test remain.

Cleanup does not remove unrelated untracked `concepts_brief.md` or `mempalace.yaml`. No application behavior was intentionally changed. Validation passed: make build, make docs, git diff --check, all seven OpenSpec specifications under strict validation, and assertions that both archives retain their full requirement blocks. The full test suite was not rerun for this dependency/documentation cleanup.

## Should fix

### 1. REMOVE: the merged semantic-accuracy worktree

- **What this is:** `.kero/worktrees/semantic-accuracy` is a second checkout at `5a410b9`.
- **Problem:** It is clean, has no untracked files, and `git rev-list --count main..semantic-accuracy` returns zero. It occupies 5.3 GB, including about 4.9 GB of build output, for work already in main.
- **Fix:** Retire the checkout with Git worktree removal after checking no live session uses it. The branch can be retired separately; do not delete remote branches as part of disk cleanup.
- **If we skip it:** Several GB remain tied up in a completed checkout and searches can see stale source.

### 2. REMOVE caches; HOLD dirty Delta snapshots

- **What this is:** `.delta/worktrees` contains 22 old Swift checkouts with separate Git metadata under `.delta/clones`. Together `.delta` occupies 1.6 GB.
- **Problem:** Nineteen snapshots have modified or untracked files. The clean `xcddpe9rzy5j` snapshot is at `b28ef4c`, which is not an ancestor of main and has no containing local branch. Deleting all of `.delta` would discard uncommitted work and could remove a useful commit reference.
- **Fix:** Mark the five `.build` directories below for removal as regenerable caches. Mark clean snapshots `egnca7af3y8a` and `kq2ta6ywv7gd` for retirement: their HEADs are ancestors of main, and all commits reachable from their refs are reachable from this repository's refs. Preserve/export the other snapshots' changes and the `b28ef4c` reference before retiring them. Check active sessions before removal.
- **If we skip it:** Old Swift copies consume disk and contaminate root-wide code searches. A blind cleanup loses work.

Cache paths, relative to `.delta/worktrees/`:

- `6xby6et0c8qj/smcli/.build` — 384 MB.
- `88v0hjd59d7r/smcli/.build` — 266 MB.
- `n9bver0n8ww8/smcli/.build` — 272 MB.
- `shwbqhzjmbth/smcli/.build` — 385 MB.
- `w7te7sada3e0/smcli/.build` — 261 MB.

All other Delta snapshot IDs are HOLD: `093c6j7g09j4`, `0dpmhv32pdam`, `52s20khptjm3`, `6xby6et0c8qj`, `83w3drm89s2j`, `88v0hjd59d7r`, `h4ce3rf9ev78`, `k7pqny9vhbmw`, `n9bver0n8ww8`, `ncm35mpg3w4d`, `nedrh8ny8hgw`, `qhmmd7sz5mt1`, `shwbqhzjmbth`, `vdac2epv7xrj`, `w0xg5z6zjx7q`, `w1p1jh6hp7e5`, `w7te7sada3e0`, `xcddpe9rzy5j`, `xdy26sbac876`, `zg5qfmmm1mzv`.

### 3. REMOVE: one unused direct dependency

- **What this is:** `crates/smcli-service/Cargo.toml:13` directly declares `taladb`.
- **Problem:** The service has no TalaDB API use. Its only source mention is a comment; it uses `smcli_search::HistorySearch` instead. The CLI does call TalaDB directly, so its dependency stays.
- **Fix:** Remove the service manifest entry, refresh the service's lockfile dependency list, and remove the direct-dependency claim in its README. Run the normal build afterward.
- **If we skip it:** The manifest advertises a dependency the service does not directly use. This saves no transitive package or measured build time: smcli-search still needs TalaDB.

### 4. REMOVE: the duplicate root design draft

- **What this is:** Untracked `design_draft.md` duplicates `docs/designs/natural-language-command-help.md`.
- **Problem:** A full diff shows just one changed line: Draft versus Proposed. Keeping 371 duplicated lines makes later edits diverge. A separate search-design document also contains a stale reference to `design_draft.md`.
- **Fix:** Keep the tracked design and retire the root duplicate. Repair the stale drafting instructions in `agents/plans/semantic-search-accuracy.md:357` before removal. Keep `concepts_brief.md` pending review: it is different content and currently untracked work.
- **If we skip it:** Agents can edit the wrong copy or interpret obsolete drafting instructions as current scope.

### 5. RETIRE after carrying forward open items: stale fgrep instructions

- **What this is:** `agents/plans/fgrep-notes.md:10` onward describes an older implementation as current behavior.
- **Problem:** It says zero-match budgets are unenforced and typing queues scans. Current `file_search.rs:412` enables `enforce_time_budget`; `picker.rs:160` coalesces queued requests. The October 30 review already records the obsolete claims, while agents/README still advertises this file as current guidance.
- **Fix:** Carry forward the still-open resume and measured interactive-budget decisions into roadmap, then retire the old note or explicitly archive it. Update the index and references in roadmap and the old fixes plan together. Preserve historical measurements in the dated review/reference material.
- **If we skip it:** Someone can waste time implementing fixes already present or cite obsolete measurements as current performance.

### 6. RETIRE: completed path-edit implementation plans

- **What this is:** `agents/plans/edit-path-args.md` and `edit-path-parsing.md` are 137 lines of implementation instructions.
- **Problem:** The code and regression cases already implement both generations. The first plan says command words are never paths; the second changes that rule. Current `parse.rs:451` accepts the shell and supports command substitutions, with unit cases at lines 337 onward and a shell scenario in `scripts/test-file-picker.py:187`.
- **Fix:** Retire both plans after preserving any useful behavior contract in current user documentation. Keep the source and tests. No incoming filename references were found in the primary tree.
- **If we skip it:** Two plans describe different rules for a shipped feature and invite redundant work.

### 7. ARCHIVE: completed scoped-navigation change

- **What this is:** `openspec/changes/fff-scoped-navigation/tasks.md` has every task checked, but remains an active change.
- **Problem:** The change still carries a scope specification and an unresolved-looking QUESTIONS file. `openspec/specs` has no canonical file-picker-scope spec, so deleting this directory would lose its only full specification.
- **Fix:** Sync its spec and resolved cwd clarification into the canonical specs, then archive the change using the existing OpenSpec workflow. Mark for active-directory removal only after that sync.
- **If we skip it:** Completed work looks pending; deleting it directly discards useful requirements.

## Recheck after command-completion changes

Rechecked against main `468e56b` on 2026-11-03, including implementation commit `725eef4`. Findings 1–7 remain applicable. The previously uncommitted command-completion sources/specs are now tracked implementation; they are not removal candidates. Only `concepts_brief.md`, `design_draft.md` and `mempalace.yaml` remain untracked at the start of this recheck.

The registered semantic-accuracy worktree is still clean and has zero commits outside main. All 22 Delta snapshots retain the same HEAD and clean/dirty classification: 19 dirty, two clean ancestor snapshots, one clean non-ancestor snapshot. Disk totals remain approximately 1.6 GB for Delta and 5.3 GB for Kero. Source-name and dependency checks again found no confirmed dead Rust function and only the previously identified unused direct service TalaDB declaration.

### 8. ARCHIVE: completed command-completion change

- **What this is:** `openspec/changes/command-completion-index/tasks.md` now has every task checked, and commit `725eef4` contains the implementation.
- **Problem:** The completed change remains in the active directory. Its only command-completion specification is under that change; there is no canonical `openspec/specs/command-completion-index/spec.md`.
- **Fix:** Sync the specification into the canonical specs and archive the change through the existing workflow. Keep `command_source.rs`, `commands.rs`, their callers and tests: they are used.
- **If we skip it:** Completed work looks pending. Deleting the change before syncing would discard the program-selection and index contracts.

### 9. RETIRE obsolete cache instructions; update the search README

- **What this is:** `crates/smcli-search/README.md:10` still advertises `corpus_vectors`; line 17 describes the old `tldr-*.vec` format. The original natural-language-help design repeats those instructions at lines 125 and 251–256.
- **Problem:** Commit `725eef4` deletes `corpus_vectors`, the cache reader/writer and the associated cache test. Runtime help now reads `commands::CommandIndex`; old `.vec` files are only legacy prune targets. Following the README names an API that no longer exists.
- **Fix:** Remove the obsolete cache paragraphs/API entry from the README and describe the current command index. Mark the original help design as implemented and its cache/program-set sections as superseded by the command-completion specification; archive it only after preserving the remaining UI and placeholder requirements. Keep legacy `.vec` pruning and its test because existing installations can still contain those files.
- **If we skip it:** Current documentation tells contributors to maintain or rebuild a deleted storage path and gives an outdated description of which programs bare `@@` searches.

Recheck verification: reviewed the implementation commit and new source/index call paths, checked literal references across the primary tree including tests/config/docs, and reran Git worktree/status and symbol/dependency scans. No build or test suite was run for this report-only update. The repository-wide `git diff --check` now passes; the earlier tldr EOF whitespace warning has been fixed.

## Keep / not proven removable

- Rust code: scanned function names across all crate sources and checked single-occurrence candidates. No confirmed dead function. Trait methods are invoked through their traits; comparison helpers in ranking.rs are called by tests in lib.rs. `FileWatcher`'s apparently unread field owns a live watcher and must stay.
- All protocol-version fixtures: explicitly consumed by service_protocol tests. `tests/reference.json` is consumed by potion.rs.
- Models, tldr data, their license files, THIRD_PARTY_NOTICES and the licensing checklist: runtime/distribution inputs, not dead documentation.
- Dated codebase reviews, semantic-search results and evidence scripts: historical evidence, not redundant current instructions. Findings marked fixed do not erase the evidence.
- `build-test-efficiency.md`: many tasks have landed, but Linux acceptance was not verified here; do not delete the remaining acceptance obligations.
- `semantic-search-accuracy.md` and its brief: implementation/results differ from the original staged proposal; not enough evidence to discard the remaining scope. Label status and repair stale references before deciding what to archive.
- `concepts_brief.md` and `mempalace.yaml`: uncommitted work; no deletion justified by this audit. Command-completion sources are now committed and used; the completed change documentation is covered by finding 8.
- `.claude` OpenSpec skills/commands and `scripts/ateam.sh`: developer-invoked entry points; absence from application callers does not make them dead.
- Crate/source READMEs and the drift report: keep until their contracts/corrections are reconciled; age alone is not proof of redundancy.

## Evidence and limits

Inspected tracked-file inventory, workspace/dependency manifests, build/test wiring, CLI/service entry points, module references, source function-name occurrences, fixtures, document graph and candidate bodies. Literal reference searches included hidden primary-tree config, tests and docs, excluding Git internals, build output, models/data and stale copied worktrees. Inspected Git status and HEAD ancestry for all 22 Delta snapshots and the registered Kero worktree, plus all-ref reachability for the two clean ancestor snapshots. Disk figures are du allocated sizes, rounded, and can change.

Verdict: retire the merged worktree first; preserve dirty snapshots before touching Delta. No confirmed dead Rust function justifies source deletion.

Lean: -508 document lines possible from the duplicate draft and two completed path plans, plus further retirement after preserving open notes; -1 direct dependency declaration, -0 transitive dependencies. About 6.4 GiB of build caches alone are reclaimable.

Not checked: full builds/tests, Linux, live process/session ownership, all-ref/reflog recovery for dirty snapshots, external consumers of public Rust APIs, and every source line. No runtime changes were made. Deleting dirty snapshots without preservation remains the main cleanup risk.
