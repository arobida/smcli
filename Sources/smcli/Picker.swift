import Foundation
import SmcliCore
import SmcliSearch

/// Interactive history picker bound by `shell-init`: Left opens ranked suggestions for this directory, Up/Ctrl-R all history;
/// typing filters that list and Tab searches all history semantically. Draws on /dev/tty and prints the chosen command to stdout.
struct Picker {
    /// Suggestions first (ranked for this directory), then every other command, most recent first.
    let candidates: [String]
    /// How many leading `candidates` are directory suggestions; the rest is full history.
    let suggestionCount: Int
    var query: String
    var selected = 0
    /// Semantic results for `semanticQuery`; replaces keyword filtering until the query changes.
    var semantic: [String]? = nil
    var semanticQuery = ""
    var status = ""
    /// History is the default; Ctrl-T switches to file-name search and Ctrl-G to content search. Each keeps its own query.
    enum Mode: String, CaseIterable { case history, files, grep }
    var mode = Mode.history
    var savedQueries: [Mode: String] = [:]
    /// Results for `fetched`, so a key that doesn't change the query doesn't search again.
    var hits: [FileHit] = []
    /// What a files or grep search returned; `complete` is false when grep stopped early.
    struct FileResults { var hits: [FileHit]; var complete = true }
    var fetched: (mode: Mode, query: String)?
    /// Files mode starts on this directory's entries, filtered as you type; Tab switches to fff over the whole project.
    var localFiles: [FileHit] = []
    var globalFiles = false
    /// Ctrl-F narrows the directory list to directories, then files, then back to both.
    enum Kind: String, CaseIterable { case all, folders, files }
    var kind = Kind.all

    init(suggestions: [String], captured: [String: CommandStats], native: [String], query: String = "") {
        self.init(suggestions: suggestions, recent: Self.recent(captured), native: native, query: query)
    }

    /// `recent` is captured history, most recently run first.
    init(suggestions: [String], recent: [String], native: [String], query: String = "") {
        var seen = Set<String>()
        var ordered: [String] = []
        func add(_ command: String) {
            let command = command.trimmingCharacters(in: .whitespaces)
            if !command.isEmpty && !isSmcliCommand(command) && seen.insert(command).inserted { ordered.append(command) }
        }
        suggestions.forEach(add)
        suggestionCount = ordered.count
        recent.forEach(add)
        native.reversed().forEach(add)
        candidates = ordered
        self.query = query
    }

    static func recent(_ captured: [String: CommandStats]) -> [String] {
        captured.sorted { $0.value.lastRun != $1.value.lastRun ? $0.value.lastRun > $1.value.lastRun : $0.key < $1.key }
            .map(\.key)
    }

    /// Commands containing every query word (case-insensitive), keeping candidate order.
    var matches: [String] {
        if let semantic, semanticQuery == query { return semantic }
        let terms = query.lowercased().split(whereSeparator: \.isWhitespace)
        guard !terms.isEmpty else { return candidates }
        return candidates.filter { command in
            let lowered = command.lowercased()
            return terms.allSatisfy { lowered.contains($0) }
        }
    }

    /// Whether `command` came from this directory's suggestions rather than full history.
    func isSuggestion(_ command: String) -> Bool {
        candidates.prefix(suggestionCount).contains(command)
    }

    enum Row: Equatable { case header(String), command(Int) }

    /// Display rows for `current`: section headers split suggestions from history (not for semantic results,
    /// whose order is by relevance). `.command` holds an index into `current`.
    func rows(_ current: [String]) -> [Row] {
        guard !(semantic != nil && semanticQuery == query) else { return current.indices.map(Row.command) }
        let split = current.firstIndex { !isSuggestion($0) } ?? current.count
        var rows: [Row] = []
        if split > 0 { rows.append(.header("Suggested in \(FileManager.default.currentDirectoryPath)")) }
        rows += (0..<split).map(Row.command)
        if split < current.count { rows.append(.header("History")) }
        rows += (split..<current.count).map(Row.command)
        return rows
    }

    // MARK: - Terminal

    enum Key: Equatable { case up, down, enter, cancel, backspace, clear, tab, files, grep, kind, text(String), other }

    enum Choice: Equatable {
        /// `query` is what was typed; `semantic` says the choice came from the Tab results rather than the filtered list.
        case command(String, query: String, semantic: Bool)
        case file(FileHit, Mode, query: String)
    }

    /// Ctrl-T toggles files and Ctrl-G grep, each back to history when pressed again; each mode keeps its own query.
    mutating func toggle(_ target: Mode) {
        savedQueries[mode] = query
        mode = mode == target ? .history : target
        query = savedQueries[mode] ?? ""
        selected = 0
        status = ""
    }

    /// Runs the picker on the controlling terminal. `files` searches the service in the files and grep modes;
    /// nil when there is none. Returns the choice, or nil if cancelled.
    mutating func run(search: ((String) async throws -> [String])?,
                      files: ((Mode, String) throws -> FileResults)? = nil) async throws -> Choice? {
        let tty = open("/dev/tty", O_RDWR | O_CLOEXEC)
        guard tty >= 0 else { throw HistoryError(message: "pick needs a terminal.") }
        defer { close(tty) }
        var original = termios()
        tcgetattr(tty, &original)
        var raw = original
        raw.c_lflag &= ~tcflag_t(ICANON | ECHO | ISIG)
        raw.c_iflag &= ~tcflag_t(IXON | ICRNL)
        tcsetattr(tty, TCSAFLUSH, &raw)
        Self.write(tty, "\u{1B}[?1049h")
        defer {
            Self.write(tty, "\u{1B}[?1049l")
            tcsetattr(tty, TCSAFLUSH, &original)
        }
        while true {
            if mode == .files && !globalFiles {
                hits = localMatches
                fetched = nil
            } else if mode != .history && fetched.map({ $0 != (mode, query) }) != false {
                refetch(files)
            }
            let current = mode == .history ? matches : hits.map(\.path)
            selected = min(selected, max(0, current.count - 1))
            Self.write(tty, render(current, size: Self.size(tty)))
            switch Self.readKey(tty) {
            case .up: selected = max(0, selected - 1)
            case .down: selected = min(max(0, current.count - 1), selected + 1)
            case .enter:
                if current.isEmpty { return nil }
                return mode == .history ? .command(current[selected], query: query, semantic: semantic != nil && semanticQuery == query)
                    : .file(hits[selected], mode, query: query)
            case .cancel: return nil
            case .files: toggle(.files)
            case .grep: toggle(.grep)
            case .kind where mode == .files && !globalFiles:
                kind = Kind.allCases[(Kind.allCases.firstIndex(of: kind)! + 1) % Kind.allCases.count]
                selected = 0
            case .kind: break
            case .backspace: if !query.isEmpty { query.removeLast(); edited() }
            case .clear: query = ""; globalFiles = false; edited()
            case .text(let text): query += text; edited()
            case .tab where mode == .files:
                globalFiles = true
                selected = 0
            case .tab:
                guard mode == .history, let search, !query.trimmingCharacters(in: .whitespaces).isEmpty else { break }
                status = "searching…"
                Self.write(tty, render(current, size: Self.size(tty)))
                do {
                    semantic = try await search(query)
                    semanticQuery = query
                    selected = 0
                    status = "semantic results · type to filter again"
                } catch {
                    status = error.localizedDescription
                }
            case .other: break
            }
        }
    }

    private mutating func edited() {
        selected = 0
        status = ""
    }

    /// This directory's entries of the chosen kind containing every query word (case-insensitive).
    var localMatches: [FileHit] {
        let terms = query.lowercased().split(whereSeparator: \.isWhitespace)
        return localFiles.filter { hit in
            let directory = hit.insert.hasSuffix("/")
            return (kind == .all || directory == (kind == .folders)) && terms.allSatisfy { hit.insert.lowercased().contains($0) }
        }
    }

    /// Entries of `directory` for files-mode suggestions: directories first (with a trailing `/`), then files,
    /// each most recently modified first. Hidden entries are listed; `.git` and `.DS_Store` are not.
    static func directoryEntries(_ directory: String, limit: Int = 200) -> [FileHit] {
        let url = URL(fileURLWithPath: directory)
        let keys: [URLResourceKey] = [.isDirectoryKey, .contentModificationDateKey]
        let urls = (try? FileManager.default.contentsOfDirectory(at: url, includingPropertiesForKeys: keys))?
            .filter { ![".git", ".DS_Store"].contains($0.lastPathComponent) } ?? []
        let entries = urls.map { url -> (name: String, directory: Bool, modified: Date) in
            let values = try? url.resourceValues(forKeys: Set(keys))
            return (url.lastPathComponent, values?.isDirectory == true, values?.contentModificationDate ?? .distantPast)
        }
        return entries.sorted { $0.directory != $1.directory ? $0.directory : $0.modified != $1.modified ? $0.modified > $1.modified : $0.name < $1.name }
            .prefix(limit)
            .map { FileHit(path: $0.name, insert: $0.name + ($0.directory ? "/" : ""), provenance: ["here"]) }
    }

    /// The `limit` best-scored picked files that still exist, inserted relative to `directory`, then `entries` not already listed.
    static func withPicked(_ scores: [String: Double], root: String, directory: String, entries: [FileHit], limit: Int = 10) -> [FileHit] {
        let picked = scores.sorted { $0.value != $1.value ? $0.value > $1.value : $0.key < $1.key }
            .filter { FileManager.default.fileExists(atPath: root + "/" + $0.key) }
            .prefix(limit)
            .map { FileHit(path: $0.key, insert: FileHit.relative($0.key, root: root, from: directory), provenance: ["picked"]) }
        let shown = Set(picked.map(\.insert))
        return picked + entries.filter { !shown.contains($0.insert) }
    }

    /// Searches for the current query; an unreachable service or a failed search shows as a status line.
    private mutating func refetch(_ files: ((Mode, String) throws -> FileResults)?) {
        fetched = (mode, query)
        hits = []
        selected = 0
        guard let files else { status = "files and grep need smcli-service"; return }
        guard mode == .files || !query.isEmpty else { status = "type to search file contents"; return }
        do {
            let results = try files(mode, query)
            hits = results.hits
            status = results.complete ? "" : "\(hits.count) grep results so far · search stopped early, type more to narrow it"
        } catch {
            status = error.localizedDescription
        }
    }

    func render(_ current: [String], size: (rows: Int, columns: Int)) -> String {
        if mode != .history { return renderFiles(size: size) }
        let width = max(20, size.columns)
        let mode = query.isEmpty ? "" : semantic != nil && semanticQuery == query ? "semantic search" : "\(current.count) matches"
        let summary = status.isEmpty ? mode : status
        // An empty query has no summary line: the suggestions header already names the directory.
        let visible = max(1, size.rows - (summary.isEmpty ? 3 : 4))
        let rows = rows(current)
        let selectedRow = rows.firstIndex(of: .command(selected)) ?? 0
        // Keep the selection on screen, along with its section header when that fits.
        let top = max(0, selectedRow - visible + 1)
        var out = "\u{1B}[H\u{1B}[2J"
        out += " \u{1B}[38;5;141m❯\u{1B}[0m \(Self.visible(query))\u{1B}[s\n"
        if !summary.isEmpty { out += " \u{1B}[2m\(Self.truncate(Self.visible(summary), width - 2))\u{1B}[0m\n" }
        for row in rows.dropFirst(top).prefix(visible) {
            switch row {
            case .header(let title):
                let label = " \(title) "
                out += " \u{1B}[1;38;5;141m\(label)\u{1B}[0;2m\(String(repeating: "─", count: max(0, width - label.count - 2)))\u{1B}[0m\n"
            case .command(let index):
                let command = current[index]
                let mark = isSuggestion(command) ? "\u{1B}[38;5;141m★\u{1B}[0m" : " "
                let line = Self.truncate(Self.visible(command), width - 4)
                if index == selected {
                    out += "\u{1B}[38;5;141m▌\u{1B}[0m\(mark)\u{1B}[48;5;236m\u{1B}[38;5;231m \(line)\u{1B}[K\u{1B}[0m\n"
                } else {
                    out += " \(mark) \(line)\n"
                }
            }
        }
        if current.isEmpty { out += "  \u{1B}[2mNo matching commands.\u{1B}[0m\n" }
        out += "\u{1B}[\(size.rows);1H\u{1B}[2m type to search · ↑↓ move · enter select · tab semantic · ^T files · ^G grep · esc cancel\u{1B}[0m"
        return out + "\u{1B}[u"
    }

    /// File and grep results: Git status, path (with line for grep), then where the result came from.
    func renderFiles(size: (rows: Int, columns: Int)) -> String {
        let width = max(20, size.columns)
        let visible = max(1, size.rows - 4)
        let top = max(0, selected - visible + 1)
        let local = mode == .files && !globalFiles
        let label = local ? kind.rawValue : mode == .files ? "files" : "grep"
        let found = local ? "\(hits.count) in \(FileManager.default.currentDirectoryPath) · tab searches the project" : "\(hits.count) \(label) results"
        let summary = hits.isEmpty && !local ? (status.isEmpty ? "no matches" : status) : (status.isEmpty ? found : status)
        var out = "\u{1B}[H\u{1B}[2J"
        out += " \u{1B}[38;5;141m\(label) ❯\u{1B}[0m \(Self.visible(query))\u{1B}[s\n"
        out += " \u{1B}[2m\(Self.truncate(Self.visible(summary), width - 2))\u{1B}[0m\n"
        for (index, hit) in hits.enumerated().dropFirst(top).prefix(visible) {
            let git = Self.visible(hit.git ?? "").padding(toLength: 2, withPad: " ", startingAt: 0)
            var name = local ? hit.insert : hit.path
            if let line = hit.line { name += ":\(line)" + (hit.text.map { "  " + $0.trimmingCharacters(in: .whitespaces) } ?? "") }
            let source = Self.visible(hit.provenance.joined(separator: "+"))
            let room = max(4, width - 8 - source.count)
            let text = Self.truncate(Self.visible(name), room)
            let gap = String(repeating: " ", count: max(1, room - text.count + 1))
            if index == selected {
                out += "\u{1B}[38;5;141m▌\u{1B}[0m\u{1B}[48;5;236m\u{1B}[38;5;231m \(git) \(text)\(gap)\u{1B}[2m\(source)\u{1B}[K\u{1B}[0m\n"
            } else {
                out += " \u{1B}[33m\(git)\u{1B}[0m \(text)\(gap)\u{1B}[2m\(source)\u{1B}[0m\n"
            }
        }
        out += "\u{1B}[\(size.rows);1H\u{1B}[2m type to search · ↑↓ move · enter insert path · \(local ? "tab search all · ^F dirs/files · " : "")\(mode == .files ? "^T history · ^G grep" : "^G history · ^T files") · esc cancel\u{1B}[0m"
        return out + "\u{1B}[u"
    }

    /// Display form of text from history or a repository: control characters (ESC, C1 CSI...) are shown as `^[`-style
    /// caret notation or `<U+009B>`, never sent to the terminal, so file names and contents can't move the cursor,
    /// clear the screen or forge picker rows. The original text is still what gets inserted.
    static func visible(_ text: String) -> String {
        var out = ""
        for scalar in text.unicodeScalars {
            switch scalar.value {
            case 0x0A: out += "↵ "
            case 0x09: out += "  "
            case 0..<0x20: out += "^" + String(UnicodeScalar(UInt8(scalar.value + 0x40)))
            case 0x7F: out += "^?"
            case 0x80..<0xA0: out += String(format: "<U+%04X>", scalar.value)
            default: out.unicodeScalars.append(scalar)
            }
        }
        return out
    }

    private static func truncate(_ text: String, _ width: Int) -> String {
        text.count > width ? String(text.prefix(max(0, width - 1))) + "…" : text
    }

    private static func size(_ tty: Int32) -> (rows: Int, columns: Int) {
        var size = winsize()
        guard ioctl(tty, TIOCGWINSZ, &size) == 0, size.ws_row > 0 else { return (24, 80) }
        return (Int(size.ws_row), Int(size.ws_col))
    }

    static func readKey(_ tty: Int32) -> Key {
        guard let byte = readByte(tty) else { return .cancel }
        switch byte {
        case 0x0A, 0x0D: return .enter
        case 0x03, 0x04: return .cancel
        case 0x07: return .grep          // Ctrl-G
        case 0x09: return .tab
        case 0x7F, 0x08: return .backspace
        case 0x06: return .kind          // Ctrl-F
        case 0x14: return .files         // Ctrl-T
        case 0x15: return .clear
        case 0x10, 0x12: return .up      // Ctrl-P, Ctrl-R
        case 0x0E, 0x13: return .down    // Ctrl-N, Ctrl-S
        case 0x1B:
            // A lone Escape cancels; arrows arrive as ESC [ A / ESC O A. Other sequences are consumed and ignored.
            // macOS poll() rejects /dev/tty, so wait for the next byte with a termios read timeout instead.
            guard let next = readByte(tty, timeout: true) else { return .cancel }
            guard next == 0x5B || next == 0x4F else { return .other }
            var code = readByte(tty)
            while let byte = code, (0x30...0x3F).contains(byte) { code = readByte(tty) }
            return code == UInt8(ascii: "A") ? .up : code == UInt8(ascii: "B") ? .down : .other
        case 0x20...0x7E: return .text(String(UnicodeScalar(byte)))
        case 0xC0...0xF7:
            var bytes = [byte]
            let length = byte >= 0xF0 ? 4 : byte >= 0xE0 ? 3 : 2
            while bytes.count < length, let next = readByte(tty) { bytes.append(next) }
            return String(bytes: bytes, encoding: .utf8).map(Key.text) ?? .other
        default: return .other
        }
    }

    private static func readByte(_ tty: Int32, timeout: Bool = false) -> UInt8? {
        var byte: UInt8 = 0
        guard timeout else { return read(tty, &byte, 1) == 1 ? byte : nil }
        var blocking = termios()
        tcgetattr(tty, &blocking)
        var timed = blocking
        withUnsafeMutableBytes(of: &timed.c_cc) { $0[Int(VMIN)] = 0; $0[Int(VTIME)] = 1 }  // 100ms
        tcsetattr(tty, TCSANOW, &timed)
        defer { tcsetattr(tty, TCSANOW, &blocking) }
        return read(tty, &byte, 1) == 1 ? byte : nil
    }

    private static func write(_ tty: Int32, _ text: String) {
        let data = Array(text.utf8)
        var offset = 0
        while offset < data.count {
            let written = data.withUnsafeBytes { Foundation.write(tty, $0.baseAddress! + offset, data.count - offset) }
            if written <= 0 { return }
            offset += written
        }
    }
}

/// `smcli pick [query...]`: prints the chosen command, or exits 1 if cancelled. A chosen file is printed as one
/// quoted shell word for SMCLI_SHELL (bash, zsh or fish) with exit status 10: the adapter inserts it, never runs it.
func pick(_ arguments: [String], captured: CapturedHistory, service: ServiceClient, home: URL,
          environment: [String: String]) async throws {
    let directory = environment["PWD"].flatMap { $0.hasPrefix("/") ? $0 : nil } ?? FileManager.default.currentDirectoryPath
    // The service answers from memory; without it, scan the database. Its recent list is the current user's
    // commands only, where commandStats also lists other users' (from imports).
    var request = ServiceRequest(op: .pick)
    (request.directory, request.limit) = (directory, 50)
    request.session = environment["SMCLI_SESSION"]
    let response = service.send(request)
    let project = ProjectContext.discover(directory)
    let context = RankingContext(project: project, session: request.session, environment: environment)
    var stats: [String: CommandStats]? = nil
    let suggestions: [String]
    let recent: [String]
    if let served = response?.suggestions, let servedRecent = response?.recent {
        (suggestions, recent) = (served.map(\.command), servedRecent)
    } else {
        suggestions = try captured.suggestions(user: NSUserName(), directory: directory, limit: 50, context: context).map(\.command)
        stats = try captured.commandStats(directory: directory, project: context.uses(.project) ? context.projectRoot : nil)
        recent = Picker.recent(stats!)
    }
    // Native files are best-effort, as in search.
    let native = try historyFiles(home: home, environment: environment)
        .flatMap { (try? $0.read())?.components(separatedBy: "\n") ?? [] }
    // Left (--suggest) shows only this directory's suggestions; Up and Ctrl-R show all history, newest first.
    // Ctrl-T (--files) opens on this directory's files and Ctrl-G (--grep) on content search; inside the picker they switch modes.
    let flag = arguments.first.flatMap { ["--suggest", "--files", "--grep"].contains($0) ? $0 : nil }
    let query = arguments.dropFirst(flag == nil ? 0 : 1).joined(separator: " ")
    var picker = flag == "--suggest"
        ? Picker(suggestions: suggestions, recent: [], native: [], query: query)
        : Picker(suggestions: [], recent: recent, native: native, query: flag == nil ? query : "")
    // Files picked before in this project lead the directory's entries; read straight from the database so it works without the service.
    let root = project.repository?.root ?? realpath(directory, nil).map { defer { free($0) }; return String(cString: $0) } ?? directory
    let scores = (try? captured.openConnection()).flatMap { try? captured.selectionScores(root: root, on: $0) } ?? [:]
    picker.localFiles = Picker.withPicked(scores, root: root, directory: directory, entries: Picker.directoryEntries(directory))
    // The command line is not a file query, so files mode starts empty.
    if flag == "--files" { picker.mode = .files }
    if flag == "--grep" { picker.mode = .grep }
    var search: ((String) async throws -> [String])? = nil
    #if canImport(TalaDB)
    // Built on the first search, not per keystroke: loading an embedding model would dominate latency.
    // Deferring it also keeps a broken model or config to a status-line error rather than a dead history key.
    var prepared: (engine: HistorySearch, corpus: [String: SearchEntry], programs: [String: Set<String>], signals: SearchSignals)?
    search = { query in
        if prepared == nil, let hits = serviceSearch(query, limit: 50, service: service, environment: environment) {
            return hits.map(\.command)
        }
        if prepared == nil {
            // Semantic search already loads a model, so the full stats scan is deferred to here.
            let corpus = searchCorpus(captured: try stats ?? captured.commandStats(directory: directory), native: native)
            prepared = (try searchEngine(captured: captured, home: home, environment: environment), corpus, programArguments(corpus),
                        try searchSignals(captured: captured, context: context))
        }
        let (engine, corpus, programs, signals) = prepared!
        return try await engine.search(query, corpus: corpus, limit: 50, programs: programs, signals: signals).map(\.command)
    }
    #endif
    let files: (Picker.Mode, String) throws -> Picker.FileResults = { mode, query in
        var request = ServiceRequest(op: mode == .files ? .files : .grep)
        (request.directory, request.query, request.limit) = (directory, query, 100)
        // The first search in a repository waits for fff's scan.
        guard let response = service.send(request, timeout: 15) else {
            throw HistoryError(message: "files and grep need smcli-service (off, unreachable or too slow)")
        }
        guard response.ok else { throw HistoryError(message: response.error?.message ?? "search failed") }
        return .init(hits: response.files ?? [], complete: response.partial != true)
    }
    guard let chosen = try await picker.run(search: search, files: service.mode == .off ? nil : files) else { exit(1) }
    switch chosen {
    case .command(let command, let query, let semantic):
        // Remembered for ranking because the user chose it for this query; best effort, in-process when no service.
        var request = ServiceRequest(op: .select)
        (request.directory, request.command, request.query, request.mode) = (directory, command, query, semantic ? "search" : "suggest")
        if service.send(request, timeout: 2) == nil, !isSmcliCommand(command),
           let filter = try? CaptureFilter(config: SmcliConfig.load(from: SmcliConfig.location(home: home, environment: environment))),
           let db = try? captured.openConnection() {
            _ = try? captured.recordPick(query: String(query.prefix(1000)), command: command, kind: request.mode ?? "suggest",
                                     directory: directory, filter: filter, on: db)
        }
        print(command)
    case .file(let hit, let mode, let query):
        // Remembered for ranking only because the user chose it; best effort.
        // Directory entries aren't root-relative, so only project search results are recorded.
        if hit.provenance != ["here"] {
            var request = ServiceRequest(op: .select)
            (request.directory, request.path, request.mode, request.query) = (directory, hit.path, mode.rawValue, query)
            _ = service.send(request, timeout: 2)
        }
        // The adapter passes the text left of the cursor; without it (a hook from an older build) we can't tell.
        let shell = environment["SMCLI_SHELL"] ?? "bash"
        guard let before = environment["SMCLI_LBUFFER"], ShellQuoting.insertable(after: before, shell: shell) else {
            FileHandle.standardError.write(Data("smcli: not inserting a path inside an open quote or substitution, or after here-document syntax; finish the command first\(environment["SMCLI_LBUFFER"] == nil ? " (or reload smcli's shell-init)" : "").\n".utf8))
            exit(1)
        }
        // Not a command: the adapter inserts this word at the cursor instead of replacing the line.
        print(ShellQuoting.quote(hit.insert, shell: shell))
        exit(10)
    }
}
