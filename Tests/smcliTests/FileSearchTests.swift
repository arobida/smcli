import Foundation
import Testing
@testable import smcli
@testable import SmcliFFF
@testable import SmcliCore

/// Awkward file names, each of which must reach a shell as exactly one word.
private let awkwardNames = [
    "plain.txt", "a b.txt", "it's.txt", "q\"uote.txt", "$HOME.txt", "`id`.txt", "star*.txt", "q?.txt", "[br].txt",
    "-dash.txt", "--flag", "~tilde", "=eq", "#hash", "a;b&c|d.txt", "back\\slash", "new\nline.txt", "tab\there",
    "cr\rhere", "\u{1}ctl", "東京 é.txt", "end ", " lead", "", "trailing\n", "multi\n\nline", "'", "''", "\\", "$'x'",
]

struct ShellQuotingTests {
    @Test func `Insert a path only where the shell reads plain words`() {
        for shell in ["bash", "zsh", "fish"] {
            for safe in ["", "cat ", "cat a b ", "echo \"done\" ", "echo 'x' ", "echo $(ls ", "echo \"$(ls ", "(cd ", "a\\ b "] {
                #expect(ShellQuoting.insertable(after: safe, shell: shell), "\(shell): \(safe)")
            }
            for unsafe in ["echo \"", "echo 'abc", "echo \"$(ls \"", "echo \\", "echo \"a\\\" "] {
                #expect(!ShellQuoting.insertable(after: unsafe, shell: shell), "\(shell): \(unsafe)")
            }
        }
        for shell in ["bash", "zsh"] {
            for unsafe in ["echo `", "echo $'a", "echo ${x:-", "echo \"`"] {
                #expect(!ShellQuoting.insertable(after: unsafe, shell: shell), "\(shell): \(unsafe)")
            }
            // POSIX single quotes have no escapes: this one is closed.
            #expect(ShellQuoting.insertable(after: "echo 'a\\' ", shell: shell))
        }
        // Fish: `\'` stays inside single quotes; backticks and `$'` are plain characters.
        #expect(!ShellQuoting.insertable(after: "echo 'a\\' ", shell: "fish"))
        #expect(ShellQuoting.insertable(after: "echo ` ", shell: "fish"))
    }

    @Test func `Here documents refuse file insertion even across a continued operator`() {
        for shell in ["bash", "zsh"] {
            for before in ["cat <<EOF\n", "cat <<-EOF\n\t", "cat <<\'EOF\'\n", "cat <\\\n<EOF\n",
                           "cat <<EOF\nbody\nEOF\necho ", "echo \'<<\' "] {
                #expect(!ShellQuoting.insertable(after: before, shell: shell), "\(shell): \(before)")
            }
        }
        #expect(ShellQuoting.insertable(after: "echo \'<<\' ", shell: "fish"))
    }

    @Test func `Leading dash gets ./ and safe names stay bare`() {
        #expect(ShellQuoting.quote("-dash.txt", shell: "bash") == "./-dash.txt")
        #expect(ShellQuoting.quote("--flag", shell: "fish") == "./--flag")
        #expect(ShellQuoting.quote("src/main.swift", shell: "zsh") == "src/main.swift")
        #expect(ShellQuoting.quote("a b", shell: "bash") == "'a b'")
        #expect(ShellQuoting.quote("it's", shell: "bash") == "'it'\\''s'")
        #expect(ShellQuoting.quote("a\nb", shell: "zsh") == "$'a\\nb'")
        #expect(ShellQuoting.quote("a\nb", shell: "fish") == "'a'\\n'b'")
        #expect(ShellQuoting.quote("", shell: "bash") == "''")
    }

    /// Runs `printf` through the real shell with the quoted word and returns what the shell made of it.
    private func parsed(_ quoted: String, shell: String) throws -> String? {
        let candidates = ["/bin", "/usr/bin", "/opt/homebrew/bin", "/usr/local/bin"].map { "\($0)/\(shell)" }
        guard let executable = candidates.first(where: FileManager.default.isExecutableFile) else { return nil }
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        // Files that an unquoted `*`, `?` or `[br]` would expand to.
        for name in ["star1.txt", "q1.txt", "b.txt", "r.txt", "x"] {
            FileManager.default.createFile(atPath: directory.appendingPathComponent(name).path, contents: nil)
        }
        let process = Process()
        process.executableURL = URL(fileURLWithPath: executable)
        process.currentDirectoryURL = directory
        process.environment = ["HOME": directory.path, "PATH": "/usr/bin:/bin", "LC_ALL": "en_US.UTF-8"]
        // Count the words too: an unquoted space or glob would add arguments.
        let script = "printf '%s\\0' \(quoted) END"
        switch shell {
        case "zsh": process.arguments = ["-f", "-c", script]
        case "fish": process.arguments = ["--no-config", "-c", script]
        default: process.arguments = ["--noprofile", "--norc", "-c", script]
        }
        let out = Pipe()
        process.standardOutput = out
        process.standardError = FileHandle.nullDevice
        try process.run()
        let data = out.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        let words = data.split(separator: 0, omittingEmptySubsequences: false).map { String(decoding: $0, as: UTF8.self) }
        // "<word>", "END", "" (after the last NUL): exactly one word before END.
        return words.count == 3 && words[1] == "END" && process.terminationStatus == 0 ? words[0] : "<\(words)>"
    }

    @Test(arguments: ["bash", "zsh", "fish"])
    func `Every awkward name reaches the shell as one unchanged word`(shell: String) throws {
        for name in awkwardNames {
            let quoted = ShellQuoting.quote(name, shell: shell)
            guard let word = try parsed(quoted, shell: shell) else { return }  // shell not installed
            let expected = name.hasPrefix("-") ? "./" + name : name
            #expect(word == expected, "\(shell): \(name.debugDescription) -> \(quoted)")
        }
    }

    @Test func `Quoted output is a single line in every shell`() {
        // Fish splits command substitution on newlines, so a newline must never be literal in the output.
        for shell in ["bash", "zsh", "fish"] {
            for name in awkwardNames {
                #expect(!ShellQuoting.quote(name, shell: shell).contains("\n"), "\(shell): \(name.debugDescription)")
            }
        }
    }
}

struct FFFTests {
    static func fixture(_ files: [String: String]) throws -> URL {
        let root = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("fff-\(UUID().uuidString)")
        for (name, text) in files {
            let url = root.appendingPathComponent(name)
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try text.write(to: url, atomically: true, encoding: .utf8)
        }
        return URL(fileURLWithPath: realpath(root.path, nil).map { String(cString: $0) } ?? root.path)
    }

    @Test func `Search and grep copy their results out of fff`() throws {
        let root = try Self.fixture(["src/main.swift": "let needle = 1\n", "a b.txt": "x\n", "-dash.txt": "needle too\n"])
        defer { try? FileManager.default.removeItem(at: root) }
        let index = try FFFIndex(root: root.path, watch: false)
        #expect(index.scanned)
        #expect(try index.search("main").first?.path == "src/main.swift")
        #expect(Set(try index.search("").map(\.path)) == ["src/main.swift", "a b.txt", "-dash.txt"])
        #expect(try index.search("zzzqqq").isEmpty)
        let grep = try index.grep("needle", limit: 10)
        #expect(grep.complete)
        let hits = grep.matches
        #expect(Set(hits.map(\.path)) == ["src/main.swift", "-dash.txt"])
        #expect(hits.first { $0.path == "src/main.swift" }.map { ($0.line, $0.column, $0.text.trimmingCharacters(in: .whitespacesAndNewlines)) }! == (1, 5, "let needle = 1"))
        // Plain text, not a regex.
        #expect(try index.grep("n.edle", limit: 10).matches.isEmpty)
        // A page cut short by the limit says more may exist.
        #expect(try !index.grep("needle", limit: 1).complete)
        #expect(throws: FFFError.self) { try index.search("a\0b") }
        #expect(throws: FFFError.self) { try index.grep("a\0b", limit: 1) }
    }

    @Test func `A failed create reports fff's error and leaks nothing`() {
        for _ in 0..<5 {
            #expect(throws: FFFError.self) { try FFFIndex(root: "") }
            #expect(throws: FFFError.self) { try FFFIndex(root: "/no/such/root\0") }
        }
    }

    @Test func `The watcher shows new and removed files in the same instance`() throws {
        let root = try Self.fixture(["one.txt": "1"])
        defer { try? FileManager.default.removeItem(at: root) }
        let index = try FFFIndex(root: root.path)
        func wait(for name: String, present: Bool) -> Bool {
            let deadline = Date().addingTimeInterval(10)
            while Date() < deadline {
                if ((try? index.search(name)) ?? []).contains(where: { $0.path == name }) == present { return true }
                Thread.sleep(forTimeInterval: 0.1)
            }
            return false
        }
        try "x".write(to: root.appendingPathComponent("later.txt"), atomically: true, encoding: .utf8)
        #expect(wait(for: "later.txt", present: true))
        try FileManager.default.removeItem(at: root.appendingPathComponent("later.txt"))
        #expect(wait(for: "later.txt", present: false))
    }

    @Test func `The pool reuses an instance per root and evicts the least recently used`() throws {
        let roots = try (0..<3).map { _ in try Self.fixture(["f.txt": "x"]) }
        defer { roots.forEach { try? FileManager.default.removeItem(at: $0) } }
        let pool = FFFPool(capacity: 2, idleTimeout: 60) { try FFFIndex(root: $0, watch: false) }
        let start = Date()
        let a = try pool.index(for: roots[0].path, now: start)
        for step in 1...5 { #expect(try pool.index(for: roots[0].path, now: start.addingTimeInterval(Double(step))).id == a.id) }
        #expect(pool.instances.count == 1)
        let b = try pool.index(for: roots[1].path, now: start.addingTimeInterval(10))
        _ = try pool.index(for: roots[0].path, now: start.addingTimeInterval(11))  // a is now newer than b
        _ = try pool.index(for: roots[2].path, now: start.addingTimeInterval(12))
        #expect(pool.instances.map(\.root).sorted() == [roots[0].path, roots[2].path].sorted())
        #expect(!pool.instances.contains { $0.id == b.id })
        // An instance evicted while a request still holds it keeps working until released.
        #expect(try b.index.search("f").first?.path == "f.txt")
        // Idle ones are dropped, by the timer or by the next request.
        #expect(pool.evictIdle(start.addingTimeInterval(12 + 61)) == 2)
        #expect(pool.instances.isEmpty)
        #expect(try pool.index(for: roots[0].path, now: start.addingTimeInterval(100)).id != a.id)
    }
}

struct FFFPoolConcurrencyTests {
    final class Counter: @unchecked Sendable {
        private let lock = NSLock()
        private(set) var made: [String: Int] = [:]
        private(set) var running = 0, peak = 0
        func start(_ root: String) { lock.lock(); made[root, default: 0] += 1; running += 1; peak = max(peak, running); lock.unlock() }
        func end() { lock.lock(); running -= 1; lock.unlock() }
    }

    @Test func `Cold concurrent requests share one creation per root, and creations are bounded`() throws {
        let roots = try (0..<4).map { _ in try FFFTests.fixture(["f.txt": "x"]) }
        defer { roots.forEach { try? FileManager.default.removeItem(at: $0) } }
        let counter = Counter()
        let pool = FFFPool(capacity: 2, idleTimeout: 60) { root in
            counter.start(root)
            defer { counter.end() }
            usleep(200_000)  // a slow first scan, so the requests overlap
            return try FFFIndex(root: root, watch: false)
        }
        let seen = Counter()
        let group = DispatchGroup()
        for i in 0..<16 {
            group.enter()
            Thread.detachNewThread {
                let root = roots[i % 2].path
                if let id = try? pool.index(for: root).id { seen.start("\(root) \(id)"); seen.end() }
                group.leave()
            }
        }
        group.wait()
        #expect(counter.made == [roots[0].path: 1, roots[1].path: 1])
        // Every request for a root got the same instance.
        #expect(seen.made.count == 2 && seen.made.values.reduce(0, +) == 16)
        let more = DispatchGroup()
        for root in roots {
            more.enter()
            Thread.detachNewThread { _ = try? pool.index(for: root.path); more.leave() }
        }
        more.wait()
        #expect(counter.peak <= 2)
    }

    @Test func `Requests waiting on a failed creation get its error`() {
        struct Boom: Error {}
        let counter = Counter()
        let pool = FFFPool(capacity: 2, idleTimeout: 60) { root in
            counter.start(root)
            defer { counter.end() }
            usleep(200_000)
            throw Boom()
        }
        let failures = Counter()
        // Real threads, so all eight overlap the slow creation (concurrentPerform may run them in turn).
        let group = DispatchGroup()
        for _ in 0..<8 {
            group.enter()
            Thread.detachNewThread {
                if (try? pool.index(for: "/nowhere")) == nil { failures.start("failed"); failures.end() }
                group.leave()
            }
        }
        group.wait()
        #expect(failures.made["failed"] == 8 && counter.made["/nowhere"] == 1)
        #expect(pool.instances.isEmpty)
    }
}

struct PickerModeTests {
    @Test func `Ctrl-T cycles history, files, grep and each keeps its own query`() {
        var picker = Picker(suggestions: ["ls"], captured: [:], native: [], query: "ls")
        picker.cycleMode()
        #expect(picker.mode == .files && picker.query == "")
        picker.query = "main"
        picker.cycleMode()
        #expect(picker.mode == .grep && picker.query == "")
        picker.query = "needle"
        picker.cycleMode()
        #expect(picker.mode == .history && picker.query == "ls")
        picker.cycleMode()
        #expect(picker.mode == .files && picker.query == "main")
    }

    @Test func `File rows show Git status, path and provenance; grep rows add line and text`() {
        var picker = Picker(suggestions: [], captured: [:], native: [])
        picker.mode = .grep
        picker.query = "needle"
        picker.hits = [FileHit(path: "src/a b.swift", insert: "a b.swift", provenance: ["grep", "picked"], git: " M",
                               line: 12, column: 5, text: "  let needle = 1"),
                       FileHit(path: "README.md", insert: "../README.md", provenance: ["grep"])]
        let screen = picker.render(picker.hits.map(\.path), size: (24, 100))
        #expect(screen.contains(" M"))
        #expect(screen.contains("src/a b.swift:12  let needle = 1"))
        #expect(screen.contains("grep+picked"))
        #expect(screen.contains("README.md"))
        #expect(screen.contains("^T history"))
        picker.mode = .history
        #expect(picker.render([], size: (24, 100)).contains("^T files"))
    }
}
