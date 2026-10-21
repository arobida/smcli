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
              now: f64) -> Result<HashMap<String, SearchEntry>> {
    let native: Vec<String> = history_files(home, environment)?
        .iter()
        .filter_map(|f| f.read().ok())
        .flat_map(|text| text.split('\n').map(String::from).collect::<Vec<_>>())
        .collect();
    let stats = captured.command_stats(Some(&working_directory(environment)), project, now)?;
    Ok(search_corpus(stats, &native))
}

/// Signals for an in-process search, read from the database: the session's previous command and what followed it
/// before, and earlier picks. (The service keeps these in memory.)
pub fn search_signals(captured: &CapturedHistory, context: Option<RankingContext>) -> Result<SearchSignals> {
    if !captured.path.exists() {
        return Ok(SearchSignals { context, ..Default::default() });
    }
    let db = captured.open_connection()?;
    let user = user_name();
    let mut signals = SearchSignals { picks: captured.picks(5_000, &db)?, ..Default::default() };
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

/// The search index next to the history database: `potionPath` from `smcli init`, otherwise the built-in model.
// shortcut: Ollama and Apple (`builtin`) configs fall back to the built-in Potion; port an embedder when someone asks.
pub fn search_engine(captured: &CapturedHistory, home: &Path, environment: &HashMap<String, String>) -> Result<HistorySearch> {
    let config = SmcliConfig::load(&SmcliConfig::location(home, environment))?;
    let potion = match &config.potion_path {
        Some(path) => Potion::from_dir(Path::new(path))?,
        None => Potion::bundled()?,
    };
    let mut directory = captured.path.clone().into_os_string();
    directory.push(".search");
    Ok(HistorySearch::new(directory, potion))
}
