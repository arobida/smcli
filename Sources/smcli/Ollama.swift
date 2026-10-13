import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// User settings written by `smcli init`.
struct SmcliConfig: Codable, Equatable {
    /// Ollama embedding model for search; nil uses Apple's built-in sentence embedding.
    var ollamaModel: String?
    /// The model's vector size, recorded at init so the index can be opened without a network call.
    var ollamaDimension: Int?

    static func location(home: URL, environment: [String: String]) -> URL {
        if let path = environment["SMCLI_CONFIG"], !path.isEmpty { return URL(fileURLWithPath: path) }
        let base = environment["XDG_CONFIG_HOME"].flatMap { $0.hasPrefix("/") ? URL(fileURLWithPath: $0) : nil }
            ?? home.appendingPathComponent(".config")
        return base.appendingPathComponent("smcli/config.json")
    }

    /// A missing file means defaults; a malformed one is an error rather than silently ignored.
    static func load(from url: URL) throws -> SmcliConfig {
        guard FileManager.default.fileExists(atPath: url.path) else { return SmcliConfig() }
        do {
            return try JSONDecoder().decode(SmcliConfig.self, from: Data(contentsOf: url))
        } catch {
            throw HistoryError(message: "Invalid config \(url.path): \(error.localizedDescription)")
        }
    }

    func save(to url: URL) throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true,
                                                attributes: [.posixPermissions: 0o700])
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try encoder.encode(self).write(to: url, options: .atomic)
    }
}

/// Minimal client for a local Ollama server's HTTP API (the server behind the `ollama` CLI).
struct Ollama: Sendable {
    let base: URL

    /// Honors OLLAMA_HOST the way the ollama CLI does: `host`, `host:port`, or a full URL.
    init(environment: [String: String]) {
        var host = environment["OLLAMA_HOST"].map { $0.trimmingCharacters(in: .whitespaces) } ?? ""
        if host.isEmpty { host = "127.0.0.1:11434" }
        if !host.contains("://") { host = "http://" + host }
        var components = URLComponents(string: host) ?? URLComponents(string: "http://127.0.0.1:11434")!
        if components.host == "0.0.0.0" || components.host?.isEmpty != false { components.host = "127.0.0.1" }
        if components.port == nil && components.scheme == "http" { components.port = 11434 }
        base = components.url!
    }

    /// Installed models that report the "embedding" capability.
    func embeddingModels() async throws -> [String] {
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

    func embed(_ texts: [String], model: String) async throws -> [[Float]] {
        struct Request: Encodable { let model: String; let input: [String] }
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

/// `smcli init`: choose the search embedding model, interactively or with --model/--builtin.
func initialize(_ arguments: [String], configURL: URL, environment: [String: String]) async throws {
    var config = try SmcliConfig.load(from: configURL)
    let ollama = Ollama(environment: environment)
    var choice: String?
    switch arguments {
    case []:
        let models: [String]
        do {
            models = try await ollama.embeddingModels()
        } catch {
            print("Ollama not available (\(error.localizedDescription)); using the built-in Apple embedding model.")
            models = []
        }
        if models.isEmpty {
            print("No Ollama embedding models found. Install one with e.g. `ollama pull embeddinggemma`, then rerun `smcli init`.")
        } else {
            guard isatty(STDIN_FILENO) != 0 else {
                throw HistoryError(message: "smcli init needs a terminal to ask; use --model NAME or --builtin.")
            }
            print("Found Ollama embedding models. Use one for `smcli search`?")
            for (number, model) in models.enumerated() {
                let current = model == config.ollamaModel ? " (current)" : ""
                print("  \(number + 1)) \(model)\(current)")
            }
            print("  0) No, use the built-in Apple model")
            while choice == nil {
                print("Choose [0-\(models.count)]: ", terminator: "")
                fflush(stdout)
                guard let line = readLine() else { throw HistoryError(message: "No choice made; config unchanged.") }
                let text = line.trimmingCharacters(in: .whitespaces)
                if text == "0" {
                    choice = ""
                } else if let number = Int(text), models.indices.contains(number - 1) {
                    choice = models[number - 1]
                }
            }
        }
    case ["--builtin"]: choice = ""
    case let arguments where arguments.count == 2 && arguments[0] == "--model": choice = arguments[1]
    default: throw HistoryError(message: "Usage: smcli init [--model NAME | --builtin]")
    }

    guard let choice else { return }  // Nothing offered: keep any existing choice.
    if !choice.isEmpty {
        let model = choice
        // Embedding once both verifies the model works and records its vector size.
        guard let dimension = try await ollama.embed(["smcli"], model: model).first?.count, dimension > 0 else {
            throw HistoryError(message: "Ollama model \(model) returned an empty embedding.")
        }
        config.ollamaModel = model
        config.ollamaDimension = dimension
    } else {
        config.ollamaModel = nil
        config.ollamaDimension = nil
    }
    try config.save(to: configURL)
    let summary = config.ollamaModel.map { "Ollama model \($0) (\(config.ollamaDimension!) dimensions)" } ?? "built-in Apple model"
    print("Search will use the \(summary). Saved \(configURL.path).")
    if config.ollamaModel != nil { print("The next search re-embeds your history with it.") }
}

#if canImport(ProximaKit)
import ProximaKit

/// Embeds through Ollama; each ProximaKit batch is one HTTP request.
struct OllamaEmbedder: TextEmbedder {
    let client: Ollama
    let model: String
    let dimension: Int

    func embed(_ text: String) async throws -> Vector {
        try await embedBatch([text])[0]
    }

    func embedBatch(_ texts: [String]) async throws -> [Vector] {
        try await client.embed(texts, model: model).map {
            guard $0.count == dimension else {
                throw HistoryError(message: "Ollama model \(model) now returns \($0.count) dimensions, not \(dimension); rerun `smcli init`.")
            }
            return Vector($0)
        }
    }
}
#endif
