# Spec Delta

## ADDED Requirements

### Requirement: Same-kind project tier

Suggest and search SHALL boost a command run in a directory whose nearest project has the same primary kind as the cwd's nearest project. The boost SHALL be smaller than the boost for runs in the cwd's own project and SHALL only raise a command's rank. When the cwd is not in a project, or a run directory's kind cannot be determined, ranking SHALL equal ranking without this tier.

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
