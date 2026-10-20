import Foundation
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif

/// What a directory is part of: its Git repository and worktree, and the projects around it. Discovery only
/// stats and reads small files (never runs Git, never executes a manifest), so it is cheap enough for the
/// typing path. The Git working-tree status comes from `GitStatus`, cached by `ContextCache`.
package struct ProjectContext: Codable, Equatable {
    package struct Repository: Codable, Equatable {
        /// The current worktree's top directory (real path).
        package var root: String
        /// This worktree's Git directory: `.git`, or `<common>/worktrees/<name>` for a linked worktree.
        package var gitDir: String
        /// The Git directory shared by every worktree of the repository; its identity.
        package var commonDir: String
        package var linkedWorktree: Bool
        /// Nil while HEAD is detached; then `head` holds the abbreviated commit.
        package var branch: String?
        package var head: String?
        /// `merge`, `rebase`, `am`, `cherry-pick`, `revert` or `bisect`, from marker files in the Git directory.
        package var operation: String?
    }

    package struct Project: Codable, Equatable {
        /// The directory holding the manifests.
        package var root: String
        /// `swift`, `node`, `rust`, `python`, `go` or `make`: the first manifest found in that order.
        package var kind: String
        package var manifests: [String]
        package var name: String?
        /// A hint from a manifest field, a lockfile or the project kind: `npm`, `pnpm`, `cargo`, `uv`...
        package var packageManager: String?
        /// Script, target or task names, by the manifest that declares them.
        package var scripts: [String: [String]]

        /// The only package.json managers smcli builds commands for; anything else in the field is ignored.
        package static let nodeManagers: Set<String> = ["npm", "pnpm", "yarn", "bun"]
    }

    /// The directory asked about, as given.
    package var directory: String
    package var repository: Repository?
    /// The nearest project first, then enclosing ones up to the repository root.
    package var projects: [Project]
    /// Where the directory is under the worktree root and the nearest project root ("." at the root itself).
    package var repoRelative: String?
    package var projectRelative: String?
    package var status: GitStatus?
    /// A status refresh is running or queued, so `status` is missing or may be out of date.
    package var statusPending = false

    package init(directory: String) {
        self.directory = directory
        projects = []
    }

    private static let manifests: [(file: String, kind: String)] = [
        ("Package.swift", "swift"), ("package.json", "node"), ("Cargo.toml", "rust"),
        ("pyproject.toml", "python"), ("go.mod", "go"), ("Makefile", "make"),
    ]
    private static let makefiles = ["Makefile", "makefile", "GNUmakefile"]
    private static let maxLevels = 64

    package static func discover(_ directory: String) -> ProjectContext {
        var context = ProjectContext(directory: directory)
        let start = realPath(directory) ?? URL(fileURLWithPath: directory).standardized.path
        var visited: [String] = []
        var current = start
        for _ in 0..<maxLevels {
            visited.append(current)
            if let repository = repository(at: current) {
                context.repository = repository
                break
            }
            let parent = (current as NSString).deletingLastPathComponent
            if parent == current || parent.isEmpty { break }
            current = parent
        }
        for level in visited {
            if let project = project(at: level) {
                context.projects.append(project)
                // Outside a repository there is no boundary to stop at, so only the nearest counts.
                if context.repository == nil { break }
            }
        }
        if let root = context.repository?.root { context.repoRelative = relative(start, to: root) }
        if let root = context.projects.first?.root { context.projectRelative = relative(start, to: root) }
        return context
    }

    // MARK: - Git

    /// Git's rules: `.git` is a directory, or a file `gitdir: <path>` (linked worktrees, submodules); a linked
    /// worktree's Git directory names the shared one in `commondir`.
    static func repository(at directory: String) -> Repository? {
        let dotGit = directory + "/.git"
        var info = stat()
        guard lstat(dotGit, &info) == 0 else { return nil }
        var gitDir: String
        if info.st_mode & mode_t(S_IFMT) == mode_t(S_IFDIR) {
            gitDir = dotGit
        } else {
            guard let text = readSmallFile(dotGit), text.hasPrefix("gitdir:") else { return nil }
            let path = text.dropFirst("gitdir:".count).trimmingCharacters(in: .whitespacesAndNewlines)
            gitDir = path.hasPrefix("/") ? path : directory + "/" + path
        }
        guard let resolvedGitDir = realPath(gitDir), let head = readSmallFile(resolvedGitDir + "/HEAD") else { return nil }
        gitDir = resolvedGitDir
        var commonDir = gitDir
        if let common = readSmallFile(gitDir + "/commondir"), !common.isEmpty,
           let resolved = realPath(common.hasPrefix("/") ? common : gitDir + "/" + common) {
            commonDir = resolved
        }
        var repository = Repository(root: realPath(directory) ?? directory, gitDir: gitDir, commonDir: commonDir,
                                    linkedWorktree: gitDir != commonDir)
        if head.hasPrefix("ref:") {
            let ref = head.dropFirst("ref:".count).trimmingCharacters(in: .whitespaces)
            repository.branch = ref.hasPrefix("refs/heads/") ? String(ref.dropFirst("refs/heads/".count)) : ref
        } else {
            repository.head = String(head.prefix(7))
        }
        let markers: [(String, String)] = [("rebase-merge", "rebase"), ("rebase-apply/rebasing", "rebase"),
                                           ("rebase-apply", "am"), ("MERGE_HEAD", "merge"),
                                           ("CHERRY_PICK_HEAD", "cherry-pick"), ("REVERT_HEAD", "revert"),
                                           ("BISECT_LOG", "bisect")]
        repository.operation = markers.first { access(gitDir + "/" + $0.0, F_OK) == 0 }?.1
        return repository
    }

    // MARK: - Projects

    static func project(at directory: String) -> Project? {
        var found: [(file: String, kind: String)] = []
        for (file, kind) in manifests {
            let names = kind == "make" ? makefiles : [file]
            if let name = names.first(where: { access(directory + "/" + $0, F_OK) == 0 }) { found.append((name, kind)) }
        }
        guard let primary = found.first else { return nil }
        var project = Project(root: directory, kind: primary.kind, manifests: found.map(\.file), name: nil,
                              packageManager: nil, scripts: [:])
        func exists(_ file: String) -> Bool { access(directory + "/" + file, F_OK) == 0 }
        for (file, kind) in found {
            let text = readSmallFile(directory + "/" + file, limit: 1 << 20)
            var names: [String] = []
            switch kind {
            case "swift": names = swiftTargets(text)
            case "node": names = nodeScripts(text, into: &project, exists: exists)
            case "rust": names = cargoTargets(text, into: &project)
            case "python": names = pythonScripts(text, into: &project, exists: exists)
            case "go": project.name = project.name ?? goModule(text)
            default: names = makeTargets(text)
            }
            if !names.isEmpty { project.scripts[file] = Array(names.prefix(200)) }
        }
        if project.packageManager == nil {
            project.packageManager = ["swift": "swiftpm", "rust": "cargo", "go": "go", "make": "make"][primary.kind]
        }
        return project
    }

    /// `.target(name: "X"`, `.executableTarget(` and `.testTarget(` in a Package.swift, read as text, not run.
    private static func swiftTargets(_ text: String?) -> [String] {
        guard let text else { return [] }
        return matches(#"\.(?:executableTarget|target|testTarget|plugin|macro)\(\s*name:\s*"([^"]+)""#, in: text)
    }

    private static func nodeScripts(_ text: String?, into project: inout Project, exists: (String) -> Bool) -> [String] {
        let json = text.flatMap { try? JSONSerialization.jsonObject(with: Data($0.utf8)) } as? [String: Any]
        project.name = json?["name"] as? String
        // The field is repository-controlled text that ends up in suggested commands, so only known names count.
        if let field = json?["packageManager"] as? String, let name = field.split(separator: "@").first,
           Project.nodeManagers.contains(String(name)) {
            project.packageManager = String(name)
        } else if let lock = [("pnpm-lock.yaml", "pnpm"), ("yarn.lock", "yarn"), ("bun.lock", "bun"),
                              ("bun.lockb", "bun"), ("package-lock.json", "npm")].first(where: { exists($0.0) }) {
            project.packageManager = lock.1
        } else {
            project.packageManager = "npm"
        }
        return ((json?["scripts"] as? [String: Any]).map { Array($0.keys) } ?? []).sorted()
    }

    private static func cargoTargets(_ text: String?, into project: inout Project) -> [String] {
        var names: [String] = []
        var section = ""
        for line in tomlLines(text) {
            if line.hasPrefix("[") { section = line; continue }
            guard let (key, value) = tomlPair(line), key == "name" else { continue }
            if section == "[package]" { project.name = value } else if section == "[[bin]]" { names.append(value) }
        }
        project.packageManager = "cargo"
        return names
    }

    private static func pythonScripts(_ text: String?, into project: inout Project, exists: (String) -> Bool) -> [String] {
        var names: [String] = []
        var section = ""
        var tools: Set<String> = []
        for line in tomlLines(text) {
            if line.hasPrefix("[") {
                section = line
                if section.hasPrefix("[tool.poetry") { tools.insert("poetry") }
                if section.hasPrefix("[tool.uv") { tools.insert("uv") }
                continue
            }
            guard let (key, value) = tomlPair(line, anyValue: true) else { continue }
            if section == "[project]" && key == "name" { project.name = value }
            if section == "[project.scripts]" || section == "[tool.poetry.scripts]" { names.append(key) }
        }
        let locks = [("uv.lock", "uv"), ("poetry.lock", "poetry"), ("Pipfile.lock", "pipenv"), ("pdm.lock", "pdm")]
        project.packageManager = locks.first { exists($0.0) }?.1 ?? ["uv", "poetry"].first { tools.contains($0) } ?? "pip"
        return names
    }

    private static func goModule(_ text: String?) -> String? {
        text?.split(separator: "\n").first { $0.hasPrefix("module ") }
            .map { $0.dropFirst("module ".count).trimmingCharacters(in: .whitespaces) }
    }

    /// Rule targets by simple parsing: `name:` at the start of a line, not an assignment, pattern or special target.
    private static func makeTargets(_ text: String?) -> [String] {
        guard let text else { return [] }
        var seen: Set<String> = []
        var names: [String] = []
        for line in text.split(separator: "\n", omittingEmptySubsequences: true) {
            guard let first = line.first, first != "\t", first != "#", first != ".", first != " " else { continue }
            guard let colon = line.firstIndex(of: ":") else { continue }
            let after = line[line.index(after: colon)...]
            if after.hasPrefix("=") || after.hasPrefix(":=") { continue }
            for word in line[..<colon].split(separator: " ")
            where word.allSatisfy({ $0.isLetter || $0.isNumber || "_-./".contains($0) }) && seen.insert(String(word)).inserted {
                names.append(String(word))
            }
        }
        return names
    }

    private static func matches(_ pattern: String, in text: String) -> [String] {
        guard let expression = try? NSRegularExpression(pattern: pattern) else { return [] }
        let range = NSRange(text.startIndex..., in: text)
        var seen: Set<String> = []
        return expression.matches(in: text, range: range).compactMap { match in
            Range(match.range(at: 1), in: text).map { String(text[$0]) }
        }.filter { seen.insert($0).inserted }
    }

    private static func tomlLines(_ text: String?) -> [String] {
        (text ?? "").split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty && !$0.hasPrefix("#") }
    }

    /// `key = "value"`; with `anyValue`, a key with any value (script tables map names to entry points).
    private static func tomlPair(_ line: String, anyValue: Bool = false) -> (String, String)? {
        guard let equals = line.firstIndex(of: "=") else { return nil }
        let key = line[..<equals].trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "\"'"))
        let value = line[line.index(after: equals)...].trimmingCharacters(in: .whitespaces)
        if value.hasPrefix("\""), let end = value.dropFirst().firstIndex(of: "\"") {
            return (key, String(value[value.index(after: value.startIndex)..<end]))
        }
        return anyValue ? (key, value) : nil
    }

    // MARK: - Files

    static func readSmallFile(_ path: String, limit: Int = 4096) -> String? {
        guard let handle = FileHandle(forReadingAtPath: path) else { return nil }
        defer { try? handle.close() }
        guard let data = try? handle.read(upToCount: limit) else { return nil }
        return String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
    }

    static func realPath(_ path: String) -> String? {
        guard let resolved = realpath(path, nil) else { return nil }
        defer { free(resolved) }
        return String(cString: resolved)
    }

    private static func relative(_ path: String, to root: String) -> String {
        if path == root { return "." }
        return path.hasPrefix(root + "/") ? String(path.dropFirst(root.count + 1)) : path
    }
}

/// `git status --porcelain=v2 --branch`, reduced to counts: what a prompt or ranking signal needs.
package struct GitStatus: Codable, Equatable {
    package var oid: String?
    /// The branch, or nil when detached.
    package var branch: String?
    package var upstream: String?
    package var ahead = 0
    package var behind = 0
    package var staged = 0
    package var unstaged = 0
    package var untracked = 0
    package var conflicted = 0
    /// Unix time this was read.
    package var fetchedAt: Double

    package init(fetchedAt: Double = Date().timeIntervalSince1970) { self.fetchedAt = fetchedAt }

    package var clean: Bool { staged + unstaged + untracked + conflicted == 0 }

    /// Header lines start with `# `; entries are `1`/`2` (changed; `XY` is index then work tree, `.` for none),
    /// `u` (unmerged) and `?` (untracked). Ignored entries (`!`) aren't requested.
    package static func parse(_ output: String, fetchedAt: Double = Date().timeIntervalSince1970) -> GitStatus {
        var status = GitStatus(fetchedAt: fetchedAt)
        for line in output.split(separator: "\n") {
            let words = line.split(separator: " ", maxSplits: 2)
            switch line.first {
            case "#":
                guard words.count >= 3 else { continue }
                let value = String(words[2])
                switch words[1] {
                case "branch.oid": status.oid = value == "(initial)" ? nil : value
                case "branch.head": status.branch = value == "(detached)" ? nil : value
                case "branch.upstream": status.upstream = value
                case "branch.ab":
                    let parts = value.split(separator: " ")
                    if parts.count == 2 { (status.ahead, status.behind) = (Int(parts[0].dropFirst()) ?? 0, Int(parts[1].dropFirst()) ?? 0) }
                default: break
                }
            case "1", "2":
                guard words.count >= 2, words[1].count == 2 else { continue }
                let codes = Array(words[1])
                if codes[0] != "." { status.staged += 1 }
                if codes[1] != "." { status.unstaged += 1 }
            case "u": status.conflicted += 1
            case "?": status.untracked += 1
            default: break
            }
        }
        return status
    }

    /// Runs Git without a shell or optional locks (so it never contends with the user's own Git), killed after
    /// `timeout` seconds. Nil on timeout or failure.
    package static func read(root: String, timeout: Double) -> GitStatus? {
        runGit(["-C", root, "status", "--porcelain=v2", "--branch"], timeout: timeout).map { parse($0) }
    }

    static func runGit(_ arguments: [String], timeout: Double) -> String? {
        var environment = ProcessInfo.processInfo.environment.filter { !$0.key.hasPrefix("GIT_") }
        environment["GIT_OPTIONAL_LOCKS"] = "0"
        environment["GIT_TERMINAL_PROMPT"] = "0"
        environment["LC_ALL"] = "C"
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = ["git", "--no-pager"] + arguments
        process.environment = environment
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        process.standardInput = FileHandle.nullDevice
        let exited = DispatchSemaphore(value: 0)
        process.terminationHandler = { _ in exited.signal() }
        guard (try? process.run()) != nil else { return nil }
        let output = OutputBox()
        let drained = DispatchSemaphore(value: 0)
        // A thread of its own: the shared pool can be busy with callers blocked on this very read.
        Thread.detachNewThread {
            output.data = pipe.fileHandleForReading.readDataToEndOfFile()
            drained.signal()
        }
        let deadline = DispatchTime.now() + timeout
        if exited.wait(timeout: deadline) == .timedOut {
            process.terminate()
            _ = exited.wait(timeout: .now() + 1)
            return nil
        }
        // A daemon Git started (fsmonitor) can keep the pipe open; the output is complete once Git exits.
        guard drained.wait(timeout: .now() + 1) == .success, process.terminationStatus == 0 else { return nil }
        return String(decoding: output.data, as: UTF8.self)
    }
}

private final class OutputBox: @unchecked Sendable {
    var data = Data()
}
