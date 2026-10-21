mod file_search;
mod service;

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::time::Duration;

use smcli_core::history::CapturedHistory;
use smcli_core::service_location::ServiceLocation;

use crate::service::Service;

/// `smcli-service --db <absolute path to history.db>`. Started on demand by clients; exits when idle.
fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.len() != 2 || arguments[0] != "--db" || !arguments[1].starts_with('/') {
        let _ = writeln!(std::io::stderr(), "Usage: smcli-service --db <absolute path to history.db>");
        std::process::exit(2);
    }
    let environment: HashMap<String, String> = std::env::vars().collect();
    if let Err(error) = start(&arguments[1], environment) {
        let _ = writeln!(std::io::stderr(), "smcli-service: {error:#}");
        std::process::exit(1);
    }
}

fn start(database: &str, environment: HashMap<String, String>) -> anyhow::Result<()> {
    let location = ServiceLocation::new(database.as_ref(), &environment);
    location.prepare()?;
    // Only the lock holder binds, so concurrent starts can't steal each other's socket.
    // The lock is released by the kernel when this process exits, however it exits.
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&location.lock)
        .map_err(|e| anyhow::anyhow!("Cannot open {}: {e}", location.lock.display()))?;
    if lock.try_lock().is_err() {
        std::process::exit(0);
    }
    // Never open (and so create or write) the database when it is the starting shell's native history.
    let history = CapturedHistory::new(database);
    history.check_separate(environment.get("HISTFILE").map_or("", String::as_str))?;
    let idle = environment.get("SMCLI_SERVICE_IDLE").and_then(|v| v.parse::<f64>().ok()).unwrap_or(1800.0);
    let service = Service::new(history, location, Duration::from_secs_f64(idle), environment)?;
    let result = service.run();
    drop(lock);
    result
}
