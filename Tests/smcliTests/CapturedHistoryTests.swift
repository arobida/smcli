import Foundation
import Testing
#if canImport(SQLite3)
import SQLite3
#else
import CSQLite
#endif
@testable import smcli
@testable import SmcliSearch
@testable import SmcliCore

struct CapturedHistoryTests {
    @Test func `Break down simple commands without evaluating arguments`() {
        let git = CommandBreakdown.parse("git commit -m 'hello world' --amend -- path/file")
        #expect(git?.mainCommand == "git")
        #expect(git?.subcommand == "commit")
        #expect(git?.flags == [.init(name: "-m", value: "hello world"), .init(name: "--amend", value: nil)])
        #expect(git?.targets == ["path/file"])
        #expect(git?.arguments == ["commit", "-m", "hello world", "--amend", "--", "path/file"])
        let ls = CommandBreakdown.parse("COLOR=1 /bin/ls -la 'my directory' -- -filename")
        #expect(ls?.mainCommand == "/bin/ls")
        #expect(ls?.subcommand == nil)
        #expect(ls?.flags == [.init(name: "-la", value: nil)])
        #expect(ls?.targets == ["my directory", "-filename"])
        let status = CommandBreakdown.parse("git -C repo status --format=short file")
        #expect(status?.subcommand == "status")
        #expect(status?.flags == [.init(name: "-C", value: "repo"), .init(name: "--format", value: "short")])
        #expect(status?.targets == ["file"])
        #expect(shellWords("echo \"$HOME\" '' a\\ b") == ["echo", "$HOME", "", "a b"])
        #expect(shellWords("echo \"a\\qb\"") == ["echo", "a\\qb"])
    }

    @Test(arguments: ["ls | grep txt", "echo $(whoami)", "echo `pwd`", "cd a && pwd", "ls > out", "echo 'unfinished", "echo a\necho b", "echo $'hello\\nworld'"])
    func `Do not invent a breakdown for compound or malformed syntax`(command: String) {
        #expect(CommandBreakdown.parse(command) == nil)
    }

    @Test func `Capture metadata and exact command counts without modifying native history`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        let log = CapturedHistory(url: home.appendingPathComponent("captured.db"))
        try log.append(command: "git status --short", shell: "bash", directory: "/first", exitCode: 0, nativeFile: "",
                       session: "one", startedAt: 1_700_000_000, endedAt: 1_700_000_001.25)
        try log.append(command: "git status --short", shell: "zsh", directory: "/second", exitCode: 1, nativeFile: "",
                       session: "two", startedAt: 1_700_000_002)
        try log.append(command: "git status", shell: "bash", directory: "/first", exitCode: 0, nativeFile: "", session: "one")
        let records = try log.records()
        #expect(records.map(\.runCount) == [1, 2, 1])
        #expect(records.map(\.exitCode) == [0, 1, 0])
        #expect(records.map(\.directory) == ["/first", "/second", "/first"])
        #expect(records.map(\.session) == ["one", "two", "one"])
        #expect(records.map(\.durationMs) == [1250, nil, nil])
        #expect(records[0].timestamp == "2023-11-14T22:13:20.000Z")
        #expect(records.allSatisfy { $0.user == NSUserName() && !$0.host.isEmpty })
        #expect(records[0].breakdown?.subcommand == "status")
        let attributes = try FileManager.default.attributesOfItem(atPath: log.url.path)
        #expect((attributes[.posixPermissions] as? NSNumber)?.intValue == 0o600)
        #expect(throws: (any Error).self) {
            try log.append(command: "echo dangerous", shell: "bash", directory: "/", exitCode: 0, nativeFile: log.url.path)
        }
        let linked = home.appendingPathComponent("native")
        try FileManager.default.linkItem(at: log.url, to: linked)
        #expect(throws: (any Error).self) {
            try log.append(command: "echo dangerous", shell: "bash", directory: "/", exitCode: 0, nativeFile: linked.path)
        }
        #expect(try log.records().count == 3)
    }

    @Test func `Order by start time so background writers cannot reorder history`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        let log = CapturedHistory(url: home.appendingPathComponent("captured.db"))
        try log.append(command: "late", shell: "zsh", directory: "/", exitCode: 0, nativeFile: "", startedAt: 20)
        try log.append(command: "early", shell: "zsh", directory: "/", exitCode: 0, nativeFile: "", startedAt: 10, endedAt: 9)
        let records = try log.records()
        #expect(records.map(\.command) == ["early", "late"])
        #expect(records[0].durationMs == 0)
    }

    @Test func `Import legacy JSONL logs`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        let legacy = home.appendingPathComponent("old.jsonl")
        try Data("""
            {"command":"ls","directory":"/tmp","exitCode":0,"runCount":1,"shell":"zsh","timestamp":"2026-10-12T10:00:00Z","user":"alice"}
            {"command":"ls","directory":"/","exitCode":2,"runCount":2,"shell":"bash","timestamp":"2026-10-12T10:01:00Z","user":"alice"}

            """.utf8).write(to: legacy)
        let log = CapturedHistory(url: home.appendingPathComponent("captured.db"))
        #expect(try log.importLegacy(from: legacy) == 2)
        let records = try log.records()
        #expect(records.map(\.runCount) == [1, 2])
        #expect(records.map(\.user) == ["alice", "alice"])
        #expect(records[1].exitCode == 2 && records[1].durationMs == nil)
        try Data("not JSON\n".utf8).write(to: legacy)
        #expect(throws: (any Error).self) { try log.importLegacy(from: legacy) }
        #expect(try log.records().count == 2)
    }

    @Test func `Import Atuin history once, keeping its metadata`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        let atuin = home.appendingPathComponent("atuin.db")
        let source = try fixtureDatabase(atuin)
        try source.execute("""
            CREATE TABLE history (id TEXT PRIMARY KEY, timestamp INTEGER NOT NULL, duration INTEGER NOT NULL,
                exit INTEGER NOT NULL, command TEXT NOT NULL, cwd TEXT NOT NULL, session TEXT NOT NULL,
                hostname TEXT NOT NULL, deleted_at INTEGER, author TEXT, intent TEXT);
            INSERT INTO history VALUES ('a1', 1760000000000000000, 1500000000, 0, 'ls', '/tmp', 's1', 'mac:alice', NULL, 'alice', NULL);
            INSERT INTO history VALUES ('a2', 1760000001000000000, -1, 1, 'make', '/src', 's1', 'mac:alice', NULL, 'claude', 'build it');
            INSERT INTO history VALUES ('a3', 1760000002000000000, 0, 0, 'secret', '/', 's1', 'mac:alice', 1760000003000000000, NULL, NULL);
            """)
        let log = CapturedHistory(url: home.appendingPathComponent("captured.db"))
        #expect(try log.importAtuin(from: atuin) == 3)
        #expect(try log.importAtuin(from: atuin) == 0)
        let records = try log.records()
        #expect(records.map(\.command) == ["ls", "make"])
        #expect(records.map(\.id) == ["a1", "a2"])
        #expect(records[0].user == "alice" && records[0].host == "mac" && records[0].durationMs == 1500)
        #expect(records[0].timestamp == "2025-10-09T08:53:20.000Z")
        #expect(records[1].durationMs == nil && records[1].author == "claude" && records[1].intent == "build it")
        #expect(records[1].shell == "unknown" && records[1].session == "atuin-s1")
        #expect(throws: (any Error).self) { try log.importAtuin(from: home.appendingPathComponent("missing.db")) }
        try log.append(command: "local only", shell: "zsh", directory: "/", exitCode: 0, nativeFile: "")
        #expect(throws: (any Error).self) { try log.importAtuin(from: home.appendingPathComponent("missing.db"), replace: true) }
        #expect(try log.records().count == 3)
        #expect(try log.importAtuin(from: atuin, replace: true) == 3)
        #expect(try log.records().map(\.command) == ["ls", "make"])
    }

    @Test func `Filter leading-space commands, secrets, and configured patterns`() throws {
        var config = SmcliConfig()
        let defaults = try CaptureFilter(config: config)
        #expect(defaults.allows(command: "git status", directory: "/"))
        #expect(!defaults.allows(command: " git status", directory: "/"))
        #expect(!defaults.allows(command: "export GITHUB_TOKEN=ghp_" + String(repeating: "a", count: 36), directory: "/"))
        #expect(!defaults.allows(command: "curl -H 'x-api-key: sk-ant-api03-abcdefghijklmnopqrstuvwxyz'", directory: "/"))
        config.secretsFilter = false
        config.historyFilter = ["^vault "]
        config.cwdFilter = ["^/secret(/|$)"]
        let custom = try CaptureFilter(config: config)
        #expect(custom.allows(command: "echo AKIAABCDEFGHIJKLMNOP", directory: "/"))
        #expect(!custom.allows(command: "vault login", directory: "/"))
        #expect(!custom.allows(command: "ls", directory: "/secret/keys"))
        #expect(custom.allows(command: "ls", directory: "/secrets"))
        config.historyFilter = ["("]
        #expect(throws: (any Error).self) { try CaptureFilter(config: config) }
    }

    @Test func `Prune removes stored commands the filter now rejects`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        let log = CapturedHistory(url: home.appendingPathComponent("captured.db"))
        for command in ["ls", "echo AKIAABCDEFGHIJKLMNOP", "vault login", "pwd"] {
            try log.append(command: command, shell: "zsh", directory: "/", exitCode: 0, nativeFile: "")
        }
        var config = SmcliConfig()
        config.historyFilter = ["^vault "]
        let filter = try CaptureFilter(config: config)
        #expect(try log.prune(filter: filter, dryRun: true) == ["echo AKIAABCDEFGHIJKLMNOP", "vault login"])
        #expect(try log.records().count == 4)
        #expect(try log.prune(filter: filter, dryRun: false).count == 2)
        #expect(try log.records().map(\.command) == ["ls", "pwd"])
        let bytes = try Data(contentsOf: log.url)
        #expect(bytes.range(of: Data("AKIAABCDEFGHIJKLMNOP".utf8)) == nil)
    }

    @Test func `Picks pass the capture filter, and prune removes picks on their own`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        let log = CapturedHistory(url: home.appendingPathComponent("captured.db"))
        try log.append(command: "ls", shell: "zsh", directory: "/", exitCode: 0, nativeFile: "")
        let db = try log.openConnection()
        let filter = try CaptureFilter(config: SmcliConfig(historyFilter: ["^vault "]))
        // A native-history command the filter rejects, and a secret typed as the query, are never stored.
        #expect(try !log.recordPick(query: "", command: "vault login", kind: "suggest", directory: "/", filter: filter, on: db))
        #expect(try !log.recordPick(query: "AKIAABCDEFGHIJKLMNOP", command: "ls", kind: "search", directory: "/", filter: filter, on: db))
        #expect(try log.recordPick(query: "l", command: "ls", kind: "suggest", directory: "/", filter: filter, on: db))
        // Picks stored before a filter existed: prune finds them though no command row matches.
        #expect(try log.recordPick(query: "", command: "aws configure", kind: "suggest", directory: "/", filter: .standard, on: db))
        #expect(try log.recordPick(query: "ghp_", command: "ls", kind: "search", directory: "/", filter: .standard, on: db))
        let stricter = try CaptureFilter(config: SmcliConfig(historyFilter: ["^aws ", "^ghp_"]))
        #expect(try log.prune(filter: stricter, dryRun: true) == ["aws configure", "ls"])
        _ = try log.prune(filter: stricter, dryRun: false)
        var left: [String] = []
        try log.openConnection().run("SELECT query FROM command_selections") { left.append(String(cString: sqlite3_column_text($0, 0))) }
        #expect(left == ["l"])
        #expect(try log.records().map(\.command) == ["ls"])
    }

    @Test func `Reject timestamps that can't be stored instead of trapping`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        let log = CapturedHistory(url: home.appendingPathComponent("captured.db"))
        for (start, end) in [(1e300, 1.0), (1.0, 1e300), (-1e300, 1.0), (Double.nan, 1.0), (1.0, Double.infinity), (-5, 1.0)] {
            #expect(throws: HistoryError.self) {
                try log.append(command: "ls", shell: "zsh", directory: "/", exitCode: 0, nativeFile: "", startedAt: start, endedAt: end)
            }
        }
        try log.append(command: "ls", shell: "zsh", directory: "/", exitCode: 0, nativeFile: "", startedAt: 1e9, endedAt: 1e9 + 2)
        #expect(try log.records().first?.durationMs == 2000)
    }

    @Test func `Never open a database that is the native history file`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: home) }
        let native = home.appendingPathComponent("zsh_history")
        let log = CapturedHistory(url: native)
        #expect(throws: HistoryError.self) { try log.checkSeparate(from: native.path) }
        #expect(throws: HistoryError.self) {
            try log.append(command: "ls", shell: "zsh", directory: "/", exitCode: 0, nativeFile: native.path)
        }
        #expect(!FileManager.default.fileExists(atPath: native.path))
    }

    @Test func `Upgrade version 1 databases in place`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        let url = home.appendingPathComponent("captured.db")
        let old = try fixtureDatabase(url)
        try old.execute("""
            CREATE TABLE sessions (id TEXT PRIMARY KEY, user TEXT NOT NULL, host TEXT NOT NULL, shell TEXT NOT NULL) WITHOUT ROWID;
            CREATE TABLE commands (id INTEGER PRIMARY KEY, session TEXT NOT NULL REFERENCES sessions(id),
                started_at REAL NOT NULL, duration_ms INTEGER, directory TEXT NOT NULL, command TEXT NOT NULL, exit_code INTEGER NOT NULL);
            INSERT INTO sessions VALUES ('s', 'bob', 'h', 'zsh');
            INSERT INTO commands (session, started_at, directory, command, exit_code) VALUES ('s', 1, '/', 'old', 0);
            PRAGMA user_version = 1;
            """)
        chmod(url.path, 0o600)
        let log = CapturedHistory(url: url)
        #expect(try log.records().map(\.command) == ["old"])
        try log.append(command: "new", shell: "zsh", directory: "/", exitCode: 0, nativeFile: "")
        let records = try log.records()
        #expect(records.map(\.command) == ["old", "new"])
        #expect(records[0].id == nil && records[1].id != nil)
    }

    @Test func `Store flags on capture and backfill them when upgrading`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        let url = home.appendingPathComponent("captured.db")
        let old = try fixtureDatabase(url)
        try old.execute(CapturedHistory.schema.replacingOccurrences(of: CapturedHistory.migrationToV3, with: "")
            .replacingOccurrences(of: CapturedHistory.migrationToV5, with: "")
            .replacingOccurrences(of: CapturedHistory.migrationToV6, with: "")
            .replacingOccurrences(of: CapturedHistory.migrationToV7, with: "") + """
            INSERT INTO sessions VALUES ('s', 'bob', 'h', 'zsh');
            INSERT INTO commands (session, started_at, directory, command, exit_code) VALUES ('s', 1, '/', 'git commit --amend -m x', 0);
            """)
        chmod(url.path, 0o600)
        let log = CapturedHistory(url: url)
        try log.append(command: "ls -la", shell: "zsh", directory: "/", exitCode: 0, nativeFile: "")
        let stats = try log.commandStats()
        #expect(stats["git commit --amend -m x"]?.flags == ["--amend", "-m"])
        #expect(stats["ls -la"]?.flags == ["-la"])
    }

    @Test func `Add pipestatus to version 3 databases without touching their rows`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        let url = home.appendingPathComponent("captured.db")
        let old = try fixtureDatabase(url)
        try old.execute(CapturedHistory.schema.replacingOccurrences(of: CapturedHistory.migrationToV5, with: "")
            .replacingOccurrences(of: CapturedHistory.migrationToV6, with: "")
            .replacingOccurrences(of: CapturedHistory.migrationToV7, with: "") + """
            PRAGMA user_version = 3;
            INSERT INTO sessions VALUES ('s', 'bob', 'h', 'zsh');
            INSERT INTO commands (session, started_at, directory, command, exit_code, flags) VALUES ('s', 1, '/', 'old', 2, '');
            """)
        chmod(url.path, 0o600)
        let log = CapturedHistory(url: url)
        #expect(try log.records().map(\.command) == ["old"])
        var version = 0
        try old.run("PRAGMA user_version") { version = Int(sqlite3_column_int64($0, 0)) }
        #expect(version == CapturedHistory.version)
        try log.append(command: "a | b", shell: "zsh", directory: "/", exitCode: 1, nativeFile: "", pipestatus: [0, 1])
        try log.append(command: "plain", shell: "fish", directory: "/", exitCode: 0, nativeFile: "")
        let records = try log.records()
        #expect(records.map(\.command) == ["old", "a | b", "plain"])
        #expect(records.map(\.pipestatus) == [nil, [0, 1], nil])
        #expect(records.map(\.exitCode) == [2, 1, 0])
        // Reopening an up-to-date database changes nothing.
        #expect(try CapturedHistory(url: url).records().count == 3)
    }

    @Test func `Add file selections to version 5 databases and keep what they record`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        let url = home.appendingPathComponent("captured.db")
        let old = try fixtureDatabase(url)
        try old.execute(CapturedHistory.schema.replacingOccurrences(of: CapturedHistory.migrationToV6, with: "")
            .replacingOccurrences(of: CapturedHistory.migrationToV7, with: "") + """
            INSERT INTO sessions VALUES ('s', 'bob', 'h', 'zsh');
            INSERT INTO commands (session, started_at, directory, command, exit_code, flags) VALUES ('s', 1, '/', 'old', 2, '');
            """)
        chmod(url.path, 0o600)
        let log = CapturedHistory(url: url)
        #expect(try log.records().map(\.command) == ["old"])
        let db = try log.openConnection()
        var version = 0
        try db.run("PRAGMA user_version") { version = Int(sqlite3_column_int64($0, 0)) }
        #expect(version == CapturedHistory.version)
        let now = 1_000_000.0
        try log.recordSelection(root: "/r", path: "a b.txt", mode: "files", query: "a", at: now, on: db)
        try log.recordSelection(root: "/r", path: "a b.txt", mode: "grep", query: "b", at: now - CapturedHistory.usageHalfLife, on: db)
        try log.recordSelection(root: "/other", path: "x", mode: "files", query: "x", at: now, on: db)
        let scores = try log.selectionScores(root: "/r", now: now, on: db)
        #expect(scores.keys.sorted() == ["a b.txt"])
        #expect(abs(scores["a b.txt"]! - 1.5) < 1e-9)
        // Reopening changes nothing.
        #expect(try CapturedHistory(url: url).records().count == 1)
        #expect(try CapturedHistory(url: url).selectionScores(root: "/r", now: now, on: db)["a b.txt"] != nil)
    }

    @Test func `Add command picks to version 6 databases`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        let url = home.appendingPathComponent("captured.db")
        let old = try fixtureDatabase(url)
        try old.execute(CapturedHistory.schema.replacingOccurrences(of: CapturedHistory.migrationToV7, with: "") + """
            INSERT INTO sessions VALUES ('s', 'bob', 'h', 'zsh');
            INSERT INTO commands (session, started_at, directory, command, exit_code, flags) VALUES ('s', 1, '/', 'old', 0, '');
            """)
        chmod(url.path, 0o600)
        let log = CapturedHistory(url: url)
        let db = try log.openConnection()
        var version = 0
        try db.run("PRAGMA user_version") { version = Int(sqlite3_column_int64($0, 0)) }
        #expect(version == 7)
        try log.recordPick(query: "ol", command: "old", kind: "suggest", directory: "/", filter: .standard, at: 5, on: db)
        #expect(try log.picks(on: db).picks == [.init(query: "ol", command: "old", time: 5)])
        #expect(try log.records().map(\.command) == ["old"])
    }

    @Test func `Parse and validate pipestatus words`() throws {
        #expect(try CapturedHistory.parsePipestatus("") == nil)
        #expect(try CapturedHistory.parsePipestatus(" 0  1\t141 ") == [0, 1, 141])
        for bad in ["x", "0 1 -1", "256", "1.5"] {
            #expect(throws: (any Error).self) { try CapturedHistory.parsePipestatus(bad) }
        }
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        let log = CapturedHistory(url: home.appendingPathComponent("captured.db"))
        #expect(throws: (any Error).self) {
            try log.append(command: "x", shell: "zsh", directory: "/", exitCode: 0, nativeFile: "", pipestatus: [300])
        }
    }

    @Test func `Take version 4 databases back to version 3 and on to the current version`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        let url = home.appendingPathComponent("captured.db")
        let old = try fixtureDatabase(url)
        try old.execute(CapturedHistory.schema.replacingOccurrences(of: CapturedHistory.migrationToV5, with: "")
            .replacingOccurrences(of: CapturedHistory.migrationToV6, with: "")
            .replacingOccurrences(of: CapturedHistory.migrationToV7, with: "") + """
            CREATE TABLE usage (command TEXT PRIMARY KEY, score REAL NOT NULL, at REAL NOT NULL) WITHOUT ROWID;
            PRAGMA user_version = 4;
            INSERT INTO sessions VALUES ('s', 'bob', 'h', 'zsh');
            INSERT INTO commands (session, started_at, directory, command, exit_code, flags) VALUES ('s', 1, '/', 'old', 0, '');
            """)
        chmod(url.path, 0o600)
        let log = CapturedHistory(url: url)
        #expect(try log.records().map(\.command) == ["old"])
        var version = 0, tables = 0
        try old.run("PRAGMA user_version") { version = Int(sqlite3_column_int64($0, 0)) }
        try old.run("SELECT count(*) FROM sqlite_master WHERE name = 'usage'") { tables = Int(sqlite3_column_int64($0, 0)) }
        #expect(version == CapturedHistory.version && tables == 0)
    }

    @Test func `Do not count interrupts or answering exit codes as failures`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        let log = CapturedHistory(url: home.appendingPathComponent("captured.db"))
        for (command, code) in [("npm run dev", 130), ("grep -r todo .", 1), ("diff a b", 1), ("make", 1), ("make", 2)] {
            try log.append(command: command, shell: "zsh", directory: "/", exitCode: code, nativeFile: "")
        }
        let stats = try log.commandStats()
        #expect(stats["npm run dev"]?.successes == 1)
        #expect(stats["grep -r todo ."]?.successes == 1 && stats["diff a b"]?.successes == 1)
        #expect(stats["make"]?.successes == 0)
    }

    @Test func `Aggregate usage per command for ranking`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        let log = CapturedHistory(url: home.appendingPathComponent("captured.db"))
        let now = 1_700_000_000.0
        try log.append(command: "make", shell: "zsh", directory: "/repo", exitCode: 0, nativeFile: "", startedAt: now, endedAt: now)
        try log.append(command: "make", shell: "zsh", directory: "/repo/sub", exitCode: 2, nativeFile: "",
                       startedAt: now - 14 * 86_400, endedAt: now - 14 * 86_400)
        try log.append(command: "make", shell: "zsh", directory: "/repository", exitCode: 0, nativeFile: "",
                       startedAt: now - 1, endedAt: now - 1)
        let stats = try #require(try log.commandStats(directory: "/repo/", now: now)["make"])
        #expect(stats.runs == 3 && stats.successes == 2 && stats.runsHere == 2)
        #expect(abs(stats.frecency - (1 + 0.5 + pow(0.5, 1 / 1_209_600.0))) < 1e-9)
        #expect(try log.commandStats(now: now)["make"]?.runsHere == 0)
        // Old habits fade: 500 runs two years ago count for almost nothing.
        for day in 0..<500 {
            let time = now - 730 * 86_400 - Double(day)
            try log.append(command: "old", shell: "zsh", directory: "/", exitCode: 0, nativeFile: "", startedAt: time, endedAt: time)
        }
        #expect(try log.commandStats(now: now)["old"]!.frecency < 1e-9)
        // Pruned runs no longer count.
        _ = try log.prune(filter: CaptureFilter(config: SmcliConfig(historyFilter: ["^make$"])), dryRun: false)
        #expect(try log.commandStats(now: now)["make"] == nil)
    }

    @Test func `Reject corrupt public and symlinked databases without rewriting them`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        let log = CapturedHistory(url: home.appendingPathComponent("captured.db"))
        try log.append(command: "echo café\n", shell: "zsh", directory: "/", exitCode: 0, nativeFile: "")
        #expect(try log.records().first?.command == "echo café\n")
        let link = home.appendingPathComponent("link")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: log.url)
        #expect(throws: (any Error).self) {
            try CapturedHistory(url: link).append(command: "echo no", shell: "zsh", directory: "/", exitCode: 0, nativeFile: "")
        }
        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: log.url.path)
        #expect(throws: (any Error).self) { try log.records() }
        let corrupt = CapturedHistory(url: home.appendingPathComponent("corrupt.db"))
        for data in [Data("not a database, just some text that is long enough\n".utf8), Data("{\"incomplete\"".utf8)] {
            try data.write(to: corrupt.url)
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: corrupt.url.path)
            #expect(throws: (any Error).self) {
                try corrupt.append(command: "echo no", shell: "zsh", directory: "/", exitCode: 0, nativeFile: "")
            }
            #expect(throws: (any Error).self) { try corrupt.records() }
            #expect(try Data(contentsOf: corrupt.url) == data)
        }
    }

    @Test func `Validate recording arguments before touching disk`() throws {
        let log = CapturedHistory(url: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
        #expect(throws: (any Error).self) {
            try log.append(command: "echo hi", shell: "tcsh", directory: "/", exitCode: 0, nativeFile: "")
        }
        #expect(throws: (any Error).self) {
            try log.append(command: "echo hi", shell: "bash", directory: "relative", exitCode: 300, nativeFile: "")
        }
        #expect(throws: (any Error).self) {
            try log.append(command: "echo hi", shell: "bash", directory: "/", exitCode: 0, nativeFile: "", session: "")
        }
        #expect(!FileManager.default.fileExists(atPath: log.url.path))
        #expect(try epoch("1700000000,5") == 1_700_000_000.5)
        #expect(try epoch("") == nil)
        #expect(throws: (any Error).self) { try epoch("soon") }
    }

    @Test func `Suggest commands ranked by directory, recency, and success`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        let log = CapturedHistory(url: home.appendingPathComponent("captured.db"))
        let now = 1_700_000_000.0
        for (command, directory, exitCode, age) in [
            ("swift build", "/repo", 0, 0.0), ("swift test", "/repo/Tests", 0, 0), ("ls", "/other", 0, 0),
            ("ls", "/other", 0, 0), ("ls", "/other", 0, 0), ("make", "/repo", 2, 0), ("git pull", "/repo", 0, 90 * 86_400), ("smcli suggest", "/repo", 0, 0), ("smcli suggest", "/repo", 0, 0),
        ] {
            try log.append(command: command, shell: "zsh", directory: directory, exitCode: exitCode, nativeFile: "",
                           session: "one", startedAt: now - age)
        }
        let suggestions = try log.suggestions(user: NSUserName(), directory: "/repo/", now: now)
        #expect(suggestions.map(\.command) == ["swift build", "ls", "swift test", "make", "git pull"])
        #expect(suggestions[0].runsHere == 1 && suggestions[3].lastExitCode == 2)
        #expect(try log.suggestions(user: NSUserName(), directory: "/repo", prefix: "swift ", limit: 1).map(\.command) == ["swift build"])
        #expect(try log.suggestions(user: "someone-else", directory: "/repo").isEmpty)
    }
}

/// Creates an empty database file and opens it by its real path (SQLiteDatabase neither creates nor follows symlinks).
private func fixtureDatabase(_ url: URL) throws -> SQLiteDatabase {
    FileManager.default.createFile(atPath: url.path, contents: nil)
    let resolved = realpath(url.path, nil)!
    defer { free(resolved) }
    return try SQLiteDatabase(path: String(cString: resolved), readOnly: false)
}

extension CaptureFilter {
    /// The default filter: leading space and likely secrets.
    static let standard = try! CaptureFilter(config: SmcliConfig())
}
