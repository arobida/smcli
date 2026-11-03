use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

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
    let text = std::fs::read_to_string(dir.join("tldr.jsonl")).context("failed to read tldr.jsonl")?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let header: DataHeader = serde_json::from_str(lines.next().ok_or_else(|| anyhow::anyhow!("tldr.jsonl is empty"))?)?;
    ensure!(header.version == 1, "unsupported tldr data version {}", header.version);
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
    Ok(Corpus { commit: header.tldr_commit, entries })
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
                if b.contains('|') { None }
                else if a.starts_with("--") && b.starts_with('-') && !b.starts_with("--") { Some(a) }
                else if b.starts_with("--") && a.starts_with('-') && !a.starts_with("--") { Some(b) }
                else { None }
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

pub fn rank(potion: &Potion, query: &str, entries: &[Entry], prefix_words: &[String]) -> Vec<(usize, f32)> {
    let texts = std::iter::once(query.to_owned())
        .chain(entries.iter().map(|entry| entry.description.clone()))
        .collect::<Vec<_>>();
    let mut embedded = potion.embed(&texts);
    let query = embedded.remove(0);
    rank_with_vectors(&query, entries, &embedded, prefix_words)
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
            let words = entry.template.split_whitespace().collect::<Vec<_>>();
            let words = if prefix_words.first().is_some_and(|word| word == &entry.program) {
                words.as_slice()
            } else {
                words.get(1..).unwrap_or_default()
            };
            let matches = words.len() >= prefix_words.len()
                && words.iter().zip(prefix_words).all(|(word, prefix)| word == prefix);
            (index, similarity + if matches && !prefix_words.is_empty() { 0.05 } else { 0.0 })
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
}

pub fn cache_name(potion: &Potion, commit: &str) -> String {
    format!("tldr-{commit}-{}-{}-d{}.vec", Potion::NAME, potion.identity, potion.dimension())
}

#[derive(Serialize, Deserialize)]
struct CacheHeader {
    programs: Vec<String>,
}

struct CachedVectors {
    order: Vec<String>,
    vectors: HashMap<String, Vec<Vec<f32>>>,
}

fn read_cache(path: &Path, corpus: &Corpus, dimension: usize) -> Result<Option<CachedVectors>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') else { return Ok(None) };
    let Ok(header) = serde_json::from_slice::<CacheHeader>(&bytes[..newline]) else { return Ok(None) };
    if header.programs.iter().collect::<HashSet<_>>().len() != header.programs.len() {
        return Ok(None);
    }
    let counts = corpus.entries.iter().fold(HashMap::<&str, usize>::new(), |mut counts, entry| {
        *counts.entry(&entry.program).or_default() += 1;
        counts
    });
    let row_bytes = dimension.checked_mul(4).ok_or_else(|| anyhow::anyhow!("invalid vector dimension"))?;
    let expected = header.programs.iter().try_fold(0usize, |total, program| {
        counts.get(program.as_str()).copied().unwrap_or(0).checked_mul(row_bytes).and_then(|n| total.checked_add(n))
    });
    if expected != Some(bytes.len() - newline - 1) {
        return Ok(None);
    }
    let mut offset = newline + 1;
    let mut vectors = HashMap::new();
    for program in &header.programs {
        let count = counts.get(program.as_str()).copied().unwrap_or(0);
        let mut page = Vec::with_capacity(count);
        for _ in 0..count {
            let mut vector = Vec::with_capacity(dimension);
            for _ in 0..dimension {
                vector.push(f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()));
                offset += 4;
            }
            page.push(vector);
        }
        vectors.insert(program.clone(), page);
    }
    Ok(Some(CachedVectors { order: header.programs, vectors }))
}

fn write_cache(path: &Path, order: &[String], vectors: &HashMap<String, Vec<Vec<f32>>>) -> Result<()> {
    let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let temporary = path.with_file_name(format!(".{}.{}-{unique}.tmp", path.file_name().unwrap().to_string_lossy(), std::process::id()));
    let result = (|| -> Result<()> {
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary)?;
        serde_json::to_writer(&mut file, &CacheHeader { programs: order.to_vec() })?;
        file.write_all(b"\n")?;
        for program in order {
            for vector in &vectors[program] {
                for value in vector {
                    file.write_all(&value.to_le_bytes())?;
                }
            }
        }
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Vectors for installed entries in corpus order. Existing pages are reused; newly installed pages are appended.
pub fn corpus_vectors(
    potion: &Potion,
    corpus: &Corpus,
    installed: &HashSet<String>,
    index_dir: &Path,
) -> Result<Vec<Vec<f32>>> {
    std::fs::create_dir_all(index_dir)?;
    let path = index_dir.join(cache_name(potion, &corpus.commit));
    let cached = read_cache(&path, corpus, potion.dimension())?;
    let mut order = cached.as_ref().map(|c| c.order.clone()).unwrap_or_default();
    let mut vectors = cached.map(|c| c.vectors).unwrap_or_default();
    let mut seen = HashSet::new();
    let missing = corpus
        .entries
        .iter()
        .filter(|entry| installed.contains(&entry.program) && !vectors.contains_key(&entry.program))
        .filter(|entry| seen.insert(entry.program.clone()))
        .map(|entry| entry.program.clone())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        let wanted = missing.iter().collect::<HashSet<_>>();
        let missing_entries = corpus.entries.iter().filter(|entry| wanted.contains(&entry.program)).collect::<Vec<_>>();
        let embedded = potion.embed(&missing_entries.iter().map(|entry| entry.description.clone()).collect::<Vec<_>>());
        for (entry, vector) in missing_entries.into_iter().zip(embedded) {
            vectors.entry(entry.program.clone()).or_insert_with(Vec::new).push(vector);
        }
        order.extend(missing);
        write_cache(&path, &order, &vectors)?;
    }
    let mut next = HashMap::<&str, usize>::new();
    Ok(corpus
        .entries
        .iter()
        .filter(|entry| installed.contains(&entry.program))
        .filter_map(|entry| {
            let index = next.entry(&entry.program).or_default();
            let vector = vectors.get(&entry.program).and_then(|page| page.get(*index)).cloned();
            *index += 1;
            vector
        })
        .collect())
}
