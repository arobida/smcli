import Foundation

/// A distinct command with whatever is known about its runs.
package struct SearchEntry: Codable, Equatable {
    package let command: String
    /// Runs recorded by capture hooks, or occurrences in native history when never captured.
    package let runs: Int
    package let lastRun: String?
    package let lastDirectory: String?
    package let lastExitCode: Int?
    /// "captured" when metadata exists, otherwise "native".
    package let source: String
    /// From the most recent captured run, when recorded.
    package var author: String? = nil
    package var intent: String? = nil
    /// Option names (`--amend`, `-m`), from the database for captured commands.
    package var flags: [String] = []
    /// Runs decayed by age (half weight every two weeks), successful runs, and runs in the searcher's directory or below (captured only).
    package var frecency: Double = 0
    package var successes: Int = 0
    package var runsHere: Int = 0
    /// Runs in the searcher's project (nearest manifest directory) or below it; includes `runsHere` when that is inside it.
    package var runsProject: Int = 0
    /// Why search ranked this command where it did; set only on search results.
    package var why: [String]? = nil

    package init(command: String, runs: Int, lastRun: String?, lastDirectory: String?, lastExitCode: Int?, source: String,
                 author: String? = nil, intent: String? = nil, flags: [String] = [], frecency: Double = 0,
                 successes: Int = 0, runsHere: Int = 0, runsProject: Int = 0, why: [String]? = nil) {
        (self.command, self.runs, self.lastRun, self.lastDirectory, self.lastExitCode, self.source) =
            (command, runs, lastRun, lastDirectory, lastExitCode, source)
        (self.author, self.intent, self.flags, self.frecency, self.successes, self.runsHere, self.runsProject, self.why) =
            (author, intent, flags, frecency, successes, runsHere, runsProject, why)
    }
}

extension SearchEntry {
    /// The executable's name without its path (`git` for `/usr/bin/git git status`); for compound
    /// shell lines the parser refuses, the first word.
    package var program: String? { Self.program(of: command, parsed: CommandBreakdown.parse(command)) }

    /// `program` from an existing parse, for callers that also need other parts of it.
    package static func program(of command: String, parsed: CommandBreakdown?) -> String? {
        if let main = parsed?.mainCommand { return main.split(separator: "/").last.map(String.init) ?? main }
        return command.split(whereSeparator: \.isWhitespace).first.map { String($0.split(separator: "/").last ?? $0) }
    }
    package var subcommand: String? { CommandBreakdown.parse(command)?.subcommand }
    package var lastRunEpoch: Double? { lastRun.flatMap { ISO8601DateFormatter().date(from: $0)?.timeIntervalSince1970 } }
}

/// Hard filters applied to search results. Time bounds apply to a command's most recent run,
/// so commands from native history (which has no times) never pass them.
package struct SearchFilter: Codable, Equatable {
    package var program: String?
    package var subcommand: String?
    /// Unix epoch seconds; `since` is inclusive, `before` exclusive.
    package var since: Double?
    package var before: Double?
    package var minRuns: Int?
    package var author: String?
    /// The last run's directory is this one or below it.
    package var directory: String?
    /// The command uses this option, by exact name (`-m` does not match `--message`).
    package var flag: String?

    package init(program: String? = nil, subcommand: String? = nil, since: Double? = nil, before: Double? = nil,
                 minRuns: Int? = nil, author: String? = nil, directory: String? = nil, flag: String? = nil) {
        (self.program, self.subcommand, self.since, self.before) = (program, subcommand, since, before)
        (self.minRuns, self.author, self.directory, self.flag) = (minRuns, author, directory, flag)
    }

    package var isEmpty: Bool { self == SearchFilter() }

    package func allows(_ entry: SearchEntry) -> Bool {
        if let program, entry.program != program { return false }
        if let subcommand, entry.subcommand != subcommand { return false }
        if let minRuns, entry.runs < minRuns { return false }
        if let author, entry.author != author { return false }
        if let flag, !entry.flags.contains(flag) { return false }
        if let directory {
            let base = directory.count > 1 && directory.hasSuffix("/") ? String(directory.dropLast()) : directory
            guard let last = entry.lastDirectory,
                  last == base || last.hasPrefix(base == "/" ? "/" : base + "/") else { return false }
        }
        if since != nil || before != nil {
            guard let last = entry.lastRunEpoch else { return false }
            if let since, last < since { return false }
            if let before, last >= before { return false }
        }
        return true
    }

    /// Parses `30m`, `12h`, `7d`, `2w` (that long before `now`), a local `YYYY-MM-DD` date, or an ISO 8601 time.
    package static func time(_ text: String, now: Double) throws -> Double {
        let units: [Character: Double] = ["m": 60, "h": 3_600, "d": 86_400, "w": 604_800]
        if let unit = text.last.flatMap({ units[$0] }), let count = Double(text.dropLast()), count >= 0 {
            return now - count * unit
        }
        let day = DateFormatter()
        day.locale = Locale(identifier: "en_US_POSIX")
        day.dateFormat = "yyyy-MM-dd"
        if let date = day.date(from: text) ?? ISO8601DateFormatter().date(from: text) { return date.timeIntervalSince1970 }
        throw HistoryError(message: "Invalid time \(text); use 30m, 12h, 7d, 2w, YYYY-MM-DD, or an ISO 8601 time.")
    }
}

