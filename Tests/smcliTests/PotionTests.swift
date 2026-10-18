import Foundation
import Testing
@testable import smcli

struct PotionTests {
    /// A two-dimensional model over a tiny WordPiece vocabulary, written in the Hugging Face layout.
    private func tinyModel() throws -> URL {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let vocab = ["[PAD]": 0, "[UNK]": 1, "git": 2, "push": 3, "-": 4, "f": 5, "##s": 6, "cafe": 7]
        let tokenizer: [String: Any] = [
            "normalizer": ["type": "BertNormalizer", "lowercase": true],
            "model": ["type": "WordPiece", "unk_token": "[UNK]", "continuing_subword_prefix": "##", "vocab": vocab],
        ]
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
        let programs: Set<String> = ["git", "docker"]
        for query in ["--soft", "git reset --soft HEAD~1", "HEAD~1", "src/main.swift", "git stash", "FOO=1", "a | b"] {
            #expect(isLiteralQuery(query, programs: programs), "\(query)")
        }
        for query in ["undo the last commit but keep the changes", "list running containers", "git rid of my changes please now"] {
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
    }

    @Test func `Embed only the command and intent for static models`() {
        var entry = entry("git commit --amend")
        entry.intent = "fix the\ntypo"
        #expect(searchDocument(entry, labeled: false) == "git commit --amend\nfix the typo")
    }
}
