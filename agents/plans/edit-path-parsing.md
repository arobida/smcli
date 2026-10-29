# Shell-aware parsing for Ctrl-E path editing

## Problem

Ctrl-E on `eval "$(/Users/x/smcli/dist/smcli shell-init zsh)"` gives `eval @`, so picking a file yields
`eval crates/smcli/src/picker.rs`. Expected: `eval "$(@ shell-init zsh)"`, then
`eval "$(crates/smcli/src/picker.rs shell-init zsh)"`.

Two causes in `crates/smcli-core/src/parse.rs`:

- `read_word` (`parse.rs:489-548`) reads everything between `"` and its closing quote as one word, so it never sees the
  `$(…)` inside.
- The command word is never a path candidate (`parse.rs:394-400`), so `/abs/bin/smcli` would be skipped even if seen.

## Approach

Extend the existing tokenizer; no new dependency. A parser crate was considered and dropped: brush-parser covers
Bash only (Fish would still need the hand tokenizer) and errors on partial input; tree-sitter adds C grammars to every
binary that links smcli-core, and Zsh syntax the Bash grammar cannot parse would still need the old tokenizer.

**Modify** `crates/smcli-core/src/parse.rs`:

1. **Nesting.** Track a frame stack the way `insertable` does (`crates/smcli/src/shell_quoting.rs:59-146`): inside `"`,
   `$(`, `<(`, `>(` and backticks open a new command frame and are emitted as Op tokens; the matching `)` or backtick
   is an Op that closes it. Fish: `(` inside or outside `"` per the same rules `insertable` uses for fish. So `path_placeholders` takes the shell:
   `path_placeholders(command, shell, exists)`, with the caller passing `SMCLI_SHELL` (`cli.rs`, already read there).
2. **Command name.** In command position, a word containing `/` (`./run.sh`, `/abs/bin/smcli`) is a path candidate. A
   bare name (`git`) never is. This is a behavior change from the shipped version.
3. **Quoted words.** A fully quoted single path (`"old file.md"`) is still replaced whole by a bare `@`, as today;
   `quote()` re-quotes the picked path. The inside of a `"…"` that holds `$(…)` is not a path itself.
4. **Unbalanced input** (unclosed `"` or `$(`) returns None: no rewrite, Ctrl-E acts like Enter.

Offsets stay in bytes; the `\x1f` output contract is unchanged.

**Quoting the picked path needs no change.** For `eval "$(@`, the innermost frame is a command frame
(`shell_quoting.rs:125-127`), so `insertable` allows it and `quote()`'s single-quoted form is correct there.

`eval "$(…)"` works only because `$(` inside `"` is real syntax. Strings passed to `eval` or `bash -c '…'` are not
reparsed.

## Out of scope

- Expanding `$HOME`, `~` or variables to decide existence. `$HOME/x.md` stays as typed.
- Filling more than the first `@` automatically (`diff <(sort @) <(sort @)` leaves the second for Tab).
- Globs: any word with `*`, `?` or `[` is never a path, including `notes/*.md`.

## Tests

Unit tests for `path_placeholders`, `exists` stubbed to know only `src`:

- bash: `eval "$(/a/b/smcli shell-init zsh)"` → `eval "$(@ shell-init zsh)"`
- bash: `cat "$(ls notes/)"` → `cat "$(ls @)"`
- bash: ``cat `ls notes/` `` → ``cat `ls @` ``
- bash: `diff <(sort a.txt) <(sort b.txt)` → `diff <(sort @) <(sort @)`
- bash: `rm "old file.md"` → `rm @`
- bash: `cp $HOME/x.md dir/` → `cp $HOME/x.md @`
- bash: `(cd src && make)` → `(cd @ && make)`
- bash: `./run.sh a.md` → `@ @`; `git log` → None
- bash: `echo "$(ls notes/` (unclosed) → None
- zsh: `eval "$(/a/b/smcli shell-init zsh)"` → same as bash
- fish: `cat (ls notes/)` → `cat (ls @)`; `echo "a/b"` → `echo @`
- Every existing `path_placeholders` test still passes, updated only where rule 2 changes the result.

`scripts/test-file-picker.py`: Ctrl-E on `eval "$(<dist>/smcli shell-init zsh)"`, pick a file named `a b.md`, and the
line becomes `eval "$('a b.md' shell-init zsh)"` (fish: its own quoting) in Bash, Zsh and Fish.
