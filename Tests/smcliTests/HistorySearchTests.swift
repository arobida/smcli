import Foundation
import Testing
@testable import smcli

struct HistorySearchTests {
    @Test func `Merge captured stats and native lines into distinct commands`() {
        let captured = ["git status": CommandStats(runs: 3, lastRun: 1_700_000_000, lastDirectory: "/repo", lastExitCode: 1)]
        let corpus = searchCorpus(captured: captured, native: ["git status", "  ls -la ", "ls -la", "", "   "])
        #expect(corpus.count == 2)
        #expect(corpus["git status"] == SearchEntry(command: "git status", runs: 3, lastRun: "2023-11-14T22:13:20Z",
                                                    lastDirectory: "/repo", lastExitCode: 1, source: "captured"))
        #expect(corpus["ls -la"]?.runs == 2)
        #expect(corpus["ls -la"]?.source == "native")
    }

    @Test func `Leave smcli's own commands out of search and suggestions`() {
        for command in ["smcli search git", "/usr/local/bin/smcli suggest", "SMCLI_DB=x .build/release/smcli init",
                        "swift run -c release smcli history", "'smcli' history | less"] {
            #expect(isSmcliCommand(command), "\(command)")
        }
        for command in ["git log --grep smcli", "cd smcli", "vim Sources/smcli/smcli.swift", "swift build", "smcli-other"] {
            #expect(!isSmcliCommand(command), "\(command)")
        }
        let corpus = searchCorpus(captured: ["smcli suggest": CommandStats(runs: 1, lastRun: 0, lastDirectory: "/", lastExitCode: 0)],
                                  native: ["smcli search x", "git status"])
        #expect(Array(corpus.keys) == ["git status"])
    }

    #if canImport(ProximaKit)
    @Test func `Tokenize whole shell words and their pieces`() {
        #expect(ShellTokenizer().tokenize("git reset --soft HEAD~1") ==
                ["git", "reset", "--soft", "soft", "head~1", "head", "1"])
    }

    @Test func `Search finds exact flags and keeps the index in sync with history`() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let engine = HistorySearch(directory: directory)
        var corpus = searchCorpus(captured: [:], native: [
            "git commit --amend --no-edit", "git reset --soft HEAD~1", "tar -xzf archive.tar.gz", "docker ps -a",
        ])
        #expect(try await engine.search("--soft", corpus: corpus, limit: 1).map(\.command) == ["git reset --soft HEAD~1"])
        #expect(try await engine.search("amend", corpus: corpus, limit: 1).map(\.command) == ["git commit --amend --no-edit"])
        // Commands that leave the history leave the index; new ones are added on the next search.
        corpus.removeValue(forKey: "git reset --soft HEAD~1")
        corpus["kubectl get pods"] = SearchEntry(command: "kubectl get pods", runs: 1, lastRun: nil, lastDirectory: nil,
                                                 lastExitCode: nil, source: "native")
        let hits = try await engine.search("pods", corpus: corpus, limit: 10).map(\.command)
        #expect(hits.first == "kubectl get pods")
        #expect(!hits.contains("git reset --soft HEAD~1"))
        await #expect(throws: (any Error).self) { try await engine.search("  ", corpus: corpus, limit: 1) }
        let attributes = try FileManager.default.attributesOfItem(atPath: directory.path)
        #expect((attributes[.posixPermissions] as? NSNumber)?.intValue == 0o700)
    }
    #endif
}
