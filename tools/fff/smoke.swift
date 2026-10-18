import CFFF
import Foundation

struct SmokeError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

/// Serial-use ownership boundary for the experiment, deliberately not Sendable.
/// No fff pointer or borrowed string escapes this type.
final class FFFIndex {
    private let handle: UnsafeMutableRawPointer

    init(root: String) throws {
        guard !root.utf8.contains(0) else { throw SmokeError(message: "NUL in root") }
        let created = try root.withCString { path in
            var options = FffCreateOptions()
            options.version = UInt32(FFF_CREATE_OPTIONS_VERSION)
            options.base_path = path
            // No LMDB stores, watcher, content index, or speculative content cache.
            // fff copies option strings during creation, before withCString returns.
            return try Self.result(fff_create_instance_with(&options)) { result in
                guard let handle = result.handle else { throw SmokeError(message: "Missing instance") }
                return handle
            }
        }
        do {
            try Self.result(fff_wait_for_scan(created, 10_000)) { result in
                guard result.int_value == 1 else { throw SmokeError(message: "Scan timed out") }
            }
        } catch {
            fff_destroy(created)
            throw error
        }
        handle = created
    }

    deinit { fff_destroy(handle) }

    func search(_ query: String) throws -> [String] {
        guard !query.utf8.contains(0) else { throw SmokeError(message: "NUL in query") }
        return try query.withCString { text in
            try Self.result(fff_search(handle, text, nil, 0, 0, 100, 0, 0)) { result in
                guard let payload = result.handle?.assumingMemoryBound(to: FffSearchResult.self) else {
                    throw SmokeError(message: "Missing search payload")
                }
                // Free the payload AND the outer envelope, even if copying throws.
                defer { fff_free_search_result(payload) }
                return try (0..<fff_search_result_get_count(payload)).map { index in
                    guard let item = fff_search_result_get_item(payload, index),
                          let path = fff_file_item_get_relative_path(item) else {
                        throw SmokeError(message: "Missing file path")
                    }
                    return String(cString: path)
                }
            }
        }
    }

    private static func result<T>(_ pointer: UnsafeMutablePointer<FffResult>?,
                                  copy: (FffResult) throws -> T) throws -> T {
        guard let pointer else { throw SmokeError(message: "Null fff result") }
        defer { fff_free_result(pointer) }
        guard pointer.pointee.success else {
            // The error string belongs to the envelope; copy before freeing it.
            throw SmokeError(message: pointer.pointee.error.map { String(cString: $0) } ?? "fff failed")
        }
        return try copy(pointer.pointee)
    }
}

func check(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
    guard try condition() else { throw SmokeError(message: message) }
}

func smoke() throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("smcli-fff-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: root.appendingPathComponent("src"), withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let expected = ["src/needle space.swift", "src/needle-東京-$('quote').swift"]
    for path in expected + ["README.md"] {
        try Data("fixture\n".utf8).write(to: root.appendingPathComponent(path))
    }

    // Exercise an actual C failure envelope, not only Swift validation.
    for _ in 0..<10 {
        do {
            _ = try FFFIndex(root: "")
            throw SmokeError(message: "Empty root unexpectedly accepted")
        } catch let error as SmokeError where error.message.contains("null or empty") {}
    }

    var queries = 0
    // A new instance each pass; many searches reuse that instance.
    for _ in 0..<10 {
        let index = try FFFIndex(root: root.path)
        for _ in 0..<50 {
            let paths = try index.search("needle")
            try check(Set(paths) == Set(expected), "Unexpected paths: \(paths)")
            try check(try index.search("no-such-file-xyz").isEmpty, "Expected no matches")
            queries += 2
        }
        do {
            _ = try index.search("needle\0truncated")
            throw SmokeError(message: "NUL query unexpectedly accepted")
        } catch let error as SmokeError where error.message == "NUL in query" {}
    }
    print("fff C API smoke passed: 10 create/scan/destroy cycles, \(queries) searches, error envelopes, Unicode and metacharacter paths.")
}

do {
    try smoke()
} catch {
    FileHandle.standardError.write(Data("fff-smoke: \(error.localizedDescription)\n".utf8))
    exit(1)
}
