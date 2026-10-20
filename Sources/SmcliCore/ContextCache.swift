import Foundation
#if canImport(CoreServices)
import CoreServices
#endif

/// Serves `ProjectContext` for the service: discovery runs on the caller's thread (it only stats and reads small
/// files), while Git status is cached per worktree and refreshed on a background queue. A request never waits
/// for Git; it gets the last status and `statusPending` while a refresh is running.
///
/// A refresh starts when the status is missing, a file-system event touched the worktree or its Git directory,
/// or the status is older than `maxAge` (events can be missed or coalesced, and `.git/index` timestamps miss
/// working-tree edits). Refreshes of one worktree start at least `minInterval` apart, so a noisy build
/// directory can't make the service run Git continuously.
package final class ContextCache: @unchecked Sendable {
    package typealias StatusReader = @Sendable (String) -> GitStatus?

    private struct Entry {
        var status: GitStatus?
        /// Running, or scheduled to start.
        var refreshing = false
        /// Something changed since the running refresh started.
        var dirty = false
        var started = Date.distantPast
        var touched = Date()
        var watcher: AnyObject?
    }

    private let lock = NSLock()
    private var entries: [String: Entry] = [:]
    private let queue = DispatchQueue(label: "smcli.context.status", qos: .utility, attributes: .concurrent)
    private let read: StatusReader
    private let maxAge: Double
    private let minInterval: Double
    private let limit: Int
    private let watch: Bool

    /// `limit` caps cached worktrees, and with them the file watchers; the least recently asked about goes first.
    package init(maxAge: Double = 5, minInterval: Double = 1, limit: Int = 16, watch: Bool = true,
                 read: @escaping StatusReader = { GitStatus.read(root: $0, timeout: 10) }) {
        (self.maxAge, self.minInterval, self.limit, self.watch, self.read) = (maxAge, minInterval, limit, watch, read)
    }

    package func context(for directory: String) -> ProjectContext {
        var context = ProjectContext.discover(directory)
        guard let root = context.repository?.root, let repository = context.repository else { return context }
        lock.lock()
        defer { lock.unlock() }
        var entry = entries[root] ?? makeEntry(for: repository)
        entry.touched = Date()
        entries[root] = entry
        let stale = entry.status.map { Date().timeIntervalSince1970 - $0.fetchedAt > maxAge } ?? true
        if (stale || entry.dirty) && !entry.refreshing { startRefresh(root) }
        context.status = entry.status
        context.statusPending = entries[root]?.refreshing ?? false
        return context
    }

    /// Marks a worktree's status out of date, as a file-system event does.
    package func invalidate(_ root: String) {
        lock.lock()
        defer { lock.unlock() }
        guard var entry = entries[root] else { return }
        entry.dirty = true
        entries[root] = entry
        if !entry.refreshing { startRefresh(root) }
    }

    /// Called with the lock held.
    private func makeEntry(for repository: ProjectContext.Repository) -> Entry {
        if entries.count >= limit, let oldest = entries.min(by: { $0.value.touched < $1.value.touched })?.key {
            entries[oldest] = nil
        }
        var entry = Entry()
        #if canImport(CoreServices)
        if watch {
            // A linked worktree's HEAD and index live outside its root, and branches in the shared directory.
            let paths = [repository.root] + [repository.gitDir, repository.commonDir]
                .filter { !($0 + "/").hasPrefix(repository.root + "/") }
            let root = repository.root
            entry.watcher = FileWatcher(paths: Array(Set(paths)).sorted()) { [weak self] in self?.invalidate(root) }
        }
        #endif
        return entry
    }

    /// Called with the lock held.
    private func startRefresh(_ root: String) {
        guard var entry = entries[root] else { return }
        entry.refreshing = true
        entry.dirty = false
        let delay = max(0, minInterval - Date().timeIntervalSince(entry.started))
        entry.started = Date().addingTimeInterval(delay)
        entries[root] = entry
        queue.asyncAfter(deadline: .now() + delay) { [self] in
            let status = read(root)
            lock.lock()
            defer { lock.unlock() }
            guard var entry = entries[root] else { return }
            // A failed or timed-out read keeps the previous status rather than erasing it.
            entry.status = status ?? entry.status
            entry.refreshing = false
            entries[root] = entry
            if entry.dirty { startRefresh(root) }
        }
    }
}

#if canImport(CoreServices)
/// FSEvents on a set of directory trees; calls `handler` (on a private queue) after changes settle.
final class FileWatcher: @unchecked Sendable {
    private var stream: FSEventStreamRef?
    private let handler: () -> Void
    private let queue = DispatchQueue(label: "smcli.context.events", qos: .utility)

    init?(paths: [String], latency: Double = 0.2, handler: @escaping () -> Void) {
        self.handler = handler
        var context = FSEventStreamContext(version: 0, info: Unmanaged.passUnretained(self).toOpaque(),
                                           retain: nil, release: nil, copyDescription: nil)
        let callback: FSEventStreamCallback = { _, info, _, _, _, _ in
            guard let info else { return }
            Unmanaged<FileWatcher>.fromOpaque(info).takeUnretainedValue().handler()
        }
        guard let stream = FSEventStreamCreate(nil, callback, &context, paths as CFArray,
                                               FSEventStreamEventId(kFSEventStreamEventIdSinceNow), latency,
                                               FSEventStreamCreateFlags(kFSEventStreamCreateFlagNoDefer)) else { return nil }
        self.stream = stream
        FSEventStreamSetDispatchQueue(stream, queue)
        guard FSEventStreamStart(stream) else {
            FSEventStreamInvalidate(stream)
            FSEventStreamRelease(stream)
            return nil
        }
    }

    deinit {
        guard let stream else { return }
        FSEventStreamStop(stream)
        FSEventStreamInvalidate(stream)
        FSEventStreamRelease(stream)
    }
}
#endif
