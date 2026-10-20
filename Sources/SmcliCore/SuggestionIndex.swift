import Foundation

/// `CapturedHistory.suggestions` kept in memory and updated per record, so a query costs the number of distinct
/// commands (plus those run in related directories) rather than a scan of every run.
///
/// The score is the same sum: each run weighs 0.5^(age / half-life), ×4 in the query directory, ×3 elsewhere in its
/// project, ×2 above or below it, ×0.25 when it failed. Transitions (what follows what in a session) and picks are
/// kept here too, and `applyRankingSignals` is shared with the scan. The decay factors out of the sum (0.5^((now − t)/h) = 2^((t − base)/h) · 2^((base − now)/h)),
/// so tallies store 2^((t − base)/h) and a query multiplies by one factor. The directory bonus is added as
/// (weight − 1) × that directory's tally. One difference: a run stamped in the future counts slightly more than 1
/// here until its time passes, where the scan clamps its age to zero.
package struct SuggestionIndex {
    private struct Tally {
        var score = 0.0, runs = 0, lastRun = -Double.infinity, lastExitCode = 0
    }

    private struct DirectoryTally {
        var score = 0.0, runs = 0
    }

    private let halfLife = CapturedHistory.usageHalfLife
    /// Reference time for stored weights; moved forward before 2^((t − base)/h) could overflow.
    private var base: Double
    private var commands: [String: Tally] = [:]
    private var directories: [String: [String: DirectoryTally]] = [:]
    private var answering: [String: Bool] = [:]
    private var history: [CapturedHistory.Run] = []
    private var affinity = CommandAffinity()
    package private(set) var picks: SelectionLog

    package init(runs: [CapturedHistory.Run], picks: SelectionLog = SelectionLog()) {
        base = runs.last?.startedAt ?? Date().timeIntervalSince1970
        self.picks = picks
        for run in runs { add(run) }
    }

    /// Remembers that `command` was picked for `query`, as `recordPick` stored it.
    package mutating func addPick(query: String, command: String, at time: Double) {
        picks.add(query: query, command: command, at: time)
    }

    /// Adds one run. Runs are expected in (started_at, id) order; an older one still counts, but doesn't
    /// replace the last-run fields.
    package mutating func add(_ run: CapturedHistory.Run) {
        guard !isSmcliCommand(run.command) else { return }
        history.append(run)
        affinity.add(session: run.session, command: run.command)
        tally(run)
    }

    private mutating func tally(_ run: CapturedHistory.Run) {
        if (run.startedAt - base) / halfLife > 500 {
            base = run.startedAt
            commands = [:]
            directories = [:]
            for old in history { tally(old) }
            return
        }
        var weight = pow(2, (run.startedAt - base) / halfLife)
        if run.exitCode != 0 && !CapturedHistory.interruptedExitCodes.contains(run.exitCode) {
            let answers = answering[run.command] ?? CapturedHistory.answersWithExitCode(run.command)
            answering[run.command] = answers
            if !(run.exitCode == 1 && answers) { weight *= 0.25 }
        }
        commands[run.command, default: Tally()].score += weight
        commands[run.command]!.runs += 1
        if run.startedAt >= commands[run.command]!.lastRun {
            commands[run.command]!.lastRun = run.startedAt
            commands[run.command]!.lastExitCode = run.exitCode
        }
        directories[run.directory, default: [:]][run.command, default: DirectoryTally()].score += weight
        directories[run.directory]![run.command]!.runs += 1
    }

    package func suggestions(directory: String, prefix: String = "", limit: Int = 10,
                             now: Double = Date().timeIntervalSince1970, context: RankingContext? = nil) -> [Suggestion] {
        let here = directory.count > 1 && directory.hasSuffix("/") ? String(directory.dropLast()) : directory
        let scale = pow(2, (base - now) / halfLife)
        var scores: [String: Double] = [:]
        for (command, tally) in commands where hasPrefix(command, prefix) {
            scores[command] = tally.score
        }
        var projectRuns: [String: Int] = [:]
        for (runDirectory, tallies) in directories {
            let bonus: Double
            let inProject = context?.inProject(runDirectory) ?? false
            if runDirectory == here {
                bonus = 3
            } else if inProject {
                bonus = 2
            } else if here != "/" && runDirectory != "/" &&
                        (runDirectory.hasPrefix(here + "/") || here.hasPrefix(runDirectory + "/")) {
                bonus = 1
            } else {
                continue
            }
            for (command, tally) in tallies where scores[command] != nil {
                scores[command]! += bonus * tally.score
                if inProject { projectRuns[command, default: 0] += tally.runs }
            }
        }
        // Scores are stored relative to `base`; the signals work on real scores.
        for (command, score) in scores { scores[command] = score * scale }
        var previous: String?
        if let session = context?.session, context?.uses(.transition) ?? true { previous = affinity.last[session] }
        let why = applyRankingSignals(to: &scores, prefix: prefix, context: context, previous: previous,
                                      followers: previous.map(affinity.following) ?? [:],
                                      picks: picks.weights(query: prefix, now: now))
        let formatter = ISO8601DateFormatter()
        return scores
            .sorted {
                if $0.value != $1.value { return $0.value > $1.value }
                let (left, right) = (commands[$0.key]?.lastRun ?? -.infinity, commands[$1.key]?.lastRun ?? -.infinity)
                if left != right { return left > right }
                return $0.key < $1.key
            }
            .prefix(limit)
            .map { command, score in
                let runsHere = directories[here]?[command]?.runs ?? 0
                guard let tally = commands[command] else {
                    return Suggestion(command: command, score: score, runs: 0, runsHere: 0, lastRun: "", lastExitCode: 0,
                                      why: suggestionWhy(runsHere: 0, projectRuns: 0, extra: why[command]))
                }
                return Suggestion(command: command, score: score, runs: tally.runs, runsHere: runsHere,
                                  lastRun: formatter.string(from: Date(timeIntervalSince1970: tally.lastRun)),
                                  lastExitCode: tally.lastExitCode,
                                  why: suggestionWhy(runsHere: runsHere, projectRuns: projectRuns[command] ?? 0, extra: why[command]))
            }
    }

    /// The last command a session ran, which transitions out of the session's next command key on.
    package func previousCommand(session: String) -> String? { affinity.last[session] }

    /// How often each command directly followed `previous` in a session.
    package func following(_ previous: String) -> [String: Int] { affinity.following(previous) }

    /// Every distinct command, most recently run first (ties by command), as the picker lists full history.
    package func recent() -> [String] {
        commands.sorted { $0.value.lastRun != $1.value.lastRun ? $0.value.lastRun > $1.value.lastRun : $0.key < $1.key }
            .map(\.key)
    }
}
