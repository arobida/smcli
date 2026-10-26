# Codebase review — 2026-10-26

## Ponytail audit — fresh complexity pass

Reviewed the current working tree at base commit `e92a7540e18f8de15a74dc35de4c80bb2293e7e3`, including existing uncommitted changes. Surveyed the four first-party crates, dependency manifests, Makefile, scripts and test scaffolding; followed candidate cuts through their call sites. Vendored dependencies, model assets and generated notices were excluded. Static review only: no source fixes, benchmarks or runtime tests were performed in this pass.

Ranked by estimated net source lines removed, including replacement code. Estimates are planning figures, not measured patch sizes. The dependency count means one direct Cargo dependency declaration; it does not promise removal from the transitive lockfile. Existing correctness findings and their original validation are retained below as historical evidence and were not revalidated by this complexity pass.

- shrink: Cut the custom `Temporary` implementation and atomic-counter import (about 22 lines net). Replace `Temporary::new()` with `tempfile::tempdir().unwrap()` and `.0` with `.path()`; `tempfile` is already a core dev-dependency. [crates/smcli-core/src/service_location.rs:110–131](../../crates/smcli-core/src/service_location.rs#L110).
- shrink: Cut the separate Bash marker-validation loops and repeated validation before removal (about 20 lines net across both scripts). Extend each existing AWK removal pass to reject nested, unmatched and unterminated markers, write only to a temporary file, then copy back only on success; preserve the install backup and malformed-block tests without adding a shared script that the installed uninstaller would need. [scripts/install.sh:98–120](../../scripts/install.sh#L98), [scripts/uninstall.sh:79–100](../../scripts/uninstall.sh#L79).
- shrink: Cut duplicated search-hit normalization, filtering and deduplication loops (about 10 lines net). Use one local `append_hits(results, filter, scale)` closure with `scale * (if best > 0.0 { score / best } else { 1.0 })`; retain separate retrieval calls and capture `parsed_useful` before appending fallback hits, passing mutable hit/seen collections as arguments to avoid long-lived mutable captures. [crates/smcli-search/src/index.rs:219–243](../../crates/smcli-search/src/index.rs#L219).
- shrink: Cut the second copy of picker candidate trimming, internal-command filtering and deduplication (about 6 lines net). Reuse the existing `add` closure for both loops; have it return `candidates.len()` and save its last return value while adding suggestions, so `suggestion_count` needs no intervening borrow of `seen`. Preserve suggestion-first ordering and native-history reversal. [crates/smcli/src/picker.rs:95–121](../../crates/smcli/src/picker.rs#L95).
- shrink: Cut the CLI's second UUID-generation route and direct `uuid` dependency (about 1 line net plus one dependency declaration). Expose the existing fallible `smcli_core::history::new_event_id`, call it before service dispatch so retries retain the same event ID, and use it in the six CLI test-directory names too; keep the core's existing UUID-v4 bit handling and entropy source. [crates/smcli-core/src/history.rs:183–193](../../crates/smcli-core/src/history.rs#L183), [crates/smcli/src/cli.rs:516](../../crates/smcli/src/cli.rs#L516), [crates/smcli/Cargo.toml:19](../../crates/smcli/Cargo.toml#L19).
- delete: Cut the tracked Python bytecode cache (binary artifact; zero source lines counted). Replacement: nothing in version control; Python regenerates it locally. [scripts/**pycache**/bench-baseline.cpython-315.pyc](../../scripts/__pycache__/bench-baseline.cpython-315.pyc).

net: -59 lines, -1 deps possible.

## Prior correctness review — retained, not revalidated

The release verdict, twelve defect reports and test results below belong to the earlier correctness review. They are outside this ponytail audit's scope; this pass neither closes them nor claims to have rerun their reproductions.

### Verdict

**Block release on the three P1 findings.** This audit identified 12 actionable defects: three P1 and nine P2. A passing test suite does not cover the failure cases below.

All findings are open as of the reviewed snapshot. This is a whole-tree review, including pre-existing defects, rather than a claim that every issue was introduced by the current diff. No fixes were applied during the audit.

### Scope and evidence

- Base commit: `e92a7540e18f8de15a74dc35de4c80bb2293e7e3` (`main`), including the working-tree alias gate, picker, CLI and test changes present on October 26, 2026.
- Inspected first-party Rust workspace code, shell integration, installation and removal scripts, tests, and documentation. Third-party dependencies were not subjected to a comprehensive security audit.
- Reproductions used temporary homes, databases, configuration files and runtime directories. Secret examples are synthetic. The user's native history was not modified.
- The working tree changed during the initial audit. The final test run used an isolated snapshot; production source files still matched that snapshot when the review finished. Draft OpenSpec documents changed separately.
- Source locations below refer to that snapshot; line numbers can move as fixes land.

### Findings

#### F01 · P1 · Make the PS0 transfer file private

**Location:** [crates/smcli/src/shell_integration.rs:112](../../crates/smcli/src/shell_integration.rs#L112)

With shared `TMPDIR` and umask `022`, Bash's PS0 hook writes alias bodies into a world-readable `0644` transfer file before capture filtering. Other users able to traverse the temporary directory can read command text that the private history database would reject.

**Evidence:** Exercised the generated `_smcli_ps0_alias` and alias lookup functions with a synthetic `sk-proj-ABCDEFGHIJKLMNOPQRST` token. The file under `/tmp` had mode `0644` and contained the token. Sending the same expansion to `record` correctly produced an empty captured history. The transfer file therefore bypasses that protection.

**Remediation:** Create the transfer file securely with mode `0600`, keep it in a private directory, and clean it up when the shell exits. Avoid following an existing path supplied by another process.

#### F02 · P1 · Reject assignments throughout alias bodies

**Location:** [crates/smcli-core/src/parse.rs:183](../../crates/smcli-core/src/parse.rs#L183); corresponding shell prechecks in `shell_integration.rs`.

The assignment check splits on whitespace and recognizes only plain `NAME=` words. Parentheses and assignment operators bypass it, allowing values from secret-bearing alias assignments to reach storage despite the gate's stated contract.

**Evidence:** Both `(TOKEN=abc123 printf ok)` and `TOKEN+=abc123 printf ok` passed the generated Bash precheck and Rust record gate. `history --json` contained their complete expansions, including `abc123`.

**Remediation:** Reject assignments conservatively across shell syntax boundaries and supported assignment operators, including parenthesized commands and `+=`. Unsupported expansions should remain typed-only.

#### F03 · P1 · Recover from services using an older protocol

**Location:** [crates/smcli-core/src/service_client.rs:102](../../crates/smcli-core/src/service_client.rs#L102)

`exchange` discards a version-mismatch response before `send` can inspect the stale build and replace it. Shutdown also uses only the current protocol. History operations fall back, but file and grep search require the service, so an incompatible service can leave those features unavailable indefinitely. Rejected requests also refresh the old service's activity time.

**Evidence:** Ran the current protocol-v2 CLI against a private socket implementing the older service's protocol-v1 rejection behavior. `service start` failed; `service stop` exited successfully while saying the service was not running. The socket remained, and the only requests sent were protocol-v2 `ping` and `shutdown`.

**Remediation:** Preserve enough mismatch information to distinguish an incompatible service from an unreachable one, and support a safe compatible shutdown/replacement path. Do not report an incompatible running service as absent.

#### F04 · P2 · Allow installation on supported Linux systems

**Location:** [scripts/install.sh:37](../../scripts/install.sh#L37)

The installer rejects every platform whose `uname` is not `Darwin`, although the repository declares Linux support. This breaks `make install` on Linux and the installer stage of `make linux`, which invokes `scripts/test-install.sh`.

**Evidence:** Substituting only `uname`'s response with `Linux` made the installer exit with status 1 and `smcli installs on macOS 14 or later.` before installation began. This was a platform-probe simulation, not a Linux runtime test.

**Remediation:** Accept supported Linux systems and make macOS-specific actions conditional.

#### F05 · P2 · Include the model in manual release installation

**Location:** [docs/install.md:67](../../docs/install.md); loader contract in [crates/smcli-search/src/potion.rs](../../crates/smcli-search/src/potion.rs).

The manual release instructions copy only the two executables. The model is now a separate required `potion-shell-16m/` folder. A development checkout masks the omission because the loader can find the checkout's model through its compiled fallback path.

**Evidence:** Copied the CLI into a temporary binary-only installation and made the checkout model inaccessible using a macOS sandbox profile. `health --json` exited 1 with `potion-shell-16m model not found beside the smcli executable`.

**Remediation:** Update manual release instructions to install the model folder beside both binaries. Correct the accompanying claims that the model is embedded in the executables.

#### F06 · P2 · Persist an absolute model directory

**Location:** [crates/smcli/src/initialize.rs:79](../../crates/smcli/src/initialize.rs#L79)

Initialization validates the model relative to the current directory, then stores the original relative path. Later commands or a service launched from another directory cannot reliably locate that same model.

**Evidence:** `init --potion Models/potion-shell-16m` succeeded from the checkout. Running `health --json` from a temporary directory then failed and attempted a Hugging Face lookup for `Models/potion-shell-16m`.

**Remediation:** Canonicalize the validated local directory before persisting `potionPath`.

#### F07 · P2 · Bound model-derived index filenames

**Location:** [crates/smcli-search/src/potion.rs:44](../../crates/smcli-search/src/potion.rs#L44)

The complete canonical model path becomes part of one index filename. Valid nested model directories can therefore produce filenames longer than the filesystem permits.

**Evidence:** A working model under a 245-byte directory path passed `init --potion`. After recording a command, `search` failed with `File name too long (os error 63)` on macOS.

**Remediation:** Use a bounded identity, such as a short readable prefix plus a hash, rather than the full flattened path.

#### F08 · P2 · Preserve the custom model during plain initialization

**Location:** [crates/smcli/src/initialize.rs:61](../../crates/smcli/src/initialize.rs#L61)

Plain `smcli init` asks about picker keys but also enters the model-reset branch. With no explicit model argument, `wanted` becomes `None`, overwriting an existing custom model choice.

**Evidence:** Configured a custom model with `init --potion <absolute-directory>`, then ran plain `init`. Both commands succeeded, but the second removed `potionPath` from the saved configuration and selected the bundled model.

**Remediation:** Preserve the existing model unless the user supplies an explicit model choice or reset option.

#### F09 · P2 · Decode Bash 3.2's quoted alias output

**Location:** [crates/smcli/src/shell_integration.rs:84](../../crates/smcli/src/shell_integration.rs#L84)

The Bash 3.2 fallback strips the outer quotes from `alias` output without decoding the shell escapes inside. The widened gate now accepts that incorrectly reconstructed body, corrupting the displayed expansion and command identity.

**Evidence:** An alias whose body was `printf '%s' hi` was captured with Bash's quote-escape sequences still present. The same lookup on Bash 5.3, using `BASH_ALIASES`, preserved the original body.

**Documentation:** Bash describes `alias` output as [a form reusable as shell input](https://www.gnu.org/software/bash/manual/html_node/Bash-Builtins.html), rather than a raw alias body.

**Remediation:** Decode only the supported printed representation without evaluating it, or keep aliases requiring unsupported decoding typed-only.

#### F10 · P2 · Break last-run timestamp ties deterministically

**Location:** [crates/smcli-core/src/history.rs:405](../../crates/smcli-core/src/history.rs#L405)

The grouped query uses `MAX(started_at)` with bare directory, exit-code and other metadata columns. When timestamps tie, those columns need not come from the latest inserted run. Whole-second shell timestamps make ties a supported real scenario. Incremental corpus updates can also disagree with a database rebuild.

**Evidence:** Recorded the same command first in `/old` with exit 0, then in `/new` with exit 1, both at timestamp `1700000000`. Rebuilt search retained `/old` and exit 0; `search --command cat --dir /new --json` returned no result.

**Documentation:** SQLite explicitly documents [arbitrary bare-column selection when maximum values tie](https://www.sqlite.org/lang_select.html#bareagg).

**Remediation:** Choose the latest row with a deterministic `(started_at, id)` ordering while computing usage aggregates separately or in the same structured query.

#### F11 · P2 · Preserve subsecond timestamps for filtering

**Location:** [crates/smcli-search/src/corpus.rs:111](../../crates/smcli-search/src/corpus.rs#L111)

Corpus construction formats `last_run` with whole-second precision. Search filters and indexed timestamp metadata then parse that truncated value, reversing results around fractional-second boundaries even though the CLI accepts fractional ISO 8601 filters.

**Evidence:** A run at `2023-11-14T22:13:20.500Z` was excluded by `--since 2023-11-14T22:13:20.250Z` and included by `--before 2023-11-14T22:13:20.250Z`. Both results are incorrect.

**Remediation:** Retain timestamp precision through corpus construction and filtering; presentation formatting must not determine comparison precision.

#### F12 · P2 · Reject FIFOs before reading manifests

**Location:** [crates/smcli-core/src/context.rs:45](../../crates/smcli-core/src/context.rs#L45)

`take(limit)` bounds bytes read but does not bound opening a file. Directory discovery accepts a FIFO with a manifest filename and blocks waiting for a writer. Service search takes its serialization mutex before discovery, so a blocked discovery can also stall subsequent searches.

**Evidence:** Created a FIFO named `Cargo.toml` inside an otherwise valid temporary directory. In-process `suggest --dir <directory>` did not return within a two-second timeout. The blocking `File::open` has no timeout or regular-file check.

**Remediation:** Refuse non-regular manifest and repository metadata files before opening them. Treat unsupported file types as unavailable context.

### Validation

The final isolated snapshot completed **`make test` successfully** on macOS with Rust `1.99.1`:

- 126 Rust tests passed, 0 failed, 3 ignored. Ignored tests were not claimed as executed.
- Six shell variants passed: Bash 3.2 DEBUG, Bash 5.3 PS0, Bash 5.3 DEBUG fallback, Bash 5.3 bash-preexec, Zsh and Fish.
- Shell composition, concurrent capture, interruption handling, pipeline metadata, key bindings and generated-hook syntax checks passed.
- Service integration passed, including the 50,002-file repository scenario.
- File and alias picker PTY tests passed.
- Install, upgrade-style reruns, uninstall and export checks passed on macOS.
- The benchmark smoke test passed with 10 synthetic rows and one sample.
- Native-history byte-equality checks passed.

Earlier runs were invalidated by concurrent source edits or missing Git metadata in the isolated test setup. Those runs were not used to claim a successful full gate; the final run included Git metadata and reached `smcli history tests passed.`

The original local test log is `/tmp/smcli-review-final-tests.log`. That path is ephemeral; the validation summary and reproduction observations above are retained in this report.

#### Limits

- Linux runtime validation was not performed: Docker could not connect to the stopped OrbStack daemon. F04's installer rejection was verified through the platform probe separately.
- The targeted failure reproductions are additional evidence beyond the passing repository suite. No regression tests or source fixes were added during the review.
- This report records the reviewed snapshot, not a guarantee that all possible defects have been found. Recheck each finding against the code before closing it.
