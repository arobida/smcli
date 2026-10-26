# Spec Delta

## Purpose

Defines the evidence the service draws from its live file index of the current project (file presence, Git state, identifiers, definitions) to rank and suggest history commands and to expand search queries.

## ADDED Requirements

### Requirement: Index signals are service-only and additive

Each project-index signal SHALL apply only when the service answers with a ready index for the cwd's project root. Without one, ranking SHALL equal ranking without that signal. A signal SHALL only raise a command's rank, never lower it.

#### Scenario: CLI falls back in-process

- **WHEN** the service is not running and the user searches
- **THEN** results match the service's results with every project-index signal disabled

#### Scenario: Index still scanning

- **WHEN** the service's index for the root has not finished its first scan
- **THEN** no command is demoted for a path or identifier the index has not seen

### Requirement: Index signals can be disabled

Each project-index signal SHALL have its own `SMCLI_RANKING_OFF` switch (`index`, `git`, `evidence`, `tests`, `expand`). With a switch set, results SHALL be identical to results without that signal.

#### Scenario: Evidence disabled

- **WHEN** `SMCLI_RANKING_OFF=evidence` is set
- **THEN** ranking is identical to a build without the project-evidence signal

### Requirement: Index-backed file presence

When a ready index exists, the file-presence boost SHALL be decided from the index for every candidate in the pool, with the same command set, output-path exclusions and boost size as the filesystem probe.

#### Scenario: Whole pool checked

- **WHEN** 200 candidates name files and the index is ready
- **THEN** every candidate whose file exists in the project gets the file-presence boost

### Requirement: Git activity boost

Search and suggest SHALL give a small boost, weaker than any usage signal, to a command whose path argument is modified, staged or untracked in the project's Git worktree, or changed in its recent commits.

#### Scenario: File being edited

- **WHEN** history has `cargo test --test ranking` and `cargo test --test history`, and only `tests/ranking.rs` is modified
- **THEN** with otherwise equal scores, the ranking command comes first

#### Scenario: Usage still dominates

- **WHEN** the history command is used much more often than the ranking command
- **THEN** the history command still comes first

### Requirement: Project evidence boost

Search and suggest SHALL give a small boost to a command whose identifier arguments appear in the project's indexed files, and a larger one when an occurrence is a definition. Lookup SHALL be bounded in time per request; when the budget runs out, unchecked commands are treated as unknown.

#### Scenario: Test still defined

- **WHEN** history has `cargo test dedup_keeps_latest` and `cargo test old_removed_case`, and only `fn dedup_keeps_latest` exists in the project
- **THEN** with otherwise equal scores, `cargo test dedup_keeps_latest` ranks first

#### Scenario: Budget exceeded

- **WHEN** the evidence search runs out of time
- **THEN** the results equal the results without the evidence signal for every unchecked command

### Requirement: Test definition suggestions

Suggest SHALL offer test commands for test functions defined in the project, using the project's test runner, marked as suggestions from the project rather than history. Only names that are plain identifiers SHALL be offered.

#### Scenario: Rust test never run

- **WHEN** the project is a Cargo crate defining `#[test] fn parses_alias` and the user types `cargo test pars`
- **THEN** `cargo test parses_alias` is offered with a project-test label

#### Scenario: Hostile name

- **WHEN** a test file defines a name containing shell syntax
- **THEN** no suggestion is made for it

### Requirement: Query expansion from project code

When the best history hits for a query score weakly, search SHALL run one bounded grep of the project for the query terms, add the names of matching files and definitions as extra terms, and search history again. Expanded results SHALL rank below any strong direct match.

#### Scenario: Concept named in code only

- **WHEN** the user searches "migration tests", no history command contains "migration", and the project defines `tests/migrate.rs`
- **THEN** `cargo test --test migrate` from history is returned

#### Scenario: Strong direct hit

- **WHEN** a history command matches the query directly and strongly
- **THEN** no expansion runs and results are unchanged
