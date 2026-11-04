# history-search-ranking Specification

## Purpose

Defines how smcli retrieves and orders captured and native history commands for a query, and how ranking quality is measured before and after changes.

## Requirements

### Requirement: Full-command lexical recall

Search SHALL find a command by any term that appears in the full command text, including text beyond the embedding length cap.

#### Scenario: Term only in the tail of a long command

- **WHEN** history holds a command longer than 1,500 characters whose only occurrence of `deploy-tail-marker` is after character 1,500
- **THEN** a literal search for `deploy-tail-marker` returns that command

#### Scenario: Shared long prefix

- **WHEN** two commands share a prefix longer than 1,500 characters and differ only in their tail
- **THEN** each one is the top result for a query naming its own tail

### Requirement: Exact literal distinctions

For literal shell queries, search SHALL rank a command that contains the query's exact whole shell tokens above a command that matches only parts of those tokens, even if the partial match was run more often.

#### Scenario: Flag variant

- **WHEN** history has `git reset --hard HEAD` run 20 times and `git reset --soft HEAD` run once
- **THEN** the query `git reset --soft` ranks `git reset --soft HEAD` first

#### Scenario: Revision suffix

- **WHEN** history has `git reset HEAD~10` and `git reset HEAD~1`
- **THEN** the query `HEAD~1` ranks `git reset HEAD~1` above `git reset HEAD~10`

### Requirement: Identifier part matching

Search SHALL match a query against camelCase and PascalCase parts of command tokens, and SHALL still prefer the command containing the full identifier.

#### Scenario: Part of a target name

- **WHEN** history has `xcodebuild -scheme FooBarTests`
- **THEN** the query `bar tests` returns that command

#### Scenario: Full identifier wins

- **WHEN** history has `run FooBar`, `run Foo` and `run Bar`
- **THEN** the query `FooBar` ranks `run FooBar` first

### Requirement: Same results with or without the service

Ranking changes SHALL give the same ordering whether the service answers, the CLI falls back in-process, or the index lock forces the text fallback, apart from signals documented as service-only.

#### Scenario: Service unavailable

- **WHEN** the service is not running and the user searches
- **THEN** results match the service's for the same history and query

### Requirement: Measured ranking quality

The ranking benchmark SHALL report MRR, top-1 and top-3 with case counts for each cohort: the original cases, the new adversarial cases and a frozen held-out set. A change SHALL NOT lower the original cohort below its existing floors.

#### Scenario: Regression in the original cohort

- **WHEN** a change raises the overall score but lowers the original search cohort below MRR 0.954 or top-1 0.909
- **THEN** `make ranking` fails

### Requirement: Native history and returned text are preserved

Search SHALL return the command exactly as recorded, apart from inserting an alias expansion as defined in the shell-context-signals capability, and SHALL NOT modify native shell history files.

#### Scenario: Black-box check

- **WHEN** the full test suite runs against a temporary HOME with native history files
- **THEN** those files are byte-for-byte unchanged afterward

### Requirement: Same-kind project tier

Suggest and search SHALL boost a command run in a directory whose nearest project has the cwd project's primary kind, by less than runs in the cwd's own project, and only to raise its rank. With no project at the cwd or no determinable kind, ranking SHALL equal ranking without the tier. Through the service, suggest sees a kind change within 60 seconds and search by the next corpus rebuild (at most 5 minutes, sooner on a change of directory).

#### Scenario: Habit from another crate in a fresh crate

- **GIVEN** `cargo nextest run` was run often in other Rust projects, and `cargo new-thing` was run equally often and equally recently outside any Rust project
- **WHEN** the user suggests or searches `cargo ` in a Rust project with no history of its own
- **THEN** `cargo nextest run` ranks above `cargo new-thing`

#### Scenario: Own project still wins

- **GIVEN** a command was run in the cwd's project and another, equally often, only in other projects of the same kind
- **WHEN** the user suggests or searches a prefix both match
- **THEN** the command from the cwd's own project ranks first

#### Scenario: Deleted or unknown directory

- **WHEN** a run's directory no longer exists or has no recognized manifest
- **THEN** that run gets no same-kind boost and is not penalized

#### Scenario: Kind change seen through the service

- **GIVEN** the service has ranked with a run directory's kind cached
- **WHEN** that directory is deleted or loses its manifest
- **THEN** through the service, suggest sees the kind change within 60 seconds and search by the next corpus rebuild (at most 5 minutes, sooner on a change of directory), and in-process ranking stops at once

#### Scenario: Outside any project

- **WHEN** the cwd has no recognized manifest in it or above it
- **THEN** results are identical to results with the tier switched off

### Requirement: Same-kind ablation switch

The same-kind tier SHALL have its own `SMCLI_RANKING_OFF` switch named `kind`. With it set, suggest and search results SHALL be identical to results without the tier, with or without the service.

#### Scenario: Switched off

- **WHEN** `SMCLI_RANKING_OFF=kind` is set
- **THEN** suggest and search return the same order and `why` text as before this change

### Requirement: Same-kind tier is proven by benchmark

The ranking benchmark SHALL include same-kind cases, reported as their own group. The tier SHALL NOT ship unless the same-kind cases score a higher MRR with the tier on than with `kind` switched off, every existing cohort stays at or above its floor, and the black-box baseline shows warm suggest and search latency within run-to-run noise of the previous baseline.

#### Scenario: Benchmark comparison

- **WHEN** `make ranking` runs with the tier on and with `SMCLI_RANKING_OFF=kind`
- **THEN** the report shows the same-kind group's MRR, top-1 and top-3 for both, and the tier-on MRR is higher

#### Scenario: No regression elsewhere

- **WHEN** `make ranking` runs with the tier on
- **THEN** the original, adversarial and held-out cohorts are each at or above their existing floors

#### Scenario: Latency

- **WHEN** `make baseline` runs before and after the change on the same machine
- **THEN** warm suggest and hybrid search medians are within run-to-run noise, and the first-request difference is recorded in `agents/reference/baseline.md`

### Requirement: Command-index affinity bonus

When command-index examples match the query, reranking SHALL add a bonus to history candidates that share program and subcommand, and more for shared flags, with a matched template. The bonus SHALL be a documented service-only signal. On literal queries it SHALL NOT lift a candidate above one with an exact keyword hit.

#### Scenario: Matching program and flags rank higher

- **WHEN** an intent query matches a `tar -xzf` template and history has `tar -xzf a.tgz` and `tar -czf b.tgz` with equal usage
- **THEN** `tar -xzf a.tgz` ranks above `tar -czf b.tgz`

#### Scenario: Exact literal hit stays on top

- **WHEN** a literal query has an exact keyword hit and another row matches a command-index template
- **THEN** the exact keyword hit still ranks first

#### Scenario: In-process fallback

- **WHEN** the service is not running
- **THEN** no affinity bonus applies and results match the documented service-only difference

### Requirement: Natural-language benchmark cohort

The ranking benchmark SHALL include a natural-language cohort of queries that share no word with their expected command, and SHALL report its MRR, top-1 and top-3 alongside the other cohorts without lowering the original cohort's floors.

#### Scenario: Cohort reported

- **WHEN** `make ranking` runs
- **THEN** it prints MRR, top-1 and top-3 for the natural-language cohort
