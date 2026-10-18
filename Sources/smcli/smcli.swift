import Foundation

@main
struct Smcli {
    static let usage = """
    Usage: smcli init [--model NAME | --coreai | --builtin]
           smcli history [--json]
           smcli shell-init bash|zsh
           smcli search [query...] [--command NAME] [--subcommand NAME] [--since TIME] [--before TIME]
                        [--min-runs N] [--author NAME] [--dir PATH] [--flag NAME] [--limit N] [--json]
           smcli suggest [prefix...] [--dir PATH] [--limit N] [--json]
           smcli pick [query...]
           smcli reindex
           smcli health [--json]
           smcli import <legacy.jsonl>
           smcli import --atuin [--replace] [history.db]
           smcli prune [--dry-run]
    Read saved shell history, or print opt-in hooks for structured metadata capture.
    search filters are exact; TIME is 30m, 12h, 7d, 2w, YYYY-MM-DD, or ISO 8601, matched against the last run.
    reindex re-embeds every command for search; health checks the index and the embedding model.
    pick opens the history picker that shell-init binds to Up and Ctrl-R (SMCLI_NO_KEYS=1 to skip).
    init picks an Ollama embedding model for search (saved to ~/.config/smcli/config.json, or SMCLI_CONFIG).
    Commands with a leading space, likely secrets, or matching historyFilter/cwdFilter in the config are not recorded;
    prune removes already-stored ones. import --atuin --replace discards existing smcli history first.
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
            let configURL = SmcliConfig.location(home: home, environment: environment)
            // Loaded only by commands that record or prune, so a bad config never blocks reading history.
            func filter() throws -> CaptureFilter { try CaptureFilter(config: SmcliConfig.load(from: configURL)) }
            if arguments.count == 2 && arguments[0] == "shell-init" {
                let executable = Bundle.main.executableURL?.path ?? URL(fileURLWithPath: CommandLine.arguments[0]).path
                print(try shellIntegration(shell: arguments[1], executable: executable))
            } else if arguments.count == 9 && arguments[0] == "record" {
                // record <shell> <session> <directory> <status> <started> <ended> <native HISTFILE> <command>
                guard let status = Int(arguments[4]) else { throw HistoryError(message: "Invalid exit code.") }
                guard try filter().allows(command: arguments[8], directory: arguments[3]) else { return }
                try captured.append(command: arguments[8], shell: arguments[1], directory: arguments[3], exitCode: status,
                                    nativeFile: arguments[7], session: arguments[2],
                                    startedAt: epoch(arguments[5]), endedAt: epoch(arguments[6]))
            } else if arguments.count == 6 && arguments[0] == "record" {
                // Hooks installed by earlier versions stay working until the shell is restarted.
                guard let status = Int(arguments[3]) else { throw HistoryError(message: "Invalid exit code.") }
                guard try filter().allows(command: arguments[4], directory: arguments[2]) else { return }
                try captured.append(command: arguments[4], shell: arguments[1], directory: arguments[2],
                                    exitCode: status, nativeFile: arguments[5])
            } else if arguments.first == "search" {
                try await search(Array(arguments.dropFirst()), captured: captured, home: home, environment: environment)
            } else if arguments == ["reindex"] {
                try await reindex(captured: captured, home: home, environment: environment)
            } else if arguments == ["health"] || arguments == ["health", "--json"] {
                try await health(json: arguments.count == 2, captured: captured, home: home, environment: environment)
            } else if arguments.first == "pick" {
                try await pick(Array(arguments.dropFirst()), captured: captured, home: home, environment: environment)
            } else if arguments.first == "init" {
                try await initialize(Array(arguments.dropFirst()), configURL: configURL,
                                     environment: environment)
            } else if arguments.first == "suggest" {
                try suggest(Array(arguments.dropFirst()), captured: captured, environment: environment)
            } else if arguments == ["prune"] || arguments == ["prune", "--dry-run"] {
                let dryRun = arguments.count == 2
                let pruned = try captured.prune(filter: filter(), dryRun: dryRun)
                // Only a dry run lists them; printing would copy secrets being removed into terminal scrollback.
                if dryRun { for command in pruned { print(command) } }
                print(dryRun ? "\(pruned.count) records would be removed." : "Removed \(pruned.count) records.")
            } else if arguments.first == "import" && arguments.dropFirst().first == "--atuin",
                      case let rest = Array(arguments.dropFirst(2)), rest.filter({ $0 != "--replace" }).count <= 1 {
                let replace = rest.contains("--replace")
                let source = rest.first { $0 != "--replace" }.map { URL(fileURLWithPath: $0) }
                    ?? CapturedHistory.atuinLocation(home: home, environment: environment)
                let count = try captured.importAtuin(from: source, replace: replace)
                print(replace ? "Replaced \(captured.url.path) with \(count) records from \(source.path)."
                              : "Imported \(count) new records from \(source.path) into \(captured.url.path).")
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
    var filter = SearchFilter()
    let now = Date().timeIntervalSince1970
    var index = 0
    while index < arguments.count {
        let option = arguments[index]
        switch option {
        case "--json": json = true
        case "--limit", "-n", "--min-runs", "--command", "--subcommand", "--since", "--before", "--author", "--dir", "--flag":
            index += 1
            guard index < arguments.count, !arguments[index].isEmpty else { throw HistoryError(message: "\(option) requires a value.") }
            let value = arguments[index]
            switch option {
            case "--command": filter.program = value
            case "--subcommand": filter.subcommand = value
            case "--author": filter.author = value
            case "--flag": filter.flag = value
            case "--dir":
                guard value.hasPrefix("/") else { throw HistoryError(message: "--dir must be an absolute path.") }
                filter.directory = value
            case "--since": filter.since = try SearchFilter.time(value, now: now)
            case "--before": filter.before = try SearchFilter.time(value, now: now)
            default:
                guard let number = Int(value), number > 0 else { throw HistoryError(message: "\(option) requires a positive number.") }
                if option == "--min-runs" { filter.minRuns = number } else { limit = number }
            }
        default: words.append(option)
        }
        index += 1
    }
    #if canImport(ProximaKit)
    let corpus = try searchCorpus(captured: captured, home: home, environment: environment)
    let engine = try searchEngine(captured: captured, home: home, environment: environment)
    let hits = try await engine.search(words.joined(separator: " "), corpus: corpus, limit: limit, filter: filter) {
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
    _ = (words, limit, json, filter, captured, home, environment)
    throw HistoryError(message: "search requires macOS 14 or later.")
    #endif
}

#if canImport(ProximaKit)
/// Captured stats plus native history. Native files are best-effort: an unreadable one should not block the rest.
func searchCorpus(captured: CapturedHistory, home: URL, environment: [String: String]) throws -> [String: SearchEntry] {
    let native = try historyFiles(home: home, environment: environment)
        .flatMap { (try? $0.read())?.components(separatedBy: "\n") ?? [] }
    return searchCorpus(captured: try captured.commandStats(), native: native)
}

/// The search index next to the history database, using the model chosen by `smcli init`.
func searchEngine(captured: CapturedHistory, home: URL, environment: [String: String]) throws -> HistorySearch {
    var engine = HistorySearch(directory: captured.url.appendingPathExtension("search"))
    let config = try SmcliConfig.load(from: SmcliConfig.location(home: home, environment: environment))
    if config.coreai == true {
        #if canImport(CoreAI)
        guard #available(macOS 27, *) else { throw HistoryError(message: "The Core AI model needs macOS 27; rerun `smcli init`.") }
        engine.coreai = CoreAIEmbedder()
        return engine
        #else
        throw HistoryError(message: "This build has no Core AI support; rerun `smcli init`.")
        #endif
    }
    if let model = config.ollamaModel, let dimension = config.ollamaDimension {
        engine.ollama = OllamaEmbedder(client: Ollama(environment: environment), model: model, dimension: dimension)
    }
    return engine
}
#endif

func reindex(captured: CapturedHistory, home: URL, environment: [String: String]) async throws {
    #if canImport(ProximaKit)
    let corpus = try searchCorpus(captured: captured, home: home, environment: environment)
    let engine = try searchEngine(captured: captured, home: home, environment: environment)
    let tty = isatty(STDERR_FILENO) == 1
    var reported = -1
    let count = try await engine.reindex(corpus: corpus) { done, total in
        // A terminal gets one updating line; logs get a line per tenth.
        let percent = total == 0 ? 100 : done * 100 / total
        if tty {
            FileHandle.standardError.write(Data("\rEmbedding commands: \(done)/\(total) (\(percent)%)".utf8))
        } else if percent / 10 != reported / 10 {
            FileHandle.standardError.write(Data("Embedding commands: \(done)/\(total) (\(percent)%)\n".utf8))
        }
        reported = percent
    }
    if tty && !corpus.isEmpty { FileHandle.standardError.write(Data("\n".utf8)) }
    print("Reindexed \(count) commands.")
    #else
    _ = (captured, home, environment)
    throw HistoryError(message: "reindex requires macOS 14 or later.")
    #endif
}

/// Reports the index's state; exits 1 when search would not work.
func health(json: Bool, captured: CapturedHistory, home: URL, environment: [String: String]) async throws {
    #if canImport(ProximaKit)
    let corpus = try searchCorpus(captured: captured, home: home, environment: environment)
    let report = try await searchEngine(captured: captured, home: home, environment: environment).health(corpus: corpus)
    if json {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        print(String(decoding: try encoder.encode(report), as: UTF8.self))
    } else {
        print("Embedder:     \(report.embedder)")
        print("Index:        \(report.path)")
        if report.built {
            print("Commands:     \(report.indexed) indexed of \(report.commands) in history")
            if report.missing > 0 { print("Missing:      \(report.missing) (embedded on the next search)") }
            if report.stale > 0 { print("Stale:        \(report.stale) (removed on the next search)") }
            print("Size:         \(ByteCountFormatter.string(fromByteCount: Int64(report.bytes), countStyle: .file))")
            if let updated = report.lastUpdated { print("Updated:      \(updated)") }
        } else {
            print("Commands:     not built yet; the next search or `smcli reindex` embeds \(report.commands)")
        }
        if !report.otherIndexes.isEmpty { print("Other models: \(report.otherIndexes.joined(separator: ", "))") }
        print(report.healthy ? "Status:       healthy" : "Status:       unhealthy")
        for problem in report.problems { print("  ✗ \(problem)") }
    }
    if !report.healthy { exit(1) }
    #else
    _ = (json, captured, home, environment)
    throw HistoryError(message: "health requires macOS 14 or later.")
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
