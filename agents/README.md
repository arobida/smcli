# Agents documentation

Working notes, reviews and reference material for agents. Nothing here is built or tested; the code lives in
`crates/`.

## reference/

- `baseline.md` — benchmark method, historical Swift-era latency numbers and the pinned fff verification notes.
- `fff-licenses.md` — transitive license audit for the pinned fff, its Rust crates and bundled C code.

## plans/

- `roadmap.md` — open work that is not scheduled: Linux verification, fgrep, search, capture gaps, known limits and release items.
- `shipit-checklist.md` — licensing and distribution checklist that must pass before any public binary ships.
- `fgrep-notes.md` — fgrep behavior on large trees, measurements, and the remaining budget and debounce decisions.
