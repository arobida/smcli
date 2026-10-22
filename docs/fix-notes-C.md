# Batch C fix notes

Completed findings 1, 3 and 19a only. No commits.

## Changes

- `scripts/uninstall.sh:13,59-71`: set `umask 077`; require sqlite3 before creating an export or removing anything; remove the unsafe live-database `cp` fallback; enforce export directory mode `0700` and each existing exported history/config file mode `0600`.
- `docs/install.md:211`: document the sqlite3 requirement, skip-export option and private export modes (including existing destinations).
- `scripts/test-shells.py:882-884`: wait for every concurrent writer, collect `(args, returncode)`, and report every failure together. Stderr remains inherited.
- `scripts/test-install.sh:37-60`: add regression checks that missing sqlite3 leaves the installation/data/hooks untouched and that existing permissive export destinations become private under caller umask `022`.

## Verification

Passed:

- `bash -n scripts/uninstall.sh scripts/test-install.sh`
- `python3 -m py_compile scripts/test-shells.py`
- `git diff --check -- scripts/uninstall.sh scripts/test-shells.py scripts/test-install.sh docs/install.md`
- Isolated Python assert-based check invoking the real uninstaller in a temporary HOME with a stub CLI: missing sqlite3 preserves files/hooks and creates no export; a real sqlite3 backup preserves a committed WAL-only row; existing `0755`/`0644` destinations become `0700`/`0600`; successful uninstall removes only the temporary installation.
- Isolated execution of the exact writer-wait statements extracted via Python AST with three fake writers: first and third fail, all three are waited, and the assertion names both failed commands and exit codes.

Not run: full install/shell integration suites, make targets, builds or Cargo checks (no Rust changed). Graph regeneration is left to the orchestrator's explicit verification step to avoid rewriting shared graph files while other batches work.
