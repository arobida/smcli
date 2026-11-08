use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

use crate::Potion;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub program: String,
    pub description: String,
    pub template: String,
}

#[derive(Debug, Clone)]
pub struct Corpus {
    pub commit: String,
    pub entries: Vec<Entry>,
}

#[derive(Deserialize)]
struct DataHeader {
    version: u32,
    tldr_commit: String,
}

#[derive(Deserialize)]
struct DataEntry {
    program: String,
    platform: String,
    description: String,
    template: String,
}

/// Platform name tldr pages use for this OS.
pub fn current_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "osx"
    } else {
        std::env::consts::OS
    }
}

/// tldr lives beside the binaries in installs, or under Data/ for development and tests.
pub fn bundled_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok();
    let beside = |p: Option<PathBuf>| p.and_then(|e| e.parent().map(|d| d.join("tldr")));
    [
        beside(exe.as_ref().and_then(|e| std::fs::canonicalize(e).ok())),
        beside(exe),
        Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Data/tldr")),
    ]
    .into_iter()
    .flatten()
    .find(|d| d.join("tldr.jsonl").is_file())
}

pub fn load(dir: &Path, platform: &str) -> Result<Corpus> {
    let text =
        std::fs::read_to_string(dir.join("tldr.jsonl")).context("failed to read tldr.jsonl")?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let header: DataHeader = serde_json::from_str(
        lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("tldr.jsonl is empty"))?,
    )?;
    ensure!(
        header.version == 1,
        "unsupported tldr data version {}",
        header.version
    );
    ensure!(!header.tldr_commit.is_empty(), "tldr commit is empty");
    let mut entries = Vec::new();
    for line in lines {
        let entry: DataEntry = serde_json::from_str(line).context("invalid tldr JSONL entry")?;
        if entry.platform == "common" || entry.platform == platform {
            entries.push(Entry {
                program: entry.program,
                description: entry.description,
                template: entry.template,
            });
        }
    }
    Ok(Corpus {
        commit: header.tldr_commit,
        entries,
    })
}

/// Replace tldr fields with smcli's bare placeholder. Balanced nested braces stay inside one field.
pub fn to_placeholders(template: &str) -> String {
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let content_start = start + 2;
        let mut cursor = content_start;
        let mut depth = 1usize;
        let mut end = None;
        while cursor < rest.len() {
            if rest[cursor..].starts_with("{{") {
                depth += 1;
                cursor += 2;
            } else if rest[cursor..].starts_with("}}") {
                depth -= 1;
                if depth == 0 {
                    end = Some(cursor);
                    break;
                }
                cursor += 2;
            } else {
                cursor += rest[cursor..].chars().next().unwrap().len_utf8();
            }
        }
        let Some(end) = end else {
            output.push_str(&rest[start..]);
            return output;
        };
        let content = rest[content_start..end].trim();
        let option = content
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .and_then(|s| s.split_once('|'))
            .and_then(|(a, b)| {
                let (a, b) = (a.trim(), b.trim());
                if b.contains('|') {
                    None
                } else if a.starts_with("--") && b.starts_with('-') && !b.starts_with("--") {
                    Some(a)
                } else if b.starts_with("--") && a.starts_with('-') && !a.starts_with("--") {
                    Some(b)
                } else {
                    None
                }
            });
        output.push_str(option.unwrap_or("@"));
        rest = &rest[end + 2..];
    }
    output.push_str(rest);
    output
}

/// Programs that resolve to an executable regular file (or symlink to one) on this PATH.
#[cfg(unix)]
pub fn installed(programs: &HashSet<String>, path: &str) -> HashSet<String> {
    use std::os::unix::fs::PermissionsExt;
    programs
        .iter()
        .filter(|program| {
            std::env::split_paths(&std::ffi::OsString::from(path)).any(|dir| {
                std::fs::metadata(dir.join(program))
                    .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            })
        })
        .cloned()
        .collect()
}

/// tldr as a `CommandSource`: wraps a loaded corpus and answers per program,
/// keeping the platform filter (`common` or current OS) inside the source.
#[derive(Debug, Clone)]
pub struct TldrSource {
    corpus: Corpus,
}

impl TldrSource {
    pub fn new(dir: &Path, platform: &str) -> Result<Self> {
        Ok(Self {
            corpus: load(dir, platform)?,
        })
    }

    /// All programs with a page on this platform.
    pub fn programs(&self) -> HashSet<String> {
        self.corpus
            .entries
            .iter()
            .map(|entry| entry.program.clone())
            .collect()
    }
}

impl smcli_core::command_source::CommandSource for TldrSource {
    fn name(&self) -> &str {
        "tldr"
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

pub fn rank(
    potion: &Potion,
    query: &str,
    entries: &[Entry],
    prefix_words: &[String],
) -> Vec<(usize, f32)> {
    let texts = std::iter::once(query.to_owned())
        .chain(entries.iter().map(|entry| entry.description.clone()))
        .collect::<Vec<_>>();
    let mut embedded = potion.embed(&texts);
    let query = embedded.remove(0);
    rank_with_vectors(&query, entries, &embedded, prefix_words)
}

/// Model-free ranking by lowercase word overlap, retaining the template-prefix bonus.
pub fn rank_words(query: &str, entries: &[Entry], prefix_words: &[String]) -> Vec<(usize, f32)> {
    let words: HashSet<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut ranked = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let text = format!("{} {}", entry.description, entry.template).to_lowercase();
            let entry_words: HashSet<&str> = text.split_whitespace().collect();
            let overlap = words
                .iter()
                .filter(|word| entry_words.contains(word.as_str()))
                .count() as f32;
            (index, overlap + prefix_bonus(entry, prefix_words))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
}

#[cfg(test)]
#[test]
fn rank_words_orders_by_overlap_and_prefix() {
    let entries = [
        Entry {
            program: "git".into(),
            description: "Show changes".into(),
            template: "git diff @".into(),
        },
        Entry {
            program: "git".into(),
            description: "Show changes".into(),
            template: "git status".into(),
        },
        Entry {
            program: "git".into(),
            description: "Commit changes".into(),
            template: "git commit @".into(),
        },
    ];
    let ranked = rank_words("SHOW changes", &entries, &["status".into()]);
    assert_eq!(ranked.iter().map(|r| r.0).collect::<Vec<_>>(), [1, 0, 2]);
    assert!((ranked[0].1 - 2.05).abs() < 0.0001);
    assert_eq!(
        rank_words("", &entries, &[]),
        vec![(0, 0.0), (1, 0.0), (2, 0.0)]
    );
    assert_eq!(rank_words("commit", &entries, &[])[0].0, 2);
    assert_eq!(
        rank_words("", &entries, &["git".into(), "diff".into()])[0].0,
        0
    );
}

fn prefix_bonus(entry: &Entry, prefix_words: &[String]) -> f32 {
    let words = entry.template.split_whitespace().collect::<Vec<_>>();
    let words = if prefix_words
        .first()
        .is_some_and(|word| word == &entry.program)
    {
        words.as_slice()
    } else {
        words.get(1..).unwrap_or_default()
    };
    let matches = words.len() >= prefix_words.len()
        && words
            .iter()
            .zip(prefix_words)
            .all(|(word, prefix)| word == prefix);
    if matches && !prefix_words.is_empty() {
        0.05
    } else {
        0.0
    }
}

/// Rank with page vectors already in memory; used by the picker on every query change.
pub fn rank_with_vectors(
    query: &[f32],
    entries: &[Entry],
    vectors: &[Vec<f32>],
    prefix_words: &[String],
) -> Vec<(usize, f32)> {
    debug_assert_eq!(entries.len(), vectors.len());
    let mut ranked = entries
        .iter()
        .zip(vectors)
        .enumerate()
        .map(|(index, (entry, vector))| {
            let similarity = query.iter().zip(vector).map(|(a, b)| a * b).sum::<f32>();
            (index, similarity + prefix_bonus(entry, prefix_words))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
}
