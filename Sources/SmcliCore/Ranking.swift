import Foundation

/// Ranking evidence beyond a command's own runs: where the query comes from (project, session) and what the
/// project itself offers (scripts, Git state). Built per query from the directory context, never stored.
package struct RankingContext: Equatable, Sendable {
    /// The signals that can be switched off, for measuring each one (`SMCLI_RANKING_OFF=project,transition`).
    package enum Signal: String, CaseIterable, Sendable { case project, transition, selection, script, hint }

    /// A command the project suggests whether or not it was ever run.
    package struct Candidate: Equatable, Sendable {
        package enum Kind: String, Sendable { case script, hint }
        package var command: String
        package var kind: Kind
        /// Shown in `why`: `npm script`, `rebase in progress`.
        package var reason: String

        package init(command: String, kind: Kind, reason: String) { (self.command, self.kind, self.reason) = (command, kind, reason) }
    }

    /// The nearest manifest's directory: runs below it count as the same project.
    package var projectRoot: String?
    /// The shell session asking, whose previous command selects transitions.
    package var session: String?
    package var candidates: [Candidate] = []
    package var off: Set<Signal> = []

    package init(projectRoot: String? = nil, session: String? = nil, candidates: [Candidate] = [], off: Set<Signal> = []) {
        (self.projectRoot, self.session, self.candidates, self.off) = (projectRoot, session, candidates, off)
    }

    /// From directory context; `off` names come from the environment.
    package init(project: ProjectContext, session: String?, environment: [String: String] = [:]) {
        self.init(projectRoot: project.projects.first?.root, session: session, candidates: project.candidates(),
                  off: Self.disabled(environment))
    }

    package static func disabled(_ environment: [String: String]) -> Set<Signal> {
        Set((environment["SMCLI_RANKING_OFF"] ?? "").split(separator: ",").compactMap { Signal(rawValue: String($0)) })
    }

    package func uses(_ signal: Signal) -> Bool { !off.contains(signal) }

    package func inProject(_ directory: String) -> Bool {
        guard uses(.project), let projectRoot else { return false }
        return directory == projectRoot || (projectRoot != "/" && directory.hasPrefix(projectRoot + "/"))
    }

    package func candidates(_ kind: Candidate.Kind) -> [Candidate] {
        guard uses(kind == .script ? .script : .hint) else { return [] }
        return candidates.filter { $0.kind == kind }
    }
}

extension ProjectContext {
    /// Commands the directory's project and Git state point to. Only names present in a manifest are produced
    /// (read as text by `discover`; nothing is executed), and only plain identifiers, so a hostile `package.json`
    /// cannot put shell syntax on someone's command line.
    package func candidates() -> [RankingContext.Candidate] {
        var found: [RankingContext.Candidate] = []
        if let operation = repository?.operation {
            let steps: [String: [String]] = [
                "rebase": ["continue", "abort", "skip"], "merge": ["continue", "abort"], "cherry-pick": ["continue", "abort", "skip"],
                "revert": ["continue", "abort", "skip"], "am": ["continue", "abort", "skip"],
            ]
            let verbs = steps[operation].map { $0.map { "--" + $0 } } ?? (operation == "bisect" ? ["good", "bad", "reset"] : [])
            for verb in verbs {
                found.append(.init(command: "git \(operation) \(verb)", kind: .hint, reason: "\(operation) in progress"))
            }
        }
        guard let project = projects.first else { return found }
        func valid(_ name: String) -> Bool {
            guard let first = name.unicodeScalars.first, first.properties.isAlphabetic || ("0"..."9").contains(first) || first == "_" else { return false }
            return name.unicodeScalars.allSatisfy { $0.isASCII && ($0.properties.isAlphabetic || ("0"..."9").contains($0) || "_.:@/-".unicodeScalars.contains($0)) }
        }
        let manager = project.packageManager ?? ""
        for (file, names) in project.scripts.sorted(by: { $0.key < $1.key }) {
            let lower = file.lowercased()
            for name in names where valid(name) {
                switch (lower, project.kind) {
                case ("package.json", _):
                    // Checked again here: a cached or hand-built context may carry any text.
                    let manager = ProjectContext.Project.nodeManagers.contains(manager) ? manager : ""
                    let command = manager == "yarn" ? "yarn \(name)" : "\(manager.isEmpty ? "npm" : manager) run \(name)"
                    found.append(.init(command: command, kind: .script, reason: "package.json script"))
                    if manager.isEmpty || manager == "npm", name == "test" || name == "start" {
                        found.append(.init(command: "npm \(name)", kind: .script, reason: "package.json script"))
                    }
                case ("makefile", _), ("gnumakefile", _):
                    found.append(.init(command: "make \(name)", kind: .script, reason: "Makefile target"))
                case ("cargo.toml", _):
                    found.append(.init(command: "cargo run --bin \(name)", kind: .script, reason: "Cargo binary"))
                case ("package.swift", _):
                    if !name.hasSuffix("Tests") { found.append(.init(command: "swift build --target \(name)", kind: .script, reason: "Package.swift target")) }
                case ("pyproject.toml", _):
                    if ["uv", "poetry", "pdm", "pipenv"].contains(manager) {
                        found.append(.init(command: "\(manager) run \(name)", kind: .script, reason: "pyproject script"))
                    }
                default: break
                }
            }
        }
        return found
    }
}

/// What the user typed when they picked a command, kept to boost that command for similar queries.
package struct SelectionLog: Equatable, Sendable {
    package struct Pick: Equatable, Sendable {
        package var query: String
        package var command: String
        package var time: Double
    }

    package private(set) var picks: [Pick] = []

    package init(picks: [Pick] = []) { for pick in picks { add(query: pick.query, command: pick.command, at: pick.time) } }

    package mutating func add(query: String, command: String, at time: Double) {
        picks.append(Pick(query: Self.normalize(query), command: command, time: time))
    }

    package static func normalize(_ query: String) -> String {
        query.lowercased().split(whereSeparator: \.isWhitespace).joined(separator: " ")
    }

    /// 1 for the same query, 0.6 when the current one is the start of the picked one (`doc` while typing toward
    /// `docker`), 0.25 when it extends the picked one (a more specific search than the pick answered), and half the
    /// word overlap when several words are shared (`start containers` and `start the containers`).
    /// An empty query only matches an empty one.
    package static func similarity(_ a: String, _ b: String) -> Double {
        if a == b { return 1 }
        if a.isEmpty || b.isEmpty { return 0 }
        // `a` is the current query, `b` the one the pick was made for.
        if a.count >= 2 && b.hasPrefix(a) { return 0.6 }
        if b.count >= 2 && a.hasPrefix(b) { return 0.25 }
        let left = Set(a.split(separator: " ")), right = Set(b.split(separator: " "))
        guard left.count > 1, right.count > 1 else { return 0 }
        let overlap = Double(left.intersection(right).count) / Double(left.union(right).count)
        return overlap >= 0.5 ? 0.5 * overlap : 0
    }

    /// Per command: picks for similar queries, each weighing 0.5^(age / half-life) × similarity.
    package func weights(query: String, now: Double) -> [String: Double] {
        let query = Self.normalize(query)
        var weights: [String: Double] = [:]
        for pick in picks {
            let similarity = Self.similarity(query, pick.query)
            guard similarity > 0 else { continue }
            weights[pick.command, default: 0] += similarity * pow(0.5, max(0, now - pick.time) / CapturedHistory.usageHalfLife)
        }
        return weights
    }
}

/// Which command follows which within a shell session, as plain counts. The previous command is the session's
/// last one that wasn't smcli itself.
package struct CommandAffinity: Equatable, Sendable {
    package private(set) var followers: [String: [String: Int]] = [:]
    package private(set) var last: [String: String] = [:]

    package init() {}

    package mutating func add(session: String, command: String) {
        if let previous = last[session] { followers[previous, default: [:]][command, default: 0] += 1 }
        last[session] = command
    }

    package func following(_ previous: String) -> [String: Int] { followers[previous] ?? [:] }
}

/// Applies the context signals to base scores (usage, recency, directory, project) and returns why each moved
/// (only the commands a signal touched).
/// Shared by the database scan and the in-memory index, so the two cannot drift apart.
///
/// - A command that often follows the session's previous one is multiplied by up to 7 (its share of that
///   command's followers, discounted until it has been seen three times).
/// - A command picked for a similar query is multiplied by up to 5 (each recent pick of the same query counts 2).
/// - A project script already in history is multiplied by 1.5; one that never ran enters at 0.3, below any habit.
/// - A Git-state hint is lifted just above the best score, so the way out of a rebase is the first suggestion.
package func applyRankingSignals(to scores: inout [String: Double], prefix: String, context: RankingContext?,
                                 previous: String?, followers: [String: Int], picks: [String: Double]) -> [String: [String]] {
    var why: [String: [String]] = [:]
    let scripts = context?.candidates(.script) ?? []
    for script in scripts where hasPrefix(script.command, prefix) {
        if scores[script.command] != nil {
            scores[script.command]! *= 1.5
        } else {
            scores[script.command] = 0.3
        }
        why[script.command, default: []].append(script.reason)
    }
    if let previous, context?.uses(.transition) ?? true, !followers.isEmpty {
        let total = Double(followers.values.reduce(0, +))
        for (command, count) in followers where scores[command] != nil {
            let share = Double(count) / total
            scores[command]! *= 1 + 6 * share * min(1, Double(count) / 3)
            why[command, default: []].append("often follows \(previous)")
        }
    }
    if context?.uses(.selection) ?? true {
        for (command, weight) in picks where scores[command] != nil {
            scores[command]! *= 1 + min(4, 2 * weight)
            why[command, default: []].append("picked for similar searches")
        }
    }
    let hints = (context?.candidates(.hint) ?? []).filter { hasPrefix($0.command, prefix) }
    if !hints.isEmpty {
        let best = max(1, scores.filter { entry in !hints.contains { $0.command == entry.key } }.values.max() ?? 0)
        for (index, hint) in hints.enumerated() {
            scores[hint.command] = max(scores[hint.command] ?? 0, best * (1.1 - 0.01 * Double(index)))
            why[hint.command, default: []].append(hint.reason)
        }
    }
    return why
}

/// SQLite's `substr(command, 1, length(prefix)) = prefix` compares code points, not Swift's graphemes.
package func hasPrefix(_ command: String, _ prefix: String) -> Bool {
    prefix.isEmpty || command.unicodeScalars.starts(with: prefix.unicodeScalars)
}

/// The `why` of a suggestion: where it was used, then what the context signals found.
package func suggestionWhy(runsHere: Int, projectRuns: Int, extra: [String]?) -> [String]? {
    var lines: [String] = []
    if runsHere > 0 { lines.append("used here") } else if projectRuns > 0 { lines.append("used in this project") }
    lines += extra ?? []
    return lines.isEmpty ? nil : lines
}
