import Foundation
import Testing
@testable import smcli

struct OllamaTests {
    @Test(arguments: [
        ("", "http://127.0.0.1:11434"), ("0.0.0.0", "http://127.0.0.1:11434"), ("example:8080", "http://example:8080"),
        ("https://ollama.example.com", "https://ollama.example.com"), ("http://box", "http://box:11434"),
    ])
    func `Resolve OLLAMA_HOST like the ollama CLI`(host: String, expected: String) {
        #expect(Ollama(environment: ["OLLAMA_HOST": host]).base.absoluteString == expected)
    }

    @Test func `Split embedding requests by size and count`() {
        #expect(embeddingBatches(["aa", "bb", "cc", "dd", "ee"], maxCharacters: 100, maxCount: 2) == [0..<2, 2..<4, 4..<5])
        #expect(embeddingBatches(["aaaa", "b", "cccccccc", "d"], maxCharacters: 5, maxCount: 10) == [0..<2, 2..<3, 3..<4])
        #expect(embeddingBatches([], maxCharacters: 5, maxCount: 10).isEmpty)
    }

    @Test func `Config defaults when missing, round-trips, and rejects malformed files`() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        let url = SmcliConfig.location(home: home, environment: [:])
        #expect(url.path == home.appendingPathComponent(".config/smcli/config.json").path)
        #expect(SmcliConfig.location(home: home, environment: ["XDG_CONFIG_HOME": "/xdg"]).path == "/xdg/smcli/config.json")
        #expect(try SmcliConfig.load(from: url) == SmcliConfig())
        let config = SmcliConfig(ollamaModel: "embeddinggemma", ollamaDimension: 768)
        try config.save(to: url)
        #expect(try SmcliConfig.load(from: url) == config)
        try Data("not json".utf8).write(to: url)
        #expect(throws: (any Error).self) { try SmcliConfig.load(from: url) }
    }
}
