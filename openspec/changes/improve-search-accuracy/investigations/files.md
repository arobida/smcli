# File presence and fff

## Decision

- **stat: GO.**
- **fff: NO-GO.**

Ship a small positive-only boost from in-process `stat` of path arguments. Do not consult fff during search, warm or cold.

## Method

This is a spike. It is not wired into `rerank`, the service protocol, or `SMCLI_RANKING_OFF`. The CLI and smcli-core still do not link fff: `cargo tree -p smcli -i fff-search` and the same query for smcli-core match no package. smcli-service does.

The stat prototype is `crates/smcli-search/tests/file_presence.rs`. Readers are `cat`, `bat`, `less`, `more`, `head`, `tail`, `wc`, `file`, `vim`, `nvim`, `vi`, `nano`, `emacs`, `hx`, `code`, `open`, `source`, and `.`. Output commands (`mkdir`, `rmdir`, `touch`, `rm`, `mv`, `cp`, `tee`, `ln`, `install`) get no bonus, so a missing output path cannot demote. `CommandBreakdown` refuses pipes, redirects, and substitutions. Tokens containing `$ * ? ~ ! [ ] { } < > | & ; \` are not stat'd, so globs, variables, and tildes are not expanded. Relative paths join the current cwd. Absolute paths are checked as written. The pass is capped at 40 commands and 4 paths. A `metadata` error is neutral. The bonus is `+0.08` on the base score. Absence adds nothing.

The fff prototype is `crates/smcli-service/src/file_presence_spike.rs`. If the service pool already has that root, it calls `FilePicker::get_file_by_path`. If not, it returns without calling `FileSearch::index`. Pinned fff is rev `95fd777`.

Machine: Apple M1 Pro, macOS 27.0.1, local APFS temp dirs. Stat pool latency is release (`cargo test --release`; `make test-rust` is debug). fff latency is the debug test profile. Percentile is nearest-rank, index `ceil(p * n) - 1`.

## Stat ablation

Flip cases list the existing file second, so the signal-off order keeps the missing file first. Guard cases must not move.

| Cohort | n   | off MRR | on MRR | delta  | off top-1 | on top-1 | off top-3 | on top-3 |
| ------ | --- | ------- | ------ | ------ | --------- | -------- | --------- | -------- |
| flip   | 9   | 0.500   | 1.000  | +0.500 | 0/9       | 9/9      | 9/9       | 9/9      |
| guard  | 14  | 1.000   | 1.000  | +0.000 | 14/14     | 14/14    | 14/14     | 14/14    |
| all    | 23  | 0.804   | 1.000  | +0.196 | 14/23     | 23/23    | 23/23     | 23/23    |

Flip covers two cwd trees (`notes.md` in one, `todo.md` in the other), rename/delete (`old.md` gone, `new.md` present), double and single quotes (`my notes.md`), `head -n 20`, an absolute path, `../notes.md`, and a 1.05 versus 1.00 close tie.

Guards that stayed put: both files deleted, `*.md`, `$HOME`, `~`, `$(echo ...)`, a pipe, `mkdir`, `touch`, `rm`, a redirect, frecency 15, a command used here, a 1.20 versus 1.00 gap, and the 41st command past the pool cap (bonus 0, not stat'd).

`+0.08` does not beat the existing usage score for frecency 15 (about 1.42) or a command used here (about 2.1). It does beat a 0.05 gap. That matches the spec: equal scores may flip, a habit must not.

## Stat latency

Release, 40 commands, 200 iterations. Each command stats one path. A cache would only make repeated paths cheaper.

| Run | hits p50 | hits p95 | miss p50 | miss p95 | mixed p50 | mixed p95 |
| --- | -------- | -------- | -------- | -------- | --------- | --------- |
| A   | 85.9 us  | 121.2 us | 71.4 us  | 81.3 us  | 77.2 us   | 225.7 us  |
| B   | 87.8 us  | 123.9 us | 73.6 us  | 91.2 us  | 77.9 us   | 112.0 us  |

Worst observed p95 is 226 us. Debug was in the same band (mixed p95 214 to 233 us). That is fine on a keystroke, on local disk.

## fff lookup

`get_file_by_path` is a non-blocking in-memory lookup: strip the index root, binary-search directories, binary-search files. On non-Windows a path outside the root returns none without `canonicalize`. It does not `stat`. Tombstoned rows are still returned; the spike treats `is_deleted` as absent. `handle_create_or_modify` does `stat` and must not be used on this path.

`FileSearch::index` is the cold path. A missing root builds a picker and may wait 10 seconds for the scan and 10 seconds for the watcher. The spike does not call it. A lookup before any index left `instances()` empty.

| Lookup                        | n   | p50    | p95    |
| ----------------------------- | --- | ------ | ------ |
| warm hit, 2000-file index     | 300 | 4.5 us | 4.6 us |
| warm miss, same index         | 300 | 4.3 us | 4.5 us |
| no warm index, does not build | 300 | 0.5 us | 0.6 us |
| stat hit, same paths          | 300 | 1.6 us | 3.8 us |
| stat miss, same paths         | 300 | 0.9 us | 1.2 us |

Cold `index()`:

| Tree                  | n   | p50              | p95      |
| --------------------- | --- | ---------------- | -------- |
| 2 files               | 9   | 12.61 ms         | 22.87 ms |
| fixture with a commit | 1   | 24.6 ms          |          |
| 2000 files            | 1   | 68.7 ms          |          |
| API cap               |     | up to about 20 s |          |

Warm lookup is cheap. Cold is not something search may start. Most history searches have no warm picker for that cwd, so the fff signal is absent exactly when a boost would be useful.

## fff versus stat

Same fixture ideas as the stat cohort: a git repo, `notes.md`, `old.md` deleted after the index, `new.md` created after the index, quoted `my notes.md`, gitignored `secret.log`, directory `src`, `modified.md` dirty before the scan, and `/etc/hosts` outside the root. Search must not wait for the watcher.

| Check                          | stat             | warm fff                                    | rank                                                              |
| ------------------------------ | ---------------- | ------------------------------------------- | ----------------------------------------------------------------- |
| notes versus missing todo      | present          | present                                     | agree                                                             |
| quoted `my notes.md`           | present          | present, git `clean`                        | agree                                                             |
| rename, immediate              | new exists       | old still live, git `clean`, not tombstoned | disagree: fff boosts the deleted file                             |
| rename, after 2 s              | new exists       | filtered lookup agreed                      | old row still returned (`caught_up=false`)                        |
| `secret.log` versus missing    | present          | absent (gitignore)                          | stat can flip a second-listed hit; fff cannot                     |
| `src` versus missing           | directory exists | absent (files only)                         | same miss                                                         |
| path outside the root          | present          | absent                                      | same miss                                                         |
| notes and modified, both exist | tie              | presence tie                                | a dirty boost would pick modified; the spec does not ask for that |
| notes edited after index       | present          | git status still `clean`                    | a dirty boost misses the edit                                     |

Post-wait winner labels differed on 2 of 4 pairs (gitignore and directory). The immediate rename is a third, and it inverts rank instead of merely failing to boost.

Git status does not fix existence. `format_git_status(None)` is `clean`, so a missing status is not neutral. At index time the dirty file was `modified`, so a scan can fill it. That is a different signal, stale on the next edit, and it would split service ranking from the CLI fallback, which cannot link fff.

fff instead of stat is worse on this fixture. fff on top of stat adds no presence win.

## Privacy

`stat` reads metadata only. It does not read file contents, expand shell syntax, or write the path into the history database, logs, or embeddings. A missing or failed lookup does not lower rank. fff would not add a new store either; this spike only reads a picker file search already owns. Neither signal reads rc files. Privacy is not the reason fff is rejected.

## Left for group 7

Not done here. For the stat GO: add `SMCLI_RANKING_OFF=files`, apply the same bonus in `rerank` before truncation, and use it on the CLI fallback. Do not call `FileSearch::index` from search. Skip the boost when `stat` can block, such as a dead network mount. This spike measured local APFS only. Do not ship fff evidence.

Interpreters and git pathspecs are not in the reader set. Add them only if real misses show up after stat ships.
