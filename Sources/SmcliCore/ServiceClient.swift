import Foundation
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif

/// Talks to the smcli-service that owns one history database. Every call returns nil rather than throwing when
/// the service can't be used, so callers fall back to doing the work in-process.
package struct ServiceClient {
    package enum Mode {
        /// Never contact a service.
        case off
        /// Use a running service, but don't start one.
        case connect
        /// Use the service, starting it when none is running.
        case spawn
    }

    package let location: ServiceLocation
    package let database: URL
    package let mode: Mode
    let executable: URL?
    let environment: [String: String]
    /// Sent with every request: the service reads this caller's config, not the one it was started with.
    let config: String

    /// The mode comes from `SMCLI_SERVICE`: unset (or `1`/`on`) starts the service as needed, `connect` only uses a
    /// running one, and `0`/`off` keeps every command in-process.
    package init(database: URL, environment: [String: String], cliExecutable: URL?) {
        self.database = database
        self.environment = environment
        location = ServiceLocation(database: database, environment: environment)
        let home = environment["HOME"].map { URL(fileURLWithPath: $0) } ?? FileManager.default.homeDirectoryForCurrentUser
        config = SmcliConfig.location(home: home, environment: environment).path
        switch environment["SMCLI_SERVICE"]?.lowercased() {
        case "0", "off", "false", "no": mode = .off
        case "connect": mode = .connect
        default: mode = .spawn
        }
        executable = environment["SMCLI_SERVICE_BIN"].flatMap { $0.hasPrefix("/") ? URL(fileURLWithPath: $0) : nil }
            ?? cliExecutable.map(Self.serviceExecutable)
    }

    /// The service built next to this CLI: `smcli` → `smcli-service`, `smcli-<build>` → `smcli-service-<build>`.
    static func serviceExecutable(beside cli: URL) -> URL {
        let resolved = cli.resolvingSymlinksInPath()
        let name = resolved.lastPathComponent
        let service = name.hasPrefix("smcli") ? "smcli-service" + name.dropFirst("smcli".count) : "smcli-service"
        return resolved.deletingLastPathComponent().appendingPathComponent(service)
    }

    /// Sends one request. Nil means unreachable, timed out, or speaking another protocol version.
    /// `timeout` bounds the wait for the reply; the first search after startup may build the whole index.
    package func send(_ request: ServiceRequest, spawn: Bool? = nil, timeout: Double = 10) -> ServiceResponse? {
        guard mode != .off || spawn != nil else { return nil }
        let spawning = spawn ?? (mode == .spawn)
        guard let response = exchange(request, spawn: spawning, timeout: timeout) else { return nil }
        guard spawning, request.op != .shutdown, let expected = executable.flatMap(ServiceProtocol.build),
              response.build != expected else { return response }
        // Left running by another build (an upgrade, or one from before services reported their build):
        // stop it, wait for it to release its lock, and let this request start the current one.
        guard let old = exchange(ServiceRequest(op: .shutdown), spawn: false, timeout: 2) else { return response }
        if let pid = old.pid {
            let deadline = Date().addingTimeInterval(2)
            while kill(pid, 0) == 0 && Date() < deadline { usleep(10_000) }
        }
        // The old service already acted on this request. Only a request that is safe to repeat is sent again:
        // a select would count the pick twice (a record is safe, its event ID makes a retry a no-op).
        guard Self.repeatable(request.op) else { return response }
        return exchange(request, spawn: true, timeout: timeout)
    }

    /// Requests the old service's answer can be replaced by the new one's without a second effect.
    static func repeatable(_ op: ServiceRequest.Operation) -> Bool {
        switch op {
        case .select, .shutdown: false
        case .ping, .record, .suggest, .pick, .search, .context, .files, .grep: true
        }
    }

    private func exchange(_ request: ServiceRequest, spawn: Bool, timeout: Double) -> ServiceResponse? {
        var request = request
        if request.config == nil { request.config = config }
        guard let connection = connect(spawn: spawn) else { return nil }
        // The private directory already keeps others out; this also refuses a socket some other user planted.
        guard connection.peerUID() == getuid() else { return nil }
        do {
            var line = try JSONEncoder().encode(request)
            line.append(0x0A)
            guard line.count <= ServiceProtocol.maxMessageBytes else { return nil }
            try connection.write(line, timeout: ServiceProtocol.ioTimeout)
            // The first suggest after startup loads history, so allow longer than a plain I/O timeout.
            guard let reply = try connection.readLine(limit: ServiceProtocol.maxResponseBytes, timeout: timeout) else { return nil }
            let response = try JSONDecoder().decode(ServiceResponse.self, from: reply)
            guard response.v == ServiceProtocol.version, response.error?.code != "version" else { return nil }
            return response
        } catch {
            return nil
        }
    }

    private func connect(spawn: Bool) -> SocketConnection? {
        if let connection = try? connectUnix(path: location.socket.path) { return connection }
        guard spawn, launch() else { return nil }
        // The winner of a concurrent start binds within milliseconds; losers exit and we reach the winner.
        let deadline = Date().addingTimeInterval(2)
        while Date() < deadline {
            if let connection = try? connectUnix(path: location.socket.path) { return connection }
            usleep(10_000)
        }
        return nil
    }

    /// Starts the service detached in its own session, with no terminal and no inherited descriptors.
    private func launch() -> Bool {
        guard let executable, FileManager.default.isExecutableFile(atPath: executable.path),
              (try? location.prepare()) != nil else { return false }
        #if canImport(Darwin)
        var attributes: posix_spawnattr_t? = nil
        var actions: posix_spawn_file_actions_t? = nil
        #else
        var attributes = posix_spawnattr_t()
        var actions = posix_spawn_file_actions_t()
        #endif
        posix_spawnattr_init(&attributes)
        posix_spawn_file_actions_init(&actions)
        defer {
            posix_spawnattr_destroy(&attributes)
            posix_spawn_file_actions_destroy(&actions)
        }
        #if canImport(Darwin)
        posix_spawnattr_setflags(&attributes, Int16(POSIX_SPAWN_SETSID | POSIX_SPAWN_CLOEXEC_DEFAULT))
        #else
        posix_spawnattr_setflags(&attributes, Int16(POSIX_SPAWN_SETSID))
        #endif
        for fd: Int32 in 0...2 {
            posix_spawn_file_actions_addopen(&actions, fd, "/dev/null", fd == 0 ? O_RDONLY : O_WRONLY, 0)
        }
        let arguments = [executable.path, "--db", database.path]
        let argv = arguments.map { strdup($0) } + [nil]
        let envp = environment.map { strdup("\($0.key)=\($0.value)") } + [nil]
        defer { (argv + envp).forEach { free($0) } }
        var pid = pid_t()
        return posix_spawn(&pid, executable.path, &actions, &attributes, argv, envp) == 0
    }
}
