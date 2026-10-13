import Foundation
import Testing
@testable import smcli

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
            try log.append(command: "echo hi", shell: "fish", directory: "/", exitCode: 0, nativeFile: "")
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
