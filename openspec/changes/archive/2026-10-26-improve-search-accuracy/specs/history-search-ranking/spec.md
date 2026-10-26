# Spec Delta

## Purpose

Defines how smcli retrieves and orders captured and native history commands for a query, and how ranking quality is measured before and after changes.

## ADDED Requirements

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
