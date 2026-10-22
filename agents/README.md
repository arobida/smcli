# Agents documentation

Working notes, reviews and reference material for agents. Nothing here is built or tested; the code lives in
`crates/`.

## reference/

- `baseline.md` — benchmark method, historical Swift-era latency numbers and the pinned fff verification notes.
- `fff-licenses.md` — transitive license audit for the pinned fff, its Rust crates and bundled C code.

## reviews/

- `caveman-review.md` — consolidated code and release-readiness review, including the 2026-10-22 final audit.

## reports/

- `2026-10-20-backend.md` — after-action report for the Swift backend plan: service, shell adapters, context, fff and ranking.
- `2026-10-22-audit-fixes.md` — after-action report for the final audit fixes across the Rust crates and install scripts.

## plans/

- `roadmap.md` — open work that is not scheduled: Linux verification, fgrep, search, capture gaps and release items.
- `shipit-checklist.md` — licensing and distribution checklist that must pass before any public binary ships.
- `fgrep-notes.md` — fgrep behavior on large trees, measurements, and the remaining budget and debounce decisions.
