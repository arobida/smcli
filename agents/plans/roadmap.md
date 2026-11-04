# Roadmap

Open work, not scheduled. macOS and Linux are supported today; search is included on both.

## Verification

- **Linux:** run `make linux` (Docker) to exercise the full suite on Linux. The historical Fish prompt
  timeout in `scripts/test-shells.py` is still unverified; record the container Fish version and the prompt
  transcript if it recurs. The musl descriptor close (`close_range`, `/proc/self/fd` fallback in
  `service_client.rs`) is compile-checked only; run it on a musl system with the Linux release.

## Search

- **fgrep on large trees:** partial results are reported, the budget is enforced on zero-match searches,
  and stale queries are coalesced. Still open: a running obsolete query finishes before the next, resume from
  `next_file_offset`, and choose an interactive budget by measurement. Historical measurements and corrections remain in the October 30 review.
- **Incremental corpus for writes from another process:** a fallback record, import or prune makes the
  next search rebuild the corpus (about 760 ms at 100k runs). Today only the service's own records update
  it in place.
- **Optional, only if profiling asks for it:** a persistent Zsh transport and async suggestions.
- **Command-index bridge follow-ups** (from the review of `f06152c`):
  - `CommandIndex::description_matches` opens the command-index TalaDB file on every service search; cache the handle beside the cached `CommandIndex` (needs a lifetime and locking design).
  - The bridge's search-time cost with the service on is unmeasured; `make baseline` ran service-off only.
  - Flagless templates (tldr `tar xvf`) score 0 on flags, so affinity can't separate `tar -xzf` from `tar -czf`.
  - `bridge_template` keeps the long option (`--patch`), so history using the short form (`-p`) gets no flag credit.
  - `rank_natural_language_eval` in `tests/ranking.rs` hits `unreachable!()` when a new NL cohort is added; report it instead.

## Capture

- **Fish:** start times are whole seconds (Fish has no clock builtin).

## Known limits (accepted, documented in code)

- **Zsh:** `HISTORY_IGNORE` is honored; a user's `zshaddhistory` function is not, because mirroring it
  would mean running user code from the hook.
- **Bash:** with ignoredups in PS0/bash-preexec modes, an ambiguous repeat is dropped rather than risk
  capturing an excluded command.
- **Swift manifests:** only literal `name: "..."` on target-like constructors is read (`context.rs`);
  `name: "A" + "B"` matches `A`. Widen only with a structured reader.
- **Index identity:** documents are keyed by the full command since the October 2026 fix; an older index
  can show commands over 1,500 characters as stale in `smcli health` until the next search rewrites them.
- **Ranking weights:** `corpus.rs` "used here"/"used in this project" pass `make ranking` only in a narrow
  window (noted at the code); re-run `make ranking` after touching them.
- **Service-off picker:** recents are bounded and the search engine is reused per session; not yet
  benchmarked on a large history.

## Release

- **Licensing:** choose the root `LICENSE`, settle the xdiff (LGPL) distribution route, and add the
  missing license texts. Tracked in `shipit-checklist.md`.
- **Notices:** regenerate `THIRD_PARTY_NOTICES.md` against the release dependency set before shipping.
- **Linux packaging:** decide on tarball, .deb or Homebrew-on-Linux, and apply `shipit-checklist.md` to those artifacts.

## Ideas (unscheduled)

- Richer target discovery; optional MCP exposure.
- Zsh ghost text: an `smcli` strategy for zsh-autosuggestions (`smcli suggest` as the source), active only
  when that plugin is loaded. Fish and Bash unchanged.
