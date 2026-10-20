import Foundation
import SmcliCore
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif

/// Merges captured stats and native history lines into one entry per distinct command.
/// Native history keeps no record boundaries we can trust, so each line is one entry.
package func searchCorpus(captured: [String: CommandStats], native: [String]) -> [String: SearchEntry] {
    CorpusState(captured: captured, native: native).entries
}

/// The search corpus, which `add` updates for one new run instead of rebuilding from every run. Usage is as of
/// `asOf` (frecency drifts with time, so a holder rebuilds now and then), and `directory` and `project` fix what
/// `runsHere` and `runsProject` count. After the same runs, `entries` equals a rebuild from `commandStats`.
package struct CorpusState {
    package let directory: String
    package let project: String?
    package let asOf: Double
    package private(set) var entries: [String: SearchEntry] = [:]
    /// Each program in `entries` with the words that follow it, for `isLiteralQuery`.
    package private(set) var programs: [String: Set<String>] = [:]
    private var captured: [String: CommandStats]
    private var nativeCounts: [String: Int] = [:]
    private var nativeUsage: [String: Double] = [:]
    private let formatter = ISO8601DateFormatter()

    package init(captured: [String: CommandStats], native: [String], directory: String = "", project: String? = nil,
                 asOf: Double = Date().timeIntervalSince1970) {
        (self.captured, self.asOf) = (captured, asOf)
        self.directory = directory
        self.project = project
        // Native lines have no times, but files are oldest first, so position stands in for age: a run's weight
        // halves every `nativeHalfLife` lines from the end. Captured commands keep whichever evidence is
        // stronger, so turning capture on does not throw away years of native habit.
        let lines = native.map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty && !isSmcliCommand($0) }
        for (index, command) in lines.enumerated() {
            nativeCounts[command, default: 0] += 1
            nativeUsage[command, default: 0] += pow(0.5, Double(lines.count - 1 - index) / nativeHalfLife)
        }
        for command in Set(captured.keys).union(nativeCounts.keys) {
            if let entry = makeEntry(command) { entries[command] = entry }
        }
        programs = programArguments(entries)
    }

    /// Adds one run the way `commandStats` would count it (see its columns), as the service does per record.
    package mutating func add(_ run: CapturedHistory.Run) {
        guard !isSmcliCommand(run.command) else { return }
        var stats = captured[run.command] ?? CommandStats(runs: 0, lastRun: -.infinity, lastDirectory: run.directory,
                                                          lastExitCode: run.exitCode, flags: commandFlags(run.command))
        stats.runs += 1
        if run.startedAt >= stats.lastRun {
            (stats.lastRun, stats.lastDirectory, stats.lastExitCode) = (run.startedAt, run.directory, run.exitCode)
            (stats.author, stats.intent) = (nil, nil)
        }
        stats.frecency += pow(0.5, max(0, asOf - run.startedAt) / CapturedHistory.usageHalfLife)
        if CapturedHistory.interruptedExitCodes.contains(run.exitCode) || run.exitCode == 0
            || (run.exitCode == 1 && CapturedHistory.answersWithExitCode(run.command)) {
            stats.successes += 1
        }
        if Self.within(run.directory, directory) { stats.runsHere += 1 }
        if let project, Self.within(run.directory, project) { stats.runsProject += 1 }
        captured[run.command] = stats
        guard let entry = makeEntry(run.command) else { return }
        entries[run.command] = entry
        addProgram(entry, to: &programs)
    }

    /// `runsHere` in SQL terms: the directory itself or below it; a root of "/" matches only itself.
    private static func within(_ path: String, _ root: String) -> Bool {
        !root.isEmpty && (path == root || (root != "/" && path.hasPrefix(root + "/")))
    }

    private func makeEntry(_ command: String) -> SearchEntry? {
        var entry: SearchEntry?
        if let stats = captured[command], !isSmcliCommand(command) {
            entry = SearchEntry(
                command: command, runs: stats.runs,
                lastRun: formatter.string(from: Date(timeIntervalSince1970: stats.lastRun)),
                lastDirectory: stats.lastDirectory, lastExitCode: stats.lastExitCode, source: "captured",
                author: stats.author, intent: stats.intent, flags: stats.flags,
                frecency: stats.frecency, successes: stats.successes, runsHere: stats.runsHere, runsProject: stats.runsProject)
        }
        guard let count = nativeCounts[command] else { return entry }
        let usage = nativeUsage[command] ?? 0
        if entry != nil {
            entry!.frecency = max(entry!.frecency, usage)
        } else {
            entry = SearchEntry(command: command, runs: count, lastRun: nil, lastDirectory: nil,
                                lastExitCode: nil, source: "native", flags: commandFlags(command), frecency: usage)
        }
        return entry
    }
}

/// Native history lines after which a run counts half; about two weeks for a busy shell, like captured usage.
package let nativeHalfLife = 1_000.0

/// The text embedded for an entry: the command plus labeled fields, so queries naming a tool or option
/// land near the right commands. Directories are left out; `--dir` and the "used here" boost cover them. Time and frequency are left to `SearchFilter`: as labels
/// they did not improve ranking and would force re-embedding as they change.
/// With `labeled` false (static models such as Potion, which mean-pool tokens and would dilute the command
/// with repeated names and label words), only the command and its recorded intent are embedded.
package func searchDocument(_ entry: SearchEntry, labeled: Bool = true) -> String {
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

/// How literal a query is. Flags, paths, revisions, assignments, variables, and commands from the history
/// carry exact constraints the keyword leg honors; plain words describe intent for embeddings.
/// `programs` maps each program in the history to the words seen right after it (`git` → `stash`, `log`).
package func isLiteralQuery(_ query: String, programs: [String: Set<String>]) -> Bool {
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
package func programArguments(_ corpus: [String: SearchEntry]) -> [String: Set<String>] {
    var programs: [String: Set<String>] = [:]
    for entry in corpus.values { addProgram(entry, to: &programs) }
    return programs
}

private func addProgram(_ entry: SearchEntry, to programs: inout [String: Set<String>]) {
    guard let program = entry.program?.lowercased() else { return }
    let words = entry.command.split(whereSeparator: \.isWhitespace)
    let index = words.firstIndex { $0.split(separator: "/").last.map { $0.lowercased() == program } ?? false }
    let next = index.flatMap { words.indices.contains($0 + 1) ? words[$0 + 1].lowercased() : nil }
    programs[program, default: []].formUnion(next.map { [$0] } ?? [])
}

/// What search knows about the moment of the query besides the corpus: the project and its candidate commands,
/// the session's previous command with what usually follows it, and earlier picks. All of it is usage and context
/// metadata applied after retrieval; none of it enters the embedded text or the keyword index.
package struct SearchSignals {
    package var context: RankingContext?
    package var previous: String?
    package var followers: [String: Int]
    package var picks: SelectionLog

    package static let none = SearchSignals()

    package init(context: RankingContext? = nil, previous: String? = nil, followers: [String: Int] = [:],
                 picks: SelectionLog = SelectionLog()) {
        (self.context, self.previous, self.followers, self.picks) = (context, previous, followers, picks)
    }

    private func uses(_ signal: RankingContext.Signal) -> Bool { context?.uses(signal) ?? true }

    /// Relevance a pick lends a command: a repeated identical query is strong evidence, even for a command the
    /// embedding finds only loosely related.
    static func pickRelevance(_ weight: Double) -> Double { min(1, 0.5 + 0.25 * weight) }

    /// Commands that belong in the pool although retrieval did not return them: ones picked for a similar query,
    /// and project scripts or Git-state commands whose words all appear in the query. They enter with a lexical
    /// relevance and still go through `rerank` like any hit; nothing is added when it does not match.
    package func additions(query: String, corpus: [String: SearchEntry], existing: Set<String>, filter: SearchFilter,
                           now: Double) -> [(entry: SearchEntry, relevance: Double)] {
        var added: [(entry: SearchEntry, relevance: Double)] = []
        var seen = existing
        if uses(.selection) {
            for (command, weight) in picks.weights(query: query, now: now) where !seen.contains(command) {
                guard let entry = corpus[command], filter.allows(entry) else { continue }
                seen.insert(command)
                added.append((entry, Self.pickRelevance(weight)))
            }
        }
        let terms = query.lowercased().split(whereSeparator: \.isWhitespace).map(String.init)
        guard !terms.isEmpty else { return added }
        for candidate in (context?.candidates(.hint) ?? []) + (context?.candidates(.script) ?? [])
        where !seen.contains(candidate.command) && Self.coverage(candidate.command, terms) == 1 {
            let entry = corpus[candidate.command] ?? SearchEntry(command: candidate.command, runs: 0, lastRun: nil, lastDirectory: nil,
                                                                  lastExitCode: nil, source: "project", flags: commandFlags(candidate.command))
            guard filter.allows(entry) else { continue }
            seen.insert(candidate.command)
            added.append((entry, 1))
        }
        return added
    }

    /// The share of query words that are a word of the command or start one (`sto` for `storybook`), counting the
    /// pieces of `--continue`; 1 when all of them are.
    static func coverage(_ command: String, _ terms: [String]) -> Double {
        guard !terms.isEmpty else { return 0 }
        var tokens: Set<String> = []
        for word in command.lowercased().split(whereSeparator: \.isWhitespace) {
            tokens.insert(String(word))
            tokens.formUnion(word.split { !($0.isLetter || $0.isNumber) }.map(String.init))
        }
        let matched = terms.filter { term in tokens.contains { $0 == term || (term.count >= 2 && $0.hasPrefix(term)) } }
        return Double(matched.count) / Double(terms.count)
    }
}

/// Reorders hybrid hits with usage evidence. `relevance` is each hit's fused score scaled so the best is 1.
/// Usage multiplies it as `boost ^ relevance`, so a habit lifts a strong match nearly fully but barely moves a
/// weak one: rank fusion leaves even distant dense neighbours a third of the best score, and a plain product
/// would let frequent unrelated commands outrank real matches.
/// Failures lower the score without excluding: the failed command may be exactly what is wanted.
/// `signals` adds the project, the previous command, earlier picks, project scripts and Git state, each with a `why`.
package func rerank(_ hits: [(entry: SearchEntry, relevance: Double)], literal: Bool, query: String,
            now: Double = Date().timeIntervalSince1970, signals: SearchSignals = .none) -> [SearchEntry] {
    let queryTerms = query.lowercased().split(whereSeparator: \.isWhitespace).map(String.init)
    let terms = Set(queryTerms)
    let context = signals.context
    let picks = context?.uses(.selection) ?? true ? signals.picks.weights(query: query, now: now) : [:]
    let candidates = Dictionary((context?.candidates(.script) ?? []).map { ($0.command, $0) }, uniquingKeysWith: { first, _ in first })
    let hints = Dictionary((context?.candidates(.hint) ?? []).map { ($0.command, $0) }, uniquingKeysWith: { first, _ in first })
    let followed = context?.uses(.transition) ?? true ? signals.followers : [:]
    let followTotal = Double(followed.values.reduce(0, +))
    return hits.map { hit -> (SearchEntry, Double) in
        var entry = hit.entry
        var why: [String] = []
        var relevance = hit.relevance
        let words = Set(entry.command.lowercased().split(whereSeparator: \.isWhitespace).map(String.init))
        let exact = !terms.isEmpty && terms.isSubset(of: words)
        why.append(exact ? "exact words" : literal ? "keyword match" : "semantic match")
        var boost = 1 + 0.15 * log1p(entry.frecency)
        if entry.runsHere > 0 {
            boost += 0.35 * min(1, Double(entry.runsHere) / Double(max(1, entry.runs)) + 0.25)
            why.append("used here")
        } else if entry.runsProject > 0, context?.uses(.project) ?? true {
            boost += 0.45 * min(1, Double(entry.runsProject) / Double(max(1, entry.runs)) + 0.25)
            why.append("used in this project")
        }
        if let last = entry.lastRunEpoch, now - last < 7 * 86_400 { boost += 0.1; why.append("recent") }
        if entry.runs >= 5 { why.append("used \(entry.runs) times") }
        if exact { boost += literal ? 0.5 : 0.2 }
        // A project command counts in proportion to how much of the query it matches, so an unrelated script does
        // not ride along on a hint or a habit.
        let covered = hints[entry.command] != nil || candidates[entry.command] != nil
            ? SearchSignals.coverage(entry.command, queryTerms) : 0
        if covered > 0, let hint = hints[entry.command] {
            boost += 1.5 * covered
            why.append(hint.reason)
        } else if covered > 0, let script = candidates[entry.command] {
            boost += 0.8 * covered
            why.append(script.reason)
        }
        if let previous = signals.previous, let count = followed[entry.command], followTotal > 0 {
            boost += Double(count) / followTotal * min(1, Double(count) / 3)
            why.append("often follows \(previous)")
        }
        if let weight = picks[entry.command] {
            boost += min(1.5, 0.75 * weight)
            relevance = max(relevance, SearchSignals.pickRelevance(weight))
            why.append("picked for similar searches")
        }
        if entry.source == "captured" && entry.runs > 0 {
            let successRate = Double(entry.successes) / Double(entry.runs)
            boost *= 0.75 + 0.25 * successRate
            if entry.successes == 0 { why.append("always failed") }
        }
        entry.why = why
        return (entry, relevance * pow(boost, relevance))
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
package struct ShellTokenizer: BM25Tokenizer {
    package static let labels = ["command: ", "subcommand: ", "flags: ", "author: "]

    package func tokenize(_ text: String) -> [String] {
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
package struct CommandEmbedder: TextEmbedder {
    package let base: NLEmbeddingProvider
    package var dimension: Int { base.dimension }

    package func embed(_ text: String) async throws -> Vector {
        let words = text.split { !($0.isLetter || $0.isNumber) }.joined(separator: " ")
        if let vector = try? await base.embed(words.isEmpty ? text : words) { return vector }
        return Vector(dimension: dimension, repeating: 1 / Float(dimension).squareRoot())
    }
}

/// A derived, rebuildable search index over distinct commands, kept next to the history database.
/// What `smcli health` reports about the search index.
package struct IndexHealth: Codable, Equatable {
    package var embedder = ""
    package var path = ""
    package var built = false
    /// Commands in the index, and distinct commands in history that should be.
    package var indexed = 0
    package var commands = 0
    /// Not yet embedded (the next search adds them), and no longer in history (the next search removes them).
    package var missing = 0
    package var stale = 0
    package var bytes = 0
    package var lastUpdated: String?
    /// Indexes for other models, which stay on disk so switching back is quick.
    package var otherIndexes: [String] = []
    /// Anything that stops search from working; empty when healthy.
    package var problems: [String] = []
    package var healthy: Bool { problems.isEmpty }
}

/// An index held open by a long-lived process between searches. Reused only while no other process has
/// saved over it (a CLI search without the service, `smcli reindex`); then it is read again.
package final class OpenIndex: @unchecked Sendable {
    fileprivate var store: HybridVectorStore?
    fileprivate var name: String?
    fileprivate var stamp: [String] = []

    package init() {}
}

extension IndexHealth {
    package init(path: String, commands: Int) { (self.path, self.commands) = (path, commands) }
}

package struct HistorySearch {
    package let directory: URL
    /// Ollama embedding model from `smcli init`; nil uses Apple's built-in model.
    package var ollama: OllamaEmbedder? = nil
    /// Local potion-code-16M-v2 from `smcli init --potion`; takes precedence over Ollama.
    package var potion: PotionEmbedder? = nil

    /// Static embeddings get the bare command; contextual models get the labeled document.
    private var labeledDocuments: Bool { potion == nil }

    /// Brings the index up to date with `corpus`, then returns the best `limit` matches that pass `filter`.
    /// An empty query is allowed with a filter and lists matches by most recent run.
    /// `programs` is `programArguments(corpus)`; callers searching one corpus repeatedly pass it to skip re-parsing.
    /// `open`, from a long-lived caller, keeps the index loaded between searches.
    package func search(_ query: String, corpus: [String: SearchEntry], limit: Int, filter: SearchFilter = SearchFilter(),
                programs: [String: Set<String>]? = nil, open: OpenIndex? = nil, signals: SearchSignals = .none,
                now: Double = Date().timeIntervalSince1970, progress: (String) -> Void = { _ in }) async throws -> [SearchEntry] {
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
        // No history yet: nothing to embed, but project commands can still answer.
        guard !corpus.isEmpty else {
            let hits = signals.additions(query: query, corpus: corpus, existing: [], filter: filter, now: now)
            return Array(rerank(hits, literal: isLiteralQuery(query, programs: programs ?? [:]), query: query, now: now,
                                signals: signals).prefix(limit))
        }
        let lock = try await acquireIndexLock()
        defer { close(lock) }
        let (store, name) = try await openIndex(reusing: open)
        let documents = try await sync(store, corpus: corpus) { done, total in
            if done == 0 && total > 200 { progress("Indexing \(total) commands for search…") }
        }
        if let open { (open.store, open.name, open.stamp) = (store, name, stamp(name)) }
        // Literal queries lean on keywords; intent queries fuse both legs by rank.
        let literal = isLiteralQuery(query, programs: programs ?? programArguments(corpus))
        await store.index.setFusion(literal ? .weightedSum(alpha: 0.3) : .rrf)
        // A bounded pool is reranked with usage; filters run after ranking, so a filtered search ranks
        // the whole index before keeping matches.
        let k = filter.isEmpty ? max(limit * 4, 40) : max(limit, documents.count)
        let results = try await store.query(query, k: k)
        let best = Double(results.map { -$0.distance }.max() ?? 1)
        var hits = results.compactMap { result -> (entry: SearchEntry, relevance: Double)? in
            guard let entry = result.decodeMetadata(as: ChunkMetadata.self).flatMap({ documents[$0.text] }).flatMap({ corpus[$0] }),
                  filter.allows(entry) else { return nil }
            return (entry, best > 0 ? Double(-result.distance) / best : 1)
        }
        hits += signals.additions(query: query, corpus: corpus, existing: Set(hits.map(\.entry.command)), filter: filter, now: now)
        return Array(rerank(hits, literal: literal, query: query, now: now, signals: signals).prefix(limit))
    }

    /// Discards the current model's index and embeds every command again. Returns the number embedded.
    package func reindex(corpus: [String: SearchEntry], progress: (Int, Int) -> Void = { _, _ in }) async throws -> Int {
        let lock = try await acquireIndexLock()
        defer { close(lock) }
        try? FileManager.default.removeItem(at: directory.appendingPathComponent(try indexName()))
        let (store, _) = try await openIndex()
        return try await sync(store, corpus: corpus, progress: progress).count
    }

    /// Inspects the index without changing it, and checks that the embedding model answers with the right size.
    package func health(corpus: [String: SearchEntry]) async -> IndexHealth {
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
            guard let lock = try lockIndex() else {
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

    package init(directory: URL, ollama: OllamaEmbedder? = nil, potion: PotionEmbedder? = nil) {
        (self.directory, self.ollama, self.potion) = (directory, ollama, potion)
    }

    package static let version = "commands-v7"

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
        // Unchanged: skip the save, so a kept-open index stays the copy on disk.
        if missing.isEmpty && indexed.isSubset(of: documents.keys) { return documents }
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
        } catch is CancellationError {
            throw CancellationError()
        } catch {
            // The index is a cache of the history; rebuild it rather than fail.
            try? FileManager.default.removeItem(at: directory.appendingPathComponent(name))
            return (try await openStore(name: name, embedder: embedder), name)
        }
    }

    /// `open`'s store while the files on disk are the ones it last saved; otherwise the index read fresh.
    private func openIndex(reusing open: OpenIndex?) async throws -> (HybridVectorStore, String) {
        let name = try indexName()
        if let open, let store = open.store, open.name == name, open.stamp == stamp(name) { return (store, name) }
        return try await openIndex()
    }

    /// Every file of the index with its size and modification time: changes when any process saves it.
    private func stamp(_ name: String) -> [String] {
        let root = directory.appendingPathComponent(name)
        let keys: [URLResourceKey] = [.fileSizeKey, .contentModificationDateKey]
        var stamp: [String] = []
        let files = FileManager.default.enumerator(at: root, includingPropertiesForKeys: keys)
        while let file = files?.nextObject() as? URL {
            let values = try? file.resourceValues(forKeys: Set(keys))
            stamp.append("\(file.path) \(values?.fileSize ?? -1) \(values?.contentModificationDate?.timeIntervalSince1970 ?? 0)")
        }
        return stamp.sorted()
    }

    /// Serializes index writers. Returns nil when another process holds the lock.
    private func lockIndex() throws -> Int32? {
        try prepareDirectory()
        let lock = open(directory.appendingPathComponent("lock").path, O_RDWR | O_CREAT | O_NOFOLLOW | O_CLOEXEC, mode_t(0o600))
        guard lock >= 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        if flock(lock, LOCK_EX | LOCK_NB) != 0 {
            let error = errno
            close(lock)
            if error == EWOULDBLOCK { return nil }
            throw NSError(domain: NSPOSIXErrorDomain, code: Int(error))
        }
        return lock
    }

    /// Waits for the index lock without blocking a thread, so a cancelled task stops waiting.
    private func acquireIndexLock() async throws -> Int32 {
        while true {
            try Task.checkCancellation()
            if let lock = try lockIndex() { return lock }
            try await Task.sleep(for: .milliseconds(50))
        }
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
