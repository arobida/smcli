# Backend gate 1: baseline and fff verification

Status: **baseline and standalone C API integration complete on macOS arm64**.
No service, shell-adapter rewrite, directory-context cache, new ranking algorithm,
or user-facing file/grep mode is implemented by this change.

## Reproduce

From the repository root:

```sh
make baseline
make fff-smoke
make test
```

`make baseline` builds the release CLI and writes raw samples, medians, nearest-rank
p95s, high-water RSS, binary SHA-256, toolchain and host metadata to
`dist/baseline.json`. It never imports or reads the user's real history. To do a
short run or retain a separate comparison:

```sh
make baseline BENCH_ARGS='--rows 1000 --samples 3 --output dist/baseline-quick.json'
```

Python 3.9+, Bash and Zsh are used for the Unix benchmark. Shells are discovered on
`PATH`; absent shells are omitted from the report. `make test` also runs a tiny
10-row/one-sample benchmark as a functional harness check, without performance
thresholds. fff is optional and is not fetched by `make build`, `make release`,
`make fresh`, or `make test`.

## Current capture → SQLite → suggestions → picker

1. [Generated shell hooks](../Sources/smcli/ShellIntegration.swift#L7) keep a
   pending command, starting cwd, session and start time. At the next prompt (or
   exit), they invoke a new `smcli record` process. Prompt recording is detached
   by default; `SMCLI_SYNC=1` and shell exit wait for it. Bash uses DEBUG,
   PROMPT_COMMAND and EXIT; Zsh uses preexec and wrapped precmd/zshexit.
2. [`record` dispatch](../Sources/smcli/smcli.swift#L49) loads the capture filter
   and calls [append](../Sources/smcli/CapturedHistory.swift#L313).
   Each append validates the destination, opens a private database connection,
   checks/migrates the schema, and generates a UUID.
   [One transaction](../Sources/smcli/CapturedHistory.swift#L551) inserts the
   session/run and updates the existing decayed usage tally. SQLite uses a
   five-second busy timeout, WAL and `synchronous=NORMAL`. A successful CLI exit
   follows COMMIT, but NORMAL is not a promise to survive power loss.
   Direct capture currently generates a new UUID for each attempt: it is **not**
   retry-idempotent.
3. [`suggest`](../Sources/smcli/smcli.swift#L275) calls
   [suggestions](../Sources/smcli/CapturedHistory.swift#L504), which scans matching
   captured runs for the current user, computes recency/directory/success weights,
   aggregates distinct commands in Swift, sorts and limits. It does not use fff
   or semantic search.
4. [`pick`](../Sources/smcli/Picker.swift#L213) obtains 50 directory suggestions,
   aggregates captured stats, reads native histories, constructs/deduplicates its
   candidate array, and loads the configured embedding model before opening the
   terminal UI. Typing uses in-memory all-words substring matching. Tab invokes
   the existing hybrid search without passing semantic results through that
   substring filter.
5. [`HistorySearch.search`](../Sources/smcli/HistorySearch.swift#L272) locks and
   opens the persisted ProximaKit index, synchronizes embedded command documents,
   queries the existing keyword/semantic fusion, and reranks a bounded candidate
   pool with usage metadata. The index is opened and saved on each CLI search.
   This experiment leaves those algorithms and storage semantics unchanged.
6. The picker prints a selected command; existing ZLE/Readline widgets insert it
   into the line buffer. They do not execute it. Fish is not implemented yet.

## Measurement method

The fixture contains 1,000 / 10,000 / 100,000 captured runs and 200 / 2,000 / 5,000
distinct captured commands, spread across 20 directories. Ten deterministic
command templates include Git, npm, Cargo, Swift, paths with spaces, and Unicode.
Every seventh run fails. Native Bash history adds 100 / 1,000 / 10,000 lines
(100 / 1,000 / 5,000 distinct commands). Capture times are deterministic, ending
near 2026-10-18. The importer and schema are the production ones, not a hand-built
database schema. SQLite row/distinct counts are verified before measurement.

Each operation has one first invocation and 20 subsequent one-shot CLI invocations.
The record samples add 21 runs of one additional command before reads; hook
samples run last. `first` is **not a machine-cold cache**: no filesystem/model
caches are flushed. The first literal search does create the command index;
the subsequent intent query uses that index. “Warm” means OS caches/index files
are warm, **not** a resident service, resident model or in-process query loop.

CLI timing includes exec, dynamic linking and exit. Picker timing uses a
controlling PTY and stops when its first footer is drawn; it excludes Ctrl-G
cancellation and does not invoke Tab search. The PTY is drained through process
exit so terminal restoration cannot block waiting for a reader.

Hook timings measure a builtin `:` command through the next real interactive
prompt. Each shell runs without user rc files, with capture off, detached, then
synchronous. Three warmups precede each 20-sample run. “Overhead” subtracts the
same-shell off median; detached recording is verified to have persisted every
sample **outside** the timed section. Async prompt return is not a persistence
acknowledgement.

RSS comes from Unix `wait4`, converted from bytes on Darwin and KiB on Linux.
It is a high-water mark, not steady-state or summed concurrent process-tree
memory. Shell results can include waited-for descendants; detached writers are
not a reliable part of shell RSS. Synthetic histories are a scaling baseline,
not evidence about real-world ranking quality, plugins or large multiline inputs.

## Recorded baseline — 2026-10-18

Apple M1 Pro, 8 logical CPUs, 16 GiB RAM; macOS 27.0.1 arm64; Apple Swift 6.4;
Python 3.15.0. Bash on PATH was 5.3.20; Zsh was 5.9. Builds and tests were not run
concurrently with this benchmark.

- Source HEAD: `36468edbf03217e86d89ec091407d1690f7385b3` (production sources unchanged).
- Release binary SHA-256: `73e600aa959ca4f67a54c7452ab5d670d6b5208dff28e83e4fa8e51f2d1c3924`.
- Raw `dist/baseline.json` SHA-256: `2502eb4415cc134505a95cd58ff583d75fd1503b00de6cf8a157570311a01f4c`.
- Run started: `2026-10-18T13:11:51.142094+00:00`.

Times are milliseconds, rounded to two decimals. Each warm cell is median / p95.

| Operation | 1k first | 1k warm | 10k first | 10k warm | 100k first | 100k warm |
|---|---:|---:|---:|---:|---:|---:|
| Record CLI | 40.24 | 18.14 / 25.92 | 38.68 | 20.85 / 28.02 | 38.01 | 21.90 / 32.99 |
| Suggest, empty prefix | 17.15 | 15.54 / 16.40 | 54.90 | 53.00 / 55.22 | 455.16 | 453.93 / 461.20 |
| Suggest, `git` prefix | 13.70 | 12.31 / 13.44 | 28.84 | 24.68 / 25.73 | 148.12 | 149.90 / 157.58 |
| Hybrid `git status` | 801.99 | 52.81 / 54.16 | 7883.69 | 153.47 / 156.15 | 28307.92 | 897.76 / 915.65 |
| Hybrid intent query | 56.55 | 53.78 / 55.44 | 158.18 | 154.69 / 159.17 | 898.08 | 884.09 / 896.55 |
| Picker first draw | 21.86 | 20.24 / 21.29 | 95.83 | 87.40 / 89.97 | 703.41 | 699.33 / 705.04 |

The intent query is `show changes in working tree`. First-search cost includes
embedding the new corpus; the intent query's first cell does not.

Median warm high-water RSS, MiB:

| Operation | 1k | 10k | 100k |
|---|---:|---:|---:|
| Record CLI | 10.55 | 10.54 | 10.53 |
| Suggest, empty prefix | 11.60 | 15.97 | 30.25 |
| Suggest, `git` prefix | 11.38 | 14.02 | 27.34 |
| Hybrid literal query | 36.92 | 52.33 | 91.19 |
| Hybrid intent query | 37.88 | 53.23 | 92.29 |
| Picker | 12.37 | 17.27 | 31.91 |

First literal search (including index construction) peaked at 137.39 / 199.55 /
352.53 MiB. These are measurements of the existing history engine, not fff.

Hook roundtrip median / p95 in milliseconds; overhead is median minus capture-off:

| Shell / mode | 1k | 10k | 100k | Overhead at 100k |
|---|---:|---:|---:|---:|
| Bash off | 0.082 / 0.128 | 0.085 / 0.110 | 0.101 / 0.135 | — |
| Bash detached | 3.826 / 4.918 | 3.387 / 4.884 | 3.451 / 4.575 | 3.351 |
| Bash synchronous | 21.520 / 31.284 | 19.283 / 27.288 | 18.976 / 28.245 | 18.875 |
| Zsh off | 0.140 / 0.183 | 0.134 / 0.170 | 0.131 / 0.149 | — |
| Zsh detached | 2.310 / 3.598 | 1.850 / 4.375 | 1.940 / 3.954 | 1.809 |
| Zsh synchronous | 16.293 / 21.681 | 16.060 / 27.146 | 16.678 / 24.436 | 16.547 |

Interpretation: the measured large-history bottleneck is not just forking. Empty
suggestions and picker startup grow with captured history; a 100k picker takes
about 699 ms warm, versus 22 ms for a direct record. Retaining a shared owner is a
reasonable next experiment, but merely moving this scan into a service will not
make it constant-time. Keep algorithms unchanged initially and compare the same
workloads before deciding on more machinery.

## Pinned fff verification

- Repository: [dmtrKovalenko/fff](https://github.com/dmtrKovalenko/fff).
- Release: `v0.11.0`, peeled commit
  `95fd777c2529fc7b4d7572dabff64cc07268f2c5`.
- Codeload source archive SHA-256:
  `d2d96d7cc472571b23476b502a0b96f974f0e968c0f290b22078cc300bea6c64`.
- Pin and download check: [build-fff.sh](../scripts/build-fff.sh#L6). Cargo uses
  upstream's committed lockfile with `--locked`. No moving branch or prebuilt
  binary is downloaded. The upstream generated header is used unchanged.
- License: MIT, verified in the pinned root `LICENSE` and `fff-c/Cargo.toml`.
  The notice is retained in [tools/fff/LICENSE](../tools/fff/LICENSE) and copied to
  `.build/fff/LICENSE`. This is not a transitive-license distribution audit;
  do that before shipping a service binary containing fff.
- Build: Cargo/Rust, a native C toolchain, zlib and the platform SDK/headers used
  by vendored libgit2/LMDB. Verified with Rust/Cargo 1.99.1 and the installed Apple
  toolchain. Upstream uses Rust edition 2024 and does not declare a single
  workspace MSRV; an older compiler's compatibility is not verified.
- Features: `cargo build --locked --release -p fff-c`, default **ripgrep**.
  No Zig, Lua, Neovim, cbindgen, TCP or MCP server is required. The alternate `zlob`
  feature requires Zig and is intentionally not enabled.
- The build is launched from this repository root with `--manifest-path`, rather
  than changing into upstream's source tree. This avoids its `rust-toolchain.toml`
  auto-updating stable/installing optional tools; use the caller's active Rust
  toolchain instead. Rust/Cargo must already be installed.
- [Upstream C release matrix](https://github.com/dmtrKovalenko/fff/blob/95fd777c2529fc7b4d7572dabff64cc07268f2c5/.github/workflows/release.yaml#L157)
  includes arm64 and x86_64 macOS, GNU Linux, musl Linux and Windows MSVC, plus
  arm64 Android. Those release builds use `zlob`. **Only macOS arm64 with ripgrep
  has been built and run here**; smcli's Intel/Linux integration remains unverified.

### Exported API and ownership

The built dylib's symbols were inspected with `nm -gU`, and the Swift program
compiled/linked against these real exports:

| API | Ownership / use |
|---|---|
| `fff_create_instance_with` | Pass version 2 `FffCreateOptions`; caller-owned UTF-8 C strings remain alive for the call. fff copies strings. Result handle owns the instance. |
| `fff_wait_for_scan` | Result integer is 1 for complete, 0 for timeout. Smoke bounds the wait to 10 seconds. |
| `fff_search` | Returns a separately owned `FffSearchResult` payload inside the result envelope. |
| `fff_search_result_get_count`, `fff_search_result_get_item`, `fff_file_item_get_relative_path` | Borrowed item/path; copy to Swift strings before freeing the search payload. No offset arithmetic or duplicated struct declarations. |
| `fff_free_search_result` | Frees the payload, arrays and their strings. |
| `fff_free_result` | Frees the envelope/error string **only**, not its handle. Needed on success and failure. |
| `fff_destroy` | Frees the instance; also called explicitly if scan completion fails during Swift initialization. |

The [serial-use Swift boundary](../tools/fff/smoke.swift#L11) owns one opaque
handle, frees it in deinit, keeps borrowed pointers inside the call, and rejects
embedded NULs. It is deliberately not Sendable. The smoke test uses temporary
files, ten instance lifecycles, 1,000 reused-instance searches, empty results,
real C error envelopes, and exact relative-path checks for Unicode, spaces,
quotes and shell metacharacters. It uses explicit throwing checks in optimized
builds, not assertions compiled out by `-O`.

Watchers, frecency/query LMDB paths, content indexing and mmap prewarming are all
disabled for this ownership test. It does not measure a large-repository index,
watcher freshness, grep, file-selection frecency or safe shell insertion. Those
remain file-search release acceptance tests, not inferred from this smoke.

`make fff-smoke` passed. `MallocStackLogging=1 leaks --atExit --
.build/fff/smoke` reported **0 leaks / 0 leaked bytes**, with the macOS
restricted-process visibility warning. This is supporting evidence, not proof
against all leaks or Rust global allocations. The experimental dynamic library
remains inside `.build`; it is not a relocatable release artifact. `smcli` does
not link it.

## Next gate

Introduce the separate service executable and versioned bounded Unix-socket
protocol, moving existing storage/retrieval ownership without replacing search.
Before routing hooks through it, establish private socket ownership,
concurrent-startup handling, persistence acknowledgements and stable event IDs
shared with direct fallback so retries cannot double-record. Then test restart,
concurrent shells and unavailable-service fallback against this baseline.

Native Bash/Zsh/Fish adapter work, cached Git/package context, fff file/grep picker
modes, ranking extensions and crash/eviction tests are later gates. No shell
initialization commands are changed by this baseline experiment.
