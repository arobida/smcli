# Spec Delta

## Purpose

Defines the shell keys that open smcli pickers from the command line and how they coexist with bindings the user or terminal already relies on.

## ADDED Requirements

### Requirement: Ctrl-O opens the command picker

When the Ctrl-O key is enabled, pressing Ctrl-O at the bash, zsh or fish prompt SHALL open the command-example picker, using the text left of the cursor as its context in the same way `@@` completion does. Choosing an example SHALL put it on the command line without running it, keeping a leading `sudo`, `env` or `command`. Cancelling SHALL leave the line unchanged.

#### Scenario: Empty line

- **WHEN** the line is empty and the user presses Ctrl-O
- **THEN** the picker opens on examples for all installed programs

#### Scenario: Line names a program

- **WHEN** the line is `sudo tar ` and the user presses Ctrl-O and chooses an example
- **THEN** the line becomes `sudo ` followed by the chosen `tar` example

#### Scenario: Cancel

- **WHEN** the user opens the picker with Ctrl-O and presses Esc
- **THEN** the command line is exactly as before

#### Scenario: No index yet

- **WHEN** no command index exists
- **THEN** the picker opens with the same status message `@@` shows

### Requirement: Ctrl-O key is configurable

The Ctrl-O key SHALL be on by default and SHALL be switchable through `smcli init` (interactive prompt and `--keys ctrl-o`) and the installer's `--keys` list. `--keys` without `ctrl-o` SHALL turn it off.

#### Scenario: Turned off

- **WHEN** the user runs `smcli init --keys ctrl-r,ctrl-t` and opens a new shell
- **THEN** Ctrl-O keeps its previous behavior

### Requirement: Ctrl-O yields to existing bindings

smcli SHALL NOT replace a Ctrl-O binding other than the shell's default (`operate-and-get-next`, `accept-line-and-down-history`); it SHALL keep it and list Ctrl-O in the kept-bindings notice. When it does bind Ctrl-O, it SHALL clear the tty DISCARD character so the key reaches the shell.

#### Scenario: User binding

- **WHEN** the user's zsh config binds `^O` to a custom widget
- **THEN** smcli leaves it and lists it as kept
