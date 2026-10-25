# Record-time alias investigation

## Summary

Record-time resolution is substantially cheaper and easier to make historically correct than query-time session snapshots. Persist only the expansion of the command actually captured, not the shell's alias table. The small ranking fixture favors indexing **both forms, with expansion-primary search semantics**. Recommend a nullable per-run `expanded_command` column, retain typed spelling, derive the alias marker from that column, and insert the captured expansion by default when the user picks an aliased row.

There are material contract changes: expansion is now durable private data, schema changes are required, and fish abbreviation spelling is already gone at preexec. **NO-GO for shipping under the current spec.** This is a promising follow-on design, not evidence to silently relax those requirements. Approval and implementation gates are listed at the end.

## Scope and reproduction

Prototype only, in this worktree. No production hooks, schema migrations, DTOs or picker were changed; no commits. The earlier query-time investigation remains intact. New artifacts:

- `scripts/spike-aliases-record-time.py`: resolve only the invoked alias, shell assertions and latency microbenchmarks; actual zsh preexec and fish interactive abbreviation checks.
- `crates/smcli-search/tests/ranking.rs`, `investigate_record_time_alias_documents`: temporary SQLite column, typed/expanded fixture storage, document-text ablations and capture-filter checks.

``` sh
python3 scripts/spike-aliases-record-time.py
make ranking
```

All shell definitions, commands and database contents are synthetic. Shell homes used for the interactive checks are temporary. No rc files or real native history are read. The Python script prints synthetic measurements, not user aliases.

## 1. Per-shell capture

### zsh

Use preexec `$1` as the typed/history text and `$3` as the shell-provided full executable text, before executing the command. Verified with an actual interactive zsh: typed `gst`, definition `printf status`, callback receives `HOOK=[gst]=>[printf status]`. The benchmark copies these arguments into pending capture state; it does not enumerate aliases or resolve the map a second time. Existing `_smcli_start` currently saves only `$1` (`shell_integration.rs`). Save both before dispatching the asynchronous recorder; never recompute at the next prompt.

`$3` is shell-rendered pre-execution text, not a promise of byte-preserving formatting or environment/variable/substitution expansion. Difference from `$1` alone is not reliable proof that an alias was involved: formatting may differ. Before setting an alias marker, confirm the supported leading alias case using only that name, or carry an explicit trusted capture classification. Reject unsupported pipelines, assignments, global/suffix alias semantics and compound commands for the initial version rather than falsely labeling them. The prototype copies a known alias callback; it does not implement this full classification. Never execute `$3` to discover its meaning.

### bash

Take the retained history/typed line and recognize only a simple unquoted first command word. On bash 4+, lookup `BASH_ALIASES[$first]` and append the untouched argument suffix. For bash 3.2, `alias -- "$first"` plus a conservative serialization parser works but needs a command-substitution subshell. No table walk and no eval. The prototype accepts single-space-separated ASCII words/flags and simple bodies, rejecting assignments, operators, quotes and substitutions. It deliberately returns unknown for syntax it cannot faithfully parse.

Resolution must happen before execution, after existing history/privacy exclusions, in the same capture state as the typed line. Current PS0/bash-preexec paths recover history at the following prompt; looking up there would be wrong if the command changes its own alias definitions. Wiring requires redesign of that timing, not adding a lookup to `_smcli_record`. DEBUG sees expanded execution fragments, not necessarily the full typed command; retain the existing history logic and record at most once. Bash alias expansion occurs at parse time: a later lookup is an approximation if traps mutate aliases. Escaped names, `command gst`, `sudo gst`, chained aliases, trailing-space alias expansion and complex multiline syntax are unknown in this prototype. Do not claim a single lookup is a general shell parser.

### fish

Abbreviations are expanded in the editable buffer before `fish_preexec`. Verified on a real PTY: typing `agst` for `abbr -a agst 'printf abbr-marker'` gives preexec `printf abbr-marker`, not `agst`. Record that actual buffer as an ordinary command. **Do not reverse-map it to an abbreviation or mark it aliased:** expansion may be ambiguous and the user may have edited it. To retain original keystrokes requires an earlier editor integration, which was not implemented and should not be smuggled into this feature.

For an alias-function, recognize the first word and call `functions` for **only that function**. Accept the generated single literal command forwarding `$argv`; reject custom bodies, branching, assignments and substitutions. Append the original suffix without evaluating it. This is function introspection, not function invocation. Default/autoloaded functions and arbitrary wrappers must remain unknown unless their exact safe form is established; do not scan all functions. The existing `fish_preexec` handler already receives the command buffer and honors private mode and `fish_should_add_to_history`.

For every shell: retain leading-space exclusion before trimming anything, cap payload bytes before transport, preserve unknown as a typed-only run, avoid recursive resolution, and do not treat the resolved text as verified equivalent executable behavior.

### Measured per-command resolution latency

Darwin arm64; bash 5.3.20, macOS bash 3.2.57, zsh 5.9, fish 4.9.3. Fifteen batches of 200 calls per shell/mode. P50 is the median batch average, p95 is nearest-rank p95 (maximum of 15 batch averages), **not** a per-event tail distribution. Modern bash/zsh clocks are internal `EPOCHREALTIME`; bash 3/fish use parent wall time and include amortized shell startup/setup. No network, SQLite write, recorder process or installed end-to-end hook is included.

| Shell    | Alias p50 / p95 µs | Plain command p50 / p95 µs | No-op loop p50 / p95 µs |
| -------- | ------------------ | -------------------------- | ----------------------- |
| bash 5.3 | 34.15 / 35.12      | 23.36 / 24.55              | 1.53 / 1.63             |
| bash 3.2 | 931.56 / 1095.32   | 932.56 / 989.65            | 21.27 / 23.66           |
| zsh      | 6.74 / 7.53        | 6.26 / 6.88                | 1.12 / 1.25             |
| fish     | 242.92 / 399.73    | 104.23 / 126.66            | 65.50 / 68.81           |

These are small compared with the previous snapshot p50 at 100 added aliases (bash 6.202 ms, zsh 3.637 ms, fish 25.848 ms), but the methods differ: this is per-invocation resolution, not full discovery, and zsh receives already-expanded arguments. Do not present the ratio as an end-to-end hook speedup. Bash 3 still pays about 1 ms per command, including misses. A negative function/alias lookup and noop loop were measured, not subtracted from published totals. No scaling claim for very large tables, very long commands or database writes was tested.

## 2. Storage and native history

`history.rs` currently has schema VERSION 7; `commands.command` is the typed text, and runs are UUID-deduplicated events. `NewRun` and `Run` carry only one text. `command_stats` groups by `c.command`; the search corpus and suggestion index likewise key by the command string. These must change deliberately, not just have another column added.

| Option                                                           | Benefits                                                                                                                                              | Costs / failures                                                                                                                                                                                  | Recommendation                         |
| ---------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------- |
| Nullable `commands.expanded_command TEXT`, next schema migration | One atomic row/event; historical meaning survives redefinition and index rebuild; ordinary queries can join typed and expansion without another table | Wider rows/WAL/backups; every reader/export/deletion/filter path must account for the field; version-7 binaries refuse a newer database                                                           | **Preferred**                          |
| Separate per-run table keyed by command row ID/UUID              | Sparse storage; can add provenance if truly needed                                                                                                    | Still a schema change; joins and atomic writes; soft deletion, prune/export/import and orphan cleanup become easier to get wrong; not a compatibility shortcut                                    | No benefit for one optional text field |
| Index-only expansion                                             | Avoids history.db migration                                                                                                                           | Lost on reindex, model/index switch, pruning or corruption; cannot reconstruct historical alias meaning; in-process/text fallback and history picker lose it; index still persists sensitive data | Reject as source of truth              |

The spike adds a nullable column only to a temporary test database, with no `user_version` change and no production SQL migration. It verifies multiple recorded `gl` expansions survive per-row storage. A real migration should add the column transactionally as schema 8 (if still the next free version), leave all existing rows NULL, and avoid alias backfill. Nonempty nonidentical, validated `expanded_command` implies `aliased`; a second redundant boolean is unnecessary unless unknown/rejected states need explicit differentiation. NULL means no trusted captured expansion, **not** proof the command never used an alias. An expansion rejected for privacy should not be represented as a valid empty expansion.

Per-run storage is essential: `gl` may mean log yesterday and ls-files today, including in conflicting sessions. Do not overwrite an old expansion with the latest one. Upgrade coordination matters: `history.rs` rejects databases whose `user_version` is newer than the binary supports; unlike a derived-index rebuild, rollback requires a compatible binary or a tested database rollback/backup procedure. Separate tables do not eliminate this rule.

Native history stays byte-for-byte read-only. Imported/native-only `gst` has no trusted historical expansion; leave expansion NULL. Neither a service nor a reindex should guess it from today's shell. A current-session lookup is acceptable only as an explicitly labeled, ephemeral “would expand now” preview, never persisted as historical truth or silently used to merge counts, rank or choose insertion. It reintroduces query-time context machinery and is not needed for this design. Recommend no such lookup initially. Previously captured rows with NULL have the same rule. Fish abbreviations already appear expanded in native history and cannot be reliably reverse-mapped.

## 3. Index, display and insertion

**Index both forms**, but use the expansion as primary semantic/program/subcommand/flag text when known; typed spelling remains an additional searchable surface. Preserve authored intent independently. Index identity must include both forms so alias metadata updates invalidate stale documents; bump the index format version and rebuild. Enforce the existing embedding cap across the combined representation. Do not let appended typed text displace the important expansion tokens, and do not reintroduce the long-command lexical truncation bug. Lexical indexing, coverage/rerank, strict-filter metadata, `is_smcli_command` and failure/answer classification must agree on which representation they examine. Text fallback must see the same fields as the indexed path.

Display `gst → git status` using escaped/sanitized presentation text, never by replacing the stored command with the label. Truncate for layout only, not insertion; escape terminal control characters. For commands with arguments, show the complete corresponding pair. Rows with unknown expansion show only typed text. One typed spelling with two historical expansions needs two distinguishable result identities, not an arbitrary latest expansion attached to combined usage.

**Default insertion: captured expansion**, not the arrow label and not an automatic execution. It remains editable in the command buffer. This works after `gst` is removed and avoids silently following a newly redefined `gst`. Preserve typed text for inspection/export and an eventual explicit “insert original” action if requested; no extra UI action was prototyped. Expanded text can still contain shell syntax/variables and is reinterpreted by the current shell (even `git` can now be an alias); it is not a replay sandbox or a guarantee of identical behavior. Initial support should be conservative simple expansions only. Keep ordinary/native commands inserting their original text.

Changing default insertion is an explicit spec change: the current requirements promise returned commands exactly as recorded. Keep DTO `command` as typed provenance and add a distinct `insert`/expanded field rather than silently changing the meaning of `command`. Picker display, selection IDs and command output need independent values.

## 4. Privacy and capture filters

**Yes, this leaks more than typed-only capture.** `deploy` reveals almost nothing; its expansion can reveal a credential, internal host/path or private arguments. This remains true although the user executed that command: native history may intentionally contain only the alias. SQLite, WAL, backups, JSON exports, logs, picker labels and search embeddings are additional disclosure surfaces. Not capturing unused definitions sharply reduces exposure relative to snapshots, but does not make used expansions harmless.

The existing `CaptureFilter` rejects leading space, known token shapes, user history regexes and cwd regexes (`config.rs`). Apply it independently to **typed and expanded** text with the same cwd before any durable write/index update. Respect native shell history/private-mode exclusions first. If either known form fails, reject the entire run, not just its expansion; otherwise an excluded secret action can still influence usage/transitions through its alias. Unknown resolution may remain typed-only, but a positively rejected known expansion must not be quietly downgraded to unknown. This policy needs shared validation used by CLI and service, and tests for both paths. Do not run filters only after persistence.

Verified with the actual Rust filter in the ranking fixture:

- typed `deploy` is accepted;
- `TOKEN=AKIAABCDEFGHIJKLMNOP ./deploy.sh` is rejected by default;
- the spec example `TOKEN=abc123 ./deploy.sh` **is accepted by the existing default filter**;
- bare `deploy abc123` is also accepted;
- leading-space expansion, configured `deploy` exclusion and configured private cwd are rejected.

Thus applying existing filters is mandatory but insufficient to guarantee secret-free expansions. They detect token shapes, not arbitrary secrets or generic `TOKEN=` assignments. The bash/fish shell prototype additionally rejects unsafe syntax, including the assignment-bearing spec example. The zsh prototype copies `$3` without a privacy classifier; it is not safe to persist directly. Production needs a consistent conservative safe-expansion gate before transmission plus filter checks at the receiving boundary. Length limits alone do not solve this. Arbitrary bare-word secrets remain a residual risk unless the supported expansion vocabulary excludes arbitrary operands, and identifiers themselves can be sensitive.

Source inspection found CLI `record` currently filters the typed line before sending, but service `record` directly calls `CapturedHistory::append`, whose validation is structural and does not apply `CaptureFilter`. Do not rely on the present client-only gate for this new persistent field. Add fail-closed config loading and shared typed/expanded filtering to service and local fallback, with no expansion in validation-error messages. Selection recording already filters picked command/query; it must also account for expanded insertion. Never place expansion bodies in `why` or diagnostics. Showing them in a dedicated alias label is a separate, intentional local disclosure, not permission to log them.

Before shipping, explicitly approve durable expansion capture in the spec/privacy contract (including embeddings and backups), or keep it opt-in under a revised product decision. Do not silently reuse the old “snapshots remain in memory” assurance. Deletion/pruning/export and re-filtering of previously captured data must handle expansion and derived indexes together; logical deletion does not promise secure erasure from SQLite WAL/backups.

## 5. Dedup and ranking identity

**Do not merge usage counts across alias and direct spellings initially.** One run remains one UUID/event, with typed and expansion metadata. Aggregate variants by `(typed_command, expanded_command)` (including NULL) so conflicting historical `gl` meanings cannot mix. This requires changing the current command-string-keyed aggregators/DTO/picker identities. Avoid indexing each form as a separate run: that doubles frequency. Multiple textual surfaces should point at one variant result identity.

Direct `git status` and captured `gst → git status` can both match. They may be visually grouped by equal insertion text later, but display group frequency must be computed from distinct run IDs, not copied into each child; separate typed provenance and unknown native counts remain intact. Native/captured overlap already uses a conservative max for frecency in the corpus; do not sum imported native runs into alias-expanded counts and count the same executions twice. Do not normalize quoting, reorder flags or infer function equivalence as part of this feature.

The SQL fixture records three `gl → git log --oneline -20` runs and a fourth `gl → git ls-files` in another session. It asserts two expansion variants and four total `gl` events. Existing typed-only `command_stats` would combine all four; the prototype intentionally does not claim to have fixed production aggregation. New ranking benchmarks must cover actual competing alias/direct variants, repeated usage, failures, transitions and picks before deciding on count sharing.

## 6. Ranking fixture ablations

`make ranking` passes three tests, including the previous query-time spike. Original cohort unchanged: search n=16 and suggest n=18 each MRR/top-1/top-3 1.000. Production ranking was untouched; this does not establish a held-out floor for enabling the proposed feature.

New isolated cohort: four aliases (`gst`, `gl`, `gd`, `gst --short`), two ordinary distractors/guards and one literal unknown old entry, seeded at fixed times. Eight queries; same usage and signals for each document-text mode; no query expansion and no live-session snapshot. The test reads expansion pairs back from its temporary SQLite column.

| Query / expected typed row           | Typed only rank | Expanded only rank | Both, typed primary | Both, expansion primary |
| ------------------------------------ | --------------- | ------------------ | ------------------- | ----------------------- |
| `gst` / `gst`                        | 1               | 2                  | 1                   | 1                       |
| `gl` / `gl`                          | 1               | 1                  | 1                   | 1                       |
| `git status` / `gst`                 | 2               | 1                  | 2                   | 1                       |
| `git log --oneline` / `gl`           | 2               | 1                  | 2                   | 1                       |
| `git diff --stat` / `gd`             | 5               | 1                  | 2                   | 1                       |
| `git status --short` / `gst --short` | 2               | 1                  | 2                   | 1                       |
| `docker ps` / `docker ps`            | 1               | 1                  | 1                   | 1                       |
| `oldalias` / `oldalias`              | 1               | 1                  | 1                   | 1                       |

| Mode (n=8)              | MRR    | Top-1       | Top-3       | ΔMRR vs typed |
| ----------------------- | ------ | ----------- | ----------- | ------------- |
| Typed only, off         | 0.7125 | 4/8 (50%)   | 7/8 (87.5%) | 0             |
| Expanded only           | 0.9375 | 7/8 (87.5%) | 8/8 (100%)  | +0.2250       |
| Both, typed primary     | 0.7500 | 4/8 (50%)   | 8/8 (100%)  | +0.0375       |
| Both, expansion primary | 1.0000 | 8/8 (100%)  | 8/8 (100%)  | +0.2875       |

Expansion-primary both gains +50 percentage points top-1 and +12.5 points top-3. Expanded-only loses exact `gst` top-1: keep the typed surface. Adding expansion merely as secondary text improves recall but not precision because program metadata and coverage still favor typed-primary semantics.

**Prototype limitation:** the existing `intent` field is a test-only bridge for the secondary search surface. Expansion-primary changes the fixture's `entry.command` for search, adds typed text as intent, and maps hits back to the typed row. Real code must not overwrite authored intent or command provenance; it needs explicit search/display/insert representations. The ranking ablation has no duplicate canonical expansion collisions; production variant-key handling, strict program/flag filters, native merge, index-lock fallback and competing direct-command usage are not validated by this table. The unknown old entry is a captured literal surrogate, not a native-file integration test. No model tuning or held-out validation was done. A test asserts 8/8 top-1 for the selected expansion-primary fixture, not a universal claim.

Record-time capture also does **not** satisfy the old query-time requirement: if no captured `gst` expansion exists, typing today's live `gst` does not magically find plain historical `git status`. That is an intentional scope trade-off and must be stated in the revised spec.

## 7. Protocol, fixtures and in-process fallback

No protocol was changed in this spike. A production implementation needs:

- Versioned record field `expandedCommand` (distinct from `path`, `command`, and author `intent`) and an unambiguous hook-to-CLI optional argument/flag. Keep existing 5/8/9-field record forms working; do not let a new positional field shift the current tail calculation silently.
- Extend `NewRun`/`Run`, SQL insert/read/export/import/prune, aggregate state and live service incremental updates. Event retry must preserve one row and the original metadata; reused UUID with different payload must not overwrite earlier expansion.
- Search/suggestion/picker results need nullable expansion, distinct stable variant identity, and explicit insertion text or a clearly defined consumer rule. `recent: Vec<String>` and the picker's current string command lists cannot display two `gl` meanings safely without DTO changes. The arrow is presentation, never executable content.
- Bump protocol VERSION from 1 when shipping, and add frozen old/new record and result fixtures: absent/null expansion, ordinary alias, unsafe expansion rejection, duplicate event, conflicting variants, empty/oversized fields, malformed payloads and sanitized display. Unknown fields ignored by an old same-version service would silently lose metadata, so additive serde fields alone are insufficient; version mismatch must reach the local fallback.
- Shared validation/filtering and identical DB writes on service and CLI fallback. Stop/restart old service and migrate safely. If service ack is lost, fallback must reuse the event ID and expansion rather than create another run. An actual database write error should remain an error, not a false success with typed-only metadata.
- Both indexed search and text fallback must build the same two-surface representation and return the same variant ordering. Reading captured expansion from history.db means no alias state is needed in either process. No fff dependency or service-only evidence is introduced. A local record must invalidate/refresh service state through the existing data-version mechanism, including expansion-only updates.
- A future `SMCLI_RANKING_OFF=alias` should suppress expansion search effects, not erase historical metadata. Specify whether display/insertion stays stable while ranking is off; recommend yes. Ablation must rebuild/switch the document representation, including embedding/index identity, rather than merely disabling a rerank bonus while expanded terms remain searchable.

## Decision and required spec/task changes

**NO-GO for shipping under the current contract.** The measured approach is worth a revised-spec implementation, but not automatic approval: persistence contradicts the current privacy/non-schema-change design, fish cannot retain abbreviation keystrokes at this hook, and end-to-end identity/filter/fallback behavior is still only a design. The latency and 8-case accuracy gains justify revisiting these constraints, not skipping them.

Required changes before implementation (recommendations only; this spike does not rewrite approved specs/tasks):

1. Revise proposal/design to permit a per-run nullable expansion column, durable expanded search documents and a coordinated schema/protocol/index migration. Replace session snapshots/query expansion with invocation-local capture, and define bounded supported shell syntax. Document rollback incompatibility and privacy disclosure.
2. Replace alias-query scenarios with recorded dual-surface scenarios. Define unknown/native-only behavior, conflicting historical expansions, exact typed provenance, expansion-primary search, separate variant counts, arrow display and expanded default insertion. Explicitly accept fish abbreviations as already-expanded ordinary buffer commands unless a separate editor-capture task is approved.
3. Amend privacy requirements to permit only explicitly approved captured expansions, never alias-table snapshots. Require the existing filters on both texts, conservative pre-send syntax checks, receiver-side revalidation, all-or-nothing rejection for a known filtered expansion, no bodies in logs/why, and deletion/export/index lifecycle coverage. Decide on informed opt-in versus an intentionally changed default; arbitrary-secret detection remains unsolved.
4. Add implementation tasks for hook timing in every bash mode, bounded fish function inspection and zsh classification; version-8 migration and historical variant aggregation; DTO/record syntax and frozen fixtures; display/insert separation; indexed/text fallback parity and alias ablation; privacy and event-retry tests.
5. Add adversarial and frozen held-out cases for alias/direct count competition, alias redefinition/removal across sessions, native-only unknowns, secret-bearing expansions, strict filters, quoted/complex syntax and identical fallback ordering. Measure installed per-command hook latency plus SQLite/index size/sync overhead. Then run `make test`, `make ranking`, `make test-shells` and Linux checks; only after approval use `make fresh` for a user-testable build.

Checks completed here: shell microbench/assertions (including bash 3.2), actual zsh preexec, actual fish abbreviation PTY behavior, `make ranking` (3 passed), `make docs` and `git diff --check`. No production migration/build, full suite, Linux gate, installed-hook latency, backup/rollback exercise or native-file byte-equality test was run. **Final decision: NO-GO until the spec/privacy contract is revised and those implementation gates pass.**
