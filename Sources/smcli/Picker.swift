import Foundation

/// Interactive history picker bound to Up/Ctrl-R by `shell-init`. Opens on ranked suggestions for this directory;
/// typing filters every known command. Draws on /dev/tty and prints the chosen command to stdout.
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

    init(suggestions: [String], captured: [String: CommandStats], native: [String], query: String = "") {
        var seen = Set<String>()
        var ordered: [String] = []
        func add(_ command: String) {
            let command = command.trimmingCharacters(in: .whitespaces)
            if !command.isEmpty && !isSmcliCommand(command) && seen.insert(command).inserted { ordered.append(command) }
        }
        suggestions.forEach(add)
        suggestionCount = ordered.count
        captured.sorted { $0.value.lastRun != $1.value.lastRun ? $0.value.lastRun > $1.value.lastRun : $0.key < $1.key }
            .forEach { add($0.key) }
        native.reversed().forEach(add)
        candidates = ordered
        self.query = query
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
        if split > 0 { rows.append(.header("Suggested here")) }
        rows += (0..<split).map(Row.command)
        if split < current.count { rows.append(.header("History")) }
        rows += (split..<current.count).map(Row.command)
        return rows
    }

    // MARK: - Terminal

    enum Key: Equatable { case up, down, enter, cancel, backspace, clear, tab, text(String), other }

    /// Runs the picker on the controlling terminal. Returns the chosen command, or nil if cancelled.
    mutating func run(search: ((String) async throws -> [String])?) async throws -> String? {
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
            let current = matches
            selected = min(selected, max(0, current.count - 1))
            Self.write(tty, render(current, size: Self.size(tty)))
            switch Self.readKey(tty) {
            case .up: selected = max(0, selected - 1)
            case .down: selected = min(max(0, current.count - 1), selected + 1)
            case .enter: return current.isEmpty ? nil : current[selected]
            case .cancel: return nil
            case .backspace: if !query.isEmpty { query.removeLast(); edited() }
            case .clear: query = ""; edited()
            case .text(let text): query += text; edited()
            case .tab:
                guard let search, !query.trimmingCharacters(in: .whitespaces).isEmpty else { break }
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

    func render(_ current: [String], size: (rows: Int, columns: Int)) -> String {
        let width = max(20, size.columns)
        let visible = max(1, size.rows - 4)
        let rows = rows(current)
        let selectedRow = rows.firstIndex(of: .command(selected)) ?? 0
        // Keep the selection on screen, along with its section header when that fits.
        let top = max(0, selectedRow - visible + 1)
        let mode = query.isEmpty ? "suggestions for \(FileManager.default.currentDirectoryPath)"
            : semantic != nil && semanticQuery == query ? "semantic search" : "\(current.count) matches"
        var out = "\u{1B}[H\u{1B}[2J"
        out += " \u{1B}[38;5;141m❯\u{1B}[0m \(query)\u{1B}[s\n"
        out += " \u{1B}[2m\(Self.truncate(status.isEmpty ? mode : status, width - 2))\u{1B}[0m\n"
        for row in rows.dropFirst(top).prefix(visible) {
            switch row {
            case .header(let title):
                let label = " \(title) "
                out += " \u{1B}[1;38;5;141m\(label)\u{1B}[0;2m\(String(repeating: "─", count: max(0, width - label.count - 2)))\u{1B}[0m\n"
            case .command(let index):
                let command = current[index]
                let mark = isSuggestion(command) ? "\u{1B}[38;5;141m★\u{1B}[0m" : " "
                let line = Self.truncate(command.replacingOccurrences(of: "\n", with: "↵ "), width - 4)
                if index == selected {
                    out += "\u{1B}[38;5;141m▌\u{1B}[0m\(mark)\u{1B}[48;5;236m\u{1B}[38;5;231m \(line)\u{1B}[K\u{1B}[0m\n"
                } else {
                    out += " \(mark) \(line)\n"
                }
            }
        }
        if current.isEmpty { out += "  \u{1B}[2mNo matching commands.\u{1B}[0m\n" }
        out += "\u{1B}[\(size.rows);1H\u{1B}[2m type to search · ↑↓ move · enter select · tab semantic · esc cancel\u{1B}[0m"
        return out + "\u{1B}[u"
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
        case 0x03, 0x04, 0x07: return .cancel
        case 0x09: return .tab
        case 0x7F, 0x08: return .backspace
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

/// `smcli pick [query...]`: prints the chosen command, or exits 1 if cancelled.
func pick(_ arguments: [String], captured: CapturedHistory, home: URL, environment: [String: String]) async throws {
    let directory = environment["PWD"].flatMap { $0.hasPrefix("/") ? $0 : nil } ?? FileManager.default.currentDirectoryPath
    let suggestions = try captured.suggestions(user: NSUserName(), directory: directory, limit: 50).map(\.command)
    let stats = try captured.commandStats(directory: directory)
    // Native files are best-effort, as in search.
    let native = try historyFiles(home: home, environment: environment)
        .flatMap { (try? $0.read())?.components(separatedBy: "\n") ?? [] }
    var picker = Picker(suggestions: suggestions, captured: stats, native: native,
                        query: arguments.joined(separator: " "))
    var search: ((String) async throws -> [String])? = nil
    #if canImport(ProximaKit)
    // Built once: loading an embedding model per keystroke would dominate search latency.
    let engine = try searchEngine(captured: captured, home: home, environment: environment)
    let corpus = searchCorpus(captured: stats, native: native)
    search = { query in try await engine.search(query, corpus: corpus, limit: 50).map(\.command) }
    #endif
    guard let chosen = try await picker.run(search: search) else { exit(1) }
    print(chosen)
}
