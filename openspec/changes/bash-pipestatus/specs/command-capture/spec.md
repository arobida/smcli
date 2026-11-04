# Spec Delta

## Purpose

Defines what smcli's shell hooks record about each command a user runs, so the stored history is the same kind of data from every supported shell.

## ADDED Requirements

### Requirement: Pipeline status capture

Bash, Zsh and Fish hooks SHALL record the exit status of every stage of the command's pipeline, in order, alongside the final exit code. Reading it SHALL NOT change `$?` or the pipeline status seen by the user's own prompt hooks. When the shell cannot report it, the hook SHALL record no pipeline status rather than a guessed one.

#### Scenario: Failing first stage in Bash

- **WHEN** a user runs `false | true` in Bash with the hook installed
- **THEN** the recorded run has exit code `0` and pipeline status `[1, 0]`

#### Scenario: Single command in Bash

- **WHEN** a user runs `false` in Bash
- **THEN** the recorded run has exit code `1` and pipeline status `[1]`

#### Scenario: bash-preexec without a saved pipeline status

- **GIVEN** Bash with a bash-preexec version that does not keep the pipeline status for precmd functions
- **WHEN** a user runs `false | true`
- **THEN** the recorded run has exit code `0` and no pipeline status

#### Scenario: User prompt hooks unaffected

- **GIVEN** a user `PROMPT_COMMAND` entry that prints `${PIPESTATUS[*]}`
- **WHEN** a user runs `false | true`
- **THEN** that entry prints the same value as without smcli's hook
