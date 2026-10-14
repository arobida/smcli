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

    @Test func `Label search documents with command, subcommand, flags, and directory`() {
        let entry = SearchEntry(command: "/usr/bin/git commit --amend", runs: 12, lastRun: "2023-11-12T22:13:20Z",
                                lastDirectory: "/repo", lastExitCode: 0, source: "captured", flags: ["--amend"])
        #expect(searchDocument(entry) ==
                "/usr/bin/git commit --amend\ncommand: git\nsubcommand: commit\nflags: --amend\ndirectory: /repo")
        var authored = entry
        authored.author = "claude"
        authored.intent = "fix the\ntypo"
        #expect(searchDocument(authored).hasSuffix("directory: /repo\nauthor: claude\nintent: fix the typo"))
        let native = SearchEntry(command: "ls -la | less", runs: 1, lastRun: nil, lastDirectory: nil, lastExitCode: nil,
                                 source: "native")
        #expect(searchDocument(native) == "ls -la | less\ncommand: ls")
        let long = SearchEntry(command: "echo " + String(repeating: "x", count: 3_000), runs: 1, lastRun: nil,
                               lastDirectory: "/d", lastExitCode: nil, source: "captured")
        #expect(searchDocument(long).hasSuffix(String(repeating: "x", count: 1_495) + "\ncommand: echo\ndirectory: /d"))
        #expect(searchCorpus(captured: [:], native: ["ls -la | grep -i --color=auto x"])["ls -la | grep -i --color=auto x"]?.flags
                == ["-la", "-i", "--color"])
    }

    @Test func `Filter entries by command, subcommand, runs, and last run`() throws {
        let now = 1_700_000_000.0
        let entry = SearchEntry(command: "git commit -m x", runs: 3, lastRun: "2023-11-12T22:13:20Z",
                                lastDirectory: "/repo", lastExitCode: 0, source: "captured", flags: ["-m"])
        #expect(SearchFilter(program: "git", subcommand: "commit", minRuns: 3).allows(entry))
        #expect(SearchFilter(flag: "-m").allows(entry))
        #expect(!SearchFilter(flag: "--message").allows(entry))
        #expect(!SearchFilter(program: "docker").allows(entry))
        #expect(!SearchFilter(subcommand: "push").allows(entry))
        #expect(!SearchFilter(minRuns: 4).allows(entry))
        #expect(SearchFilter(directory: "/").allows(entry))
        #expect(SearchFilter(directory: "/repo/").allows(entry))
        #expect(!SearchFilter(directory: "/rep").allows(entry))
        #expect(!SearchFilter(author: "claude").allows(entry))
        #expect(SearchFilter(since: try SearchFilter.time("3d", now: now)).allows(entry))
        #expect(!SearchFilter(since: try SearchFilter.time("1d", now: now)).allows(entry))
        #expect(!SearchFilter(before: try SearchFilter.time("2023-11-12T00:00:00Z", now: now)).allows(entry))
        let native = SearchEntry(command: "git status", runs: 9, lastRun: nil, lastDirectory: nil, lastExitCode: nil,
                                 source: "native")
        #expect(!SearchFilter(since: 0).allows(native))
        #expect(throws: (any Error).self) { try SearchFilter.time("yesterday", now: now) }
    }

    #if canImport(ProximaKit)
    @Test func `Filtered search keeps only matches, and lists them without a query`() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let engine = HistorySearch(directory: directory)
        let corpus = searchCorpus(captured: [:], native: ["git commit --amend", "git push", "docker build .", "docker build ."])
        let hits = try await engine.search("build", corpus: corpus, limit: 5, filter: SearchFilter(program: "git"))
        #expect(Set(hits.map(\.command)) == ["git commit --amend", "git push"])
        let listed = try await engine.search("", corpus: corpus, limit: 5, filter: SearchFilter(minRuns: 2))
        #expect(listed.map(\.command) == ["docker build ."])
    }

    @Test func `Report index health and rebuild with reindex`() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let engine = HistorySearch(directory: directory)
        var corpus = searchCorpus(captured: [:], native: ["git push", "docker ps", "ls -la"])
        var health = await engine.health(corpus: corpus)
        #expect(health.healthy && !health.built && health.missing == 3)
        var steps: [Int] = []
        #expect(try await engine.reindex(corpus: corpus) { done, _ in steps.append(done) } == 3)
        #expect(steps == [0, 3])
        corpus.removeValue(forKey: "ls -la")
        corpus["make test"] = SearchEntry(command: "make test", runs: 1, lastRun: nil, lastDirectory: nil,
                                          lastExitCode: nil, source: "native")
        health = await engine.health(corpus: corpus)
        #expect(health.healthy && health.built && health.indexed == 3 && health.missing == 1 && health.stale == 1)
        #expect(health.bytes > 0 && health.lastUpdated != nil)
        #expect(try await engine.reindex(corpus: corpus) == 3)
        health = await engine.health(corpus: corpus)
        #expect(health.missing == 0 && health.stale == 0)
    }

    @Test func `Tokenize whole shell words and their pieces`() {
        #expect(ShellTokenizer().tokenize("git reset --soft HEAD~1") ==
                ["git", "reset", "--soft", "soft", "head~1", "head", "1"])
        #expect(ShellTokenizer().tokenize("git push -f\ncommand: git\nflags: -f") == ["git", "push", "-f", "f"])
        #expect(ShellTokenizer().tokenize("ls\ndirectory: /a\nintent: list files") == ["ls", "intent:", "intent", "list", "files"])
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
