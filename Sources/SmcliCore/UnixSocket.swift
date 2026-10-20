import Foundation
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif

package enum SocketError: LocalizedError, Equatable {
    case tooLarge, timeout, closed
    case system(String)

    package var errorDescription: String? {
        switch self {
        case .tooLarge: "Message too large."
        case .timeout: "Timed out."
        case .closed: "Connection closed mid-message."
        case .system(let message): message
        }
    }

    static func current(_ what: String) -> SocketError { .system("\(what): \(String(cString: strerror(errno)))") }
}

/// One connected stream socket, closed on deinit. Reads and writes share a deadline, so a peer that trickles
/// bytes can't hold the connection past `timeout`.
package final class SocketConnection {
    package let fd: Int32

    package init(fd: Int32) {
        self.fd = fd
        #if canImport(Darwin)
        // A peer that hangs up mid-write must cost an error, not a SIGPIPE that kills the process.
        var on: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
        #endif
    }

    deinit { close(fd) }

    /// The connected process's user, from the kernel rather than anything it sent.
    package func peerUID() -> uid_t? {
        #if canImport(Darwin)
        var uid = uid_t(), gid = gid_t()
        return getpeereid(fd, &uid, &gid) == 0 ? uid : nil
        #else
        var credentials = ucred()
        var length = socklen_t(MemoryLayout<ucred>.size)
        return getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &credentials, &length) == 0 ? credentials.uid : nil
        #endif
    }

    /// Reads one line, without its newline. Nil when the peer closed before sending anything.
    package func readLine(limit: Int, timeout: Double) throws -> Data? {
        let deadline = Date().addingTimeInterval(timeout)
        var buffer = Data()
        var chunk = [UInt8](repeating: 0, count: 64 * 1024)
        while true {
            try setTimeout(SO_RCVTIMEO, until: deadline)
            let count = read(fd, &chunk, chunk.count)
            if count < 0 {
                if errno == EINTR { continue }
                throw errno == EAGAIN || errno == EWOULDBLOCK ? SocketError.timeout : SocketError.current("read")
            }
            if count == 0 {
                if buffer.isEmpty { return nil }
                throw SocketError.closed
            }
            if let newline = chunk[..<count].firstIndex(of: 0x0A) {
                buffer.append(contentsOf: chunk[..<newline])
                guard buffer.count < limit else { throw SocketError.tooLarge }
                return buffer
            }
            buffer.append(contentsOf: chunk[..<count])
            guard buffer.count < limit else { throw SocketError.tooLarge }
        }
    }

    package func write(_ data: Data, timeout: Double) throws {
        let deadline = Date().addingTimeInterval(timeout)
        try data.withUnsafeBytes { bytes in
            var offset = 0
            while offset < bytes.count {
                try setTimeout(SO_SNDTIMEO, until: deadline)
                #if canImport(Darwin)
                let written = send(fd, bytes.baseAddress! + offset, bytes.count - offset, 0)
                #else
                let written = send(fd, bytes.baseAddress! + offset, bytes.count - offset, Int32(MSG_NOSIGNAL))
                #endif
                if written < 0 {
                    if errno == EINTR { continue }
                    throw errno == EAGAIN || errno == EWOULDBLOCK ? SocketError.timeout : SocketError.current("write")
                }
                offset += written
            }
        }
    }

    private func setTimeout(_ option: Int32, until deadline: Date) throws {
        let remaining = deadline.timeIntervalSinceNow
        guard remaining > 0 else { throw SocketError.timeout }
        var value = timeval(tv_sec: Int(remaining), tv_usec: Int32((remaining - remaining.rounded(.down)) * 1_000_000))
        // A zero timeval means "never time out".
        if value.tv_sec == 0 && value.tv_usec == 0 { value.tv_usec = 1 }
        setsockopt(fd, SOL_SOCKET, option, &value, socklen_t(MemoryLayout<timeval>.size))
    }
}

private func address(_ path: String) throws -> sockaddr_un {
    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    let bytes = Array(path.utf8)
    let capacity = MemoryLayout.size(ofValue: address.sun_path)
    guard bytes.count < capacity else { throw SocketError.system("Socket path is too long (\(bytes.count) bytes): \(path)") }
    withUnsafeMutableBytes(of: &address.sun_path) { buffer in
        buffer.copyBytes(from: bytes)
        buffer[bytes.count] = 0
    }
    return address
}

private func streamSocket() throws -> Int32 {
    #if canImport(Darwin)
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    #else
    let fd = socket(AF_UNIX, Int32(SOCK_STREAM.rawValue), 0)
    #endif
    guard fd >= 0 else { throw SocketError.current("socket") }
    _ = fcntl(fd, F_SETFD, FD_CLOEXEC)
    return fd
}

/// Binds and listens at `path`, replacing whatever socket file is there. Callers hold the service lock first,
/// so the file being replaced belongs to a service that is no longer running.
package func listenUnix(path: String) throws -> Int32 {
    var address = try address(path)
    let fd = try streamSocket()
    unlink(path)
    let bound = withUnsafePointer(to: &address) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
    }
    guard bound == 0 else { let error = SocketError.current("bind"); close(fd); throw error }
    chmod(path, 0o600)
    guard listen(fd, 128) == 0 else { let error = SocketError.current("listen"); close(fd); throw error }
    return fd
}

package func connectUnix(path: String) throws -> SocketConnection {
    var address = try address(path)
    let connection = SocketConnection(fd: try streamSocket())
    let connected = withUnsafePointer(to: &address) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            connect(connection.fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
        }
    }
    guard connected == 0 else { throw SocketError.current("connect") }
    return connection
}
