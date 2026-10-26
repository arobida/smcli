use std::path::PathBuf;
use smcli_search::ProjectEvidence;
use super::Index;

pub(crate) struct IndexEvidence(pub Index);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expansion_grep_returns_stems_and_definition_names() {
        use fff_search::{FilePicker, FilePickerOptions, SharedFilePicker, SharedFrecency};
        use std::{sync::Arc, time::Duration};
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("migrate.rs"), "pub fn migration_case() {}\n// migration\n").unwrap();
        let shared = SharedFilePicker::default();
        FilePicker::new_with_shared_state(shared.clone(), SharedFrecency::default(), FilePickerOptions {
            base_path: root.path().to_string_lossy().into_owned(), enable_content_indexing: true, ..Default::default()
        }).unwrap();
        let evidence = IndexEvidence(Arc::new(super::super::OwnedIndex(shared.clone())));
        assert!(shared.wait_for_scan(Duration::from_secs(10)));
        let terms = evidence.grep_terms(&["migration".into()], Duration::from_millis(20));
        assert!(terms.contains(&"migrate".into()), "{terms:?}");
        assert!(terms.contains(&"migration_case".into()), "{terms:?}");
        assert_eq!(terms.len(), 2);
        assert!(evidence.grep_terms(&["migration".into()], Duration::ZERO).is_empty());
        let mut capped = vec![];
        for i in 0..30 { push_term(&mut capped, &format!("name_{i}")); }
        assert_eq!(capped.len(), 16);
    }

    #[test]
    fn find_identifiers_marks_a_definition_and_a_zero_budget_checks_nothing() {
        use fff_search::{FilePicker, FilePickerOptions, SharedFilePicker, SharedFrecency};
        use std::{sync::Arc, time::Duration};
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("lib.rs"), "fn dedup_keeps_latest() {}\nlet _ = old_removed_case;\n").unwrap();
        let shared = SharedFilePicker::default();
        FilePicker::new_with_shared_state(shared.clone(), SharedFrecency::default(), FilePickerOptions {
            base_path: root.path().to_string_lossy().into_owned(), enable_content_indexing: true, ..Default::default()
        }).unwrap();
        let evidence = IndexEvidence(Arc::new(super::super::OwnedIndex(shared.clone())));
        assert!(shared.wait_for_scan(Duration::from_secs(10)));
        let found = evidence.find_identifiers(
            &["dedup_keeps_latest".into(), "old_removed_case".into(), "missing_symbol".into()], Duration::from_millis(20));
        let defined = found.iter().find(|e| e.name == "dedup_keeps_latest").unwrap();
        assert!(defined.is_definition, "{found:?}");
        let mention = found.iter().find(|e| e.name == "old_removed_case").unwrap();
        assert!(!mention.is_definition, "{found:?}");
        assert!(found.iter().all(|e| e.name != "missing_symbol"));
        assert!(evidence.find_identifiers(&["dedup_keeps_latest".into()], Duration::ZERO).is_empty());
    }
}

pub(crate) fn identifier_flags(evidence: &dyn ProjectEvidence, commands: &[String]) -> Vec<(bool, bool)> {
    let refs: Vec<&str> = commands.iter().map(String::as_str).collect();
    let names = smcli_search::identifiers::identifiers(&refs);
    if names.is_empty() {
        return vec![(false, false); commands.len()];
    }
    let found = evidence.find_identifiers(&names, std::time::Duration::from_millis(20));
    refs.iter().map(|command| smcli_search::evidence::command_flags(command, &found)).collect()
}

fn contains_ident(line: &str, name: &str) -> bool {
    let bytes = line.as_bytes();
    let mut start = 0;
    while let Some(at) = line[start..].find(name) {
        let abs = start + at;
        let before = abs == 0 || !ident_byte(bytes[abs - 1]);
        let end = abs + name.len();
        let after = end >= bytes.len() || !ident_byte(bytes[end]);
        if before && after {
            return true;
        }
        start = abs + 1;
    }
    false
}

fn ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

fn push_term(terms: &mut Vec<String>, term: &str) {
    if terms.len() < 16 && (3..=128).contains(&term.len())
        && term.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
        && !terms.iter().any(|t| t == term) {
        terms.push(term.into());
    }
}

impl ProjectEvidence for IndexEvidence {
    fn exists(&self, paths: &[PathBuf]) -> Vec<bool> {
        let Ok(guard) = self.0.read() else { return vec![] };
        let Some(picker) = guard.as_ref() else { return vec![] };
        if picker.is_scan_active() { return vec![]; }
        paths.iter().map(|path| picker.get_file_by_path(path).is_some()).collect()
    }

    fn grep_terms(&self, terms: &[String], budget: std::time::Duration) -> Vec<String> {
        let patterns: Vec<&str> = terms.iter().filter(|t| t.len() >= 3 && t.len() <= 128)
            .take(16).map(String::as_str).collect();
        if patterns.is_empty() || budget.is_zero() { return vec![]; }
        let Ok(guard) = self.0.read() else { return vec![] };
        let Some(picker) = guard.as_ref() else { return vec![] };
        if picker.is_scan_active() { return vec![]; }
        let result = picker.multi_grep(&patterns, &[], &fff_search::GrepSearchOptions {
            page_limit: 32, max_matches_per_file: 2, classify_definitions: true,
            time_budget_ms: budget.as_millis().clamp(1, 20) as u64, enforce_time_budget: true,
            ..Default::default()
        });
        let mut terms = Vec::new();
        for hit in &result.matches {
            if let Some(file) = result.files.get(hit.file_index) {
                let path = file.relative_path(picker);
                if let Some(stem) = std::path::Path::new(&path).file_stem().and_then(|s| s.to_str()) {
                    push_term(&mut terms, stem);
                }
            }
            if hit.is_definition {
                let words: Vec<&str> = hit.line_content.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .filter(|s| !s.is_empty()).collect();
                for pair in words.windows(2) {
                    if matches!(pair[0], "fn" | "def" | "class" | "struct" | "enum" | "trait" | "function" | "interface" | "type") {
                        push_term(&mut terms, pair[1]);
                        break;
                    }
                }
            }
            if terms.len() == 16 { break; }
        }
        terms
    }

    fn find_identifiers(&self, names: &[String], budget: std::time::Duration) -> Vec<smcli_search::evidence::IdentifierEvidence> {
        let ms = u64::try_from(budget.as_millis()).unwrap_or(u64::MAX);
        if names.is_empty() || ms == 0 {
            return vec![];
        }
        let Ok(guard) = self.0.read() else { return vec![] };
        let Some(picker) = guard.as_ref() else { return vec![] };
        if picker.is_scan_active() { return vec![]; }
        let patterns: Vec<&str> = names.iter().map(String::as_str).collect();
        let page = picker.multi_grep(&patterns, &[], &fff_search::GrepSearchOptions {
            page_limit: 256,
            max_matches_per_file: 0,
            time_budget_ms: ms,
            enforce_time_budget: true,
            classify_definitions: true,
            smart_case: false,
            max_file_size: 10 * 1024 * 1024,
            ..Default::default()
        });
        names.iter().filter_map(|name| {
            let mut defined = false;
            let mut seen = false;
            for hit in &page.matches {
                if !contains_ident(&hit.line_content, name) {
                    continue;
                }
                seen = true;
                defined |= hit.is_definition;
                if defined {
                    break;
                }
            }
            seen.then(|| smcli_search::evidence::IdentifierEvidence { name: name.clone(), is_definition: defined })
        }).collect()
    }

    fn git_activity(&self, paths: &[PathBuf]) -> Vec<f64> {
        let Ok(guard) = self.0.read() else { return vec![] };
        let Some(picker) = guard.as_ref() else { return vec![] };
        if picker.is_scan_active() { return vec![]; }
        paths.iter().map(|path| picker.get_file_by_path(path).map_or(0.0, |file| {
            let status = fff_search::git::format_git_status_opt(file.git_status).unwrap_or("clean");
            let dirty = matches!(status, "modified" | "untracked" | "staged_new" | "staged_modified");
            if dirty { 1.0 } else { (file.git_recency_score.max(0) as f64 / 100.0).min(1.0) }
        })).collect()
    }
}
