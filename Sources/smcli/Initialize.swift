import Foundation
import SmcliCore
import SmcliSearch

/// Asks yes/no, with the current answer as the default. Nil at end of input.
private func ask(_ question: String, default yes: Bool) -> Bool? {
    while true {
        print("\(question) [\(yes ? "Y/n" : "y/N")]: ", terminator: "")
        fflush(stdout)
        guard let line = readLine() else { return nil }
        switch line.trimmingCharacters(in: .whitespaces).lowercased() {
        case "": return yes
        case "y", "yes": return true
        case "n", "no": return false
        default: continue
        }
    }
}

/// Asks which picker keys shell-init binds, and saves them. Up is always bound.
private func chooseKeys(configURL: URL) throws {
    var config = try SmcliConfig.load(from: configURL)
    guard let ctrlR = ask("Open the history picker with Ctrl-R?", default: config.ctrlRKey ?? true),
          let left = ask("Open it with Left arrow when the command line is empty?", default: config.leftKey ?? false) else {
        throw HistoryError(message: "No choice made; key bindings unchanged.")
    }
    (config.ctrlRKey, config.leftKey) = (ctrlR, left)
    try config.save(to: configURL)
    print("Saved key bindings to \(configURL.path). Open a new shell (or re-run shell-init) to use them.")
}

/// Saves keys given as `--keys ctrl-r,left` (either, both, or `none`), for install scripts.
private func setKeys(_ list: String, configURL: URL) throws {
    let names = Set(list.lowercased().split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) })
    guard names.isSubset(of: ["ctrl-r", "left", "none"]), !names.isEmpty, !(names.contains("none") && names.count > 1) else {
        throw HistoryError(message: "--keys takes ctrl-r, left, ctrl-r,left or none.")
    }
    var config = try SmcliConfig.load(from: configURL)
    (config.ctrlRKey, config.leftKey) = (names.contains("ctrl-r"), names.contains("left"))
    try config.save(to: configURL)
    let chosen = ["Up"] + (names.contains("ctrl-r") ? ["Ctrl-R"] : []) + (names.contains("left") ? ["Left on an empty line"] : [])
    print("The picker opens with \(chosen.joined(separator: ", ")). Saved \(configURL.path).")
}

/// `smcli init`: choose the picker keys and the search embedding model. With no arguments both are asked.
/// `--keys LIST` sets the keys without asking (`--keys` alone asks); it combines with the model flags
/// --model/--potion/--builtin/--default, which never ask about keys, so scripts don't block on a question.
func initialize(_ arguments: [String], configURL: URL, environment: [String: String]) async throws {
    var arguments = arguments
    if let flag = arguments.firstIndex(of: "--keys") {
        let value = arguments.index(after: flag) < arguments.endIndex && !arguments[flag + 1].hasPrefix("--")
            ? arguments[flag + 1] : nil
        arguments.removeSubrange(flag...(value == nil ? flag : flag + 1))
        if let value {
            try setKeys(value, configURL: configURL)
        } else {
            guard isatty(STDIN_FILENO) != 0 else {
                throw HistoryError(message: "smcli init --keys needs a terminal to ask; pass e.g. --keys ctrl-r,left.")
            }
            try chooseKeys(configURL: configURL)
        }
        if arguments.isEmpty { return }
    } else if arguments.isEmpty && isatty(STDIN_FILENO) != 0 {
        try chooseKeys(configURL: configURL)
    }
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
    default: throw HistoryError(message: "Usage: smcli init [--model NAME | --potion [DIR] | --builtin | --default] [--keys [ctrl-r,left|none]]")
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
