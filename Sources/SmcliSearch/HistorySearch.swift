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

#if canImport(TalaDB)
// NLEmbedding is immutable once loaded, so sharing it across tasks is safe.
@preconcurrency import NaturalLanguage
import TalaDB

/// BM25 terms for shell text: each whole shell word (so `--amend` and `HEAD~1` match exactly)
/// plus its alphanumeric pieces (so `amend` and `head` match too). Label lines from `searchDocument`
/// are skipped: they are for the embedding, and words like "last" or "run" in every document would
/// add keyword noise to queries that use them.
package struct ShellTokenizer {
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

    /// Terms as TalaDB full-text tokens: `t` plus lowercase hex of the UTF-8, which its alphanumeric splitter
    /// keeps whole, so BM25 scores exactly these terms (`--amend`, `-m`) rather than its own pieces.
    package func encoded(_ text: String) -> String {
        let hex = Array("0123456789abcdef".utf8)
        return tokenize(text).map { term in
            String(decoding: [UInt8(ascii: "t")] + term.utf8.flatMap { [hex[Int($0 >> 4)], hex[Int($0 & 15)]] }, as: UTF8.self)
        }.joined(separator: " ")
    }
}

/// One indexed command; metadata fields mirror `SearchFilter` so filters narrow candidates before ranking.
package struct IndexedCommand: Codable, Sendable {
    /// `searchDocument` text: the identity, and what is embedded.
    var doc: String
    var command: String
    var terms: String
    var embedding: [Float] = []
    var program: String?
    var subcommand: String?
    var flags: [String]
    var author: String?
    /// The last directory and every ancestor, so a directory filter is a membership test.
    var dirs: [String]
    var lastRun: Double?
    var runs: Int
    /// Fingerprint of the metadata fields, to update them without re-embedding.
    var meta: String

    init(_ entry: SearchEntry, doc: String) {
        // Terms are only needed on insert; `sync` fills them so unchanged commands skip tokenizing.
        (self.doc, command, terms) = (doc, entry.command, "")
        let parsed = CommandBreakdown.parse(entry.command)
        (program, subcommand) = (SearchEntry.program(of: entry.command, parsed: parsed), parsed?.subcommand)
        (flags, author, lastRun, runs) = (entry.flags, entry.author, entry.lastRunEpoch, entry.runs)
        var dirs: [String] = []
        var path = entry.lastDirectory ?? ""
        while !path.isEmpty {
            dirs.append(path)
            guard path != "/" else { break }
            path = (path as NSString).deletingLastPathComponent
        }
        self.dirs = dirs
        meta = "\(program ?? "")\u{0}\(subcommand ?? "")\u{0}\(flags)\u{0}\(author ?? "")\u{0}\(dirs)\u{0}\(lastRun ?? -1)\u{0}\(runs)"
    }

    var metadata: [String: JSONValue] {
        ["program": program.map(JSONValue.string) ?? .null, "subcommand": subcommand.map(JSONValue.string) ?? .null,
         "flags": .array(flags.map(JSONValue.string)), "author": author.map(JSONValue.string) ?? .null,
         "dirs": .array(dirs.map(JSONValue.string)), "lastRun": lastRun.map(JSONValue.double) ?? .null,
         "runs": .int(Int64(runs)), "meta": .string(meta)]
    }
}

extension SearchFilter {
    /// The filter as a TalaDB query over `IndexedCommand`, nil when empty. `allows` stays the final check.
    package var indexFilter: Filter? {
        var parts: [Filter] = []
        if let program { parts.append(["program": .string(program)]) }
        if let subcommand { parts.append(["subcommand": .string(subcommand)]) }
        if let author { parts.append(["author": .string(author)]) }
        if let flag { parts.append(["flags": ["$in": [.string(flag)]]]) }
        if let directory {
            let base = directory.count > 1 && directory.hasSuffix("/") ? String(directory.dropLast()) : directory
            parts.append(["dirs": ["$in": [.string(base)]]])
        }
        if since != nil || before != nil {
            var range: [String: JSONValue] = [:]
            if let since { range["$gte"] = .double(since) }
            if let before { range["$lt"] = .double(before) }
            parts.append(["lastRun": .object(range)])
        }
        if let minRuns { parts.append(["runs": ["$gte": .int(Int64(minRuns))]]) }
        if parts.count <= 1 { return parts.first }
        return ["$and": .array(parts.map(JSONValue.object))]
    }
}

/// Apple's on-device sentence embedding, fed words rather than shell punctuation.
/// Text the model cannot embed gets a fixed vector, leaving its ranking to the BM25 leg.
package struct CommandEmbedder: TextEmbedder {
    package let base: NLEmbedding
    package var dimension: Int { base.dimension }

    package init() throws {
        guard let base = NLEmbedding.sentenceEmbedding(for: .english) else {
            throw HistoryError(message: "Apple's English sentence embedding is unavailable on this Mac.")
        }
        self.base = base
    }

    package func embed(_ text: String) async throws -> [Float] {
        let words = text.split { !($0.isLetter || $0.isNumber) }.joined(separator: " ")
        if let vector = base.vector(for: words.isEmpty ? text : words) { return vector.map(Float.init) }
        return [Float](repeating: 1 / Float(dimension).squareRoot(), count: dimension)
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

/// What a long-lived process remembers about the index between searches: each indexed document's metadata
/// fingerprint, reused only while no other process has written the file (a CLI search without the service,
/// `smcli reindex`). No handle stays open: TalaDB refuses a second process outright.
package final class OpenIndex: @unchecked Sendable {
    fileprivate var indexed: [String: String]?
    fileprivate var name: String?
    fileprivate var stamp: [String] = []
    /// Each command's entry and the document built from it, for `name`'s index, so unchanged entries skip rebuilding.
    fileprivate var built: [String: (entry: SearchEntry, command: IndexedCommand)] = [:]

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
        let name = try indexName()
        let known = open.flatMap { $0.name == name && $0.stamp == stamp(name) ? $0.indexed : nil }
        let embedder = try makeEmbedder()
        // Literal queries lean on keywords; intent queries weigh both legs equally.
        let literal = isLiteralQuery(query, programs: programs ?? programArguments(corpus))
        // Fused score per command, best first.
        var results: [(command: String, score: Double)] = []
        do {
            let db = try await openIndex(name)
            defer { db.close() }
            let commands = db.collection("commands", as: IndexedCommand.self)
            if open?.name != name { open?.built = [:] }
            let indexed = try await sync(commands, embedder: embedder, corpus: corpus, known: known, open: open) { done, total in
                if done == 0 && total > 200 { progress("Indexing \(total) commands for search…") }
            }
            open?.indexed = indexed
            // Filters narrow candidates in the index, so a bounded pool is enough to rerank with usage.
            let (text, vector, k) = (ShellTokenizer().encoded(query), try await embedder.embed(query), max(limit * 4, 40))
            if literal {
                // Weighted sum of each leg's score over its best: rank fusion would leave the runner-up level
                // with an exact keyword hit, and usage would then decide.
                let keyword = try await commands.searchText("terms", query: text, topK: k, filter: filter.indexFilter)
                let dense = try await commands.findNearest("embedding", vector: vector, topK: k, filter: filter.indexFilter)
                var scores: [String: Double] = [:]
                for (hits, weight) in [(keyword, 0.7), (dense, 0.3)] {
                    let best = hits.map(\.score).max() ?? 0
                    for hit in hits where best > 0 { scores[hit.document.command, default: 0] += weight * hit.score / best }
                }
                results = scores.sorted { $0.value != $1.value ? $0.value > $1.value : $0.key < $1.key }.map { ($0.key, $0.value) }
            } else {
                results = try await commands.hybridSearch(
                    textField: "terms", text: text, vectorField: "embedding", vector: vector, topK: k,
                    filter: filter.indexFilter).map { ($0.document.command, $0.score) }
            }
        }
        if let open { (open.name, open.stamp) = (name, stamp(name)) }
        let best = results.map(\.score).max() ?? 1
        var hits = results.compactMap { result -> (entry: SearchEntry, relevance: Double)? in
            guard let entry = corpus[result.command], filter.allows(entry) else { return nil }
            return (entry, best > 0 ? result.score / best : 1)
        }
        hits += signals.additions(query: query, corpus: corpus, existing: Set(hits.map(\.entry.command)), filter: filter, now: now)
        return Array(rerank(hits, literal: literal, query: query, now: now, signals: signals).prefix(limit))
    }

    /// Loads the embedding model and reads the index's fingerprints into `open`, so a first search only has to
    /// sync what changed. Needs no corpus, so a service can run it before any client asks.
    package func warm(open: OpenIndex) async throws {
        let lock = try await acquireIndexLock()
        defer { close(lock) }
        let name = try indexName()
        _ = try await makeEmbedder().embed("smcli")
        guard FileManager.default.fileExists(atPath: directory.appendingPathComponent(name + ".taladb").path) else { return }
        do {
            let db = try await openIndex(name)
            defer { db.close() }
            open.indexed = try await fingerprints(db.collection("commands", as: IndexedCommand.self))
        }
        (open.name, open.stamp) = (name, stamp(name))
    }

    /// Discards the current model's index and embeds every command again. Returns the number embedded.
    package func reindex(corpus: [String: SearchEntry], progress: (Int, Int) -> Void = { _, _ in }) async throws -> Int {
        let lock = try await acquireIndexLock()
        defer { close(lock) }
        let name = try indexName()
        removeFiles(name)
        let db = try await openIndex(name)
        defer { db.close() }
        return try await sync(db.collection("commands", as: IndexedCommand.self), embedder: try makeEmbedder(),
                              corpus: corpus, known: nil, open: nil, progress: progress).count
    }

    /// Inspects the index without changing it, and checks that the embedding model answers with the right size.
    package func health(corpus: [String: SearchEntry]) async -> IndexHealth {
        var health = IndexHealth(path: directory.path, commands: corpus.count)
        do {
            let name = try indexName()
            let url = directory.appendingPathComponent(name + ".taladb")
            health.embedder = potion.map { "\(PotionModel.name) (\($0.dimension) dimensions), \($0.model.source == "built in" ? "built in" : "at " + $0.model.source)" }
                ?? ollama.map { "Ollama \($0.model) (\($0.dimension) dimensions)" } ?? "Apple built-in"
            health.path = url.path
            let embedder = try makeEmbedder()
            do {
                let vector = try await embedder.embed("smcli health check")
                if vector.count != embedder.dimension {
                    health.problems.append("Embedding model returns \(vector.count) dimensions, not \(embedder.dimension); rerun `smcli init`.")
                }
            } catch {
                health.problems.append("Embedding model unavailable: \(error.localizedDescription)")
            }
            let names = (try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []
            health.otherIndexes = names.filter { $0.hasPrefix("commands-v") && !$0.hasPrefix(name) }.sorted()
            guard FileManager.default.fileExists(atPath: url.path) else {
                health.missing = corpus.count
                return health
            }
            health.built = true
            try checkDirectory()
            var latest: Date?
            for file in files(name) {
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
            let indexed: Set<String>
            do {
                let db = try await openFile(name)
                defer { db.close() }
                indexed = Set(try await fingerprints(db.collection("commands", as: IndexedCommand.self)).keys)
            } catch {
                health.problems.append("Index is unreadable (\(error.localizedDescription)); run `smcli reindex`.")
                return health
            }
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

    package static let version = "commands-v8"

    /// Brings `commands` in line with `corpus`. `known` is each indexed document's metadata fingerprint, when
    /// a caller remembers it; returns the same for the updated index.
    private func sync(_ commands: TalaCollection<IndexedCommand>, embedder: any TextEmbedder, corpus: [String: SearchEntry],
                      known: [String: String]?, open: OpenIndex?,
                      progress: (Int, Int) -> Void) async throws -> [String: String] {
        // Documents are keyed by their text, so an entry whose labels change is re-embedded.
        var wanted: [String: IndexedCommand] = [:]
        var built: [String: (entry: SearchEntry, command: IndexedCommand)] = [:]
        for entry in corpus.values {
            if let cached = open?.built[entry.command], cached.entry == entry {
                (wanted[cached.command.doc], built[entry.command]) = (cached.command, cached)
                continue
            }
            let doc = searchDocument(entry, labeled: labeledDocuments)
            let command = IndexedCommand(entry, doc: doc)
            (wanted[doc], built[entry.command]) = (command, (entry, command))
        }
        open?.built = built
        var indexed = known ?? [:]
        if known == nil {
            indexed = try await fingerprints(commands)
        }
        let stale = indexed.keys.filter { wanted[$0] == nil }
        if !stale.isEmpty { try await commands.deleteMany(["doc": ["$in": .array(stale.map(JSONValue.string))]]) }
        // Unchanged documents with new usage keep their vectors.
        for (doc, meta) in indexed {
            if let command = wanted[doc], command.meta != meta {
                try await commands.updateOne(["doc": .string(doc)], ["$set": .object(command.metadata)])
            }
        }
        let missing = wanted.keys.filter { indexed[$0] == nil }.sorted()
        if !missing.isEmpty { progress(0, missing.count) }
        for start in stride(from: 0, to: missing.count, by: 256) {
            let batch = Array(missing[start..<min(start + 256, missing.count)])
            let vectors = try await embedder.embedBatch(batch)
            try await commands.insertMany(zip(batch, vectors).map { doc, vector in
                var command = wanted[doc]!
                (command.terms, command.embedding) = (ShellTokenizer().encoded(doc), vector)
                return command
            })
            progress(start + batch.count, missing.count)
        }
        return wanted.mapValues(\.meta)
    }

    /// Each indexed document's metadata fingerprint. Projected, since decoding every embedding through
    /// `find()` took seconds.
    private func fingerprints(_ commands: TalaCollection<IndexedCommand>) async throws -> [String: String] {
        var result: [String: String] = [:]
        for row in try await commands.aggregate([["$project": ["doc": 1, "meta": 1]]]) {
            if case .object(let fields) = row, case .string(let doc) = fields["doc"], case .string(let meta) = fields["meta"] {
                result[doc] = meta
            }
        }
        return result
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
        return try CommandEmbedder()
    }

    /// The index file, ready to search; rebuilt when unreadable. The caller closes it.
    private func openIndex(_ name: String) async throws -> TalaDB {
        removeOldIndexes()
        let db: TalaDB
        do {
            db = try await openFile(name)
        } catch is CancellationError {
            throw CancellationError()
        } catch let error as TalaDBError where Self.isBusy(error) {
            throw error
        } catch {
            // The index is a cache of the history; rebuild it rather than fail.
            removeFiles(name)
            db = try await openFile(name)
        }
        do {
            let commands = db.collection("commands", as: IndexedCommand.self)
            for field in ["doc", "program", "subcommand", "author", "lastRun"] { try await commands.createIndex(field) }
            let indexes = try await commands.listIndexes()
            if !indexes.fts.contains("terms") { try await commands.createFtsIndex("terms") }
            if !indexes.vector.contains("embedding") {
                // Exact search: faster than HNSW below tens of thousands of commands.
                try await commands.createVectorIndex("embedding", dimensions: try makeEmbedder().dimension, metric: .cosine)
            }
        } catch {
            db.close()
            throw error
        }
        return db
    }

    /// Opens the file, retrying once if a process that skipped the flock (an older smcli) still has it open.
    private func openFile(_ name: String) async throws -> TalaDB {
        let url = directory.appendingPathComponent(name + ".taladb")
        do {
            return try await TalaDB.open(at: url)
        } catch let error as TalaDBError where Self.isBusy(error) {
            try await Task.sleep(for: .milliseconds(200))
            return try await TalaDB.open(at: url)
        }
    }

    private static func isBusy(_ error: TalaDBError) -> Bool {
        if case .engine(let message) = error { return message.contains("already open") }
        return false
    }

    /// The index file and any sidecars the engine keeps next to it.
    private func files(_ name: String) -> [URL] {
        let names = (try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []
        return names.filter { $0.hasPrefix(name + ".") }.map { directory.appendingPathComponent($0) }
    }

    private func removeFiles(_ name: String) {
        for file in files(name) { try? FileManager.default.removeItem(at: file) }
    }

    /// Every file of the index with its size and modification time: changes when any process writes it.
    private func stamp(_ name: String) -> [String] {
        let keys: [URLResourceKey] = [.fileSizeKey, .contentModificationDateKey]
        return files(name).map { file in
            let values = try? file.resourceValues(forKeys: Set(keys))
            return "\(file.path) \(values?.fileSize ?? -1) \(values?.contentModificationDate?.timeIntervalSince1970 ?? 0)"
        }.sorted()
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
