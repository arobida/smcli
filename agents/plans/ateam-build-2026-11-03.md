# ateam build plan: four OpenSpec changes (2026-11-03)

Build the four changes in `openspec/changes/` with the ateam. The orchestrator (Claude, via the kero skill) starts each agent, hands it the brief below, reviews the result and merges. Agents implement; they do not redesign. When a change's spec, design and tasks disagree with the code, the agent stops and reports instead of guessing.

| ID  | Change                          | Builder             | Reviewer                    | Wave | Size  |
| --- | ------------------------------- | ------------------- | --------------------------- | ---- | ----- |
| A   | `bash-pipestatus`               | piluna              | piterra                     | 1    | small |
| B   | `file-picker-middle-truncation` | piluna              | piterra                     | 1    | small |
| C   | `cheat-command-source`          | piluna              | piterra                     | 1    | mid   |
| D   | `project-kind-ranking`          | coder (Claude Code) | code-reviewer (Claude Code) | 2    | mid   |

Commit messages: pigrok45, following `agents/reference/commits.md`.

## Rules for every agent

- Read the change first: `openspec/changes/<change>/{proposal,design,tasks}.md` and `specs/**/spec.md`. The tasks file is the work list; tick each `- [ ]` to `- [x]` as it is done and verified.
- Follow `AGENTS.md`: build and test only through the Makefile, search code with `semble search "<query>" crates scripts`, edit markdown through iwe and run `make docs` after any markdown edit. Never edit `THIRD_PARTY_NOTICES.md` by hand (`make notices` only). Never rewrite native shell history files.
- Run the focused test slice named in the brief, once, when the work is done. Do not run `make test`, `make linux` or `make baseline` unless the brief says so; the orchestrator runs the full suite once per wave.
- Stay inside the files the brief lists. Touching anything else needs a note in the report saying why.
- Do not commit, push, archive the change, or change `openspec/specs/`.
- Finish with a report: tasks ticked, files changed, the exact test commands run and their result (pass/fail with failing output), and anything left open.

## Step 0: prerequisites (orchestrator, needs the user's go-ahead)

1. Commit the four change folders and the roadmap edit on `main`, so worktrees branched from it contain the change artifacts. Untracked files from earlier work (`concepts_brief.md`, `design_draft.md`, `mempalace.yaml`, the archived openspec moves) are left out unless the user says otherwise.
2. Create one worktree and branch per change, outside the repo:
   `git worktree add ../smcli-wt/<change> -b <change> main`
   Each worktree has its own `target/`, so parallel builds don't block on Cargo's lock or overwrite each other's `dist/`.
3. Start each agent in its worktree: `kero` session per agent, cwd `../smcli-wt/<change>`, using the command from the agents list in `AGENTS.md`.

## Wave 1: A, B, C in parallel

The three changes touch disjoint files (checked against the designs):

```
A  crates/smcli/src/shell_integration.rs (Bash template), scripts/test-shells.py, agents/plans/roadmap.md
B  crates/smcli/src/picker.rs
C  crates/smcli-search/src/{commands.rs, new cheat module, lib.rs}, crates/smcli-service/src/service.rs,
   crates/smcli/src/cli.rs, crates/smcli-core/src/command_source.rs (doc comment), crates/smcli-search/README.md,
   scripts/{gen-cheat.py, gen-notices.py, install.sh, uninstall.sh}, Makefile, Data/cheat/, THIRD_PARTY_NOTICES.md
```

### Brief A: `bash-pipestatus` (piluna)

Goal: Bash records `pipestatus` like Zsh and Fish.

- Work list: `openspec/changes/bash-pipestatus/tasks.md`, all groups.
- Where: the Bash template in `crates/smcli/src/shell_integration.rs`. `_smcli_prompt` starts with `local status=$? ...` (around line 196); add `PIPESTATUS` to that same `local` statement, never a separate one. `_smcli_record` (around line 125) passes the pipeline as the optional argument after the exit code, exactly as the Zsh `_smcli_record` does (around line 529: `... "$1" "$3" "$_smcli_started" ...`). bash-preexec mode (`precmd_functions`, around line 240): use `BP_PIPESTATUS` when set, otherwise pass nothing.
- Find and record the first bash-preexec release that sets `BP_PIPESTATUS` (its changelog or source); put it in the Bash header note (line 41 says "pipeline status is unavailable") and in your report.
- Tests: `scripts/test-shells.py` currently asserts Bash has no `pipestatus` (around line 316). Flip it and add the cases in task group 2.
- Verify: `make build`, then `make test-shells`.

### Brief B: `file-picker-middle-truncation` (piluna)

Goal: long paths in file and grep picker rows keep the file name, shortened in the middle with `…`.

- Work list: `openspec/changes/file-picker-middle-truncation/tasks.md`.
- Where: `crates/smcli/src/picker.rs`. File and grep rows are built in the `hits.iter()` branch of the list rendering (around line 622). `history_row` (around line 908) is the pattern to copy for measuring display width with `Span::width`. LS_COLORS style stays computed from the full `name`.
- Room is the row's inner width minus cursor, git mark and trailing provenance/status spans; in grep mode the path gets at most half of it.
- Display only: the inserted path must not change.
- Verify: `make build`, `make test-rust PKG=smcli`, `make test-picker`.

### Brief C: `cheat-command-source` (piluna)

Goal: cheat/cheatsheets as a second bundled `@@` source; sources tried in priority order per program (tldr first, cheat only when tldr has nothing).

- Work list: `openspec/changes/cheat-command-source/tasks.md`, in order. Group 1 first: priority order and the shared source list are what everything else builds on.
- Where:
  - `crates/smcli-search/src/commands.rs` `CommandIndex::gather` (around line 157) flat-maps every source; make it first-non-empty. Update its tests (`PlatformSource` stub around line 276).
  - `TldrSource::new` is built directly at `crates/smcli-service/src/service.rs` (around lines 409, 788, 832, 885; `command_index` at 437 takes `&TldrSource`) and `crates/smcli/src/cli.rs` (around lines 1096 and 1482). Replace all of them with one shared constructor in `smcli-search` that returns the bundled sources in priority order.
  - New cheat module beside `crates/smcli-search/src/tldr.rs`, same loading style.
  - `scripts/gen-cheat.py` modelled on `scripts/gen-tldr.py` (pinned clean checkout, never downloads, size cap). Get the data with `git clone https://github.com/cheat/cheatsheets` into a scratch directory outside the repo and pin the commit you used.
  - Packaging: `Makefile` (a `cheat` target; release copy beside line 23's tldr copy), `scripts/install.sh` (tldr handling around lines 62-87; cheat is optional, never an error), `scripts/uninstall.sh` (around line 172).
  - Notices: `scripts/gen-notices.py` tldr section (around line 128) is the pattern; run `make notices`.
- The cheat source must be optional: with `Data/cheat` missing, `@@` works with tldr alone.
- Verify: `make test-rust PKG=smcli-search`, `make test-rust PKG=smcli-service`, `make test-install`, `make tldr-ranking` (scores must not change). Report the pinned cheat commit and the example and program counts in `cheat.jsonl`.

### Wave 1 review and merge (orchestrator + piterra)

1. For each finished branch, piterra reviews `git diff main...<change>` against that change's spec and design: spec scenarios covered by a test, no files outside the brief, no hand edits to generated files. piterra reads the builder's test report and does not rerun those slices unless the diff changed after them.
2. Fixes go back to the original builder in the same session. If a re-review still says FIX, the next round goes to piluna instead of pikcode.
3. Merge in order A, B, C into `main` (A and B can't conflict with C; C last because it's largest). pigrok45 writes each commit message. Commit and push only with the user's go-ahead, per `agents/reference/commits.md`.
4. Once, on merged `main`: `make test`. If it fails, the orchestrator finds the branch responsible and sends it back.

## Wave 2: D `project-kind-ranking` (Claude Code agents)

Wave 2 uses the project's Claude Code agents (`.claude/agents/`), run by the orchestrator through its Agent tool: `coder` implements Brief D, `tester` runs `make ranking` and `make baseline` for the gate, `code-reviewer` reviews. D starts only after wave 1 is merged and `make test` passes on `main`, because its gate compares against that tree, and it shares `service.rs` with C. Branch the worktree from the merged `main`.

The baseline benchmark is timing-sensitive: while D runs `make baseline`, no other agent builds or tests on this machine. The orchestrator holds other work for that window.

### Brief D: `project-kind-ranking` (coder; tester runs the gate)

Goal: a same-kind project tier in suggest and search, shipped only if the benchmark proves it.

- Work list: `openspec/changes/project-kind-ranking/tasks.md`, strictly in order. Group 1 (save the before numbers) and group 2 (benchmark cases) come before any scoring code.
- Where:
  - Kind detection: `crates/smcli-core/src/context.rs` (`MANIFESTS`, `discover`, `Project.kind`). Do not change detection rules.
  - `crates/smcli-core/src/ranking.rs`: add `Signal::Kind` (`kind` in `Signal::parse`) and `project_kind` on `RankingContext` (`new` around line 81, beside `project_root`).
  - Three scoring sites that must stay in agreement (spec "same results with or without the service"):
    - in-process suggest, `crates/smcli-core/src/history.rs` around line 485 (×4 here, ×3 in project, ×2 ancestor)
    - service suggest, `crates/smcli-core/src/suggestion_index.rs` around line 233
    - search, `crates/smcli-search/src/corpus.rs` (`runs_project` around line 96; boosts around lines 586 and 665)
  - The service caches directory kinds where it builds its suggestion index (`crates/smcli-service/src/service.rs` around line 325).
  - Benchmark cases: `crates/smcli-search/tests/ranking.rs`, new `kind` group in the Adversarial cohort, fixture projects under the existing temporary root.
- Careful: the roadmap notes that the `corpus.rs` "used here"/"used in this project" weights pass `make ranking` only in a narrow window. Add the new boost beside them; don't retune them.
- The gate (task group 5), copied from the spec:
  - `make ranking` with the tier on and with `SMCLI_RANKING_OFF=kind`: the `kind` group's MRR must be higher with the tier on, and every cohort must stay at or above its floor.
  - `make baseline` compared with the group 1 numbers: warm suggest and hybrid search medians within run-to-run noise. Record the first-request difference in `agents/reference/baseline.md`.
  - If either fails after tuning within the spec's tier order, stop. Report both sets of numbers and do not hand the branch over for merge.
- Verify: `make test-rust PKG=smcli-core`, `make test-rust PKG=smcli-search`, the gate above, then `make test` once.

### Wave 2 review and merge

`code-reviewer` reviews the diff and the gate numbers (MRR, top-1 and top-3 per cohort, before and after; baseline medians). Merge only if the gate passed. pigrok45 writes the commit message; commit with the user's go-ahead.

## After all four

- `openspec archive <change>` for each merged change (orchestrator, after the user reviews), then `make docs`.
- `make fresh`, then hand the user the test pane:
  `kero split --right -- eval "$("/Users/arobida/Dev/Apps/smcli/dist/smcli" shell-init zsh)"`
  Things to try: `false | true` in Bash (via `smcli history` or `why`), a long path in the file picker, `@@` on a program only cheat covers, suggest in a fresh crate.
- Remove the worktrees: `git worktree remove ../smcli-wt/<change>`.

## Stop and ask the user when

- A spec scenario can't be met as written.
- Kind ranking fails its gate.
- The cheat/cheatsheets license or format differs from the design (CC0; `# description` followed by command lines).
- A merge conflicts outside the files listed in the briefs.
