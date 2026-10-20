import Foundation
import SmcliCore
import SmcliFFF
import SmcliSearch
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif

/// The long-lived owner of one history database: it keeps the database open and suggestions in memory, and
/// answers smcli clients over a private Unix-domain socket. Started on demand by clients; exits when idle.
@main
struct SmcliService {
    static func main() {
        let arguments = Array(CommandLine.arguments.dropFirst())
        guard arguments.count == 2, arguments[0] == "--db", arguments[1].hasPrefix("/") else {
            FileHandle.standardError.write(Data("Usage: smcli-service --db <absolute path to history.db>\n".utf8))
            exit(2)
        }
        let environment = ProcessInfo.processInfo.environment
        let database = URL(fileURLWithPath: arguments[1])
        let location = ServiceLocation(database: database, environment: environment)
        do {
            try location.prepare()
            // Only the lock holder binds, so concurrent starts can't steal each other's socket.
            // The lock is released by the kernel when this process exits, however it exits.
            let lock = open(location.lock.path, O_RDWR | O_CREAT | O_NOFOLLOW | O_CLOEXEC, mode_t(0o600))
            guard lock >= 0 else { throw HistoryError(message: "Cannot open \(location.lock.path)") }
            guard flock(lock, LOCK_EX | LOCK_NB) == 0 else { exit(0) }
            // Never open (and so create or write) the database when it is the starting shell's native history.
            let history = CapturedHistory(url: database)
            try history.checkSeparate(from: environment["HISTFILE"] ?? "")
            let service = try Service(history: history, location: location,
                                      idle: environment["SMCLI_SERVICE_IDLE"].flatMap(Double.init) ?? 1800)
            try service.run()
        } catch {
            FileHandle.standardError.write(Data("smcli-service: \(error.localizedDescription)\n".utf8))
            exit(1)
        }
    }
}

/// All history state, touched only on `queue`.
final class Service: @unchecked Sendable {
    private let history: CapturedHistory
    private let location: ServiceLocation
    private let idle: Double
    private let queue = DispatchQueue(label: "smcli.service.state")
    private let db: SQLiteDatabase
    private let user = NSUserName()
    /// Stamped on every response so clients can tell this service is from an older build.
    private let build = Bundle.main.executableURL.flatMap(ServiceProtocol.build)
    /// Built on the first suggest and dropped whenever another process changes the database.
    private var index: SuggestionIndex?
    private var dataVersion: Int
    private var lastActivity = Date()
    private var socketInode: ino_t = 0
    private let connections = NSLock()
    private var active = 0
    /// Search state. The corpus is rebuilt after any write, the engine when the config changes; the index stays
    /// open across both. Searches run one at a time, off `queue`, so records aren't held up by one.
    private let searching = NSLock()
    private let open = OpenIndex()
    private var engine: (config: SmcliConfig, search: HistorySearch)?
    private var corpus: SearchCorpus?

    init(history: CapturedHistory, location: ServiceLocation, idle: Double) throws {
        self.history = history
        self.location = location
        self.idle = idle
        db = try history.openConnection()
        dataVersion = try db.dataVersion()
        fileSearch = makeFileSearch()
    }

    func run() throws {
        let listener = try listenUnix(path: location.socket.path)
        var info = stat()
        if stat(location.socket.path, &info) == 0 { socketInode = info.st_ino }
        for signal in [SIGTERM, SIGINT, SIGHUP] {
            Foundation.signal(signal, SIG_IGN)
            let source = DispatchSource.makeSignalSource(signal: signal, queue: queue)
            source.setEventHandler { [self] in stop() }
            source.resume()
            retained.append(source)
        }
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + 15, repeating: 15)
        timer.setEventHandler { [self] in
            // Idle too long, or our socket was removed or replaced: let a fresh service take over.
            var info = stat()
            fileSearch.pool.evictIdle()
            if Date().timeIntervalSince(lastActivity) > idle
                || stat(location.socket.path, &info) != 0 || info.st_ino != socketInode {
                stop()
            }
        }
        timer.resume()
        retained.append(timer)
        Thread.detachNewThread { [self] in
            while true {
                let fd = accept(listener, nil, nil)
                // Out of descriptors: back off instead of spinning until one frees up.
                if fd < 0 { if errno == EMFILE || errno == ENFILE { usleep(10_000) }; continue }
                _ = fcntl(fd, F_SETFD, FD_CLOEXEC)
                DispatchQueue.global().async { [self] in serve(fd) }
            }
        }
        dispatchMain()
    }

    private var retained: [any DispatchSourceProtocol] = []

    /// Directory context from cache: discovery is a few stats, and Git status refreshes in the background.
    private let contexts = ContextCache()

    /// fff searches: at most `SMCLI_FFF_MAX` instances (4), each dropped after `SMCLI_FFF_IDLE` idle seconds (600).
    private var fileSearch: FileSearch!

    private func makeFileSearch() -> FileSearch {
        let environment = ProcessInfo.processInfo.environment
        let pool = FFFPool(capacity: environment["SMCLI_FFF_MAX"].flatMap(Int.init) ?? 4,
                           idleTimeout: environment["SMCLI_FFF_IDLE"].flatMap(Double.init) ?? 600)
        return FileSearch(pool: pool, contexts: contexts,
                          scores: { [self] root in
                              queue.sync { (try? history.selectionScores(root: root, on: db)) ?? [:] }
                          },
                          record: { [self] root, path, mode, query in
                              try queue.sync {
                                  lastActivity = Date()
                                  try history.recordSelection(root: root, path: path, mode: mode, query: query, on: db)
                              }
                          })
    }

    /// Runs a file operation off `queue`, keeping the service alive while it works.
    private func files(_ request: ServiceRequest, _ work: (FileSearch) -> ServiceResponse) -> ServiceResponse {
        queue.sync { lastActivity = Date() }
        let response = work(fileSearch)
        queue.sync { lastActivity = Date() }
        return response
    }

    private func context(_ request: ServiceRequest) -> ServiceResponse {
        queue.sync { lastActivity = Date() }
        guard let directory = request.directory, directory.hasPrefix("/") else {
            return .failure("bad_request", "context needs an absolute directory.")
        }
        var response = ServiceResponse(ok: true)
        response.context = contexts.context(for: directory)
        return response
    }

    /// Removes the socket only while it is still ours, then exits (which releases the lock).
    private func stop() -> Never {
        var info = stat()
        if stat(location.socket.path, &info) == 0 && info.st_ino == socketInode { unlink(location.socket.path) }
        exit(0)
    }

    private func serve(_ fd: Int32) {
        let connection = SocketConnection(fd: fd)
        guard connection.peerUID() == getuid() else { return }
        connections.lock()
        active += 1
        let admitted = active <= ServiceProtocol.maxConnections
        connections.unlock()
        defer { connections.lock(); active -= 1; connections.unlock() }
        var shutdown = false
        var response: ServiceResponse
        if !admitted {
            response = .failure("busy", "Too many connections.")
        } else {
            do {
                guard let line = try connection.readLine(limit: ServiceProtocol.maxMessageBytes,
                                                         timeout: ServiceProtocol.ioTimeout) else { return }
                if let request = try? JSONDecoder().decode(ServiceRequest.self, from: line) {
                    shutdown = request.op == .shutdown && request.v == ServiceProtocol.version
                    // Search and context run off `queue`: one takes long, the other must never wait behind a record.
                    let current = request.v == ServiceProtocol.version
                    response = !current ? queue.sync { handle(request) }
                        : request.op == .search ? search(request)
                        : request.op == .context ? context(request)
                        : request.op == .files ? files(request) { $0.files(request) }
                        : request.op == .grep ? files(request) { $0.grep(request) }
                        : request.op == .select && ["suggest", "search"].contains(request.mode) ? selectCommand(request)
                        : request.op == .select ? files(request) { $0.select(request) }
                        : request.op == .suggest || request.op == .pick ? ranked(request)
                        : queue.sync { handle(request) }
                } else {
                    response = .failure("bad_request", "Malformed request.")
                }
            } catch SocketError.tooLarge {
                response = .failure("too_large", "Request exceeds \(ServiceProtocol.maxMessageBytes) bytes.")
            } catch SocketError.timeout {
                response = .failure("timeout", "Request not received within \(ServiceProtocol.ioTimeout) s.")
            } catch {
                return
            }
        }
        response.build = build
        if var data = try? JSONEncoder().encode(response) {
            data.append(0x0A)
            try? connection.write(data, timeout: ServiceProtocol.ioTimeout)
        }
        if shutdown { queue.sync { stop() } }
    }

    /// Directory context is read before taking `queue` (discovery stats files and may parse a manifest), so a
    /// record never waits behind it.
    private func ranked(_ request: ServiceRequest) -> ServiceResponse {
        let context = request.directory.flatMap { $0.hasPrefix("/") ? rankingContext(request, directory: $0) : nil }
        return queue.sync { handle(request, context: context) }
    }

    private func rankingContext(_ request: ServiceRequest, directory: String) -> RankingContext {
        RankingContext(project: contexts.context(for: directory), session: request.session,
                       environment: ProcessInfo.processInfo.environment)
    }

    /// Remembers which suggestion or search result the user chose for what they typed. Best effort for the picker.
    private func selectCommand(_ request: ServiceRequest) -> ServiceResponse {
        guard let command = request.command, let directory = request.directory, directory.hasPrefix("/"),
              let mode = request.mode, !command.isEmpty, command.utf8.count <= 1 << 16, !isSmcliCommand(command) else {
            return .failure("bad_request", "select needs a command, a directory and a mode of suggest or search.")
        }
        let query = String((request.query ?? "").prefix(1000))
        return queue.sync {
            lastActivity = Date()
            do {
                // An unreadable config records nothing rather than skipping the filter.
                let filter = try CaptureFilter(config: SmcliConfig.load(from: configURL(request)))
                try refresh()
                let time = Date().timeIntervalSince1970
                if try history.recordPick(query: query, command: command, kind: mode, directory: directory, filter: filter,
                                          at: time, on: db) {
                    index?.addPick(query: query, command: command, at: time)
                }
                return ServiceResponse(ok: true)
            } catch {
                return .failure("failed", error.localizedDescription)
            }
        }
    }

    private func handle(_ request: ServiceRequest, context: RankingContext? = nil) -> ServiceResponse {
        lastActivity = Date()
        guard request.v == ServiceProtocol.version else {
            return .failure("version", "Protocol version \(ServiceProtocol.version) required.")
        }
        do {
            switch request.op {
            case .ping, .shutdown:
                var response = ServiceResponse(ok: true)
                response.pid = getpid()
                response.indexes = fileSearch.pool.instances.map { FileIndexInfo(id: $0.id, root: $0.root) }
                return response
            case .record:
                return try record(request)
            case .suggest:
                return try suggest(request, context: context)
            case .pick:
                return try pick(request, context: context)
            case .search, .context, .files, .grep, .select:
                return .failure("bad_request", "\(request.op.rawValue) is not served on the state queue.")
            }
        } catch {
            return .failure("failed", error.localizedDescription)
        }
    }

    /// Acknowledged only after the insert commits. A retried event ID is acknowledged again without a new row.
    private func record(_ request: ServiceRequest) throws -> ServiceResponse {
        guard let event = request.event, let command = request.command, let shell = request.shell,
              let session = request.session, let directory = request.directory, let exitCode = request.exitCode else {
            return .failure("bad_request", "record needs event, command, shell, session, directory and exitCode.")
        }
        try refresh()
        let started = request.startedAt ?? Date().timeIntervalSince1970
        let inserted = try history.append(command: command, shell: shell, directory: directory, exitCode: exitCode,
                                          nativeFile: request.nativeFile ?? "", session: session, startedAt: started,
                                          endedAt: request.endedAt, pipestatus: request.pipestatus,
                                          event: event, on: db)
        if inserted {
            let run = CapturedHistory.Run(command: command, directory: directory, startedAt: started, exitCode: exitCode,
                                          session: session)
            index?.add(run)
            // The corpus is updated in place (a rebuild is ~230 ms at 100k runs); an unused one just stays valid.
            corpus?.state.add(run)
        }
        var response = ServiceResponse(ok: true)
        response.event = event
        response.duplicate = !inserted
        return response
    }

    private func suggest(_ request: ServiceRequest, context: RankingContext?) throws -> ServiceResponse {
        guard let directory = request.directory, directory.hasPrefix("/") else {
            return .failure("bad_request", "suggest needs an absolute directory.")
        }
        let limit = request.limit ?? 10
        guard (1...1000).contains(limit) else { return .failure("bad_request", "limit must be 1 to 1000.") }
        var response = ServiceResponse(ok: true)
        response.suggestions = try currentIndex().suggestions(directory: directory, prefix: request.prefix ?? "", limit: limit,
                                                              context: context)
        return response
    }

    /// The picker's first draw: this directory's suggestions, then all history by last run.
    private func pick(_ request: ServiceRequest, context: RankingContext?) throws -> ServiceResponse {
        guard let directory = request.directory, directory.hasPrefix("/") else {
            return .failure("bad_request", "pick needs an absolute directory.")
        }
        let limit = request.limit ?? 50
        guard (1...1000).contains(limit) else { return .failure("bad_request", "limit must be 1 to 1000.") }
        let index = try currentIndex()
        var response = ServiceResponse(ok: true)
        response.suggestions = index.suggestions(directory: directory, limit: limit, context: context)
        response.recent = index.recent()
        return response
    }

    private func currentIndex() throws -> SuggestionIndex {
        try refresh()
        if index == nil { index = SuggestionIndex(runs: try history.runs(user: user, on: db), picks: try history.picks(on: db)) }
        return index!
    }

    /// Another process (a fallback record, an import, prune) changed the database: rebuild on next use.
    private func refresh() throws {
        let version = try db.dataVersion()
        if version != dataVersion {
            dataVersion = version
            index = nil
            corpus = nil
        }
    }

    private var home: URL {
        ProcessInfo.processInfo.environment["HOME"].map { URL(fileURLWithPath: $0) }
            ?? FileManager.default.homeDirectoryForCurrentUser
    }

    /// The caller's config file; requests from older clients fall back to the one this service started with.
    private func configURL(_ request: ServiceRequest) -> URL {
        if let path = request.config, path.hasPrefix("/") { return URL(fileURLWithPath: path) }
        return SmcliConfig.location(home: home, environment: ProcessInfo.processInfo.environment)
    }

    // MARK: - Search

    #if canImport(ProximaKit)

    /// Blocks this connection's thread, not `queue`, for the length of the search.
    private func search(_ request: ServiceRequest) -> ServiceResponse {
        searching.lock()
        defer { searching.unlock() }
        let done = DispatchSemaphore(value: 0)
        let result = ResultBox()
        Task {
            result.response = await searchNow(request)
            done.signal()
        }
        done.wait()
        return result.response
    }

    private func searchNow(_ request: ServiceRequest) async -> ServiceResponse {
        queue.sync { lastActivity = Date() }
        guard let query = request.query, let directory = request.directory, directory.hasPrefix("/") else {
            return .failure("bad_request", "search needs a query and an absolute directory.")
        }
        let limit = request.limit ?? 10
        guard (1...1000).contains(limit) else { return .failure("bad_request", "limit must be 1 to 1000.") }
        do {
            let context = rankingContext(request, directory: directory)
            let (engine, corpus, signals) = try queue.sync { () throws -> (HistorySearch, SearchCorpus, SearchSignals) in
                try refresh()
                let index = try currentIndex()
                var signals = SearchSignals(context: context, picks: index.picks)
                if context.uses(.transition), let previous = request.session.flatMap(index.previousCommand) {
                    (signals.previous, signals.followers) = (previous, index.following(previous))
                }
                return (try currentEngine(request), try currentCorpus(request, directory: directory, project: context.uses(.project) ? context.projectRoot : nil),
                        signals)
            }
            var response = ServiceResponse(ok: true)
            response.hits = try await engine.search(query, corpus: corpus.state.entries, limit: limit,
                                                    filter: request.filter ?? SearchFilter(), programs: corpus.state.programs,
                                                    open: open, signals: signals)
            return response
        } catch {
            return .failure("failed", error.localizedDescription)
        }
    }

    /// The caller's view of the environment: its HISTFILE and SHELL pick the native history files.
    private func environment(for request: ServiceRequest) -> [String: String] {
        var environment = ProcessInfo.processInfo.environment
        environment["HISTFILE"] = request.histfile
        environment["SHELL"] = request.loginShell
        return environment
    }

    private func currentEngine(_ request: ServiceRequest) throws -> HistorySearch {
        var environment = ProcessInfo.processInfo.environment
        environment["SMCLI_CONFIG"] = configURL(request).path
        let config = try SmcliConfig.load(from: configURL(request))
        if let engine, engine.config == config { return engine.search }
        let search = try searchEngine(captured: history, home: home, environment: environment)
        engine = (config, search)
        return search
    }

    /// Reused for the same directory, project and native files, for up to five minutes (frecency drifts with time).
    /// A record updates it in place; anything else that writes the database drops it.
    private func currentCorpus(_ request: ServiceRequest, directory: String, project: String?) throws -> SearchCorpus {
        let environment = environment(for: request)
        let files = try historyFiles(home: home, environment: environment)
        let stamp = files.map { file -> String in
            let values = try? file.url.resourceValues(forKeys: [.fileSizeKey, .contentModificationDateKey])
            return "\(file.url.path) \(values?.fileSize ?? -1) \(values?.contentModificationDate?.timeIntervalSince1970 ?? 0)"
        }
        if let corpus, corpus.state.directory == directory, corpus.state.project == project, corpus.native == stamp,
           Date().timeIntervalSince(corpus.built) < 300 {
            return corpus
        }
        let native = files.flatMap { (try? $0.read())?.components(separatedBy: "\n") ?? [] }
        let now = Date().timeIntervalSince1970
        let state = CorpusState(captured: try history.commandStats(directory: directory, project: project, now: now),
                                native: native, directory: directory, project: project, asOf: now)
        let built = SearchCorpus(native: stamp, built: Date(), state: state)
        corpus = built
        return built
    }
    #else
    private func search(_ request: ServiceRequest) -> ServiceResponse {
        .failure("unsupported", "search requires macOS 14 or later.")
    }
    #endif
}

struct SearchCorpus {
    /// Path, size and modification time of each native history file read.
    let native: [String]
    let built: Date
    var state: CorpusState
}

private final class ResultBox: @unchecked Sendable {
    var response = ServiceResponse.failure("failed", "Search did not finish.")
}
