import Foundation
import SmcliCore
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// Minimal client for a local Ollama server's HTTP API (the server behind the `ollama` CLI).
package struct Ollama: Sendable {
    package let base: URL

    /// Honors OLLAMA_HOST the way the ollama CLI does: `host`, `host:port`, or a full URL.
    package init(environment: [String: String]) {
        var host = environment["OLLAMA_HOST"].map { $0.trimmingCharacters(in: .whitespaces) } ?? ""
        if host.isEmpty { host = "127.0.0.1:11434" }
        if !host.contains("://") { host = "http://" + host }
        var components = URLComponents(string: host) ?? URLComponents(string: "http://127.0.0.1:11434")!
        if components.host == "0.0.0.0" || components.host?.isEmpty != false { components.host = "127.0.0.1" }
        if components.port == nil && components.scheme == "http" { components.port = 11434 }
        base = components.url!
    }

    /// Installed models that report the "embedding" capability.
    package func embeddingModels() async throws -> [String] {
        struct Tags: Decodable { struct Model: Decodable { let name: String }; let models: [Model] }
        struct Show: Decodable { let capabilities: [String]? }
        let names = try await request("api/tags", body: nil as [String: String]?, as: Tags.self).models.map(\.name)
        var models: [String] = []
        for name in names.sorted() {
            let show = try await request("api/show", body: ["model": name], as: Show.self)
            if show.capabilities?.contains("embedding") == true { models.append(name) }
        }
        return models
    }

    package func embed(_ texts: [String], model: String) async throws -> [[Float]] {
        // truncate: inputs past the model's context are cut rather than rejected.
        struct Request: Encodable { let model: String; let input: [String]; var truncate = true }
        struct Response: Decodable { let embeddings: [[Float]] }
        let response = try await request("api/embed", body: Request(model: model, input: texts), as: Response.self)
        guard response.embeddings.count == texts.count else {
            throw HistoryError(message: "Ollama returned \(response.embeddings.count) embeddings for \(texts.count) texts.")
        }
        return response.embeddings
    }

    private func request<Body: Encodable, Result: Decodable>(_ path: String, body: Body?, as: Result.Type) async throws -> Result {
        var request = URLRequest(url: base.appendingPathComponent(path))
        if let body {
            request.httpMethod = "POST"
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONEncoder().encode(body)
        }
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await URLSession.shared.data(for: request)
        } catch {
            throw HistoryError(message: "Cannot reach Ollama at \(base.absoluteString) (is `ollama serve` running?): \(error.localizedDescription)")
        }
        if let status = (response as? HTTPURLResponse)?.statusCode, !(200..<300).contains(status) {
            let detail = (try? JSONDecoder().decode([String: String].self, from: data))?["error"] ?? "HTTP \(status)"
            throw HistoryError(message: "Ollama \(path) failed: \(detail)")
        }
        return try JSONDecoder().decode(Result.self, from: data)
    }
}

/// Consecutive ranges of `texts` holding at most `maxCount` texts and `maxCharacters` characters each;
/// a single text longer than the budget gets a range of its own.
package func embeddingBatches(_ texts: [String], maxCharacters: Int, maxCount: Int) -> [Range<Int>] {
    var ranges: [Range<Int>] = []
    var start = 0
    var characters = 0
    for (index, text) in texts.enumerated() {
        if index > start && (index - start >= maxCount || characters + text.count > maxCharacters) {
            ranges.append(start..<index)
            start = index
            characters = 0
        }
        characters += text.count
    }
    if start < texts.count { ranges.append(start..<texts.count) }
    return ranges
}

#if canImport(TalaDB)

/// Embeds through Ollama, splitting each index batch into requests small enough for the model's
/// runner: one request carrying hundreds of long commands can exhaust its memory and crash it.
package struct OllamaEmbedder: TextEmbedder {
    package let client: Ollama
    package let model: String
    package let dimension: Int
    package var maxRequestCharacters = 16_000
    package var maxRequestInputs = 32

    package func embed(_ text: String) async throws -> [Float] {
        try await embedBatch([text])[0]
    }

    package func embedBatch(_ texts: [String]) async throws -> [[Float]] {
        var vectors: [[Float]] = []
        vectors.reserveCapacity(texts.count)
        for range in embeddingBatches(texts, maxCharacters: maxRequestCharacters, maxCount: maxRequestInputs) {
            for embedding in try await client.embed(Array(texts[range]), model: model) {
                guard embedding.count == dimension else {
                    throw HistoryError(message: "Ollama model \(model) now returns \(embedding.count) dimensions, not \(dimension); rerun `smcli init`.")
                }
                vectors.append(embedding)
            }
        }
        return vectors
    }
}
#endif
