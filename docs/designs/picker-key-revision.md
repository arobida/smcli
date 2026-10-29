# Picker key revision (^R / ^T / ^G)

- **Author:** Andrew Robida
- **Status:** Draft
- **Date:** 2026-10-29

------------------------------------------------------------------------

## Overview

The smcli picker has three modes: history, files and grep. In the terminal,
^R, ^T and ^G already open each one. Inside the picker the same keys behave
differently. ^T switches between files and history, ^F cycles the file filter,
^G switches between grep and history, and ^R, ^P, ^N and ^S move the
selection. This change makes each key open its own mode in both places. Pressing
the key again inside its mode cycles that mode's own options.

### Terms

- **Full history:** recent captured history plus native shell history, newest
  first. This is what ^R opens today.
- **Suggested history:** this directory's ranked suggestions, the list → opens
  on an empty prompt (`smcli pick --suggest`, the `suggest` flag in cli.rs).
- **Semantic search:** Tab in History. It runs an embedding search over full
  history.
- **`kind`:** the Files filter. 0 is files+dirs, 1 is dirs and 2 is files. `@`
  directory completion sets `SMCLI_FILES_DIRS`, which opens Files on 1.

## The Problem

- Inside the picker, ^R moves the selection up instead of opening history,
  even though ^R opens history from the terminal.
- Full history (^R) and suggested history (→ on an empty prompt) are two
  separate pickers. You can't switch from one to the other without closing the
  picker.
- The file filter is on ^F, an extra key to remember, while ^T just switches
  back to history.
- ^P, ^N, ^R and ^S as move keys duplicate ↑/↓ and take up ctrl keys the author
  wants free for other uses.

## Goals

- ^R opens history from anywhere: the terminal, files mode or grep mode. Inside
  history, ^R flips between full history and suggested history. A flip keeps
  the query, resets the selection to the top, and clears semantic (tab)
  results. Coming back from files or grep returns to the last history view
  used.
- ^T opens files from anywhere and resets the filter to its default: files+dirs,
  or dirs when opened from `@` directory completion. Inside files, ^T cycles
  the filter files+dirs → dirs → files. ^F is removed.
- ^G only opens grep. Inside grep it does nothing.
- An empty suggested view (no service, or no suggestions here) still flips. It
  shows the status "no suggestions for this directory", and ^R flips back.
- Only ↑/↓ and PgUp/PgDn move the selection. The ^P, ^N, ^R and ^S move keys
  are removed.
- The hint rows show the new keys, including a ^R full/suggested hint in the
  compact inline picker.

## Non-Goals

- No change to terminal bindings, `smcli init --keys` names or the key config.
- No change to what full history shows: recent and native history, no
  suggestions mixed in, no ✨ marks.
- No change to semantic search: tab always searches full history.
- No new key bindings for ^P, ^N or ^S. They stay unbound.

## Open Questions

None. The decisions are recorded in the Goals and Non-Goals above.

## Technical Plan

### State

`Picker` (`crates/smcli/src/picker.rs:82`) keeps two history lists instead of one:

- `full`: recent history plus native history, deduped. This is the list the
  ^R picker builds today, with no suggestions in it.
- `suggested`: the suggestions only. This is the list `smcli pick --suggest`
  builds today.
- `suggested_view: bool`: which of the two lists History shows. It is set once
  when the picker opens (true for `--suggest`, false otherwise). After that only
  ^R inside History changes it, so leaving History and coming back keeps it.

There is no new `Mode` variant. `Mode` stays History / Files / Grep, so the
per-mode `saved` query map and the file fetch key are unchanged.

The history filter (picker.rs:478) reads the active list. `starred` (:392)
holds the suggestions, and marks show only while `suggested_view` is true.
The `filtered` cache key (:473) adds `suggested_view` so a flip rebuilds the list.

### Construction

`cli.rs:946-958` stops branching on `suggest`. It always passes suggestions,
recent and native history into `Picker::new`, then sets `suggested_view = suggest`. The
fourth argument (the start query) stays the existing expression at
cli.rs:956: Files gets `SMCLI_FILES_QUERY`, Grep gets an empty query and
History gets the command line. Both lists are fetched on every path today (cli.rs:929-940), so this
adds no service call.

### Keys

```
key   in History            in Files                  in Grep
^R    flip full/suggested   → History (last view)     → History (last view)
^T    → Files, kind reset   cycle files+dirs→dirs→files    → Files, kind reset
^G    → Grep                → Grep                    does nothing
↑↓    move                  move                      move
```

- Every mode switch saves the query for the mode being left and restores the
  saved query for the target mode, and resets the selection. That is today's
  behaviour, kept as it is.
- Leaving Grep clears `single_file_path` and `saved_file_scope`, the same as today.
- A ^R flip keeps the query, sets `selected = 0` and clears `semantic`, so
  the plain list for the new view shows.
- The ^P/^R move arm (:687), the ^N/^S move arm (:689) and the ^F arm (:723)
  are deleted.

### Labels and hints

- cli.rs keeps setting `title = "Suggested for {d}"` on every open, because
  only cli.rs knows the directory `d`. The History label and compact header show
  `title` (compact: "suggested @ <dir>") only while `suggested_view` is true,
  and "history" otherwise.
- Filter row (:589-599):
  - History: `^R full/suggested` in both full and compact layouts, in both
    views. The full layout keeps `tab semantic` in both views. Compact
    shows it above the list, the same place Files shows `tab into`.
  - Files: `^T dirs/files` replaces `^F dirs/files`.
- Global row (:604-608): `^R history` in Files and Grep, `^T files` outside
  Files, `^G grep` outside Grep.

## Risks

- The ^P/^N/^R/^S move keys are removed on purpose, so fzf and readline
  habits stop working inside the picker.
- test-file-picker.py, README.md and usage.txt must change in the same commit
  (Detailed Implementation steps 3-5).
- Every open now dedupes all lists, and the suggest path also reads the native
  history files. This is accepted: the full path already reads them, and the
  service lists are capped at 50.

## Alternatives Considered

- **One deduped list sliced at `suggestion_count`.** All three sources go into
  today's single `candidates` list. Suggested view shows the head, full view the
  rest. Rejected: either full view gains suggestions on top with ✨ marks, which
  changes today's full history, or commands that are both suggested and in
  history disappear from the full view.
- **Keep ^F for the file filter, ^T only switches modes.** This is the smallest
  diff. The four Python test waits (Detailed
  Implementation step 3) and the docs stay as they are. Rejected: the
  author wants each mode key to open its mode, then cycle that mode's options.
- **A fourth `Mode` (or a nested view enum) for suggested history.** Rejected:
  it splits the per-mode `saved` query map and the file fetch key
  (picker.rs:401), and every `Mode::History` check would need a second case. A
  bool on `Picker` does the same job.
- **Keep ^P/^N/^R/^S as move keys.** This keeps fzf and readline habits.
  Rejected: the author wants those keys free, and ↑/↓ already move.

## Detailed Implementation

1. `crates/smcli/src/picker.rs`: the key, state and hint changes.
   - `Picker` (:82-145): `candidates` and `suggestion_count` become `full: Vec<String>` and `suggested: Vec<String>`, plus `pub suggested_view: bool`.
     `Picker::new(suggestions, recent, native, query)` keeps its signature. It
     builds `suggested` from `suggestions`, and `full` from recent and native
     only, with the same `add` dedupe closure and a separate `seen` set per list.
     The copied loop at :119-131 goes away. Why: full view must equal today's ^R
     list, and a flip only has to change which list is read.
   - `starred` (:392) holds the `suggested` set. Rows get `*`/✨ only while
     `suggested_view` is true (picker.rs:561-567), so full view has no marks. The history filter (:478) reads `if suggested_view { &suggested } else { &full }`. The `filtered` key (:473)
     becomes `(query, semantic_active, suggested_view)`.
   - Remove the `'p' | 'r'` arm (:687) and the `'n' | 's'` arm (:689-691).
   - Split the shared `'t' | 'g'` arm (:711-722) into three arms with one shared
     helper, `switch_mode`: save the query, clear the grep scope when
     leaving Grep, set the mode, restore the query, `selected = 0`,
     `status.clear()`.
     - ^R: in History, flip `suggested_view`, `semantic = None` and `selected = 0`. Otherwise switch to History.
       If the active list is empty, set the status "no suggestions for this
       directory".
     - ^T: in Files, `kind = (kind + 1) % 3` and `selected = 0`. Otherwise
       switch to Files and set `kind = picker.kind`, the start kind that cli.rs
       already sets (:959-961) and the loop copies at :377.
     - ^G: if not in Grep, switch to Grep. In Grep, nothing.
   - Remove the ^F arm (:723-726).
   - Label (:494) and compact header (:510-512): in History, use `title` only
     while `suggested_view` is true, else "history".
   - Hints:
     - Filter row (:589-599): History gives `("^R", "full/suggested")` in
       compact and full layouts, with `tab semantic` kept in full layout in
       both views. Tab behaves the same in both views. Files gives
       `("^T", "dirs/files")`.
     - Global row (:604-608): `^T files` unless in Files, `^R history` unless in
       History, `^G grep` unless in Grep.
   - Tests in the `mod tests` block:
     - `Picker::new` keeps `full` identical to the old no-suggestion list, and
       `suggested` holds only the suggestions.
     - The Keys table, as a test of `switch_mode`, which is the only new
       function.
     - A flip keeps the query and resets the selection.
     - The ^T kind cycle wraps 0→1→2→0, and entering Files resets to
       `picker.kind`.
     - Rewrite the existing tests at :1111-1115 and :1143-1147 to read `full`
       (or `suggested` where they pass suggestions) instead of `candidates`.
2. `crates/smcli/src/cli.rs` (:946-962): always call `Picker::new(suggestions, recent, native, start_query)`, where
   `start_query` is the existing :956 expression. Then set
   `picker.suggested_view = suggest` and always set `title = "Suggested for {d}"`. `kind` at :959-961 is unchanged. Why: both lists already
   exist here, and the picker needs both to flip.
3. `scripts/test-file-picker.py` (:116, :137, :178, :194): change the waits from
   `"^F dirs/files"` to `"^T dirs/files"`. :115 (Ctrl-T from History to Files)
   still holds. Why: the hint text changes.
4. `README.md` (:73): "^T cycles files+dirs, folders and files; ^G searches file
   contents; ^R goes back to history". Add one line under the history picker
   section: ^R flips full/suggested, and only ↑/↓ move. Why: the documented keys
   change.
5. `crates/smcli/src/usage.txt`: at :24-25, Ctrl-F becomes Ctrl-T in files mode.
   At :26, "Ctrl-T and Ctrl-G switch to files and grep and back to history" becomes
   "Ctrl-T, Ctrl-G and Ctrl-R open files, grep and history; Ctrl-R in history
   flips between all history and this directory's suggestions". At :21, add "Ctrl-R in the picker flips to
   suggestions". Why: `smcli help` shows this text.
6. Not touched, checked: `shell_integration.rs` and `initialize.rs` (terminal
   keys and `--keys` names are unchanged), `scripts/test-shells.py:398` (^R
   typed at the prompt, not in the picker) and `scripts/test.sh`.
7. Verify: `make docs` after the README edit, `make test`, then `make fresh`
   for a manual try.
