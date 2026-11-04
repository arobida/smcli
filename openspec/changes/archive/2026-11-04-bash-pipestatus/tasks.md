# Tasks

## 1. Hook

- [x] 1.1 In the Bash template, read `PIPESTATUS` in `_smcli_prompt`'s first `local` statement (or `BP_PIPESTATUS` in bash-preexec mode when set) and pass it to `_smcli_record` as the optional pipeline argument; verify with `make build` and `bash -n` on the output of `smcli shell-init bash`.
- [x] 1.2 Confirm the minimum bash-preexec version that sets `BP_PIPESTATUS` and update the Bash header note (replace "pipeline status is unavailable"); verify the note matches the implemented behavior.

## 2. Tests

- [x] 2.1 Update `scripts/test-shells.py` so Bash asserts `pipestatus == [1]` for the existing single-command case, and add `false | true` -> exit `0`, `pipestatus [1, 0]` for Bash in PS0 and DEBUG modes; verify with `make test-shells`.
- [x] 2.2 Add a bash-preexec case (with and without `BP_PIPESTATUS`) and a case where a user `PROMPT_COMMAND` entry prints `${PIPESTATUS[*]}` unchanged; verify with `make test-shells`.

## 3. Docs

- [x] 3.1 Remove `Bash: record pipestatus` from `agents/plans/roadmap.md`; run `make docs`.
- [x] 3.2 Run `make test`; verify it passes.

## Workflow follow-up

- Archive the change after review.
