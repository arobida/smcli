# Design

## Context

`picker.rs` keeps `global_files`: false lists `picker.local_files` (one level of cwd, filtered in the picker), true sends `files`/`grep` requests whose `directory` the service snaps to the repository root (`FileSearch::root`). Grep is disabled in "global" mode. Each fff root is one pooled instance (`capacity`, idle eviction).

## Goals / Non-Goals

**Goals:** one scope variable driving every file/grep request; Tab/Shift+Tab navigation; both cwd and scope visible.

**Non-Goals:** searching outside `~`; a scope history/stack beyond "parent of current"; persisting scope across picker sessions; changing `@` completion (it keeps project-root behavior).

## Decisions

- **Scope is a directory path held by the picker.** Initial value is the project root (computed the same way as `FileSearch::root`). Requests carry it as `directory`; a new `scope: true` flag tells the service to use it verbatim as the fff root instead of snapping to the repo root. Alternative: always snap and filter by prefix — rejected, it can't widen above the repo.
- **Narrowing reuses the parent's instance when possible.** Drilling into a subdirectory of an already-indexed root filters that root's results by path prefix instead of creating a new fff instance; widening above an indexed root creates (or reuses) an instance for the new root. Keeps `~` scans to when the user actually asks for them.
- **Single-file grep** is a grep request with a `path` field; the service filters grep hits to that relative path (fff's grep over its index, restricted). In-process fallback reads the one file directly.
- **Shift+Tab out of single-file grep** returns to file mode at the saved scope rather than the file's parent — matches "leaving goes back".
- **Shift+Tab** remaps from nothing (`BackTab`), so no conflict; `Tab` in History mode keeps its semantic-search meaning.
- **Inserts**: `relative_insert` computes from the cwd, falling back to `~/…` then absolute.
- **Protocol**: optional `scope` and `path` fields, protocol version bump, new frozen fixtures; old clients unaffected, mismatch falls back in-process.

## Risks / Trade-offs

- [Indexing `~` is slow and large] → only on explicit Shift+Tab to `~`; show the existing partial/scanning status; pool eviction frees it.
- [Content indexing of `~` memory] → create home-or-above-repo instances with content indexing off; grep at wide scopes stays available but runs unindexed/partial with the existing "partial results" status.
- [Losing `^A` muscle memory] → hint line shows `tab`/`⇧tab` in its place.
