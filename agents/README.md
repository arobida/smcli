# Agents documentation

Working notes, reviews and reference material for agents. Nothing here is built or tested; the code lives in
`crates/`.

## reference/

- `baseline.md` — benchmark method, historical Swift-era latency numbers and the pinned fff verification notes.
- `fff-licenses.md` — transitive license audit for the pinned fff, its Rust crates and bundled C code.
- `commits.md` — stage, caveman-commit messages, and push-to-remote runbook for agents.

## plans/

- `roadmap.md` — open work that is not scheduled: Linux verification, fgrep, search, capture gaps, known limits and release items.
- `shipit-checklist.md` — licensing and distribution checklist that must pass before any public binary ships.
- `codebase-review-fixes.md` — wave plan and agent assignments for fixing the 2026-10-30 review findings.
- `semantic-search-accuracy.md` — design for natural-language Tab search accuracy: query correction, coverage gate, head-command boost, evidence/relevance scoring; brief in `semantic-search-accuracy-brief.md`.
- `architecture-remediation.md` — full plan for the 2026-11-04 architecture audit (H1–H3, M4–M8, L9–L12): service-only model ownership, content-hash model identity, storage ownership, caller context and scrubbed service env, typed requests, housekeeping and the `cli.rs` split.

## reviews/

[Codebase review — 2026-11-05](reviews/codebase-review-2026-11-05.md)

[Current codebase review after semantic-accuracy merge (2026-11-02)](reviews/codebase-review-2026-11-02.md)

[Codebase review and fff investigation (2026-10-30)](reviews/codebase-review-2026-10-30.md)

[Removal audit — dead code, worktrees and documentation (2026-11-03)](reviews/removal-audit-2026-11-03.md)
