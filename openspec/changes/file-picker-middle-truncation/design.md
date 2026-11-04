# Design

## Context

File and grep rows are built in `picker.rs` from spans: optional git mark, the path (`visible(name)`, styled by LS_COLORS from the real name), `:line  text` in grep mode, then provenance or git status. Nothing measures the path; the terminal cuts the line at the edge. `history_row` already cuts an alias expansion by display width (`Span::width`), which is the pattern to follow.

## Goals / Non-Goals

**Goals:** file name always visible; display width correct for wide characters; no change to what is inserted.

**Non-Goals:** shortening directory names to initials (`s/b/i/h/user_profile.go`), configurable truncation, changing history or tldr rows.

## Decisions

**One helper, `truncate_middle(path, room)`.** Keep the last component; fill the remaining room from the start of the path; join with `…`. Measured with display width, char by char, like `history_row`. If the last component alone exceeds `room`, end-cut it with `…`. Alternative: cut at directory boundaries only; rejected because it wastes room on narrow terminals.

**Room.** Row inner width minus the cursor, git mark and trailing provenance/status spans (already computed for the row). In grep mode the path gets at most half the room so `:line text` stays readable; the text keeps the terminal's end cut.

**Styling.** LS_COLORS style is still chosen from the full name, so directories and executables keep their colour when shortened.

## Risks / Trade-offs

- [Two rows can look identical after shortening when only the middle differs] → the leading part and file name usually differ; the selected row's full path is what gets inserted, so nothing is lost. Accepted.
- [Grep's half-row cap hides more path than needed for short match text] → simple and predictable; tune only if it bothers in use.
