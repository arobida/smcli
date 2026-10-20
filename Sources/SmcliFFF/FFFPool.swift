import Foundation

/// One `FFFIndex` per root, reused across requests. Each instance owns a file watcher and an in-memory index,
/// so the pool bounds both: at most `capacity` instances, and one idle for `idleTimeout` seconds is dropped.
package final class FFFPool: @unchecked Sendable {
    package struct Info: Equatable, Sendable {
        package let id: Int
        package let root: String
    }

    private struct Entry {
        let id: Int
        let index: FFFIndex
        var used: Date
    }

    package let capacity: Int
    package let idleTimeout: Double
    private let make: @Sendable (String) throws -> FFFIndex
    private let lock = NSLock()
    private var entries: [String: Entry] = [:]
    private var next = 1

    package init(capacity: Int = 4, idleTimeout: Double = 600,
                 make: @escaping @Sendable (String) throws -> FFFIndex = { try FFFIndex(root: $0) }) {
        self.capacity = max(1, capacity)
        self.idleTimeout = idleTimeout
        self.make = make
    }

    /// The instance for `root`, created (and the least recently used one evicted) when needed.
    /// Creation waits for the first scan, so it happens outside the lock: other roots stay responsive.
    package func index(for root: String, now: Date = Date()) throws -> (index: FFFIndex, id: Int) {
        lock.lock()
        dropStale(now)
        if var entry = entries[root] {
            entry.used = now
            entries[root] = entry
            lock.unlock()
            return (entry.index, entry.id)
        }
        lock.unlock()
        let created = try make(root)
        lock.lock()
        defer { lock.unlock() }
        // A concurrent request for the same root may have won; keep its instance and drop ours.
        if var entry = entries[root] {
            entry.used = now
            entries[root] = entry
            return (entry.index, entry.id)
        }
        while entries.count >= capacity, let oldest = entries.min(by: { $0.value.used < $1.value.used }) {
            entries[oldest.key] = nil
        }
        let id = next
        next += 1
        entries[root] = Entry(id: id, index: created, used: now)
        return (created, id)
    }

    /// Drops instances unused for `idleTimeout`; returns how many.
    @discardableResult
    package func evictIdle(_ now: Date = Date()) -> Int {
        lock.lock()
        defer { lock.unlock() }
        return dropStale(now)
    }

    @discardableResult
    private func dropStale(_ now: Date) -> Int {
        let stale = entries.filter { now.timeIntervalSince($0.value.used) > idleTimeout }.map(\.key)
        for root in stale { entries[root] = nil }
        return stale.count
    }

    /// Live instances, oldest id first.
    package var instances: [Info] {
        lock.lock()
        defer { lock.unlock() }
        return entries.map { Info(id: $0.value.id, root: $0.key) }.sorted { $0.id < $1.id }
    }
}
