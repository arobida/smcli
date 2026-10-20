import Foundation
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif

/// Newline-delimited JSON between smcli clients and smcli-service: one request line, one response line.
/// A request with another `v` gets a `version` error, and the client falls back to working in-process.
package enum ServiceProtocol {
    package static let version = 1
    /// Longest request or response line, newline included. Command lines are capped well below this.
    package static let maxMessageBytes = 1 << 20
    /// Longest response line: a `pick` response carries every distinct command.
    package static let maxResponseBytes = 64 << 20
    /// Time a peer gets to send its request, or to take the response, before the connection is dropped.
    package static let ioTimeout = 2.0
    /// Connections served at once; more are refused with a `busy` error.
    package static let maxConnections = 64

    /// Identifies an executable by real path, modification time and size, so a rebuilt or replaced binary differs
    /// from the copy a service started from. Nil when the file can't be examined.
    package static func build(of executable: URL) -> String? {
        guard let resolved = realpath(executable.path, nil) else { return nil }
        defer { free(resolved) }
        var info = stat()
        guard stat(resolved, &info) == 0 else { return nil }
        #if canImport(Darwin)
        let modified = info.st_mtimespec
        #else
        let modified = info.st_mtim
        #endif
        return "\(String(cString: resolved))|\(modified.tv_sec).\(modified.tv_nsec)|\(info.st_size)"
    }
}

package struct ServiceRequest: Codable, Equatable {
    package enum Operation: String, Codable {
        case ping, record, suggest, pick, search, context, files, grep, select, shutdown
    }

    package var v = ServiceProtocol.version
    package var op: Operation

    // record
    package var event: String?
    package var command: String?
    package var shell: String?
    /// Also on suggest, pick and search: the asking shell's session, whose previous command selects transitions.
    package var session: String?
    package var exitCode: Int?
    package var startedAt: Double?
    package var endedAt: Double?
    package var nativeFile: String?
    /// Optional: each pipeline stage's exit status, when the shell reports it.
    package var pipestatus: [Int]?

    // record, suggest, pick, search, context, select
    package var directory: String?

    // suggest (prefix); suggest, pick and search (limit)
    package var prefix: String?
    package var limit: Int?

    // search: the query, its filters, and the caller's native history file and login shell, which decide
    // which native files are read (HISTFILE and SHELL in the CLI).
    package var query: String?
    package var filter: SearchFilter?
    package var histfile: String?
    package var loginShell: String?

    // select: `query` is what was typed. For a file, `path` is the chosen file relative to the searched root and
    // `mode` is files or grep. For a command, `command` is the one picked and `mode` is suggest (from the
    // directory suggestions or history list) or search (from search results).
    package var path: String?
    package var mode: String?

    // search, select: the caller's config file (SMCLI_CONFIG or the default), since a running service may have been
    // started by a shell with another. Filled in by ServiceClient.
    package var config: String?

    package init(op: Operation) { self.op = op }
}

/// One `files` or `grep` result.
package struct FileHit: Codable, Equatable {
    /// Relative to the searched root; this is what `select` takes back.
    package var path: String
    /// Relative to the requesting directory (`../` where needed), never absolute: what gets inserted.
    package var insert: String
    /// How it was found: `files` (fuzzy name), `grep` (content), plus `picked` once the user chose it before.
    package var provenance: [String]
    /// Two-letter status as in `git status --short` (`M `, ` M`, `A `, `??`, `!!`); nil when clean or outside Git.
    package var git: String?
    /// grep only.
    package var line: Int?
    package var column: Int?
    package var text: String?

    package init(path: String, insert: String, provenance: [String], git: String? = nil,
                 line: Int? = nil, column: Int? = nil, text: String? = nil) {
        (self.path, self.insert, self.provenance, self.git) = (path, insert, provenance, git)
        (self.line, self.column, self.text) = (line, column, text)
    }

    /// `path` (under `root`) as seen from `directory`: `a.swift` from its own folder, `../x/b.swift` from a sibling.
    package static func relative(_ path: String, root: String, from directory: String) -> String {
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

/// One live fff instance in the service, for `ping`.
package struct FileIndexInfo: Codable, Equatable {
    package var id: Int
    package var root: String
    package init(id: Int, root: String) { (self.id, self.root) = (id, root) }
}

package struct ServiceResponse: Codable, Equatable {
    package struct Failure: Codable, Equatable {
        /// `bad_request`, `version`, `unsupported`, `busy`, `too_large`, `timeout`, or `failed`.
        package let code: String
        package let message: String
    }

    package var v = ServiceProtocol.version
    package var ok: Bool
    package var error: Failure?

    // record: the stored event ID, and whether it had been stored before (a retry).
    package var event: String?
    package var duplicate: Bool?

    // suggest, pick
    package var suggestions: [Suggestion]?

    // pick: every other distinct command, most recently run first.
    package var recent: [String]?

    // search
    package var hits: [SearchEntry]?

    // context: served from cache; `statusPending` says a Git status refresh is still running.
    package var context: ProjectContext?

    // files, grep: `root` is the Git worktree (or the directory itself outside Git) that was searched, and
    // `instance` the fff instance that answered, which stays the same while the service reuses it.
    package var files: [FileHit]?
    package var root: String?
    package var instance: Int?
    /// grep: true when the time budget or limit stopped the search early, so more matches may exist.
    package var partial: Bool?

    // ping, shutdown
    package var pid: Int32?
    /// ping: live fff instances, to show they are reused and bounded.
    package var indexes: [FileIndexInfo]?

    /// Set on every response: which service executable answered (`ServiceProtocol.build`). A client that expects
    /// another build, or finds none (a service from before this field), restarts the service.
    package var build: String?

    package init(ok: Bool) { self.ok = ok }

    package static func failure(_ code: String, _ message: String) -> ServiceResponse {
        var response = ServiceResponse(ok: false)
        response.error = Failure(code: code, message: message)
        return response
    }
}

/// Where a service for one history database listens. Each database gets its own socket, so tests and
/// `SMCLI_DB` users never reach a service that owns another file.
package struct ServiceLocation {
    package let directory: URL
    package let socket: URL
    package let lock: URL

    package init(database: URL, environment: [String: String]) {
        let base: URL
        if let path = environment["SMCLI_RUNTIME_DIR"], path.hasPrefix("/") {
            base = URL(fileURLWithPath: path)
        } else if let path = environment["XDG_RUNTIME_DIR"], path.hasPrefix("/") {
            base = URL(fileURLWithPath: path).appendingPathComponent("smcli")
        } else {
            // macOS's TMPDIR is already per-user; elsewhere the uid keeps users apart.
            let temporary = environment["TMPDIR"].flatMap { $0.hasPrefix("/") ? $0 : nil } ?? "/tmp"
            base = URL(fileURLWithPath: temporary).appendingPathComponent("smcli-\(getuid())")
        }
        // The path exactly as given (the client passes the same string to the service). Foundation's
        // standardizing maps /private/tmp to /tmp only once the file exists, which would split one database
        // across two sockets.
        let name = "service-" + Self.fingerprint(database.path)
        directory = base
        socket = base.appendingPathComponent(name + ".sock")
        lock = base.appendingPathComponent(name + ".lock")
    }

    /// FNV-1a; only needs to tell database paths apart, not resist an attacker (the directory is private).
    static func fingerprint(_ text: String) -> String {
        var hash: UInt64 = 0xcbf29ce484222325
        for byte in text.utf8 { hash = (hash ^ UInt64(byte)) &* 0x100000001b3 }
        return String(hash, radix: 16).leftPadded(to: 16)
    }

    /// Creates the directory private to this user, or checks an existing one is: a directory (not a symlink),
    /// owned by us, with no group or other access.
    package func prepare() throws {
        if mkdir(directory.path, 0o700) != 0 && errno != EEXIST {
            throw HistoryError(message: "Cannot create \(directory.path): \(String(cString: strerror(errno)))")
        }
        var info = stat()
        guard lstat(directory.path, &info) == 0, info.st_mode & mode_t(S_IFMT) == mode_t(S_IFDIR),
              info.st_uid == getuid(), info.st_mode & 0o077 == 0 else {
            throw HistoryError(message: "Service directory must be a private directory owned by you (chmod 700): \(directory.path)")
        }
    }
}

private extension String {
    func leftPadded(to width: Int) -> String { String(repeating: "0", count: max(0, width - count)) + self }
}
