# Tasks

## 1. Expansion gate

- [x] 1.1 Widen `parse::safe_expansion` (variables, quotes, `&&`, `||`, `|`, `;`, leading `~`; reject substitution, redirection, `&`, assignments, control characters). Verify with the `parse.rs` gate test covering the `mkal` body.
- [x] 1.2 Replace `_smcli_safe` in the zsh, bash and fish hooks with the 8,192-byte pre-check, and widen the fish alias-function body match. Verify with `make test-shells` cases for `mkal`, operators, quotes, redirects, substitution, assignments and a 130-byte body in every shell.
- [x] 1.3 Make `smcli record` drop an expansion that fails the full rule instead of failing the run. Verify that `make test` passes.

## 2. Picker

- [x] 2.1 Render alias rows as typed, then `@`, then the dimmed truncated expansion, and insert the typed alias. Verify with the `picker.rs` unit tests and `scripts/test-alias-picker.py`.
- [x] 2.2 Update the `usage.txt` help text and verify it with `smcli --help`.

## 3. Evaluation

- [x] 3.1 Add `rank_alias_eval` with `alias-eval.tsv` (77 aliases, 156 queries) and `alias-eval-distractors.txt`. Verify that `make ranking` prints typed-only versus expanded scores and asserts expanded wins.
- [x] 3.2 Run it against a local export of real history via `SMCLI_ALIAS_EVAL_DISTRACTORS` (result: typed-only MRR 0.019, expanded 0.230).

## 4. Integration

- [x] 4.1 Run `make linux` and `make fresh`. In a shell using the new hooks, run `mkal`, confirm `history.db` stores its expansion as written, and confirm that the picker shows `mkal@ …` and inserts `mkal`. Done: `make fresh` built; a real zsh session with the user's alias file stored `mkal`, `zshconfig`, `cpd` and `cplc` as written; `smcli search "edit my zsh aliases"` returned `mkal` first. `make linux`: bash and zsh pass, and fish times out setting up its prompt on Linux fish 4.0.2, identically on unchanged `main` (an existing harness issue).

## Workflow follow-up

- Archive `improve-search-accuracy` before this change.
