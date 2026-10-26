# Spec Delta

## MODIFIED Requirements

### Requirement: Record-time alias expansion

When a captured command starts with a shell alias, smcli SHALL store both the typed text and the expansion exactly as written when it ran. Expansions with variables, quotes and the `&&`, `||`, `|`, `;` operators SHALL be recorded. Expansions with command or process substitution, redirection, backgrounding or variable assignments SHALL be stored typed-only. smcli SHALL NOT read rc files, snapshot the alias table, read variable values, or guess expansions for native history or older entries.

#### Scenario: zsh alias

- **WHEN** a zsh user with `alias gst='git status'` runs `gst`
- **THEN** the run is stored with typed text `gst` and expansion `git status`

#### Scenario: Alias with variables and operators

- **WHEN** a user with `alias mkal='$EDITOR $ZDOTDIR/conf.d/50-aliases.zsh && source "$ZDOTDIR/.zshrc"'` runs `mkal`
- **THEN** the run is stored with typed text `mkal` and that expansion exactly as written, with `$EDITOR` and `$ZDOTDIR` unexpanded

#### Scenario: Redefined alias

- **WHEN** `gl` meant `git log` yesterday and `git ls-files` today
- **THEN** each run keeps the expansion it had when it ran, and the two are separate results

#### Scenario: fish abbreviation

- **WHEN** a fish abbreviation expands in the buffer before the command runs
- **THEN** the command is recorded as the expanded text, with no alias marker

#### Scenario: Unsupported expansion

- **WHEN** an alias expands to something containing `$(…)`, backticks, `<` or `>`, a lone `&`, or a variable assignment
- **THEN** the run is stored typed-only, with no expansion

### Requirement: Alias-aware search and insertion

Search SHALL match an aliased entry by both its typed text and its expansion, with the expansion treated as the primary text. The picker SHALL show the typed alias as the main text, followed by an `@` marker and the expansion, dimmed and truncated to the row width. Picking SHALL insert the typed alias. Alias runs and direct runs of the expansion SHALL keep separate usage counts.

#### Scenario: Search by expansion

- **WHEN** history has `gst` recorded with expansion `git status`
- **THEN** the query `git status` returns it, shown as `gst@ git status`

#### Scenario: Insert expansion

- **WHEN** the user picks `gst@ git status`
- **THEN** the typed alias `gst` is inserted instead of the expansion, and nothing is run

#### Scenario: Long expansion

- **WHEN** an expansion is wider than the space left in the picker row
- **THEN** it is truncated with an ellipsis, and the typed alias is never truncated before the expansion

### Requirement: Shell context privacy

Capture filters SHALL be applied to both the typed text and the expansion, on both the service and in-process record paths, before anything is written. If either fails, the whole run SHALL be rejected. Alias expansion bodies SHALL NOT appear in `why` output, logs or diagnostics. PATH SHALL NOT be stored.

#### Scenario: Secret-bearing alias

- **WHEN** a user runs `deploy` with `alias deploy='TOKEN=abc123 ./deploy.sh'`
- **THEN** no expansion is stored or passed to `smcli record`, and `why` output never contains `abc123`

#### Scenario: Filtered expansion

- **WHEN** an expansion matches a configured history exclusion pattern
- **THEN** the run isn't recorded at all, through either the service or the fallback

## ADDED Requirements

### Requirement: Measured alias recall

The ranking suite SHALL score how well plain-language queries find aliases among competing commands, with and without recorded expansions. Recording expansions SHALL score a higher mean reciprocal rank than typed-only.

#### Scenario: Expansion beats typed-only

- **WHEN** `make ranking` runs the alias evaluation
- **THEN** it reports MRR, top 1 and top 3 for typed-only and expanded aliases, and fails if expanded does not beat typed-only
