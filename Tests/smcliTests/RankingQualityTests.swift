import Foundation
import Testing
@testable import smcli
@testable import SmcliSearch
@testable import SmcliCore

/// A fixed query set over a deterministic synthetic history, scored by mean reciprocal rank and top-1/top-3.
/// `make ranking` prints the scores; the test fails when they fall below the floors, so a ranking change that makes
/// the set worse is caught. The fixture never touches real history: it builds real directories (manifests, `.git`
/// markers) under a temporary root, since project and Git context are read from disk.
struct RankingCase {
    enum Kind { case suggest, search }
    let group: String
    let kind: Kind
    /// Relative to the fixture root.
    let directory: String
    let query: String
    let expected: String
    var session: String? = nil
}

struct RankingScore {
    var reciprocal = 0.0, top1 = 0, top3 = 0, count = 0
    var mrr: Double { count == 0 ? 0 : reciprocal / Double(count) }
    var firstRate: Double { count == 0 ? 0 : Double(top1) / Double(count) }
    var thirdRate: Double { count == 0 ? 0 : Double(top3) / Double(count) }

    mutating func add(rank: Int?) {
        count += 1
        guard let rank else { return }
        reciprocal += 1 / Double(rank)
        if rank == 1 { top1 += 1 }
        if rank <= 3 { top3 += 1 }
    }

    var line: String { String(format: "MRR %.3f  top1 %.2f  top3 %.2f  (n=%d)", mrr, firstRate, thirdRate, count) }
}

final class RankingWorld {
    static let now = 1_800_000_000.0
    let root: String
    let log: CapturedHistory
    private let base: URL
    private var sequence = 0
    private var db: SQLiteDatabase!

    /// Picks the user made, as `select` records them: what was typed, what was chosen, how many days ago.
    static let selections: [(query: String, command: String, days: Double)] = [
        ("docker", "docker compose up -d", 1), ("docker", "docker compose up -d", 2), ("docker", "docker compose up -d", 4),
        ("git l", "git log --oneline -20", 1), ("git l", "git log --oneline -20", 3),
        ("start containers", "docker compose up -d", 2), ("start containers", "docker compose up -d", 5),
    ]

    init() throws {
        base = FileManager.default.temporaryDirectory.appendingPathComponent("rank-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        root = ProjectContext.realPath(base.path)!
        log = CapturedHistory(url: URL(fileURLWithPath: root + "/history.db"))
        try layout()
        db = try log.openConnection()
        try history()
        for pick in Self.selections {
            try log.recordPick(query: pick.query, command: pick.command, kind: "suggest", directory: path("notes"),
                               at: Self.now - pick.days * 86_400, on: db)
        }
    }

    deinit { try? FileManager.default.removeItem(at: base) }

    func path(_ relative: String) -> String { relative.isEmpty ? root : root + "/" + relative }

    private func write(_ text: String, _ file: String) throws {
        let url = URL(fileURLWithPath: path(file))
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try text.write(to: url, atomically: true, encoding: .utf8)
    }

    private func repository(_ directory: String, marker: String? = nil) throws {
        try write("ref: refs/heads/main\n", directory + "/.git/HEAD")
        if let marker { try write("x\n", directory + "/.git/" + marker) }
    }

    /// The directories the history and queries refer to.
    private func layout() throws {
        try repository("webapp")
        try write(#"{"name":"webapp","scripts":{"dev":"vite","build":"vite build","test":"vitest","lint":"eslint .","storybook":"storybook dev"}}"#,
                  "webapp/package.json")
        try repository("mono")
        try write(#"{"name":"mono","scripts":{"dev":"turbo dev"}}"#, "mono/package.json")
        try write(#"{"name":"ui","scripts":{"docs":"typedoc","test":"vitest"}}"#, "mono/packages/ui/package.json")
        try write(#"{"name":"core","scripts":{"bundle":"rollup -c"}}"#, "mono/packages/core/package.json")
        try repository("api")
        try write("""
            // swift-tools-version: 6.0
            import PackageDescription
            let package = Package(name: "api", targets: [
                .executableTarget(name: "App"), .target(name: "Lib"), .testTarget(name: "AppTests"),
            ])
            """, "api/Package.swift")
        try write(".PHONY: lint fmt release docs\nlint:\n\techo lint\nfmt:\n\techo fmt\nrelease:\n\techo release\ndocs:\n\techo docs\n",
                  "tools/Makefile")
        try repository("rebasing", marker: "rebase-merge/done")
        try repository("merging", marker: "MERGE_HEAD")
        for directory in ["notes", "other", "api/Sources/App", "api/Sources/Lib", "mono/packages/ui/src", "mono/packages/ui/stories"] {
            try FileManager.default.createDirectory(atPath: path(directory), withIntermediateDirectories: true)
        }
    }

    /// `count` runs spread evenly from `from` to `from + span` days before `now`, each in its own session so
    /// that habits create no command transitions of their own.
    private func habit(_ command: String, _ directory: String, _ count: Int, from: Double = 0.5, span: Double = 10,
                       exit: Int = 0) throws {
        for index in 0..<count {
            let age = from + span * Double(index) / Double(max(1, count - 1))
            try run(command, directory, at: Self.now - age * 86_400, session: "x\(sequence)", exit: exit)
        }
    }

    /// `first` followed by `second` in one session, `count` times.
    private func pair(_ first: String, _ second: String, _ directory: String, _ count: Int, from: Double = 0.5,
                      span: Double = 10) throws {
        for index in 0..<count {
            let at = Self.now - (from + span * Double(index) / Double(max(1, count - 1))) * 86_400
            let session = "p\(sequence)"
            try run(first, directory, at: at, session: session)
            try run(second, directory, at: at + 5, session: session)
        }
    }

    private func run(_ command: String, _ directory: String, at time: Double, session: String, exit: Int = 0) throws {
        sequence += 1
        // A distinct sub-millisecond offset per run keeps equal times from deciding rankings by insertion order.
        try log.append(command: command, shell: "zsh", directory: path(directory), exitCode: exit, nativeFile: "",
                       session: session, startedAt: time + Double(sequence) * 1e-4, on: db)
    }

    private func history() throws {
        // Directory habits.
        try habit("npm run dev", "webapp", 14)
        try habit("npm run build", "webapp", 8)
        try habit("npm test", "webapp", 12)
        try habit("git status", "webapp", 50)
        try habit("git diff", "webapp", 20)
        try habit("swift build", "api/Sources/Lib", 50)
        try habit("swift test --filter Lib", "api/Sources/Lib", 12)
        try habit("swift test --parallel", "tools", 16)
        try habit("swift build -c release", "tools", 15)
        try habit("make release", "other", 25)
        try habit("ls -la", "notes", 30)
        // Monorepo: root, the package being worked in, and a sibling package.
        try habit("npm run dev", "mono", 14)
        try habit("npm run docs", "mono/packages/ui/stories", 12, span: 8)
        try habit("npm run bundle", "mono/packages/core", 20)
        // Transitions: what follows what, within a session.
        try pair("git add -A", "git commit -m wip", "other", 14)
        try pair("git add -A", "git status", "other", 2)
        try pair("git add -A", "git diff --staged", "other", 1)
        try habit("git commit --amend", "other", 30)
        try pair("make fmt", "make lint", "tools", 10)
        // Picks: the history behind what the user chose.
        try habit("docker ps", "notes", 30)
        try habit("docker logs -f web", "notes", 10)
        try habit("docker compose up -d", "notes", 6)
        try habit("git ls-files", "other", 20)
        try habit("git log --oneline -20", "other", 8)
        // Git state: finished operations, and the habits a repository in the middle of one competes with.
        try habit("git rebase -i HEAD~3", "other", 3)
        try habit("git rebase --continue", "other", 2, from: 20, span: 5)
        try habit("git merge main", "other", 9)
        try habit("git mergetool", "other", 2)
        try habit("git merge --abort", "other", 1, from: 25, span: 1)
        try habit("git cherry-pick --continue", "other", 4)
        // The commands each live session just ran.
        try run("git add -A", "notes", at: Self.now - 30, session: "live1")
        try run("make fmt", "notes", at: Self.now - 40, session: "live2")
    }

    // MARK: - Queries

    static let cases: [RankingCase] = [
        // Directory: the command used where you are.
        RankingCase(group: "directory", kind: .suggest, directory: "webapp", query: "npm run ", expected: "npm run dev"),
        RankingCase(group: "directory", kind: .suggest, directory: "api/Sources/Lib", query: "swift b", expected: "swift build"),
        RankingCase(group: "directory", kind: .suggest, directory: "tools", query: "make r", expected: "make release"),
        RankingCase(group: "directory", kind: .search, directory: "api/Sources/Lib", query: "build", expected: "swift build"),
        RankingCase(group: "directory", kind: .search, directory: "webapp", query: "npm test", expected: "npm test"),
        // Project: commands from sibling directories of the same package outrank unrelated, more frequent ones.
        RankingCase(group: "project", kind: .suggest, directory: "api/Sources/App", query: "swift t", expected: "swift test --filter Lib"),
        RankingCase(group: "project", kind: .suggest, directory: "mono/packages/ui/src", query: "npm run ", expected: "npm run docs"),
        RankingCase(group: "project", kind: .search, directory: "api/Sources/App", query: "swift test", expected: "swift test --filter Lib"),
        RankingCase(group: "project", kind: .search, directory: "mono/packages/ui/src", query: "npm run", expected: "npm run docs"),
        // Transitions: what usually comes after the command you just ran.
        RankingCase(group: "transition", kind: .suggest, directory: "notes", query: "git ", expected: "git commit -m wip", session: "live1"),
        RankingCase(group: "transition", kind: .suggest, directory: "notes", query: "make ", expected: "make lint", session: "live2"),
        RankingCase(group: "transition", kind: .search, directory: "notes", query: "git commit", expected: "git commit -m wip", session: "live1"),
        // Selections: what you picked for a similar query before.
        RankingCase(group: "selection", kind: .suggest, directory: "other", query: "docker", expected: "docker compose up -d"),
        RankingCase(group: "selection", kind: .suggest, directory: "other", query: "git l", expected: "git log --oneline -20"),
        RankingCase(group: "selection", kind: .search, directory: "other", query: "start containers", expected: "docker compose up -d"),
        // Project scripts and targets that exist in a manifest but were never run.
        RankingCase(group: "scripts", kind: .suggest, directory: "webapp", query: "npm run sto", expected: "npm run storybook"),
        RankingCase(group: "scripts", kind: .suggest, directory: "tools", query: "make doc", expected: "make docs"),
        RankingCase(group: "scripts", kind: .search, directory: "webapp", query: "storybook", expected: "npm run storybook"),
        RankingCase(group: "scripts", kind: .search, directory: "tools", query: "docs", expected: "make docs"),
        // Guards: context signals must not disturb queries they have nothing to say about.
        RankingCase(group: "guard", kind: .suggest, directory: "notes", query: "ls", expected: "ls -la", session: "live1"),
        RankingCase(group: "guard", kind: .suggest, directory: "rebasing", query: "git s", expected: "git status"),
        RankingCase(group: "guard", kind: .suggest, directory: "webapp", query: "npm run b", expected: "npm run build"),
        RankingCase(group: "guard", kind: .search, directory: "rebasing", query: "show running containers", expected: "docker ps"),
        // Git state: the way out of an operation in progress.
        RankingCase(group: "git state", kind: .suggest, directory: "rebasing", query: "git ", expected: "git rebase --continue"),
        RankingCase(group: "git state", kind: .suggest, directory: "merging", query: "git m", expected: "git merge --continue"),
        RankingCase(group: "git state", kind: .search, directory: "rebasing", query: "continue", expected: "git rebase --continue"),
        RankingCase(group: "git state", kind: .search, directory: "merging", query: "continue", expected: "git merge --continue"),
    ]

    // MARK: - Adapter: how a case is answered, by the code under test

    /// `SMCLI_RANKING_OFF=project,transition,...` switches signals off, to measure each one.
    func context(_ item: RankingCase) -> RankingContext {
        RankingContext(project: ProjectContext.discover(path(item.directory)), session: item.session,
                       environment: ProcessInfo.processInfo.environment)
    }

    func suggest(_ item: RankingCase) throws -> [String] {
        try log.suggestions(user: NSUserName(), directory: path(item.directory), prefix: item.query, limit: 10,
                            now: Self.now, context: context(item)).map(\.command)
    }

    #if canImport(ProximaKit)
    private lazy var engine: HistorySearch = {
        let model = try! PotionModel(directory: URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .appendingPathComponent("../../Models/\(PotionModel.name)").standardizedFileURL)
        return HistorySearch(directory: URL(fileURLWithPath: path("index")), potion: PotionEmbedder(model: model))
    }()

    /// Why the last search ranked each command where it did, for the miss report.
    var reasons: [String: [String]] = [:]

    func search(_ item: RankingCase) async throws -> [String] {
        let directory = path(item.directory)
        let context = context(item)
        let corpus = searchCorpus(captured: try log.commandStats(directory: directory, project: context.projectRoot, now: Self.now), native: [])
        let signals = try searchSignals(captured: log, context: context)
        let hits = try await engine.search(item.query, corpus: corpus, limit: 10, signals: signals, now: Self.now)
        reasons = Dictionary(hits.map { ($0.command, $0.why ?? []) }, uniquingKeysWith: { first, _ in first })
        return hits.map(\.command)
    }
    #endif
}

struct RankingQualityTests {
    /// Floors for the scores printed by `make ranking`, set just under what the current ranking achieves.
    static let floors: [RankingCase.Kind: (mrr: Double, top1: Double)] = [.suggest: (0.95, 0.9), .search: (0.9, 0.85)]

    @Test func `Rank the fixed query set above the regression floor`() async throws {
        let world = try RankingWorld()
        var scores: [String: RankingScore] = [:]
        var total: [RankingCase.Kind: RankingScore] = [:]
        var misses: [String] = []
        for item in RankingWorld.cases {
            let ranked: [String]
            switch item.kind {
            case .suggest: ranked = try world.suggest(item)
            case .search:
                #if canImport(ProximaKit)
                ranked = try await world.search(item)
                #else
                continue
                #endif
            }
            let rank = ranked.firstIndex(of: item.expected).map { $0 + 1 }
            scores["\(item.kind == .suggest ? "suggest" : "search") \(item.group)", default: RankingScore()].add(rank: rank)
            total[item.kind, default: RankingScore()].add(rank: rank)
            if rank != 1 {
                misses.append("  \(item.kind) [\(item.group)] \(item.query.debugDescription) in \(item.directory): wanted "
                              + "\(item.expected.debugDescription), rank \(rank.map(String.init) ?? "none"); got \(ranked.prefix(3))"
                              + (item.kind == .search ? "\n      why: \((ranked.prefix(1) + [item.expected]).map { "\($0): \(world.reasons[$0]?.joined(separator: ", ") ?? "-")" })" : ""))
            }
        }
        print("ranking:")
        func label(_ text: String) -> String { text.padding(toLength: 24, withPad: " ", startingAt: 0) }
        for key in scores.keys.sorted() { print("  \(label(key)) \(scores[key]!.line)") }
        for kind in [RankingCase.Kind.suggest, .search] {
            if let score = total[kind] { print("  \(label((kind == .suggest ? "suggest" : "search") + " TOTAL")) \(score.line)") }
        }
        if !misses.isEmpty { print("ranking misses:\n" + misses.joined(separator: "\n")) }
        // Floors only hold with every signal on; SMCLI_RANKING_OFF is for measuring one at a time.
        for (kind, floor) in Self.floors where ProcessInfo.processInfo.environment["SMCLI_RANKING_OFF"] == nil {
            guard let score = total[kind] else { continue }
            #expect(score.mrr >= floor.mrr, "\(kind) MRR \(score.mrr) fell below \(floor.mrr)")
            #expect(score.firstRate >= floor.top1, "\(kind) top-1 \(score.firstRate) fell below \(floor.top1)")
        }
    }
}
