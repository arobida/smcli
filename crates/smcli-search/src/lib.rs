pub mod cheat;
pub mod commands;
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
pub mod tldr;
mod tokenizer;

pub use index::HistorySearch;
pub use potion::Potion;
pub use cheat::CheatSource;
pub use tldr::{Corpus, Entry, TldrSource, rank};
pub use tokenizer::ShellTokenizer;

#[cfg(test)]
extern crate self as smcli_search;
#[cfg(test)]
#[path = "../tests/ranking.rs"]
mod ranking;

#[cfg(test)]
#[test]
fn project_config_comparison() {
    ranking::compare_project_config(|mode| corpus::PROJECT_CONFIG_MODE.with(|m| m.set(mode)));
}

#[cfg(test)]
#[test]
fn vector_first_comparison() {
    ranking::compare_vector_first(|enabled| index::VECTOR_FIRST.with(|v| v.set(enabled)));
}
