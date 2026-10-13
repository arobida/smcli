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
            lastDirectory: stats.lastDirectory, lastExitCode: stats.lastExitCode, source: "captured")
    }
    var nativeCounts: [String: Int] = [:]
    for line in native {
        let command = line.trimmingCharacters(in: .whitespaces)
        if !command.isEmpty && corpus[command] == nil && !isSmcliCommand(command) { nativeCounts[command, default: 0] += 1 }
    }
    for (command, count) in nativeCounts {
        corpus[command] = SearchEntry(command: command, runs: count, lastRun: nil, lastDirectory: nil,
                                      lastExitCode: nil, source: "native")
    }
    return corpus
}

#if canImport(ProximaKit)
import ProximaKit
import ProximaEmbeddings

/// BM25 terms for shell text: each whole shell word (so `--amend` and `HEAD~1` match exactly)
/// plus its alphanumeric pieces (so `amend` and `head` match too).
struct ShellTokenizer: BM25Tokenizer {
    func tokenize(_ text: String) -> [String] {
        var tokens: [String] = []
        for word in text.lowercased().split(whereSeparator: \.isWhitespace) {
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
struct HistorySearch {
    let directory: URL
    /// Ollama embedding model from `smcli init`; nil uses Apple's built-in model.
    var ollama: OllamaEmbedder? = nil

    /// Brings the index up to date with `corpus`, then returns the best `limit` matches.
    func search(_ query: String, corpus: [String: SearchEntry], limit: Int,
                progress: (String) -> Void = { _ in }) async throws -> [SearchEntry] {
        guard !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw HistoryError(message: "Search query is empty.")
        }
        guard !corpus.isEmpty else { return [] }
        try prepareDirectory()
        // Serialize concurrent searches: each may write the index.
        let lock = open(directory.appendingPathComponent("lock").path, O_RDWR | O_CREAT | O_NOFOLLOW | O_CLOEXEC, mode_t(0o600))
        guard lock >= 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        defer { close(lock) }
        guard flock(lock, LOCK_EX) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }

        let embedder: any TextEmbedder
        let name: String
        // Embedding model or tokenizer changes need a fresh index; the name pins both.
        if let ollama {
            embedder = ollama
            let slug = String(ollama.model.map { $0.isLetter || $0.isNumber || $0 == "." || $0 == "-" ? $0 : "_" })
            name = "commands-v1-ollama-\(slug)-d\(ollama.dimension)"
        } else {
            embedder = CommandEmbedder(base: try NLEmbeddingProvider(language: .english))
            name = "commands-v1-d\(embedder.dimension)"
        }
        let store: HybridVectorStore
        do {
            store = try await openStore(name: name, embedder: embedder)
        } catch {
            // The index is a cache of the history; rebuild it rather than fail.
            try? FileManager.default.removeItem(at: directory.appendingPathComponent(name))
            store = try await openStore(name: name, embedder: embedder)
        }

        let indexed = await store.documentIds
        for stale in indexed.subtracting(corpus.keys) {
            try await store.removeDocument(id: stale)
        }
        let missing = corpus.keys.filter { !indexed.contains($0) }.sorted()
        if missing.count > 200 { progress("Indexing \(missing.count) commands for search…") }
        for start in stride(from: 0, to: missing.count, by: 256) {
            let batch = Array(missing[start..<min(start + 256, missing.count)])
            try await store.addChunks(batch, metadata: batch.map { ChunkMetadata(documentId: $0, chunkIndex: 0, text: $0) })
        }
        try await store.save()

        return try await store.query(query, k: limit).compactMap {
            $0.decodeMetadata(as: ChunkMetadata.self).flatMap { corpus[$0.documentId] }
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
        var info = stat()
        guard lstat(directory.path, &info) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        guard info.st_mode & mode_t(S_IFMT) == mode_t(S_IFDIR), info.st_uid == getuid(), info.st_mode & 0o077 == 0 else {
            throw HistoryError(message: "Search index must be a private directory owned by you (chmod 700): \(directory.path)")
        }
    }
}
#endif
