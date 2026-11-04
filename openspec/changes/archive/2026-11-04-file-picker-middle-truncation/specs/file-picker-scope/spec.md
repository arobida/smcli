# Spec Delta

## ADDED Requirements

### Requirement: Long paths keep the file name

When a path in a file or grep picker row is wider than the space available for it, the picker SHALL shorten it by replacing a middle part with `…`, keeping the final path component whole and as many leading characters as fit. Only when the final component alone does not fit SHALL its end be cut. Shortening SHALL change only what is displayed: the inserted path SHALL be the full path.

#### Scenario: Deep file path

- **GIVEN** a row width that fits 40 characters of path
- **WHEN** the file picker shows `services/backend/internal/handlers/user_profile.go`
- **THEN** the row shows a path that starts with `services/`, contains `…` and ends with `/user_profile.go`, at most 40 characters wide

#### Scenario: Path that fits

- **WHEN** a path fits in the available width
- **THEN** it is shown unchanged

#### Scenario: Picking a shortened row

- **WHEN** the user picks a row whose path was shortened
- **THEN** the full path is inserted, quoted and relative as before

#### Scenario: Grep row keeps the match visible

- **GIVEN** grep mode and a long path
- **WHEN** a match row is shown
- **THEN** the path is shortened to its share of the row and the `:line` and part of the matched text remain visible

#### Scenario: Wide characters

- **WHEN** a path contains double-width characters
- **THEN** the shortened path is measured by display width and never exceeds the available space
