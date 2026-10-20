import Foundation
import Testing
@testable import smcli
@testable import SmcliSearch
@testable import SmcliCore

struct ServiceTests {
    private func temporaryHistory() -> (CapturedHistory, URL) {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        return (CapturedHistory(url: home.appendingPathComponent("captured.db")), home)
    }

    @Test func `A retried event ID stores one row and reports the duplicate`() throws {
        let (log, home) = temporaryHistory()
        defer { try? FileManager.default.removeItem(at: home) }
        let event = UUID().uuidString.lowercased()
        #expect(try log.append(command: "make test", shell: "zsh", directory: "/repo", exitCode: 0, nativeFile: "",
                               session: "one", startedAt: 1_700_000_000, event: event))
        #expect(try !log.append(command: "make test", shell: "zsh", directory: "/repo", exitCode: 0, nativeFile: "",
                                session: "one", startedAt: 1_700_000_000, event: event))
        #expect(try log.records().map(\.id) == [event])
        for bad in ["", "not-a-uuid", event.uppercased(), event + "x"] {
            #expect(throws: HistoryError.self) {
                try log.append(command: "ls", shell: "zsh", directory: "/", exitCode: 0, nativeFile: "", event: bad)
            }
        }
    }

    @Test func `The in-memory index ranks exactly like the database scan`() throws {
        let (log, home) = temporaryHistory()
        defer { try? FileManager.default.removeItem(at: home) }
        let commands = ["git status", "git push", "grep -r TODO .", "make test", "cargo build", "smcli suggest",
                        "ls 'my dir'", "echo héllo", "npm run build", "swift test"]
        let directories = ["/", "/repo", "/repo/sub", "/repo/sub/deep", "/other", "/repo2", "/repo/pkg/a", "/repo/pkg/b"]
        let exitCodes = [0, 0, 0, 1, 2, 130, 141]
        let sessions = ["s1", "s2", "s3"]
        var generator = SystemRandomNumberGenerator()
        var runs: [CapturedHistory.Run] = []
        let start = 1_700_000_000.0
        for step in 0..<400 {
            let run = CapturedHistory.Run(command: commands.randomElement(using: &generator)!,
                                          directory: directories.randomElement(using: &generator)!,
                                          startedAt: start + Double(step) * 3_600,
                                          exitCode: exitCodes.randomElement(using: &generator)!,
                                          session: sessions.randomElement(using: &generator)!)
            runs.append(run)
            try log.append(command: run.command, shell: "zsh", directory: run.directory, exitCode: run.exitCode,
                           nativeFile: "", session: run.session, startedAt: run.startedAt)
        }
        let now = start + 400 * 3_600 + 60
        let db = try log.openConnection()
        // Picks for some prefixes, some of them similar to a query asked below.
        let picks: [(query: String, command: String, time: Double)] = [
            ("git", "git push", now - 3_600), ("g", "git status", now - 86_400), ("make", "make test", now - 5 * 86_400),
            ("", "ls 'my dir'", now - 7_200), ("echo hé", "echo héllo", now - 100),
        ]
        for pick in picks { try log.recordPick(query: pick.query, command: pick.command, kind: "suggest", directory: "/", filter: .standard, at: pick.time, on: db) }
        #expect(try log.runs(user: NSUserName(), on: db) == runs)
        // Built from half the runs and then fed the rest, as the service does between records.
        var index = SuggestionIndex(runs: Array(runs.prefix(200)), picks: try log.picks(on: db))
        for run in runs.dropFirst(200) { index.add(run) }
        let candidates: [RankingContext.Candidate] = [
            .init(command: "swift test --filter A", kind: .script, reason: "Package.swift target"),
            .init(command: "make test", kind: .script, reason: "Makefile target"),
            .init(command: "git rebase --continue", kind: .hint, reason: "rebase in progress"),
            .init(command: "git push", kind: .hint, reason: "x"),
        ]
        let contexts: [RankingContext?] = [
            nil, RankingContext(projectRoot: "/repo", session: "s1"),
            RankingContext(projectRoot: "/repo/pkg", session: "s2", candidates: candidates),
            RankingContext(projectRoot: "/repo/pkg/a", session: "nobody", candidates: candidates),
            RankingContext(projectRoot: "/", candidates: candidates),
            RankingContext(projectRoot: "/repo", session: "s3", candidates: candidates, off: [.transition, .hint]),
        ]
        for context in contexts {
            for directory in directories + ["/repo/", "/elsewhere"] {
                for prefix in ["", "git", "g", "smcli", "echo h", "zzz", "swift", "make "] {
                    let label = "\(directory) \(prefix) \(String(describing: context?.projectRoot)) \(String(describing: context?.session))"
                    let expected = try log.suggestions(user: NSUserName(), directory: directory, prefix: prefix, limit: 5, now: now, context: context)
                    let actual = index.suggestions(directory: directory, prefix: prefix, limit: 5, now: now, context: context)
                    #expect(actual.map(\.command) == expected.map(\.command), "\(label)")
                    #expect(actual.map(\.runs) == expected.map(\.runs))
                    #expect(actual.map(\.runsHere) == expected.map(\.runsHere))
                    #expect(actual.map(\.lastRun) == expected.map(\.lastRun))
                    #expect(actual.map(\.lastExitCode) == expected.map(\.lastExitCode))
                    #expect(actual.map(\.why) == expected.map(\.why), "\(label)")
                    for (a, e) in zip(actual, expected) { #expect(abs(a.score - e.score) <= 1e-9 * max(1, e.score)) }
                }
            }
        }
        let recent = Picker.recent(try log.commandStats()).filter { !isSmcliCommand($0) }
        #expect(index.recent() == recent)
        // The session's previous command and what followed it agree too.
        for session in sessions {
            let previous = try log.previousCommand(session: session, user: NSUserName(), on: db)
            #expect(index.previousCommand(session: session) == previous)
            if let previous { #expect(index.following(previous) == (try log.followers(after: previous, user: NSUserName(), on: db))) }
        }
    }

    @Test func `The search corpus updated per run equals one rebuilt from the database`() throws {
        let (log, home) = temporaryHistory()
        defer { try? FileManager.default.removeItem(at: home) }
        let commands = ["git status", "grep -r TODO .", "make test", "cargo build", "smcli suggest", "echo héllo", "diff a b",
                        "npm run build", "swift test", "docker ps"]
        let directories = ["/", "/repo", "/repo/sub", "/repo/pkg/a", "/other", "/repo2"]
        let exitCodes = [0, 0, 1, 2, 130, 141]
        var generator = SystemRandomNumberGenerator()
        var runs: [CapturedHistory.Run] = []
        let start = 1_700_000_000.0
        for step in 0..<300 {
            let run = CapturedHistory.Run(command: commands.randomElement(using: &generator)!,
                                          directory: directories.randomElement(using: &generator)!,
                                          startedAt: start + Double(step) * 1_800,
                                          exitCode: exitCodes.randomElement(using: &generator)!, session: "s")
            runs.append(run)
            try log.append(command: run.command, shell: "zsh", directory: run.directory, exitCode: run.exitCode,
                           nativeFile: "", session: "s", startedAt: run.startedAt)
        }
        let asOf = start + 300 * 1_800 + 60
        let native = ["git status", "ls -la", "make test", "ls -la", "npm run build"]
        for (directory, project) in [("/repo", "/repo/pkg"), ("/repo/sub", nil), ("/", "/"), ("", nil)] as [(String, String?)] {
            // The first half is "already in the corpus"; the second half arrives as records do.
            let halfway = start + 150 * 1_800 - 1
            var half = CorpusState(captured: try statsUpTo(log, halfway, directory: directory, project: project, now: asOf),
                                   native: native, directory: directory, project: project, asOf: asOf)
            for run in runs where run.startedAt > halfway { half.add(run) }
            let rebuilt = CorpusState(captured: try log.commandStats(directory: directory.isEmpty ? nil : directory, project: project, now: asOf),
                                      native: native, directory: directory, project: project, asOf: asOf)
            #expect(Set(half.entries.keys) == Set(rebuilt.entries.keys))
            #expect(half.programs == rebuilt.programs)
            for (command, expected) in rebuilt.entries {
                var actual = try #require(half.entries[command])
                #expect(abs(actual.frecency - expected.frecency) <= 1e-9 * max(1, expected.frecency), "\(command) \(directory)")
                actual.frecency = expected.frecency
                #expect(actual == expected, "\(command) \(directory) \(String(describing: project))")
            }
        }
    }

    /// `commandStats` as of an earlier moment: the database copy holds only runs up to `time`.
    private func statsUpTo(_ log: CapturedHistory, _ time: Double, directory: String, project: String?, now: Double) throws -> [String: CommandStats] {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        let earlier = CapturedHistory(url: home.appendingPathComponent("earlier.db"))
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        for record in try log.records() {
            let started = formatter.date(from: record.timestamp)!.timeIntervalSince1970
            guard started <= time else { continue }
            try earlier.append(command: record.command, shell: "zsh", directory: record.directory, exitCode: record.exitCode,
                               nativeFile: "", session: record.session, startedAt: started)
        }
        return try earlier.commandStats(directory: directory.isEmpty ? nil : directory, project: project, now: now)
    }

    @Test func `Requests decode strictly and responses round-trip`() throws {
        let decoder = JSONDecoder()
        #expect(throws: (any Error).self) { try decoder.decode(ServiceRequest.self, from: Data(#"{"v":1,"op":"rm"}"#.utf8)) }
        #expect(throws: (any Error).self) { try decoder.decode(ServiceRequest.self, from: Data("not json".utf8)) }
        let request = try decoder.decode(ServiceRequest.self, from: Data(#"{"v":1,"op":"suggest","directory":"/","limit":3}"#.utf8))
        #expect(request.op == .suggest && request.limit == 3 && request.prefix == nil)
        var response = ServiceResponse(ok: true)
        response.event = UUID().uuidString.lowercased()
        response.duplicate = true
        #expect(try decoder.decode(ServiceResponse.self, from: JSONEncoder().encode(response)) == response)
    }

    @Test func `The service is on unless switched off`() {
        func mode(_ value: String?) -> ServiceClient.Mode {
            ServiceClient(database: URL(fileURLWithPath: "/a/history.db"),
                          environment: value.map { ["SMCLI_SERVICE": $0] } ?? [:], cliExecutable: nil).mode
        }
        #expect(mode(nil) == .spawn && mode("1") == .spawn && mode("on") == .spawn && mode("") == .spawn)
        #expect(mode("connect") == .connect)
        #expect(mode("0") == .off && mode("off") == .off && mode("OFF") == .off && mode("false") == .off)
    }

    @Test func `A build is identified by path, time and size`() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let file = directory.appendingPathComponent("smcli-service")
        let link = directory.appendingPathComponent("link")
        try Data("one".utf8).write(to: file)
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: file)
        let first = try #require(ServiceProtocol.build(of: file))
        #expect(ServiceProtocol.build(of: link) == first, "a symlink names the same build as its target")
        try Data("longer".utf8).write(to: file)
        #expect(ServiceProtocol.build(of: file) != first)
        #expect(ServiceProtocol.build(of: directory.appendingPathComponent("missing")) == nil)
    }

    @Test func `Record requests carry an optional pipestatus`() throws {
        var request = ServiceRequest(op: .record)
        request.pipestatus = [0, 1]
        #expect(try JSONDecoder().decode(ServiceRequest.self, from: JSONEncoder().encode(request)).pipestatus == [0, 1])
        let legacy = try JSONDecoder().decode(ServiceRequest.self, from: Data(#"{"v":1,"op":"ping"}"#.utf8))
        #expect(legacy.pipestatus == nil)
        let reply = try JSONDecoder().decode(ServiceResponse.self, from: Data(#"{"v":1,"ok":true}"#.utf8))
        #expect(reply.build == nil)
    }

    @Test func `Each database gets its own socket, named from the path as given`() {
        let environment = ["SMCLI_RUNTIME_DIR": "/run/x"]
        let one = ServiceLocation(database: URL(fileURLWithPath: "/a/history.db"), environment: environment)
        let two = ServiceLocation(database: URL(fileURLWithPath: "/b/history.db"), environment: environment)
        #expect(one.socket != two.socket)
        #expect(one.socket.deletingLastPathComponent().path == "/run/x")
        #expect(ServiceClient.serviceExecutable(beside: URL(fileURLWithPath: "/x/smcli")).path == "/x/smcli-service")
        #expect(ServiceClient.serviceExecutable(beside: URL(fileURLWithPath: "/x/smcli-20261019-abc")).path
                == "/x/smcli-service-20261019-abc")
    }
}

struct ServiceClientReplayTests {
    @Test func `After replacing an old build's service, only repeatable requests are sent again`() {
        #expect(!ServiceClient.repeatable(.select))
        #expect(!ServiceClient.repeatable(.shutdown))
        for op: ServiceRequest.Operation in [.ping, .record, .suggest, .pick, .search, .context, .files, .grep] {
            #expect(ServiceClient.repeatable(op))
        }
    }
}
