use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use smcli_core::entry::{SearchEntry, SearchFilter, trim_slash};
use smcli_core::parse::{CommandBreakdown, program_of};
use taladb::fts::HybridQuery;
use taladb::json::filter_from_json;
use taladb::{Database, Stage, TypedCollection, Value};

use crate::corpus::{Hit, SearchSignals, coverage, embedding_text, is_literal_query, parse_markers, program_arguments, query_filter, rerank, search_document};
use crate::{Potion, ShellTokenizer};

/// One indexed command; metadata fields mirror `SearchFilter` so filters narrow candidates before ranking.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexedCommand {
    /// Full-command identity (`search_document`), not the truncated text that is embedded.
    pub doc: String,
    pub command: String,
    pub terms: String,
    pub embedding: Vec<f32>,
    pub program: Option<String>,
    pub subcommand: Option<String>,
    pub flags: Vec<String>,
    pub author: Option<String>,
    /// The last directory and every ancestor, so a directory filter is a membership test.
    pub dirs: Vec<String>,
    pub last_run: Option<f64>,
    pub runs: i64,
    /// Fingerprint of the metadata fields, to update them without re-embedding.
    pub meta: String,
}

impl IndexedCommand {
    pub fn new(entry: &SearchEntry, doc: String) -> Self {
        let parsed = CommandBreakdown::parse(&entry.command);
        let program = program_of(&entry.command, parsed.as_ref());
        let subcommand = parsed.and_then(|p| p.subcommand);
        let mut dirs = vec![];
        let mut path = entry.last_directory.clone().unwrap_or_default();
        while !path.is_empty() {
            dirs.push(path.clone());
            if path == "/" {
                break;
            }
            path = Path::new(&path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        }
        let last_run = entry.last_run_epoch();
        let meta = format!("{program:?}\0{subcommand:?}\0{:?}\0{:?}\0{dirs:?}\0{last_run:?}\0{}", entry.flags, entry.author, entry.runs);
        // Terms are only needed on insert; `sync` fills them so unchanged commands skip tokenizing.
        Self {
            doc, command: entry.command.clone(), terms: String::new(), embedding: vec![], program, subcommand,
            flags: entry.flags.clone(), author: entry.author.clone(), dirs, last_run, runs: entry.runs, meta,
        }
    }

    fn metadata(&self) -> Json {
        json!({"program": self.program, "subcommand": self.subcommand, "flags": self.flags, "author": self.author,
               "dirs": self.dirs, "lastRun": self.last_run, "runs": self.runs, "meta": self.meta})
    }
}

/// The filter as a TalaDB query over `IndexedCommand`, `None` when empty. `allows` stays the final check.
pub fn index_filter(filter: &SearchFilter) -> Option<Json> {
    let mut parts = vec![];
    if let Some(program) = &filter.program { parts.push(json!({"program": program})); }
    if let Some(subcommand) = &filter.subcommand { parts.push(json!({"subcommand": subcommand})); }
    if let Some(author) = &filter.author { parts.push(json!({"author": author})); }
    if let Some(flag) = &filter.flag { parts.push(json!({"flags": {"$in": [flag]}})); }
    if let Some(directory) = &filter.directory { parts.push(json!({"dirs": {"$in": [trim_slash(directory)]}})); }
    if filter.since.is_some() || filter.before.is_some() {
        let mut range = serde_json::Map::new();
        if let Some(since) = filter.since { range.insert("$gte".into(), json!(since)); }
        if let Some(before) = filter.before { range.insert("$lt".into(), json!(before)); }
        parts.push(json!({"lastRun": range}));
    }
    if let Some(min) = filter.min_runs { parts.push(json!({"runs": {"$gte": min}})); }
    match parts.len() {
        0 => None,
        1 => parts.pop(),
        _ => Some(json!({"$and": parts})),
    }
}

/// A derived, rebuildable search index over distinct commands, kept next to the history database.
pub struct HistorySearch {
    pub directory: PathBuf,
    pub potion: Potion,
}

/// Swift's `commands-v*` files live in the same directory; this prefix keeps the two from deleting each other's index.
const VERSION: &str = "rs-commands-v1";

impl HistorySearch {
    pub fn new(directory: impl Into<PathBuf>, potion: Potion) -> Self {
        Self { directory: directory.into(), potion }
    }

    /// Embedding model, tokenizer, or document format changes need a fresh index; the name pins all three.
    pub fn index_name(&self) -> String {
        format!("{VERSION}-{}-{}-d{}", Potion::NAME, self.potion.identity, self.potion.dimension())
    }

    /// Brings the index up to date with `corpus`, then returns the best `limit` matches that pass `filter`.
    /// An empty query is allowed with a filter and lists matches by most recent run.
    /// When another process holds the index, search falls back to word matching over the corpus.
    pub fn search(&self, query: &str, corpus: &HashMap<String, SearchEntry>, limit: usize, filter: &SearchFilter,
                  programs: Option<&HashMap<String, HashSet<String>>>, signals: &SearchSignals, now: f64) -> Result<Vec<SearchEntry>> {
        if query.trim().is_empty() && !filter.is_empty() {
            let mut matches: Vec<&SearchEntry> = corpus.values().filter(|e| filter.allows(e)).collect();
            matches.sort_by(|a, b| {
                b.last_run_epoch().unwrap_or(0.0).total_cmp(&a.last_run_epoch().unwrap_or(0.0))
                    .then(b.runs.cmp(&a.runs))
                    .then_with(|| a.command.cmp(&b.command))
            });
            return Ok(matches.into_iter().take(limit).cloned().collect());
        }
        if query.trim().is_empty() {
            bail!("Search query is empty.");
        }
        let computed;
        let programs = match programs {
            Some(p) => p,
            None => {
                computed = program_arguments(corpus);
                &computed
            }
        };
        // Explicit `$word` markers take priority over automatic parsing. A marker that resolves to a
        // program, subcommand, or directory is strict; unresolved marker text (`$HOME`) is ordinary query text.
        let directories: HashSet<String> = corpus.values().filter_map(|e| e.last_directory.clone()).collect();
        let (stripped, marked_filter, has_markers) = parse_markers(query, programs, &directories);
        let parsed_filter = if has_markers { marked_filter } else { query_filter(query, programs, &directories) };
        // Literal queries lean on keywords; intent queries weigh both legs equally. Both see the
        // stripped query so `$word` markers do not accidentally make a query literal.
        let literal = is_literal_query(&stripped, programs);
        let parsed_any = has_constraint(&parsed_filter);
        let mut merged_filter = filter.clone();
        if parsed_any {
            merged_filter.merge_parsed(&parsed_filter);
        }
        // No history yet: nothing to embed, but project commands can still answer.
        if corpus.is_empty() {
            let active = addition_filter(filter, &merged_filter, has_markers, parsed_any, false);
            let hits = signals.additions(&stripped, corpus, &HashSet::new(), active, now);
            return Ok(rerank(hits, literal, &stripped, now, signals).into_iter().take(limit).collect());
        }
        let Some(_lock) = self.lock_index(20)? else {
            // The service (or a reindex) holds the index; TalaDB refuses a second process, so no semantic leg.
            return Ok(text_fallback(&stripped, corpus, limit, filter, literal, signals, now, &parsed_filter, has_markers));
        };
        let name = self.index_name();
        let db = self.open_index(&name)?;
        let commands = db.typed::<IndexedCommand>("commands")?;
        self.sync(&commands, corpus)?;
        // Filters narrow candidates in the index, so a bounded pool is enough to rerank with usage.
        let text = ShellTokenizer::encoded(&stripped);
        let vector = self.potion.embed(&[stripped.clone()]).remove(0);
        let k = (limit * 4).max(40);
        let raw = commands.raw();
        let command_of = |document: &taladb::Document| match document.get("command") {
            Some(Value::Str(command)) => Some(command.clone()),
            _ => None,
        };
        let search_with = |filter: &SearchFilter| -> Result<Vec<(String, f64)>> {
            let index_filter = index_filter(filter).map(|f| filter_from_json(&f)).transpose()?;
            if literal {
                // Weighted sum of each leg's score over its best: rank fusion would leave the runner-up level
                // with an exact keyword hit, and usage would then decide.
                let keyword: Vec<(String, f64)> = raw
                    .search_text_with("terms", &text, k, &Default::default(), index_filter.clone())?
                    .into_iter()
                    .filter_map(|hit| Some((command_of(&hit.document)?, f64::from(hit.score))))
                    .collect();
                let dense: Vec<(String, f64)> = raw
                    .find_nearest("embedding", &vector, k, index_filter)?
                    .into_iter()
                    .filter_map(|hit| Some((command_of(&hit.document)?, f64::from(hit.score))))
                    .collect();
                let mut scores: HashMap<String, f64> = HashMap::new();
                for (hits, weight) in [(keyword, 0.7), (dense, 0.3)] {
                    let best = hits.iter().map(|h| h.1).fold(0.0, f64::max);
                    if best > 0.0 {
                        for (command, score) in hits {
                            *scores.entry(command).or_insert(0.0) += weight * score / best;
                        }
                    }
                }
                let mut sorted: Vec<(String, f64)> = scores.into_iter().collect();
                sorted.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                Ok(sorted)
            } else {
                let mut hybrid = HybridQuery::new("terms", &text, "embedding", &vector, k);
                hybrid.filter = index_filter;
                Ok(raw.hybrid_search(hybrid)?
                    .into_iter()
                    .filter_map(|hit| Some((command_of(&hit.document)?, f64::from(hit.score))))
                    .collect())
            }
        };
        let active_filter: &SearchFilter = if parsed_any { &merged_filter } else { filter };
        let mut hits: Vec<Hit> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut results = search_with(active_filter)?;
        let best = results.iter().map(|r| r.1).fold(f64::NEG_INFINITY, f64::max);
        for (command, score) in results.drain(..) {
            if let Some(entry) = corpus.get(&command).filter(|e| active_filter.allows(e)) {
                if seen.insert(command.clone()) {
                    hits.push((entry.clone(), if best > 0.0 { score / best } else { 1.0 }));
                }
            }
        }
        // Auto-parsed filters get a safety net: if they return too few hits, fall back to the
        // explicit filter. When the parsed filter produced no hits at all it was likely wrong, so
        // the fallback is unscaled; otherwise fallback hits are damped so filtered hits rank first.
        // A resolved marker is strict and does not fall back. Unresolved marker text is not a constraint.
        let parsed_useful = parsed_any && !hits.is_empty();
        if hits.len() < limit && parsed_any && !has_markers {
            let mut results = search_with(filter)?;
            let best = results.iter().map(|r| r.1).fold(f64::NEG_INFINITY, f64::max);
            let scale = if parsed_useful { 0.5 } else { 1.0 };
            for (command, score) in results.drain(..) {
                if let Some(entry) = corpus.get(&command).filter(|e| filter.allows(e)) {
                    if seen.insert(command.clone()) {
                        hits.push((entry.clone(), (if best > 0.0 { score / best } else { 1.0 }) * scale));
                    }
                }
            }
        }
        drop(db);
        let final_filter = addition_filter(filter, &merged_filter, has_markers, parsed_any, parsed_useful);
        let existing: HashSet<String> = hits.iter().map(|h| h.0.command.clone()).collect();
        hits.extend(signals.additions(&stripped, corpus, &existing, final_filter, now));
        Ok(rerank(hits, literal, &stripped, now, signals).into_iter().take(limit).collect())
    }

    /// Discards the index and embeds every command again. Returns the number embedded.
    pub fn reindex(&self, corpus: &HashMap<String, SearchEntry>) -> Result<usize> {
        let Some(_lock) = self.lock_index(u32::MAX)? else { bail!("Another smcli is updating the index.") };
        let name = self.index_name();
        self.remove_files(&name);
        let db = self.open_index(&name)?;
        self.sync(&db.typed::<IndexedCommand>("commands")?, corpus)
    }

    /// Brings `commands` in line with `corpus`. Returns the number of documents now indexed.
    fn sync(&self, commands: &TypedCollection<IndexedCommand>, corpus: &HashMap<String, SearchEntry>) -> Result<usize> {
        // Keyed by the full command, not the truncated embedding text: two pasted scripts that share a
        // 1,500-character prefix must both stay retrievable. Vectors and terms still see only that bounded text.
        let wanted: HashMap<String, (IndexedCommand, String)> = corpus
            .values()
            .map(|entry| {
                let embed = embedding_text(entry);
                (entry.command.clone(), (IndexedCommand::new(entry, search_document(entry, false)), embed))
            })
            .collect();
        let indexed = fingerprints(commands)?;
        let stale: Vec<&String> = indexed
            .iter()
            .filter(|(command, (doc, _))| wanted.get(*command).map(|(row, _)| &row.doc != doc).unwrap_or(true))
            .map(|(command, _)| command)
            .collect();
        if !stale.is_empty() {
            commands.delete_many(json!({"command": {"$in": stale}}))?;
        }
        // Same identity and embedding text: usage changes update metadata and keep the vector.
        for (command, (doc, meta)) in &indexed {
            if let Some((row, _)) = wanted.get(command).filter(|(row, _)| &row.doc == doc && &row.meta != meta) {
                commands.update_one(json!({"command": command}), json!({"$set": row.metadata()}))?;
            }
        }
        let mut missing: Vec<&String> = wanted
            .keys()
            .filter(|command| match indexed.get(*command) {
                None => true,
                Some((doc, _)) => &wanted[*command].0.doc != doc,
            })
            .collect();
        missing.sort();
        for batch in missing.chunks(256) {
            let texts: Vec<String> = batch.iter().map(|command| wanted[*command].1.clone()).collect();
            let vectors = self.potion.embed(&texts);
            let documents: Vec<IndexedCommand> = batch
                .iter()
                .zip(vectors)
                .map(|(command, embedding)| IndexedCommand {
                    terms: ShellTokenizer::encoded(&wanted[*command].1),
                    embedding,
                    ..wanted[*command].0.clone()
                })
                .collect();
            commands.insert_many(&documents)?;
        }
        Ok(wanted.len())
    }

    /// The index file, ready to search. Only a corrupt or incompatible cache is discarded; other errors keep it.
    fn open_index(&self, name: &str) -> Result<Database> {
        self.open_cached(name, |db| self.setup_index(db))
    }

    fn setup_index(&self, db: &Database) -> Result<()> {
        let commands = db.typed::<IndexedCommand>("commands")?;
        for field in ["doc", "program", "subcommand", "author", "lastRun"] {
            commands.create_index(field)?;
        }
        let indexes = commands.raw().list_indexes()?;
        if !indexes.fts.iter().any(|f| f == "terms") {
            commands.create_fts_index("terms")?;
        }
        if !indexes.vector.iter().any(|f| f == "embedding") {
            // Exact search: faster than HNSW below tens of thousands of commands.
            commands.create_vector_index("embedding", self.potion.dimension(), None, None)?;
        }
        Ok(())
    }

    /// `setup` is injected so a test can fail setup without corrupting the file.
    fn open_cached(&self, name: &str, setup: impl Fn(&Database) -> Result<()>) -> Result<Database> {
        self.remove_old_indexes();
        let path = self.directory.join(format!("{name}.taladb"));
        let open = || -> Result<Database> {
            let db = taladb::open(&path)?;
            setup(&db)?;
            Ok(db)
        };
        match open() {
            Ok(db) => Ok(db),
            Err(err) if index_cache_unusable(&err) => {
                self.remove_files(name);
                open().map_err(|retry| retry.context(format!("while rebuilding a corrupt index: {err:#}")))
            }
            Err(err) => Err(err),
        }
    }

    /// The index file and any sidecars the engine keeps next to it.
    fn files(&self, name: &str) -> Vec<PathBuf> {
        let prefix = format!("{name}.");
        std::fs::read_dir(&self.directory)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
                    .map(|e| e.path())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn remove_files(&self, name: &str) {
        for file in self.files(name) {
            let _ = std::fs::remove_file(file);
        }
    }

    /// Indexes from earlier document formats are never read again.
    fn remove_old_indexes(&self) {
        let Ok(entries) = std::fs::read_dir(&self.directory) else { return };
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("rs-commands-v") && !name.starts_with(&format!("{VERSION}-")) {
                let path = entry.path();
                let _ = if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) };
            }
        }
    }

    /// Serializes index writers with the same `lock` file (flock) the Swift build uses. Tries `attempts` times,
    /// 50 ms apart; `None` when another process still holds it.
    fn lock_index(&self, attempts: u32) -> Result<Option<File>> {
        self.prepare_directory()?;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(self.directory.join("lock"))?;
        for attempt in 0..attempts {
            match lock.try_lock() {
                Ok(()) => return Ok(Some(lock)),
                Err(std::fs::TryLockError::WouldBlock) if attempt + 1 < attempts => {
                    std::thread::sleep(std::time::Duration::from_millis(50))
                }
                Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
                Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
            }
        }
        Ok(None)
    }

    /// The index holds command text, so it gets the same privacy as the history database.
    fn prepare_directory(&self) -> Result<()> {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&self.directory)?;
        let info = std::fs::symlink_metadata(&self.directory)?;
        // SAFETY: getuid has no preconditions.
        if !info.is_dir() || info.uid() != unsafe { libc::getuid() } || info.mode() & 0o077 != 0 {
            bail!("Search index must be a private directory owned by you (chmod 700): {}", self.directory.display());
        }
        Ok(())
    }
}

/// Each indexed command's document identity and metadata fingerprint. Projected, since decoding every embedding is slow.
fn fingerprints(commands: &TypedCollection<IndexedCommand>) -> Result<HashMap<String, (String, String)>> {
    let rows = commands.raw().aggregate(vec![Stage::Project {
        fields: vec!["command".into(), "doc".into(), "meta".into()],
        include: true,
        keep_id: false,
    }])?;
    Ok(rows
        .iter()
        .filter_map(|row| match (row.get("command"), row.get("doc"), row.get("meta")) {
            (Some(Value::Str(command)), Some(Value::Str(doc)), Some(Value::Str(meta))) => Some((command.clone(), (doc.clone(), meta.clone()))),
            _ => None,
        })
        .collect())
}

fn has_constraint(filter: &SearchFilter) -> bool {
    filter.program.is_some() || filter.subcommand.is_some() || filter.directory.is_some()
}

/// Picks and project additions keep a resolved `$word` marker even when retrieval is empty.
/// An automatic parse is kept only when it actually matched. Unresolved marker text is not a constraint.
fn addition_filter<'a>(explicit: &'a SearchFilter, merged: &'a SearchFilter, has_markers: bool, parsed_any: bool, retrieved: bool) -> &'a SearchFilter {
    if (has_markers && parsed_any) || retrieved { merged } else { explicit }
}

/// Corrupt or incompatible cache. A full disk, a permission error, or a failed setup step is not.
/// redb reports a bad magic number as InvalidData text, not as `Corrupted`.
fn index_cache_unusable(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        let msg = cause.to_string();
        msg.contains("Manual upgrade required")
            || msg.contains("file is version")
            || msg.contains("corrupt")
            || msg.contains("Not a redb database")
            || msg.contains("magic number mismatch")
            || matches!(
                cause.downcast_ref::<taladb::TalaDbError>(),
                Some(
                    taladb::TalaDbError::Serialization(_)
                        | taladb::TalaDbError::Postcard(_)
                        | taladb::TalaDbError::VectorDimensionMismatch { .. }
                        | taladb::TalaDbError::TypeError { .. }
                        | taladb::TalaDbError::Migration(_)
                )
            )
    })
}

/// Search without the index: commands sharing the query's words, by how much of the query they cover.
/// `query` is already stripped of `$word` markers; `parsed_filter` and `has_markers` come from the
/// same parsing done on the indexed path so a held lock does not change ranking behavior.
fn text_fallback(query: &str, corpus: &HashMap<String, SearchEntry>, limit: usize, filter: &SearchFilter, literal: bool,
                 signals: &SearchSignals, now: f64, parsed_filter: &SearchFilter, has_markers: bool) -> Vec<SearchEntry> {
    let parsed_any = has_constraint(parsed_filter);
    let mut merged_filter = filter.clone();
    if parsed_any {
        merged_filter.merge_parsed(parsed_filter);
    }
    let terms: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
    let mut hits: Vec<Hit> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    if parsed_any {
        let mut filtered: Vec<Hit> = corpus
            .values()
            .filter(|e| merged_filter.allows(e))
            .filter_map(|e| Some((e.clone(), Some(coverage(&e.command, &terms)).filter(|c| *c > 0.0)?)))
            .collect();
        filtered.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.command.cmp(&b.0.command)));
        filtered.truncate((limit * 4).max(40));
        for (entry, score) in filtered {
            if seen.insert(entry.command.clone()) {
                hits.push((entry, score));
            }
        }
    }
    let parsed_useful = parsed_any && !hits.is_empty();
    // A resolved marker is strict. Unresolved `$HOME` text must not suppress ordinary matching.
    if hits.len() < limit && !(has_markers && parsed_any) {
        let mut fallback: Vec<Hit> = corpus
            .values()
            .filter(|e| filter.allows(e))
            .filter_map(|e| Some((e.clone(), Some(coverage(&e.command, &terms)).filter(|c| *c > 0.0)?)))
            .collect();
        fallback.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.command.cmp(&b.0.command)));
        fallback.truncate((limit * 4).max(40));
        let scale = if parsed_useful { 0.5 } else { 1.0 };
        for (entry, score) in fallback {
            if seen.insert(entry.command.clone()) {
                hits.push((entry, score * scale));
            }
        }
    }
    let final_filter = addition_filter(filter, &merged_filter, has_markers, parsed_any, parsed_useful);
    let existing: HashSet<String> = hits.iter().map(|h| h.0.command.clone()).collect();
    hits.extend(signals.additions(query, corpus, &existing, final_filter, now));
    rerank(hits, literal, query, now, signals).into_iter().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_search_filters_to_index_filters() {
        assert_eq!(index_filter(&SearchFilter::default()), None);
        assert_eq!(index_filter(&SearchFilter { program: Some("git".into()), ..Default::default() }), Some(json!({"program": "git"})));
        let filter = SearchFilter { min_runs: Some(2), directory: Some("/repo/".into()), flag: Some("-m".into()), ..Default::default() };
        assert_eq!(index_filter(&filter), Some(json!({"$and": [{"flags": {"$in": ["-m"]}}, {"dirs": {"$in": ["/repo"]}}, {"runs": {"$gte": 2}}]})));
        let range = SearchFilter { since: Some(10.0), before: Some(20.0), ..Default::default() };
        assert_eq!(index_filter(&range), Some(json!({"lastRun": {"$gte": 10.0, "$lt": 20.0}})));
        let entry = SearchEntry { last_directory: Some("/repo/app".into()), ..SearchEntry::new("git status", 1, "captured") };
        assert_eq!(IndexedCommand::new(&entry, "git status".into()).dirs, ["/repo/app", "/repo", "/"]);
    }

    #[test]
    fn search_the_index_and_fall_back_when_it_is_locked() {
        let dir = tempfile::tempdir().unwrap();
        let engine = HistorySearch::new(dir.path().join("index"), Potion::bundled().unwrap());
        let corpus: HashMap<String, SearchEntry> = ["git status", "docker ps", "ls -la"]
            .iter()
            .map(|c| (c.to_string(), SearchEntry::new(c, 1, "native")))
            .collect();
        let none = SearchSignals::default();
        let filter = SearchFilter::default();
        let hits = engine.search("docker", &corpus, 3, &filter, None, &none, 0.0).unwrap();
        assert_eq!(hits[0].command, "docker ps");
        // Removing a command from history removes it from the index.
        let smaller: HashMap<String, SearchEntry> = corpus.iter().filter(|(k, _)| *k != "docker ps").map(|(k, v)| (k.clone(), v.clone())).collect();
        assert!(!engine.search("docker", &smaller, 3, &filter, None, &none, 0.0).unwrap().iter().any(|e| e.command == "docker ps"));
        // Another process holding the lock gets word matching instead of waiting.
        let held = engine.lock_index(1).unwrap().unwrap();
        let fallback = engine.search("git", &corpus, 3, &filter, None, &none, 0.0).unwrap();
        assert_eq!(fallback.iter().map(|e| e.command.as_str()).collect::<Vec<_>>(), ["git status"]);
        drop(held);
    }

    #[test]
    fn setup_creates_expected_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let engine = HistorySearch::new(dir.path().join("index"), Potion::bundled().unwrap());
        let _lock = engine.lock_index(1).unwrap().unwrap();
        let name = engine.index_name();
        let db = engine.open_index(&name).unwrap();
        let commands = db.typed::<IndexedCommand>("commands").unwrap();
        let info = commands.raw().list_indexes().unwrap();
        for field in ["doc", "program", "subcommand", "author", "lastRun"] {
            assert!(info.btree.contains(&field.to_string()), "missing btree index on {field}");
        }
        assert!(info.fts.contains(&"terms".to_string()), "missing fts index on terms");
        assert!(info.vector.contains(&"embedding".to_string()), "missing vector index on embedding");
    }

    #[test]
    fn setup_twice_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let engine = HistorySearch::new(dir.path().join("index"), Potion::bundled().unwrap());
        let corpus: HashMap<String, SearchEntry> = ["git status", "docker ps"]
            .iter()
            .map(|c| (c.to_string(), SearchEntry::new(c, 1, "native")))
            .collect();
        let none = SearchSignals::default();
        engine.search("docker", &corpus, 1, &SearchFilter::default(), None, &none, 0.0).unwrap();

        let _lock = engine.lock_index(1).unwrap().unwrap();
        let name = engine.index_name();
        let first = engine.open_index(&name).unwrap();
        let commands = first.typed::<IndexedCommand>("commands").unwrap();
        let info = commands.raw().list_indexes().unwrap();
        let count = commands.count(json!({})).unwrap();
        drop(commands);
        drop(first);

        let second = engine.open_index(&name).unwrap();
        let commands2 = second.typed::<IndexedCommand>("commands").unwrap();
        assert_eq!(commands2.count(json!({})).unwrap(), count);
        assert_eq!(commands2.raw().list_indexes().unwrap().btree, info.btree);
        assert_eq!(commands2.raw().list_indexes().unwrap().fts, info.fts);
        assert_eq!(commands2.raw().list_indexes().unwrap().vector, info.vector);
    }

    #[test]
    fn stored_vector_dimension_matches_model() {
        let dir = tempfile::tempdir().unwrap();
        let engine = HistorySearch::new(dir.path().join("index"), Potion::bundled().unwrap());
        let corpus: HashMap<String, SearchEntry> = [("docker ps".to_string(), SearchEntry::new("docker ps", 1, "native"))].into_iter().collect();
        engine.search("docker", &corpus, 1, &SearchFilter::default(), None, &SearchSignals::default(), 0.0).unwrap();

        let name = engine.index_name();
        let db = engine.open_index(&name).unwrap();
        let commands = db.typed::<IndexedCommand>("commands").unwrap();
        let indexed = commands.find_one(json!({"command": "docker ps"})).unwrap().unwrap();
        assert_eq!(indexed.embedding.len(), engine.potion.dimension());
    }

    #[test]
    fn hybrid_query_returns_expected_doc() {
        let dir = tempfile::tempdir().unwrap();
        let engine = HistorySearch::new(dir.path().join("index"), Potion::bundled().unwrap());
        let corpus: HashMap<String, SearchEntry> = ["git status", "docker ps", "ls -la"]
            .iter()
            .map(|c| (c.to_string(), SearchEntry::new(c, 1, "native")))
            .collect();
        // "ps docker" is not a literal command, so this exercises the HybridQuery path.
        let hits = engine.search("ps docker", &corpus, 3, &SearchFilter::default(), None, &SearchSignals::default(), 0.0).unwrap();
        assert!(hits.iter().any(|e| e.command == "docker ps"), "hybrid query missed docker ps: {:?}", hits);
    }

    #[test]
    fn filters_narrow_results() {
        let dir = tempfile::tempdir().unwrap();
        let engine = HistorySearch::new(dir.path().join("index"), Potion::bundled().unwrap());
        let mut git = SearchEntry::new("git status", 1, "native");
        git.last_directory = Some("/repo/app".into());
        git.author = Some("alice".into());
        git.flags = vec!["-v".into()];
        let mut docker = SearchEntry::new("docker ps", 1, "native");
        docker.last_directory = Some("/other".into());
        docker.author = Some("bob".into());
        let corpus: HashMap<String, SearchEntry> = [(git.command.clone(), git), (docker.command.clone(), docker)].into_iter().collect();
        let none = SearchSignals::default();

        let only_git = SearchFilter { program: Some("git".into()), ..Default::default() };
        let hits = engine.search("status", &corpus, 3, &only_git, None, &none, 0.0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].command, "git status");

        let in_repo = SearchFilter { directory: Some("/repo".into()), ..Default::default() };
        let hits = engine.search("status", &corpus, 3, &in_repo, None, &none, 0.0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].command, "git status");

        let with_flag = SearchFilter { flag: Some("-v".into()), ..Default::default() };
        let hits = engine.search("status", &corpus, 3, &with_flag, None, &none, 0.0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].command, "git status");

        let by_author = SearchFilter { author: Some("bob".into()), ..Default::default() };
        let hits = engine.search("ps", &corpus, 3, &by_author, None, &none, 0.0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].command, "docker ps");
    }

    #[test]
    fn unresolved_markers_still_match_when_the_index_is_locked() {
        let dir = tempfile::tempdir().unwrap();
        let engine = HistorySearch::new(dir.path().join("index"), Potion::bundled().unwrap());
        let corpus: HashMap<String, SearchEntry> = ["echo hello", "git status", "docker ps"]
            .iter()
            .map(|c| (c.to_string(), SearchEntry::new(c, 1, "native")))
            .collect();
        let held = engine.lock_index(1).unwrap().unwrap();
        let unresolved = engine.search("echo $HOME", &corpus, 5, &SearchFilter::default(), None, &SearchSignals::default(), 0.0).unwrap();
        assert!(unresolved.iter().any(|e| e.command == "echo hello"), "{unresolved:?}");
        let strict = engine.search("$git ps", &corpus, 5, &SearchFilter::default(), None, &SearchSignals::default(), 0.0).unwrap();
        assert!(strict.iter().any(|e| e.command == "git status"), "{strict:?}");
        assert!(!strict.iter().any(|e| e.command == "docker ps"), "{strict:?}");
        drop(held);
    }

    #[test]
    fn strict_markers_reject_outside_selections_when_nothing_matches() {
        let dir = tempfile::tempdir().unwrap();
        let engine = HistorySearch::new(dir.path().join("index"), Potion::bundled().unwrap());
        let mut git = SearchEntry::new("git status", 1, "captured");
        git.last_directory = Some("/repo".into());
        let mut docker = SearchEntry::new("docker ps", 1, "captured");
        docker.last_directory = Some("/other".into());
        let corpus: HashMap<String, SearchEntry> = [(git.command.clone(), git), (docker.command.clone(), docker)].into_iter().collect();
        let mut picks = smcli_core::ranking::SelectionLog::default();
        picks.add("git other", "docker ps", 0.0);
        let signals = SearchSignals { picks, ..Default::default() };
        let query = "$git $other";
        let indexed = engine.search(query, &corpus, 5, &SearchFilter::default(), None, &signals, 0.0).unwrap();
        assert!(indexed.is_empty(), "indexed: {indexed:?}");
        let held = engine.lock_index(1).unwrap().unwrap();
        let locked = engine.search(query, &corpus, 5, &SearchFilter::default(), None, &signals, 0.0).unwrap();
        assert!(locked.is_empty(), "locked: {locked:?}");
        drop(held);
    }

    #[test]
    fn colliding_command_prefixes_keep_both_identities() {
        let dir = tempfile::tempdir().unwrap();
        let engine = HistorySearch::new(dir.path().join("index"), Potion::bundled().unwrap());
        let prefix = "x".repeat(1_500);
        let alpha = format!("{prefix} ALPHA");
        let beta = format!("{prefix} BETA");
        let mut first = SearchEntry::new(&alpha, 1, "captured");
        let mut second = SearchEntry::new(&beta, 2, "captured");
        first.intent = Some("paste".into());
        second.intent = Some("paste".into());
        let corpus = HashMap::from([(alpha.clone(), first.clone()), (beta.clone(), second.clone())]);
        engine.search("x", &corpus, 5, &SearchFilter::default(), None, &SearchSignals::default(), 0.0).unwrap();
        let rows = indexed_commands(&engine);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows.iter().find(|r| r.command == alpha).unwrap().embedding, rows.iter().find(|r| r.command == beta).unwrap().embedding);

        first.runs = 9;
        let corpus = HashMap::from([(alpha.clone(), first), (beta.clone(), second)]);
        engine.search("x", &corpus, 5, &SearchFilter::default(), None, &SearchSignals::default(), 0.0).unwrap();
        let rows = indexed_commands(&engine);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows.iter().find(|r| r.command == alpha).unwrap().runs, 9);
        assert_eq!(rows.iter().find(|r| r.command == beta).unwrap().runs, 2);
    }

    #[test]
    fn setup_failure_keeps_the_cache_and_corrupt_cache_is_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        let engine = HistorySearch::new(dir.path().join("index"), Potion::bundled().unwrap());
        let corpus: HashMap<String, SearchEntry> = [("docker ps".into(), SearchEntry::new("docker ps", 1, "native"))].into_iter().collect();
        engine.search("docker", &corpus, 1, &SearchFilter::default(), None, &SearchSignals::default(), 0.0).unwrap();
        let name = engine.index_name();
        let err = match engine.open_cached(&name, |_| bail!("transient setup I/O failure")) {
            Err(err) => err,
            Ok(_) => panic!("setup failure was ignored"),
        };
        assert!(err.to_string().contains("transient setup I/O failure"), "{err:#}");
        assert_eq!(indexed_commands(&engine).len(), 1);

        let path = engine.directory.join(format!("{name}.taladb"));
        std::fs::write(&path, b"this is not a taladb file").unwrap();
        let hits = engine.search("docker", &corpus, 1, &SearchFilter::default(), None, &SearchSignals::default(), 0.0).unwrap();
        assert_eq!(hits[0].command, "docker ps");
    }

    fn indexed_commands(engine: &HistorySearch) -> Vec<IndexedCommand> {
        let db = engine.open_index(&engine.index_name()).unwrap();
        db.typed::<IndexedCommand>("commands").unwrap().find(json!({})).unwrap()
    }
}
