# Roadmap

Open work, not scheduled. macOS is the current target. Linux is still wanted, but all Linux work is on the backburner; see [Linux (backburner)](#linux-backburner).

## Search

- **fgrep on large trees:** partial results are reported, the budget is enforced on zero-match searches,
  and stale queries are coalesced. Still open: a running obsolete query finishes before the next, resume from
  `next_file_offset`, and choose an interactive budget by measurement. Key measurements are in the [2026-11-08 review](../reviews/codebase-review-2026-11-08.md).
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
- **Picker typeahead:** input read together with the selecting Enter is dropped when the picker exits,
  because crossterm has already queued it. A very fast second Enter can be lost.
- **Suggestion snapshot:** at 100,000 distinct commands it takes 9.4 ms with a prefix and 41.2 ms with an
  empty prefix, over the 5 ms planning budget. Going lower needs a shared structure instead of a copy.
- **Project-file boost:** the unlisted-script rule covers `<tool> run x` only, so `yarn x` can still be
  boosted when `x` is not a listed script.

## Release

- **Licensing:** choose the root `LICENSE`, settle the xdiff (LGPL) distribution route, and add the
  missing license texts. Tracked in `shipit-checklist.md`.
- **Notices:** regenerate `THIRD_PARTY_NOTICES.md` against the release dependency set before shipping.

## Linux (backburner)

Not a priority: do not run `make linux`, and do not let Linux issues block macOS work. The existing Linux code stays in place. Pick these up together when Linux becomes the focus.

- **Full suite:** `make linux` has not been confirmed passing; Docker was unavailable on the last recorded attempts. Run it first. It includes the `/proc/<pid>/environ` check in `scripts/test-service.py`, which runs only on Linux.
- **Fish:** the historical prompt timeout in `scripts/test-shells.py` is unverified. Record the container Fish version and the prompt transcript if it recurs.
- **musl:** the descriptor close (`close_range`, `/proc/self/fd` fallback in `service_client.rs`) is compile-checked only. Run it on a musl system with the Linux release.
- **Earlier acceptance:** the Linux acceptance step in `archive/build-test-efficiency.md` is unverified.
- **Install:** `scripts/install.sh` stops on anything but macOS, so Linux users copy files by hand. Add a Linux path, or delete its unreachable Linux `stat` branch.
- **Packaging:** decide on tarball, .deb or Homebrew-on-Linux, and apply `shipit-checklist.md` to those artifacts.
- **File watching:** inotify needs one watch per directory, so a huge tree can hit the user's limit and leave Git status refreshing by age only (`context_cache.rs`). The recursive watch is also set up while the context cache lock is held; its cost on Linux is unmeasured.
- **Dead-code audit:** `make unused` on macOS cannot see private code compiled only on Linux. Run it on Linux too.
- **Docs:** `README.md` and `docs/install.md` say "Linux support coming soon!". When Linux ships, replace that and restore the build prerequisites: a C toolchain, `pkg-config` and SQLite development headers (`build-essential pkg-config libsqlite3-dev` on Debian/Ubuntu).

## Ideas (unscheduled)

- Richer target discovery; optional MCP exposure.
- Zsh ghost text: an `smcli` strategy for zsh-autosuggestions (`smcli suggest` as the source), active only
  when that plugin is loaded. Fish and Bash unchanged.
