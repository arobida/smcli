# Design

## Context

Today `tldr.rs` owns everything: `load` (platform filter), `installed` (PATH stat over every tldr program), `corpus_vectors` (append-only `.vec` cache), ranking. `pick_tldr` in `cli.rs` wires it straight to the picker.

## Goals / Non-Goals

**Goals:** smcli-owned program set (OS-shipped ∪ history-seen, ∩ PATH); a small source trait in `smcli-core`; tldr ported to it; persistent index with incremental add.

**Non-Goals:** implementing sources beyond tldr in this change; network fetch at query time (a network source like cheat.sh must cache offline, later change); man/`--help` parsing; Linux OS-shipped detection beyond a best-effort list.

## Decisions

- **Trait in `smcli-core`**, implementations elsewhere: `CommandSource { name(), version(), examples(program, platform) -> Vec<CommandExample> }`, `CommandExample { program, description, template, source }`. Core stays model- and fff-free; tldr impl stays in `smcli-search` where its data loader lives. Alternative (trait in `smcli-search`) couples sources to the search crate for no gain.
- **Program enumeration owned by smcli, not sources.** Sources only answer by name; smcli lists OS dirs and history. Keeps cheat.sh-style sources (no program list) workable.
- **OS-shipped = fixed system dirs.** macOS: `/bin /sbin /usr/bin /usr/sbin` (sealed system volume, so contents are Apple's). Linux: same dirs; shortcut: includes distro-package tools, refine with package-manager ownership if it matters.
- **Storage: TalaDB index beside the history index**, named from model identity + sorted `name@version` of sources. Reuses `smcli indexes` listing/prune.
- **Service owns discovery** in `record`: after the history write, check `command_key` first word against in-memory indexed + known-empty sets; on miss, PATH-stat (request's `shell_path_snapshot`) and query sources off the reply path. CLI falls back in-process.
- **Sync, not per-program add.** The service computes the wanted set (OS-shipped ∪ history-seen, ∩ PATH, with examples), diffs it against the index, adds and deletes. Triggered on record when the program is unknown or the request's PATH differs from the last synced PATH; runs on a background thread, one at a time. `@@` only reads (one `all()` read).
- **PATH flapping:** syncing against each shell's PATH can drop a program another shell still has. Accepted: the next record from that shell resyncs it.

## Risks / Trade-offs

- [~1000 binaries in system dirs, most without examples] → enumeration is a dir read; only programs with examples get embedded.
- [Ranking mixes sources with different styles] → out of scope; per-source weighting is a later knob.
- [Protocol change] → version bump + frozen fixture; old CLIs fall back.

## Migration Plan

First `@@` after upgrade builds the index; old `tldr-*.vec` files listed by `smcli indexes`, removed by `--prune`.

## Open Questions

- Which sources follow tldr, and their data/licensing (record in `THIRD_PARTY_NOTICES` via `make notices`) — each is its own change.
