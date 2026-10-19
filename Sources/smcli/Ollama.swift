import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// User settings written by `smcli init`.
struct SmcliConfig: Codable, Equatable {
    /// Ollama embedding model for search. With no model chosen, search uses the built-in potion-code-16M-v2.
    var ollamaModel: String?
    /// The model's vector size, recorded at init so the index can be opened without a network call.
    var ollamaDimension: Int?
    /// Directory of a potion-code-16M-v2 model (safetensors + tokenizer) to use instead of the built-in copy.
    var potionPath: String?
    /// Apple's built-in embedding instead of Potion (`smcli init --builtin`).
    var builtin: Bool?
    /// Regexes; commands matching any, or run in a directory matching any of `cwdFilter`, are not recorded.
    var historyFilter: [String]?
    var cwdFilter: [String]?
    /// Skip commands containing likely API keys and tokens; on unless set to false.
    var secretsFilter: Bool?

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

/// `smcli init`: choose the search embedding model, interactively or with --model/--potion/--builtin.
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
            print("Ollama not available (\(error.localizedDescription)); keeping the default model.")
            models = []
        }
        if models.isEmpty {
            print("No Ollama embedding models found. Install one with e.g. `ollama pull embeddinggemma`, then rerun `smcli init`.")
            print("Until then search uses the default: the built-in \(PotionModel.name).")
        } else {
            guard isatty(STDIN_FILENO) != 0 else {
                throw HistoryError(message: "smcli init needs a terminal to ask; use --model NAME or --builtin.")
            }
            print("Found Ollama embedding models. Use one for `smcli search`?")
            for (number, model) in models.enumerated() {
                let current = model == config.ollamaModel ? " (current)" : ""
                print("  \(number + 1)) \(model)\(current)")
            }
            print("  0) No, use the default (the built-in \(PotionModel.name))")
            while choice == nil {
                print("Choose [0-\(models.count)]: ", terminator: "")
                fflush(stdout)
                guard let line = readLine() else { throw HistoryError(message: "No choice made; config unchanged.") }
                let text = line.trimmingCharacters(in: .whitespaces)
                if text == "0" {
                    choice = "default"
                } else if let number = Int(text), models.indices.contains(number - 1) {
                    choice = models[number - 1]
                }
            }
        }
    case ["--builtin"]: choice = ""
    case ["--default"]: choice = "default"
    case let arguments where arguments.first == "--potion" && arguments.count <= 2:
        let home = environment["HOME"].map { URL(fileURLWithPath: $0) } ?? FileManager.default.homeDirectoryForCurrentUser
        // Without a directory: the built-in copy, or in a build without one, the Hugging Face cache.
        let builtIn = arguments.count == 1 ? try PotionModel.bundled() : nil
        let directory = arguments.count == 2 ? URL(fileURLWithPath: arguments[1]).standardizedFileURL
            : builtIn == nil ? PotionModel.cachedSnapshot(home: home, environment: environment) : nil
        guard builtIn != nil || directory != nil else {
            throw HistoryError(message: "No \(PotionModel.name) in the Hugging Face cache; download it (e.g. `hf download minishlab/\(PotionModel.name)`) or pass its directory.")
        }
        // Loading and embedding once verifies the files before they are saved as the choice.
        let model = try builtIn ?? PotionModel(directory: directory!)
        guard model.embed("smcli") != nil else { throw HistoryError(message: "Potion model in \(model.source) cannot embed text.") }
        config.potionPath = directory?.path
        config.ollamaModel = nil
        config.ollamaDimension = nil
        config.builtin = nil
        try config.save(to: configURL)
        print("Search will use \(PotionModel.name) (\(model.dimension) dimensions), \(directory.map { "from " + $0.path } ?? "built in"). Saved \(configURL.path).")
        print("The next search re-embeds your history with it.")
        return
    case let arguments where arguments.count == 2 && arguments[0] == "--model": choice = arguments[1]
    default: throw HistoryError(message: "Usage: smcli init [--model NAME | --potion [DIR] | --builtin | --default]")
    }

    guard let choice else { return }  // Nothing offered: keep any existing choice.
    config.builtin = nil
    if choice == "default" {
        config.ollamaModel = nil
        config.ollamaDimension = nil
        config.potionPath = nil
        try config.save(to: configURL)
        print("Search will use the default: the built-in \(PotionModel.name). Saved \(configURL.path).")
        return
    } else if !choice.isEmpty {
        let model = choice
        // Embedding once both verifies the model works and records its vector size.
        guard let dimension = try await ollama.embed(["smcli"], model: model).first?.count, dimension > 0 else {
            throw HistoryError(message: "Ollama model \(model) returned an empty embedding.")
        }
        config.ollamaModel = model
        config.ollamaDimension = dimension
        config.potionPath = nil
    } else {
        config.ollamaModel = nil
        config.ollamaDimension = nil
        config.potionPath = nil
        config.builtin = true
    }
    try config.save(to: configURL)
    let summary = config.ollamaModel.map { "Ollama model \($0) (\(config.ollamaDimension!) dimensions)" } ?? "built-in Apple model"
    print("Search will use the \(summary). Saved \(configURL.path).")
    if config.ollamaModel != nil { print("The next search re-embeds your history with it.") }
}

/// Consecutive ranges of `texts` holding at most `maxCount` texts and `maxCharacters` characters each;
/// a single text longer than the budget gets a range of its own.
func embeddingBatches(_ texts: [String], maxCharacters: Int, maxCount: Int) -> [Range<Int>] {
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

#if canImport(ProximaKit)
import ProximaKit

/// Embeds through Ollama, splitting each ProximaKit batch into requests small enough for the model's
/// runner: one request carrying hundreds of long commands can exhaust its memory and crash it.
struct OllamaEmbedder: TextEmbedder {
    let client: Ollama
    let model: String
    let dimension: Int
    var maxRequestCharacters = 16_000
    var maxRequestInputs = 32

    func embed(_ text: String) async throws -> Vector {
        try await embedBatch([text])[0]
    }

    func embedBatch(_ texts: [String]) async throws -> [Vector] {
        var vectors: [Vector] = []
        vectors.reserveCapacity(texts.count)
        for range in embeddingBatches(texts, maxCharacters: maxRequestCharacters, maxCount: maxRequestInputs) {
            for embedding in try await client.embed(Array(texts[range]), model: model) {
                guard embedding.count == dimension else {
                    throw HistoryError(message: "Ollama model \(model) now returns \(embedding.count) dimensions, not \(dimension); rerun `smcli init`.")
                }
                vectors.append(Vector(embedding))
            }
        }
        return vectors
    }
}
#endif
