#![cfg(unix)]

use std::collections::HashMap;
use std::path::Path;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use smcli_core::service_client::ServiceClient;
use smcli_core::service_location::{ServiceLocation, build};
use smcli_core::service_protocol::{
    MAX_RESPONSE_BYTES, Operation, ServiceRequest, ServiceResponse, decode_line, encode_line,
};
use smcli_core::unix_socket::connect_unix;

const TIMEOUT: Duration = Duration::from_secs(60);

struct Service {
    client: ServiceClient,
    location: ServiceLocation,
    _temporary: tempfile::TempDir,
}

impl Service {
    fn request(&self, request: &ServiceRequest) -> ServiceResponse {
        let response = self.client.send(request, None, TIMEOUT)
            .unwrap_or_else(|| {
                let wire = (|| -> Result<Option<ServiceResponse>> {
                    let mut connection = connect_unix(&self.location.socket)?;
                    connection.write(&encode_line(request)?, TIMEOUT)?;
                    connection.read_line(MAX_RESPONSE_BYTES, TIMEOUT)?
                        .map(|line| decode_line(&line).map_err(Into::into)).transpose()
                })();
                panic!("No client response to {request:?}; direct socket response: {wire:?}")
            });
        assert!(response.ok, "Request {request:?}: {response:?}");
        response
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        // Also stop the detached child when an assertion panics.
        let _ = self.client.send(
            &ServiceRequest::new(Operation::Shutdown), Some(false), Duration::from_secs(2),
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.location.socket.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn client_talks_to_real_service() -> Result<()> {
    let temporary = tempfile::Builder::new().prefix("smcli-").tempdir_in("/tmp")?;
    std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))?;
    let home = temporary.path().join("home");
    let repository = temporary.path().join("repo");
    std::fs::create_dir(&home)?;
    std::fs::create_dir(&repository)?;
    let config = temporary.path().join("config.json");
    std::fs::write(&config, "{}")?;
    let output = Command::new("/usr/bin/git")
        .args(["init", "--quiet"])
        .arg(&repository)
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .output()?;
    assert!(output.status.success(), "git init: {output:?}");
    let repository = std::fs::canonicalize(repository)?;
    let directory = repository.to_string_lossy().into_owned();
    let file = "client-integration-needle.txt";
    std::fs::write(repository.join(file), "isolated service fixture\n")?;
    let binary = Path::new(env!("CARGO_BIN_EXE_smcli-service"));
    let database = temporary.path().join("history.db");
    // Pass only fixture-owned history/config paths; never inherit the user's HISTFILE or config.
    let environment: HashMap<String, String> = [
        ("HOME", home.to_string_lossy().into_owned()),
        ("SMCLI_DB", database.to_string_lossy().into_owned()),
        ("SMCLI_RUNTIME_DIR", temporary.path().to_string_lossy().into_owned()),
        ("SMCLI_CONFIG", config.to_string_lossy().into_owned()),
        ("SMCLI_SERVICE_BIN", binary.to_string_lossy().into_owned()),
        ("SMCLI_SERVICE", "on".into()),
        ("PWD", directory.clone()),
        ("PATH", "/usr/bin:/bin".into()),
        ("SHELL", "/bin/zsh".into()),
        ("HISTFILE", home.join(".zsh_history").to_string_lossy().into_owned()),
    ].into_iter().map(|(key, value)| (key.to_owned(), value)).collect();
    let service = Service {
        client: ServiceClient::new(&database, &environment, binary),
        location: ServiceLocation::new(&database, &environment),
        _temporary: temporary,
    };

    assert!(!service.location.socket.exists());
    let ping = service.request(&ServiceRequest::new(Operation::Ping));
    assert!(service.location.socket.exists(), "send must spawn on first request");
    assert!(ping.pid.is_some_and(|pid| pid > 0));
    assert_eq!(ping.build, build(binary));
    assert!(ping.build.is_some());

    let command = "printf clientintegrationneedle";
    let mut record = ServiceRequest::new(Operation::Record);
    record.event = Some("b62adf73-2b8e-4d17-9fc1-a832e6a41e05".into());
    record.command = Some(command.into());
    record.shell = Some("zsh".into());
    record.session = Some("client-integration".into());
    record.directory = Some(directory.clone());
    record.exit_code = Some(0);
    let first = service.request(&record);
    assert_eq!(first.event, record.event);
    assert_eq!(first.duplicate, Some(false));
    let retry = service.request(&record);
    assert_eq!(retry.event, record.event);
    assert_eq!(retry.duplicate, Some(true));

    let mut suggest = ServiceRequest::new(Operation::Suggest);
    suggest.directory = Some(directory.clone());
    suggest.prefix = Some("printf clientintegration".into());
    let response = service.request(&suggest);
    assert!(response.suggestions.as_ref().is_some_and(|items| items.iter().any(|s| s.command == command)), "{response:?}");

    let mut pick = ServiceRequest::new(Operation::Pick);
    pick.directory = Some(directory.clone());
    let response = service.request(&pick);
    assert!(response.suggestions.as_ref().is_some_and(|items| items.iter().any(|s| s.command == command)), "{response:?}");
    assert!(response.recent.as_ref().is_some_and(|items| items.iter().any(|s| s == command)), "{response:?}");

    let mut search = ServiceRequest::new(Operation::Search);
    search.directory = Some(directory.clone());
    search.query = Some("clientintegrationneedle".into());
    let response = service.request(&search);
    assert!(response.hits.as_ref().is_some_and(|items| items.iter().any(|s| s.command == command)), "{response:?}");

    let mut context = ServiceRequest::new(Operation::Context);
    context.directory = Some(directory.clone());
    let response = service.request(&context);
    assert_eq!(response.context.and_then(|c| c.repository).map(|r| r.root), Some(directory.clone()));

    let mut files = ServiceRequest::new(Operation::Files);
    files.directory = Some(directory);
    files.query = Some(file.into());
    let response = service.request(&files);
    assert!(response.files.as_ref().is_some_and(|items| items.iter().any(|f| f.path == file)), "{response:?}");

    // send() deliberately hides version failures, so inspect the wire response as well.
    let mut wrong_version = ServiceRequest::new(Operation::Ping);
    wrong_version.v = 1;
    let mut connection = connect_unix(&service.location.socket)?;
    connection.write(&encode_line(&wrong_version)?, TIMEOUT)?;
    let line = connection.read_line(MAX_RESPONSE_BYTES, TIMEOUT)?.context("Missing version response")?;
    let response: ServiceResponse = decode_line(&line)?;
    assert!(!response.ok, "{response:?}");
    assert_eq!(response.error.as_ref().map(|e| e.code.as_str()), Some("version"));
    assert!(service.client.send(&wrong_version, Some(false), TIMEOUT).is_none());

    let response = service.request(&ServiceRequest::new(Operation::Shutdown));
    assert_eq!(response.pid, ping.pid);
    let deadline = Instant::now() + Duration::from_secs(5);
    while service.location.socket.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!service.location.socket.exists(), "shutdown must remove the socket");
    assert!(service.client.send(&ServiceRequest::new(Operation::Ping), Some(false), TIMEOUT).is_none());
    Ok(())
}
