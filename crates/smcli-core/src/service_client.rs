use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::caller::CallerContext;
use crate::service_location::{ServiceLocation, build, build_parts};
use crate::service_protocol::{
    self, Operation, ServiceRequest, ServiceResponse, decode_line, encode_line,
};
use crate::unix_socket::{SocketConnection, connect_unix};

#[derive(PartialEq)]
enum Mode {
    Off,
    Connect,
    Spawn,
}

#[derive(Debug)]
enum ExchangeError {
    Unavailable,
    /// The peer answered in another protocol version. `send` replaces that service.
    Mismatch(ServiceResponse),
}

/// Talks to the smcli-service that owns one history database. Every call returns None rather than throwing when
/// the service can't be used, so callers fall back to doing the work in-process.
pub struct ServiceClient {
    location: ServiceLocation,
    database: PathBuf,
    mode: Mode,
    executable: PathBuf,
    environment: HashMap<String, String>,
    caller: CallerContext,
}
impl ServiceClient {
    pub fn socket(&self) -> &std::path::Path {
        &self.location.socket
    }

    /// The mode comes from `SMCLI_SERVICE`: unset (or `1`/`on`) starts the service as needed, `connect` only uses a
    /// running one, and `0`/`off` keeps every command in-process.
    pub fn new(database: &Path, environment: &HashMap<String, String>, cli: &Path) -> Self {
        let mode = match environment
            .get("SMCLI_SERVICE")
            .map(|s| s.to_lowercase())
            .as_deref()
        {
            Some("0" | "off" | "false" | "no") => Mode::Off,
            Some("connect") => Mode::Connect,
            _ => Mode::Spawn,
        };
        Self {
            location: ServiceLocation::new(database, environment),
            database: database.to_owned(),
            mode,
            executable: environment
                .get("SMCLI_SERVICE_BIN")
                .filter(|p| p.starts_with('/'))
                .map(PathBuf::from)
                .unwrap_or_else(|| Self::service_executable(cli)),
            environment: environment.clone(),
            caller: CallerContext::from_env(environment),
        }
    }

    /// The service built next to this CLI: `smcli` → `smcli-service`, `smcli-<build>` → `smcli-service-<build>`.
    fn service_executable(cli: &Path) -> PathBuf {
        let resolved = std::fs::canonicalize(cli).unwrap_or_else(|_| cli.to_owned());
        let name = resolved.file_name().unwrap_or_default().to_string_lossy();
        let suffix = name.strip_prefix("smcli").unwrap_or("");
        resolved.with_file_name(format!("smcli-service{suffix}"))
    }

    /// Sends one request. None means unreachable or timed out. A service speaking another protocol
    /// version is stopped and replaced, the same way a stale build is.
    /// `timeout` bounds the wait for the reply; the first search after startup may build the whole index.
    pub fn send(
        &self,
        request: &ServiceRequest,
        spawn: Option<bool>,
        timeout: Duration,
    ) -> Option<ServiceResponse> {
        if self.mode == Mode::Off && spawn.is_none() {
            return None;
        }
        let spawning = spawn.unwrap_or(self.mode == Mode::Spawn);
        let response = match self.exchange(request, spawning, timeout) {
            Ok(response) => response,
            Err(ExchangeError::Mismatch(stale))
                if spawning || request.op == Operation::Shutdown =>
            {
                return self.replace(request, stale, spawning, timeout);
            }
            Err(_) => return None,
        };
        if !spawning || request.op == Operation::Shutdown {
            return Some(response);
        }
        let Some(expected) = build(&self.executable) else {
            return Some(response);
        };
        if !should_replace(response.build.as_deref(), &expected) {
            return Some(response);
        }
        self.replace(request, response, spawning, timeout)
    }

    /// Stop a service left by another build or protocol, wait for its lock, and repeat this request on the current one.
    fn replace(
        &self,
        request: &ServiceRequest,
        stale: ServiceResponse,
        spawning: bool,
        timeout: Duration,
    ) -> Option<ServiceResponse> {
        let mut shutdown = ServiceRequest::new(Operation::Shutdown);
        // An older protocol rejects a current shutdown and keeps the socket.
        if stale.v != service_protocol::VERSION {
            shutdown.v = stale.v;
        }
        let old = match self.exchange(&shutdown, false, Duration::from_secs(2)) {
            Ok(response) | Err(ExchangeError::Mismatch(response)) => response,
            Err(ExchangeError::Unavailable) => {
                return (request.op != Operation::Shutdown).then_some(stale);
            }
        };
        if let Some(pid) = old.pid {
            let deadline = Instant::now() + Duration::from_secs(2);
            while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        // Stop must report the shutdown the socket accepted, not the version rejection that preceded it.
        if request.op == Operation::Shutdown {
            return Some(old);
        }
        // A select would count the pick twice; record is safe because its event ID makes a retry a no-op.
        if !spawning || !Self::repeatable(request.op) {
            return Some(stale);
        }
        self.exchange(request, true, timeout).ok()
    }

    /// Requests the old service's answer can be replaced by the new one's without a second effect.
    fn repeatable(op: Operation) -> bool {
        !matches!(op, Operation::Select | Operation::Shutdown)
    }

    /// Fills the caller fields this operation carries. The service does not read the rest of the environment.
    fn prepare_request(&self, request: &ServiceRequest) -> ServiceRequest {
        let mut request = request.clone();
        self.caller.apply(&mut request);
        request
    }

    fn exchange(
        &self,
        request: &ServiceRequest,
        spawn: bool,
        timeout: Duration,
    ) -> Result<ServiceResponse, ExchangeError> {
        let request = self.prepare_request(request);
        let Some(mut connection) = self.connect(spawn) else {
            return Err(ExchangeError::Unavailable);
        };
        // The private directory already keeps others out; also refuse a socket another user planted.
        if connection.peer_uid() != Some(unsafe { libc::getuid() }) {
            return Err(ExchangeError::Unavailable);
        }
        let Ok(line) = encode_line(&request) else {
            return Err(ExchangeError::Unavailable);
        };
        if line.len() > service_protocol::MAX_MESSAGE_BYTES {
            return Err(ExchangeError::Unavailable);
        }
        if connection
            .write(&line, Duration::from_secs_f64(service_protocol::IO_TIMEOUT))
            .is_err()
        {
            return Err(ExchangeError::Unavailable);
        }
        // The first suggest after startup loads history, so allow longer than a plain I/O timeout.
        let Ok(Some(reply)) = connection.read_line(service_protocol::MAX_RESPONSE_BYTES, timeout)
        else {
            return Err(ExchangeError::Unavailable);
        };
        let Ok(response) = decode_line::<ServiceResponse>(&reply) else {
            return Err(ExchangeError::Unavailable);
        };
        if response.v != service_protocol::VERSION {
            return Err(ExchangeError::Mismatch(response));
        }
        // Same version refusing our request is not a stale service; do not replace it.
        if response
            .error
            .as_ref()
            .is_some_and(|e| e.code() == Some(service_protocol::FailureCode::Version))
        {
            return Err(ExchangeError::Unavailable);
        }
        Ok(response)
    }
    fn connect(&self, spawn: bool) -> Option<SocketConnection> {
        if let Ok(connection) = connect_unix(&self.location.socket) {
            return Some(connection);
        }
        if !spawn || !self.launch() {
            return None;
        }
        // The winner of a concurrent start binds within milliseconds; losers exit and we reach the winner.
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(connection) = connect_unix(&self.location.socket) {
                return Some(connection);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    /// Starts the service detached in its own session, with no terminal. Inherited descriptors
    /// above stdio are closed in the child so the service cannot keep a shell FIFO or redirected fd
    /// open: macOS via CLOEXEC_DEFAULT, glibc 2.34+ via `posix_spawn_file_actions_addclosefrom_np`,
    /// musl (no closefrom action) via one close action per descriptor listed in /proc/self/fd.
    fn launch(&self) -> bool {
        use std::os::unix::ffi::OsStrExt;
        let Ok(executable) = CString::new(self.executable.as_os_str().as_bytes()) else {
            return false;
        };
        if unsafe { libc::access(executable.as_ptr(), libc::X_OK) } != 0
            || self.location.prepare().is_err()
        {
            return false;
        }
        let Ok(database) = CString::new(self.database.as_os_str().as_bytes()) else {
            return false;
        };
        let arguments = [executable.clone(), CString::new("--db").unwrap(), database];
        let allowed = Self::launch_environment(&self.environment);
        let Ok(environment): Result<Vec<_>, _> = allowed
            .iter()
            .map(|(k, v)| CString::new(format!("{k}={v}")))
            .collect()
        else {
            return false;
        };
        let pointers = |strings: &[CString]| {
            strings
                .iter()
                .map(|s| s.as_ptr() as *mut libc::c_char)
                .chain(std::iter::once(std::ptr::null_mut()))
                .collect::<Vec<_>>()
        };
        let argv = pointers(&arguments);
        let envp = pointers(&environment);
        unsafe {
            let mut attributes = std::mem::zeroed();
            let mut actions = std::mem::zeroed();
            if libc::posix_spawnattr_init(&mut attributes) != 0 {
                return false;
            }
            if libc::posix_spawn_file_actions_init(&mut actions) != 0 {
                libc::posix_spawnattr_destroy(&mut attributes);
                return false;
            }
            #[cfg(target_os = "macos")]
            // Darwin's POSIX_SPAWN_SETSID is not exposed by libc 0.2.
            let flags = 0x0400 | libc::POSIX_SPAWN_CLOEXEC_DEFAULT;
            #[cfg(target_os = "linux")]
            let flags = libc::POSIX_SPAWN_SETSID;
            let mut status = libc::posix_spawnattr_setflags(&mut attributes, flags as i16);
            for fd in 0..=2 {
                status |= libc::posix_spawn_file_actions_addopen(
                    &mut actions,
                    fd,
                    c"/dev/null".as_ptr(),
                    if fd == 0 {
                        libc::O_RDONLY
                    } else {
                        libc::O_WRONLY
                    },
                    0,
                );
            }
            #[cfg(all(target_os = "linux", target_env = "gnu"))]
            {
                // glibc ≥ 2.34 resolves this to close_range(2) in the child.
                status |= libc::posix_spawn_file_actions_addclosefrom_np(&mut actions, 3);
            }
            #[cfg(all(target_os = "linux", not(target_env = "gnu")))]
            {
                // Child-side only: this process keeps its database and socket descriptors.
                for fd in Self::open_fds() {
                    status |= libc::posix_spawn_file_actions_addclose(&mut actions, fd);
                }
            }
            let mut pid = 0;
            if status == 0 {
                status = libc::posix_spawn(
                    &mut pid,
                    executable.as_ptr(),
                    &actions,
                    &attributes,
                    argv.as_ptr(),
                    envp.as_ptr(),
                );
            }
            libc::posix_spawn_file_actions_destroy(&mut actions);
            libc::posix_spawnattr_destroy(&mut attributes);
            status == 0
        }
    }

    /// The service process gets this allowlist, not the caller's tokens or ranking switches.
    /// `PATH` stays because directory context runs `git` through `/usr/bin/env`. `HISTFILE` stays so
    /// startup can refuse to open the shell's own history file.
    fn launch_environment(env: &HashMap<String, String>) -> HashMap<String, String> {
        env.iter()
            .filter(|(key, _)| Self::launch_key_allowed(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    fn launch_key_allowed(key: &str) -> bool {
        matches!(
            key,
            "HOME"
                | "USER"
                | "LOGNAME"
                | "PATH"
                | "TMPDIR"
                | "LANG"
                | "XDG_RUNTIME_DIR"
                | "XDG_CONFIG_HOME"
                | "XDG_DATA_HOME"
                | "XDG_STATE_HOME"
                | "XDG_CACHE_HOME"
                | "HISTFILE"
                | "SMCLI_DB"
                | "SMCLI_CONFIG"
                | "SMCLI_RUNTIME_DIR"
                | "SMCLI_SERVICE_IDLE"
                | "SMCLI_FFF_MAX"
                | "SMCLI_FFF_IDLE"
        ) || key.starts_with("LC_")
    }

    /// Descriptors above stdio open in this process. The listing's own fd is included but closed by
    /// the time the child runs; musl ignores close errors there.
    #[cfg(all(target_os = "linux", any(test, not(target_env = "gnu"))))]
    fn open_fds() -> Vec<i32> {
        let Ok(entries) = std::fs::read_dir("/proc/self/fd") else {
            return vec![];
        };
        entries
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
            .filter(|&fd| fd >= 3)
            .collect()
    }
}

/// Same binary path replaces on any difference; another path only when the service build is older.
fn should_replace(service: Option<&str>, expected: &str) -> bool {
    if service == Some(expected) {
        return false;
    }
    match (service.and_then(build_parts), build_parts(expected)) {
        (Some((old, old_time)), Some((new, new_time))) if old != new => old_time < new_time,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_replacement_rules() {
        let new = "/a/smcli-service|1700000000.5|3";
        assert!(!should_replace(Some(new), new));
        assert!(should_replace(Some("/a/smcli-service|1700000001.5|3"), new));
        assert!(should_replace(Some("/b/smcli-service|1699999999.5|3"), new));
        assert!(!should_replace(
            Some("/b/smcli-service|1700000001.5|3"),
            new
        ));
        assert!(should_replace(Some("garbage"), new));
        assert!(should_replace(None, new));
        assert!(!should_replace(
            Some("/b/s|1700000000.5|3"),
            "/a/s|1700000000.5|3"
        ));
        // 9 ns is later than 10 ns only as text.
        assert!(!should_replace(
            Some("/b/s|1700000000.10|3"),
            "/a/s|1700000000.9|3"
        ));
        assert!(should_replace(
            Some("/b/s|1700000000.9|3"),
            "/a/s|1700000000.10|3"
        ));
    }

    #[test]
    fn service_executable_naming() {
        for (cli, service) in [
            ("smcli", "smcli-service"),
            ("smcli-123-abcd", "smcli-service-123-abcd"),
            ("other", "smcli-service"),
        ] {
            assert_eq!(
                ServiceClient::service_executable(&Path::new("/missing").join(cli)),
                Path::new("/missing").join(service)
            );
        }
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("smcli-build");
        std::fs::write(&target, "").unwrap();
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        assert_eq!(
            ServiceClient::service_executable(&alias),
            std::fs::canonicalize(dir.path())
                .unwrap()
                .join("smcli-service-build")
        );
    }
    /// The musl close list runs on any Linux host: it names an open descriptor and never stdio.
    #[cfg(target_os = "linux")]
    #[test]
    fn open_fds_lists_extra_descriptors_only() {
        use std::os::fd::AsRawFd;
        let file = tempfile::NamedTempFile::new().unwrap();
        let fds = ServiceClient::open_fds();
        assert!(fds.contains(&file.as_file().as_raw_fd()));
        assert!(fds.iter().all(|&fd| fd >= 3));
    }
    #[test]
    fn ranking_off_forwarded_for_suggest_and_pick() {
        let environment = HashMap::from([
            ("SMCLI_SERVICE".into(), "0".into()),
            ("SMCLI_RANKING_OFF".into(), "script".into()),
        ]);
        let client = ServiceClient::new(
            Path::new("/missing/db"),
            &environment,
            Path::new("/missing/smcli"),
        );
        for op in [Operation::Suggest, Operation::Pick, Operation::Search] {
            assert_eq!(
                client
                    .prepare_request(&ServiceRequest::new(op))
                    .ranking_off
                    .as_deref(),
                Some("script"),
                "{op:?}"
            );
        }
        assert!(
            client
                .prepare_request(&ServiceRequest::new(Operation::Ping))
                .ranking_off
                .is_none()
        );
        let bare = ServiceClient::new(
            Path::new("/missing/db"),
            &HashMap::from([("SMCLI_SERVICE".into(), "0".into())]),
            Path::new("/missing/smcli"),
        );
        assert_eq!(
            bare.prepare_request(&ServiceRequest::new(Operation::Suggest))
                .ranking_off
                .as_deref(),
            Some("")
        );
    }

    #[test]
    fn launch_environment_is_allowlisted() {
        let mut env = HashMap::from([
            ("HOME".into(), "/home/me".into()),
            ("USER".into(), "me".into()),
            ("LOGNAME".into(), "me".into()),
            ("PATH".into(), "/usr/bin".into()),
            ("TMPDIR".into(), "/tmp".into()),
            ("LANG".into(), "en_US.UTF-8".into()),
            ("LC_ALL".into(), "C".into()),
            ("XDG_RUNTIME_DIR".into(), "/run/user/1".into()),
            ("XDG_CONFIG_HOME".into(), "/home/me/.config".into()),
            ("XDG_DATA_HOME".into(), "/home/me/.local/share".into()),
            ("XDG_CACHE_HOME".into(), "/home/me/.cache".into()),
            ("HISTFILE".into(), "/home/me/.zsh_history".into()),
            ("SMCLI_DB".into(), "/home/me/history.db".into()),
            ("SMCLI_CONFIG".into(), "/home/me/config.json".into()),
            ("SMCLI_RUNTIME_DIR".into(), "/run/user/1/smcli".into()),
            ("SMCLI_SERVICE_IDLE".into(), "30".into()),
            ("SMCLI_FFF_MAX".into(), "2".into()),
            ("SMCLI_FFF_IDLE".into(), "10".into()),
            ("SHELL".into(), "/bin/zsh".into()),
            ("SMCLI_RANKING_OFF".into(), "path".into()),
            ("SMCLI_TEST_SECRET".into(), "x".into()),
            ("AWS_SECRET_ACCESS_KEY".into(), "secret".into()),
        ]);
        let launched = ServiceClient::launch_environment(&env);
        for key in [
            "HOME",
            "USER",
            "LOGNAME",
            "PATH",
            "TMPDIR",
            "LANG",
            "LC_ALL",
            "XDG_RUNTIME_DIR",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "HISTFILE",
            "SMCLI_DB",
            "SMCLI_CONFIG",
            "SMCLI_RUNTIME_DIR",
            "SMCLI_SERVICE_IDLE",
            "SMCLI_FFF_MAX",
            "SMCLI_FFF_IDLE",
        ] {
            assert_eq!(launched.get(key), env.get(key), "{key} must be kept");
        }
        for key in [
            "SHELL",
            "SMCLI_RANKING_OFF",
            "SMCLI_TEST_SECRET",
            "AWS_SECRET_ACCESS_KEY",
        ] {
            assert!(!launched.contains_key(key), "{key} must be dropped");
        }
        env.insert("LC_CTYPE".into(), "UTF-8".into());
        assert_eq!(
            ServiceClient::launch_environment(&env)
                .get("LC_CTYPE")
                .map(String::as_str),
            Some("UTF-8")
        );
    }

    #[test]
    fn embed_forwards_caller_home_and_shell_path() {
        let environment = HashMap::from([
            ("HOME".into(), "/caller".into()),
            ("PATH".into(), "/bin:/caller/bin".into()),
        ]);
        let client = ServiceClient::new(
            Path::new("/missing/db"),
            &environment,
            Path::new("/missing/smcli"),
        );
        let mut request = ServiceRequest::new(Operation::Embed);
        let prepared = client.prepare_request(&request);
        assert_eq!(prepared.home.as_deref(), Some("/caller"));
        assert_eq!(prepared.shell_path.as_deref(), Some("/bin:/caller/bin"));
        request.home = Some("/explicit".into());
        assert_eq!(
            client.prepare_request(&request).home.as_deref(),
            Some("/explicit")
        );
        assert_eq!(
            client
                .prepare_request(&ServiceRequest::new(Operation::Ping))
                .home
                .as_deref(),
            Some("/caller")
        );
    }

    #[test]
    fn modes_and_repeatable_operations() {
        for value in ["0", "OFF", "false", "no", "connect", "on"] {
            let client = ServiceClient::new(
                Path::new("/missing/db"),
                &HashMap::from([("SMCLI_SERVICE".into(), value.into())]),
                Path::new("/missing/smcli"),
            );
            assert!(
                client.mode
                    == match value {
                        "connect" => Mode::Connect,
                        "on" => Mode::Spawn,
                        _ => Mode::Off,
                    }
            );
        }
        for op in [
            Operation::Ping,
            Operation::Record,
            Operation::Suggest,
            Operation::Pick,
            Operation::Search,
            Operation::Context,
            Operation::Files,
            Operation::Grep,
            Operation::Embed,
        ] {
            assert!(ServiceClient::repeatable(op));
        }
        assert!(!ServiceClient::repeatable(Operation::Select));
        assert!(!ServiceClient::repeatable(Operation::Shutdown));
    }

    /// A socket that rejects this protocol, the way an older service does, is stopped and replaced.
    #[test]
    fn old_version_socket_is_replaced_on_start_and_stop() {
        let python = python3();
        // Stop must not report the answering socket as absent.
        let stopped = talk_to_old(&python, Operation::Shutdown, false);
        assert!(stopped.ok, "{stopped:?}");
        // Start replaces it with the service this client launches.
        let started = talk_to_old(&python, Operation::Ping, true);
        assert!(
            started.ok && started.v == service_protocol::VERSION,
            "{started:?}"
        );
    }

    fn python3() -> String {
        let output = std::process::Command::new("python3")
            .arg("-c")
            .arg("import sys; print(sys.executable)")
            .output()
            .expect("python3");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    /// Speaks protocol v1: rejects anything else, accepts its own shutdown, then exits.
    fn talk_to_old(python: &str, op: Operation, spawn: bool) -> ServiceResponse {
        use std::os::unix::fs::PermissionsExt;
        let temporary = tempfile::Builder::new()
            .prefix("smcli-")
            .tempdir_in("/tmp")
            .unwrap();
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let database = temporary.path().join("history.db");
        let marker = temporary.path().join("replaced");
        let log = temporary.path().join("log");
        let ready = temporary.path().join("ready");
        let mut environment = HashMap::from([(
            "SMCLI_RUNTIME_DIR".into(),
            temporary.path().to_string_lossy().into_owned(),
        )]);
        let socket = crate::service_location::ServiceLocation::new(&database, &environment).socket;
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let old = fixtures.join("old_service.py");
        if spawn {
            let stub = temporary.path().join("replacement_service.py");
            // The frozen helper predates v4; the replacement must speak the current protocol.
            let source = std::fs::read_to_string(fixtures.join("replacement_service.py")).unwrap();
            std::fs::write(
                &stub,
                source.replace("\"v\": 3", &format!("\"v\": {}", service_protocol::VERSION)),
            )
            .unwrap();
            let wrapper = temporary.path().join("smcli-service");
            std::fs::write(
                &wrapper,
                format!(
                    "#!/bin/sh\nexec {python} {stub} {socket} {marker}\n",
                    stub = stub.display(),
                    socket = socket.display(),
                    marker = marker.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
            environment.insert(
                "SMCLI_SERVICE_BIN".into(),
                wrapper.to_string_lossy().into_owned(),
            );
        }
        let mut child = std::process::Command::new(python)
            .arg(&old)
            .arg(&socket)
            .arg(&log)
            .arg(&ready)
            .spawn()
            .unwrap();
        let old_pid = child.id() as i32;
        let _reap = KillOnDrop(old_pid);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !ready.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ready.exists(), "old socket did not bind");
        // Reap so the client's pid wait does not spin on a zombie.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        let client = ServiceClient::new(&database, &environment, Path::new("/missing/smcli"));
        let response = client
            .send(
                &ServiceRequest::new(op),
                Some(spawn),
                Duration::from_secs(5),
            )
            .unwrap_or_else(|| {
                panic!(
                    "no reply; log={}",
                    std::fs::read_to_string(&log).unwrap_or_default()
                )
            });
        let seen = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(
            seen.contains("1 shutdown"),
            "compatible shutdown was not sent: {seen}"
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while unsafe { libc::kill(old_pid, 0) } == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_ne!(
            unsafe { libc::kill(old_pid, 0) },
            0,
            "old service still running"
        );
        assert!(
            !socket.exists() || marker.exists(),
            "socket still answers and was not replaced"
        );
        if spawn {
            assert!(marker.exists(), "replacement service was not started");
            if let Some(pid) = response.pid {
                unsafe {
                    libc::kill(pid, libc::SIGTERM);
                }
            }
        } else {
            assert_eq!(response.pid, Some(old_pid));
        }
        response
    }

    struct KillOnDrop(i32);
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            unsafe {
                libc::kill(self.0, libc::SIGTERM);
            }
        }
    }
}
