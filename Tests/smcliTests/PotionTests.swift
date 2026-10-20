import Foundation
import Testing
@testable import smcli
@testable import SmcliSearch
@testable import SmcliCore

struct PotionTests {
    /// A two-dimensional model over a tiny WordPiece vocabulary, written in the Hugging Face layout.
    /// `edit` adjusts the tokenizer JSON before it is written.
    private func tinyModel(at given: URL? = nil, vocab extra: [String: Int] = [:],
                           edit: (inout [String: Any]) -> Void = { _ in }) throws -> URL {
        let directory = given ?? FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let vocab = ["[PAD]": 0, "[UNK]": 1, "git": 2, "push": 3, "-": 4, "f": 5, "##s": 6, "cafe": 7].merging(extra) { $1 }
        var tokenizer: [String: Any] = [
            "normalizer": ["type": "BertNormalizer", "lowercase": true],
            "pre_tokenizer": ["type": "BertPreTokenizer"],
            "model": ["type": "WordPiece", "unk_token": "[UNK]", "continuing_subword_prefix": "##", "vocab": vocab],
        ]
        edit(&tokenizer)
        try JSONSerialization.data(withJSONObject: tokenizer).write(to: directory.appendingPathComponent("tokenizer.json"))
        // float16 rows: 1.0 = 0x3C00, 2.0 = 0x4000, -1.0 = 0xBC00, 0.5 = 0x3800.
        let rows: [[UInt16]] = [[0, 0], [0x3C00, 0x3C00], [0x3C00, 0], [0, 0x3C00], [0x4000, 0x4000],
                                [0xBC00, 0], [0x3800, 0x3800], [0, 0x4000]]
        let header = Data(#"{"embeddings":{"dtype":"F16","shape":[8,2],"data_offsets":[0,32]}}"#.utf8)
        var file = Data((0..<8).map { UInt8(truncatingIfNeeded: header.count >> (8 * $0)) })
        file += header
        for row in rows { for value in row { file += [UInt8(value & 0xFF), UInt8(value >> 8)] } }
        try file.write(to: directory.appendingPathComponent("model.safetensors"))
        return directory
    }

    @Test func `Tokenize like BERT WordPiece and drop unknown words`() throws {
        let directory = try tinyModel()
        defer { try? FileManager.default.removeItem(at: directory) }
        let model = try PotionModel(directory: directory)
        #expect(model.dimension == 2)
        #expect(model.tokenize("GIT push -f") == [2, 3, 4, 5])
        #expect(model.tokenize("gits") == [2, 6])
        // Accents are stripped with lowercasing; unknown words vanish rather than becoming [UNK].
        #expect(model.tokenize("Café zzz") == [7])
        #expect(model.embed("zzz") == nil)
    }

    @Test func `Honor the tokenizer's normalizer and WordPiece options`() throws {
        // Accents kept: "café" no longer reduces to "cafe".
        let accents = try tinyModel(vocab: ["café": 7, "cafe": 0]) { $0["normalizer"] = ["type": "BertNormalizer", "lowercase": true, "strip_accents": false] }
        defer { try? FileManager.default.removeItem(at: accents) }
        #expect(try PotionModel(directory: accents).tokenize("Café") == [7])
        // No normalizer: case is kept.
        let cased = try tinyModel { $0["normalizer"] = NSNull() }
        defer { try? FileManager.default.removeItem(at: cased) }
        #expect(try PotionModel(directory: cased).tokenize("GIT git") == [2])
        // Another subword prefix.
        let prefix = try tinyModel(vocab: ["##s": 0, "@@s": 6]) {
            $0["model"] = ["type": "WordPiece", "unk_token": "[UNK]", "continuing_subword_prefix": "@@",
                           "vocab": ["[PAD]": 0, "[UNK]": 1, "git": 2, "@@s": 6]]
        }
        defer { try? FileManager.default.removeItem(at: prefix) }
        #expect(try PotionModel(directory: prefix).tokenize("gits") == [2, 6])
        // Pieces this port does not implement are refused rather than approximated.
        let other = try tinyModel { $0["pre_tokenizer"] = ["type": "Metaspace"] }
        defer { try? FileManager.default.removeItem(at: other) }
        #expect(throws: HistoryError.self) { try PotionModel(directory: other) }
    }

    @Test func `Find the snapshot refs/main names under XDG_CACHE_HOME`() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = root.appendingPathComponent("xdg/huggingface/hub/models--minishlab--\(PotionModel.name)")
        let main = try tinyModel(at: repository.appendingPathComponent("snapshots/aaa"))
        _ = try tinyModel(at: repository.appendingPathComponent("snapshots/bbb"))
        try FileManager.default.createDirectory(at: repository.appendingPathComponent("refs"), withIntermediateDirectories: true)
        try "aaa".write(to: repository.appendingPathComponent("refs/main"), atomically: true, encoding: .utf8)
        let environment = ["XDG_CACHE_HOME": root.appendingPathComponent("xdg").path]
        #expect(PotionModel.cachedSnapshot(home: root, environment: environment)?.lastPathComponent == main.lastPathComponent)
        #expect(PotionModel.cachedSnapshot(home: root, environment: [:]) == nil)
    }

    #if canImport(ProximaKit)
    @Test func `Default to the cached Potion model unless Apple's is chosen`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        let captured = CapturedHistory(url: home.appendingPathComponent("history.db"))
        let environment = ["SMCLI_CONFIG": home.appendingPathComponent("config.json").path]
        #expect(try searchEngine(captured: captured, home: home, environment: environment).potion == nil)
        _ = try tinyModel(at: home.appendingPathComponent(".cache/huggingface/hub/models--minishlab--\(PotionModel.name)/snapshots/aaa"))
        #expect(try searchEngine(captured: captured, home: home, environment: environment).potion != nil)
        try SmcliConfig(builtin: true).save(to: home.appendingPathComponent("config.json"))
        #expect(try searchEngine(captured: captured, home: home, environment: environment).potion == nil)
    }
    #endif

    @Test func `Ship a loadable model in Models`() throws {
        let directory = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .appendingPathComponent("../../Models/\(PotionModel.name)").standardizedFileURL
        let model = try PotionModel(directory: directory)
        #expect(model.dimension == 256)
        #expect(model.embed("git reset --soft HEAD~1") != nil)
        // The test runner is not the smcli binary, so the linked copy is checked only when present.
        if let bundled = try PotionModel.bundled() {
            #expect(bundled.embed("git status") == model.embed("git status"))
        }
    }

    @Test func `Embed as the normalized mean of token rows`() throws {
        let directory = try tinyModel()
        defer { try? FileManager.default.removeItem(at: directory) }
        let vector = try #require(try PotionModel(directory: directory).embed("git push"))
        #expect(abs(vector[0] - Float(0.5).squareRoot()) < 1e-6 && abs(vector[1] - Float(0.5).squareRoot()) < 1e-6)
        let flag = try #require(try PotionModel(directory: directory).embed("-f"))
        // (2 - 1, 2 + 0) / 2 → (0.5, 1) normalized.
        #expect(abs(flag[0] - 1 / Float(5).squareRoot()) < 1e-6 && abs(flag[1] - 2 / Float(5).squareRoot()) < 1e-6)
    }

    /// Matches the exporter's fp32 reference when SMCLI_POTION_DIR (model) and SMCLI_POTION_REFERENCE
    /// (reference.json from export_potion.py) are set; skipped otherwise.
    @Test func `Match model2vec reference embeddings`() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let path = environment["SMCLI_POTION_DIR"], let reference = environment["SMCLI_POTION_REFERENCE"] else { return }
        let model = try PotionModel(directory: URL(fileURLWithPath: path))
        let json = try #require(try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: reference))) as? [String: Any])
        let texts = try #require(json["texts"] as? [String: [String: String]])
        let embeddings = try #require(json["embeddings"] as? [String: [Double]])
        for (key, entry) in texts {
            let got = try #require(model.embed(entry["text"]!))
            let want = embeddings[key]!.map(Float.init)
            let cosine = zip(got, want).map(*).reduce(0, +) / (sqrt(want.map { $0 * $0 }.reduce(0, +)))
            #expect(cosine > 0.9999, "\(key): \(cosine)")
        }
    }
}

struct RankingTests {
    private func entry(_ command: String, runs: Int = 1, frecency: Double = 0, successes: Int? = nil,
                       runsHere: Int = 0) -> SearchEntry {
        SearchEntry(command: command, runs: runs, lastRun: "2023-11-14T22:13:20Z", lastDirectory: "/repo",
                    lastExitCode: 0, source: "captured", frecency: frecency, successes: successes ?? runs, runsHere: runsHere)
    }

    @Test func `Classify literal and intent queries`() {
        let programs = programArguments(Dictionary(uniqueKeysWithValues: [
            "git stash", "git reset --hard", "docker ps", "find . -name x", "make test", "/usr/bin/head -n 5 log",
        ].map { ($0, SearchEntry(command: $0, runs: 1, lastRun: nil, lastDirectory: nil, lastExitCode: nil, source: "native")) }))
        #expect(programs["git"] == ["stash", "reset"] && programs["head"] == ["-n"])
        for query in ["--soft", "git reset --soft HEAD~1", "HEAD~1", "src/main.swift", "git stash", "FOO=1", "a | b",
                      "make test", "git", "echo $HOME", "scp host:file"] {
            #expect(isLiteralQuery(query, programs: programs), "\(query)")
        }
        for query in ["undo the last commit but keep the changes", "list running containers", "git rid of my changes please now",
                      "find large files", "show head of log", "make it faster", "error: not found", "cost in $"] {
            #expect(!isLiteralQuery(query, programs: programs), "\(query)")
        }
    }

    @Test func `Rerank by usage and context without surfacing weak matches`() {
        let now = 1_700_000_000.0
        let ranked = rerank([
            (entry("git reset --hard"), 1.0),
            (entry("git reset --soft HEAD~1", runs: 20, frecency: 15, runsHere: 10), 0.9),
            (entry("git log", runs: 500, frecency: 400), 0.1),
        ], literal: false, query: "undo commit", now: now)
        #expect(ranked.map(\.command) == ["git reset --soft HEAD~1", "git reset --hard", "git log"])
        #expect(ranked[0].why == ["semantic match", "used here", "recent", "used 20 times"])
        // Failures are demoted, not dropped.
        let failing = rerank([(entry("make deploy", runs: 4, successes: 0), 1.0), (entry("make test", runs: 4), 0.9)],
                             literal: true, query: "make", now: now)
        #expect(failing.map(\.command) == ["make test", "make deploy"])
        #expect(failing[1].why?.contains("always failed") == true)
        // A heavy habit that is only a distant neighbour stays below a real match.
        let habit = rerank([(entry("rm -rf build"), 0.49), (entry("ls -la", runs: 80, frecency: 60, runsHere: 80), 0.34)],
                           literal: false, query: "undo last commit", now: now)
        #expect(habit.map(\.command) == ["rm -rf build", "ls -la"])
    }

    @Test func `Embed only the command and intent for static models`() {
        var entry = entry("git commit --amend")
        entry.intent = "fix the\ntypo"
        #expect(searchDocument(entry, labeled: false) == "git commit --amend\nfix the typo")
    }
}
