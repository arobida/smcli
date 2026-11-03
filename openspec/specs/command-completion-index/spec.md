# command-completion-index Specification

## Purpose

Defines the command-completion index: the programs smcli offers command examples for through `@@`, chosen from the OS's own CLI tools and the user's history, with examples supplied by pluggable sources such as tldr.

## Requirements

### Requirement: OS-shipped programs

The index SHALL include every program that ships with the OS and resolves to an executable on the user's `PATH`. On macOS, OS-shipped means an executable in `/bin`, `/sbin`, `/usr/bin` or `/usr/sbin`. Programs from other directories (Homebrew, `~/.local/bin`, etc.) SHALL NOT be added by this rule.

#### Scenario: Built-in tool indexed

- **WHEN** smcli builds the index on macOS
- **THEN** `/usr/bin/tar` is indexed, if any source has examples for `tar`

#### Scenario: Homebrew tool not indexed by default

- **WHEN** `/opt/homebrew/bin/jq` exists but the user has never run `jq`
- **THEN** `jq` is not in the index

### Requirement: Programs on PATH only

A program SHALL be indexed only while it resolves to an executable file (or symlink to one) on the user's shell `PATH`. When the service syncs and a program no longer resolves, its examples SHALL be deleted.

#### Scenario: System dir not on PATH

- **WHEN** the user's `PATH` omits `/usr/sbin`
- **THEN** programs only found in `/usr/sbin` are not indexed

#### Scenario: Program uninstalled

- **WHEN** an indexed program is removed and the service next syncs
- **THEN** its examples are gone from the index

### Requirement: Service-owned sync

The service SHALL keep the index in sync: the wanted set is (OS-shipped programs ∪ programs in captured history) that resolve on `PATH` and have examples from a source. It SHALL add missing programs and delete programs no longer wanted. A sync SHALL run when a captured command's program is not yet known or the captured shell's `PATH` differs from the last synced one, off the capture reply path. `@@` queries SHALL only read the index and SHALL NOT perform discovery or PATH checks.

#### Scenario: Newly used program

- **WHEN** the user installs `jq` and runs `jq . file.json`
- **THEN** the next bare `@@ pretty print json` can return `jq` examples

#### Scenario: No examples anywhere

- **WHEN** a captured program has no examples from any source
- **THEN** the index is unchanged and the program is not looked up again

#### Scenario: Query does no discovery

- **WHEN** the user runs `@@` repeatedly
- **THEN** no PATH scan or history scan happens per query

#### Scenario: No service

- **WHEN** the service is not running and no index exists
- **THEN** the CLI builds the index once in-process; later sync waits for the service

### Requirement: Pluggable example sources

Examples SHALL come from one or more sources, each identified by name and version, that answer "examples for program P on this platform". Each example SHALL carry a description, a command template and its source name. Adding or removing a source SHALL NOT require changes to the index or `@@` behavior beyond the examples offered. Sources SHALL answer offline at query time.

#### Scenario: tldr as a source

- **WHEN** tldr is the only configured source
- **THEN** `@@` results match today's tldr examples for indexed programs, and only platform `common` or current-OS pages are used

#### Scenario: Two sources for one program

- **WHEN** two sources both have examples for `tar`
- **THEN** examples from both are indexed and each result names its source

### Requirement: Index lifecycle

The index SHALL be named by model identity and the set of source names and versions, rebuilt when any change, listed by `smcli indexes`, and removed by `--prune` when not current. `@@` SHALL work with or without the service running.

#### Scenario: Model switch

- **WHEN** the user runs `smcli init --potion DIR`
- **THEN** the next `@@` uses an index built for the new model, and the old one is prunable
