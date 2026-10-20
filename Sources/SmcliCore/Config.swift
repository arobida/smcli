import Foundation

/// User settings written by `smcli init`.
package struct SmcliConfig: Codable, Equatable {
    /// Ollama embedding model for search. With no model chosen, search uses the built-in potion-code-16M-v2.
    package var ollamaModel: String?
    /// The model's vector size, recorded at init so the index can be opened without a network call.
    package var ollamaDimension: Int?
    /// Directory of a potion-code-16M-v2 model (safetensors + tokenizer) to use instead of the built-in copy.
    package var potionPath: String?
    /// Apple's built-in embedding instead of Potion (`smcli init --builtin`).
    package var builtin: Bool?
    /// Regexes; commands matching any, or run in a directory matching any of `cwdFilter`, are not recorded.
    package var historyFilter: [String]?
    package var cwdFilter: [String]?
    /// Skip commands containing likely API keys and tokens; on unless set to false.
    package var secretsFilter: Bool?
    /// Picker keys bound by shell-init, chosen in `smcli init`: Ctrl-R and Ctrl-T file search (on unless false) and Left
    /// on an empty line (off unless true). Up is always bound; SMCLI_NO_KEYS=1 still skips them all.
    package var ctrlRKey: Bool?
    package var ctrlTKey: Bool?
    /// Ctrl-G grep at the prompt; on unless false.
    package var ctrlGKey: Bool?
    package var leftKey: Bool?

    package static func location(home: URL, environment: [String: String]) -> URL {
        if let path = environment["SMCLI_CONFIG"], !path.isEmpty { return URL(fileURLWithPath: path) }
        let base = environment["XDG_CONFIG_HOME"].flatMap { $0.hasPrefix("/") ? URL(fileURLWithPath: $0) : nil }
            ?? home.appendingPathComponent(".config")
        return base.appendingPathComponent("smcli/config.json")
    }

    /// A missing file means defaults; a malformed one is an error rather than silently ignored.
    package static func load(from url: URL) throws -> SmcliConfig {
        guard FileManager.default.fileExists(atPath: url.path) else { return SmcliConfig() }
        do {
            return try JSONDecoder().decode(SmcliConfig.self, from: Data(contentsOf: url))
        } catch {
            throw HistoryError(message: "Invalid config \(url.path): \(error.localizedDescription)")
        }
    }

    package func save(to url: URL) throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true,
                                                attributes: [.posixPermissions: 0o700])
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try encoder.encode(self).write(to: url, options: .atomic)
    }
}
