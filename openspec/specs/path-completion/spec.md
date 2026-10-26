# path-completion Specification

## Purpose
Lets the user complete file and directory paths from the current project on the command line by starting a word with `@`, ranked by the project index and the user's past picks.

## Requirements

### Requirement: At-sign trigger

When the user invokes completion on a word starting with `@`, smcli SHALL offer project paths matching the rest of the word and replace the whole word, `@` included, with the chosen path quoted for the shell. Typed text SHALL NOT change unless the user invokes completion.

#### Scenario: Fuzzy file pick

- **WHEN** the user types `vim @rnkng` and invokes completion in a project containing `crates/smcli-search/tests/ranking.rs`
- **THEN** that file is offered and, once chosen, the line reads `vim crates/smcli-search/tests/ranking.rs`

#### Scenario: Scoped package left alone

- **WHEN** the user types `npm i @types/node` and presses Enter without invoking completion
- **THEN** the command runs exactly as typed

### Requirement: Directories after cd

For the argument of `cd`, `pushd` and similar directory commands, `@` completion SHALL offer directories only.

#### Scenario: Directory only

- **WHEN** the user completes `cd @serv`
- **THEN** only directories such as `crates/smcli-service` are offered

### Requirement: Glob completion

When the `@` word contains glob characters, completion SHALL offer the project files matching that glob, respecting the project's ignore rules.

#### Scenario: Glob

- **WHEN** the user completes `@src/**/*.rs`
- **THEN** the offered paths are the project's matching Rust files

### Requirement: Completion ranking and fallback

Offered paths SHALL rank previously chosen paths above equally good matches. Without the service, completion SHALL still work against the cwd's files, without the index ranking.

#### Scenario: Previous pick first

- **WHEN** two files match equally and the user chose one of them before
- **THEN** the chosen one is offered first

#### Scenario: No service

- **WHEN** the service is not running
- **THEN** `@` completion still offers matching paths
