# Spec Delta

## ADDED Requirements

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
