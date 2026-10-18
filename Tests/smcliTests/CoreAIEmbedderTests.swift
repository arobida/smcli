#if canImport(CoreAI) && canImport(ProximaKit)
import Foundation
import Testing
@testable import smcli

struct CoreAIEmbedderTests {
    @Test func `Reproduces the reference embeddings from the export`() async throws {
        guard #available(macOS 27, *) else { return }
        let url = try #require(Bundle.module.url(forResource: "F2LLM-v2-80M", withExtension: nil))
        struct Reference: Decodable {
            struct Text: Decodable { let kind: String; let text: String }
            let prompts: [String: String]
            let texts: [String: Text]
            let embeddings: [String: [Float]]
        }
        let reference = try JSONDecoder().decode(Reference.self, from: Data(contentsOf: url.appendingPathComponent("reference.json")))
        let keys = reference.texts.keys.sorted()
        // embedBatch adds no prompt, so pass the export's own prompts to compare like for like.
        let texts = keys.map { (reference.prompts[reference.texts[$0]!.kind] ?? "") + reference.texts[$0]!.text }
        let vectors = try await CoreAIEmbedder().embedBatch(texts)
        for (key, vector) in zip(keys, vectors) {
            let expected = reference.embeddings[key]!
            let cosine = zip(vector.components, expected).map(*).reduce(0, +)
            #expect(cosine > 0.999, "\(key): cosine \(cosine)")
        }
    }

    @Test func `Infers the query task from shell syntax`() {
        guard #available(macOS 27, *) else { return }
        #expect(CoreAIEmbedder.Task.infer(from: "undo last commit") == .describe)
        #expect(CoreAIEmbedder.Task.infer(from: "git push -f") == .similar)
        #expect(CoreAIEmbedder.Task.infer(from: "cat log | grep error") == .similar)
    }
}
#endif
