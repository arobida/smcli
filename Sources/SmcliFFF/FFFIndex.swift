import CFFF
import Foundation

package struct FFFError: LocalizedError {
    package let message: String
    package var errorDescription: String? { message }
}

/// A file found by name.
package struct FFFFile: Equatable, Sendable {
    /// Relative to the indexed root.
    package let path: String
    /// fff's Git state by name (`modified`, `untracked`, `staged_new`, `clean`...); nil outside a repository.
    /// (The header's two-letter examples are wrong for v0.11.0.)
    package let git: String?
    /// fff's relevance score and whether the query equals the name, for callers that re-rank.
    package let score: Int
    package let exact: Bool
}

/// A line found by content search.
/// One page of grep matches. `complete` is false when the time budget or the limit stopped the search before every
/// file was read, so more matches may exist.
package struct FFFGrep: Equatable, Sendable {
    package let matches: [FFFMatch]
    package let complete: Bool
}

package struct FFFMatch: Equatable, Sendable {
    package let path: String
    package let line: Int
    /// 1-based byte column of the match.
    package let column: Int
    package let text: String
    package let git: String?
}

/// Owns one fff instance for one root. Every fff pointer and borrowed string stays inside this type: strings
/// are copied into Swift values before the payload that owns them is freed (ownership table in docs/baseline.md).
/// fff is used serially, so calls take `lock`; `deinit` destroys the instance (and its watcher), so an instance
/// evicted while a request still holds it lives until that request ends.
package final class FFFIndex: @unchecked Sendable {
    package let root: String
    /// False when the initial scan outlasted `scanTimeout`: results are partial until it finishes.
    package let scanned: Bool
    private let handle: UnsafeMutableRawPointer
    private let lock = NSLock()

    /// No LMDB stores: frecency lives in smcli's own database. The watcher keeps the index current.
    package init(root: String, watch: Bool = true, scanTimeout: UInt64 = 10_000) throws {
        guard !root.utf8.contains(0) else { throw FFFError(message: "NUL in root") }
        self.root = root
        let created = try root.withCString { path in
            var options = FffCreateOptions()
            options.version = UInt32(FFF_CREATE_OPTIONS_VERSION)
            options.base_path = path
            options.watch = watch
            // fff copies option strings during creation, before withCString returns.
            return try Self.result(fff_create_instance_with(&options)) { result in
                guard let handle = result.handle else { throw FFFError(message: "Missing instance") }
                return handle
            }
        }
        // A slow scan is not an error: search what is indexed so far.
        let finished = (try? Self.result(fff_wait_for_scan(created, scanTimeout)) { $0.int_value == 1 }) ?? false
        scanned = finished
        // The watcher starts after the scan; a file created before it is ready is never reported (found with 5000+
        // files: a new file in the first seconds stayed invisible). Wait for it so later changes are seen.
        if watch && finished { _ = try? Self.result(fff_wait_for_watcher(created, scanTimeout)) { $0.int_value } }
        handle = created
    }

    deinit { fff_destroy(handle) }

    /// Fuzzy file search, best first.
    package func search(_ query: String, limit: Int = 100) throws -> [FFFFile] {
        guard !query.utf8.contains(0) else { throw FFFError(message: "NUL in query") }
        lock.lock()
        defer { lock.unlock() }
        return try query.withCString { text in
            try Self.result(fff_search(handle, text, nil, 0, 0, UInt32(limit), 0, 0)) { result in
                guard let payload = result.handle?.assumingMemoryBound(to: FffSearchResult.self) else {
                    throw FFFError(message: "Missing search payload")
                }
                defer { fff_free_search_result(payload) }
                return try (0..<fff_search_result_get_count(payload)).map { index in
                    guard let item = fff_search_result_get_item(payload, index),
                          let path = fff_file_item_get_relative_path(item) else {
                        throw FFFError(message: "Missing file path")
                    }
                    let score = fff_search_result_get_score(payload, index)
                    return FFFFile(path: String(cString: path), git: fff_file_item_get_git_status(item).map { String(cString: $0) },
                                   score: Int(score?.pointee.total ?? 0), exact: score?.pointee.exact_match ?? false)
                }
            }
        }
    }

    /// Plain-text content search (smart case), up to `limit` matches within `budget` ms.
    package func grep(_ query: String, limit: Int, budget: UInt64 = 1000) throws -> FFFGrep {
        guard !query.utf8.contains(0) else { throw FFFError(message: "NUL in query") }
        lock.lock()
        defer { lock.unlock() }
        return try query.withCString { text in
            try Self.result(fff_live_grep(handle, text, 0, 0, 3, true, 0, UInt32(limit), budget, 0, 0, false)) { result in
                guard let payload = result.handle?.assumingMemoryBound(to: FffGrepResult.self) else {
                    throw FFFError(message: "Missing grep payload")
                }
                defer { fff_free_grep_result(payload) }
                let complete = fff_grep_result_get_next_file_offset(payload) == 0
                let matches = try (0..<fff_grep_result_get_count(payload)).map { index in
                    guard let match = fff_grep_result_get_match(payload, index),
                          let path = fff_grep_match_get_relative_path(match) else {
                        throw FFFError(message: "Missing grep match")
                    }
                    return FFFMatch(path: String(cString: path), line: Int(fff_grep_match_get_line_number(match)),
                                    column: Int(fff_grep_match_get_col(match)) + 1,
                                    text: fff_grep_match_get_line_content(match).map { String(cString: $0) } ?? "",
                                    git: fff_grep_match_get_git_status(match).map { String(cString: $0) })
                }
                return FFFGrep(matches: matches, complete: complete)
            }
        }
    }

    /// Always frees the envelope (and only the envelope); the error string is copied first. `copy` must take
    /// whatever it needs from `result.handle` before returning.
    private static func result<T>(_ pointer: UnsafeMutablePointer<FffResult>?,
                                  copy: (FffResult) throws -> T) throws -> T {
        guard let pointer else { throw FFFError(message: "Null fff result") }
        defer { fff_free_result(pointer) }
        guard pointer.pointee.success else {
            throw FFFError(message: pointer.pointee.error.map { String(cString: $0) } ?? "fff failed")
        }
        return try copy(pointer.pointee)
    }
}
