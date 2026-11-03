# Natural Language Command Help (`@@`)

- **Author:** Andrew Robida
- **Status:** Proposed
- **Date:** 2026-11-03

------------------------------------------------------------------------

## Overview

When a user forgets how to do something with a command, they usually open a browser and search. This feature answers the question in the terminal instead.

The user types a program name followed by `@@`, for example `bun @@`, and presses Tab. smcli's existing picker opens, and the user types a plain-English question such as "how do I install a package globally?". smcli ranks example commands from bundled tldr pages by how close their descriptions are to the question. It uses the Potion embedding model that smcli already ships, works offline, and uses no language model.

The chosen command replaces `@@` on the prompt. Each tldr placeholder becomes smcli's bare `@` placeholder: `bun add --global {{package}}` is inserted as `bun add --global @`. The existing edit flow then lets the user fill each field. smcli never runs the command.

A bare `@@`, with no program name before it, searches every bundled tldr page. Each result is labeled with its program.

## The Problem

smcli's history search only finds commands the user has already run on this machine. A command they have never run, or ran on another machine, isn't in history. Today the only way to find it is to leave the terminal: open a browser, search, then copy the result back and edit it.

tldr pages already pair short English descriptions with example commands for thousands of tools. Potion can compare a question to those descriptions in a few milliseconds per entry. Putting the two together inside smcli's picker answers "how do I…" questions without leaving the shell, using the same Tab-and-pick motion users already know from `@` file completion.

## Goals

- `<program> @@` followed by Tab opens the picker in bash, zsh and fish, with results limited to that program's tldr page.
- A bare `@@` followed by Tab searches the bundled tldr pages of every program installed on PATH, and each row shows its program.
- Text after `@@` on the line, as in `bun @@ install globally`, is used as the picker's starting query.
- For each program, the expected answer appears in the top 3 results for at least 80% of a fixed set of evaluation questions. This target is provisional and gets fixed once the evaluation set exists.
- The selected command is inserted, never run, with every `{{name}}` turned into a bare `@`.
- Everything works offline. No network access happens when the user presses Tab.
- The existing `@` file completion behaves exactly as it does today for every word except the exact word `@@`.
- The picker opens within a latency budget to be measured in the Technical Plan, both with and without the service running.
- Bundled tldr data carries its license attribution in `THIRD_PARTY_NOTICES.md`.

## Non-Goals

- Generating commands that aren't in tldr. There is no LLM, local or remote.
- ezlo, navi, carapace, or man and `--help` as sources in v1.
- Running `<program> --help`, or any unknown binary.
- Filling placeholders with live values, such as a branch list for `@`.
- A new service protocol operation in v1.
- Downloading or updating tldr pages at runtime.
- Questions typed on the command line itself. The question is only typed in the picker.

## Open Questions

- **tldr license**: Believed to be CC BY 4.0. It must be confirmed from the tldr repository before bundling.
- **tldr platforms**: Which page sets to bundle (`common`, `osx`, `linux`), and whether to filter them by the current OS.

## Critical User Journeys (CUJs)

### CUJ 1: Program help

- **Given**: smcli hooks are loaded in bash, zsh or fish, and `bun` has a bundled tldr page.
- **When**: The user types `bun @@`, presses Tab, types "install a package globally", and presses Enter on the top row.
- **Then**: The prompt line becomes `bun add --global @` with the cursor on the `@`. Nothing runs. The picker rows showed each tldr description with its original template.

### CUJ 1b: Prefilled question

- **Given**: The same setup as CUJ 1.
- **When**: The user types `bun @@ install a package globally` and presses Tab.
- **Then**: The picker opens with that query and ranked results already showing. Enter inserts `bun add --global @`, which replaces the question text.

### CUJ 2: Bare `@@`

- **Given**: No program is typed. The prompt holds only `@@`.
- **When**: The user presses Tab and types "extract a tar archive".
- **Then**: Rows come from every bundled page, each labeled with its program (for example `tar`). The first bare `@@` after an install or a model switch shows "indexing tldr…" while the vector cache is built. Later calls read the cache.

### CUJ 3: Program not covered

- **Given**: `mytool` has no tldr page.
- **When**: The user types `mytool @@` and presses Tab.
- **Then**: The picker says "no tldr page for mytool" and offers the whole-corpus search with the same query. Esc restores `mytool @@` unchanged.

### CUJ 4: Existing `@` completion is unchanged

- **Given**: The user types `cat @src`, or `npm i @scope/pkg`, or `@@x`.
- **When**: The user presses Tab.
- **Then**: The behavior matches today's file completion exactly. Only the whole word `@@` triggers command help.

## Technical Plan (Architecture)

### Data flow

``` text
shell hook (bash/zsh/fish)
  word == "@@" exactly?  ──no──> existing @ file completion (unchanged)
        │yes
        ▼
smcli pick --tldr   (SMCLI_LBUFFER = text before @@)
        │
        ├─ program = first word of LBUFFER after sudo/env/command, or none
        ├─ load tldr entries        smcli-search::tldr  (bundled JSONL)
        ├─ load Potion              configured potionPath, else bundled
        ├─ vectors:
        │    program given → embed that page's ~5-15 entries now (no cache)
        │    bare @@       → read the corpus vector cache, building it on a miss
        ▼
picker (Mode::Tldr): user types question → embed query → cosine rank → rows
        │ Enter
        ▼
template with {{x}} → @   →  hook replaces the line, cursor on the first @
```

### Components

- **Bundled data.** `scripts/gen-tldr.py` converts one pinned tldr commit into `tldr/tldr.jsonl`. Each line is `{"program", "platform", "description", "template"}`, one per example, and the file carries a header line with the tldr commit. It ships as a `tldr/` folder beside the binaries and `potion-shell-16m/`, found the same way the model is (`Potion::bundled_dir`): beside the executable first, then the checkout for dev builds and tests. Only the `common` pages and the current OS's pages (`osx` or `linux`) are loaded at runtime. Generation happens offline at build time and is never done by smcli.

- **Entries and placeholders** (`smcli-search`, new `tldr.rs`). These are pure functions:

  - parse the JSONL;
  - filter it by program and platform;
  - convert every `{{…}}` in a template to a bare `@`.

  The conversion also handles tldr's `{{[-g|--global]}}` long and short option form by keeping the long option, and it is covered by unit tests.

- **Ranking.** The query and each entry's description are embedded with `Potion::embed`, which returns normalized vectors. Results are sorted by dot product. There is no BM25 and no TalaDB. On a tie, the entry that appears earlier in its page ranks higher.

- **Vector cache** (bare `@@` only). This is the decision taken in the brainstorm: cache on first use, keyed by model identity.

  - **Location:** the cache sits in the same directory as the search indexes.
  - **File name:** `tldr-<tldrcommit>-<Potion::NAME>-<identity>-d<dimension>.vec`, following the `index_name` pattern (`crates/smcli-search/src/index.rs:116`).
  - **Contents:** one header line, then the raw little-endian `f32` vectors in JSONL order.
  - **Writes:** the file is written to a temporary name, then renamed, so a crash never leaves a partial cache.
  - **Invalidation:** a model switch or a new tldr commit changes the file name. The old file then becomes "other" data for `smcli indexes --prune`, which is extended to list and delete `tldr-*.vec` alongside `.taladb`.
  - **Program queries skip the cache:** a single page is small enough to embed directly.
  - **Installed programs only:** the bare-`@@` corpus contains only pages whose program resolves on the shell's current PATH, using the existing PATH snapshot (`shell_path_snapshot` in `smcli-core`). That cuts the corpus from thousands of pages to the tools the user actually has.
    - The cache header records the set of installed programs.
    - When a program is added, only the new pages are embedded and appended. The update is still written to a temporary file and renamed into place.
    - When a program is removed, its rows are skipped on read.
    - Commands that only exist as aliases or functions don't count as installed.

- **Picker.** A new `Mode::Tldr` in `picker.rs` reuses the existing list, query box and keys. Each row shows the description dimmed, the template in bold, and the program name when the search is bare. Selecting a row returns `Choice::Command` with `edit_paths: false`, so the file picker never opens on its own. A tldr `@` is often not a file, as in `bun add --global @`.

- **CLI.** `smcli pick --tldr` conflicts with `--files`, `--grep` and `--suggest`. It reads the program from `SMCLI_LBUFFER`, as the other pick modes read context. The output contract is the same as `pick`: the chosen line on stdout, nothing on Esc.

- **Shell hooks** (`shell_integration.rs`). In bash, zsh and fish, every place that tests for a word starting with `@` first tests whether the word is exactly `@@`. When it is, the hook calls `smcli pick --tldr`. These are the places:

  - bash `_smcli_tab`, and bash's second completion path;
  - zsh's completion widget;
  - fish's `@*` match.

  On a result, the hook replaces the whole line from the program onward with the template, keeping a leading `sudo` or `env`, and puts the cursor on the first `@`. When there's no result, or when an old binary doesn't know `--tldr` and exits with an error, the line is left unchanged.

- **In-process only.** There is no new service operation and no `VERSION` bump. The CLI loads Potion itself. If measured cold latency misses the budget, a service operation is added in a later design.

### Prefilled query

Text typed after `@@` on the same line becomes the picker's starting query, so the picker opens with results already ranked. For example, `bun @@ install globally` + Tab opens the picker with "install globally" already searched.

- **Trigger rule:** the hook reads the raw line (`READLINE_LINE`, `BUFFER`, `commandline`), not shell words. Command help triggers when the cursor sits after a whole word `@@` that is preceded by a space or the start of the line. The program is the text before `@@`, and the query is the text after it.
- **Behavior on Enter instead of Tab:** the program receives a literal `@@`, followed by the words of the question. In zsh, a `?` in the question gives a "no matches found" error, and an apostrophe opens a quote continuation. Both are harmless, and neither runs the intended command.
- **On insert:** the template replaces both `@@` and the prefilled text.

### Subcommand context

When there are words between the program and `@@`, as in `git rebase @@`, they are not used as a filter. Instead, each entry whose template starts with those words gets a fixed score bonus. Insertion replaces the whole line, so the words are never duplicated.

### Latency budget (to be measured)

- **`<program> @@`, cold, in-process:** the picker is visible in under 300 ms. Most of that time is loading Potion.
- **Bare `@@` with a warm cache:** under 300 ms.
- **Bare `@@` while the cache is being built:** no budget. The "indexing tldr…" status shows while it builds.

Both numbers are checked once with a timing test, `make baseline`, before release.

## Alternatives Considered

### Alternative A: ezlo specs as a v1 source

- **Pros**: The best data shape: titled recipes with full templates and typed placeholders, plus described flags. Apache-2.0.
- **Cons**: Only 46 spec files. The format is v0.x and its author says it can change with no migration path. The spec files' data license is unconfirmed.
- **Reason for Rejection**: Too little coverage to justify a second parser in v1. It's the first candidate for a later source.

### Alternative B: navi cheatsheets

- **Pros**: Same description-plus-command shape, and `<placeholder>` syntax.
- **Cons**: navi is in maintenance mode and points users to ezlo. Community cheats are scattered across separate git repos. A literal `<x>` is a shell redirect.
- **Reason for Rejection**: There is no single bundleable corpus. Reading the user's local `.cheat` files is deferred.

### Alternative C: Pre-embed vectors at build time

- **Pros**: Fast from the first query, with no runtime cache file.
- **Cons**: Adds tens of MB to the bundle and a build step. The vectors don't match after `smcli init --potion` switches models, so a runtime fallback is needed anyway.
- **Reason for Rejection**: It needs the cache path regardless, so the shipped vectors add size without removing code.

### Alternative D: Placeholder handling on insert

- **Strip all placeholders**: `tar -xf {{archive}} -C {{dir}}` becomes `tar -xf -C`, which reorders the arguments wrongly.
- **Cut at the first placeholder**: safe, but it loses the rest of the template.
- **Turn the rest into a `#` comment**: zsh has interactive comments off by default, so the rest is passed as arguments.
- **Named `@name` placeholders**: the picker reads `@word` as text, as in npm's `@scope/pkg`, not as a placeholder.
- **Reason for Rejection**: A bare `@` keeps the whole template, is inert if run by mistake, and uses the existing placeholder handling.

### Alternative E: A `#` comment as the trigger

- **Pros**: `bun #how do I install globally?` reads naturally, and it acts as a comment in bash and fish.
- **Cons**: zsh has `interactivecomments` off by default, so pressing Enter instead of Tab passes every word to the program as an argument. Writing a real comment and pressing Tab would also open the help picker unexpectedly.
- **Reason for Rejection**: `@@` with prefilled text gives the same "type the question once" flow, and pressing Enter by mistake does nothing harmful.

### Alternative F: A subcommand or configurable hotkey instead of `@@`

- **Pros**: No new token grammar in the shell hooks. A key could follow the `up_key` pattern.
- **Cons**: It doesn't match the Tab-and-pick motion users know from `@`. A hotkey is one more binding to discover and configure.
- **Reason for Rejection**: The owner chose `@@` + Tab.

------------------------------------------------------------------------

## Non-Functional Requirements

- **Security**: No binary is run to gather data. tldr data is read-only and bundled. Templates are inserted on the prompt and never run. The cache file is written only under smcli's index directory, using write-to-temp-then-rename.
- **Performance**: The latency budget is in the Technical Plan. The bundled `tldr.jsonl` must stay under 10 MB. The cache only holds vectors for installed programs.
- **Rollout and rollback**: There is no protocol or schema change. Rolling back means installing the older binary. The old hooks never send `@@`, and the new hooks paired with an old binary leave the line unchanged. A stale `tldr-*.vec` file is harmless and is removed by `smcli indexes --prune`.

------------------------------------------------------------------------

## Detailed Implementation Plan

### Target Files to Create/Modify

#### 1. `scripts/gen-tldr.py` (Create)

- **Type / Scope**: Build-time data generator, run by hand or by `make tldr`. It is not part of `make release`.
- **Proposed Changes**:
  - It takes a path to a local tldr-pages checkout at a pinned commit. It never downloads anything.
  - It reads the English pages under `pages/common`, `pages/osx` and `pages/linux`.
  - For each example, it writes one JSON line: `{"program", "platform", "description", "template"}`.
  - The template is the raw example command, keeping the `{{…}}` placeholders.
  - The first line is a header: `{"version": 1, "tldr_commit": "<sha>", "license": "CC-BY-4.0"}`.
  - It fails if the output is larger than 10 MB.

#### 2. `Data/tldr/tldr.jsonl` and `Data/tldr/LICENSE.md` (Create)

- **Type / Scope**: Checked-in bundled data, alongside `Models/`.
- **Proposed Changes**: These are the generated JSONL and the copy of tldr's license with its attribution text. They're regenerated only when the pinned tldr commit changes.

#### 3. `crates/smcli-search/src/tldr.rs` (Create)

- **Type / Scope**: Rust library module.
- **Proposed Changes**:
  - **Pure functions:**
    - `bundled_dir() -> Option<PathBuf>` uses the same lookup order as `Potion::bundled_dir`: beside the executable (`tldr/`), then the checkout's `Data/tldr`.
    - `load(dir, platform) -> Result<Corpus>` keeps only `common` plus the current OS.
    - `Corpus { commit, entries: Vec<Entry> }`, where `Entry { program, description, template }`.
    - `to_placeholders(template) -> String` turns `{{…}}` into a bare `@`. For `{{[-g|--global]}}`, it keeps the long option.
    - `installed(programs, path) -> HashSet<String>` checks each program for an executable file in the PATH directories.
  - **Ranking:** `rank(potion, query, entries, prefix_words) -> Vec<(usize, f32)>` scores each entry by dot product with the query vector. Entries whose template starts with the words typed before `@@` get a fixed bonus of 0.05. Ties keep page order.
  - **Cache:** `corpus_vectors(potion, corpus, installed, index_dir) -> Result<Vec<Vec<f32>>>` reads, appends to or builds `tldr-<commit>-<NAME>-<identity>-d<dim>.vec`. The header lists the programs it covers. Writes go to a temporary file, then a rename.

#### 4. `crates/smcli-search/src/lib.rs` (Modify)

- **Type / Scope**: Module exports.
- **Proposed Changes**: Add `pub mod tldr;` and re-export `Corpus`, `Entry`, `rank` and `corpus_vectors`.

#### 5. `crates/smcli/src/cli.rs` (Modify)

- **Type / Scope**: CLI.
- **Proposed Changes**:
  - **New flag:** `Pick` gains `--tldr`, which conflicts with `--suggest`, `--files` and `--grep`.
  - **Inputs:** the program comes from `SMCLI_LBUFFER` as its first word, after skipping a leading `sudo`, `env` or `command`. Words between the program and `@@` are the prefix words. The starting query comes from `SMCLI_TLDR_QUERY`, the text after `@@`.
  - **Model:** Potion loads from the configured `potionPath`, or the bundled model otherwise, following the existing search setup.
  - **Program with a page:** embed that page's entries.
  - **Program with no page:** show the status "no tldr page for X; Ctrl-A searches all installed".
  - **Bare `@@`:** load the cached vectors.
  - **Selection:** the line printed on stdout is the leading `sudo`/`env`, if any, plus `to_placeholders(template)`. Esc prints nothing.
  - **`indexes`:** list `tldr-*.vec` files next to the `.taladb` files, mark the current one, and delete the others with `--prune`.

#### 6. `crates/smcli/src/picker.rs` (Modify)

- **Type / Scope**: TUI.
- **Proposed Changes**:
  - **New mode:** add `Mode::Tldr`, whose `as_str` is "tldr".
  - **Rows:** each row shows the description dimmed, then the template in bold, with the program name in front when the search is bare.
  - **Query:** the query box starts with the prefilled text. Each keystroke re-ranks through a callback, with the vectors already in memory.
  - **Status line:** shows "indexing tldr…" while the cache builds.
  - **Ctrl-A:** switches between this program's page and all installed programs.
  - **Selection:** returns `Choice::Command` with `edit_paths: false`.
  - **Untouched:** the `Files` and `History` behavior stays the same.

#### 7. `crates/smcli/src/shell_integration.rs` (Modify)

- **Type / Scope**: Generated shell hooks for bash, zsh and fish.
- **Proposed Changes**:
  - **Where the check goes:** at each `@` entry point, a new check runs first. This covers bash `_smcli_tab` and its second completion path, the zsh completion widget, and fish's `@*` branch.
  - **The check:** split the raw line before the cursor at the last whole word `@@`. That word must be preceded by a space or the start of the line, and followed by a space or the cursor.
  - **On a match:** call `smcli pick --tldr` with `SMCLI_LBUFFER` set to the text before `@@` and `SMCLI_TLDR_QUERY` set to the text after it.
  - **On output:** replace the line from the program's start through the cursor, and move the cursor to the first bare `@`. In bash, the cursor is `READLINE_POINT`. In zsh, it's `CURSOR`. In fish, it's `commandline -C`.
  - **On empty output or an error:** leave the line unchanged.

#### 8. `crates/smcli/src/usage.txt` and `crates/smcli/src/README.md` (Modify)

- **Type / Scope**: User help and crate docs.
- **Proposed Changes**: Document `<program> @@ [question]` + Tab, a bare `@@`, and `pick --tldr`. Note that `indexes` now lists the tldr cache. Edit the README through iwe and run `make docs`.

#### 9. `crates/smcli-search/README.md` (Modify)

- **Type / Scope**: Crate docs.
- **Proposed Changes**: Describe the `tldr` module, the cache file format and its naming. Edit through iwe and run `make docs`.

#### 10. `Makefile` (Modify)

- **Type / Scope**: Build.
- **Proposed Changes**:
  - **`release`:** also copies `Data/tldr` to `$(DIST)/tldr`, next to the model copy (line 22).
  - **New `tldr` target:** runs `scripts/gen-tldr.py TLDR=<checkout>`.
  - **New `tldr-ranking` target:** runs the tldr evaluation test.

#### 11. `scripts/install.sh` and `scripts/uninstall.sh` (Modify)

- **Type / Scope**: Install and uninstall.
- **Proposed Changes**:
  - **Install:** stage and move `tldr/` beside `potion-shell-16m/` (lines 57-81), and fail if `tldr.jsonl` is missing or empty.
  - **Linux message:** the manual-install message (line 37) also names `tldr/`.
  - **Uninstall:** removes `$prefix/tldr` (near line 171).

#### 12. `scripts/gen-notices.py` and `THIRD_PARTY_NOTICES.md` (Modify)

- **Type / Scope**: License notices.
- **Proposed Changes**:
  - The script appends a "Bundled data" section built from `Data/tldr/tldr.jsonl`'s header and `Data/tldr/LICENSE.md`: name, commit, license and attribution.
  - It fails if a folder under `Data/` has no `LICENSE.md`.
  - Regenerate the notices with `make notices`. Never edit `THIRD_PARTY_NOTICES.md` by hand.

#### 13. `crates/smcli-search/tests/tldr.rs` and `crates/smcli-search/tests/fixtures/tldr-eval.tsv` (Create)

- **Type / Scope**: Unit tests and the ranking evaluation.
- **Proposed Changes**:
  - **Unit tests:**
    - `to_placeholders`: single, multiple, `{{[-g|--global]}}` and nested braces;
    - JSONL loading and platform filtering;
    - the `installed` lookup against a temporary PATH;
    - cache build, append and reread;
    - a different model identity giving a different file name.
  - **Evaluation:** about 50 questions across 5 programs (`bun`, `git`, `tar`, `docker`, `find`), each paired with the expected template. The test reports top-3 accuracy and MRR, and fails below the Goals target.

#### 14. `scripts/test-shells.py` (Modify)

- **Type / Scope**: Black-box shell hook tests.

- **Proposed Changes**: Run these cases in bash, zsh and fish:

  - `bun @@` + Tab;
  - `bun @@ install globally` + Tab, with the cursor at the end;
  - a bare `@@` + Tab;
  - `@@x`, `cat @src` and `npm i @scope/pkg` + Tab, which must behave as before;
  - Esc restores the line;
  - a fake binary without `--tldr` leaves the line unchanged.

  These tests use a stub picker through the existing test hooks, honoring `SMCLI_TEST_ONLY`.

#### 15. `scripts/test-file-picker.py` (Modify)

- **Type / Scope**: Picker tests.
- **Proposed Changes**: Add a `pick --tldr` scenario against the bundled data. It types a question, checks the top row and the inserted line `bun add --global @`, and checks the "no tldr page" status.

### Verification Plan

- **Compilation/Build**: `make build`, then `make fresh`.
- **Unit Tests**: `make test-rust PKG=smcli-search FILTER=tldr` and `make test-rust PKG=smcli`.
- **Slices**: `make test-shells`, `make test-picker` and `make test-install`, then the full `make test`.
- **Ranking**: `make tldr-ranking` meets the Goals target. `make ranking` is unchanged, which shows history ranking isn't affected.
- **Latency**: `make baseline` with a `pick --tldr` case, cold and warm, against the Technical Plan budget.
- **Docs/Notices**: `make docs` and `make notices`, with no unexpected diffs.
- **Manual**: CUJs 1 through 4 in each shell, using a `make fresh` build.

------------------------------------------------------------------------

*Drafted with EGM*
