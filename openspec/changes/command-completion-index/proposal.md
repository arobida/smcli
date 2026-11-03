# Proposal

## Why

`@@` command help is hard-wired to tldr: the corpus, the program list and the vector cache all live in `smcli-search/src/tldr.rs`, and every Tab rescans tldr's whole program list against `PATH`. smcli should own which programs it knows about (the OS's own CLI tools, plus what the user actually runs) and treat tldr as one interchangeable source of examples among several.

## What Changes

- New command-completion index, separate from the history search index, holding examples and vectors per program.
- **Program set** decided by smcli, not by a source:
  - programs that ship with the OS and resolve on the user's `PATH` (macOS: the system volume dirs `/bin`, `/sbin`, `/usr/bin`, `/usr/sbin`, `/usr/libexec` is excluded);
  - plus programs the user runs (seen in captured history) that resolve on `PATH`.
- **Source abstraction in `smcli-core`**: a command-example source supplies examples for a program by name. tldr becomes the first implementation; others (cheat.sh and further sources) plug in later without touching index or picker code.
- Programs with no examples from any source stay out of the index.
- `@@` reads from the index; index named by source versions + model identity; `smcli indexes [--prune]` covers it.

## Capabilities

### New Capabilities

- `command-completion-index`: which programs are indexed, how sources contribute examples, how history grows the index, and how `@@` reads it.

### Modified Capabilities

(none)

## Impact

- `smcli-core`: new source trait + example type (no model or fff dependency).
- `smcli-search/src/tldr.rs`: becomes a source implementation; `corpus_vectors` `.vec` cache replaced.
- `smcli/src/cli.rs` (`pick_tldr` → index-backed), `smcli-service/src/service.rs` (`record` discovery).
- Possible protocol bump + fixture in `crates/smcli-core/tests/fixtures/service_protocol`.
- Old `tldr-*.vec` caches orphaned; prune handles them.
