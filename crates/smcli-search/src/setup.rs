use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use smcli_core::config::SmcliConfig;
use smcli_core::entry::SearchEntry;
use smcli_core::history::{CapturedHistory, user_name};
use smcli_core::ranking::{RankingContext, Signal};

use crate::corpus::{SearchSignals, search_corpus};
use crate::native::history_files;
use crate::{HistorySearch, Potion};

/// Captured stats plus native history. Native files are best-effort: an unreadable one should not block the rest.
pub fn corpus(captured: &CapturedHistory, home: &Path, environment: &HashMap<String, String>, project: Option<&str>,
              kind: Option<&str>, now: f64) -> Result<HashMap<String, SearchEntry>> {
    let native: Vec<String> = history_files(home, environment)?
        .iter()
        .filter_map(|f| f.read().ok())
        .flat_map(|text| text.split('\n').map(String::from).collect::<Vec<_>>())
        .collect();
    let stats = captured.command_stats(Some(&working_directory(environment)), project, kind, now)?;
    Ok(search_corpus(stats, &native))
}

/// Signals for an in-process search, read from the database: the session's previous command and what followed it
/// before, and earlier picks. (The service keeps these in memory.)
pub fn search_signals(captured: &CapturedHistory, context: Option<RankingContext>, environment: &HashMap<String, String>) -> Result<SearchSignals> {
    let shell_path = smcli_core::service_protocol::shell_path_snapshot(environment.get("PATH").map(String::as_str));
    let directory = Some(working_directory(environment));
    if !captured.path.exists() {
        return Ok(SearchSignals { context, shell_path, directory, ..Default::default() });
    }
    let db = captured.open_connection()?;
    let user = user_name();
    let mut signals = SearchSignals { picks: captured.picks(5_000, &db)?, shell_path, directory, ..Default::default() };
    if let Some(session) = context.as_ref().filter(|c| c.uses(Signal::Transition)).and_then(|c| c.session.as_deref())
        && let Some(previous) = captured.previous_command(session, &user, &db)? {
            signals.followers = captured.followers(&previous, &user, &db)?;
            signals.previous = Some(previous);
        }
    signals.context = context;
    Ok(signals)
}

/// The shell's $PWD keeps symlinked paths as typed, matching what the capture hooks recorded.
pub fn working_directory(environment: &HashMap<String, String>) -> String {
    environment
        .get("PWD")
        .filter(|p| p.starts_with('/'))
        .cloned()
        .unwrap_or_else(|| std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_else(|_| "/".into()))
}

/// The shared index directory next to the history database.
pub fn index_directory(captured: &CapturedHistory) -> std::path::PathBuf {
    let mut directory = captured.path.clone().into_os_string();
    directory.push(".search");
    directory.into()
}

/// Describe the configured model without constructing it.
pub fn configured_model(home: &Path, environment: &HashMap<String, String>) -> Result<crate::ModelId> {
    let config = SmcliConfig::load(&SmcliConfig::location(home, environment))?;
    Potion::describe(config.potion_path.as_deref().map(Path::new))
}

/// Hold while model weights are resident; separate from the index writer lock.
pub fn model_lock(captured: &CapturedHistory, wait: std::time::Duration) -> Result<Option<std::fs::File>> {
    use std::os::unix::fs::OpenOptionsExt;
    let directory = index_directory(captured);
    crate::index::prepare_directory(&directory)?;
    let lock = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false)
        .mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(directory.join("model.lock"))?;
    let start = std::time::Instant::now();
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(Some(lock)),
            Err(std::fs::TryLockError::WouldBlock) => {
                let remaining = wait.saturating_sub(start.elapsed());
                if remaining.is_zero() { return Ok(None); }
                std::thread::sleep(remaining.min(std::time::Duration::from_millis(50)));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
#[test]
fn model_lock_is_exclusive_and_waits() {
    use std::time::{Duration, Instant};
    let root = tempfile::tempdir().unwrap();
    let captured = CapturedHistory::new(root.path().join("history.db"));
    let lock = model_lock(&captured, Duration::ZERO).unwrap().unwrap();
    assert!(model_lock(&captured, Duration::ZERO).unwrap().is_none());
    let start = Instant::now();
    assert!(model_lock(&captured, Duration::from_millis(60)).unwrap().is_none());
    assert!(start.elapsed() >= Duration::from_millis(60));
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(60));
        drop(lock);
    });
    assert!(model_lock(&captured, Duration::from_secs(2)).unwrap().is_some());
    release.join().unwrap();
    assert!(model_lock(&captured, Duration::ZERO).unwrap().is_some());
}

/// The search index next to the history database: `potionPath` from `smcli init`, otherwise the built-in model.
// shortcut: Ollama and Apple (`builtin`) configs fall back to the built-in Potion; port an embedder when someone asks.
pub fn search_engine(captured: &CapturedHistory, home: &Path, environment: &HashMap<String, String>) -> Result<HistorySearch> {
    let config = SmcliConfig::load(&SmcliConfig::location(home, environment))?;
    let potion = match &config.potion_path {
        Some(path) => Potion::from_dir(Path::new(path))?,
        None => Potion::bundled()?,
    };
    Ok(HistorySearch::new(index_directory(captured), potion))
}
