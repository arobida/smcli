# PATH availability

## Decision

**GO**, with constraints: a weak (+0.1), positive-only boost when a command's program resolves to an executable in the live shell's PATH snapshot, taken at query time from the CLI's inherited environment and sent per request as a new `shellPath` protocol field. Unknowns, builtins, shebang wrappers and absolute-path programs stay neutral. Wire it in task 7.1 behind `SMCLI_RANKING_OFF=path`, and keep the boost weaker than every usage signal.

## Method

Prototype in this worktree: `Signal::Path` in `smcli-core/src/ranking.rs` (parsed from `SMCLI_RANKING_OFF`), the availability check in `smcli-search/src/path.rs`, a `shell_path` snapshot on `SearchSignals`, and a bounded boost in `rerank` (`corpus.rs`) that resolves each distinct program once per query and adds `why` text `program found in PATH`. The check is `stat`-based: regular file, executable, not a shebang script, first entry wins across the snapshot's absolute directories. Builtins come from a fixed list; functions are indistinguishable from unknown programs and are neutral by construction; an absolute-path first word is never boosted. The fixture is synthetic: a temp `bin/` snapshot with a real executable (`txbuild`), a shebang wrapper (`txxe`) and an executable named `echo` to prove builtins ignore shadowing.

## Fixture cohort (make ranking)

Each pair runs the signal on and off in one process; the expected winner is the twin the signal should (or must not) prefer:

| Case                                                           | On rank | Off rank | Verdict                                               |
| -------------------------------------------------------------- | ------- | -------- | ----------------------------------------------------- |
| present (`txbuild`, in snapshot, 4 runs, vs `uxbuild`, 6 runs) | 1       | 1        | hold; embedding favors `txbuild` already              |
| wrapper (`txxe` shebang script)                                | 2       | 2        | neutral, no flip                                      |
| builtin (`echo`, file shadows it in snapshot)                  | 2       | 2        | neutral, no flip                                      |
| absolute path (`/bin/txbuild ...`)                             | 1       | 1        | neutral                                               |
| unknown vs unknown                                             | 1       | 1        | neutral                                               |
| present, index held (text fallback, equal coverage)            | 1       | 2        | deterministic flip: usage decides off, boost flips on |

The rerank unit test (`corpus::tests::path_availability_flips_only_near_ties_and_stays_positive`) asserts the flip on equal relevance: 4-run `txbuild` beats 6-run `uxbuild` with the signal on, loses with it off, and a relevance gap (0.5 vs 1.0) is never crossed. Neutral cases get no boost.

## Ablation deltas

- The 34-case fixed set is untouched: with the synthetic snapshot, its programs are not in it, so `make ranking` totals stay search MRR 1.000 / top-1 1.00 (n=16) and suggest 1.000 (n=18). Floors hold with the signal on.
- The only measurable effect is the deterministic fallback flip above: rank 1 with the signal, rank 2 without. On the indexed path, a 1-2% embedding difference between near-identical twins outweighs a 4-vs-6-run usage gap, so the boost (like usage) only decides true ties.

## Cost

- Snapshot size: synthetic 519 B (8 dirs); this machine's real PATH 3,069 B (mise-heavy, 63 dirs). Sending it as a `shellPath` field costs 533 B of JSON per request, against the protocol's 1 MiB cap.
- Latency, release build, rerank over 120 hits with 12 distinct programs, 200 samples: p50 432 us, p95 960 us with the signal on vs p50 100 us, p95 102 us off. The marginal cost is the per-distinct-program resolution, memoized once per query; the real pool (<= max(4 x limit, 40)) is smaller.
- Per request vs per session: the CLI runs as the live shell's child, so its inherited environment already holds the shell's PATH; no hook change or per-session service state is needed. A per-session snapshot would save ~0.5-3 KB per search request but adds staleness (mise, direnv, venv activations change PATH mid-session), eviction and service state; not worth it.

## Privacy review

The snapshot never enters the index, the database, logs or embeddings; it is request-scoped. `why` shows fixed text only, so no path leaks into the UI. PATH entries can reveal tool usage and directory names; cap the field (e.g. 8 KiB) when wiring it in task 7.

## Risks

- A stale or wrong snapshot can only lift a command; it can never demote. The boost (+0.1) is below every usage signal, so a wrong lift loses at most a tie.
- Compiled wrapper shims are indistinguishable from real binaries and count as found; harmless, since the program genuinely resolves.

## Skipped

- Suggest-side boost, hook changes, protocol wiring and fixtures (task 7.1), and real-history evaluation: the synthetic corpus cannot show the real win (tool installed vs removed), only the tie behavior and the safety guarantees.
