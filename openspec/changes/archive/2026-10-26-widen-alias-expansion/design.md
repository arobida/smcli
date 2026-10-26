# Design

## Context

See proposal.md (Why). Two copies of the expansion gate exist:

- **Shell hooks:** `_smcli_safe` in the zsh, bash and fish hooks.
- **Rust:** `parse::safe_expansion`, applied by `history.rs`, `service.rs` and `cli.rs`.

zsh keeps an expansion only when preexec's `$3` equals `body + suffix` (`_smcli_alias`). That check stays.

## Goals / Non-Goals

**Goals:**

- Real-world aliases get an expansion.
- The picker shows the name the user knows.
- Alias recall is measured on many cases, not one.

**Non-Goals:**

- Resolving variables. A resolved form (`$EDITOR` → `fresh`) was built and measured: it moved MRR from 0.230 to 0.233 against 6,770 real history commands, so it was dropped.
- Shell functions, global or suffix aliases.
- Backfilling old rows.

## Decisions

1. **`parse::safe_expansion` is the authority.**
   - **Accepts:** printable ASCII, quotes, `$NAME`, `${NAME}`, `&&`, `||`, `|`, `;`, and a leading `~`.
   - **Rejects:**
     - `$(`, backticks, `<`, `>`
     - a lone `&`
     - `${NAME:…}` modifiers
     - any word matching `NAME=` (optionally quoted)
     - control characters
     - more than 8,192 bytes
2. **Hooks pre-check, `record` decides.**
   - The hooks reject control characters, substitution, redirection, assignments and oversize text, so a secret assignment never reaches `record`'s argv.
   - `smcli record` applies the full rule and drops an expansion that fails it. The run is kept typed-only rather than lost.
   - Mirroring the full rule in three shells would drift.
3. **No storage, protocol or index change.** The existing `expanded_command` column and protocol v2 carry the wider text.
4. **Picker:**
   - A row shows the typed alias, then `@` in the accent style, then the dimmed expansion truncated with `…`.
   - Insertion uses the typed text.
   - `primary()` stays the expansion for search and ranking.
5. **Evaluation (`rank_alias_eval`):**
   - Aliases and queries live in `alias-eval.tsv`: public oh-my-zsh git aliases, plus personal aliases with a neutral home.
   - The test ranks each alias typed-only and with its expansion, among the fixed competing commands and every other alias.
   - `SMCLI_ALIAS_EVAL_DISTRACTORS` adds a local, uncommitted export of real history.

## Risks / Trade-offs

- [Inserting the typed alias runs today's definition, not the one recorded] → This is intended. The user knows the alias, and the expansion is still shown.
- [Some expansions now carry paths or values written inline in the alias] → Capture filters run on the expansion, and assignments are refused before they leave the shell.
- [zsh reformats bodies containing `;`, so `$3` no longer matches] → Those runs stay typed-only, a known gap.
- [Most plain-language misses remain (72 of 156 against fixed commands)] → These are wording gaps the static model can't bridge. They're a separate change (alias descriptions or synonyms), measured with this evaluation.
