# Spec Delta

## Purpose

Defines the shell keys that open smcli pickers from the command line and how they coexist with bindings the user or terminal already relies on.

## ADDED Requirements

### Requirement: Ctrl-H opens the command picker

When the Ctrl-H key is enabled, pressing Ctrl-H at the bash, zsh or fish prompt SHALL open the command-example picker, using the text left of the cursor as its context in the same way `@@` completion does. Choosing an example SHALL put it on the command line without running it, keeping a leading `sudo`, `env` or `command`. Cancelling SHALL leave the line unchanged.

#### Scenario: Empty line

- **WHEN** the line is empty and the user presses Ctrl-H
- **THEN** the picker opens on examples for all installed programs

#### Scenario: Line names a program

- **WHEN** the line is `sudo tar ` and the user presses Ctrl-H and chooses an example
- **THEN** the line becomes `sudo ` followed by the chosen `tar` example

#### Scenario: Cancel

- **WHEN** the user opens the picker with Ctrl-H and presses Esc
- **THEN** the command line is exactly as before

#### Scenario: No index yet

- **WHEN** no command index exists
- **THEN** the picker opens with the same status message `@@` shows

### Requirement: Ctrl-H key is configurable

The Ctrl-H key SHALL be on by default and SHALL be switchable through `smcli init` (interactive prompt and `--keys ctrl-h`) and the installer's `--keys` list. `--keys` without `ctrl-h` SHALL turn it off.

#### Scenario: Turned off

- **WHEN** the user runs `smcli init --keys ctrl-r,ctrl-t` and opens a new shell
- **THEN** Ctrl-H keeps its previous behavior

### Requirement: Ctrl-H yields to Backspace

smcli SHALL NOT bind Ctrl-H when the terminal's erase character is `^H`, or when Ctrl-H holds a binding other than the shell's default backward-delete; in those cases it SHALL keep the existing binding and list Ctrl-H in the kept-bindings notice. Inside the picker, Ctrl-H SHALL keep deleting the previous query character.

#### Scenario: Terminal sends ^H for Backspace

- **WHEN** `stty` reports erase as `^H` and the shell starts
- **THEN** Backspace still deletes, and the notice lists Ctrl-H as kept

#### Scenario: User binding

- **WHEN** the user's zsh config binds `^H` to a custom widget
- **THEN** smcli leaves it and lists it as kept
