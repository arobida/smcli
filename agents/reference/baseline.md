# Backend baseline and fff verification

Historical reference (2026-10-18 to 2026-10-20). These measurements were taken on the Swift implementation
before the Rust port. The method, fixtures and fff ownership notes still describe how the benchmark works,
but the latency numbers predate the Rust binaries and were not re-measured. Current sources are `crates/`.

Status at the time: **steps 1 to 7 measured; macOS arm64 was the primary target and macOS x86_64 built,
linked and passed the service and file-picker tests under Rosetta. Linux was not supported then; the Rust
port now supports it (see `../plans/roadmap.md`).** The original step-1 numbers (2026-10-18, in-process) are
kept below for comparison with the later ones.

Two modes exist and the report names the one it measured: `SMCLI_SERVICE=0` (in-process, the bench default
`--service off`) and the service (`--service on`). smcli itself starts `smcli-service` by default, so a normal install
behaves like `--service on`. The service owns the history database connection, the suggestion index, the search index,
cached directory context and the fff pool. Only `smcli-service` links fff; `smcli` has no fff symbols.
Schema version is **7** (`pipestatus`, `file_selections`, `command_selections`).

## Reproduce

From the repository root:

```sh
make baseline                                 # in-process, writes dist/baseline.json
make baseline BENCH_ARGS="--service on --output dist/baseline-service.json"   # through the service, plus repositories
make baseline BENCH_ARGS="--service on --rows 1000 --repo-files 10000"        # a short service run
make fff-smoke
make test
```

`make baseline` builds the release CLI and writes raw samples, medians, nearest-rank
p95s, high-water RSS, binary SHA-256, toolchain and host metadata to
`dist/baseline.json`. It never imports or reads the user's real history. `--service on`
additionally records the service's resident size after each fixture (`service_rss_mib`) and runs the
`repositories` section: a generated Git repository of 10,000 and 50,000 tracked files, measuring directory
context, fff `files` and `grep` through the socket (cold = first request of a fresh service, warm = median).

Python 3.9+, Bash and Zsh are used for the Unix benchmark. Shells are discovered on
`PATH`; absent shells are omitted from the report. `make test` also runs a tiny
10-row/one-sample benchmark as a functional harness check, without performance
thresholds. fff is built by `make fff` (a prerequisite of `build`, `release`, `fresh` and `test`;
Cargo must be installed) and linked statically into `smcli-service`.

## Request path (Swift implementation, 2026-10-20)

1. Generated shell hooks (`crates/smcli/src/shell_integration.rs` in the current tree) (Bash, Zsh, Fish) keep a
   pending command, starting cwd, session and start time. At the next prompt (or exit)
   they call `smcli record`. Prompt recording is detached by default; `SMCLI_SYNC=1` and
   shell exit wait for it. Zsh registers in `precmd_functions`/`preexec_functions`/`zshexit_functions`;
   Bash prefers bash-preexec, then `PS0`, then the DEBUG trap; Fish uses `fish_preexec`/`fish_postexec`.
2. `smcli record` sends a versioned, newline-delimited JSON `record` request to `smcli-service` over a
   user-private Unix socket (peer UID checked). A client-generated event ID (`uuid`) makes retries
   idempotent; the ack follows COMMIT. If the service is unreachable the client records directly in-process.
   The service holds one SQLite connection (WAL, `synchronous=NORMAL`, five-second busy timeout), so
   a successful ack follows COMMIT but is not a promise to survive power loss.
3. `suggest` and the picker's first draw are answered from `SuggestionIndex`, an in-memory tally of
   history that is updated per record and rebuilt only when another process writes
   (`PRAGMA data_version`). Usage is decayed when read (`smcli_decay`, two-week half-life); there is no `usage`
   table. Ranking adds project, previous-command, picked-query, project-script and Git-state signals
   (see the plan, step 6). In-process mode scans the database as before.
4. `search` runs the hybrid TalaDB index (keyword plus the built-in potion-code-16M-v2 embedder; ProximaKit when
   these numbers were taken). Each search opens and closes the index under a file lock; the service caches the
   indexed metadata between searches and a write by another process forces a full re-read.
   The picker builds its own engine lazily on the first Tab when there is no service.
5. Directory context (`ContextCache`) and fff `files`/`grep` are service-only: cached Git status refreshed in
   the background, and one fff instance per repository root (4 by default, idle ones dropped).
6. The picker prints a selected command or one shell-quoted path; the adapters insert it into the line
   buffer. They never execute it.

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
are warm. In-process runs (`SMCLI_SERVICE=0`) have no resident service or model; in service mode the work happens in a
long-lived `smcli-service` and the timing is the client process (exec, one socket round trip, exit).

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

## Recorded baseline — 2026-10-20 (step 7)

Same machine, toolchain, shells and method as the earlier runs (Apple M1 Pro, macOS 27.0.1 arm64, Swift 6.4,
Python 3.15.0, Bash 5.3.20, Zsh 5.9). Source: `3822ac5` **plus the uncommitted working-tree changes of steps 2 to 7**
(the recorded git commit is only the base). Release binary SHA-256
`3b1fc9c7d696008c0190187aee39ae184c125344ebe4073498e57bf6a09e6975`; raw files `dist/baseline.json`
(`9436c450b3e8...`, in-process, started 2026-10-19T21:26Z) and `dist/baseline-service.json` (`8df54d533cd8...`, service,
started 21:30Z). Times are milliseconds; each warm cell is median / p95. In service mode the number is what the
**client process** takes, including exec; the work happens in the already running service.

### Warm and first-request latency

100,000 captured runs, the numbers that mattered in step 1:

| Operation | Step 1 in-process (10-18) | Now in-process | Now through service | First request, service |
|---|---:|---:|---:|---:|
| Record CLI | 21.90 | 20.97 / 37.91 | 10.78 / 12.33 | 66.72 (starts the service) |
| Suggest, empty prefix | 453.93 | 458.69 / 462.95 | **12.72 / 13.34** | 515.82 (builds the in-memory index) |
| Suggest, `git` prefix | 149.90 | 151.91 / 154.21 | 10.96 / 12.02 | 11.89 |
| Hybrid `git status` | 897.76 | 894.43 / 905.74 | **25.77 / 27.13** | 7,707.97 (builds the index once) |
| Hybrid intent query | 884.09 | 893.07 / 900.18 | 24.69 / 25.82 | 27.30 |
| Picker first draw | 699.33 | 651.04 / 655.34 | **61.19 / 62.38** | 61.80 |

Service mode, all sizes:

| Operation | 1k first | 1k warm | 10k first | 10k warm | 100k first | 100k warm |
|---|---:|---:|---:|---:|---:|---:|
| Record CLI | 61.48 | 9.84 / 11.23 | 52.39 | 9.80 / 11.81 | 66.72 | 10.78 / 12.33 |
| Suggest, empty prefix | 17.99 | 9.82 / 11.69 | 63.19 | 10.96 / 11.59 | 515.82 | 12.72 / 13.34 |
| Suggest, `git` prefix | 10.08 | 10.00 / 10.88 | 9.59 | 10.51 / 11.93 | 11.11 | 10.96 / 12.02 |
| Hybrid `git status` | 251.18 | 19.23 / 21.48 | 2134.04 | 21.06 / 22.28 | 7707.97 | 25.77 / 27.13 |
| Hybrid intent query | 20.66 | 18.47 / 19.72 | 19.75 | 20.47 / 21.53 | 27.30 | 24.69 / 25.82 |
| Picker first draw | 12.94 | 11.88 / 13.23 | 21.81 | 22.93 / 23.96 | 61.80 | 61.19 / 62.38 |

In-process mode, all sizes (the 10-18 numbers are in the section below):

| Operation | 1k first | 1k warm | 10k first | 10k warm | 100k first | 100k warm |
|---|---:|---:|---:|---:|---:|---:|
| Record CLI | 57.41 | 23.26 / 43.05 | 43.63 | 39.05 / 49.67 | 39.98 | 20.97 / 37.91 |
| Suggest, empty prefix | 18.49 | 17.52 / 19.05 | 64.63 | 61.38 / 65.26 | 460.96 | 458.69 / 462.95 |
| Suggest, `git` prefix | 15.49 | 13.88 / 15.32 | 28.62 | 29.03 / 30.55 | 149.84 | 151.91 / 154.21 |
| Hybrid `git status` | 228.68 | 62.76 / 63.85 | 2084.02 | 223.71 / 225.43 | 7730.16 | 894.43 / 905.74 |
| Hybrid intent query | 61.68 | 62.51 / 64.49 | 223.76 | 224.05 / 228.96 | 893.10 | 893.07 / 900.18 |
| Picker first draw | 22.27 | 20.31 / 21.98 | 85.23 | 86.64 / 87.29 | 653.09 | 651.04 / 655.34 |

Cold versus warm: "first" is the first invocation for a fixture (OS caches not flushed). Through the service the
first request pays for building the state it keeps (suggestion index 0.5 s at 100k; the search index 7.7 s at 100k,
unchanged from in-process and done once per history, after which a record updates it in place). The first
record also starts the service, which adds roughly 50 ms to that one command. Every later request is
warm: about 10 ms, which is process start-up and one socket round trip.
The step-1 finding holds in both directions: moving the scan into the service did not make it fast, but the incremental
state did (suggest 454 -> 13 ms, search 884 -> 25 ms, picker 699 -> 61 ms at 100k). The picker's remaining 61 ms is
the native-history read it still does in-process. In-process numbers are unchanged from step 1 within noise
(the 1k/10k record cells are noisier than before, 23 and 39 ms medians against 18 and 21 ms).

### Memory

Median warm high-water RSS of one client process, MiB (step 1 at 100k in parentheses):

| Operation | In-process 1k | 10k | 100k | Service client, any size |
|---|---:|---:|---:|---:|
| Record CLI | 11.50 | 11.53 | 11.50 (10.53) | 9.91 |
| Suggest, empty prefix | 12.61 | 16.36 | 31.25 (30.25) | 9.59 |
| Suggest, `git` prefix | 12.39 | 14.67 | 27.41 (27.34) | 9.61 |
| Hybrid literal / intent query | 26.88 | 36.86 | 63.42 (91.19) | 9.84 |
| Picker | 13.31 | 17.45 | 32.64 (31.91) | 10.48 to 12.59 |

First literal search (index construction) peaked at 30.48 / 55.25 / 145.30 MiB in-process (352.53 MiB at 100k in
step 1). The memory moves into the service, which holds it for as long as it runs. Resident size of
`smcli-service` after a whole fixture run (records, suggest, search, picker, six hook runs):

| History | 1k | 10k | 100k |
|---|---:|---:|---:|
| `smcli-service` RSS, MiB | 33.3 | 62.8 | 211.3 |

An idle service exits after `SMCLI_SERVICE_IDLE` seconds, so this cost is paid only while shells are in use.

### Hooks

Hook roundtrip median / p95 in milliseconds at 100k runs; overhead is median minus capture-off:

| Shell / mode | In-process | Overhead | Service | Overhead |
|---|---:|---:|---:|---:|
| Bash off | 0.087 / 0.102 | — | 0.088 / 0.095 | — |
| Bash detached | 4.243 / 5.562 | 4.155 | 2.992 / 3.225 | 2.904 |
| Bash synchronous | 21.406 / 42.012 | 21.319 | 10.322 / 11.962 | 10.234 |
| Zsh off | 0.141 / 0.160 | — | 0.139 / 0.149 | — |
| Zsh detached | 2.244 / 8.042 | 2.103 | 1.752 / 2.728 | 1.612 |
| Zsh synchronous | 24.203 / 42.869 | 24.062 | 9.935 / 10.878 | 9.796 |

The service halves the synchronous cost and tightens the p95 considerably (a service client no longer opens the
database or checks the schema). Detached recording is not on the typing path in either mode.

### Directory context and fff (service only)

Generated Git repositories of 100 files per directory plus 5 untracked files, queried from a subdirectory.
Cold is the first request of a fresh service; warm is the median of 20 repeats.

| Tracked files | context cold | Git status ready | context warm | files cold | files warm | grep cold | grep warm | service RSS before / after |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 10,000 | 1.0 ms | 28.8 ms | 0.18 ms | 243 ms | 0.47 ms | 100 ms | 102 ms | 10.1 / 25.9 MiB |
| 50,000 | 1.0 ms | 97.8 ms | 0.19 ms | 1,401 ms | 0.96 ms | 1,020 ms | 1,488 ms | 10.1 / 59.6 MiB |

Context never waits for Git: the first answer carries `statusPending` and the status lands within about 100 ms even at
50,000 files. The fff instance is created on the first `files`/`grep` for a root and costs its scan time (1.4 s at
50,000 files); after that file search is about a millisecond. **Grep is the weak spot:** warm grep at 50,000 files
takes 1.5 s because each query scans the content (the call has a 1 s budget and returns what it found), so very large
trees may get partial grep results rather than a stall. `scripts/test-service.py` also builds a 50,000-file repository
and asserts correctness (right file, line and column; new files appear; the service stays under 1.5 GB).

### Changes since 2026-10-19 (pre-service run)

Record and hook costs in-process are unchanged (within noise), as are warm hybrid searches at 1k/10k (63 and 224 ms).
The service is the answer to the 100k scans, as the step-1 finding predicted.

## Recorded baseline — 2026-10-19 (pre-service run, in-process)

Same machine, toolchain, shells and method as the 2026-10-18 run below.

- Source HEAD: `3822ac5b097cf293511d31b6090dc1b80a8ecb5d` (built-in Potion embedder, lazy picker engine).
- Release binary SHA-256: `1840d0b9559c566bd9dc6a313d0e6c2ad32fa06b1a7b821b509d7b92dac9c50f`.
- Raw `dist/baseline.json` SHA-256: `bf396ec2eeed9775c48b83d9f9dc056b604ef456371573490f8a6565c27e2752`.
- Run started: `2026-10-19T11:57:46.599791+00:00`.

| Operation | 1k first | 1k warm | 10k first | 10k warm | 100k first | 100k warm |
|---|---:|---:|---:|---:|---:|---:|
| Record CLI | 41.69 | 19.44 / 35.45 | 29.42 | 23.98 / 29.42 | 40.86 | 23.22 / 27.72 |
| Suggest, empty prefix | 21.10 | 18.38 / 19.19 | 61.46 | 62.75 / 65.38 | 493.69 | 492.86 / 498.94 |
| Suggest, `git` prefix | 14.90 | 14.94 / 15.69 | 30.01 | 30.35 / 31.64 | 158.68 | 162.32 / 164.79 |
| Hybrid `git status` | 253.24 | 65.32 / 67.15 | 2235.09 | 215.39 / 217.39 | 8897.78 | 876.64 / 882.02 |
| Hybrid intent query | 66.57 | 65.45 / 66.29 | 217.36 | 214.07 / 218.24 | 876.84 | 881.35 / 885.94 |
| Picker first draw | 22.69 | 21.77 / 22.73 | 91.86 | 91.29 / 93.12 | 691.19 | 696.17 / 699.15 |

Median warm high-water RSS, MiB:

| Operation | 1k | 10k | 100k |
|---|---:|---:|---:|
| Record CLI | 10.48 | 10.52 | 10.53 |
| Suggest, empty prefix | 11.62 | 15.42 | 30.84 |
| Suggest, `git` prefix | 11.44 | 13.69 | 26.71 |
| Hybrid literal query | 26.46 | 36.91 | 63.10 |
| Hybrid intent query | 26.44 | 37.18 | 62.85 |
| Picker | 12.22 | 16.77 | 32.64 |

First literal search (including index construction) peaked at 30.16 / 54.97 / 147.20 MiB.

Hook roundtrip median / p95 in milliseconds; overhead is median minus capture-off:

| Shell / mode | 1k | 10k | 100k | Overhead at 100k |
|---|---:|---:|---:|---:|
| Bash off | 0.079 / 0.088 | 0.076 / 0.094 | 0.079 / 0.094 | — |
| Bash detached | 3.782 / 5.282 | 3.881 / 5.902 | 4.330 / 5.566 | 4.251 |
| Bash synchronous | 23.171 / 28.005 | 21.290 / 28.340 | 20.644 / 31.208 | 20.564 |
| Zsh off | 0.119 / 0.151 | 0.141 / 0.179 | 0.136 / 0.164 | — |
| Zsh detached | 1.946 / 3.203 | 2.214 / 3.470 | 2.568 / 4.180 | 2.432 |
| Zsh synchronous | 20.219 / 27.042 | 21.204 / 24.559 | 19.513 / 29.571 | 19.378 |

Changes since 2026-10-18: the first hybrid search (index construction) is 3–4× faster
(100k: 28.3 s → 8.9 s) and its peak RSS fell (352.5 → 147.2 MiB), from the Potion
embedder. Warm hybrid searches are slightly slower at 1k/10k (53 → 65 ms, 153 → 215 ms)
and about equal at 100k. The bottleneck is unchanged: at 100k, empty-prefix `suggest`
(~493 ms) and picker first draw (~696 ms) still scan all history; the lazy engine did
not change picker first draw because the benchmark never pressed Tab. Record and hook
costs are within noise.

## Recorded baseline — 2026-10-18 (step 1)

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

Historical build notes for the C wrapper. The Rust port links `fff-search` from Cargo instead of building
`scripts/build-fff.sh`; the upstream pin and license facts below still apply.

- Repository: [dmtrKovalenko/fff](https://github.com/dmtrKovalenko/fff).
- Release: `v0.11.0`, peeled commit
  `95fd777c2529fc7b4d7572dabff64cc07268f2c5`.
- Codeload source archive SHA-256:
  `d2d96d7cc472571b23476b502a0b96f974f0e968c0f290b22078cc300bea6c64`.
- Pin and download check: `scripts/build-fff.sh` (removed with the Rust port). Cargo uses
  upstream's committed lockfile with `--locked`. No moving branch or prebuilt
  binary is downloaded. The upstream generated header is used unchanged.
- License: MIT, verified in the pinned root `LICENSE` and `fff-c/Cargo.toml`.
  The notice was retained in `tools/fff/LICENSE` (removed with the Rust port) and copied to
  `.build/fff/LICENSE`. The transitive-license audit is in
  [fff-licenses.md](fff-licenses.md).
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
  arm64 Android. Those release builds use `zlob`. smcli builds the default **ripgrep** feature set from source; that has been
  built and run on **macOS arm64** and **macOS x86_64** (see "Platforms"). Linux has not.

### Exported API and ownership

The built library's symbols were inspected with `nm`, and the Swift code
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

The serial-use Swift boundary (`tools/fff/smoke.swift`, removed with the Rust port) owned one opaque
handle, freed it in deinit, kept borrowed pointers inside the call, and rejected
embedded NULs. It was deliberately not Sendable. The smoke test used temporary
files, ten instance lifecycles, 1,000 reused-instance searches, empty results,
real C error envelopes, and exact relative-path checks for Unicode, spaces,
quotes and shell metacharacters. It uses explicit throwing checks in optimized
builds, not assertions compiled out by `-O`.

Watchers, frecency/query LMDB paths, content indexing and mmap prewarming are all
disabled for this ownership test. It does not measure a large-repository index,
watcher freshness, grep, file-selection frecency or safe shell insertion. Those
are covered by the service tests instead (`scripts/test-service.py`: watcher freshness, grep, selections, awkward
names, a 50,000-file repository, eviction), not inferred from this smoke.

`make fff-smoke` passed. `MallocStackLogging=1 leaks --atExit --
.build/fff/smoke` reported **0 leaks / 0 leaked bytes**, with the macOS
restricted-process visibility warning. This is supporting evidence, not proof
against all leaks or Rust global allocations. The smoke test uses a dynamic library kept inside `.build`; the product
links the static archive `.build/fff/lib/libfff_c.a` into `smcli-service` only. `smcli` has no fff symbols.

## Platforms

Historical table from the Swift build. Current support is macOS and Linux with search included on both;
`make linux` runs the full suite in Docker (see `../plans/roadmap.md` for the unverified historical Fish
prompt timeout).

| Target | Status |
|---|---|
| macOS arm64 | Built and tested (`make test`, shell tests, service tests, baseline). |
| macOS x86_64 | `rustup target add x86_64-apple-darwin`, then `make x86_64` cross-builds fff (Cargo `--target`) and both executables with SwiftPM `--arch x86_64` into `.build/x86_64-scratch/`. Run under Rosetta: `scripts/test-service.py` (including the 50,000-file repository and fff) and `scripts/test-file-picker.py` pass. The Swift unit tests and shell tests were not repeated on x86_64. |
| macOS universal | `make fff-universal` lipo-joins the two fff archives into `.build/fff/lib/libfff_c.a`; `swift build -c release --arch arm64 --arch x86_64` then produces fat `smcli` and `smcli-service`. Built and checked with `lipo -info`; not run. |
| Linux | **Not supported, not tried.** `Package.swift` links fff and the potion model only on macOS (`.when(platforms: [.macOS])`), `taladb-swift` links its engine as a macOS xcframework here (its Linux system-library path is untried), FSEvents is macOS-only, and `scripts/build-fff.sh` has no Linux branch. The service product would not link without fff; `smcli search` already reports itself unavailable. A Linux port needs the fff link line, a Linux TalaDB engine build and a decision on the `zlob` feature. |

`scripts/build-fff.sh` always writes `.build/fff/lib/libfff_c.a` (thin for this machine by default; `FFF_ARCHS="arm64 x86_64"`
for universal) and `Package.swift` links that path. On x86_64 the linker prints four warnings for fff's hand-written
BLAKE3 assembly objects ("built for newer macOS (27.0) than being linked (14.0)"): that code ignores
`MACOSX_DEPLOYMENT_TARGET` and the warning is harmless. The arm64 slice is clean. SwiftPM also warns that x86_64 is deprecated
for the macOS 27 SDK; the project's deployment target remains macOS 14.

## Found while writing the step 7 tests

- fff only reports file-system changes once its watcher is running, which on a larger tree is some time after the
  initial scan completes. Files created in that gap (seen with 5,000+ files, and with 50,000) were never indexed until
  the instance was recreated. `FFFIndex` now also waits for the watcher (`fff_wait_for_watcher`) before it is used, and
  the large-repository test creates a file right after the first query.
- `scripts/test-service.py` matched the service process by an unstamped binary name; it now also matches the
  timestamped `dist/smcli-service-*` names.

## Status of later gates

Native shell adapters, cached Git/package context, the fff file/grep picker modes, ranking signals and the crash,
concurrency, malformed-input, stale-index, large-repository and eviction tests are done and listed in
`../reports/2026-10-20-backend.md`. Still open: a license for smcli and the xdiff (LGPL) decision
(`THIRD_PARTY_NOTICES.md`, `fff-licenses.md`), and persistent-socket or async-suggestion work that is only
justified if measurements ask for it.
