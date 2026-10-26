# Proposal

## Why

Aliases with any real logic are never expanded. For example, `mkal` is `$EDITOR $ZDOTDIR/conf.d/50-aliases.zsh && source "$ZDOTDIR/.zshrc"`. The current expansion gate accepts only plain words, so every `mkal` run is stored typed-only and semantic search sees just `mkal`. Measured against real history, an alias's name alone is almost never found by a plain-language query.

## What Changes

- Widen the record-time expansion gate. It accepts `$VAR`, `${VAR}`, single and double quotes, `&&`, `||`, `|`, `;` and a leading `~`. It still rejects command or process substitution, redirection, backgrounding and variable assignments, and capture filters still run on both forms.
- Store the alias body exactly as written. Variables are never read or resolved.
- `smcli record` drops an expansion that fails the full rule and records the run typed-only, instead of failing the run.
- **BREAKING (picker behavior):** an aliased entry shows the typed alias, then an accent `@`, then the expansion dimmed and truncated to fit. Picking it inserts the typed alias, not the expansion.
- Add an alias ranking evaluation: 77 aliases and 156 plain-language queries, run against fixed competing commands. Optionally it can also run against a local export of real history.

No schema or protocol change: the existing `expanded_command` column and protocol v2 carry the wider expansions.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `shell-context-signals`: record-time alias expansion accepts more syntax. The picker displays and inserts the typed alias.

## Impact

- **Shell hooks** (`crates/smcli/src/shell_integration.rs`): `_smcli_safe` pre-check for zsh, bash and fish, raised to 8,192 bytes. Fish alias function bodies are accepted beyond plain words.
- **Expansion rule and recording:** `smcli-core` `parse::safe_expansion`, and `smcli` `cli.rs` `record`.
- **Picker:** `picker.rs` row rendering and insertion. The `usage.txt` help text.
- **Tests:**
  - `crates/smcli-search/tests/ranking.rs` (`rank_alias_eval`) and its fixtures `alias-eval.tsv` and `alias-eval-distractors.txt`
  - `scripts/test-shells.py`
  - `scripts/test-alias-picker.py`
- **Ordering:** depends on `improve-search-accuracy`, which defines `shell-context-signals`. Archive it first.
