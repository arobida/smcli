# Roadmap

Open work, not scheduled. macOS and Linux are supported today; search is included on both.

## Verification

- **Linux:** run `make linux` (Docker) to exercise the full suite on Linux. The historical Fish prompt
  timeout in `scripts/test-shells.py` is still unverified; record the container Fish version and the prompt
  transcript if it recurs.
- **Picker resize:** `TerminalSession::new` reads the terminal size once and the event loop ignores
  `Event::Resize` (`crates/smcli/src/picker.rs`, recorded `shortcut:` decision). Re-read per frame only if
  resizing matters in practice.

## Search

- **fgrep on large trees:** partial results are reported now. Still open: enforce the time budget on
  zero-match searches, debounce or cancel stale keystrokes, resume from `next_file_offset`, and choose an
  interactive budget. See `fgrep-notes.md`.
- **Incremental corpus for writes from another process:** a fallback record, import or prune makes the
  next search rebuild the corpus (about 760 ms at 100k runs). Today only the service's own records update
  it in place.
- **Ranking quality:** one known project-search miss (`"swift test"` in a package, expected
  `swift test --filter Lib`). Tune only with a targeted `make ranking` comparison before and after.
- **Optional, only if profiling asks for it:** a persistent Zsh transport and async suggestions.

## Capture

- **Fish:** start times are whole seconds (Fish has no clock builtin), and the picker and search do not
  read Fish's native history file.
- **Bash:** record `pipestatus` like Zsh and Fish do.

## Release

- **Licensing:** choose the root `LICENSE`, settle the xdiff (LGPL) distribution route, and add the
  missing license texts. Tracked in `shipit-checklist.md`.
- **Notices:** regenerate `THIRD_PARTY_NOTICES.md` against the release dependency set before shipping.
- **Linux packaging:** decide on tarball, .deb or Homebrew-on-Linux, and apply `shipit-checklist.md` to those artifacts.

## Ideas (unscheduled)

- Richer target discovery; optional MCP exposure.
