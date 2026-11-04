# Proposal

## Why

Zsh and Fish record each pipeline stage's exit status (`pipestatus`); Bash records only the final `$?`, so a failing `make | tee log` looks successful from Bash. It is an open roadmap item and the record path already accepts pipeline statuses.

## What Changes

- The Bash hook reads `PIPESTATUS` in the same statement as `$?` and passes it to `smcli record`, as the Zsh hook does.
- In bash-preexec mode, the hook uses bash-preexec's saved copy of the pipeline status when that version provides one, and records none otherwise.
- The Bash hook header note ("pipeline status is unavailable") and the roadmap item are updated.

## Capabilities

### New Capabilities

- `command-capture`: what smcli's shell hooks record about each command; this change adds its pipeline-status requirement.

### Modified Capabilities

## Impact

- `crates/smcli/src/shell_integration.rs` (Bash template only).
- `scripts/test-shells.py` (Bash now asserts `pipestatus`).
- `agents/plans/roadmap.md` (remove the Bash item).
- No CLI, protocol or schema change: `record` already takes an optional pipeline-status argument and stores it.
