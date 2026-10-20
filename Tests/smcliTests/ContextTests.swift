import Foundation
import Testing
@testable import SmcliCore

struct ContextTests {
    private func temporaryDirectory() throws -> URL {
        // Real path, as discovery reports (macOS's temporary directory sits behind /var → /private/var).
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("ctx-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return URL(fileURLWithPath: ProjectContext.realPath(url.path)!)
    }

    @discardableResult
    private func git(_ arguments: [String], in directory: URL) throws -> String {
        let identity = ["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false",
                        "-c", "init.defaultBranch=main"]
        let output = GitStatus.runGit(["-C", directory.path] + identity + arguments, timeout: 30)
        return try #require(output, "git \(arguments.joined(separator: " ")) failed")
    }

    private func repository(_ directory: URL) throws {
        try git(["init", "-q"], in: directory)
        try write("hello\n", "a.txt", in: directory)
        try git(["add", "."], in: directory)
        try git(["commit", "-q", "-m", "one"], in: directory)
    }

    private func write(_ text: String, _ path: String, in directory: URL) throws {
        let url = directory.appendingPathComponent(path)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try text.write(to: url, atomically: true, encoding: .utf8)
    }

    private func wait(_ seconds: Double = 10, until condition: () -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(seconds)
        while !condition() && Date() < deadline { usleep(20_000) }
        return condition()
    }

    @Test func `Nested projects are found nearest first up to the repository root`() throws {
        let root = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        try repository(root)
        try write(#"{"name":"web","packageManager":"pnpm@9.0.0","scripts":{"test":"x","build":"y"}}"#, "web/package.json", in: root)
        try write("", "web/yarn.lock", in: root)
        try write("[package]\nname = \"core\"\n\n[[bin]]\nname = \"tool\"\n", "web/native/Cargo.toml", in: root)
        try write("build: deps\n\tcc x\nCC := gcc\n.PHONY: build\ntest lint:\n%.o: %.c\n", "Makefile", in: root)
        try write("deep", "web/native/src/main.rs", in: root)
        let context = ProjectContext.discover(root.path + "/web/native/src")
        #expect(context.repository?.root == root.path)
        #expect(context.projects.map(\.kind) == ["rust", "node", "make"])
        #expect(context.projects.map(\.root) == [root.path + "/web/native", root.path + "/web", root.path])
        #expect(context.projects[0].name == "core" && context.projects[0].scripts["Cargo.toml"] == ["tool"])
        #expect(context.projects[1].packageManager == "pnpm" && context.projects[1].scripts["package.json"] == ["build", "test"])
        #expect(context.projects[2].scripts["Makefile"] == ["build", "test", "lint"])
        #expect(context.repoRelative == "web/native/src" && context.projectRelative == "src")
        #expect(ProjectContext.discover(root.path).repoRelative == ".")
    }

    @Test func `Package manager hints come from lockfiles and manifests`() throws {
        let root = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        try write(#"{"scripts":{"dev":"x"}}"#, "node/package.json", in: root)
        try write("", "node/bun.lock", in: root)
        try write("[project]\nname = \"py\"\n[project.scripts]\ncli = \"py:main\"\n", "py/pyproject.toml", in: root)
        try write("", "py/uv.lock", in: root)
        try write("module example.com/m\n\ngo 1.22\n", "go/go.mod", in: root)
        try write("// swift-tools-version: 6.0\nlet package = Package(name: \"p\", targets: [.executableTarget(name: \"app\"), .testTarget(name: \"appTests\")])\n", "swift/Package.swift", in: root)
        #expect(ProjectContext.discover(root.path + "/node").projects.first?.packageManager == "bun")
        let python = ProjectContext.discover(root.path + "/py").projects.first
        #expect(python?.packageManager == "uv" && python?.name == "py" && python?.scripts["pyproject.toml"] == ["cli"])
        #expect(ProjectContext.discover(root.path + "/go").projects.first?.name == "example.com/m")
        #expect(ProjectContext.discover(root.path + "/swift").projects.first?.scripts["Package.swift"] == ["app", "appTests"])
    }

    @Test func `A directory outside Git has no repository and only its nearest project`() throws {
        let root = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        try write("all:\n", "Makefile", in: root)
        try write("module a\n", "inner/go.mod", in: root)
        let context = ProjectContext.discover(root.path + "/inner/sub")
        #expect(context.repository == nil && context.repoRelative == nil)
        #expect(context.projects.map(\.kind) == ["go"])
        // A directory that doesn't exist still resolves against the ancestors that do.
        #expect(ProjectContext.discover(root.path + "/missing/dir").projects.map(\.kind) == ["make"])
    }

    @Test func `Linked worktrees share the repository identity but have their own root and branch`() throws {
        let root = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let main = root.appendingPathComponent("main")
        try FileManager.default.createDirectory(at: main, withIntermediateDirectories: true)
        try repository(main)
        let linked = root.appendingPathComponent("linked")
        try git(["worktree", "add", "-q", "-b", "feature", linked.path], in: main)
        let first = try #require(ProjectContext.discover(main.path).repository)
        let second = try #require(ProjectContext.discover(linked.path).repository)
        #expect(!first.linkedWorktree && second.linkedWorktree)
        #expect(first.commonDir == second.commonDir && first.gitDir != second.gitDir)
        #expect(first.root == main.path && second.root == linked.path)
        #expect(first.branch == "main" && second.branch == "feature")
        // A `.git` file with a relative gitdir resolves too.
        let relative = try #require(ProjectContext.repository(at: linked.path))
        #expect(relative.gitDir.hasSuffix("/worktrees/linked"))
    }

    @Test func `Branch switches, detached HEAD and operations in progress are read from the Git directory`() throws {
        let root = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        try repository(root)
        try git(["checkout", "-q", "-b", "topic/x"], in: root)
        #expect(ProjectContext.discover(root.path).repository?.branch == "topic/x")
        try git(["checkout", "-q", "--detach"], in: root)
        let detached = try #require(ProjectContext.discover(root.path).repository)
        #expect(detached.branch == nil && detached.head?.count == 7)
        let gitDir = root.appendingPathComponent(".git")
        for (marker, operation) in [("MERGE_HEAD", "merge"), ("CHERRY_PICK_HEAD", "cherry-pick"), ("BISECT_LOG", "bisect")] {
            try write("x", ".git/" + marker, in: root)
            #expect(ProjectContext.discover(root.path).repository?.operation == operation)
            try FileManager.default.removeItem(at: gitDir.appendingPathComponent(marker))
        }
        try FileManager.default.createDirectory(at: gitDir.appendingPathComponent("rebase-merge"), withIntermediateDirectories: true)
        #expect(ProjectContext.discover(root.path).repository?.operation == "rebase")
    }

    @Test func `Porcelain v2 output is counted`() {
        let output = """
        # branch.oid 0123456789abcdef
        # branch.head main
        # branch.upstream origin/main
        # branch.ab +2 -1
        1 M. N... 100644 100644 100644 a b staged.txt
        1 .M N... 100644 100644 100644 a b edited.txt
        1 MM N... 100644 100644 100644 a b both.txt
        2 R. N... 100644 100644 100644 a b R100 new.txt\told.txt
        u UU N... 100644 100644 100644 100644 a b c conflict.txt
        ? untracked.txt
        """
        let status = GitStatus.parse(output)
        #expect((status.staged, status.unstaged, status.untracked, status.conflicted) == (3, 2, 1, 1))
        #expect((status.ahead, status.behind, status.branch, status.upstream) == (2, 1, "main", "origin/main"))
        #expect(!status.clean && GitStatus.parse("# branch.head (detached)\n").branch == nil)
        #expect(GitStatus.parse("# branch.oid (initial)\n# branch.head main\n").clean)
    }

    @Test func `Git status reflects unstaged edits and new files`() throws {
        let root = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        try repository(root)
        #expect(GitStatus.read(root: root.path, timeout: 30)?.clean == true)
        try write("changed\n", "a.txt", in: root)
        try write("new\n", "b.txt", in: root)
        let status = try #require(GitStatus.read(root: root.path, timeout: 30))
        #expect(status.unstaged == 1 && status.untracked == 1 && status.branch == "main")
        #expect(GitStatus.read(root: root.path + "/nowhere", timeout: 30) == nil)
    }

    @Test func `Context requests return at once while the status refresh is pending`() throws {
        let root = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        try repository(root)
        let release = DispatchSemaphore(value: 0)
        let reads = Counter()
        let cache = ContextCache(watch: false) { _ in
            reads.increment()
            release.wait()
            return GitStatus(fetchedAt: Date().timeIntervalSince1970)
        }
        let started = Date()
        for _ in 0..<20 {
            let context = cache.context(for: root.path)
            #expect(context.status == nil && context.statusPending && context.repository?.branch == "main")
        }
        #expect(Date().timeIntervalSince(started) < 1)
        release.signal()
        #expect(wait { cache.context(for: root.path).status != nil })
        #expect(!cache.context(for: root.path).statusPending)
        #expect(reads.value == 1)
        // Stale status is served while the next refresh is pending.
        cache.invalidate(root.path)
        let stale = cache.context(for: root.path)
        #expect(stale.status != nil && stale.statusPending)
        release.signal()
        #expect(wait { !cache.context(for: root.path).statusPending })
    }

    @Test func `A cached status ages out even without events`() throws {
        let root = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        try repository(root)
        let reads = Counter()
        let cache = ContextCache(maxAge: 0.05, minInterval: 0, watch: false) { _ in reads.increment(); return GitStatus() }
        _ = cache.context(for: root.path)
        #expect(wait { reads.value == 1 })
        usleep(100_000)
        _ = cache.context(for: root.path)
        #expect(wait { reads.value == 2 })
    }

    @Test func `The least recently used worktree is evicted past the limit`() throws {
        let roots = try (0..<3).map { _ in try temporaryDirectory() }
        defer { roots.forEach { try? FileManager.default.removeItem(at: $0) } }
        try roots.forEach(repository)
        let reads = Counter()
        // Aging is off, so a read happens only when a worktree has no cached entry.
        let cache = ContextCache(maxAge: 3600, minInterval: 0, limit: 2, watch: false) { _ in reads.increment(); return GitStatus() }
        let (a, b, c) = (roots[0].path, roots[1].path, roots[2].path)
        _ = cache.context(for: a)
        #expect(wait { reads.value == 1 })
        _ = cache.context(for: b)
        #expect(wait { reads.value == 2 })
        usleep(5_000)
        _ = cache.context(for: a)  // a is now newer than b
        usleep(5_000)
        _ = cache.context(for: c)  // evicts b
        #expect(wait { reads.value == 3 })
        _ = cache.context(for: a)
        usleep(100_000)
        #expect(reads.value == 3, "a stayed cached")
        _ = cache.context(for: b)  // evicted: read again
        #expect(wait { reads.value == 4 })
    }

    #if canImport(CoreServices)
    @Test func `File-system events refresh the status`() throws {
        let root = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        try repository(root)
        // Aging is effectively off, so only events can change what is served.
        let cache = ContextCache(maxAge: 3600, minInterval: 0.1)
        _ = cache.context(for: root.path)
        #expect(wait { cache.context(for: root.path).status?.clean == true })
        usleep(500_000)  // FSEvents only reports changes after the stream is running
        try write("edited\n", "a.txt", in: root)
        #expect(wait { cache.context(for: root.path).status?.unstaged == 1 })
        try git(["checkout", "-q", "-b", "other"], in: root)
        #expect(wait { cache.context(for: root.path).status?.branch == "other" })
    }
    #endif
}

private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    func increment() { lock.lock(); count += 1; lock.unlock() }
    var value: Int { lock.lock(); defer { lock.unlock() }; return count }
}
