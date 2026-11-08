use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

use crate::tldr::Corpus;

#[derive(Deserialize)]
struct DataHeader {
    version: u32,
    cheat_commit: String,
}

#[derive(Deserialize)]
struct DataEntry {
    program: String,
    description: String,
    template: String,
}

/// cheat/cheatsheets lives beside the binaries in installs, or under Data/ for development and tests.
pub fn bundled_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok();
    let beside = |p: Option<PathBuf>| p.and_then(|e| e.parent().map(|d| d.join("cheat")));
    [
        beside(exe.as_ref().and_then(|e| std::fs::canonicalize(e).ok())),
        beside(exe),
        Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Data/cheat")),
    ]
    .into_iter()
    .flatten()
    .find(|d| d.join("cheat.jsonl").is_file())
}

/// Parse cheat.jsonl: header, then one example per command line. Every entry is platform `common`.
pub fn load(dir: &Path) -> Result<Corpus> {
    let text =
        std::fs::read_to_string(dir.join("cheat.jsonl")).context("failed to read cheat.jsonl")?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let header: DataHeader = serde_json::from_str(
        lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("cheat.jsonl is empty"))?,
    )?;
    ensure!(
        header.version == 1,
        "unsupported cheat data version {}",
        header.version
    );
    ensure!(!header.cheat_commit.is_empty(), "cheat commit is empty");
    let mut entries = Vec::new();
    for line in lines {
        let entry: DataEntry = serde_json::from_str(line).context("invalid cheat JSONL entry")?;
        entries.push(crate::tldr::Entry {
            program: entry.program,
            description: entry.description,
            template: entry.template,
        });
    }
    Ok(Corpus {
        commit: header.cheat_commit,
        entries,
    })
}

/// cheat/cheatsheets as a `CommandSource`: the fallback behind tldr. Templates keep their
/// literal placeholders (e.g. `/path/to/foo.tar`), inserted as written.
#[derive(Debug, Clone)]
pub struct CheatSource {
    corpus: Corpus,
}

impl CheatSource {
    pub fn new(dir: &Path) -> Result<Self> {
        Ok(Self { corpus: load(dir)? })
    }

    /// All programs with a sheet.
    pub fn programs(&self) -> HashSet<String> {
        self.corpus
            .entries
            .iter()
            .map(|entry| entry.program.clone())
            .collect()
    }
}

impl smcli_core::command_source::CommandSource for CheatSource {
    fn name(&self) -> &str {
        "cheat"
    }

    fn version(&self) -> &str {
        &self.corpus.commit
    }

    fn examples(
        &self,
        program: &str,
        _platform: &str,
    ) -> Vec<smcli_core::command_source::CommandExample> {
        self.corpus
            .entries
            .iter()
            .filter(|entry| entry.program == program)
            .map(|entry| smcli_core::command_source::CommandExample {
                program: entry.program.clone(),
                description: entry.description.clone(),
                template: entry.template.clone(),
                source: self.name().into(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smcli_core::command_source::CommandSource;

    fn fixture(dir: &Path, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("cheat.jsonl"), body).unwrap();
    }

    #[test]
    fn load_parses_header_and_entries() {
        let root = tempfile::tempdir().unwrap();
        fixture(
            root.path(),
            "{\"version\": 1, \"cheat_commit\": \"abc123\", \"license\": \"CC0-1.0\"}\n{\"program\": \"fd\", \"platform\": \"common\", \"description\": \"Find files\", \"template\": \"fd pattern\"}\n",
        );
        let corpus = load(root.path()).unwrap();
        assert_eq!(corpus.commit, "abc123");
        assert_eq!(corpus.entries.len(), 1);
        assert_eq!(corpus.entries[0].program, "fd");
    }

    #[test]
    fn source_serves_only_its_programs() {
        let root = tempfile::tempdir().unwrap();
        fixture(
            root.path(),
            "{\"version\": 1, \"cheat_commit\": \"abc123\", \"license\": \"CC0-1.0\"}\n{\"program\": \"fd\", \"platform\": \"common\", \"description\": \"Find files\", \"template\": \"fd pattern\"}\n",
        );
        let source = CheatSource::new(root.path()).unwrap();
        assert_eq!(source.name(), "cheat");
        assert_eq!(source.version(), "abc123");
        let examples = source.examples("fd", "osx");
        assert_eq!(examples.len(), 1);
        assert_eq!(examples[0].source, "cheat");
        assert!(source.examples("tar", "osx").is_empty());
    }

    #[test]
    fn missing_data_is_an_error_for_direct_construction() {
        let root = tempfile::tempdir().unwrap();
        assert!(CheatSource::new(root.path()).is_err());
    }
}
