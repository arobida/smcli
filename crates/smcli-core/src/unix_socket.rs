use std::fmt;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt, net::{UnixListener, UnixStream}};
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq, Eq)]
pub enum SocketError { TooLarge, Timeout, Closed, System(String) }
impl fmt::Display for SocketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TooLarge => "Message too large.",
            Self::Timeout => "Timed out.",
            Self::Closed => "Connection closed mid-message.",
            Self::System(message) => message,
        })
    }
}
impl std::error::Error for SocketError {}
fn system(operation: &str, error: io::Error) -> SocketError {
    if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) {
        SocketError::Timeout
    } else {
        SocketError::System(format!("{operation}: {error}"))
    }
}

/// One connected stream socket, closed on drop. Reads and writes share a deadline, so a peer that trickles
/// bytes can't hold the connection past `timeout`.
pub struct SocketConnection { stream: UnixStream }
impl From<UnixStream> for SocketConnection {
    fn from(stream: UnixStream) -> Self { Self { stream } }
}
impl SocketConnection {
    /// The connected process's user, from the kernel rather than anything it sent.
    pub fn peer_uid(&self) -> Option<u32> {
        unsafe {
            #[cfg(target_os = "macos")]
            {
                let (mut uid, mut gid) = (0, 0);
                (libc::getpeereid(self.stream.as_raw_fd(), &mut uid, &mut gid) == 0).then_some(uid)
            }
            #[cfg(target_os = "linux")]
            {
                let mut credentials: libc::ucred = std::mem::zeroed();
                let mut length = std::mem::size_of_val(&credentials) as libc::socklen_t;
                (libc::getsockopt(self.stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED,
                    &mut credentials as *mut _ as *mut _, &mut length) == 0).then_some(credentials.uid)
            }
        }
    }

    /// Reads one line, without its newline. None when the peer closed before sending anything.
    pub fn read_line(&mut self, limit: usize, timeout: Duration) -> Result<Option<Vec<u8>>, SocketError> {
        let deadline = Instant::now() + timeout;
        let mut buffer = Vec::new();
        let mut chunk = [0; 64 * 1024];
        loop {
            let _ = self.stream.set_read_timeout(Some(remaining(deadline)?));
            let count = match self.stream.read(&mut chunk) {
                Ok(0) => return if buffer.is_empty() { Ok(None) } else { Err(SocketError::Closed) },
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(system("read", e)),
            };
            let newline = chunk[..count].iter().position(|&b| b == b'\n');
            buffer.extend_from_slice(&chunk[..newline.unwrap_or(count)]);
            if buffer.len() >= limit { return Err(SocketError::TooLarge); }
            if newline.is_some() { return Ok(Some(buffer)); }
        }
    }

    pub fn write(&mut self, mut data: &[u8], timeout: Duration) -> Result<(), SocketError> {
        let deadline = Instant::now() + timeout;
        while !data.is_empty() {
            let _ = self.stream.set_write_timeout(Some(remaining(deadline)?));
            // UnixStream suppresses SIGPIPE when a peer hangs up mid-write.
            match self.stream.write(data) {
                Ok(0) => return Err(SocketError::Closed),
                Ok(n) => data = &data[n..],
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(system("write", e)),
            }
        }
        Ok(())
    }
}
fn remaining(deadline: Instant) -> Result<Duration, SocketError> {
    deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero())
        .map(|d| d.max(Duration::from_micros(1))).ok_or(SocketError::Timeout)
}
fn check_path(path: &Path) -> Result<(), SocketError> {
    let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let bytes = path.as_os_str().as_bytes().len();
    if bytes >= address.sun_path.len() {
        return Err(SocketError::System(format!("Socket path is too long ({bytes} bytes): {}", path.display())));
    }
    Ok(())
}
/// Binds and listens at `path`, replacing whatever socket file is there. Callers hold the service lock first,
/// so the file being replaced belongs to a service that is no longer running.
pub fn listen_unix(path: &Path) -> anyhow::Result<UnixListener> {
    check_path(path)?;
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path).map_err(|e| system("bind", e))?;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    if unsafe { libc::listen(listener.as_raw_fd(), 128) } != 0 {
        return Err(system("listen", io::Error::last_os_error()).into());
    }
    Ok(listener)
}
pub fn connect_unix(path: &Path) -> anyhow::Result<SocketConnection> {
    check_path(path)?;
    Ok(UnixStream::connect(path).map_err(|e| system("connect", e))?.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    const TIMEOUT: Duration = Duration::from_millis(40);
    #[test]
    fn line_limits_closed_and_write() {
        for (payload, limit, expected) in [
            (b"abc\n".as_slice(), 4, Ok(Some(b"abc".to_vec()))),
            (b"abc\n", 3, Err(SocketError::TooLarge)),
            (b"abc", 4, Err(SocketError::Closed)),
            (b"", 4, Ok(None)),
            (b"\n", 1, Ok(Some(vec![]))),
            (b"abc", 3, Err(SocketError::TooLarge)),
        ] {
            let (a, b) = UnixStream::pair().unwrap();
            let mut writer = SocketConnection::from(a);
            writer.write(payload, TIMEOUT).unwrap();
            drop(writer);
            assert_eq!(SocketConnection::from(b).read_line(limit, TIMEOUT), expected);
        }
    }
    #[test]
    fn timeout_and_peer_uid() {
        let (_a, b) = UnixStream::pair().unwrap();
        let mut connection = SocketConnection::from(b);
        assert_eq!(connection.peer_uid(), Some(unsafe { libc::getuid() }));
        assert_eq!(connection.read_line(10, TIMEOUT), Err(SocketError::Timeout));
        assert_eq!(connection.read_line(10, Duration::ZERO), Err(SocketError::Timeout));
    }
    #[test]
    fn replaces_stale_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("socket");
        drop(UnixListener::bind(&path).unwrap());
        let listener = listen_unix(&path).unwrap();
        let mut client = connect_unix(&path).unwrap();
        client.write(b"ok\n", TIMEOUT).unwrap();
        let mut server = SocketConnection::from(listener.accept().unwrap().0);
        assert_eq!(server.read_line(3, TIMEOUT).unwrap(), Some(b"ok".to_vec()));
        assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
