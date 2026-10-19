import Foundation
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif

/// A distinct command with whatever is known about its runs.
struct SearchEntry: Codable, Equatable {
    let command: String
    /// Runs recorded by capture hooks, or occurrences in native history when never captured.
    let runs: Int
    let lastRun: String?
    let lastDirectory: String?
    let lastExitCode: Int?
    /// "captured" when metadata exists, otherwise "native".
    let source: String
    /// From the most recent captured run, when recorded.
    var author: String? = nil
    var intent: String? = nil
    /// Option names (`--amend`, `-m`), from the database for captured commands.
    var flags: [String] = []
    /// Runs decayed by age (half weight every two weeks), successful runs, and runs in the searcher's directory or below (captured only).
    var frecency: Double = 0
    var successes: Int = 0
    var runsHere: Int = 0
    /// Why search ranked this command where it did; set only on search results.
    var why: [String]? = nil
}

/// True when a command line runs smcli itself (`smcli …`, `/path/to/smcli …`, `swift run smcli …`),
/// so searches and suggestions are not about the tool. Leading `VAR=value` assignments are skipped.
func isSmcliCommand(_ command: String) -> Bool {
    var words = command.split(whereSeparator: \.isWhitespace).map { $0.trimmingCharacters(in: CharacterSet(charactersIn: "'\"")) }
    while let first = words.first, let equals = first.firstIndex(of: "="), equals != first.startIndex,
          !first[..<equals].contains("/") {
        words.removeFirst()
    }
    // `swift run` options may take values (`-c release`), so look for the product among the next few words.
    if words.first == "swift" && words.dropFirst().first == "run" {
        return words.dropFirst(2).prefix(5).contains("smcli")
    }
    return words.first.map { $0.split(separator: "/").last == "smcli" } ?? false
}

/// Merges captured stats and native history lines into one entry per distinct command.
/// Native history keeps no record boundaries we can trust, so each line is one entry.
func searchCorpus(captured: [String: CommandStats], native: [String]) -> [String: SearchEntry] {
    let formatter = ISO8601DateFormatter()
    var corpus: [String: SearchEntry] = [:]
    for (command, stats) in captured where !isSmcliCommand(command) {
        corpus[command] = SearchEntry(
            command: command, runs: stats.runs,
            lastRun: formatter.string(from: Date(timeIntervalSince1970: stats.lastRun)),
            lastDirectory: stats.lastDirectory, lastExitCode: stats.lastExitCode, source: "captured",
            author: stats.author, intent: stats.intent, flags: stats.flags,
            frecency: stats.frecency, successes: stats.successes, runsHere: stats.runsHere)
    }
    // Native lines have no times, but files are oldest first, so position stands in for age: a run's weight
    // halves every `nativeHalfLife` lines from the end. Captured commands keep whichever evidence is
    // stronger, so turning capture on does not throw away years of native habit.
    let lines = native.map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty && !isSmcliCommand($0) }
    var nativeCounts: [String: Int] = [:]
    var nativeUsage: [String: Double] = [:]
    for (index, command) in lines.enumerated() {
        nativeCounts[command, default: 0] += 1
        nativeUsage[command, default: 0] += pow(0.5, Double(lines.count - 1 - index) / nativeHalfLife)
    }
    for (command, count) in nativeCounts {
        let usage = nativeUsage[command] ?? 0
        if let entry = corpus[command] {
            corpus[command]?.frecency = max(entry.frecency, usage)
        } else {
            corpus[command] = SearchEntry(command: command, runs: count, lastRun: nil, lastDirectory: nil,
                                          lastExitCode: nil, source: "native", flags: commandFlags(command), frecency: usage)
        }
    }
    return corpus
}

/// Native history lines after which a run counts half; about two weeks for a busy shell, like captured usage.
let nativeHalfLife = 1_000.0

extension SearchEntry {
    /// The executable's name without its path (`git` for `/usr/bin/git git status`); for compound
    /// shell lines the parser refuses, the first word.
    var program: String? { Self.program(of: command, parsed: CommandBreakdown.parse(command)) }

    /// `program` from an existing parse, for callers that also need other parts of it.
    static func program(of command: String, parsed: CommandBreakdown?) -> String? {
        if let main = parsed?.mainCommand { return main.split(separator: "/").last.map(String.init) ?? main }
        return command.split(whereSeparator: \.isWhitespace).first.map { String($0.split(separator: "/").last ?? $0) }
    }
    var subcommand: String? { CommandBreakdown.parse(command)?.subcommand }
    var lastRunEpoch: Double? { lastRun.flatMap { ISO8601DateFormatter().date(from: $0)?.timeIntervalSince1970 } }
}

/// The text embedded for an entry: the command plus labeled fields, so queries naming a tool or option
/// land near the right commands. Directories are left out; `--dir` and the "used here" boost cover them. Time and frequency are left to `SearchFilter`: as labels
/// they did not improve ranking and would force re-embedding as they change.
/// With `labeled` false (static models such as Potion, which mean-pool tokens and would dilute the command
/// with repeated names and label words), only the command and its recorded intent are embedded.
func searchDocument(_ entry: SearchEntry, labeled: Bool = true) -> String {
    // Long commands (heredocs, pasted scripts) are cut so they fit embedding contexts with their labels intact.
    var lines = [entry.command.count > 1_500 ? String(entry.command.prefix(1_500)) : entry.command]
    guard labeled else {
        if let intent = entry.intent, !intent.isEmpty { lines.append(intent.replacingOccurrences(of: "\n", with: " ")) }
        return lines.joined(separator: "\n")
    }
    let parsed = CommandBreakdown.parse(entry.command)
    if let program = SearchEntry.program(of: entry.command, parsed: parsed) { lines.append("command: \(program)") }
    if let subcommand = parsed?.subcommand { lines.append("subcommand: \(subcommand)") }
    if !entry.flags.isEmpty { lines.append("flags: \(entry.flags.joined(separator: " "))") }
    if let author = entry.author, !author.isEmpty { lines.append("author: \(author)") }
    // Intent stays out of the label list in ShellTokenizer: its words are worth matching as keywords.
    if let intent = entry.intent, !intent.isEmpty { lines.append("intent: \(intent.replacingOccurrences(of: "\n", with: " "))") }
    return lines.joined(separator: "\n")
}

/// Hard filters applied to search results. Time bounds apply to a command's most recent run,
/// so commands from native history (which has no times) never pass them.
struct SearchFilter: Equatable {
    var program: String?
    var subcommand: String?
    /// Unix epoch seconds; `since` is inclusive, `before` exclusive.
    var since: Double?
    var before: Double?
    var minRuns: Int?
    var author: String?
    /// The last run's directory is this one or below it.
    var directory: String?
    /// The command uses this option, by exact name (`-m` does not match `--message`).
    var flag: String?

    var isEmpty: Bool { self == SearchFilter() }

    func allows(_ entry: SearchEntry) -> Bool {
        if let program, entry.program != program { return false }
        if let subcommand, entry.subcommand != subcommand { return false }
        if let minRuns, entry.runs < minRuns { return false }
        if let author, entry.author != author { return false }
        if let flag, !entry.flags.contains(flag) { return false }
        if let directory {
            let base = directory.count > 1 && directory.hasSuffix("/") ? String(directory.dropLast()) : directory
            guard let last = entry.lastDirectory,
                  last == base || last.hasPrefix(base == "/" ? "/" : base + "/") else { return false }
        }
        if since != nil || before != nil {
            guard let last = entry.lastRunEpoch else { return false }
            if let since, last < since { return false }
            if let before, last >= before { return false }
        }
        return true
    }

    /// Parses `30m`, `12h`, `7d`, `2w` (that long before `now`), a local `YYYY-MM-DD` date, or an ISO 8601 time.
    static func time(_ text: String, now: Double) throws -> Double {
        let units: [Character: Double] = ["m": 60, "h": 3_600, "d": 86_400, "w": 604_800]
        if let unit = text.last.flatMap({ units[$0] }), let count = Double(text.dropLast()), count >= 0 {
            return now - count * unit
        }
        let day = DateFormatter()
        day.locale = Locale(identifier: "en_US_POSIX")
        day.dateFormat = "yyyy-MM-dd"
        if let date = day.date(from: text) ?? ISO8601DateFormatter().date(from: text) { return date.timeIntervalSince1970 }
        throw HistoryError(message: "Invalid time \(text); use 30m, 12h, 7d, 2w, YYYY-MM-DD, or an ISO 8601 time.")
    }
}

/// How literal a query is. Flags, paths, revisions, assignments, variables, and commands from the history
/// carry exact constraints the keyword leg honors; plain words describe intent for embeddings.
/// `programs` maps each program in the history to the words seen right after it (`git` → `stash`, `log`).
func isLiteralQuery(_ query: String, programs: [String: Set<String>]) -> Bool {
    let words = query.split(whereSeparator: \.isWhitespace)
    guard let first = words.first else { return false }
    if words.contains(where: { word in
        word.hasPrefix("-") || word.contains("/") || word.contains("=") || word.hasPrefix("~")
            || word.contains(where: { "~^@|*{}".contains($0) }) || word == "HEAD"
            || (word.hasPrefix("$") && word.count > 1) || word.dropFirst().dropLast().contains(":")
            || (word.contains(".") && !word.hasSuffix("."))
    }) { return true }
    // A short query that reads as a command from the history (`git stash`, `make test`) is literal. Many
    // programs are also verbs (find, make, open), so the next word must be one seen after that program:
    // "find large files" is a request.
    guard words.count <= 4, let next = programs[first.lowercased()] else { return false }
    return words.count == 1 || next.contains(words[1].lowercased())
}

/// Each program in `corpus` with the words that follow it, for `isLiteralQuery`.
func programArguments(_ corpus: [String: SearchEntry]) -> [String: Set<String>] {
    var programs: [String: Set<String>] = [:]
    for entry in corpus.values {
        guard let program = entry.program?.lowercased() else { continue }
        let words = entry.command.split(whereSeparator: \.isWhitespace)
        let index = words.firstIndex { $0.split(separator: "/").last.map { $0.lowercased() == program } ?? false }
        let next = index.flatMap { words.indices.contains($0 + 1) ? words[$0 + 1].lowercased() : nil }
        programs[program, default: []].formUnion(next.map { [$0] } ?? [])
    }
    return programs
}

/// Reorders hybrid hits with usage evidence. `relevance` is each hit's fused score scaled so the best is 1.
/// Usage multiplies it as `boost ^ relevance`, so a habit lifts a strong match nearly fully but barely moves a
/// weak one: rank fusion leaves even distant dense neighbours a third of the best score, and a plain product
/// would let frequent unrelated commands outrank real matches.
/// Failures lower the score without excluding: the failed command may be exactly what is wanted.
func rerank(_ hits: [(entry: SearchEntry, relevance: Double)], literal: Bool, query: String,
            now: Double = Date().timeIntervalSince1970) -> [SearchEntry] {
    let terms = Set(query.lowercased().split(whereSeparator: \.isWhitespace).map(String.init))
    return hits.map { hit -> (SearchEntry, Double) in
        var entry = hit.entry
        var why: [String] = []
        let words = Set(entry.command.lowercased().split(whereSeparator: \.isWhitespace).map(String.init))
        let exact = !terms.isEmpty && terms.isSubset(of: words)
        why.append(exact ? "exact words" : literal ? "keyword match" : "semantic match")
        var boost = 1 + 0.15 * log1p(entry.frecency)
        if entry.runsHere > 0 {
            boost += 0.35 * min(1, Double(entry.runsHere) / Double(max(1, entry.runs)) + 0.25)
            why.append("used here")
        }
        if let last = entry.lastRunEpoch, now - last < 7 * 86_400 { boost += 0.1; why.append("recent") }
        if entry.runs >= 5 { why.append("used \(entry.runs) times") }
        if exact { boost += literal ? 0.5 : 0.2 }
        if entry.source == "captured" && entry.runs > 0 {
            let successRate = Double(entry.successes) / Double(entry.runs)
            boost *= 0.75 + 0.25 * successRate
            if entry.successes == 0 { why.append("always failed") }
        }
        entry.why = why
        return (entry, hit.relevance * pow(boost, hit.relevance))
    }
    .enumerated()
    .sorted { $0.element.1 != $1.element.1 ? $0.element.1 > $1.element.1 : $0.offset < $1.offset }
    .map(\.element.0)
}

#if canImport(ProximaKit)
import ProximaKit
import ProximaEmbeddings

/// BM25 terms for shell text: each whole shell word (so `--amend` and `HEAD~1` match exactly)
/// plus its alphanumeric pieces (so `amend` and `head` match too). Label lines from `searchDocument`
/// are skipped: they are for the embedding, and words like "last" or "run" in every document would
/// add keyword noise to queries that use them.
struct ShellTokenizer: BM25Tokenizer {
    static let labels = ["command: ", "subcommand: ", "flags: ", "author: "]

    func tokenize(_ text: String) -> [String] {
        var tokens: [String] = []
        let lines = text.lowercased().split(separator: "\n", omittingEmptySubsequences: false).enumerated()
            .filter { $0.offset == 0 || !Self.labels.contains(where: $0.element.hasPrefix) }.map(\.element)
        for word in lines.joined(separator: " ").split(whereSeparator: \.isWhitespace) {
            tokens.append(String(word))
            let pieces = word.split { !($0.isLetter || $0.isNumber) }
            if pieces.count != 1 || pieces[0] != word { tokens += pieces.map(String.init) }
        }
        return tokens
    }
}

/// Apple's on-device sentence embedding, fed words rather than shell punctuation.
/// Text the model cannot embed gets a fixed vector, leaving its ranking to the BM25 leg.
struct CommandEmbedder: TextEmbedder {
    let base: NLEmbeddingProvider
    var dimension: Int { base.dimension }

    func embed(_ text: String) async throws -> Vector {
        let words = text.split { !($0.isLetter || $0.isNumber) }.joined(separator: " ")
        if let vector = try? await base.embed(words.isEmpty ? text : words) { return vector }
        return Vector(dimension: dimension, repeating: 1 / Float(dimension).squareRoot())
    }
}

/// A derived, rebuildable search index over distinct commands, kept next to the history database.
/// What `smcli health` reports about the search index.
struct IndexHealth: Codable, Equatable {
    var embedder = ""
    var path = ""
    var built = false
    /// Commands in the index, and distinct commands in history that should be.
    var indexed = 0
    var commands = 0
    /// Not yet embedded (the next search adds them), and no longer in history (the next search removes them).
    var missing = 0
    var stale = 0
    var bytes = 0
    var lastUpdated: String?
    /// Indexes for other models, which stay on disk so switching back is quick.
    var otherIndexes: [String] = []
    /// Anything that stops search from working; empty when healthy.
    var problems: [String] = []
    var healthy: Bool { problems.isEmpty }
}

struct HistorySearch {
    let directory: URL
    /// Ollama embedding model from `smcli init`; nil uses Apple's built-in model.
    var ollama: OllamaEmbedder? = nil
    /// Local potion-code-16M-v2 from `smcli init --potion`; takes precedence over Ollama.
    var potion: PotionEmbedder? = nil

    /// Static embeddings get the bare command; contextual models get the labeled document.
    private var labeledDocuments: Bool { potion == nil }

    /// Brings the index up to date with `corpus`, then returns the best `limit` matches that pass `filter`.
    /// An empty query is allowed with a filter and lists matches by most recent run.
    /// `programs` is `programArguments(corpus)`; callers searching one corpus repeatedly pass it to skip re-parsing.
    func search(_ query: String, corpus: [String: SearchEntry], limit: Int, filter: SearchFilter = SearchFilter(),
                programs: [String: Set<String>]? = nil, progress: (String) -> Void = { _ in }) async throws -> [SearchEntry] {
        if query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !filter.isEmpty {
            return corpus.values.filter(filter.allows).sorted {
                if $0.lastRunEpoch != $1.lastRunEpoch { return ($0.lastRunEpoch ?? 0) > ($1.lastRunEpoch ?? 0) }
                if $0.runs != $1.runs { return $0.runs > $1.runs }
                return $0.command < $1.command
            }.prefix(limit).map { $0 }
        }
        guard !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw HistoryError(message: "Search query is empty.")
        }
        guard !corpus.isEmpty else { return [] }
        let lock = try lockIndex(wait: true)!
        defer { close(lock) }
        let (store, _) = try await openIndex()
        let documents = try await sync(store, corpus: corpus) { done, total in
            if done == 0 && total > 200 { progress("Indexing \(total) commands for search…") }
        }
        // Literal queries lean on keywords; intent queries fuse both legs by rank.
        let literal = isLiteralQuery(query, programs: programs ?? programArguments(corpus))
        await store.index.setFusion(literal ? .weightedSum(alpha: 0.3) : .rrf)
        // A bounded pool is reranked with usage; filters run after ranking, so a filtered search ranks
        // the whole index before keeping matches.
        let k = filter.isEmpty ? max(limit * 4, 40) : max(limit, documents.count)
        let results = try await store.query(query, k: k)
        let best = Double(results.map { -$0.distance }.max() ?? 1)
        let hits = results.compactMap { result -> (entry: SearchEntry, relevance: Double)? in
            guard let entry = result.decodeMetadata(as: ChunkMetadata.self).flatMap({ documents[$0.text] }).flatMap({ corpus[$0] }),
                  filter.allows(entry) else { return nil }
            return (entry, best > 0 ? Double(-result.distance) / best : 1)
        }
        return Array(rerank(hits, literal: literal, query: query).prefix(limit))
    }

    /// Discards the current model's index and embeds every command again. Returns the number embedded.
    func reindex(corpus: [String: SearchEntry], progress: (Int, Int) -> Void = { _, _ in }) async throws -> Int {
        let lock = try lockIndex(wait: true)!
        defer { close(lock) }
        try? FileManager.default.removeItem(at: directory.appendingPathComponent(try indexName()))
        let (store, _) = try await openIndex()
        return try await sync(store, corpus: corpus, progress: progress).count
    }

    /// Inspects the index without changing it, and checks that the embedding model answers with the right size.
    func health(corpus: [String: SearchEntry]) async -> IndexHealth {
        var health = IndexHealth(path: directory.path, commands: corpus.count)
        do {
            let name = try indexName()
            let url = directory.appendingPathComponent(name)
            health.embedder = potion.map { "\(PotionModel.name) (\($0.dimension) dimensions), \($0.model.source == "built in" ? "built in" : "at " + $0.model.source)" }
                ?? ollama.map { "Ollama \($0.model) (\($0.dimension) dimensions)" } ?? "Apple built-in"
            health.path = url.path
            let embedder = try makeEmbedder()
            do {
                let vector = try await embedder.embed("smcli health check")
                if vector.dimension != embedder.dimension {
                    health.problems.append("Embedding model returns \(vector.dimension) dimensions, not \(embedder.dimension); rerun `smcli init`.")
                }
            } catch {
                health.problems.append("Embedding model unavailable: \(error.localizedDescription)")
            }
            let names = (try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []
            health.otherIndexes = names.filter { $0.hasPrefix("commands-v") && $0 != name }.sorted()
            guard FileManager.default.fileExists(atPath: url.path) else {
                health.missing = corpus.count
                return health
            }
            health.built = true
            try checkDirectory()
            let files = FileManager.default.enumerator(at: url, includingPropertiesForKeys: [.fileSizeKey, .contentModificationDateKey])
            var latest: Date?
            while let file = files?.nextObject() as? URL {
                let values = try? file.resourceValues(forKeys: [.fileSizeKey, .contentModificationDateKey])
                health.bytes += values?.fileSize ?? 0
                if let date = values?.contentModificationDate, date > latest ?? .distantPast { latest = date }
            }
            health.lastUpdated = latest.map { ISO8601DateFormatter().string(from: $0) }
            guard let lock = try lockIndex(wait: false) else {
                health.problems.append("Another smcli is updating the index; check again when it finishes.")
                return health
            }
            defer { close(lock) }
            let store: HybridVectorStore
            do {
                store = try await openStore(name: name, embedder: embedder)
            } catch {
                health.problems.append("Index is unreadable (\(error.localizedDescription)); run `smcli reindex`.")
                return health
            }
            let indexed = await store.documentIds
            let expected = Set(corpus.values.map { searchDocument($0, labeled: labeledDocuments) })
            health.indexed = indexed.count
            health.missing = expected.subtracting(indexed).count
            health.stale = indexed.subtracting(expected).count
        } catch {
            health.problems.append(error.localizedDescription)
        }
        return health
    }

    static let version = "commands-v7"

    /// Brings `store` in line with `corpus` and saves it. Returns each indexed document's command.
    private func sync(_ store: HybridVectorStore, corpus: [String: SearchEntry],
                      progress: (Int, Int) -> Void) async throws -> [String: String] {
        // Documents are keyed by their text, so an entry whose labels change is re-embedded.
        var documents: [String: String] = [:]
        for (command, entry) in corpus { documents[searchDocument(entry, labeled: labeledDocuments)] = command }
        let indexed = await store.documentIds
        for stale in indexed.subtracting(documents.keys) {
            try await store.removeDocument(id: stale)
        }
        let missing = documents.keys.filter { !indexed.contains($0) }.sorted()
        if !missing.isEmpty { progress(0, missing.count) }
        for start in stride(from: 0, to: missing.count, by: 256) {
            let batch = Array(missing[start..<min(start + 256, missing.count)])
            // ProximaKit rebuilds BM25 from the metadata text on open, so it must be the indexed document
            // itself; searches map it back to the command.
            try await store.addChunks(batch, metadata: batch.map { ChunkMetadata(documentId: $0, chunkIndex: 0, text: $0) })
            progress(start + batch.count, missing.count)
        }
        try await store.save()
        return documents
    }

    /// Embedding model, tokenizer, or document format changes need a fresh index; the name pins all three.
    private func indexName() throws -> String {
        // Keyed by the model's directory, which for a Hugging Face snapshot names its revision: another
        // model or revision with the same dimension must not reuse these vectors.
        if let potion { return "\(Self.version)-potion-\(potion.model.identity)-d\(potion.dimension)" }
        if let ollama {
            let slug = String(ollama.model.map { $0.isLetter || $0.isNumber || $0 == "." || $0 == "-" ? $0 : "_" })
            return "\(Self.version)-ollama-\(slug)-d\(ollama.dimension)"
        }
        return "\(Self.version)-d\(try makeEmbedder().dimension)"
    }

    private func makeEmbedder() throws -> any TextEmbedder {
        if let potion { return potion }
        if let ollama { return ollama }
        return CommandEmbedder(base: try NLEmbeddingProvider(language: .english))
    }

    private func openIndex() async throws -> (HybridVectorStore, String) {
        let name = try indexName()
        let embedder = try makeEmbedder()
        removeOldIndexes()
        do {
            return (try await openStore(name: name, embedder: embedder), name)
        } catch {
            // The index is a cache of the history; rebuild it rather than fail.
            try? FileManager.default.removeItem(at: directory.appendingPathComponent(name))
            return (try await openStore(name: name, embedder: embedder), name)
        }
    }

    /// Serializes index writers. Without `wait`, returns nil when another process holds the lock.
    private func lockIndex(wait: Bool) throws -> Int32? {
        try prepareDirectory()
        let lock = open(directory.appendingPathComponent("lock").path, O_RDWR | O_CREAT | O_NOFOLLOW | O_CLOEXEC, mode_t(0o600))
        guard lock >= 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        if flock(lock, wait ? LOCK_EX : LOCK_EX | LOCK_NB) != 0 {
            let error = errno
            close(lock)
            if !wait && error == EWOULDBLOCK { return nil }
            throw NSError(domain: NSPOSIXErrorDomain, code: Int(error))
        }
        return lock
    }

    /// Indexes from earlier document formats are never read again.
    private func removeOldIndexes() {
        let names = (try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []
        for name in names where name.hasPrefix("commands-v") && !name.hasPrefix(Self.version + "-") {
            try? FileManager.default.removeItem(at: directory.appendingPathComponent(name))
        }
    }

    private func openStore(name: String, embedder: any TextEmbedder) async throws -> HybridVectorStore {
        try await HybridVectorStore.open(name: name, embedder: embedder, storageDirectory: directory,
                                         tokenizer: ShellTokenizer(), checkpointAutomatically: WALCheckpointPolicy())
    }

    /// The index holds command text, so it gets the same privacy as the history database.
    private func prepareDirectory() throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true,
                                                attributes: [.posixPermissions: 0o700])
        try checkDirectory()
    }

    private func checkDirectory() throws {
        var info = stat()
        guard lstat(directory.path, &info) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        guard info.st_mode & mode_t(S_IFMT) == mode_t(S_IFDIR), info.st_uid == getuid(), info.st_mode & 0o077 == 0 else {
            throw HistoryError(message: "Search index must be a private directory owned by you (chmod 700): \(directory.path)")
        }
    }
}
#endif
