# Edit a command's path arguments

## Goal

In the history picker, the user picks a past command that names files or directories and swaps those paths for new ones
without retyping the rest of the command.

## Flow

1. In the history picker, the user highlights a command such as `cp notes/a.md backup/`.
2. The user presses **Ctrl-E** (edit paths).
3. The shell puts `cp @ @` on the command line and opens the file picker at once on the first `@`.
4. Enter replaces that `@` with the chosen path, quoted, as `@`-Tab does today.
5. The user moves the cursor to the next `@` by hand and presses Tab, which opens `@`-Tab as usual.

Tab only opens the picker on a stand-alone `@`, so later placeholders inside a word (`--out=@`) are filled by hand. The
placeholders are literal text: pressing Enter on `cp @ @` runs cp with `@` arguments.

## Design

- **Key.** Ctrl-E in `picker.rs`, History mode only, with a selection (same guard as Enter). Ctrl-Enter is not used:
  most terminals send it as plain Enter. Ctrl-E is free in the picker, and the query has no cursor to move.
- **Choice.** A `edit_paths: bool` on `Choice::Command`, so `cli.rs` keeps one match arm and still records the pick for
  ranking.
- **Exit code.** `smcli pick` tells the shell what it returned by exit code: 0 is a command, 10 is a path. EditPaths
  adds 11, whose output is the rewritten command. With no paths found, Ctrl-E returns exit 0, like Enter.
- **Path detection.** A pure function in smcli-core, `path_placeholders(command, exists) -> String`, with an `exists`
  closure. It needs a quote-aware splitter that returns byte ranges (check `shell_quoting.rs` first); words that are
  not paths keep their quoting as typed. Detection reads the text first:
  - **Looks like a path** (replaced without touching disk): contains `/`, starts with `.`, `~` or `$HOME`, or ends in
    a file extension (`a.md`, `Cargo.toml`). Works for deleted files and from any directory.
  - **Bare word** (`log`, `src`): replaced only when `exists` says so. `exists` checks the directory the command ran
    in, looked up in the SQLite log for captured history; native history has no directory, so it checks the current
    one.
  - **Never paths:** the first word of each command (after `|`, `&&`, `;`, and after `VAR=x` assignments), flags
    (`-x`; for `--out=x` only the value part is checked), words starting with `@`, URLs (`://`), and `user@host:`
    remotes. Globs are left alone. Redirection targets (`> out`) are paths.
  - Detection runs on the typed command, not the alias expansion.
- **Shell side.** On exit 11, each pick widget sets the buffer and runs `pick --files` directly with an empty query.
  It then fills the first placeholder wherever it sits, including inside a word such as `--out=@`, and leaves the
  cursor after the path. Searching the line for `@` would hit `@scope/pkg` first, so smcli prints the line split at the
  first placeholder: the text before it, a unit separator (0x1f), then the text after it. The shell sets the buffer to
  before + path + after. Calling the `@`-Tab
  widget is avoided: with exactly one hit it inserts without opening the picker, and in Bash it rebinds `\e[0n`. The
  cursor offset is computed in the shell (Bash 4 counts `READLINE_POINT` in bytes). Fish replaces its trailing
  `commandline -C 999999`. Without key bindings (`SMCLI_NO_KEYS`, Bash < 4), the line is left as `cp @ @`.

## Out of scope (first version)

- **Multi-select (Ctrl-Space).** The file picker returns one path today. Choosing several paths would also help plain
  `@`-Tab, so it gets its own change.
- Moving to the next `@` after a pick. It is one cursor move plus Tab.
- Bare-word paths from native history that ran in another directory. They are not detected; the user edits them by
  hand.

## Tests

- Unit tests for `path_placeholders`, with an `exists` closure that knows only `src`:
  - `cp notes/a.md backup/` → `cp @ @` (nothing on disk)
  - `rm -rf "old file.md"` → `rm -rf @`
  - `vim ~/x.toml` → `vim @`
  - `git log` → unchanged, exit 0
  - `ls src` → `ls @` (bare word, exists)
  - `npm i @scope/pkg ./a.md` → `npm i @scope/pkg @`
  - `FOO=./a.md cat b.txt` → `FOO=./a.md cat @`
  - `cargo build --manifest-path=x/Cargo.toml` → `cargo build --manifest-path=@`
  - `git clone https://x.com/r.git` → unchanged
  - `echo hi > out.log` → `echo hi > @`
- `scripts/test-file-picker.py`: Ctrl-E on a recorded `cp` command opens the file picker, and Enter leaves
  `cp <path> @` on the line, in Bash, Zsh and Fish.
- String asserts in `shell_integration.rs` tests for the exit-11 branch, and the footer hint text.
- Docs: footer hint (`picker.rs`), `usage.txt`, README.
