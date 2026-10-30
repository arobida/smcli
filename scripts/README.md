# `scripts/` — Build, install, and maintenance scripts

## Overview

Production scripts for building, installing, benchmarking, cleaning, and legal-notice generation. Test scripts (`test.sh`, `test-cli.sh`, `test-*.py`) and fixtures are excluded here; they are covered by the test gate.

## Interface

- `ateam.sh` — dispatches a brief to a team of agents via `kero`.
  - Usage: `bash scripts/ateam.sh <brief> <alias=command>...`
  - `brief` may be empty, inline text, a task file, or a task directory containing `<alias>.task` files.
  - Env overrides: `ATEAM_SHELL` (default `zsh`), `ATEAM_DIR` (default `$PWD`).
- `bench-baseline.py` — black-box benchmark of the CLI and generated hooks with synthetic history.
  - Usage: `python3 scripts/bench-baseline.py --binary PATH [--rows N...] [--samples N] [--output PATH] [--service on|off] [--repo-files N...]`
  - Writes `dist/baseline.json` by default.
- `clean-dist.sh` — removes old `dist/smcli-*` builds while keeping the pair the `dist/smcli` and `dist/smcli-service` symlinks point to.
  - Usage: `bash scripts/clean-dist.sh <dist directory>`
- `gen-notices.py` — regenerates `THIRD_PARTY_NOTICES.md` from `Cargo.lock` and dependency license files.
  - Usage: `python3 scripts/gen-notices.py`
- `install.sh` — macOS installer for binaries, model, shell hooks, picker keys, and service start.
  - Usage: `bash scripts/install.sh [--prefix DIR] [--from DIR] [--shells LIST|all|none] [--keys LIST] [--atuin yes|no] [--yes]`
  - From a source checkout it builds with `make release` first if binaries are not beside the script.
- `uninstall.sh` — exported as `smcli-uninstall`; removes binaries, hooks, database, index, and settings.
  - Usage: `smcli-uninstall [--export DIR] [--no-export] [--keep-data] [--prefix DIR] [--yes]`

## Invariants

- `ateam.sh` must be run with `bash`, not sourced from zsh.
- `install.sh` is macOS-only; Linux installs copy binaries and `potion-shell-16m/` manually (see [`docs/install.md`](../docs/install.md)).
- `install.sh` and `uninstall.sh` only touch the marked `# >>> smcli >>>` / `# <<< smcli <<<` block in shell startup files; hand-added `shell-init` lines are reported but not removed.
- `gen-notices.py` only reads dependency sources and license files; it never interprets license text as legal advice.
- `bench-baseline.py` uses synthetic data and never reads the user's real history.

## Side Effects

- `ateam.sh`: starts `kero` sessions, sends messages, prints `alias\tsession-id` lines.
- `bench-baseline.py`: creates temporary directories/seeds a synthetic SQLite database, imports fixture history, starts/stops `smcli-service`, allocates PTYs, writes `dist/baseline.json`.
- `clean-dist.sh`: deletes files in `dist/`.
- `gen-notices.py`: runs `cargo metadata`, reads crate sources and license files, overwrites `THIRD_PARTY_NOTICES.md`.
- `install.sh`: copies `smcli`, `smcli-service`, `potion-shell-16m/`, and `uninstall.sh` into the prefix; appends shell hooks to `.zshrc`/`.bashrc`/`config.fish`; runs `smcli init`; optionally imports Atuin history; starts the service.
- `uninstall.sh`: by default exports the selected database using `sqlite3` and, when the CLI exists, JSON history; an attempted export failure stops removal. It attempts to stop the selected database’s service, removes hooks and binaries, and deletes the selected database, search index and config. `--keep-data` skips export and preserves data/settings; `--no-export` deletes without backup. When deleting data it also removes every `service-*` entry in the selected/default runtime directories, potentially affecting other database instances whose services were not stopped.

## Verification

- Run the full gate: `make test`
- Run install/uninstall tests after `make build`: `make test-install`
- Regenerate notices: `make notices`
- Run baseline: `make baseline`
