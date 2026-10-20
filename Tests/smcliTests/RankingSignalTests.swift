import Foundation
import Testing
@testable import smcli
@testable import SmcliSearch
@testable import SmcliCore

struct RankingSignalTests {
    private func temporary() throws -> URL {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("sig-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return URL(fileURLWithPath: ProjectContext.realPath(url.path)!)
    }

    private func write(_ text: String, _ path: String, in directory: URL) throws {
        let url = directory.appendingPathComponent(path)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try text.write(to: url, atomically: true, encoding: .utf8)
    }

    @Test func `Offer only scripts and targets a manifest names, as plain words`() throws {
        let root = try temporary()
        defer { try? FileManager.default.removeItem(at: root) }
        try write(#"{"name":"x","packageManager":"pnpm@9","scripts":{"dev":"a","build":"b","bad name":"c","x;rm -rf ~":"d","$(id)":"e","test":"t"}}"#,
                  "package.json", in: root)
        try write("lint:\n\techo\nVAR := 1\n%.o: %.c\n\tcc\n.PHONY: lint\n", "Makefile", in: root)
        let commands = ProjectContext.discover(root.path).candidates().map(\.command)
        #expect(commands.contains("pnpm run dev") && commands.contains("pnpm run build") && commands.contains("pnpm run test"))
        #expect(commands.contains("make lint"))
        #expect(!commands.contains { $0.contains(";") || $0.contains("$(") || $0.contains("bad name") })
        #expect(!commands.contains("make VAR") && !commands.contains("make %.o"))
        #expect(!commands.contains("pnpm run deploy") && !commands.contains("make deploy"))
        // Nothing outside a project, and nothing is executed to find out.
        let empty = try temporary()
        defer { try? FileManager.default.removeItem(at: empty) }
        #expect(ProjectContext.discover(empty.path).candidates().isEmpty)
    }

    @Test func `Point to the way out of an operation in progress`() throws {
        let root = try temporary()
        defer { try? FileManager.default.removeItem(at: root) }
        try write("ref: refs/heads/main\n", ".git/HEAD", in: root)
        #expect(ProjectContext.discover(root.path).candidates().isEmpty)
        try write("x", ".git/rebase-merge/done", in: root)
        let rebase = ProjectContext.discover(root.path).candidates()
        #expect(rebase.map(\.command) == ["git rebase --continue", "git rebase --abort", "git rebase --skip"])
        #expect(rebase.allSatisfy { $0.kind == .hint && $0.reason == "rebase in progress" })
        try FileManager.default.removeItem(at: root.appendingPathComponent(".git/rebase-merge"))
        try write("x", ".git/MERGE_HEAD", in: root)
        #expect(ProjectContext.discover(root.path).candidates().map(\.command) == ["git merge --continue", "git merge --abort"])
    }

    @Test func `Weigh picks by how similar the query is and how recent`() {
        #expect(SelectionLog.similarity("docker", "docker") == 1)
        #expect(SelectionLog.similarity("doc", "docker") == 0.6 && SelectionLog.similarity("docker ps", "docker") == 0.25)
        #expect(SelectionLog.similarity("", "docker") == 0 && SelectionLog.similarity("", "") == 1)
        #expect(SelectionLog.similarity("start containers", "start the containers") > 0)
        #expect(SelectionLog.similarity("git log", "docker ps") == 0)
        var log = SelectionLog()
        let now = 1_700_000_000.0
        log.add(query: "  Docker  ", command: "docker compose up", at: now)
        log.add(query: "docker", command: "docker compose up", at: now - CapturedHistory.usageHalfLife)
        log.add(query: "make", command: "make test", at: now)
        let weights = log.weights(query: "docker", now: now)
        #expect(abs(weights["docker compose up"]! - 1.5) < 1e-9 && weights["make test"] == nil)
    }

    @Test func `Store picks, drop them with the command, and survive a reopen`() throws {
        let root = try temporary()
        defer { try? FileManager.default.removeItem(at: root) }
        let log = CapturedHistory(url: root.appendingPathComponent("h.db"))
        try log.append(command: "docker compose up", shell: "zsh", directory: "/r", exitCode: 0, nativeFile: "", session: "s")
        let db = try log.openConnection()
        try log.recordPick(query: "Docker", command: "docker compose up", kind: "suggest", directory: "/r", on: db)
        try log.recordPick(query: "other", command: "ls", kind: "search", directory: "/r", on: db)
        #expect(try log.picks(on: db).picks.map(\.query) == ["docker", "other"])
        var config = SmcliConfig()
        config.historyFilter = ["^docker "]
        _ = try log.prune(filter: CaptureFilter(config: config), dryRun: false)
        #expect(try log.picks(on: db).picks.map(\.command) == ["ls"])
    }

    @Test func `Rank a command that follows the previous one, and a pick, above plain frequency`() throws {
        let root = try temporary()
        defer { try? FileManager.default.removeItem(at: root) }
        let log = CapturedHistory(url: root.appendingPathComponent("h.db"))
        let now = 1_700_000_000.0
        for index in 0..<10 {
            try log.append(command: "git status", shell: "zsh", directory: "/r", exitCode: 0, nativeFile: "", session: "x\(index)", startedAt: now - 100 - Double(index))
        }
        for index in 0..<4 {
            let at = now - 2_000 - Double(index) * 100
            try log.append(command: "git add -A", shell: "zsh", directory: "/r", exitCode: 0, nativeFile: "", session: "p\(index)", startedAt: at)
            try log.append(command: "git commit -m x", shell: "zsh", directory: "/r", exitCode: 0, nativeFile: "", session: "p\(index)", startedAt: at + 5)
        }
        try log.append(command: "git add -A", shell: "zsh", directory: "/r", exitCode: 0, nativeFile: "", session: "live", startedAt: now - 5)
        func top(_ session: String?) throws -> Suggestion {
            try log.suggestions(user: NSUserName(), directory: "/r", prefix: "git ", limit: 1, now: now,
                                context: RankingContext(session: session))[0]
        }
        #expect(try top(nil).command == "git status")
        let boosted = try top("live")
        #expect(boosted.command == "git commit -m x" && boosted.why == ["used here", "often follows git add -A"])
        // A pick for the same prefix wins too, and says so.
        let db = try log.openConnection()
        for _ in 0..<3 { try log.recordPick(query: "git ", command: "git add -A", kind: "suggest", directory: "/r", at: now - 10, on: db) }
        let picked = try log.suggestions(user: NSUserName(), directory: "/r", prefix: "git ", limit: 1, now: now)[0]
        #expect(picked.command == "git add -A" && picked.why?.last == "picked for similar searches")
    }

    @Test func `Put the way out of a rebase first, but only where the prefix allows`() {
        let context = RankingContext(candidates: [
            .init(command: "git rebase --continue", kind: .hint, reason: "rebase in progress"),
            .init(command: "git rebase --abort", kind: .hint, reason: "rebase in progress"),
        ])
        let index = SuggestionIndex(runs: (0..<5).map { .init(command: "git status", directory: "/r", startedAt: 1_700_000_000 + Double($0), exitCode: 0) })
        let all = index.suggestions(directory: "/r", prefix: "git ", now: 1_700_000_100, context: context)
        #expect(all.map(\.command) == ["git rebase --continue", "git rebase --abort", "git status"])
        #expect(all[0].why == ["rebase in progress"] && all[0].runs == 0 && all[0].lastRun.isEmpty)
        #expect(index.suggestions(directory: "/r", prefix: "git s", now: 1_700_000_100, context: context).map(\.command) == ["git status"])
        let off = RankingContext(candidates: context.candidates, off: [.hint])
        #expect(index.suggestions(directory: "/r", prefix: "git ", now: 1_700_000_100, context: off).map(\.command) == ["git status"])
    }

    @Test func `Keep usage and context out of the embedded text`() {
        let entry = SearchEntry(command: "git commit --amend", runs: 3, lastRun: "2023-11-14T22:13:20Z", lastDirectory: "/r",
                                lastExitCode: 0, source: "captured", flags: ["--amend"])
        let used = SearchEntry(command: "git commit --amend", runs: 40, lastRun: "2023-11-15T22:13:20Z", lastDirectory: "/elsewhere",
                               lastExitCode: 1, source: "captured", flags: ["--amend"], frecency: 9, successes: 3, runsHere: 4,
                               runsProject: 4, why: ["used here"])
        #expect(searchDocument(used) == searchDocument(entry) && searchDocument(used, labeled: false) == searchDocument(entry, labeled: false))
    }

    @Test func `Explain every signal in a search result and keep semantic hits without a keyword match`() {
        let now = 1_700_000_000.0
        func entry(_ command: String, runs: Int = 4, runsHere: Int = 0, runsProject: Int = 0) -> SearchEntry {
            SearchEntry(command: command, runs: runs, lastRun: "2023-11-14T22:13:20Z", lastDirectory: "/r", lastExitCode: 0,
                        source: "captured", successes: runs, runsHere: runsHere, runsProject: runsProject)
        }
        var picks = SelectionLog()
        picks.add(query: "undo commit", command: "git reset --soft HEAD~1", at: now)
        let signals = SearchSignals(
            context: RankingContext(candidates: [.init(command: "git rebase --abort", kind: .hint, reason: "rebase in progress")]),
            previous: "git add -A", followers: ["git commit -m x": 3], picks: picks)
        let ranked = rerank([(entry("git log"), 1), (entry("git reset --soft HEAD~1"), 0.2), (entry("git commit -m x", runsProject: 2), 0.3),
                             (entry("git rebase --abort"), 0.3)], literal: false, query: "undo commit", now: now, signals: signals)
        func why(_ command: String) -> [String] { ranked.first { $0.command == command }?.why ?? [] }
        #expect(why("git reset --soft HEAD~1").contains("picked for similar searches"))
        #expect(why("git commit -m x").contains("often follows git add -A") && why("git commit -m x").contains("used in this project"))
        // The hint matches none of the query's words, so it gets no lift; and a semantic hit stays a semantic hit.
        #expect(!why("git rebase --abort").contains("rebase in progress"))
        #expect(why("git log").first == "semantic match")
        // An identical query earlier lifts the pick over a hit that is otherwise closer.
        #expect(ranked.first?.command == "git reset --soft HEAD~1")
        // Words in the query bring in project commands the index did not return, never ones that do not match.
        let additions = SearchSignals(context: RankingContext(candidates: [
            .init(command: "npm run storybook", kind: .script, reason: "package.json script"),
            .init(command: "npm run build", kind: .script, reason: "package.json script"),
        ])).additions(query: "story", corpus: [:], existing: [], filter: SearchFilter(), now: now)
        #expect(additions.map(\.entry.command) == ["npm run storybook"] && additions[0].entry.source == "project")
    }
}
