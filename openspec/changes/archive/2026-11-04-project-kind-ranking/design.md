# Design

## Context

Project context comes from `context.rs` `discover`, which stats manifests upward from a directory and reports projects with a primary `kind` (the first manifest in `MANIFESTS` order, languages before task runners). Ranking only uses `project_root` (`RankingContext::in_project`). Usage weights live in three places that must agree (spec: same results with or without the service):

- in-process suggest: `history.rs` (×4 here, ×3 in project, ×2 ancestor/descendant)
- service suggest: `suggestion_index.rs` (same tiers)
- search: `corpus.rs` (`runs_here`, `runs_project` counts feeding boosts; the "used here"/"used in this project" weights pass `make ranking` only in a narrow window, see roadmap)

Runs store only `directory`, not a kind.

## Goals / Non-Goals

**Goals:**

- One kind lookup used by all three scoring sites.
- Measurable on and off (`kind` switch) with a pass/fail gate.

**Non-Goals:**

- Storing kind per run (no schema change). Revisit only if the benchmark shows deleted directories matter.
- Matching on secondary kinds (a Rust crate with a Makefile is `rust`, not `make`).

## Decisions

**Kind of a past run directory: a cheap manifest probe, cached per distinct directory.** Kind is the primary kind of the nearest project at or above the run directory, found by walking up and statting only the `MANIFESTS` names (equal to `discover`'s first project kind, without reading Git or parsing manifests, starting from the directory's real path as `discover` does). The service keeps the kinds in a cache whose entries expire after 60 seconds; a lookup probes only directories holding a command that matched the prefix, an expired or missing entry is probed again, and a deleted directory probes as unknown. The in-process path memoizes per call. All kind work is skipped when the cwd has no kind or `kind` is switched off. Alternative: a `kind` column at record time (schema v9) survives deletions but needs a migration and a backfill that can only use `discover` anyway, so it adds code before it is known to matter.

**Cost bound.** Distinct run directories at 100k runs are expected to be in the low thousands; `discover` only stats and reads small files. The baseline gate (spec) is the check; if first-request cost is too high, bound the lookup to the top-N directories by run count and treat the rest as unknown (no boost), which the spec already allows.

**Tier weight.** Suggest: ×1.5 for a same-kind run, between in-project (×3) and everywhere (×1), not combined with the in-project or here multipliers (the strongest tier wins, as today). Search: add `runs_kind` beside `runs_project` with a boost smaller than the project boost. Exact values are tuned with `make ranking`; the order of tiers is fixed by the spec.

**Kind of the query.** From the cwd's `ProjectContext.projects.first().kind`, carried on `RankingContext` as `project_kind`, so the service and in-process paths read it the same way.

**Benchmark cases.** Add a `kind` group to `ranking.rs`: fixture projects of two kinds (e.g. a Rust crate and a Node package under the temporary root) with a fresh crate of each kind, a habit run only in the sibling of the same kind, and an equally frequent distractor run in a project of the other kind. Plus one own-project-wins case and one outside-project case. Cases go in the Adversarial cohort so the original floors are untouched.

## Risks / Trade-offs

- [Narrow-window weights in `corpus.rs` shift] → the new boost is added beside, not inside, the existing project boost; rerun `make ranking` and keep every floor.
- [Directories under a monorepo root report the root's kind] → acceptable; the same rule already defines "in this project".
- [Kinds go stale in the service] → entries expire after 60 seconds, so through the service suggest sees a kind change within 60 seconds and search by the next corpus rebuild (at most 5 minutes, sooner on a change of directory), while in-process ranking stops at once; the search corpus counts `runs_kind` when it is built (rebuilt at most every 5 minutes). A stale kind can leave a boost on a deleted directory for up to 60 seconds in suggest and until the next corpus rebuild in search, never more.
- [Index build slows on very large histories] → measured by the baseline gate; fallback is the top-N bound above.
