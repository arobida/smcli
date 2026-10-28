# Spec Delta

## Purpose

Defines how the interactive file and grep picker chooses its search scope, how the user narrows and widens it with Tab and Shift+Tab, and how the cwd and scope are shown.

## ADDED Requirements

### Requirement: Project-wide initial scope

When the file picker opens, it SHALL search the whole project: the repository root containing the cwd, or the cwd itself when it is not inside a repository. It SHALL NOT start from a one-level listing of the cwd.

#### Scenario: Opened in a subdirectory

- **WHEN** the user opens the file picker in `smcli/crates/smcli` and types `ranking`
- **THEN** `crates/smcli-search/tests/ranking.rs` is offered

#### Scenario: Outside a repository

- **WHEN** the user opens the file picker in a directory that is not in a repository
- **THEN** the scope is that directory and its subtree

### Requirement: No scope toggle key

The picker SHALL NOT bind `Ctrl+A` to switch between cwd and global search; scope changes only through Tab and Shift+Tab.

#### Scenario: Ctrl+A

- **WHEN** the user presses `Ctrl+A` in the file picker
- **THEN** the scope does not change

### Requirement: Tab narrows into a directory

When the selected file-picker entry is a directory, `Tab` SHALL make that directory the search scope, clear the query, and show results from within it.

#### Scenario: Drill down

- **WHEN** the scope is the project root and the user presses Tab on `crates/smcli-service/`
- **THEN** the scope becomes `crates/smcli-service` and only paths under it are offered

### Requirement: Tab on a file greps that file

When the selected file-picker entry is a file, `Tab` SHALL switch to grep mode restricted to that file only. Leaving that grep (Shift+Tab or switching mode) SHALL return to the file picker at the previous scope.

#### Scenario: Grep one file

- **WHEN** the user presses Tab on `crates/smcli/src/picker.rs` and types `KeyCode`
- **THEN** only matching lines from `picker.rs` are offered

### Requirement: Shift+Tab widens to the parent

`Shift+Tab` SHALL change the scope to the parent directory of the current scope regardless of the selection, one level per press, and SHALL stop at the user's home directory. When the scope is `~` or not under `~`, Shift+Tab SHALL NOT widen further.

#### Scenario: Walk up to home

- **WHEN** the scope is `/Users/u/Dev/Apps/smcli` and the user presses Shift+Tab three times
- **THEN** the scope is `/Users/u/Dev/Apps`, then `/Users/u/Dev`, then `/Users/u`

#### Scenario: Stop at home

- **WHEN** the scope is `~` and the user presses Shift+Tab
- **THEN** the scope stays `~`

### Requirement: Cwd and scope displayed

The picker SHALL show the cwd and the current scope at all times in file and grep modes, with paths under `~` abbreviated as `~/…`. A single-file grep SHALL show that file as its scope.

#### Scenario: Header after drill down

- **WHEN** the cwd is `~/Dev/Apps/smcli` and the scope is `~/Dev/Apps/smcli/crates`
- **THEN** both `~/Dev/Apps/smcli` and `~/Dev/Apps/smcli/crates` are visible in the picker

### Requirement: Inserted paths

A chosen path SHALL be inserted relative to the cwd when it is inside the cwd, and as a `~/`-relative or absolute path otherwise, quoted for the shell.

#### Scenario: Pick outside cwd

- **WHEN** the cwd is `~/Dev/Apps/smcli`, the scope is `~/Dev`, and the user picks `~/Dev/other/README.md`
- **THEN** the inserted path resolves to that file from the cwd

### Requirement: Works without the service

Scope narrowing, widening and single-file grep SHALL work when the service is not running, using in-process search over the same scope.

#### Scenario: No service

- **WHEN** the service is stopped and the user presses Shift+Tab then types a query
- **THEN** results come from the parent directory
