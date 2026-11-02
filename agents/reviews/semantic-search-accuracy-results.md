# Semantic search accuracy results

## Selection

**No Stage 1–2 candidate passed the zero-regression gate. Retain the current production behavior.** All 60 candidates worsened 9–11 existing expected-command ranks (including alias queries). Every non-current formula also worsened at least one held-out wanted-command rank. No candidate is eligible for production selection; a higher aggregate reproduction score does not override the gate. Production defaults therefore remain baseline, with the candidate implementations accessible through the private test-only policy override, not a runtime setting.

This is a completed comparison, not a claim that the requested search improvements have shipped. Stage 3 was not implemented. Pool tracing and latency instrumentation were omitted as requested; piterra owns black-box timing.

## Method

- Frozen nine reproduction queries (ten wanted commands), ten tuning queries and ten held-out queries (nine wanted commands; the empty-meaningful-input case contributes no wanted command).
- Indexed-local search with bundled Potion, isolated synthetic history and real temporary Makefile, justfile and package-script manifests. No service-only evidence is supplied; these are not service or Tab acceptance results.
- Original embeddings are retained. Candidate correction uses one unambiguous ASCII alphabetic distance-one neighbor only for query words of at least four characters without any global exact/permitted-prefix match. Vocabulary includes corpus lexical documents and current project candidates. Corrected BM25 is separately normalized, multiplied by 0.75 and merged by maximum relevance. Coverage preserves unmatched terms and alias-surface maximum; additions use fractional coverage.
- Head-command evidence is 0.5; plain sudo/env wrappers and leading assignments are unwrapped conservatively. Literal prefix minimum stays two. Intent prefix/stopword choices are listed below.
- Four prefix choices × five formulas × three floors = 60 candidates. Floors 0.2, 0.25 and 1/3 were evaluated independently and produced identical results for each prefix/formula pair in this fixed workload. Tables group those three equivalent rows, not untested interpolation.
- Existing regression count compares every expected-command position in the 53 original/adversarial/held-out cases plus both long-tail cases and both typed-only and expanded alias evaluation cohorts (312 query results). Missing top-ten/top-twenty results have rank infinity; reciprocal rank is zero. Forbidden top-three checks are preserved. Every prefix/formula also runs the record-time-alias and filesystem assertion groups; any failed group would add one regression. Both groups passed for every evaluated policy. The selected baseline also passes these assertions in the normal ranking/full gates.
- Alias fixtures have no project candidates, so their identical results are reused across the three floor values; every prefix/formula runs fresh retrieval. Project/natural-language retrieval runs independently for every candidate. Held-out regressions count individual worsened ranks, not aggregate MRR decreases.

Formula labels:

- **Current:** existing relevance × boost^relevance with existing failure/post-score placement, plus candidate coverage/correction/head changes.
- **Product .5:** r × (1 + 0.5 × min(e, 4)).
- **Add sqrt:** r + 0.5 × sqrt(r) × min(e, 4).
- **Add gate:** r + 0.5 × min(1, 2r) × min(e, 4).
- **Product .25:** r × (1 + 0.25 × min(e, 4)).

For the four new formulas, relevance is clamped to [0,1], positive evidence includes head and existing increments exactly once, and the success-rate multiplier applies to the whole result. No evidence is neutral and zero relevance remains zero.

## Sweep table

Metric cells are **top-1 / top-3 / MRR**. Existing regressions and held-out regressions are per candidate, not summed across the grouped floor values.

| Intent prefix minimum | Intent open stopword | Formula     | Floors tested  | Reproduction          | Tuning                | Held-out              | Existing regressions | Held-out regressions |
| --------------------- | -------------------- | ----------- | -------------- | --------------------- | --------------------- | --------------------- | -------------------- | -------------------- |
| Baseline              | No                   | Unchanged   | 1.0            | 0.100 / 0.500 / 0.387 | 0.800 / 0.900 / 0.864 | 0.444 / 0.667 / 0.565 | 0                    | 0                    |
| 2                     | true                 | Current     | 0.2, 0.25, 1/3 | 0.100 / 0.600 / 0.412 | 0.800 / 0.900 / 0.864 | 0.444 / 0.778 / 0.574 | 10                   | 0                    |
| 2                     | true                 | Product .5  | 0.2, 0.25, 1/3 | 0.100 / 0.700 / 0.428 | 0.800 / 0.900 / 0.848 | 0.444 / 0.667 / 0.565 | 9                    | 1                    |
| 2                     | true                 | Add sqrt    | 0.2, 0.25, 1/3 | 0.100 / 0.700 / 0.446 | 0.800 / 0.900 / 0.844 | 0.444 / 0.667 / 0.579 | 10                   | 1                    |
| 2                     | true                 | Add gate    | 0.2, 0.25, 1/3 | 0.200 / 0.600 / 0.468 | 0.800 / 0.900 / 0.833 | 0.444 / 0.667 / 0.576 | 11                   | 1                    |
| 2                     | true                 | Product .25 | 0.2, 0.25, 1/3 | 0.100 / 0.600 / 0.415 | 0.800 / 0.900 / 0.867 | 0.444 / 0.667 / 0.583 | 9                    | 1                    |
| 5                     | false                | Current     | 0.2, 0.25, 1/3 | 0.100 / 0.600 / 0.412 | 0.800 / 0.900 / 0.850 | 0.444 / 0.667 / 0.537 | 10                   | 1                    |
| 5                     | false                | Product .5  | 0.2, 0.25, 1/3 | 0.100 / 0.700 / 0.428 | 0.800 / 0.900 / 0.833 | 0.444 / 0.556 / 0.528 | 9                    | 2                    |
| 5                     | false                | Add sqrt    | 0.2, 0.25, 1/3 | 0.100 / 0.700 / 0.429 | 0.800 / 0.900 / 0.833 | 0.444 / 0.556 / 0.542 | 10                   | 2                    |
| 5                     | false                | Add gate    | 0.2, 0.25, 1/3 | 0.200 / 0.600 / 0.463 | 0.800 / 0.900 / 0.833 | 0.444 / 0.556 / 0.539 | 11                   | 2                    |
| 5                     | false                | Product .25 | 0.2, 0.25, 1/3 | 0.100 / 0.600 / 0.415 | 0.800 / 0.900 / 0.850 | 0.444 / 0.556 / 0.528 | 9                    | 2                    |
| 6                     | false                | Current     | 0.2, 0.25, 1/3 | 0.100 / 0.600 / 0.412 | 0.800 / 0.900 / 0.850 | 0.444 / 0.667 / 0.537 | 10                   | 1                    |
| 6                     | false                | Product .5  | 0.2, 0.25, 1/3 | 0.100 / 0.700 / 0.428 | 0.800 / 0.900 / 0.833 | 0.444 / 0.556 / 0.528 | 9                    | 2                    |
| 6                     | false                | Add sqrt    | 0.2, 0.25, 1/3 | 0.100 / 0.700 / 0.429 | 0.800 / 0.900 / 0.833 | 0.444 / 0.556 / 0.542 | 10                   | 2                    |
| 6                     | false                | Add gate    | 0.2, 0.25, 1/3 | 0.200 / 0.600 / 0.463 | 0.800 / 0.900 / 0.833 | 0.444 / 0.556 / 0.539 | 11                   | 2                    |
| 6                     | false                | Product .25 | 0.2, 0.25, 1/3 | 0.100 / 0.600 / 0.415 | 0.800 / 0.900 / 0.850 | 0.444 / 0.556 / 0.528 | 9                    | 2                    |
| 5                     | true                 | Current     | 0.2, 0.25, 1/3 | 0.100 / 0.600 / 0.412 | 0.800 / 0.900 / 0.850 | 0.444 / 0.667 / 0.537 | 10                   | 1                    |
| 5                     | true                 | Product .5  | 0.2, 0.25, 1/3 | 0.100 / 0.700 / 0.428 | 0.800 / 0.900 / 0.833 | 0.444 / 0.556 / 0.528 | 9                    | 2                    |
| 5                     | true                 | Add sqrt    | 0.2, 0.25, 1/3 | 0.100 / 0.700 / 0.446 | 0.800 / 0.900 / 0.833 | 0.444 / 0.556 / 0.542 | 10                   | 2                    |
| 5                     | true                 | Add gate    | 0.2, 0.25, 1/3 | 0.200 / 0.600 / 0.468 | 0.800 / 0.900 / 0.833 | 0.444 / 0.556 / 0.539 | 11                   | 2                    |
| 5                     | true                 | Product .25 | 0.2, 0.25, 1/3 | 0.100 / 0.600 / 0.415 | 0.800 / 0.900 / 0.850 | 0.444 / 0.556 / 0.528 | 9                    | 2                    |

## Concrete blockers

Gated additive (min(1, 2r)) moves the existing query “see what changed” from rank 1 to rank 4 and “show recent commits” from rank 7 to rank 9. The sweep also detects alias regressions, so reverting only those two cases would not make the candidate eligible. We did not change expectations, exclude unfavorable queries, or enable a runtime bypass.

## Reproduction and artifacts

Run `make test-rust PKG=smcli-search FILTER="--release semantic_policy_sweep -- --ignored --nocapture"` for the comparison. Normal `make ranking` retains the concise baseline evaluation and does not run the ignored policy sweep. Detailed sweep output contains only synthetic command text.

Raw local sweep artifact: `/tmp/semantic-policy-verified.log` (exit status 0; 60 candidates plus baseline). An initial complete sweep was repeated with expanded long-tail, record-time-alias, filesystem, and forbidden-top-three checks; all 183 metric rows were byte-for-byte identical. Earlier exploratory runs were stopped to correct the Current-formula experiment and eliminate redundant alias retrieval for floor-only changes; they are not the reported comparison.

No dependencies, protocol definitions/fixtures, stored tokenizer/index documents, native-history readers, CLI plumbing or Stage 3 code changed. No commit or push.

## Verification

- `make build`: passed.
- `make test-rust PKG=smcli-search`: passed, including focused correction/head tests and existing assertions under the retained production baseline.
- `make ranking`: passed.
- `make test`: passed (background process, exit status 0; `/tmp/semantic-full-final.log`).

## Vector-first

This new direction supersedes the earlier decision to retain hybrid search for natural-language requests. Production now uses **vector-only retrieval for intent**, with corrected query text embedded before retrieval. Literal queries retain their existing hybrid/usage pipeline. Exact typed alias invocations retain the legacy alias-aware path, and standalone uppercase identifier queries remain lexical so long-tail exact terms are not lost to embedding truncation.

### Retrieval and score

- Original corpus documents, model identity and stored vectors are unchanged; no reindex is required. Intent uses the index cosine directly, without best-hit normalization, BM25 candidates/fusion, grep expansion or the old exponent reranker.
- Current Hint/Script/Test project candidates are deduplicated, batch-embedded with the same `embedding_text` and scored by the same cosine. Stored history candidates keep their index cosine.
- Unambiguous distance-one ASCII word corrections use the corpus/project vocabulary and happen **before embedding**. Other words, filler, punctuation and case are retained; stopword stripping is not applied to the embedding query. The typo and corrected installation queries therefore embed identical text.
- Only explicit filters and resolved markers constrain intent retrieval. Automatically inferred program/directory hints no longer filter the vector pool. The lock-contention path embeds the filtered in-memory corpus and uses the same score; it is vector-only too, but costs more than reading stored vectors.
- Final intent score is `cosine + min(0.10, positive_context)`. A cosine gap greater than 0.10 cannot be reversed by context. There is no multiplicative usage rerank or service grep expansion on this path.

Selected named constants (all positive contributions share the total cap):

| Signal                                      | Weight                      | Individual cap |
| ------------------------------------------- | --------------------------- | -------------- |
| runs here                                   | 0.01 × ln(1 + runs_here)    | 0.04           |
| project runs                                | 0.01 × ln(1 + runs_project) | 0.02           |
| frecency                                    | 0.005 × ln(1 + frecency)    | 0.03           |
| exact parsed head command                   | 0.02                        | 0.02           |
| lexical query coverage                      | 0.005 × coverage            | 0.005          |
| current project Hint/Script/Test membership | 0.03                        | 0.03           |
| program available in PATH                   | 0.03                        | 0.03           |
| existing input file                         | 0.01                        | 0.01           |

The private legacy `Policy::default()` is restored to the baseline (no production fractional-additions/open-stopword/legacy scoring changes). Vector-first has its own named bounded constants; no runtime policy option was added. Head parsing remains conservative about sudo/env options and shell syntax.

### Tuning and old → new results

Replayed 1,296 bounded combinations on the fixed reproduction+tuning signals: here weight {0.01,0.02,0.04,0.06}, here cap {0.04,0.06,0.08}, head {0.02,0.03,0.06}, coverage {0.005,0.01,0.02}, frecency {0.005,0.01}, project-task {0,0.02,0.03}, total cap {0.10,0.15}. The selected compact constants achieved the best reproduction+tuning top-three count (17/20); held-out queries were not used to select constants. PATH's small weight was calibrated separately by the existing filesystem acceptance case. Final metrics below come from fresh actual searches, not score replay.

These are synthetic indexed-local measurements, not a rerun of the owner's history probe or a service/Tab timing claim. Metric cells show **top-1 / top-3 / MRR**. Missing top-ten results contribute zero reciprocal rank.

| Cohort       | Old hybrid            | Vector-first          |
| ------------ | --------------------- | --------------------- |
| reproduction | 0.100 / 0.500 / 0.387 | 0.600 / 0.800 / 0.750 |
| tuning       | 0.800 / 0.900 / 0.864 | 0.700 / 0.900 / 0.783 |
| held-out     | 0.444 / 0.667 / 0.565 | 0.667 / 0.889 / 0.759 |

**Literal regression check: 15 fixed search cases, zero worsened ranks.** Expanded-alias evaluation also improves in aggregate: MRR 0.437→0.476, top-one 0.29→0.34, top-three 0.54→0.55 (156 queries); this is not a claim of zero per-query alias-paraphrase regressions. Alias/strict-marker and filesystem tests also pass. Suggestion floors are unchanged. In accordance with the new literal-only regression requirement, the old aggregate search floor and adversarial top-one assertions now apply to literal search, not all natural-language cases; intent losses remain printed and reported below rather than hidden behind changed labels.

### Per-query ranks

`>10` means outside the returned top ten, not absent from the candidate pool.

| Cohort       | Query                         | Wanted command                                 | Old rank | New rank |
| ------------ | ----------------------------- | ---------------------------------------------- | -------- | -------- |
| reproduction | build and insall locally      | `make install`                                 | 6        | 1        |
| reproduction | build and install locally     | `make install`                                 | 2        | 1        |
| reproduction | install smcli on this machine | `make install`                                 | 2        | 1        |
| reproduction | run the tests                 | `make test`                                    | >10      | 1        |
| reproduction | format the docs               | `make docs`                                    | 2        | 2        |
| reproduction | rebuild a clean release       | `make fresh`                                   | 5        | 4        |
| reproduction | open pi with gpt 6            | `pi --model openai/gpt-6.1-sol --thinking low` | 4        | 1        |
| reproduction | start pi with sol             | `pi --model openai/gpt-6.1-sol --thinking low` | 4        | 4        |
| reproduction | commit and push               | `git commit`                                   | 2        | 2        |
| reproduction | commit and push               | `git push`                                     | 1        | 1        |
| tuning       | install this project          | `make install`                                 | 1        | 1        |
| tuning       | test this project             | `make test`                                    | 1        | 1        |
| tuning       | format documentation          | `make docs`                                    | 2        | 2        |
| tuning       | fresh release                 | `make fresh`                                   | 1        | 3        |
| tuning       | just lint                     | `just lint`                                    | 1        | 1        |
| tuning       | npm run storybook             | `npm run storybook`                            | 1        | 1        |
| tuning       | pi                            | `pi`                                           | 1        | 1        |
| tuning       | open                          | `open .`                                       | 1        | 1        |
| tuning       | sto                           | `npm run storybook`                            | 7        | >10      |
| tuning       | insall                        | `make install`                                 | 1        | 1        |
| held-out     | install locally               | `make install`                                 | 1        | 1        |
| held-out     | run project tests             | `make test`                                    | >10      | 1        |
| held-out     | format docs                   | `make docs`                                    | 2        | 2        |
| held-out     | clean release                 | `make fresh`                                   | 3        | 3        |
| held-out     | just lin                      | `just lint`                                    | 4        | 1        |
| held-out     | npm run sto                   | `npm run storybook`                            | 1        | 1        |
| held-out     | oc mini                       | `oc mini -m openai/gpt-6.1-sol`                | 1        | 1        |
| held-out     | tost                          | `make test`                                    | >10      | >10      |
| held-out     | gst                           | `gst`                                          | 1        | 1        |

The empty-meaningful-input case has no wanted label and is excluded from metric denominators. Two reproductions remain fourth (`rebuild a clean release`, `start pi with sol`). Tuning top-one/MRR decline because `fresh release` moves 1→3 and `sto` loses its lexical-prefix advantage (7→outside ten). This change does not claim every natural-language query improves.

### Existing natural-language trade-offs

The full fixed query set also records these changed ranks; literal ranks are enforced separately.

| Query                                 | Old rank | New rank |
| ------------------------------------- | -------- | -------- |
| how do i create a new bun project     | 1        | 2        |
| how do i build my smcli project       | 1        | 4        |
| bar tests                             | 1        | 2        |
| undo the last commit but keep changes | 3        | 4        |
| show recent commits                   | 7        | 10       |
| find the biggest files here           | 3        | 1        |
| unpack a tarball                      | 2        | 1        |

### Verification and artifacts

- `make build`: passed.
- `make test-rust PKG=smcli-search`: passed.
- `make ranking`: passed, including alias and filesystem cases.
- `make test`: passed in the background on the final behavior (exit status 0; `/tmp/vector-first-full-final.log`).
- Added small tests for corrected embedding text, absolute-cosine parity between indexed/held-lock paths, bounded boosts, stable ties, and no intent grep expansion. The former expansion test explicitly exercises the retained legacy pipeline.
- Reproduce the old/new comparison with `make test-rust PKG=smcli-search FILTER="--release vector_first_comparison -- --nocapture"`; it is also a normal unit test and asserts literal rank parity.

Final comparison: `/tmp/vector-first-final-compare.log`. Tuning signals: `/tmp/vector-first-tuning.log`. Gates: `/tmp/vector-first-build.log`, `/tmp/vector-first-tests.log`, `/tmp/vector-first-ranking.log`, `/tmp/vector-first-full-final.log`.

Pool tracing, latency instrumentation, owner-history access and Stage 3 remain skipped. No protocol/version/fixture, tokenizer/index format, dependency, shell hook or native-history writer changed. No commit or push. Lock contention now trades additional embedding work for vector semantics; black-box timing remains a separate check.

## Project config grep

On top of vector-first, retained **keyword-only project description evidence**. Literal search, suggestions, stored corpus/model embeddings and the protocol are unchanged. No runtime option or dependency was added.

### Text-only extraction and score

`Candidate.detail` holds at most 400 Unicode characters. Makefile/GNUmakefile and justfile descriptions contain contiguous comments directly above the declaration plus its recipe text; package.json descriptions contain the script command string; Package.swift descriptions contain literal target/product names and constructor kinds. Existing safe command-name validation remains authoritative. Reads use the existing bounded regular-file reader (1 MiB maximum); shell text is never executed, copied into a suggested command, or stored in the history index.

For natural-language requests only, the same shell tokenizer splits corrected query text and descriptions, case-insensitively. Stopwords and duplicate query terms are dropped. Matching is exact or a query prefix of at least four characters (`format` matches `Formats`). For each term, within that manifest's described candidates, rarity is `1 - matched_candidates / candidates`. The match score is the sum of matched-term rarities divided by the number of meaningful query terms. A term shared by all described candidates contributes zero, so the common-word decoy receives no description boost. A file with only one described candidate is neutral under this per-file rarity rule.

The named `INTENT_CONFIG_WEIGHT = 0.12` multiplies a score clamped to [0,1]. It is **separate** from the previous context cap of 0.10: final intent score is `cosine + min(0.10, context) + min(0.12, config_match)`. No best-hit score is copied onto a keyword hit, and keyword matches do not invoke BM25 retrieval or repository grep. Descriptions are tokenized once and document frequencies are counted per query term/file, rather than rescanning every sibling for every candidate.

Project candidates already join the vector pool independently of the stored-history cutoff. Thus a description match can still participate when its command fell below that cutoff, using its own command cosine plus only these bounded contributions. It is not promoted to the strongest vector's similarity.

### Old → new

Baseline means the preceding vector-first implementation with the new fixture labels/comments present but config boosting disabled. Cells show **top-1 / top-3 / MRR**. The fixture now mirrors the real comments for fresh/install/docs and adds a common-word-only `routine` decoy.

| Cohort         | Vector-first baseline | + project config keywords |
| -------------- | --------------------- | ------------------------- |
| reproduction   | 0.600 / 0.800 / 0.750 | 0.600 / 0.900 / 0.758     |
| tuning         | 0.700 / 0.900 / 0.783 | 0.700 / 0.900 / 0.783     |
| held-out       | 0.667 / 0.889 / 0.759 | 0.667 / 0.889 / 0.759     |
| project-config | 0.333 / 0.667 / 0.583 | 0.667 / 1.000 / 0.833     |

| Added query                               | Wanted command | Old rank | New rank |
| ----------------------------------------- | -------------- | -------- | -------- |
| clean rebuild from scratch                | `make fresh`   | 1        | 1        |
| install the release build on this machine | `make install` | 4        | 2        |
| format every hand-written markdown doc    | `make docs`    | 2        | 1        |

The existing `rebuild a clean release` label improves 4→3; the other 28 preceding NL labels do not worsen. The comparison asserts rank non-regression for all 32 current NL labels (including the three new ones) and all 35 fixed search cases, not just the literal subset. Existing ranking assertions were not weakened for this change. `start pi with sol` remains fourth; config text does not solve every semantic miss.

### Rejected embedding experiment

Also tried embedding `command + detail` for the project candidates already embedded at query time. It produced no additional reproduction/tuning/new-query benefit but worsened held-out top-three **0.889→0.778** and MRR **0.759→0.750**: `clean release` moved 3→4. That code was removed; production and the retained comparison embed bare commands as before. The exploratory three-mode output is preserved at `/tmp/config-compare.log`.

### Verification

- `make build`: passed.
- `make test-rust PKG=smcli-core`: passed, including frozen protocol fixtures.
- `make test-rust PKG=smcli-search`: passed.
- `make ranking`: passed.
- `make test`: passed in the background, exit status 0 (`/tmp/config-full.log`).
- Added extraction tests for all requested formats, contiguous comments/recipes, Unicode length bounds, malformed JSON and shell text kept inert. Scoring tests cover correction, prefix length, stopwords, rarity, per-file isolation, common-word decoy and disabling script evidence.
- Reproduce the baseline/keyword comparison with `make test-rust PKG=smcli-search FILTER="--release project_config_comparison -- --nocapture"`; it is also a normal unit test with the non-regression assertions above.

Artifacts: `/tmp/config-final-compare.log`, `/tmp/config-build.log`, `/tmp/config-core.log`, `/tmp/config-search.log`, `/tmp/config-ranking.log`, `/tmp/config-full.log`.

No commit or push. Owner-history validation and latency instrumentation remain outside this change. Extraction is intentionally a bounded text reader, not an interpreter for computed Make/just/Swift declarations.

## Config boost tuning (real history)

### Bench measurement (0.18 under a no-loss rule)

An earlier owner-history sweep selected **0.18 (1.5× the previous 0.12)** under a strict no-loss criterion. That measurement is retained below as historical evidence; it is **not** the shipped constant.

| Weight      | NL wanted in top 3 | MRR   | NL rank losses vs 1× | Changed literal controls | Eligible under no-loss |
| ----------- | ------------------ | ----- | -------------------- | ------------------------ | ---------------------- |
| 0.12 (1×)   | 5/13               | 0.394 | 0                    | 0                        | yes                    |
| 0.18 (1.5×) | 6/13               | 0.406 | 0                    | 0                        | measured selection     |
| 0.24 (2×)   | 6/13               | 0.396 | 1                    | 0                        | no                     |
| 0.36 (3×)   | 8/13               | 0.468 | 1                    | 0                        | no                     |
| 0.48 (4×)   | 8/13               | 0.558 | 1                    | 0                        | no                     |
| 0.72 (6×)   | 8/13               | 0.558 | 1                    | 1                        | no                     |

At 0.18, `format the docs` → `make docs` improves **4→3** and `rebuild a clean release` → `make fresh` improves **8→5**. Values 2× and above make `run the tests` worse (1→2); 6× also changes the `cargo test` control top-three.

### Shipped value (0.48)

Current main ships `INTENT_CONFIG_WEIGHT = 0.48` (`crates/smcli-search/src/corpus.rs`, documented in `crates/smcli-search/README.md`), for a separate config cap of 0.48 and a combined positive context/config bound of 0.58. That choice accepts the measured regression of `run the tests` (1→2) from the table above in exchange for higher aggregate NL hit rate and MRR. Do not treat the 0.18 no-loss validation claims as evidence for 0.48 behavior; the private owner-history benchmark was not rerun for this documentation correction.

`make release`, `make test-rust PKG=smcli-search` and `make ranking` passed on the final override-free build that first measured 0.18. `make test` passed in the background (exit status 0). No latency claims.

The full value-by-query rank table and private literal-control arrays are in the scratch report `scratchpad/bench/results5.md` and `scratchpad/bench5/`; no private retrieved command text is copied here. Gate logs: `/tmp/config5-release-final.log`, `/tmp/config5-search-final.log`, `/tmp/config5-ranking-final.log`, `/tmp/config5-full-final.log`.
