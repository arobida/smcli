# Port the command search index from ProximaKit to TalaDB

Branch: `taladb-port` (experimental). Gate to merge: `make test` green and `make ranking` no worse than `main`.

## Today (ProximaKit 1.8)

All index code lives in `Sources/SmcliSearch` behind `#if canImport(ProximaKit)`:

| Piece | Where | ProximaKit API used |
|---|---|---|
| Hybrid store, one directory per model (`commands-v7-…`) | `HistorySearch.swift` `openStore`, `sync`, `search`, `health`, `reindex` | `HybridVectorStore.open/addChunks/removeDocument/query/save/documentIds`, `ChunkMetadata`, `index.setFusion(.rrf / .weightedSum(alpha: 0.3))`, `WALCheckpointPolicy` |
| Shell-aware BM25 terms | `ShellTokenizer` | `BM25Tokenizer` |
| Embedders | `CommandEmbedder` (Apple), `PotionEmbedder` (`Potion.swift`), `OllamaEmbedder` (`Ollama.swift`) | `TextEmbedder`, `Vector`, `NLEmbeddingProvider` (ProximaEmbeddings) |
| Callers guarded by `canImport(ProximaKit)` | `SearchSetup.swift`, `smcli.swift`, `Picker.swift`, `SmcliService.swift`, tests | — |

Filters (`SearchFilter`) run in Swift **after** ranking, so a filtered search pulls `k = documents.count`, i.e. ranks the whole index.

## TalaDB facts this plan relies on

Confirmed in chunk 0 against `.build/checkouts/taladb-swift` (tag v0.1.1, engine `TalaDBFFI-0.12.0.xcframework`, ~73 MB download) and the engine source (`taladb/taladb` `packages/core/src/fts.rs`).

- SPM: `.package(url: "https://github.com/taladb/taladb-swift", from: "0.1.1")`, product `TalaDB`, package identity `taladb-swift`. Package platforms macOS 10.15+. Every call is `async throws` (`TalaDBError`: `.engine(String)`, `.closed`, `.invalidArgument`, `.decoding`, `.incompatibleEngine`).
- Open/close: `TalaDB.open(at: URL, config: TalaDBConfig = .init(passphrase: nil, flushEveryWrite: true), migrations: []) async throws -> TalaDB`; `db.close()` (sync, blocks until in-flight ops finish, idempotent; `deinit` also closes); `db.isClosed`, `flush()`, `compact()`.
- `db.collection("name", as: T.self) -> TalaCollection<T>` (`T: Codable & Sendable`, must encode to a JSON object; `_id` via `CodingKeys case id = "_id"`, nil ⇒ engine ULID; a supplied `_id` must be a ULID). No I/O until first write.
- Filter/update type: `typealias Filter = [String: JSONValue]`, `Update = [String: JSONValue]`. `JSONValue` is an enum (`null bool int(Int64) double string array object`) with all literal conformances, so `["runs": ["$gte": 3]]` works; non-literals need `.string(s)`, `.double(d)`, `.int(Int64(n))`, `.array(xs.map(JSONValue.string))`. `$and` is `["$and": .array([.object(f1), .object(f2)])]`.
- Documents: `insert(_:) -> String` (`@discardableResult`), `insertMany(_:) -> [String]`, `find(_ filter: Filter = [:]) -> [T]` (**no projection**), `findOne`, `count(_ filter: Filter = [:]) -> Int`, `updateOne(_ filter:, _ update:) -> Bool`, `updateMany -> Int`, `deleteOne -> Bool`, `deleteMany(_ filter:) -> Int`, `aggregate([JSONValue]) -> [JSONValue]`.
- Indexes: `createIndex(_ field:)` (B-tree, no-op if exists), `createCompoundIndex([String])`, `createFtsIndex(_ field:)`, `createVectorIndex(_ field:, dimensions: Int, metric: VectorMetric = .cosine, hnsw: HnswOptions? = nil)`; `VectorMetric` `.cosine .dot .euclidean`; `HnswOptions(m: 32, efConstruction: 200, quantization: .unquantized)`. **Default (`hnsw: nil`) is exact flat search**, which the docs say is faster below tens of thousands of vectors. `listIndexes() -> IndexInfo{btree, fts, vector}`.
- `findNearest(_ field:, vector: [Float], topK: Int, filter: Filter? = nil) -> [ScoredDocument<T>]` (`document`, `score: Double`); filter narrows candidates before ranking. `searchVectors(... options: VectorQueryOptions)` for mode/efSearch/threshold.
- `searchText(_ field:, query:, topK:, filter:, options: Bm25Options(k1:, b:)) -> [ScoredDocument<T>]`, BM25, OR semantics.
- `hybridSearch(textField:, text:, vectorField:, vector: [Float], topK:, filter: Filter? = nil, options: HybridOptions = .init()) -> [HybridHit<T>]`; `HybridOptions(rrfK: Double?, textWeight: Double?, vectorWeight: Double?, candidates: Int?, bm25: Bm25Options)` (nil ⇒ engine default); `HybridHit` has `document`, fused `score: Double`, `textRank: Int?`, `vectorRank: Int?`.
- FTS tokenizer (fixed, not configurable): split on `!char.is_alphanumeric()` (Unicode), drop tokens with < 2 **bytes**, lowercase. No max token length (a 320-char token matched in the spike). A short English stopword list exists for queries only (`an the and or …`); hex-encoded `t…` tokens can never hit it. Spike: `git --amend -m x` indexes `git`, `amend`; `m` finds nothing.
- Cross-process: a second process's `TalaDB.open` on a file already open elsewhere **fails immediately** (no blocking) with `TalaDBError.engine("storage error: Database already open. Cannot acquire lock.")` (redb file lock). Measured in a two-process spike.

## Design

### Storage
One TalaDB file per index name: `history.db.search/commands-v8-<model>-d<dim>.taladb`, collection `commands`. Bumping `version` to `commands-v8` makes `removeOldIndexes()` delete the old ProximaKit `commands-v7-*` directories; nothing to migrate, the index is a rebuildable cache.

Document (`IndexedCommand: Codable`):

| Field | Type | Index | Purpose |
|---|---|---|---|
| `doc` | String | B-tree | `searchDocument(entry, labeled:)`, the identity today; changes ⇒ re-embed |
| `command` | String | — | maps a hit back to `corpus` |
| `terms` | String | FTS | `ShellTokenizer` output, encoded (below) |
| `embedding` | [Float] | vector, cosine, `dimension` (flat/exact, no HNSW) | embedding of `doc` |
| `program` | String? | B-tree | filter `program` |
| `subcommand` | String? | B-tree | filter `subcommand` |
| `flags` | [String] | — | filter `flag` (contains) |
| `author` | String? | B-tree | filter `author` |
| `dirs` | [String] | — | `lastDirectory` and every ancestor; filter `directory` (contains) |
| `lastRun` | Double? | B-tree | filter `since` / `before` |
| `runs` | Int | — | filter `minRuns` |
| `meta` | String | — | fingerprint (joined string, not a hash: Swift's `Hasher` is per-process) of the metadata fields, to detect changes without re-embedding |

### Tokenizer
TalaDB's tokenizer would turn `--amend` into `amend` and drop `-m`. `ShellTokenizer` stays the source of terms; each term is stored as `t` + lowercase hex of its UTF-8 bytes (all alphanumeric, never 1 char), and the query text goes through the same encoding. BM25 then scores exactly ShellTokenizer's terms. `ShellTokenizer` loses its `BM25Tokenizer` conformance and gains `encoded(_:) -> String`.

### Embedders
`TextEmbedder` and `Vector` came from ProximaKit. Add a package protocol in SmcliSearch:

```swift
package protocol TextEmbedder: Sendable {
    var dimension: Int { get }
    func embed(_ text: String) async throws -> [Float]
    func embedBatch(_ texts: [String]) async throws -> [[Float]]   // default: loop over embed
}
```

`PotionEmbedder` and `OllamaEmbedder` return `[Float]` (they already build from `[Float]`). `CommandEmbedder` wraps `NLEmbedding.sentenceEmbedding(for: .english)` directly (`vector(for:)` gives `[Double]` → `[Float]`), replacing `NLEmbeddingProvider`. The fixed fallback vector stays.

### Metadata filters
`SearchFilter` maps to a TalaDB filter passed to `hybridSearch`, so filtering happens before ranking and `k` no longer needs to be the whole index:

| `SearchFilter` | TalaDB filter |
|---|---|
| `program` | `{"program": p}` |
| `subcommand` | `{"subcommand": s}` |
| `author` | `{"author": a}` |
| `flag` | `{"flags": {"$in": [f]}}` |
| `directory` | `{"dirs": {"$in": [base]}}` (trailing `/` trimmed as today) |
| `since` / `before` | `{"lastRun": {"$gte": since, "$lt": before}}` |
| `minRuns` | `{"runs": {"$gte": n}}` |

Combined with `$and`. `filter.allows(entry)` stays as a post-check against the live corpus (metadata in the index can be one sync behind). `k` becomes `max(limit * 4, 40)` always.

### Search, fusion and scores
- Query embedding from `makeEmbedder()`, then `hybridSearch(textField: "terms", text: encoded(query), vectorField: "embedding", vector:, topK: k, filter:, options:)`.
- Literal queries (today `.weightedSum(alpha: 0.3)`): **deviation (chunk 2)** — weighted RRF (`textWeight: 0.7, vectorWeight: 0.3`, any `rrfK`) was tried and fails: RRF leaves the runner-up nearly level with an exact keyword hit, so usage/picks decide (`test-service.py` "fresh record is searchable" broke; `rrfK` 1–40 hurt `make ranking`). Literal queries instead run `searchText` + `findNearest` (same filter, same k) and sum `0.7·bm25/best + 0.3·cosine/best`, a direct port of the weighted sum. Intent queries: `hybridSearch` with engine defaults. `make ranking` search: MRR 0.955, top1 0.91, top3 1.00 (n=11).
- Relevance stays `score / best` so `rerank` sees the same 0…1 range.

### Sync
Same shape as today: index keyed by `doc`; delete docs whose `doc` left the corpus (`deleteMany {"doc": {"$in": stale}}`); embed and insert missing in batches of 256; for kept docs whose `meta` hash changed, `updateOne` metadata only (no re-embed). Skip all writes when nothing changed so `OpenIndex` stays valid.

### Concurrency and reuse
- Keep `flock` on `lock` around every open/write. It serialises CLI and service; TalaDB's own lock only makes the loser fail.
- `OpenIndex` keeps the index name, stamp, and each indexed doc's `meta` (so an unchanged sync skips `find()`, which decodes every embedding), not a live `TalaDB` handle (see decision below); `stamp()` now stats the single `.taladb` file (and any sidecars) instead of walking a directory.
- **Decision (chunk 0):** redb refuses a second open outright ("Database already open. Cannot acquire lock."), so a handle may only be held while holding `flock`. A long-lived service handle would make every CLI fallback fail, so: every user (CLI and service) opens, works and `close()`s the handle inside one `flock` section; `OpenIndex` caches the corpus/stamp, not the open handle. Open cost is ~40 ms on a small file (measure on a real index in chunk 3; if too slow, switch to "service holds the handle, CLI fallback treats the open error as `index busy` and searches without the index").
- Treat the open error as transient: if `open` throws the lock error despite `flock` (e.g. a stale pre-flock process), retry once after a short wait, then report it.

### Health and reindex
- `health`: `count({})` for `indexed`; `find([:])` (no projection in the API, so it decodes whole docs incl. embeddings; acceptable for health) for missing/stale; `bytes`/`lastUpdated` from the file.
- `reindex`: delete the file, open, sync.

## Chunks (one subagent each, run in order — they share `HistorySearch.swift`)

0. **Dependency + API spike.** Swap ProximaKit for taladb-swift in `Package.swift` (macOS-only condition kept). Read the resolved package source; correct the "TalaDB facts" section with exact signatures, the filter value type, and the two-process open behaviour (small throwaway test). No smcli logic changes beyond what makes it compile-blocked-but-honest.
1. **Embedders.** Add `TextEmbedder`, port `PotionEmbedder`, `OllamaEmbedder`, `CommandEmbedder` to `[Float]`; replace `NLEmbeddingProvider`. Switch every `#if canImport(ProximaKit)` to `#if canImport(TalaDB)`.
2. **Store, filters, sync.** Implement `IndexedCommand`, tokenizer encoding, `SearchFilter → filter`, `openStore/sync/search/health/reindex` on TalaDB, bump to `commands-v8`. Unit tests for the filter mapping and the token encoding.
3. **Verify and document.** `make test`, `make ranking` vs `main` (record both scores here), tune fusion weights, update `THIRD_PARTY_NOTICES.md`, README caveats, `graphify update .`.

## Risks
- taladb-swift is 0.1.x and ships a prebuilt binary: API churn, and the binary must be checked into notices/licences.
- No custom tokenizer: the hex encoding is a workaround; if TalaDB caps token length, long tokens need hashing instead.
- Weighted RRF is not the same as ProximaKit's weighted score sum: literal-query ranking may shift; `make ranking` is the gate.
- One process at a time per index file (chunk 0): open/close per operation costs latency on every search.

## Chunk 3 results (2026-10-21)

**Ranking (`make ranking`).** taladb-port: suggest MRR 1.000 (n=16); search MRR 0.955, top1 0.91, top3 1.00 (n=11).
main: suggest 1.000; search MRR 0.939, top1 0.91, top3 1.00. The one miss on both is `"swift test"` in api/Sources/App
wanting `swift test --filter Lib` (rank 2 here, rank 3 on main). No fusion-weight tuning needed: the port is not worse.

**Latency** (release builds, 4,000 synthetic zsh-history commands, temp HOME/SMCLI_DB/SMCLI_RUNTIME_DIR, in-process
CLI `smcli search`, wall time incl. process start). The service could not be started in the agent sandbox
(`smcli service start` exits 1 on both branches), so only the in-process path was measured.

| | cold (build index) | warm, first seconds after build | warm, steady |
|---|---|---|---|
| taladb-port | 1.8–3.2 s | ~250 ms | **~1.6–1.8 s** |
| main (ProximaKit) | 5.8 s | ~52 ms | ~52 ms (≈420 ms after a failed `service start`) |

- Index file: 35.9 MB for 4,000 commands (TalaDB stores the 256-d vectors as JSON docs).
- Steady warm cost is almost all `sync`'s `commands.find()` with `known == nil` (every in-process search): `sample`
  shows ~97% in taladb-swift `decodeJSON` → `JSONNumberScanner` decoding every document's embedding. `health` costs
  the same (~1.5 s). This is a large regression for the CLI fallback and the picker without a service. Options:
  store `meta` in a second small collection (or a vector-free projection, if taladb-swift adds one) so sync reads only
  doc→meta; or let the in-process path skip sync when the stamp matches a stamp file written after the last sync.
  Not done here: it is a design change, not the minimal cache fix.
- **mtime / sidecars:** no sidecar files are created (only the `.taladb` file and `lock`). Every open+close —
  including read-only `health` — rewrites the `.taladb` mtime (size unchanged). This does **not** defeat the
  `OpenIndex` cache: `search` checks the stamp before opening and records it after `close()`, so the service's own
  open/close is absorbed; only another process's open (CLI fallback, `health`) invalidates it, which is the intended
  behaviour, though now even a read-only `health` forces the service to re-read all docs on its next search.
- Service warm latency (which uses the cache and skips `find()`) is still unmeasured; measure outside the sandbox.

**Docs.** THIRD_PARTY_NOTICES.md regenerated (scripts/gen-notices.py now emits taladb-swift 0.1.1 MIT and the
TalaDBFFI v0.12.0 xcframework; the xcframework has no licence file and its compiled Rust crates are an open item).
docs/baseline.md ProximaKit mentions updated. No README mentions existed.
