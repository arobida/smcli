import Foundation

@main
struct Smcli {
    static let usage = """
    Usage: smcli init [--model NAME | --builtin]
           smcli history [--json]
           smcli shell-init bash|zsh
           smcli search <query...> [--limit N] [--json]
           smcli suggest [prefix...] [--dir PATH] [--limit N] [--json]
           smcli import <legacy.jsonl>
    Read saved shell history, or print opt-in hooks for structured metadata capture.
    init picks an Ollama embedding model for search (saved to ~/.config/smcli/config.json, or SMCLI_CONFIG).
    SMCLI_DB selects the metadata database (default: ~/.local/share/smcli/history.db).
    """

    static func main() async {
        var arguments = Array(CommandLine.arguments.dropFirst())
        if arguments.isEmpty && Menu.isInteractive {
            guard let chosen = Menu.choose(environment: ProcessInfo.processInfo.environment) else { return }
            arguments = chosen
        }
        if arguments.isEmpty || arguments == ["--help"] || arguments == ["-h"] {
            print(usage)
            return
        }
        do {
            let environment = ProcessInfo.processInfo.environment
            let home = environment["HOME"].map { URL(fileURLWithPath: $0) }
                ?? FileManager.default.homeDirectoryForCurrentUser
            let captured = CapturedHistory(url: CapturedHistory.location(home: home, environment: environment))
            if arguments.count == 2 && arguments[0] == "shell-init" {
                let executable = Bundle.main.executableURL?.path ?? URL(fileURLWithPath: CommandLine.arguments[0]).path
                print(try shellIntegration(shell: arguments[1], executable: executable))
            } else if arguments.count == 9 && arguments[0] == "record" {
                // record <shell> <session> <directory> <status> <started> <ended> <native HISTFILE> <command>
                guard let status = Int(arguments[4]) else { throw HistoryError(message: "Invalid exit code.") }
                try captured.append(command: arguments[8], shell: arguments[1], directory: arguments[3], exitCode: status,
                                    nativeFile: arguments[7], session: arguments[2],
                                    startedAt: epoch(arguments[5]), endedAt: epoch(arguments[6]))
            } else if arguments.count == 6 && arguments[0] == "record" {
                // Hooks installed by earlier versions stay working until the shell is restarted.
                guard let status = Int(arguments[3]) else { throw HistoryError(message: "Invalid exit code.") }
                try captured.append(command: arguments[4], shell: arguments[1], directory: arguments[2],
                                    exitCode: status, nativeFile: arguments[5])
            } else if arguments.first == "search" {
                try await search(Array(arguments.dropFirst()), captured: captured, home: home, environment: environment)
            } else if arguments.first == "init" {
                try await initialize(Array(arguments.dropFirst()),
                                     configURL: SmcliConfig.location(home: home, environment: environment),
                                     environment: environment)
            } else if arguments.first == "suggest" {
                try suggest(Array(arguments.dropFirst()), captured: captured, environment: environment)
            } else if arguments.count == 2 && arguments[0] == "import" {
                let count = try captured.importLegacy(from: URL(fileURLWithPath: arguments[1]))
                print("Imported \(count) records into \(captured.url.path).")
            } else if arguments == ["history", "--json"] {
                let records = try captured.records()
                let encoder = JSONEncoder()
                encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
                print(String(decoding: try encoder.encode(records), as: UTF8.self))
            } else if arguments == ["history"] {
                let files = try historyFiles(home: home, environment: environment)
                // Read everything before printing so failures never look like a complete history.
                let histories = try files.map { ($0.url.path, try $0.read()) }
                let records = try captured.records()
                if histories.isEmpty && records.isEmpty { print("No saved Bash or Zsh history found.") }
                for (path, history) in histories {
                    print("==> \(path) <==")
                    print(history, terminator: history.hasSuffix("\n") ? "" : "\n")
                }
                if !records.isEmpty {
                    print("==> \(captured.url.path) <==")
                    for record in records {
                        let duration = record.durationMs.map { " duration=\($0)ms" } ?? ""
                        print("\(record.timestamp) [\(record.user)@\(record.host) \(record.directory)] \(record.shell) exit=\(record.exitCode)\(duration) runCount=\(record.runCount)")
                        print(record.command)
                    }
                }
            } else {
                FileHandle.standardError.write(Data("\(usage)\n".utf8))
                exit(2)
            }
        } catch {
            FileHandle.standardError.write(Data("smcli: \(error.localizedDescription)\n".utf8))
            exit(1)
        }
    }
}

/// Hybrid (keyword + semantic) search over distinct commands from captured and native history.
func search(_ arguments: [String], captured: CapturedHistory, home: URL, environment: [String: String]) async throws {
    var words: [String] = []
    var limit = 10
    var json = false
    var index = 0
    while index < arguments.count {
        switch arguments[index] {
        case "--json": json = true
        case "--limit", "-n":
            index += 1
            guard index < arguments.count, let value = Int(arguments[index]), value > 0 else {
                throw HistoryError(message: "--limit requires a positive number.")
            }
            limit = value
        default: words.append(arguments[index])
        }
        index += 1
    }
    #if canImport(ProximaKit)
    // Native files are best-effort here: an unreadable one should not block searching the rest.
    let native = try historyFiles(home: home, environment: environment)
        .flatMap { (try? $0.read())?.components(separatedBy: "\n") ?? [] }
    let corpus = searchCorpus(captured: try captured.commandStats(), native: native)
    var engine = HistorySearch(directory: captured.url.appendingPathExtension("search"))
    let config = try SmcliConfig.load(from: SmcliConfig.location(home: home, environment: environment))
    if let model = config.ollamaModel, let dimension = config.ollamaDimension {
        engine.ollama = OllamaEmbedder(client: Ollama(environment: environment), model: model, dimension: dimension)
    }
    let hits = try await engine.search(words.joined(separator: " "), corpus: corpus, limit: limit) {
        FileHandle.standardError.write(Data("\($0)\n".utf8))
    }
    if json {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        print(String(decoding: try encoder.encode(hits), as: UTF8.self))
    } else if hits.isEmpty {
        print("No matching commands.")
    } else {
        for hit in hits {
            var details = ["runs=\(hit.runs)"]
            if let lastRun = hit.lastRun { details.append("last=\(lastRun)") }
            if let code = hit.lastExitCode { details.append("exit=\(code)") }
            if let directory = hit.lastDirectory { details.append(directory) }
            if hit.source == "native" { details.append("native history") }
            print(hit.command)
            print("    " + details.joined(separator: "  "))
        }
    }
    #else
    _ = (words, limit, json, captured, home, environment)
    throw HistoryError(message: "search requires macOS 14 or later.")
    #endif
}

/// Top captured commands for the current user and directory, for shell autocompletion.
func suggest(_ arguments: [String], captured: CapturedHistory, environment: [String: String]) throws {
    var words: [String] = []
    var limit = 10
    var json = false
    // The shell's $PWD keeps symlinked paths as typed, matching what the capture hooks recorded.
    var directory = environment["PWD"].flatMap { $0.hasPrefix("/") ? $0 : nil }
        ?? FileManager.default.currentDirectoryPath
    var index = 0
    while index < arguments.count {
        switch arguments[index] {
        case "--json": json = true
        case "--limit", "-n", "--dir":
            let option = arguments[index]
            index += 1
            guard index < arguments.count else { throw HistoryError(message: "\(option) requires a value.") }
            if option == "--dir" {
                guard arguments[index].hasPrefix("/") else { throw HistoryError(message: "--dir must be an absolute path.") }
                directory = arguments[index]
            } else {
                guard let value = Int(arguments[index]), value > 0 else {
                    throw HistoryError(message: "--limit requires a positive number.")
                }
                limit = value
            }
        default: words.append(arguments[index])
        }
        index += 1
    }
    let suggestions = try captured.suggestions(user: NSUserName(), directory: directory,
                                               prefix: words.joined(separator: " "), limit: limit)
    if json {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        print(String(decoding: try encoder.encode(suggestions), as: UTF8.self))
    } else {
        // One command per line so shells can feed the output straight into completion.
        for suggestion in suggestions { print(suggestion.command) }
    }
}

/// Parses a shell epoch time; EPOCHREALTIME uses the locale's decimal separator. Empty means unknown.
func epoch(_ text: String) throws -> Double? {
    if text.isEmpty { return nil }
    guard let value = Double(text.replacingOccurrences(of: ",", with: ".")), value > 0 else {
        throw HistoryError(message: "Invalid time: \(text)")
    }
    return value
}
