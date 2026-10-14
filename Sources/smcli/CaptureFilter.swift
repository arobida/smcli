import Foundation

/// Decides which commands are never stored, following Atuin's capture filters:
/// a leading space (the shell convention for "keep this out of history"), likely secrets,
/// and user regexes over the command (`historyFilter`) or its directory (`cwdFilter`).
struct CaptureFilter {
    let historyFilter: [NSRegularExpression]
    let cwdFilter: [NSRegularExpression]
    let secretsFilter: Bool

    /// Token shapes adapted from Atuin's default secrets filter.
    static let secretPatterns: [NSRegularExpression] = [
        "AKIA[0-9A-Z]{16}",                          // AWS access key ID
        "gh[pousr]_[A-Za-z0-9]{36}",                 // GitHub tokens
        "github_pat_[A-Za-z0-9_]{82}",               // GitHub fine-grained PAT
        "glpat-[A-Za-z0-9_-]{20}",                   // GitLab PAT
        "xox[abposr]-[A-Za-z0-9-]{10,}",             // Slack tokens
        "https://hooks\\.slack\\.com/services/\\S+", // Slack webhook
        "[sr]k_live_[A-Za-z0-9]{24,}",               // Stripe live keys
        "AIza[0-9A-Za-z_-]{35}",                     // Google API key
        "npm_[A-Za-z0-9]{36}",                       // npm token
        "sk-(ant|proj)-[A-Za-z0-9_-]{20,}",          // Anthropic and OpenAI keys
    ].map { try! NSRegularExpression(pattern: $0) }

    init(config: SmcliConfig) throws {
        func compile(_ patterns: [String]?, _ name: String) throws -> [NSRegularExpression] {
            try (patterns ?? []).map {
                do { return try NSRegularExpression(pattern: $0) } catch {
                    throw HistoryError(message: "Invalid \(name) pattern: \($0)")
                }
            }
        }
        historyFilter = try compile(config.historyFilter, "historyFilter")
        cwdFilter = try compile(config.cwdFilter, "cwdFilter")
        secretsFilter = config.secretsFilter ?? true
    }

    func allows(command: String, directory: String) -> Bool {
        if command.first == " " { return false }
        func matches(_ patterns: [NSRegularExpression], _ text: String) -> Bool {
            patterns.contains { $0.firstMatch(in: text, range: NSRange(text.startIndex..., in: text)) != nil }
        }
        if secretsFilter && matches(Self.secretPatterns, command) { return false }
        return !matches(historyFilter, command) && !matches(cwdFilter, directory)
    }
}
