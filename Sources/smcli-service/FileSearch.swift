import Foundation
import SmcliCore
import SmcliFFF
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif

/// `files`, `grep` and `select` of a file: fff searches over one instance per root, with the root and Git worktree taken
/// from the directory context. Runs on the caller's connection thread, never on the state queue.
final class FileSearch: @unchecked Sendable {
    let pool: FFFPool
    private let contexts: ContextCache
    /// Decayed selection counts per path for a root; reads the history database on the service's state queue.
    private let scores: @Sendable (String) -> [String: Double]
    private let record: @Sendable (_ root: String, _ path: String, _ mode: String, _ query: String) throws -> Void

    init(pool: FFFPool, contexts: ContextCache,
         scores: @escaping @Sendable (String) -> [String: Double],
         record: @escaping @Sendable (String, String, String, String) throws -> Void) {
        (self.pool, self.contexts, self.scores, self.record) = (pool, contexts, scores, record)
    }

    func files(_ request: ServiceRequest) -> ServiceResponse { search(request, grep: false) }
    func grep(_ request: ServiceRequest) -> ServiceResponse { search(request, grep: true) }

    /// The directory's Git worktree, or the directory itself outside Git. Real paths, so symlinks don't split instances.
    func root(of directory: String) -> String? {
        if let root = contexts.context(for: directory).repository?.root { return root }
        guard let resolved = realpath(directory, nil) else { return nil }
        defer { free(resolved) }
        return String(cString: resolved)
    }

    private func search(_ request: ServiceRequest, grep: Bool) -> ServiceResponse {
        let name = grep ? "grep" : "files"
        guard let directory = request.directory, directory.hasPrefix("/"), let query = request.query else {
            return .failure("bad_request", "\(name) needs a query and an absolute directory.")
        }
        let limit = request.limit ?? 50
        guard (1...500).contains(limit) else { return .failure("bad_request", "limit must be 1 to 500.") }
        if grep && query.isEmpty { return .failure("bad_request", "grep needs a query.") }
        guard let root = root(of: directory) else { return .failure("failed", "Cannot resolve \(directory).") }
        do {
            let (index, id) = try pool.index(for: root)
            let picked = scores(root)
            var hits: [FileHit]
            if grep {
                hits = try index.grep(query, limit: limit).map {
                    FileHit(path: $0.path, insert: Self.relative($0.path, root: root, from: directory),
                            provenance: ["grep"] + (picked[$0.path] != nil ? ["picked"] : []), git: Self.git($0.git),
                            line: $0.line, column: $0.column, text: $0.text)
                }
            } else {
                // Take extra candidates so files picked before can move up. Exact name matches stay first, and
                // only files scoring close to the best are lifted, so a favourite never beats a better match.
                let found = try index.search(query, limit: min(500, limit * 3))
                let best = Double(found.first?.score ?? 0)
                func key(_ entry: (offset: Int, element: FFFFile)) -> Double {
                    let pickedScore = min(3, picked[entry.element.path] ?? 0)
                    let eligible = !entry.element.exact && Double(entry.element.score) >= 0.8 * best
                    return Double(entry.offset) - (eligible ? 5 * pickedScore : 0) - (entry.element.exact ? 1000 : 0)
                }
                let ranked = found.enumerated().sorted { key($0) != key($1) ? key($0) < key($1) : $0.offset < $1.offset }
                hits = ranked.prefix(limit).map {
                    FileHit(path: $0.element.path, insert: Self.relative($0.element.path, root: root, from: directory),
                            provenance: ["files"] + (picked[$0.element.path] != nil ? ["picked"] : []),
                            git: Self.git($0.element.git))
                }
            }
            var response = ServiceResponse(ok: true)
            (response.files, response.root, response.instance) = (hits, root, id)
            return response
        } catch {
            return .failure("failed", error.localizedDescription)
        }
    }

    /// Records an explicit choice. The path must stay inside the root; fff and the picker only ever send relative ones.
    func select(_ request: ServiceRequest) -> ServiceResponse {
        guard let directory = request.directory, directory.hasPrefix("/"), let path = request.path,
              let mode = request.mode, ["files", "grep"].contains(mode) else {
            return .failure("bad_request", "select needs a directory, a path and a mode of files or grep.")
        }
        let parts = path.split(separator: "/", omittingEmptySubsequences: false)
        guard !path.isEmpty, !path.hasPrefix("/"), path.utf8.count <= 4096, !path.utf8.contains(0),
              !parts.contains(where: { $0 == ".." || $0.isEmpty }) else {
            return .failure("bad_request", "select needs a path relative to the searched root.")
        }
        guard let root = root(of: directory) else { return .failure("failed", "Cannot resolve \(directory).") }
        do {
            try record(root, path, mode, String((request.query ?? "").prefix(1000)))
            var response = ServiceResponse(ok: true)
            response.root = root
            return response
        } catch {
            return .failure("failed", error.localizedDescription)
        }
    }

    /// fff names the state ("modified", "staged_new"...); shown like `git status --short`: index column, then work tree.
    /// Clean files and anything outside a repository have none.
    static func git(_ status: String?) -> String? {
        switch status {
        case "modified": return " M"
        case "untracked": return "??"
        case "deleted": return " D"
        case "renamed": return " R"
        case "staged_new": return "A "
        case "staged_modified": return "M "
        case "staged_deleted": return "D "
        case "ignored": return "!!"
        case nil, "clean": return nil
        default: return "~~"  // conflicted or type-changed: fff has no name for it
        }
    }

    /// `path` (under `root`) as seen from `directory`: `a.swift` from its own folder, `../x/b.swift` from a sibling.
    static func relative(_ path: String, root: String, from directory: String) -> String {
        let real = realpath(directory, nil).map { defer { free($0) }; return String(cString: $0) } ?? directory
        let rootParts = root.split(separator: "/").map(String.init)
        let here = real.split(separator: "/").map(String.init)
        // A directory outside the root can't be expressed relative to it; the root-relative path is the best we have.
        guard here.starts(with: rootParts) else { return path }
        let down = Array(here.dropFirst(rootParts.count))
        let target = path.split(separator: "/", omittingEmptySubsequences: false).map(String.init)
        var common = 0
        while common < down.count, common < target.count - 1, down[common] == target[common] { common += 1 }
        return (Array(repeating: "..", count: down.count - common) + target.dropFirst(common)).joined(separator: "/")
    }
}
