# Picker restyle plan (Mistral-retro, theme-driven)

Target: the mock (rounded search box, rounded list card, yellow selection bar, zsh-highlighted
commands, muted hint row). Style guide: the Mistral ratatui notes — ANSI-16 only so the user's
terminal theme paints, hierarchy via BOLD/DIM, ROUNDED borders, 2-cell padding, one loud element
(the selection).

Scope: `crates/smcli/src/picker.rs` draw closure (~L203-243) and `menu.rs` (L142, L158) so both
TUIs match. No behavior/key changes. Skipped on purpose: sunset stripe, hero line, preview pane,
animation, 80-col cap — not in the mock.

## 1. Theme consts (top of picker.rs, `pub(crate)` so menu.rs reuses them)

| Role      | Style                                   | Used for                      |
|-----------|-----------------------------------------|-------------------------------|
| MUTED     | `Reset` + DIM                           | borders, placeholder, hints   |
| HEADING   | `Reset` + BOLD                          | command word                  |
| ACCENT    | `Magenta`                               | title line, `›`               |
| SELECTED  | `bg Yellow, fg Black` + BOLD            | selected row                  |

No `Color::White`/`DarkGray` left anywhere (today's highlight breaks on light themes).

## 2. Layout (top → bottom)

1. Title line, ACCENT: `Suggested for <cwd> ›` in history mode (existing `picker.title`),
   else current `label`. Indent 1.
2. Search box, 3 rows: `Block::bordered().border_set(ROUNDED).border_style(MUTED)`,
   content `⌕ ` + query, or MUTED placeholder `type to search history…` (per mode) when empty.
   Set the real cursor with `frame.set_cursor_position` after the query.
3. Results card, fill: same rounded MUTED block. Status/summary text (today row 2) moves
   to the block's `title_bottom`, right-aligned, MUTED — keeps the count without a row.
   Add `Scrollbar` (MUTED) on the right edge, inside the card.
4. Hint row, MUTED, 1 blank row above where height allows (≥ 12 rows).

Graceful shrink: if `area.height < 8`, drop borders (current flat layout) so tiny panes still work.

## 3. Rows

- Marker: `★` for starred, space otherwise (unchanged), MUTED; on selected row unstyled so it
  inherits SELECTED.
- `highlight_symbol` → none; selection is the full-width yellow bar (`HighlightSpacing::Always`
  with `" "` keeps columns stable).
- History command text goes through `highlight(cmd) -> Line` (section 4).
- Files/grep rows: path in default fg, `:line` MUTED, provenance MUTED, git marker ACCENT.

## 4. Zsh syntax highlighting (`fn highlight(&str) -> Vec<Span>` in picker.rs)

Small hand lexer over the already-`visible()` string; no new deps. ShellTokenizer
(smcli-search) is for indexing and drops quoting, so don't reuse it.

| Token                                 | Style                    |
|---------------------------------------|--------------------------|
| command word (first, or after `\| && \|\| ; $(`) | BOLD           |
| subcommand (first bare word after a command, alnum/`-`, no `/`) | Cyan |
| flag `-x` / `--long`                  | Yellow                   |
| `'…'` / `"…"` strings                 | Green                    |
| `$VAR`, `${…}`                        | Magenta                  |
| `$(` `)` `\|` `&&` `\|\|` `;` `>` `<` | Red                      |
| path (contains `/` or starts `~`)     | Blue + UNDERLINED        |
| `#…` comment                          | DIM + ITALIC             |
| rest                                  | default                  |

On the selected row only DIM/BOLD/UNDERLINED survive (patch fg to unset) so text stays
readable on yellow. Unterminated quotes: style to end of line, never panic. Lexer must
round-trip — concatenated span text == input (unit test asserts this plus 4 cases:
`git status --short`, `eval $(x shell-init zsh) -- /p history`, `printf '%s\n' "$HOME" | rg --files`,
`semble search "a b" | jq`).

## 5. Hints (context-aware, replace the static string)

- Empty query: `↑↓ move · enter select · type to search · tab semantic · ^T files · ^G grep · esc cancel`
- Typing: drop `type to search`; lead with `enter select · tab semantic`.
- No matches: `^U clear · esc cancel` (only if ^U exists; else just `esc cancel`).
- Files/grep: keep today's mode-specific text.
Separators ` · ` MUTED, key names Reset (not dim) so keys read before labels.

## 6. Verify

- `cargo test -p smcli` (new lexer test), `make test` — `scripts/test-file-picker.py` greps
  screen text; update its expected strings (`❯`, hint text) as they change.
- `make fresh`, try in a light (Tinacious light) and a dark Ghostty theme, plus an 80×12 pane.
- `graphify update .`

Order: 1 → 2 → 3 → 5 → 4 (highlighting last; it's the only real logic).
