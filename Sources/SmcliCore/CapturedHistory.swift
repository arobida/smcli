import Foundation
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif
#if canImport(SQLite3)
import SQLite3
#else
import CSQLite
#endif

package struct CommandFlag: Codable, Equatable {
    package let name: String
    package let value: String?
}

package struct CommandBreakdown: Codable, Equatable {
    package let mainCommand: String
    package let subcommand: String?
    package let flags: [CommandFlag]
    package let targets: [String]
    package let arguments: [String]

    // shortcut: common tools/options only; use complete tool schemas for semantic parsing.
    package static let tools: Set<String> = ["git", "docker", "npm", "cargo", "swift", "kubectl", "brew", "smcli"]
    package static let valueOptions: [String: Set<String>] = [
        "git": ["-C", "-c", "--git-dir", "--work-tree"],
        "git commit": ["-m", "--message", "-F", "--file", "--author"],
        "docker": ["--context"],
        "docker build": ["-f", "--file", "-t", "--tag"],
        "docker run": ["--name", "-p", "--publish", "-v", "--volume", "-e", "--env"],
        "npm": ["--prefix", "--registry"],
        "cargo": ["--manifest-path", "--target", "--features", "-p", "--package"],
        "swift": ["-c", "--configuration", "--package-path", "--target", "--product", "--filter"],
        "kubectl": ["-n", "--namespace", "--context", "-f", "--filename", "-o", "--output"],
        "ls": ["-I", "--ignore"],
        "grep": ["-e", "--regexp", "-f", "--file", "-A", "-B", "-C"],
    ]

    package static func parse(_ command: String) -> CommandBreakdown? {
        guard var words = shellWords(command), !words.isEmpty else { return nil }
        // Assignment prefixes are not executables. Keep the unmodified command in the record.
        while let first = words.first, let equals = first.firstIndex(of: "="), let head = first.first,
              head == "_" || (head.isASCII && head.isLetter),
              first[..<equals].allSatisfy({ $0 == "_" || ($0.isASCII && ($0.isLetter || $0.isNumber)) }) {
            words.removeFirst()
        }
        guard !words.isEmpty else { return nil }
        let main = words.removeFirst()
        let arguments = words
        let tool = main.split(separator: "/").last.map(String.init) ?? main
        var subcommand: String?
        var flags: [CommandFlag] = []
        var targets: [String] = []
        var options = true
        var index = 0
        while index < words.count {
            let word = words[index]
            if options && word == "--" {
                options = false
            } else if options && word.hasPrefix("-") && word != "-" {
                if let equals = word.firstIndex(of: "=") {
                    flags.append(CommandFlag(name: String(word[..<equals]), value: String(word[word.index(after: equals)...])))
                } else if (Self.valueOptions[tool]?.contains(word) == true || Self.valueOptions["\(tool) \(subcommand ?? "")"]?.contains(word) == true) && index + 1 < words.count {
                    index += 1
                    flags.append(CommandFlag(name: word, value: words[index]))
                } else {
                    flags.append(CommandFlag(name: word, value: nil))
                }
            } else if options && Self.tools.contains(tool) && subcommand == nil {
                subcommand = word
            } else {
                targets.append(word)
            }
            index += 1
        }
        return CommandBreakdown(mainCommand: main, subcommand: subcommand, flags: flags, targets: targets, arguments: arguments)
    }
}

/// Option names in a command (`--amend`, `-m`), without their values. Lines the parser refuses
/// (pipes, substitutions) fall back to whitespace-separated words that start with `-`.
package func commandFlags(_ command: String) -> [String] {
    if let breakdown = CommandBreakdown.parse(command) { return breakdown.flags.map(\.name) }
    return command.split(whereSeparator: \.isWhitespace).map(String.init).filter { $0.hasPrefix("-") && $0 != "-" && $0 != "--" }
        .map { $0.split(separator: "=", maxSplits: 1).first.map(String.init) ?? $0 }
}

// Tokenize without executing or expanding anything. Compound shell syntax is deliberately not guessed.
package func shellWords(_ command: String) -> [String]? {
    let characters = Array(command)
    var words: [String] = []
    var word = ""
    var started = false
    var quote: Character?
    var index = 0
    while index < characters.count {
        let character = characters[index]
        if quote == "'" {
            if character == "'" { quote = nil } else { word.append(character) }
        } else if character == "\\" {
            index += 1
            guard index < characters.count else { return nil }
            let next = characters[index]
            if next != "\n" {
                if quote == "\"" && !["$", "`", "\"", "\\"].contains(next) { word.append("\\") }
                word.append(next)
            }
            started = true
        } else if character == "`" || (character == "$" && index + 1 < characters.count &&
            (characters[index + 1] == "(" || (quote == nil && ["'", "\""].contains(characters[index + 1])))) {
            return nil
        } else if let currentQuote = quote {
            if character == currentQuote { quote = nil } else { word.append(character) }
        } else if character == "'" || character == "\"" {
            quote = character
            started = true
        } else if "|&;<>\n(){}".contains(character) {
            return nil
        } else if character.isWhitespace {
            if started { words.append(word); word = ""; started = false }
        } else if character == "#" && !started {
            break
        } else {
            word.append(character)
            started = true
        }
        index += 1
    }
    guard quote == nil else { return nil }
    if started { words.append(word) }
    return words
}

/// True when a command line runs smcli itself (`smcli …`, `/path/to/smcli …`, `swift run smcli …`),
/// so searches and suggestions are not about the tool. Leading `VAR=value` assignments are skipped.
package func isSmcliCommand(_ command: String) -> Bool {
    var words = command.split(whereSeparator: \.isWhitespace).map { $0.trimmingCharacters(in: CharacterSet(charactersIn: "'\"")) }
    while let first = words.first, let equals = first.firstIndex(of: "="), equals != first.startIndex,
          !first[..<equals].contains("/") {
        words.removeFirst()
    }
    // `swift run` options may take values (`-c release`), so look for the product among the next few words.
    if words.first == "swift" && words.dropFirst().first == "run" {
        return words.dropFirst(2).prefix(5).contains("smcli")
    }
    return words.first.map { $0.split(separator: "/").last == "smcli" } ?? false
}

/// A captured command as presented to readers. `runCount` and `breakdown` are derived at read time,
/// so they reflect the whole log and the current parser rather than whatever was true when written.
package struct CapturedCommand: Codable, Equatable {
    /// Stable UUID, shared with Atuin for imported rows so re-imports deduplicate.
    package let id: String?
    package let timestamp: String
    package let durationMs: Int?
    package let user: String
    package let host: String
    package let session: String
    package let directory: String
    package let shell: String
    package let command: String
    package let exitCode: Int
    /// Each stage's exit status of a pipeline (Zsh and Fish `pipestatus`); nil when the shell didn't report it.
    package let pipestatus: [Int]?
    /// Who wrote the command (a person or an agent), and why; both optional, as in Atuin.
    package let author: String?
    package let intent: String?
    package let runCount: Int
    package let breakdown: CommandBreakdown?
}

package struct CommandStats: Equatable {
    package var runs: Int
    package var lastRun: Double
    package var lastDirectory: String
    package var lastExitCode: Int
    package var author: String? = nil
    package var intent: String? = nil
    package var flags: [String] = []
    /// Runs weighted by age (a run two weeks old counts half), so current habits outrank old ones.
    package var frecency: Double = 0
    package var successes: Int = 0
    /// Runs in the directory passed to `commandStats`, or below it.
    package var runsHere: Int = 0
    /// Runs in the project root passed to `commandStats`, or below it.
    package var runsProject: Int = 0

    package init(runs: Int, lastRun: Double, lastDirectory: String, lastExitCode: Int, author: String? = nil,
                 intent: String? = nil, flags: [String] = [], frecency: Double = 0, successes: Int = 0,
                 runsHere: Int = 0, runsProject: Int = 0) {
        (self.runs, self.lastRun, self.lastDirectory, self.lastExitCode) = (runs, lastRun, lastDirectory, lastExitCode)
        (self.author, self.intent, self.flags, self.frecency) = (author, intent, flags, frecency)
        (self.successes, self.runsHere, self.runsProject) = (successes, runsHere, runsProject)
    }
}

package struct Suggestion: Codable, Equatable {
    package let command: String
    package let score: Double
    package let runs: Int
    /// Runs in exactly the requested directory.
    package let runsHere: Int
    package let lastRun: String
    package let lastExitCode: Int
    /// What moved it beyond usage: where it ran, what it often follows, a pick, a project script or Git state.
    package var why: [String]? = nil
}

package struct HistoryError: LocalizedError {
    package let message: String
    package var errorDescription: String? { message }

    package init(message: String) { self.message = message }
}

package enum SQLValue {
    case text(String), integer(Int), real(Double), null
}

package final class SQLiteDatabase {
    private let db: OpaquePointer

    package init(path: String, readOnly: Bool) throws {
        var handle: OpaquePointer?
        let flags = (readOnly ? SQLITE_OPEN_READONLY : SQLITE_OPEN_READWRITE) | SQLITE_OPEN_NOFOLLOW | SQLITE_OPEN_NOMUTEX
        guard sqlite3_open_v2(path, &handle, flags, nil) == SQLITE_OK, let handle else {
            let message = handle.map { String(cString: sqlite3_errmsg($0)) } ?? "out of memory"
            sqlite3_close(handle)
            throw HistoryError(message: "Cannot open history database \(path): \(message)")
        }
        db = handle
        // Concurrent terminals wait briefly for the writer instead of failing.
        sqlite3_busy_timeout(db, 5000)
        // SQLite has no portable pow(), so usage decay is a function of ours: smcli_decay(age in seconds).
        sqlite3_create_function_v2(db, "smcli_decay", 1, SQLITE_UTF8 | SQLITE_DETERMINISTIC, nil, { context, _, arguments in
            let age = max(0, sqlite3_value_double(arguments![0]))
            sqlite3_result_double(context, pow(0.5, age / CapturedHistory.usageHalfLife))
        }, nil, nil, nil)
        // Lets a query skip smcli's own commands the way `isSmcliCommand` does everywhere else.
        sqlite3_create_function_v2(db, "smcli_internal", 1, SQLITE_UTF8 | SQLITE_DETERMINISTIC, nil, { context, _, arguments in
            let command = sqlite3_value_text(arguments![0]).map { String(cString: $0) } ?? ""
            sqlite3_result_int(context, isSmcliCommand(command) ? 1 : 0)
        }, nil, nil, nil)
    }

    deinit { sqlite3_close(db) }

    /// Changes whenever another connection commits, so a long-lived reader knows its cache is stale.
    package func dataVersion() throws -> Int {
        var version = 0
        try run("PRAGMA data_version") { version = Int(sqlite3_column_int64($0, 0)) }
        return version
    }

    /// Rows changed by the most recent INSERT, UPDATE, or DELETE.
    package var changes: Int { Int(sqlite3_changes(db)) }

    package func execute(_ sql: String) throws {
        try check(sqlite3_exec(db, sql, nil, nil, nil))
    }

    package func run(_ sql: String, _ values: [SQLValue] = [], row: ((OpaquePointer) throws -> Void)? = nil) throws {
        var statement: OpaquePointer?
        try check(sqlite3_prepare_v2(db, sql, -1, &statement, nil))
        defer { sqlite3_finalize(statement) }
        let transient = unsafeBitCast(OpaquePointer(bitPattern: -1), to: sqlite3_destructor_type.self)
        for (offset, value) in values.enumerated() {
            let index = Int32(offset + 1)
            switch value {
            case .text(let text): try check(sqlite3_bind_text(statement, index, text, -1, transient))
            case .integer(let number): try check(sqlite3_bind_int64(statement, index, Int64(number)))
            case .real(let number): try check(sqlite3_bind_double(statement, index, number))
            case .null: try check(sqlite3_bind_null(statement, index))
            }
        }
        while true {
            let code = sqlite3_step(statement)
            guard code == SQLITE_ROW else { try check(code); return }
            try row?(statement!)
        }
    }

    private func check(_ code: Int32) throws {
        guard code == SQLITE_OK || code == SQLITE_ROW || code == SQLITE_DONE else {
            throw HistoryError(message: "History database error: \(String(cString: sqlite3_errmsg(db)))")
        }
    }
}

private func text(_ statement: OpaquePointer, _ column: Int32) -> String {
    sqlite3_column_text(statement, column).map { String(cString: $0) } ?? ""
}

private func optionalText(_ statement: OpaquePointer, _ column: Int32) -> String? {
    sqlite3_column_type(statement, column) == SQLITE_NULL ? nil : text(statement, column)
}

package struct CapturedHistory {
    package let url: URL

    package init(url: URL) { self.url = url }

    package static let schema = """
        CREATE TABLE sessions (
            id TEXT PRIMARY KEY, user TEXT NOT NULL, host TEXT NOT NULL, shell TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE commands (
            id INTEGER PRIMARY KEY, session TEXT NOT NULL REFERENCES sessions(id),
            started_at REAL NOT NULL, duration_ms INTEGER, directory TEXT NOT NULL,
            command TEXT NOT NULL, exit_code INTEGER NOT NULL
        );
        CREATE INDEX commands_by_command ON commands(command, started_at);
        CREATE INDEX commands_by_directory ON commands(directory, started_at);
        """ + migrationToV2 + migrationToV3 + migrationToV5 + migrationToV6 + migrationToV7

    /// Fields borrowed from Atuin's history table: a UUID for deduplication, soft deletion, and authorship.
    package static let migrationToV2 = """
        ALTER TABLE commands ADD COLUMN uuid TEXT;
        ALTER TABLE commands ADD COLUMN deleted_at REAL;
        ALTER TABLE commands ADD COLUMN author TEXT;
        ALTER TABLE commands ADD COLUMN intent TEXT;
        CREATE UNIQUE INDEX commands_by_uuid ON commands(uuid);
        PRAGMA user_version = 2;
        """
    /// Option names, space-separated (empty when none), so flags can be read without re-parsing.
    /// Existing rows are filled in by `backfillFlags`.
    package static let migrationToV3 = """
        ALTER TABLE commands ADD COLUMN flags TEXT;
        PRAGMA user_version = 3;
        """
    /// Space-separated exit status of each pipeline stage; NULL when the shell doesn't report it.
    package static let migrationToV5 = """
        ALTER TABLE commands ADD COLUMN pipestatus TEXT;
        PRAGMA user_version = 5;
        """
    /// Files the user explicitly picked in the file or grep picker (never inferred from command text), for frecency.
    /// `path` is relative to `root`; `query` is what was typed.
    package static let migrationToV6 = """
        CREATE TABLE file_selections (
            id INTEGER PRIMARY KEY, root TEXT NOT NULL, path TEXT NOT NULL, mode TEXT NOT NULL,
            query TEXT NOT NULL, selected_at REAL NOT NULL
        );
        CREATE INDEX file_selections_by_root ON file_selections(root, path);
        PRAGMA user_version = 6;
        """
    /// Commands the user explicitly picked in the picker (from suggestions, history or search results), with what was
    /// typed at the time, so a similar query later puts that command first. `kind` is `suggest` or `search`.
    package static let migrationToV7 = """
        CREATE TABLE command_selections (
            id INTEGER PRIMARY KEY, query TEXT NOT NULL, command TEXT NOT NULL, kind TEXT NOT NULL,
            directory TEXT NOT NULL, selected_at REAL NOT NULL
        );
        PRAGMA user_version = 7;
        """
    package static let version = 7
    /// A short-lived build stored usage scores in a `usage` table at version 4. Usage is now computed when read,
    /// so such databases are taken back to version 3, which every binary since flags can open.
    /// Real schema changes therefore start at version 5.
    package static let discardedUsageVersion = 4
    /// Usage halves every two weeks, so habits outweigh one-offs without old habits winning forever.
    package static let usageHalfLife = 14.0 * 86_400

    /// Exit codes that do not mean the command failed: stopped with Ctrl-C (130) or by a closed pipe (141).
    package static let interruptedExitCodes = [130, 141]
    /// Programs whose exit code 1 is an answer ("no match", "files differ", "false"), not an error.
    package static let answeringPrograms: Set<String> = ["grep", "egrep", "fgrep", "rg", "ag", "ack", "pgrep", "diff", "cmp",
                                                 "colordiff", "test", "[", "which", "type", "command", "false"]

    package static func answersWithExitCode(_ command: String) -> Bool {
        let program = CommandBreakdown.parse(command)?.mainCommand
            ?? command.split(whereSeparator: \.isWhitespace).first.map(String.init)
        return program.map { answeringPrograms.contains(String($0.split(separator: "/").last ?? Substring($0))) } ?? false
    }

    package static func atuinLocation(home: URL, environment: [String: String]) -> URL {
        if let path = environment["ATUIN_DB_PATH"], !path.isEmpty { return URL(fileURLWithPath: path) }
        if let data = environment["XDG_DATA_HOME"], !data.isEmpty {
            return URL(fileURLWithPath: data).appendingPathComponent("atuin/history.db")
        }
        return home.appendingPathComponent(".local/share/atuin/history.db")
    }

    package static func location(home: URL, environment: [String: String]) -> URL {
        if let path = environment["SMCLI_DB"], !path.isEmpty { return URL(fileURLWithPath: path) }
        return home.appendingPathComponent(".local/share/smcli/history.db")
    }

    /// Records one finished command. Times are Unix epoch seconds supplied by the shell hook;
    /// without them the start is the save time and the duration is unknown.
    /// `event` is the caller's ID for this record (stored as `uuid`): retrying with the same ID never adds a
    /// second row. Returns false when that ID was already stored. `db` reuses an open connection.
    @discardableResult
    package func append(command: String, shell: String, directory: String, exitCode: Int, nativeFile: String,
                        session: String = "unknown", startedAt: Double? = nil, endedAt: Double? = nil,
                        pipestatus: [Int]? = nil, event: String = UUID().uuidString.lowercased(), on db: SQLiteDatabase? = nil) throws -> Bool {
        guard ["bash", "zsh", "fish"].contains(shell), (0...255).contains(exitCode),
              directory.hasPrefix("/"), !command.isEmpty, !session.isEmpty,
              (pipestatus ?? []).count <= 256, (pipestatus ?? []).allSatisfy({ (0...255).contains($0) }) else {
            throw HistoryError(message: "Invalid shell, directory, exit code, pipestatus, session, or empty command.")
        }
        if !nativeFile.isEmpty && url.resolvingSymlinksInPath() == URL(fileURLWithPath: nativeFile).resolvingSymlinksInPath() {
            throw HistoryError(message: "SMCLI_DB must be separate from the shell's native HISTFILE.")
        }
        let start = startedAt ?? Date().timeIntervalSince1970
        let duration = endedAt.map { max(0, Int((($0 - start) * 1000).rounded())) }
        guard Self.isEventID(event) else { throw HistoryError(message: "Invalid event ID.") }
        let db = try db ?? connect(write: true)
        if !nativeFile.isEmpty {
            var logInfo = stat()
            var nativeInfo = stat()
            if stat(url.path, &logInfo) == 0 && stat(nativeFile, &nativeInfo) == 0,
               logInfo.st_dev == nativeInfo.st_dev && logInfo.st_ino == nativeInfo.st_ino {
                throw HistoryError(message: "SMCLI_DB must not be a hard link to native HISTFILE.")
            }
        }
        return try insert(into: db, session: session, user: NSUserName(), host: ProcessInfo.processInfo.hostName,
                          shell: shell, startedAt: start, durationMs: duration, directory: directory, command: command,
                          exitCode: exitCode, uuid: event, pipestatus: pipestatus)
    }

    /// Records one explicit file selection.
    package func recordSelection(root: String, path: String, mode: String, query: String,
                                 at time: Double = Date().timeIntervalSince1970, on db: SQLiteDatabase) throws {
        try db.run("INSERT INTO file_selections (root, path, mode, query, selected_at) VALUES (?, ?, ?, ?, ?)",
                   [.text(root), .text(path), .text(mode), .text(query), .real(time)])
    }

    /// Selection count per path under `root`, decayed like command usage, so recent picks outrank old ones.
    package func selectionScores(root: String, now: Double = Date().timeIntervalSince1970,
                                 on db: SQLiteDatabase) throws -> [String: Double] {
        var scores: [String: Double] = [:]
        try db.run("SELECT path, sum(smcli_decay(? - selected_at)) FROM file_selections WHERE root = ? GROUP BY path",
                   [.real(now), .text(root)]) { scores[String(cString: sqlite3_column_text($0, 0))] = sqlite3_column_double($0, 1) }
        return scores
    }

    /// Records that the user picked `command` after typing `query` (empty for the unfiltered list).
    package func recordPick(query: String, command: String, kind: String, directory: String,
                            at time: Double = Date().timeIntervalSince1970, on db: SQLiteDatabase) throws {
        try db.run("INSERT INTO command_selections (query, command, kind, directory, selected_at) VALUES (?, ?, ?, ?, ?)",
                   [.text(query), .text(command), .text(kind), .text(directory), .real(time)])
    }

    /// The most recent picks, oldest first; older ones have faded anyway.
    package func picks(limit: Int = 5_000, on db: SQLiteDatabase) throws -> SelectionLog {
        var rows: [SelectionLog.Pick] = []
        try db.run("SELECT query, command, selected_at FROM command_selections ORDER BY id DESC LIMIT ?", [.integer(limit)]) {
            rows.append(.init(query: SelectionLog.normalize(text($0, 0)), command: text($0, 1), time: sqlite3_column_double($0, 2)))
        }
        var log = SelectionLog()
        for pick in rows.reversed() { log.add(query: pick.query, command: pick.command, at: pick.time) }
        return log
    }

    /// The last command `session` ran (not counting smcli's own), for transitions.
    package func previousCommand(session: String, user: String, on db: SQLiteDatabase) throws -> String? {
        var command: String?
        try db.run("""
            SELECT c.command FROM commands c JOIN sessions s ON s.id = c.session
            WHERE c.session = ? AND s.user = ? AND c.deleted_at IS NULL AND NOT smcli_internal(c.command)
            ORDER BY c.started_at DESC, c.id DESC LIMIT 1
            """, [.text(session), .text(user)]) { command = text($0, 0) }
        return command
    }

    /// How often each command directly followed `previous` within one session.
    package func followers(after previous: String, user: String, on db: SQLiteDatabase) throws -> [String: Int] {
        var counts: [String: Int] = [:]
        try db.run("""
            SELECT command, count(*) FROM (
                SELECT c.command AS command,
                       LAG(c.command) OVER (PARTITION BY c.session ORDER BY c.started_at, c.id) AS prev
                FROM commands c JOIN sessions s ON s.id = c.session
                WHERE s.user = ? AND c.deleted_at IS NULL AND NOT smcli_internal(c.command)
            ) WHERE prev = ? GROUP BY command
            """, [.text(user), .text(previous)]) { counts[text($0, 0)] = Int(sqlite3_column_int64($0, 1)) }
        return counts
    }

    /// Parses a shell's `pipestatus` words ("0 1 0"). Empty means not reported.
    package static func parsePipestatus(_ text: String) throws -> [Int]? {
        let words = text.split(whereSeparator: \.isWhitespace)
        if words.isEmpty { return nil }
        let codes = words.compactMap { Int($0) }
        guard codes.count == words.count, codes.count <= 256, codes.allSatisfy({ (0...255).contains($0) }) else {
            throw HistoryError(message: "Invalid pipestatus: \(text)")
        }
        return codes
    }

    /// Event IDs are lowercase UUIDs, so a client can't collide with Atuin's IDs or smuggle in arbitrary text.
    package static func isEventID(_ text: String) -> Bool {
        text.utf8.count == 36 && UUID(uuidString: text) != nil && text == text.lowercased()
    }

    /// One captured run, as the in-memory suggestion index needs it.
    package struct Run: Equatable, Sendable {
        package let command: String
        package let directory: String
        package let startedAt: Double
        package let exitCode: Int
        /// The shell session, whose runs form the command transitions.
        package let session: String

        package init(command: String, directory: String, startedAt: Double, exitCode: Int, session: String = "") {
            self.command = command
            self.directory = directory
            self.startedAt = startedAt
            self.exitCode = exitCode
            self.session = session
        }
    }

    /// `user`'s live runs in the order `suggestions` reads them.
    package func runs(user: String, on db: SQLiteDatabase) throws -> [Run] {
        var runs: [Run] = []
        try db.run("""
            SELECT c.command, c.directory, c.started_at, c.exit_code, c.session
            FROM commands c JOIN sessions s ON s.id = c.session
            WHERE s.user = ? AND c.deleted_at IS NULL
            ORDER BY c.started_at, c.id
            """, [.text(user)]) { row in
            runs.append(Run(command: text(row, 0), directory: text(row, 1), startedAt: sqlite3_column_double(row, 2),
                            exitCode: Int(sqlite3_column_int64(row, 3)), session: text(row, 4)))
        }
        return runs
    }

    /// A long-lived writable connection, for the service.
    package func openConnection() throws -> SQLiteDatabase { try connect(write: true) }

    /// Imports Atuin's history.db, keeping its UUIDs so repeated imports skip rows already present.
    /// Atuin stores times as Unix nanoseconds, uses -1 for unknown durations, and `user:host` hostnames.
    /// With `replace`, existing smcli history is deleted in the same transaction, so a failed import keeps it.
    /// Returns the number of new records.
    package func importAtuin(from file: URL, replace: Bool = false) throws -> Int {
        guard FileManager.default.fileExists(atPath: file.path), let resolved = realpath(file.path, nil) else {
            throw HistoryError(message: "No Atuin database at \(file.path)")
        }
        let source = try SQLiteDatabase(path: String(cString: resolved), readOnly: true)
        free(resolved)
        var columns: Set<String> = []
        try source.run("PRAGMA table_info(history)") { columns.insert(text($0, 1)) }
        guard columns.isSuperset(of: ["id", "timestamp", "duration", "exit", "command", "cwd", "session", "hostname"]) else {
            throw HistoryError(message: "Not an Atuin history database: \(file.path)")
        }
        // Newer Atuin versions add these; older databases lack them.
        let optional = ["deleted_at", "shell", "author", "intent"].map { columns.contains($0) ? $0 : "NULL" }
        let db = try connect(write: true)
        var imported = 0
        try db.execute("BEGIN IMMEDIATE")
        do {
            if replace { try db.execute("DELETE FROM commands; DELETE FROM sessions; DELETE FROM command_selections;") }
            try source.run("""
                SELECT id, timestamp, duration, exit, command, cwd, session, hostname, \(optional.joined(separator: ", "))
                FROM history ORDER BY timestamp
                """) { row in
                let hostname = text(row, 7)
                let (host, user) = hostname.firstIndex(of: ":").map {
                    (String(hostname[..<$0]), String(hostname[hostname.index(after: $0)...]))
                } ?? (hostname, "unknown")
                let duration = sqlite3_column_int64(row, 2)
                let deletedAt = sqlite3_column_type(row, 8) == SQLITE_NULL ? nil : Double(sqlite3_column_int64(row, 8)) / 1e9
                let shell = optionalText(row, 9).flatMap { $0.isEmpty ? nil : $0 } ?? "unknown"
                try insert(into: db, session: "atuin-\(text(row, 6))", user: user.isEmpty ? "unknown" : user,
                           host: host.isEmpty ? "unknown" : host, shell: shell,
                           startedAt: Double(sqlite3_column_int64(row, 1)) / 1e9,
                           durationMs: duration < 0 ? nil : Int(duration / 1_000_000),
                           directory: text(row, 5), command: text(row, 4), exitCode: Int(sqlite3_column_int64(row, 3)),
                           uuid: text(row, 0), deletedAt: deletedAt, author: optionalText(row, 10),
                           intent: optionalText(row, 11), transaction: false)
                imported += db.changes
            }
            try db.execute("COMMIT")
        } catch {
            try? db.execute("ROLLBACK")
            throw error
        }
        if replace {
            // Reclaim the old rows' pages so replaced history is not left on disk.
            try db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
            try db.execute("VACUUM")
        }
        return imported
    }

    /// Imports records from the JSONL log written by earlier smcli versions. Returns the number imported.
    package func importLegacy(from file: URL) throws -> Int {
        struct Legacy: Decodable { let timestamp, user, directory, shell, command: String; let exitCode: Int }
        let lines = try Data(contentsOf: file).split(separator: 0x0A)
        let legacy = try lines.map { try JSONDecoder().decode(Legacy.self, from: Data($0)) }
        let formatter = ISO8601DateFormatter()
        let db = try connect(write: true)
        try db.execute("BEGIN IMMEDIATE")
        do {
            for record in legacy {
                guard let date = formatter.date(from: record.timestamp) else {
                    throw HistoryError(message: "Invalid timestamp in \(file.path): \(record.timestamp)")
                }
                try insert(into: db, session: "imported-\(record.user)-\(record.shell)", user: record.user, host: "unknown",
                           shell: record.shell, startedAt: date.timeIntervalSince1970, durationMs: nil,
                           directory: record.directory, command: record.command, exitCode: record.exitCode, transaction: false)
            }
            try db.execute("COMMIT")
        } catch {
            try? db.execute("ROLLBACK")
            throw error
        }
        return legacy.count
    }

    /// Permanently deletes stored commands the filter would now reject (like `atuin history prune`).
    /// With `dryRun`, only reports them. Returns the matching commands in time order.
    package func prune(filter: CaptureFilter, dryRun: Bool) throws -> [String] {
        guard FileManager.default.fileExists(atPath: url.path) else { return [] }
        let db = try connect(write: !dryRun)
        var ids: [Int] = []
        var commands: [String] = []
        try db.run("SELECT id, command, directory FROM commands ORDER BY started_at, id") { row in
            if !filter.allows(command: text(row, 1), directory: text(row, 2)) {
                ids.append(Int(sqlite3_column_int64(row, 0)))
                commands.append(text(row, 1))
            }
        }
        guard !dryRun, !ids.isEmpty else { return commands }
        try db.execute("BEGIN IMMEDIATE")
        do {
            for id in ids { try db.run("DELETE FROM commands WHERE id = ?", [.integer(id)]) }
            // A pick keeps the command's text too.
            for command in Set(commands) { try db.run("DELETE FROM command_selections WHERE command = ?", [.text(command)]) }
            try db.execute("COMMIT")
        } catch {
            try? db.execute("ROLLBACK")
            throw error
        }
        // Deleted text can linger in free pages and the WAL until they are rewritten.
        try db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        try db.execute("VACUUM")
        return commands
    }

    package func records() throws -> [CapturedCommand] {
        guard FileManager.default.fileExists(atPath: url.path) else { return [] }
        let db = try connect(write: false)
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        var records: [CapturedCommand] = []
        try db.run("""
            SELECT c.started_at, c.duration_ms, s.user, s.host, s.id, s.shell, c.directory, c.command, c.exit_code,
                   COUNT(*) OVER (PARTITION BY c.command ORDER BY c.started_at, c.id), c.uuid, c.author, c.intent,
                   c.pipestatus
            FROM commands c JOIN sessions s ON s.id = c.session
            WHERE c.deleted_at IS NULL
            ORDER BY c.started_at, c.id
            """) { row in
            let command = text(row, 7)
            records.append(CapturedCommand(
                id: optionalText(row, 10),
                timestamp: formatter.string(from: Date(timeIntervalSince1970: sqlite3_column_double(row, 0))),
                durationMs: sqlite3_column_type(row, 1) == SQLITE_NULL ? nil : Int(sqlite3_column_int64(row, 1)),
                user: text(row, 2), host: text(row, 3), session: text(row, 4), directory: text(row, 6), shell: text(row, 5),
                command: command, exitCode: Int(sqlite3_column_int64(row, 8)),
                pipestatus: optionalText(row, 13).map { $0.split(separator: " ").compactMap { Int($0) } },
                author: optionalText(row, 11), intent: optionalText(row, 12), runCount: Int(sqlite3_column_int64(row, 9)),
                breakdown: CommandBreakdown.parse(command)
            ))
        }
        return records
    }

    /// One row per distinct command: run count, its flags, and the most recent run's time, directory, exit code,
    /// author, and intent, plus usage aggregates from the same grouped query; `directory` scopes `runsHere`.
    package func commandStats(directory: String? = nil, project: String? = nil,
                              now: Double = Date().timeIntervalSince1970) throws -> [String: CommandStats] {
        guard FileManager.default.fileExists(atPath: url.path) else { return [:] }
        let db = try connect(write: false)
        var stats: [String: CommandStats] = [:]
        let here = directory.map { $0.count > 1 && $0.hasSuffix("/") ? String($0.dropLast()) : $0 } ?? ""
        let root = project ?? ""
        // SQLite returns the bare columns from the row holding MAX(started_at).
        try db.run("""
            SELECT c.command, COUNT(*), MAX(c.started_at), c.directory, c.exit_code, c.author, c.intent, c.flags,
                   SUM(smcli_decay(? - c.started_at)), SUM(c.exit_code IN (0, \(Self.interruptedExitCodes.map(String.init).joined(separator: ", ")))),
                   SUM(c.exit_code = 1),
                   SUM(? <> '' AND (c.directory = ? OR (? <> '/' AND substr(c.directory, 1, length(?) + 1) = ? || '/'))),
                   SUM(? <> '' AND (c.directory = ? OR (? <> '/' AND substr(c.directory, 1, length(?) + 1) = ? || '/')))
            FROM commands c
            WHERE c.deleted_at IS NULL GROUP BY c.command
            """, [.real(now)] + [here, here, here, here, here, root, root, root, root, root].map(SQLValue.text)) { row in
            let command = text(row, 0)
            let answers = sqlite3_column_int64(row, 10) > 0 && Self.answersWithExitCode(command)
            stats[command] = CommandStats(
                runs: Int(sqlite3_column_int64(row, 1)), lastRun: sqlite3_column_double(row, 2),
                lastDirectory: text(row, 3), lastExitCode: Int(sqlite3_column_int64(row, 4)),
                author: optionalText(row, 5), intent: optionalText(row, 6),
                flags: text(row, 7).split(separator: " ").map(String.init),
                frecency: sqlite3_column_double(row, 8),
                successes: Int(sqlite3_column_int64(row, 9) + (answers ? sqlite3_column_int64(row, 10) : 0)),
                runsHere: Int(sqlite3_column_int64(row, 11)), runsProject: Int(sqlite3_column_int64(row, 12)))
        }
        return stats
    }

    /// Ranks `user`'s distinct commands for `directory`: each run counts more when it is recent, ran in this
    /// directory (or in the same project, or above/below it), and succeeded. Only commands starting with `prefix`
    /// are considered. `context` adds the signals of the moment (see `applyRankingSignals`); picks apply always.
    /// `SuggestionIndex` computes the same ranking from memory.
    package func suggestions(user: String, directory: String, prefix: String = "", limit: Int = 10,
                     now: Double = Date().timeIntervalSince1970, context: RankingContext? = nil) throws -> [Suggestion] {
        guard FileManager.default.fileExists(atPath: url.path) else { return [] }
        let db = try connect(write: false)
        let here = directory.count > 1 && directory.hasSuffix("/") ? String(directory.dropLast()) : directory
        struct Tally { var score = 0.0, runs = 0, runsHere = 0, projectRuns = 0, lastRun = 0.0, lastExitCode = 0 }
        var tallies: [String: Tally] = [:]
        var answering: [String: Bool] = [:]
        try db.run("""
            SELECT c.command, c.directory, c.started_at, c.exit_code
            FROM commands c JOIN sessions s ON s.id = c.session
            WHERE s.user = ? AND c.deleted_at IS NULL AND substr(c.command, 1, length(?)) = ?
            ORDER BY c.started_at, c.id
            """, [.text(user), .text(prefix), .text(prefix)]) { row in
            guard !isSmcliCommand(text(row, 0)) else { return }
            let runDirectory = text(row, 1)
            let started = sqlite3_column_double(row, 2)
            let exitCode = Int(sqlite3_column_int64(row, 3))
            let inProject = context?.inProject(runDirectory) ?? false
            // Halves every two weeks, so habits outweigh one-offs without old habits winning forever.
            var weight = pow(0.5, max(0, now - started) / Self.usageHalfLife)
            if runDirectory == here {
                weight *= 4
            } else if inProject {
                weight *= 3
            } else if here != "/" && runDirectory != "/" &&
                        (runDirectory.hasPrefix(here + "/") || here.hasPrefix(runDirectory + "/")) {
                weight *= 2
            }
            if exitCode != 0 && !Self.interruptedExitCodes.contains(exitCode) {
                let command = text(row, 0)
                let answers = answering[command] ?? Self.answersWithExitCode(command)
                answering[command] = answers
                if !(exitCode == 1 && answers) { weight *= 0.25 }
            }
            var tally = tallies[text(row, 0), default: Tally()]
            tally.score += weight
            tally.runs += 1
            if runDirectory == here { tally.runsHere += 1 }
            if inProject { tally.projectRuns += 1 }
            tally.lastRun = started
            tally.lastExitCode = exitCode
            tallies[text(row, 0)] = tally
        }
        var scores = tallies.mapValues(\.score)
        var previous: String?
        if let session = context?.session, context?.uses(.transition) ?? true {
            previous = try previousCommand(session: session, user: user, on: db)
        }
        let why = applyRankingSignals(
            to: &scores, prefix: prefix, context: context, previous: previous,
            followers: try previous.map { try followers(after: $0, user: user, on: db) } ?? [:],
            picks: try picks(on: db).weights(query: prefix, now: now))
        let formatter = ISO8601DateFormatter()
        return scores
            .sorted {
                if $0.value != $1.value { return $0.value > $1.value }
                let (left, right) = (tallies[$0.key]?.lastRun ?? -.infinity, tallies[$1.key]?.lastRun ?? -.infinity)
                if left != right { return left > right }
                return $0.key < $1.key
            }
            .prefix(limit)
            .map { command, score in
                guard let tally = tallies[command] else {
                    return Suggestion(command: command, score: score, runs: 0, runsHere: 0, lastRun: "", lastExitCode: 0,
                                      why: suggestionWhy(runsHere: 0, projectRuns: 0, extra: why[command]))
                }
                return Suggestion(command: command, score: score, runs: tally.runs, runsHere: tally.runsHere,
                                  lastRun: formatter.string(from: Date(timeIntervalSince1970: tally.lastRun)),
                                  lastExitCode: tally.lastExitCode,
                                  why: suggestionWhy(runsHere: tally.runsHere, projectRuns: tally.projectRuns, extra: why[command]))
            }
    }

    @discardableResult
    private func insert(into db: SQLiteDatabase, session: String, user: String, host: String, shell: String,
                        startedAt: Double, durationMs: Int?, directory: String, command: String, exitCode: Int,
                        uuid: String? = nil, deletedAt: Double? = nil, author: String? = nil, intent: String? = nil,
                        pipestatus: [Int]? = nil, transaction: Bool = true) throws -> Bool {
        // One transaction means one WAL commit per command.
        if transaction { try db.execute("BEGIN IMMEDIATE") }
        do {
            try db.run("INSERT OR IGNORE INTO sessions (id, user, host, shell) VALUES (?, ?, ?, ?)",
                       [.text(session), .text(user), .text(host), .text(shell)])
            try db.run("""
                INSERT OR IGNORE INTO commands
                    (session, started_at, duration_ms, directory, command, exit_code, uuid, deleted_at, author, intent, flags,
                     pipestatus)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """, [.text(session), .real(startedAt), durationMs.map(SQLValue.integer) ?? .null,
                      .text(directory), .text(command), .integer(exitCode), uuid.map(SQLValue.text) ?? .null,
                      deletedAt.map(SQLValue.real) ?? .null, author.map(SQLValue.text) ?? .null,
                      intent.map(SQLValue.text) ?? .null, .text(commandFlags(command).joined(separator: " ")),
                      pipestatus.map { SQLValue.text($0.map(String.init).joined(separator: " ")) } ?? .null])
            // OR IGNORE skips a row whose uuid is already stored.
            let inserted = db.changes > 0
            if transaction { try db.execute("COMMIT") }
            return inserted
        } catch {
            if transaction { try? db.execute("ROLLBACK") }
            throw error
        }
    }

    /// Fills the flags column for rows written before it existed. Runs inside the migration's transaction.
    private func backfillFlags(_ db: SQLiteDatabase) throws {
        var rows: [(Int, String)] = []
        try db.run("SELECT id, command FROM commands WHERE flags IS NULL") { rows.append((Int(sqlite3_column_int64($0, 0)), text($0, 1))) }
        for (id, command) in rows {
            try db.run("UPDATE commands SET flags = ? WHERE id = ?", [.text(commandFlags(command).joined(separator: " ")), .integer(id)])
        }
    }

    private func connect(write: Bool) throws -> SQLiteDatabase {
        if write {
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true,
                                                    attributes: [.posixPermissions: 0o700])
            // Create the file ourselves so it is private from the start; SQLite gives its WAL files the same mode.
            let fd = open(url.path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, mode_t(0o600))
            if fd >= 0 { close(fd) } else if errno != EEXIST { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        }
        var info = stat()
        guard lstat(url.path, &info) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        guard info.st_mode & mode_t(S_IFMT) == mode_t(S_IFREG), info.st_uid == getuid(), info.st_mode & 0o077 == 0 else {
            throw HistoryError(message: "History database must be a regular file owned by you with private permissions (chmod 600): \(url.path)")
        }
        // SQLite's NOFOLLOW refuses symlinks anywhere in the path (e.g. macOS /var), so resolve parent directories;
        // the file itself must still not be a symlink.
        // Foundation's resolvingSymlinksInPath maps /private/var back to /var, so use realpath.
        guard let parent = realpath(url.deletingLastPathComponent().path, nil) else {
            throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
        }
        defer { free(parent) }
        let path = URL(fileURLWithPath: String(cString: parent)).appendingPathComponent(url.lastPathComponent).path
        let db = try SQLiteDatabase(path: path, readOnly: !write)
        var version = 0
        try db.run("PRAGMA user_version") { version = Int(sqlite3_column_int64($0, 0)) }
        if version > Self.version && version != Self.discardedUsageVersion { throw HistoryError(message: "History database was written by a newer smcli: \(url.path)") }
        if !write && version != 0 && version != Self.version {
            // Readers need the current columns; upgrade once through a writable connection.
            return try connect(write: true)
        }
        if write {
            if version != Self.version {
                try db.execute("BEGIN IMMEDIATE")
                // Another terminal may have created or upgraded the schema while this one waited for the lock.
                var tables = 0
                try db.run("SELECT count(*) FROM sqlite_master") { tables = Int(sqlite3_column_int64($0, 0)) }
                try db.run("PRAGMA user_version") { version = Int(sqlite3_column_int64($0, 0)) }
                if tables == 0 {
                    try db.execute(Self.schema)
                } else {
                    if version == 1 { try db.execute(Self.migrationToV2) }
                    if version <= 2 {
                        try db.execute(Self.migrationToV3)
                        try backfillFlags(db)
                    }
                    if version == Self.discardedUsageVersion {
                        try db.execute("DROP TABLE IF EXISTS usage; PRAGMA user_version = 3;")
                    }
                    if version < 5 { try db.execute(Self.migrationToV5) }
                    if version < 6 { try db.execute(Self.migrationToV6) }
                    if version < 7 { try db.execute(Self.migrationToV7) }
                }
                try db.execute("COMMIT")
                try db.execute("PRAGMA journal_mode = WAL")
            }
            // WAL with NORMAL sync avoids an fsync per command; a power loss can drop only the last few records.
            try db.execute("PRAGMA synchronous = NORMAL")
        } else if version == 0 {
            throw HistoryError(message: "Not an smcli history database: \(url.path)")
        }
        return db
    }
}
