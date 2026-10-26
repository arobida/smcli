use std::{path::PathBuf, time::Duration};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct IdentifierEvidence {
    pub name: String,
    pub is_definition: bool,
}

/// Returns (seen, defined) for the identifiers extracted from a single command.
pub fn command_flags(command: &str, found: &[IdentifierEvidence]) -> (bool, bool) {
    let mut seen = false;
    let mut defined = false;
    for name in crate::identifiers::identifiers(&[command]) {
        if let Some(hit) = found.iter().find(|e| e.name == name) {
            seen = true;
            defined |= hit.is_definition;
        }
    }
    (seen, defined)
}

/// Request-scoped project facts. Missing entries are unknown, never negative evidence.
pub trait ProjectEvidence {
    fn exists(&self, paths: &[PathBuf]) -> Vec<bool>;
    fn git_activity(&self, _paths: &[PathBuf]) -> Vec<f64> { vec![] }
    fn find_identifiers(&self, _names: &[String], _budget: Duration) -> Vec<IdentifierEvidence> { vec![] }
    fn grep_terms(&self, _terms: &[String], _budget: Duration) -> Vec<String> { vec![] }
}
