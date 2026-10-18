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
            author: stats.author, intent: stats.intent, flags: stats.flags)
    }
    var nativeCounts: [String: Int] = [:]
    for line in native {
        let command = line.trimmingCharacters(in: .whitespaces)
        if !command.isEmpty && corpus[command] == nil && !isSmcliCommand(command) { nativeCounts[command, default: 0] += 1 }
    }
    for (command, count) in nativeCounts {
        corpus[command] = SearchEntry(command: command, runs: count, lastRun: nil, lastDirectory: nil,
                                      lastExitCode: nil, source: "native", flags: commandFlags(command))
    }
    return corpus
}

extension SearchEntry {
    /// The executable's name without its path (`git` for `/usr/bin/git git status`); for compound
    /// shell lines the parser refuses, the first word.
    var program: String? {
        if let main = CommandBreakdown.parse(command)?.mainCommand { return main.split(separator: "/").last.map(String.init) ?? main }
        return command.split(whereSeparator: \.isWhitespace).first.map { String($0.split(separator: "/").last ?? $0) }
    }
    var subcommand: String? { CommandBreakdown.parse(command)?.subcommand }
    var lastRunEpoch: Double? { lastRun.flatMap { ISO8601DateFormatter().date(from: $0)?.timeIntervalSince1970 } }
}

/// The text embedded for an entry: the command plus labeled fields, so queries naming a tool, option,
/// or project land near the right commands. Time and frequency are left to `SearchFilter`: as labels
/// they did not improve ranking and would force re-embedding as they change.
func searchDocument(_ entry: SearchEntry) -> String {
    // Long commands (heredocs, pasted scripts) are cut so they fit embedding contexts with their labels intact.
    var lines = [entry.command.count > 1_500 ? String(entry.command.prefix(1_500)) : entry.command]
    if let program = entry.program { lines.append("command: \(program)") }
    if let subcommand = entry.subcommand { lines.append("subcommand: \(subcommand)") }
    if !entry.flags.isEmpty { lines.append("flags: \(entry.flags.joined(separator: " "))") }
    if let directory = entry.lastDirectory { lines.append("directory: \(directory)") }
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

#if canImport(ProximaKit)
import ProximaKit
import ProximaEmbeddings

/// BM25 terms for shell text: each whole shell word (so `--amend` and `HEAD~1` match exactly)
/// plus its alphanumeric pieces (so `amend` and `head` match too). Label lines from `searchDocument`
/// are skipped: they are for the embedding, and words like "last" or "run" in every document would
/// add keyword noise to queries that use them.
struct ShellTokenizer: BM25Tokenizer {
    static let labels = ["command: ", "subcommand: ", "flags: ", "directory: ", "author: "]

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
    /// The bundled Core AI model from `smcli init --coreai`; takes precedence over Ollama.
    var coreai: (any TextEmbedder)? = nil

    /// Brings the index up to date with `corpus`, then returns the best `limit` matches that pass `filter`.
    /// An empty query is allowed with a filter and lists matches by most recent run.
    func search(_ query: String, corpus: [String: SearchEntry], limit: Int, filter: SearchFilter = SearchFilter(),
                progress: (String) -> Void = { _ in }) async throws -> [SearchEntry] {
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
        let count = try await sync(store, corpus: corpus) { done, total in
            if done == 0 && total > 200 { progress("Indexing \(total) commands for search…") }
        }
        // Filters run after ranking, so a filtered search ranks the whole index before keeping matches.
        let k = filter.isEmpty ? limit : max(limit, count)
        let hits = try await store.query(query, k: k).compactMap {
            $0.decodeMetadata(as: ChunkMetadata.self).flatMap { corpus[$0.text] }
        }
        return Array(hits.filter(filter.allows).prefix(limit))
    }

    /// Discards the current model's index and embeds every command again. Returns the number embedded.
    func reindex(corpus: [String: SearchEntry], progress: (Int, Int) -> Void = { _, _ in }) async throws -> Int {
        let lock = try lockIndex(wait: true)!
        defer { close(lock) }
        try? FileManager.default.removeItem(at: directory.appendingPathComponent(try indexName()))
        let (store, _) = try await openIndex()
        return try await sync(store, corpus: corpus, progress: progress)
    }

    /// Inspects the index without changing it, and checks that the embedding model answers with the right size.
    func health(corpus: [String: SearchEntry]) async -> IndexHealth {
        var health = IndexHealth(path: directory.path, commands: corpus.count)
        do {
            let name = try indexName()
            let url = directory.appendingPathComponent(name)
            health.embedder = coreai.map { "Core AI F2LLM-v2-80M (\($0.dimension) dimensions)" } ?? ollama.map { "Ollama \($0.model) (\($0.dimension) dimensions)" } ?? "Apple built-in"
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
            let expected = Set(corpus.values.map(searchDocument))
            health.indexed = indexed.count
            health.missing = expected.subtracting(indexed).count
            health.stale = indexed.subtracting(expected).count
        } catch {
            health.problems.append(error.localizedDescription)
        }
        return health
    }

    static let version = "commands-v5"

    /// Brings `store` in line with `corpus` and saves it. Returns the number of indexed commands.
    private func sync(_ store: HybridVectorStore, corpus: [String: SearchEntry],
                      progress: (Int, Int) -> Void) async throws -> Int {
        // Documents are keyed by their labeled text, so an entry whose labels change is re-embedded.
        var documents: [String: String] = [:]
        for (command, entry) in corpus { documents[searchDocument(entry)] = command }
        let indexed = await store.documentIds
        for stale in indexed.subtracting(documents.keys) {
            try await store.removeDocument(id: stale)
        }
        let missing = documents.keys.filter { !indexed.contains($0) }.sorted()
        if !missing.isEmpty { progress(0, missing.count) }
        for start in stride(from: 0, to: missing.count, by: 256) {
            let batch = Array(missing[start..<min(start + 256, missing.count)])
            try await store.addChunks(batch, metadata: batch.map {
                ChunkMetadata(documentId: $0, chunkIndex: 0, text: documents[$0]!)
            })
            progress(start + batch.count, missing.count)
        }
        try await store.save()
        return documents.count
    }

    /// Embedding model, tokenizer, or document format changes need a fresh index; the name pins all three.
    private func indexName() throws -> String {
        if let coreai { return "\(Self.version)-coreai-f2llm-v2-80m-d\(coreai.dimension)" }
        if let ollama {
            let slug = String(ollama.model.map { $0.isLetter || $0.isNumber || $0 == "." || $0 == "-" ? $0 : "_" })
            return "\(Self.version)-ollama-\(slug)-d\(ollama.dimension)"
        }
        return "\(Self.version)-d\(try makeEmbedder().dimension)"
    }

    private func makeEmbedder() throws -> any TextEmbedder {
        if let coreai { return coreai }
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
