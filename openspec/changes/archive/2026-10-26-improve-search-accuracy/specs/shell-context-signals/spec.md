# Spec Delta

## Purpose

Defines which signals from the user's live shell and working directory (recorded alias expansions, command availability, file presence) may influence ranking, and the privacy, freshness and fallback rules they must follow.

## ADDED Requirements

### Requirement: Signals can be disabled

Each shell-context signal SHALL have a ranking ablation switch (`alias`, `path`, `files` in `SMCLI_RANKING_OFF`). With a signal's switch set, ranking SHALL be identical to ranking without that signal.

#### Scenario: Signal disabled

- **WHEN** `SMCLI_RANKING_OFF=path` is set
- **THEN** ranking is identical to a build without the PATH signal

### Requirement: Unknown is neutral

A missing, stale, partial, slow or failed context lookup SHALL NOT lower a command's rank.

#### Scenario: Executable not found

- **WHEN** a command's program isn't found in the caller's PATH, or is a builtin, function, wrapper or absolute path
- **THEN** the command gets no PATH boost and is not demoted

#### Scenario: Missing file

- **WHEN** a command's path argument doesn't exist in the cwd
- **THEN** the command is not demoted

### Requirement: PATH availability boost

Search SHALL give a small boost, weaker than any usage signal, to a command whose program resolves to an executable file on the PATH of the shell making the request. The PATH SHALL come from the requesting client's environment, never the service's own environment, and SHALL NOT be persisted.

#### Scenario: Tool installed

- **WHEN** two otherwise equal commands start with `rg` and `ag`, and only `rg` is on the caller's PATH
- **THEN** the `rg` command ranks first

#### Scenario: Usage still dominates

- **WHEN** an `ag` command is used much more often than an otherwise equal `rg` command, and only `rg` is on the caller's PATH
- **THEN** the `ag` command still ranks first

### Requirement: File-presence boost

Search SHALL give a small boost to a command whose path-like arguments, for a fixed set of commands that read files, exist relative to the current cwd. The check SHALL use plain filesystem metadata without shell expansion, SHALL be bounded to the candidate pool, and SHALL be skipped where it could block, such as a remote or network mount.

#### Scenario: File exists here

- **WHEN** history has `cat notes.md` and `cat todo.md`, and only `notes.md` exists in the cwd
- **THEN** with otherwise equal scores, `cat notes.md` ranks first

#### Scenario: Output paths are ignored

- **WHEN** history has `mkdir build` and `build` doesn't exist
- **THEN** the command gets no file boost and is not demoted

### Requirement: Record-time alias expansion

When a captured command starts with a shell alias, smcli SHALL store both the command as typed and the alias's expansion at the time it ran. Only simple expansions (program, subcommand, flags and plain words; no substitutions, assignments, operators or quotes) SHALL be recorded. Anything else SHALL be stored typed-only. smcli SHALL NOT read rc files or snapshot the alias table, and SHALL NOT guess expansions for native history or older entries.

#### Scenario: zsh alias

- **WHEN** a zsh user with `alias gst='git status'` runs `gst`
- **THEN** the run is stored with typed text `gst` and expansion `git status`

#### Scenario: Redefined alias

- **WHEN** `gl` meant `git log` yesterday and `git ls-files` today
- **THEN** each run keeps the expansion it had when it ran, and the two are separate results

#### Scenario: fish abbreviation

- **WHEN** a fish abbreviation expands in the buffer before the command runs
- **THEN** the command is recorded as the expanded text, with no alias marker

#### Scenario: Unsupported expansion

- **WHEN** an alias expands to something containing `$(…)`, `;`, `|`, an assignment or quotes
- **THEN** the run is stored typed-only, with no expansion

### Requirement: Alias-aware search and insertion

Search SHALL match an aliased entry by both its typed text and its expansion, with the expansion treated as the primary text. The picker SHALL show aliased entries as `typed → expansion`, and picking one SHALL insert the expansion. Runs of an alias and runs of its expansion typed directly SHALL keep separate usage counts.

#### Scenario: Search by expansion

- **WHEN** history has `gst` recorded with expansion `git status`
- **THEN** the query `git status` returns it, shown as `gst → git status`

#### Scenario: Insert expansion

- **WHEN** the user picks `gst → git status`
- **THEN** `git status` is inserted into the command line and is not run

### Requirement: Shell context privacy

Capture filters SHALL be applied to both the typed text and the expansion, on both the service and in-process record paths, before anything is written. If either fails, the whole run SHALL be rejected. Alias expansion bodies SHALL NOT appear in `why` output, logs or diagnostics. PATH SHALL NOT be stored.

#### Scenario: Secret-bearing alias

- **WHEN** a user runs `deploy` with `alias deploy='TOKEN=abc123 ./deploy.sh'`
- **THEN** no expansion is stored, and `why` output never contains `abc123`

#### Scenario: Filtered expansion

- **WHEN** an expansion matches a configured history exclusion pattern
- **THEN** the run isn't recorded at all, through either the service or the fallback

### Requirement: No fff in ranking

fff's file index SHALL NOT be used as a ranking signal. The CLI and smcli-core SHALL NOT link fff.

#### Scenario: Fallback path

- **WHEN** the service is unavailable
- **THEN** search gives the same results using in-process file checks, and the CLI binary has no fff dependency
