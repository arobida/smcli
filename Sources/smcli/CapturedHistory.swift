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

struct CommandFlag: Codable, Equatable {
    let name: String
    let value: String?
}

struct CommandBreakdown: Codable, Equatable {
    let mainCommand: String
    let subcommand: String?
    let flags: [CommandFlag]
    let targets: [String]
    let arguments: [String]

    // shortcut: common tools/options only; use complete tool schemas for semantic parsing.
    static let tools: Set<String> = ["git", "docker", "npm", "cargo", "swift", "kubectl", "brew", "smcli"]
    static let valueOptions: [String: Set<String>] = [
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

    static func parse(_ command: String) -> CommandBreakdown? {
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
func commandFlags(_ command: String) -> [String] {
    if let breakdown = CommandBreakdown.parse(command) { return breakdown.flags.map(\.name) }
    return command.split(whereSeparator: \.isWhitespace).map(String.init).filter { $0.hasPrefix("-") && $0 != "-" && $0 != "--" }
        .map { $0.split(separator: "=", maxSplits: 1).first.map(String.init) ?? $0 }
}

// Tokenize without executing or expanding anything. Compound shell syntax is deliberately not guessed.
func shellWords(_ command: String) -> [String]? {
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

/// A captured command as presented to readers. `runCount` and `breakdown` are derived at read time,
/// so they reflect the whole log and the current parser rather than whatever was true when written.
struct CapturedCommand: Codable, Equatable {
    /// Stable UUID, shared with Atuin for imported rows so re-imports deduplicate.
    let id: String?
    let timestamp: String
    let durationMs: Int?
    let user: String
    let host: String
    let session: String
    let directory: String
    let shell: String
    let command: String
    let exitCode: Int
    /// Who wrote the command (a person or an agent), and why; both optional, as in Atuin.
    let author: String?
    let intent: String?
    let runCount: Int
    let breakdown: CommandBreakdown?
}

struct CommandStats: Equatable {
    let runs: Int
    let lastRun: Double
    let lastDirectory: String
    let lastExitCode: Int
    var author: String? = nil
    var intent: String? = nil
    var flags: [String] = []
}

struct Suggestion: Codable, Equatable {
    let command: String
    let score: Double
    let runs: Int
    /// Runs in exactly the requested directory.
    let runsHere: Int
    let lastRun: String
    let lastExitCode: Int
}

struct HistoryError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

enum SQLValue {
    case text(String), integer(Int), real(Double), null
}

final class SQLiteDatabase {
    private let db: OpaquePointer

    init(path: String, readOnly: Bool) throws {
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
    }

    deinit { sqlite3_close(db) }

    /// Rows changed by the most recent INSERT, UPDATE, or DELETE.
    var changes: Int { Int(sqlite3_changes(db)) }

    func execute(_ sql: String) throws {
        try check(sqlite3_exec(db, sql, nil, nil, nil))
    }

    func run(_ sql: String, _ values: [SQLValue] = [], row: ((OpaquePointer) throws -> Void)? = nil) throws {
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

struct CapturedHistory {
    let url: URL

    static let schema = """
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
        """ + migrationToV2 + migrationToV3

    /// Fields borrowed from Atuin's history table: a UUID for deduplication, soft deletion, and authorship.
    static let migrationToV2 = """
        ALTER TABLE commands ADD COLUMN uuid TEXT;
        ALTER TABLE commands ADD COLUMN deleted_at REAL;
        ALTER TABLE commands ADD COLUMN author TEXT;
        ALTER TABLE commands ADD COLUMN intent TEXT;
        CREATE UNIQUE INDEX commands_by_uuid ON commands(uuid);
        PRAGMA user_version = 2;
        """
    /// Option names, space-separated (empty when none), so flags can be read without re-parsing.
    /// Existing rows are filled in by `backfillFlags`.
    static let migrationToV3 = """
        ALTER TABLE commands ADD COLUMN flags TEXT;
        PRAGMA user_version = 3;
        """
    static let version = 3

    static func atuinLocation(home: URL, environment: [String: String]) -> URL {
        if let path = environment["ATUIN_DB_PATH"], !path.isEmpty { return URL(fileURLWithPath: path) }
        if let data = environment["XDG_DATA_HOME"], !data.isEmpty {
            return URL(fileURLWithPath: data).appendingPathComponent("atuin/history.db")
        }
        return home.appendingPathComponent(".local/share/atuin/history.db")
    }

    static func location(home: URL, environment: [String: String]) -> URL {
        if let path = environment["SMCLI_DB"], !path.isEmpty { return URL(fileURLWithPath: path) }
        return home.appendingPathComponent(".local/share/smcli/history.db")
    }

    /// Records one finished command. Times are Unix epoch seconds supplied by the shell hook;
    /// without them the start is the save time and the duration is unknown.
    func append(command: String, shell: String, directory: String, exitCode: Int, nativeFile: String,
                session: String = "unknown", startedAt: Double? = nil, endedAt: Double? = nil) throws {
        guard ["bash", "zsh"].contains(shell), (0...255).contains(exitCode),
              directory.hasPrefix("/"), !command.isEmpty, !session.isEmpty else {
            throw HistoryError(message: "Invalid shell, directory, exit code, session, or empty command.")
        }
        if !nativeFile.isEmpty && url.resolvingSymlinksInPath() == URL(fileURLWithPath: nativeFile).resolvingSymlinksInPath() {
            throw HistoryError(message: "SMCLI_DB must be separate from the shell's native HISTFILE.")
        }
        let start = startedAt ?? Date().timeIntervalSince1970
        let duration = endedAt.map { max(0, Int((($0 - start) * 1000).rounded())) }
        let db = try connect(write: true)
        if !nativeFile.isEmpty {
            var logInfo = stat()
            var nativeInfo = stat()
            if stat(url.path, &logInfo) == 0 && stat(nativeFile, &nativeInfo) == 0,
               logInfo.st_dev == nativeInfo.st_dev && logInfo.st_ino == nativeInfo.st_ino {
                throw HistoryError(message: "SMCLI_DB must not be a hard link to native HISTFILE.")
            }
        }
        try insert(into: db, session: session, user: NSUserName(), host: ProcessInfo.processInfo.hostName, shell: shell,
                   startedAt: start, durationMs: duration, directory: directory, command: command, exitCode: exitCode,
                   uuid: UUID().uuidString.lowercased())
    }

    /// Imports Atuin's history.db, keeping its UUIDs so repeated imports skip rows already present.
    /// Atuin stores times as Unix nanoseconds, uses -1 for unknown durations, and `user:host` hostnames.
    /// With `replace`, existing smcli history is deleted in the same transaction, so a failed import keeps it.
    /// Returns the number of new records.
    func importAtuin(from file: URL, replace: Bool = false) throws -> Int {
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
            if replace { try db.execute("DELETE FROM commands; DELETE FROM sessions;") }
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
    func importLegacy(from file: URL) throws -> Int {
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
    func prune(filter: CaptureFilter, dryRun: Bool) throws -> [String] {
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

    func records() throws -> [CapturedCommand] {
        guard FileManager.default.fileExists(atPath: url.path) else { return [] }
        let db = try connect(write: false)
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        var records: [CapturedCommand] = []
        try db.run("""
            SELECT c.started_at, c.duration_ms, s.user, s.host, s.id, s.shell, c.directory, c.command, c.exit_code,
                   COUNT(*) OVER (PARTITION BY c.command ORDER BY c.started_at, c.id), c.uuid, c.author, c.intent
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
                author: optionalText(row, 11), intent: optionalText(row, 12), runCount: Int(sqlite3_column_int64(row, 9)),
                breakdown: CommandBreakdown.parse(command)
            ))
        }
        return records
    }

    /// One row per distinct command: run count, its flags, and the most recent run's time, directory, exit code, author, and intent.
    func commandStats() throws -> [String: CommandStats] {
        guard FileManager.default.fileExists(atPath: url.path) else { return [:] }
        let db = try connect(write: false)
        var stats: [String: CommandStats] = [:]
        // SQLite returns the bare columns from the row holding MAX(started_at).
        try db.run("""
            SELECT command, COUNT(*), MAX(started_at), directory, exit_code, author, intent, flags
            FROM commands WHERE deleted_at IS NULL GROUP BY command
            """) { row in
            stats[text(row, 0)] = CommandStats(
                runs: Int(sqlite3_column_int64(row, 1)), lastRun: sqlite3_column_double(row, 2),
                lastDirectory: text(row, 3), lastExitCode: Int(sqlite3_column_int64(row, 4)),
                author: optionalText(row, 5), intent: optionalText(row, 6),
                flags: text(row, 7).split(separator: " ").map(String.init))
        }
        return stats
    }

    /// Ranks `user`'s distinct commands for `directory`: each run counts more when it is recent, ran in this
    /// directory (or above/below it), and succeeded. Only commands starting with `prefix` are considered.
    func suggestions(user: String, directory: String, prefix: String = "", limit: Int = 10,
                     now: Double = Date().timeIntervalSince1970) throws -> [Suggestion] {
        guard FileManager.default.fileExists(atPath: url.path) else { return [] }
        let db = try connect(write: false)
        let here = directory.count > 1 && directory.hasSuffix("/") ? String(directory.dropLast()) : directory
        struct Tally { var score = 0.0, runs = 0, runsHere = 0, lastRun = 0.0, lastExitCode = 0 }
        var tallies: [String: Tally] = [:]
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
            // Halves every two weeks, so habits outweigh one-offs without old habits winning forever.
            var weight = pow(0.5, max(0, now - started) / (14 * 86_400))
            if runDirectory == here {
                weight *= 4
            } else if here != "/" && runDirectory != "/" &&
                        (runDirectory.hasPrefix(here + "/") || here.hasPrefix(runDirectory + "/")) {
                weight *= 2
            }
            if exitCode != 0 { weight *= 0.25 }
            var tally = tallies[text(row, 0), default: Tally()]
            tally.score += weight
            tally.runs += 1
            if runDirectory == here { tally.runsHere += 1 }
            tally.lastRun = started
            tally.lastExitCode = exitCode
            tallies[text(row, 0)] = tally
        }
        let formatter = ISO8601DateFormatter()
        return tallies
            .sorted {
                if $0.value.score != $1.value.score { return $0.value.score > $1.value.score }
                if $0.value.lastRun != $1.value.lastRun { return $0.value.lastRun > $1.value.lastRun }
                return $0.key < $1.key
            }
            .prefix(limit)
            .map { Suggestion(command: $0.key, score: $0.value.score, runs: $0.value.runs, runsHere: $0.value.runsHere,
                              lastRun: formatter.string(from: Date(timeIntervalSince1970: $0.value.lastRun)),
                              lastExitCode: $0.value.lastExitCode) }
    }

    private func insert(into db: SQLiteDatabase, session: String, user: String, host: String, shell: String,
                        startedAt: Double, durationMs: Int?, directory: String, command: String, exitCode: Int,
                        uuid: String? = nil, deletedAt: Double? = nil, author: String? = nil, intent: String? = nil,
                        transaction: Bool = true) throws {
        // One transaction means one WAL commit per command.
        if transaction { try db.execute("BEGIN IMMEDIATE") }
        do {
            try db.run("INSERT OR IGNORE INTO sessions (id, user, host, shell) VALUES (?, ?, ?, ?)",
                       [.text(session), .text(user), .text(host), .text(shell)])
            try db.run("""
                INSERT OR IGNORE INTO commands
                    (session, started_at, duration_ms, directory, command, exit_code, uuid, deleted_at, author, intent, flags)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """, [.text(session), .real(startedAt), durationMs.map(SQLValue.integer) ?? .null,
                      .text(directory), .text(command), .integer(exitCode), uuid.map(SQLValue.text) ?? .null,
                      deletedAt.map(SQLValue.real) ?? .null, author.map(SQLValue.text) ?? .null,
                      intent.map(SQLValue.text) ?? .null, .text(commandFlags(command).joined(separator: " "))])
            if transaction { try db.execute("COMMIT") }
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
        if version > Self.version { throw HistoryError(message: "History database was written by a newer smcli: \(url.path)") }
        if !write && version != 0 && version < Self.version {
            // Readers need the current columns; upgrade once through a writable connection.
            return try connect(write: true)
        }
        if write {
            if version < Self.version {
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
