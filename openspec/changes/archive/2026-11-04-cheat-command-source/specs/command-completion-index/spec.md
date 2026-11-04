# Spec Delta

## MODIFIED Requirements

### Requirement: Pluggable example sources

Examples SHALL come from one or more sources, each identified by name and version, that answer "examples for program P on this platform". Each example SHALL carry a description, a command template and its source name. Adding or removing a source SHALL NOT require changes to the index or `@@` behavior beyond the examples offered. Sources SHALL answer offline at query time. Sources SHALL be ordered by priority, and a program's examples SHALL come only from the highest-priority source that has any for it.

#### Scenario: tldr as a source

- **WHEN** tldr is the only configured source
- **THEN** `@@` results match today's tldr examples for indexed programs, and only platform `common` or current-OS pages are used

#### Scenario: Two sources for one program

- **WHEN** two sources both have examples for `tar`
- **THEN** only the higher-priority source's examples for `tar` are indexed, and each result names that source

#### Scenario: Fallback to a lower-priority source

- **WHEN** the highest-priority source has no examples for a program and a lower-priority source does
- **THEN** that program's examples come from the lower-priority source and each result names it

## ADDED Requirements

### Requirement: Bundled cheat sheet fallback

smcli SHALL ship cheat/cheatsheets examples as a source with lower priority than tldr. Its data SHALL be generated offline from a pinned commit, bundled beside the executables like tldr, and attributed in the third-party notices. When its data is missing, `@@` SHALL behave as with tldr alone.

#### Scenario: Program only in cheat sheets

- **GIVEN** a program on `PATH` that has a cheat sheet and no tldr page
- **WHEN** the user runs `@@` for that program
- **THEN** results come from the cheat sheet and name `cheat` as their source

#### Scenario: Program in both

- **GIVEN** `tar` has a tldr page and a cheat sheet
- **WHEN** the user runs `@@` for `tar`
- **THEN** only tldr examples are offered

#### Scenario: Cheat data missing

- **WHEN** the bundled cheat data is absent or unreadable
- **THEN** `@@` works with tldr examples only and the index is named as a tldr-only index
