import Foundation
import Testing
@testable import smcli
@testable import SmcliSearch
@testable import SmcliCore

struct HistoryTests {
    @Test(arguments: [
        (HistoryFile.Shell.bash, "#1700000000\necho one\necho two\n", "echo one\necho two\n"),
        (.bash, "echo first\necho first\n# comment\n", "echo first\necho first\n# comment\n"),
        (.zsh, ": 1700000000:0;echo one\n: 1700000001:2;echo two\n", "echo one\necho two\n"),
        (.zsh, "echo plain\n", "echo plain\n"),
        (.zsh, ": 1:0;echo 'first\\\n: 2:0;second'\n", "echo 'first\n: 2:0;second'\n"),
        (.zsh, "echo \\\\\nnext\n", "echo \\\nnext\n"),
        (.zsh, "echo \\", "echo \\"),
        (.bash, "", ""),
    ])
    func formats(shell: HistoryFile.Shell, input: String, expected: String) throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: url) }
        try Data(input.utf8).write(to: url)
        #expect(try HistoryFile(url: url, shell: shell).read() == expected)
        #expect(try Data(contentsOf: url) == Data(input.utf8))
    }

    @Test func `Decode Zsh metafied Unicode`() throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: url) }
        let command = "echo café 🐚\n"
        let bytes = command.utf8.flatMap { byte in
            byte >= 0x80 ? [UInt8(0x83), byte ^ 32] : [byte]
        }
        try Data(bytes).write(to: url)
        #expect(try HistoryFile(url: url, shell: .zsh).read() == command)
    }

    @Test func `Discover both shells sessions and custom history without duplicate paths`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        let paths = [".bash_history", ".zsh_history", ".bash_sessions/a.history",
                     ".bash_sessions/a.historynew", ".zsh_sessions/b.history", ".zsh_sessions/b.session", "custom"]
        for path in paths {
            let url = home.appendingPathComponent(path)
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try Data("echo test\n".utf8).write(to: url)
        }
        let files = try historyFiles(home: home, environment: ["HISTFILE": home.appendingPathComponent("custom").path, "SHELL": "/bin/bash"])
        #expect(files.count == 6)
        #expect(files.last?.shell == .bash)
        #expect(!files.contains { $0.url.pathExtension == "session" })
        let duplicate = try historyFiles(home: home, environment: ["HISTFILE": home.appendingPathComponent(".zsh_history").path])
        #expect(duplicate.count == 5)
    }

    @Test func `Read the entire file without truncation`() throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: url) }
        let input = (0..<15_000).map { "echo command \($0)\n" }.joined()
        try Data(input.utf8).write(to: url)
        #expect(try HistoryFile(url: url, shell: .bash).read() == input)
    }

    @Test func `Missing defaults are fine but an explicit missing HISTFILE fails when read`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        #expect(try historyFiles(home: home, environment: [:]).isEmpty)
        let files = try historyFiles(home: home, environment: ["HISTFILE": home.appendingPathComponent("missing").path])
        #expect(files.count == 1)
        #expect(throws: (any Error).self) { try files[0].read() }
    }
}
