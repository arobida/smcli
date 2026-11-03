# Natural Language Command Help (`@@`)

- **Author:** Andrew Robida
- **Date:** 2026-11-03

## Overview

People forget how to run a command and open a browser to look it up. This feature answers that question in the terminal. The user types a program name followed by `@@`, for example `bun @@`, and presses Tab. smcli's picker opens. The user types a plain-English question such as "how do I install a package globally?" and gets a ranked list of example commands. The chosen command replaces `@@` on the prompt, with its placeholders still unfilled, and is never run automatically.

It works offline and uses no language model. smcli embeds a small set of (description, command template) entries for that program with the Potion model it already ships, and ranks them by how close each one is to the question. It is aimed at commands the user has never run. Commands they have run before are already covered by history search.

## Analysis

Six reviewers looked at the idea. These are their main points, most serious first.

**Blockers**

1. **`@@` already opens the file picker.** Every shell hook treats any word starting with `@` as a file query, so today `@@` searches files for "@". Each shell has more than one place where this happens:

   - bash: `_smcli_tab` and a second completion path (`crates/smcli/src/shell_integration.rs` around lines 392 and 417);
   - zsh: around line 618;
   - fish: around line 835.

   The new check must come first at every one of those places. Only the bare word `@@` should trigger it. Words like `@@x` must keep today's behavior. The edit-paths flow also writes `@` placeholders into the line, so a path containing `@@` must not trip the trigger. New hooks paired with an old binary must leave the line unchanged.

2. **Data licenses are unverified, and the notices script doesn't cover data.**

   - tldr pages are believed to be CC BY 4.0, which requires attribution.
   - ezlo's code is Apache-2.0, but its spec files may be licensed differently, and its format is still v0.x.
   - `scripts/gen-notices.py` only lists Cargo dependencies. Any bundled dataset needs a manifest recording its source URL, commit and license, plus a build check that fails when a dataset has no notice entry.

3. **The delay on Tab hasn't been measured.** Loading Potion reads about 33 MB of weights, then runs a warmup encode (`crates/smcli-search/src/potion.rs:57-67`). When the service isn't running, the CLI pays that cost on every press. Measure it cold and warm before deciding the fallback. A suggested budget is under 100 ms through the service and under 300 ms without it.

**Major points**

- **Too many sources.** Five of six reviewers say four sources is too many for v1, and most recommend tldr only. smcli has no parser for any of these formats today, so each source is new code.

  - Running `prog --help` executes an unknown binary, which can hang or have side effects, and its output makes poor templates. It should be cut.
  - ezlo is small (46 specs) and its format may change.
  - navi is in maintenance mode.

- **Placeholder syntax differs by source.** tldr uses `{{x}}`, navi uses `<x>` (which the shell reads as a redirect), and man pages use `FILE`. Convert all of them to the picker's existing `@` placeholder convention (`crates/smcli/src/picker.rs:1038`), so the edit flow can move between fields. Write the conversion as a pure function with a test.

- **Index strategy.** There are two options:

  - Embed one program's 5 to 50 entries on each call, with nothing saved. This is simple and doesn't depend on which model is active.
  - Pre-embed at build time. This is faster, but the stored vectors break when `smcli init --potion` switches models.

  Either way, this feature uses no TalaDB index and has no interaction with `smcli indexes --prune`.

- **Protocol change.** Running this through the service means a new protocol version (v4), a `HelpSearch` operation, frozen fixtures, and a test that the service path and the in-process path return the same results. Reviewers suggest staying in-process for v1 and adding the operation only if the measured load time requires it. If the service does run it, it must reuse its loaded model, because AGENTS.md forbids holding two models at once.

- **Ranking quality is unproven.** Potion was tuned on shell commands, not English questions. Before committing to sources, build a separate evaluation set of about 50 questions across 5 programs, with targets for top-3 accuracy and mean reciprocal rank (MRR).

- **Crate placement:**

  - source parsers in `smcli-core`;
  - embedding and ranking in `smcli-search`, with no dependency on fff;
  - the trigger check in `shell_integration.rs`;
  - results shown as rows in the existing `picker.rs`, with no new widget.

- **Shipping the data.** Today only the model folder ships with the binaries. The data needs either a folder beside `potion-shell-16m/`, with a fallback to the checkout and removal on uninstall, or embedding in the binary with `include_str!`. It also needs a defined format (for example versioned JSONL with fields `{program, description, template, source}`), a size cap, and a choice of which tldr platforms to include.

- **Edge cases to specify:**

  - `@@` with no program name before it;
  - "program not covered" and "no matches" shown as different messages;
  - Esc restores the line;
  - subcommand context: in `git rebase @@`, the earlier words should narrow the results and not be repeated on insert;
  - `sudo bun` and aliases;
  - never auto-select a result;
  - each row shows its source.

**Minor points**

- Mention `@@` in the `@` picker's footer and in `--help`.
- Add a debug flag that prints which sources loaded, the entry counts, whether it ran in the service or in-process, timings and scores.
- Never write to local navi files.

## Goals vs. Non-Goals

- **In-Scope Goals:**
  - the `bun @@` + Tab trigger in bash, zsh and fish;
  - the question typed in the picker;
  - ranked example commands with placeholders, inserted on the prompt and never run;
  - fully offline, no LLM;
  - tldr data bundled with license attribution.
- **Out-of-Scope Non-Goals:**
  - generating new commands;
  - running `--help` on unknown binaries;
  - filling placeholders with live values;
  - a service protocol change before latency is measured;
  - any network fetch when the trigger fires.

## Alternatives Weighed

- **Embedding `--help` or man output only:** returns flag lines, not full commands, and runs unknown binaries. Rejected as a primary source.
- **A local or remote LLM to compose commands:** rejected by the offline, no-LLM constraint.
- **carapace as a source:** structured flag specs for many tools, but no examples, and a large external Go binary. Dropped by the user.
- **navi as a bundled source:** maintenance mode, and its cheats are spread across many repos. At most an optional reader for local `.cheat` files.
- **ezlo as a v1 source:** the best data shape (titled recipes and described flags), but only 46 specs and an unstable v0.x format. Deferred.
- **Trigger `?` / `??`:** a glob character; in bash, pressing Enter expands it to two-character filenames.
- **Trigger `!`, `#`, `$` (single or doubled):** history expansion, comment start, and PID expansion respectively. Each one can run something the user didn't intend.
- **Trigger `++`:** safe in all three shells. `@@` was chosen to match the existing `@` file trigger.
- **Typing the question on the command line:** Tab only sees the last word, and apostrophes open shell quotes. The question goes in the picker.
- **A subcommand (`smcli how bun "..."`) or a configurable hotkey (like `up_key`):** raised by a reviewer as simpler. Not chosen yet.

## Key Decisions & Staging for Next Steps

Settled:

- The trigger is the bare word `@@` followed by Tab. It is checked before every `@` branch in all shells.
- The question is typed in the picker.
- Offline, no LLM, Potion only.
- The index is held in memory.
- The command is inserted, never run.
- v1 source is tldr only. ezlo, navi and man/`--help` are deferred.
- Load time is not a blocker. It is still measured once.
- Each tldr `{{name}}` placeholder becomes a bare `@`, so `tar -xf {{archive}} -C {{dir}}` is inserted as `tar -xf @ -C @`. The bare `@` is the picker's existing placeholder (`crates/smcli/src/picker.rs`, `is_placeholder`): the edit flow numbers the fields, and Tab on each one opens the file picker. A named `@archive` would not work, because the picker reads `@word` as text, such as npm's `@scope/pkg`. The picker row still shows the original template with its names.

Still open:

- **D4 v1 sources:** tldr only (reviewer consensus), or tldr plus ezlo.
- **D5 service operation:** none in v1 unless measured load time calls for it.
- **D2 index:** embed on each call or pre-embed.
- **D3 bundle:** format, location and size cap.
- **D6 placeholder mapping:** to the `@` convention.
- **D8 budgets:** a latency budget and ranking targets.

Do first:

- Check the tldr and ezlo data licenses.
- Measure Potion load time cold and warm.
- Build the question-ranking evaluation set.
