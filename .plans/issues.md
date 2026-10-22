# Open issues

Left open after the Rust port (`.plans/rust-port.md`, commit `d050644`).

- **fish shell checks fail on Linux.** In `make linux` (Debian `rust:latest` container), `scripts/test-shells.py` times out waiting for a fish prompt while it types the setup commands. bash and zsh pass there, and fish 4.9.3 passes on macOS. Cause not found; check the container's fish version and how the harness types into it.
- **Ranking: "search project" scores MRR 0.750** (top1 0.50, n=2) in `make ranking`. Not yet compared with the phase 3 result (search MRR 0.955 overall); find out whether this is a regression.
- **Flaky `context_cache::tests::refresh_is_nonblocking_and_failure_keeps_status`.** Its `< 300 ms` assertion failed once under the full `make test` load and passed every time on its own.
- **Picker reads the terminal size once** (`crates/smcli/src/picker.rs`, `TerminalSession::new`), so resizing during a pick is not followed. Swift re-read it every frame.
- **Licensing:** ten crates ship no license file (listed in `THIRD_PARTY_NOTICES.md`), and the xdiff LGPL question is still open (`.plans/shipit-checklist.md`).
- **Local leftovers to delete:** `.build/` (old Swift build output, gitignored) and the untracked `Sources/smcli/Resources/F2LLM-v2-80M` (only `.DS_Store` files).
