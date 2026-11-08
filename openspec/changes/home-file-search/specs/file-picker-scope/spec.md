# Spec Delta

## ADDED Requirements

### Requirement: Home scope searches the whole home tree

When the file picker's scope is the user's home directory, a file search SHALL return matches from any folder under it, not only from the first folder walked. This SHALL hold on macOS and Linux.

#### Scenario: Match outside the first dot-folder

- **GIVEN** a home directory holding `.config/` with many files and `Documents/report.txt`
- **WHEN** the scope is `~` and the user types `report`
- **THEN** `Documents/report.txt` is offered

#### Scenario: Widened up to home

- **WHEN** the user presses Shift+Tab until the scope is `~` and types the name of a file in another project under `~`
- **THEN** that file is offered

### Requirement: Fallback search reaches every top-level folder

When the service is not running, the in-process file search SHALL visit entries level by level: every entry at one depth below the scope before any entry one level deeper. When the entry limit is reached, only deeper levels SHALL be cut.

#### Scenario: Large first folder without the service

- **GIVEN** the service is stopped and the scope holds `a/`, whose few direct children lead to a deep subtree of more entries than the search limit, and `b/target.txt`
- **WHEN** the user types `target`
- **THEN** `b/target.txt` is offered

### Requirement: Frecent directories and files rank first

File results SHALL be boosted by how often and how recently the user worked in each directory (from captured commands) and picked each file or directory, counting records made under any project root inside the current scope. A record SHALL count only when its path is the scope or lies under it by exact, case-sensitive path components. Exact name matches SHALL still rank first.

#### Scenario: Pick made inside a project counts at home

- **GIVEN** the user often picked `crates/smcli/src/picker.rs` with the scope at `~/Dev/Apps/smcli`
- **WHEN** the scope is `~` and the user types `picker`
- **THEN** `Dev/Apps/smcli/crates/smcli/src/picker.rs` ranks above other `picker` files they never picked

#### Scenario: Directory worked in often

- **GIVEN** many captured commands ran in `~/Dev/Apps/smcli`
- **WHEN** the scope is `~` and the user types `smcli`
- **THEN** `Dev/Apps/smcli/` ranks above other matching directories with no history

#### Scenario: Records outside the scope are ignored

- **GIVEN** history and picks under `~/project_one`, `~/projectXone` and `~/Project_One`
- **WHEN** the scope is `~/project_one`
- **THEN** only records under `~/project_one` affect the ranking

#### Scenario: Blank query at home

- **WHEN** the scope is `~` and the query is empty
- **THEN** the most frecent directories and files are listed first

### Requirement: Ctrl+Enter changes into a directory

In the file picker, Enter SHALL insert the selected path as before. Ctrl+Enter, and Alt+Enter in every terminal, SHALL close the picker and change the shell's working directory to the selected directory without running anything else, in bash, zsh and fish. On a file, both keys SHALL do nothing.

#### Scenario: Jump to a directory

- **WHEN** the user selects `Dev/Apps/smcli/` and presses Alt+Enter
- **THEN** the picker closes, the shell's working directory is `~/Dev/Apps/smcli`, and the command line is unchanged

#### Scenario: Enter still inserts

- **WHEN** the user selects a directory and presses Enter
- **THEN** its quoted path is inserted at the cursor and the directory does not change

#### Scenario: File selected

- **WHEN** the user selects a file and presses Ctrl+Enter
- **THEN** the picker stays open and nothing changes

#### Scenario: One level wider than the limit

- **GIVEN** the service is stopped and the scope's top level holds `a/` with more direct children than the search limit, and `b/target.txt`
- **WHEN** the user types `b`
- **THEN** `b/` is offered, and `b/target.txt` is not guaranteed
