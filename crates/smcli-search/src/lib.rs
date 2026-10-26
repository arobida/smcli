pub mod corpus;
pub mod evidence;
pub mod identifiers;
pub use evidence::ProjectEvidence;
mod filesystem;

pub fn command_file_presence(commands: &[String], directory: &str, evidence: &dyn ProjectEvidence) -> Vec<bool> {
    let hits = commands.iter().map(|command| (smcli_core::entry::SearchEntry::from_key(command), 1.0)).collect::<Vec<_>>();
    let signals = corpus::SearchSignals { directory: Some(directory.into()), ..Default::default() };
    filesystem::evidence_with_index(&hits, &signals, false, true, Some(evidence)).into_iter().map(|r| r.1).collect()
}
pub fn command_git_activity(commands: &[String], directory: &str, evidence: &dyn ProjectEvidence) -> Vec<f64> {
    let hits = commands.iter().map(|command| (smcli_core::entry::SearchEntry::from_key(command), 1.0)).collect::<Vec<_>>();
    let signals = corpus::SearchSignals { directory: Some(directory.into()), ..Default::default() };
    filesystem::evidence_with_index(&hits, &signals, false, true, Some(evidence)).into_iter().map(|r| r.2).collect()
}
pub mod index;
pub mod native;
mod potion;
pub mod setup;
mod tokenizer;

pub use index::HistorySearch;
pub use potion::Potion;
pub use tokenizer::ShellTokenizer;
