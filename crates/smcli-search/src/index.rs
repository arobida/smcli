use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use smcli_core::entry::{SearchEntry, SearchFilter, trim_slash};
use smcli_core::parse::{CommandBreakdown, program_of};
use smcli_core::ranking::{Signal, uses};
use taladb::fts::HybridQuery;
use taladb::json::filter_from_json;
use taladb::{Database, Stage, TypedCollection, Value};

use crate::corpus::{
    Hit, SearchSignals, embedding_text, entry_coverage, is_literal_query, lexical_text,
    parse_markers, program_arguments, query_filter, search_document,
};
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
        let parsed = CommandBreakdown::parse(entry.primary());
        let program = program_of(entry.primary(), parsed.as_ref());
        let subcommand = parsed.and_then(|p| p.subcommand);
        let mut dirs = vec![];
        let mut path = trim_slash(entry.last_directory.as_deref().unwrap_or_default()).to_string();
        while !path.is_empty() {
            dirs.push(path.clone());
            if path == "/" {
                break;
            }
            path = Path::new(&path)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        let last_run = entry.last_run_epoch();
        let meta = format!(
            "{program:?}\0{subcommand:?}\0{:?}\0{:?}\0{dirs:?}\0{last_run:?}\0{}",
            entry.flags, entry.author, entry.runs
        );
        // Terms are only needed on insert; `sync` fills them so unchanged commands skip tokenizing.
        Self {
            doc,
            command: entry.key(),
            terms: String::new(),
            embedding: vec![],
            program,
            subcommand,
            flags: entry.flags.clone(),
            author: entry.author.clone(),
            dirs,
            last_run,
            runs: entry.runs,
            meta,
        }
    }

    fn metadata(&self) -> Json {
        json!({"program": self.program, "subcommand": self.subcommand, "flags": self.flags, "author": self.author,
               "dirs": self.dirs, "lastRun": self.last_run, "runs": self.runs, "meta": self.meta})
    }
}

/// Command-index descriptions consulted per search, and the similarity they must reach.
const BRIDGE_MATCHES: usize = 3;
const BRIDGE_FLOOR: f64 = 0.60;

/// Affinity uses indexed metadata and only scores history candidates, never project additions.
fn command_affinity(
    commands: &TypedCollection<IndexedCommand>,
    hits: &[Hit],
    matches: &[crate::commands::DescriptionMatch],
) -> Result<HashMap<String, f64>> {
    if matches.is_empty() {
        return Ok(HashMap::new());
    }
    let templates = matches
        .iter()
        .map(|matched| {
            let mut entry = SearchEntry::new(&matched.template, 0, "template");
            entry.flags = smcli_core::parse::command_flags(&matched.template);
            IndexedCommand::new(&entry, String::new())
        })
        .collect::<Vec<_>>();
    let keys = hits
        .iter()
        .map(|(entry, _)| entry.key())
        .collect::<Vec<_>>();
    Ok(commands
        .find(json!({"command": {"$in": keys}}))?
        .into_iter()
        .map(|entry| {
            let bonus = templates
                .iter()
                .filter(|template| entry.program.is_some() && entry.program == template.program)
                .map(|template| {
                    let subcommand =
                        entry.subcommand.is_some() && entry.subcommand == template.subcommand;
                    let flags = if template.flags.is_empty() {
                        0.0
                    } else {
                        template
                            .flags
                            .iter()
                            .filter(|flag| entry.flags.contains(flag))
                            .count() as f64
                            / template.flags.len() as f64
                    };
                    0.01 + if subcommand { 0.02 } else { 0.0 } + 0.02 * flags
                })
                .fold(0.0, f64::max);
            (entry.command, bonus)
        })
        .collect())
}

/// The filter as a TalaDB query over `IndexedCommand`, `None` when empty. `allows` stays the final check.
pub fn index_filter(filter: &SearchFilter) -> Option<Json> {
    let mut parts = vec![];
    if let Some(program) = &filter.program {
        parts.push(json!({"program": program}));
    }
    if let Some(subcommand) = &filter.subcommand {
        parts.push(json!({"subcommand": subcommand}));
    }
    if let Some(author) = &filter.author {
        parts.push(json!({"author": author}));
    }
    if let Some(flag) = &filter.flag {
        parts.push(json!({"flags": {"$in": [flag]}}));
    }
    if let Some(directory) = &filter.directory {
        parts.push(json!({"dirs": {"$in": [trim_slash(directory)]}}));
    }
    if filter.since.is_some() || filter.before.is_some() {
        let mut range = serde_json::Map::new();
        if let Some(since) = filter.since {
            range.insert("$gte".into(), json!(since));
        }
        if let Some(before) = filter.before {
            range.insert("$lt".into(), json!(before));
        }
        parts.push(json!({"lastRun": range}));
    }
    if let Some(min) = filter.min_runs {
        parts.push(json!({"runs": {"$gte": min}}));
    }
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

/// History-index version prefix. A rebuild deletes older history indexes and leaves command-example caches alone.
const VERSION: &str = "rs-commands-v4";

#[cfg(test)]
thread_local! { pub(crate) static VECTOR_FIRST: std::cell::Cell<bool> = const { std::cell::Cell::new(true) }; }
fn vector_first() -> bool {
    #[cfg(test)]
    {
        VECTOR_FIRST.with(|v| v.get())
    }
    #[cfg(not(test))]
    {
        true
    }
}

struct QueryPlan<'a> {
    corpus: std::borrow::Cow<'a, HashMap<String, SearchEntry>>,
    stripped: String,
    parsed_filter: SearchFilter,
    has_markers: bool,
    literal: bool,
    alias_query: bool,
    merged_filter: SearchFilter,
    parsed_any: bool,
    alias_off: bool,
}

fn plan_query<'a>(
    query: &str,
    corpus: &'a HashMap<String, SearchEntry>,
    limit: usize,
    filter: &SearchFilter,
    programs: Option<&HashMap<String, HashSet<String>>>,
    signals: &SearchSignals,
) -> Result<Result<QueryPlan<'a>, Vec<SearchEntry>>> {
    let alias_off = !uses(signals.context.as_ref(), Signal::Alias);
    let typed_only = alias_off.then(|| {
        corpus
            .iter()
            .map(|(key, entry)| {
                let mut entry = entry.clone();
                entry.alias_off = true;
                entry.flags = smcli_core::parse::command_flags(&entry.command);
                (key.clone(), entry)
            })
            .collect::<HashMap<_, _>>()
    });
    let original = corpus;
    let corpus = typed_only.as_ref().unwrap_or(original);
    let programs = if alias_off { None } else { programs };
    if query.trim().is_empty() && !filter.is_empty() {
        let mut matches: Vec<&SearchEntry> = corpus.values().filter(|e| filter.allows(e)).collect();
        matches.sort_by(|a, b| {
            b.last_run_epoch()
                .unwrap_or(0.0)
                .total_cmp(&a.last_run_epoch().unwrap_or(0.0))
                .then(b.runs.cmp(&a.runs))
                .then_with(|| a.command.cmp(&b.command))
        });
        return Ok(Err(matches.into_iter().take(limit).cloned().collect()));
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
    let directories: HashSet<String> = corpus
        .values()
        .filter_map(|e| e.last_directory.clone())
        .collect();
    let (stripped, marked_filter, has_markers) = parse_markers(query, programs, &directories);
    let parsed_filter = if has_markers {
        marked_filter
    } else {
        query_filter(query, programs, &directories)
    };
    // Interpret stripped text so `$word` markers do not accidentally make intent literal.
    let literal = is_literal_query(&stripped, programs);
    // An exact typed alias is an explicit spelling, even when only its expansion is a known program.
    let alias_query = corpus.values().any(|e| {
        e.secondary()
            .is_some_and(|s| s.eq_ignore_ascii_case(stripped.trim()))
    });
    let parsed_any = has_constraint(&parsed_filter);
    let mut merged_filter = filter.clone();
    if parsed_any {
        merged_filter.merge_parsed(&parsed_filter);
    }

    let corpus = match typed_only {
        Some(corpus) => std::borrow::Cow::Owned(corpus),
        None => std::borrow::Cow::Borrowed(original),
    };
    Ok(Ok(QueryPlan {
        corpus,
        stripped,
        parsed_filter,
        has_markers,
        literal,
        alias_query,
        merged_filter,
        parsed_any,
        alias_off,
    }))
}

/// Keyword-only search: never opens an index or loads an embedding model.
pub fn lexical_search(
    query: &str,
    corpus: &HashMap<String, SearchEntry>,
    limit: usize,
    filter: &SearchFilter,
    signals: &SearchSignals,
    now: f64,
) -> Result<Vec<SearchEntry>> {
    let plan = match plan_query(query, corpus, limit, filter, None, signals)? {
        Ok(plan) => plan,
        Err(matches) => return Ok(matches),
    };
    let QueryPlan {
        corpus,
        stripped,
        parsed_filter,
        has_markers,
        literal,
        merged_filter,
        parsed_any,
        ..
    } = plan;
    if corpus.is_empty() {
        let active = addition_filter(filter, &merged_filter, has_markers, parsed_any, false);
        let hits = signals.additions(&stripped, &corpus, &HashSet::new(), active, now);
        return Ok(crate::corpus::rerank_with_evidence(
            hits, literal, &stripped, now, signals, None,
        )
        .into_iter()
        .take(limit)
        .collect());
    }
    Ok(text_fallback_with_evidence(
        &stripped,
        &corpus,
        limit,
        filter,
        literal,
        signals,
        now,
        &parsed_filter,
        has_markers,
        None,
    ))
}

pub fn index_name(model: &crate::ModelId) -> String {
    format!(
        "{VERSION}-{}-{}-d{}",
        Potion::NAME,
        model.identity,
        model.dimension
    )
}

impl HistorySearch {
    pub fn new(directory: impl Into<PathBuf>, potion: Potion) -> Self {
        Self {
            directory: directory.into(),
            potion,
        }
    }

    /// Embedding model, tokenizer, or document format changes need a fresh index; the name pins all three.
    pub fn index_name(&self) -> String {
        index_name(&self.potion.model_id())
    }

    /// Brings the index up to date with `corpus`, then returns the best `limit` matches that pass `filter`.
    /// An empty query is allowed with a filter and lists matches by most recent run.
    /// Under index contention, literals use word matching; intent embeds the in-memory corpus.
    pub fn search(
        &self,
        query: &str,
        corpus: &HashMap<String, SearchEntry>,
        limit: usize,
        filter: &SearchFilter,
        programs: Option<&HashMap<String, HashSet<String>>>,
        signals: &SearchSignals,
        now: f64,
    ) -> Result<Vec<SearchEntry>> {
        self.search_with_evidence(query, corpus, limit, filter, programs, signals, now, None)
    }

    pub fn search_with_evidence(
        &self,
        query: &str,
        corpus: &HashMap<String, SearchEntry>,
        limit: usize,
        filter: &SearchFilter,
        programs: Option<&HashMap<String, HashSet<String>>>,
        signals: &SearchSignals,
        now: f64,
        evidence: Option<&dyn crate::ProjectEvidence>,
    ) -> Result<Vec<SearchEntry>> {
        let lock = self.lock_index(20)?;
        self.search_with_evidence_using_lock(
            query,
            corpus,
            limit,
            filter,
            programs,
            signals,
            now,
            evidence,
            lock.as_ref(),
        )
    }

    /// `lock` must be acquired before reading the corpus; without it only the text fallback is used.
    pub fn search_with_evidence_using_lock(
        &self,
        query: &str,
        corpus: &HashMap<String, SearchEntry>,
        limit: usize,
        filter: &SearchFilter,
        programs: Option<&HashMap<String, HashSet<String>>>,
        signals: &SearchSignals,
        now: f64,
        evidence: Option<&dyn crate::ProjectEvidence>,
        lock: Option<&File>,
    ) -> Result<Vec<SearchEntry>> {
        self.search_with_command_index(
            query, corpus, limit, filter, programs, signals, now, evidence, lock, None,
        )
    }

    /// Service-only bridge; an unavailable command index leaves the existing search path unchanged.
    pub fn search_with_command_index(
        &self,
        query: &str,
        corpus: &HashMap<String, SearchEntry>,
        limit: usize,
        filter: &SearchFilter,
        programs: Option<&HashMap<String, HashSet<String>>>,
        signals: &SearchSignals,
        now: f64,
        evidence: Option<&dyn crate::ProjectEvidence>,
        lock: Option<&File>,
        command_index: Option<&crate::commands::CommandIndex>,
    ) -> Result<Vec<SearchEntry>> {
        let plan = match plan_query(query, corpus, limit, filter, programs, signals)? {
            Ok(plan) => plan,
            Err(matches) => return Ok(matches),
        };
        let QueryPlan {
            corpus,
            stripped,
            parsed_filter,
            has_markers,
            literal,
            alias_query,
            merged_filter,
            parsed_any,
            alias_off,
        } = plan;
        let corpus = &corpus;
        if vector_first() && !literal && !alias_query && (corpus.is_empty() || lock.is_none()) {
            let active = if has_markers && parsed_any {
                &merged_filter
            } else {
                filter
            };
            return Ok(self.intent_search(&stripped, corpus, limit, active, signals, None));
        }
        // No history yet: nothing to embed, but project commands can still answer.
        if corpus.is_empty() {
            let active = addition_filter(filter, &merged_filter, has_markers, parsed_any, false);
            let hits = signals.additions(&stripped, corpus, &HashSet::new(), active, now);
            return Ok(crate::corpus::rerank_with_evidence(
                hits, literal, &stripped, now, signals, evidence,
            )
            .into_iter()
            .take(limit)
            .collect());
        }
        if lock.is_none() {
            // The service (or a reindex) holds the index; TalaDB refuses a second process, so no semantic leg.
            return Ok(text_fallback_with_evidence(
                &stripped,
                corpus,
                limit,
                filter,
                literal,
                signals,
                now,
                &parsed_filter,
                has_markers,
                evidence,
            ));
        };
        let name = if alias_off {
            format!("{}-typed", self.index_name())
        } else {
            self.index_name()
        };
        let db = self.open_index(&name)?;
        let commands = db.typed::<IndexedCommand>("commands")?;
        self.sync(&db, &commands, corpus)?;
        if vector_first() && !literal && !alias_query {
            let active = if has_markers && parsed_any {
                &merged_filter
            } else {
                filter
            };
            let vocabulary = crate::corpus::query_vocabulary(corpus, signals);
            let prepared = crate::corpus::prepare_intent_query(
                &stripped,
                &vocabulary,
                Some(&self.potion.vocabulary),
            );
            let vector = self.potion.embed(&[prepared.embedding_query()]).remove(0);
            let mut hits: Vec<Hit> = commands
                .raw()
                .find_nearest(
                    "embedding",
                    &vector,
                    (limit * 4).max(40),
                    index_filter(active)
                        .map(|f| filter_from_json(&f))
                        .transpose()?,
                )?
                .into_iter()
                .filter_map(|hit| {
                    let Some(Value::Str(key)) = hit.document.get("command") else {
                        return None;
                    };
                    Some((
                        corpus.get(key).filter(|e| active.allows(e))?.clone(),
                        f64::from(hit.score),
                    ))
                })
                .collect();
            let matches = command_index
                .map(|index| {
                    index
                        .description_matches(&vector, BRIDGE_MATCHES, BRIDGE_FLOOR)
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            if !matches.is_empty() {
                // Commit the whole recall leg only on success, so contention cannot leave partial results.
                let recalled = (|| -> Result<Vec<Hit>> {
                    let templates = matches
                        .iter()
                        .map(|m| m.template.clone())
                        .collect::<Vec<_>>();
                    let vectors = self.potion.embed(&templates);
                    let mut recalled = Vec::new();
                    for (matched, template_vector) in matches.iter().zip(vectors) {
                        for hit in commands.raw().find_nearest(
                            "embedding",
                            &template_vector,
                            (limit * 4).max(40),
                            index_filter(active)
                                .map(|f| filter_from_json(&f))
                                .transpose()?,
                        )? {
                            let Some(Value::Str(key)) = hit.document.get("command") else {
                                continue;
                            };
                            if let Some(entry) = corpus.get(key).filter(|e| active.allows(e)) {
                                recalled.push((
                                    entry.clone(),
                                    matched.similarity * f64::from(hit.score),
                                ));
                            }
                        }
                    }
                    Ok(recalled)
                })();
                if let Ok(recalled) = recalled {
                    let mut position: HashMap<String, usize> = hits
                        .iter()
                        .enumerate()
                        .map(|(i, h)| (h.0.key(), i))
                        .collect();
                    for (entry, score) in recalled {
                        match position.entry(entry.key()) {
                            std::collections::hash_map::Entry::Occupied(i) => {
                                let hit = &mut hits[*i.get()];
                                hit.1 = hit.1.max(score);
                            }
                            std::collections::hash_map::Entry::Vacant(slot) => {
                                slot.insert(hits.len());
                                hits.push((entry, score));
                            }
                        }
                    }
                }
            }
            let affinity = command_affinity(&commands, &hits, &matches).unwrap_or_default();
            return Ok(self.intent_search_with_affinity(
                &stripped,
                corpus,
                limit,
                active,
                signals,
                Some((prepared, vector, hits)),
                &affinity,
            ));
        }
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
            let index_filter = index_filter(filter)
                .map(|f| filter_from_json(&f))
                .transpose()?;
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
                Ok(raw
                    .hybrid_search(hybrid)?
                    .into_iter()
                    .filter_map(|hit| Some((command_of(&hit.document)?, f64::from(hit.score))))
                    .collect())
            }
        };
        let active_filter: &SearchFilter = if parsed_any { &merged_filter } else { filter };
        let mut hits: Vec<Hit> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut results = search_with(active_filter)?;
        let best = results
            .iter()
            .map(|r| r.1)
            .fold(f64::NEG_INFINITY, f64::max);
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
            let best = results
                .iter()
                .map(|r| r.1)
                .fold(f64::NEG_INFINITY, f64::max);
            let scale = if parsed_useful { 0.5 } else { 1.0 };
            for (command, score) in results.drain(..) {
                if let Some(entry) = corpus.get(&command).filter(|e| filter.allows(e)) {
                    if seen.insert(command.clone()) {
                        hits.push((
                            entry.clone(),
                            (if best > 0.0 { score / best } else { 1.0 }) * scale,
                        ));
                    }
                }
            }
        }
        let final_filter = addition_filter(
            filter,
            &merged_filter,
            has_markers,
            parsed_any,
            parsed_useful,
        );
        let expansion = expansion_terms(&hits, &stripped, signals, evidence);
        if !expansion.is_empty() {
            let text = ShellTokenizer::encoded(&expansion.join(" "));
            let expanded = raw.search_text_with(
                "terms",
                &text,
                k,
                &Default::default(),
                index_filter(final_filter)
                    .map(|f| filter_from_json(&f))
                    .transpose()?,
            )?;
            let best = expanded
                .iter()
                .map(|h| f64::from(h.score))
                .fold(0.0, f64::max);
            for hit in expanded {
                if let Some(entry) = command_of(&hit.document)
                    .and_then(|c| corpus.get(&c))
                    .filter(|e| final_filter.allows(e))
                {
                    merge_expansion(
                        &mut hits,
                        entry,
                        if best > 0.0 {
                            f64::from(hit.score) / best
                        } else {
                            0.0
                        },
                    );
                }
            }
        }
        let matches = command_index
            .map(|index| {
                index
                    .description_matches(&vector, BRIDGE_MATCHES, BRIDGE_FLOOR)
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let affinity = command_affinity(&commands, &hits, &matches).unwrap_or_default();
        drop(db);
        let existing: HashSet<String> = hits.iter().map(|h| h.0.key()).collect();
        hits.extend(signals.additions(&stripped, corpus, &existing, final_filter, now));
        Ok(crate::corpus::rerank_with_affinity(
            hits, literal, &stripped, now, signals, evidence, &affinity,
        )
        .into_iter()
        .take(limit)
        .collect())
    }

    /// Index contention uses the same cosine semantics, but must embed the in-memory corpus.
    fn intent_search(
        &self,
        query: &str,
        corpus: &HashMap<String, SearchEntry>,
        limit: usize,
        filter: &SearchFilter,
        signals: &SearchSignals,
        indexed: Option<(crate::corpus::PreparedQuery, Vec<f32>, Vec<Hit>)>,
    ) -> Vec<SearchEntry> {
        self.intent_search_with_affinity(
            query,
            corpus,
            limit,
            filter,
            signals,
            indexed,
            &HashMap::new(),
        )
    }

    fn intent_search_with_affinity(
        &self,
        query: &str,
        corpus: &HashMap<String, SearchEntry>,
        limit: usize,
        filter: &SearchFilter,
        signals: &SearchSignals,
        indexed: Option<(crate::corpus::PreparedQuery, Vec<f32>, Vec<Hit>)>,
        affinity: &HashMap<String, f64>,
    ) -> Vec<SearchEntry> {
        let fallback = indexed.is_none();
        let (prepared, vector, mut hits) = indexed.unwrap_or_else(|| {
            let prepared = crate::corpus::prepare_intent_query(
                query,
                &crate::corpus::query_vocabulary(corpus, signals),
                Some(&self.potion.vocabulary),
            );
            let vector = self.potion.embed(&[prepared.embedding_query()]).remove(0);
            (prepared, vector, vec![])
        });
        let mut seen = hits.iter().map(|(e, _)| e.key()).collect::<HashSet<_>>();
        let mut pending = Vec::new();
        // A missing indexed pool means empty history or a held lock, not a BM25 fallback.
        if fallback {
            let mut entries = corpus
                .values()
                .filter(|e| filter.allows(e))
                .cloned()
                .collect::<Vec<_>>();
            entries.sort_by_key(|e| e.key());
            for entry in entries {
                if seen.insert(entry.key()) {
                    pending.push(entry);
                }
            }
        }
        if let Some(context) = &signals.context {
            for kind in [
                smcli_core::ranking::CandidateKind::Hint,
                smcli_core::ranking::CandidateKind::Script,
                smcli_core::ranking::CandidateKind::Test,
            ] {
                for candidate in context.candidates_of(kind) {
                    let entry =
                        corpus
                            .get(&candidate.command)
                            .cloned()
                            .unwrap_or_else(|| SearchEntry {
                                flags: smcli_core::parse::command_flags(&candidate.command),
                                ..SearchEntry::new(&candidate.command, 0, "project")
                            });
                    if filter.allows(&entry) && seen.insert(entry.key()) {
                        pending.push(entry);
                    }
                }
            }
        }
        if !pending.is_empty() {
            let vectors = self
                .potion
                .embed(&pending.iter().map(embedding_text).collect::<Vec<_>>());
            hits.extend(
                pending
                    .into_iter()
                    .zip(vectors)
                    .map(|(entry, v)| (entry, cosine(&vector, &v))),
            );
        }
        crate::corpus::rerank_intent_with_affinity(hits, &prepared, signals, affinity)
            .into_iter()
            .take(limit)
            .collect()
    }

    /// Discards the index and embeds every command again. Returns the number embedded.
    pub fn reindex(&self, corpus: &HashMap<String, SearchEntry>) -> Result<usize> {
        let Some(lock) = self.lock_index(u32::MAX)? else {
            bail!("Another smcli is updating the index.")
        };
        self.reindex_using_lock(corpus, &lock)
    }

    /// `lock` must be acquired before reading the corpus.
    pub fn reindex_using_lock(
        &self,
        corpus: &HashMap<String, SearchEntry>,
        _lock: &File,
    ) -> Result<usize> {
        let name = self.index_name();
        self.remove_files(&name);
        let db = self.open_index(&name)?;
        self.sync(&db, &db.typed::<IndexedCommand>("commands")?, corpus)
    }

    /// Brings `commands` in line with `corpus`. Returns the number of documents now indexed.
    fn sync(
        &self,
        db: &Database,
        commands: &TypedCollection<IndexedCommand>,
        corpus: &HashMap<String, SearchEntry>,
    ) -> Result<usize> {
        // Keyed by the full command, not the truncated embedding text: two pasted scripts that share a
        // 1,500-character prefix must both stay retrievable. Embeddings stay capped; lexical terms use a
        // larger ceiling so tail words are searchable.
        let wanted: HashMap<String, (IndexedCommand, String, String)> = corpus
            .values()
            .map(|entry| {
                let embed = embedding_text(entry);
                let lexical = lexical_text(entry);
                (
                    entry.key(),
                    (
                        IndexedCommand::new(entry, search_document(entry, false)),
                        embed,
                        lexical,
                    ),
                )
            })
            .collect();
        let indexed = fingerprints(commands)?;
        let stale: Vec<&String> = indexed
            .iter()
            .filter(|(command, (doc, _))| {
                wanted
                    .get(*command)
                    .map(|(row, _, _)| &row.doc != doc)
                    .unwrap_or(true)
            })
            .map(|(command, _)| command)
            .collect();
        if !stale.is_empty() {
            commands.delete_many(json!({"command": {"$in": stale}}))?;
            if stale.len() >= 256 && stale.len() * 4 >= indexed.len() {
                // Reclaim file space after deleting a substantial share of indexed commands.
                db.compact()?;
            }
        }
        // Same identity and embedding text: usage changes update metadata and keep the vector.
        for (command, (doc, meta)) in &indexed {
            if let Some((row, _, _)) = wanted
                .get(command)
                .filter(|(row, _, _)| &row.doc == doc && &row.meta != meta)
            {
                commands
                    .update_one(json!({"command": command}), json!({"$set": row.metadata()}))?;
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
            let texts: Vec<String> = batch
                .iter()
                .map(|command| wanted[*command].1.clone())
                .collect();
            let vectors = self.potion.embed(&texts);
            let documents: Vec<IndexedCommand> = batch
                .iter()
                .zip(vectors)
                .map(|(command, embedding)| IndexedCommand {
                    terms: ShellTokenizer::encoded(&wanted[*command].2),
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
        for field in [
            "command",
            "doc",
            "program",
            "subcommand",
            "author",
            "lastRun",
        ] {
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
        remove_old_indexes(&self.directory)?;
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
                open().map_err(|retry| {
                    retry.context(format!("while rebuilding a corrupt index: {err:#}"))
                })
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

    /// Serializes index writers with flock on the shared lock file. Tries `attempts` times,
    /// 50 ms apart; `None` when another process still holds it.
    pub fn lock_index(&self, attempts: u32) -> Result<Option<File>> {
        lock_index_directory(&self.directory, attempts)
    }
}

/// Model-free health facts; CLI presentation and serialization stay with the caller.
#[derive(Debug, Default)]
pub struct IndexReport {
    pub path: String,
    pub built: bool,
    pub indexed: usize,
    pub commands: usize,
    pub missing: usize,
    pub stale: usize,
    pub bytes: u64,
    pub last_updated: Option<String>,
    pub other_indexes: Vec<String>,
    pub problems: Vec<String>,
    pub healthy: bool,
}

/// Inspect coverage without repairing or rebuilding a cache. Unreadable caches are reported, never deleted.
pub fn inspect(
    directory: &Path,
    model: &crate::ModelId,
    corpus: &HashMap<String, SearchEntry>,
) -> IndexReport {
    let mut report = IndexReport {
        commands: corpus.len(),
        ..Default::default()
    };
    let result = (|| -> Result<()> {
        let index_dir = directory;
        let name = index_name(model);
        let command_current = crate::commands::bundled_sources().map(|sources| {
            let refs = crate::commands::source_refs(&sources);
            crate::commands::CommandIndex::reader(index_dir, model.clone(), &refs).index_name()
        });
        let path = index_dir.join(format!("{name}.taladb"));
        report.path = path.to_string_lossy().into_owned();
        let directory = match std::fs::symlink_metadata(&index_dir) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                report.missing = corpus.len();
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        anyhow::ensure!(
            directory.is_dir()
                && directory.uid() == unsafe { libc::getuid() }
                && directory.mode() & 0o077 == 0,
            "Search index must be a private directory owned by you (chmod 700): {}",
            index_dir.display()
        );
        let mut latest = None;
        for entry in std::fs::read_dir(&index_dir)? {
            let entry = entry?;
            let filename = entry.file_name().to_string_lossy().into_owned();
            if (filename.starts_with("rs-commands-v") || filename.starts_with("rs-examples-"))
                && filename.ends_with(".taladb")
                && filename != format!("{name}.taladb")
                && command_current
                    .as_ref()
                    .is_none_or(|current| filename != format!("{current}.taladb"))
            {
                report.other_indexes.push(filename.clone());
            }
            if filename.starts_with(&format!("{name}.")) {
                let info = std::fs::symlink_metadata(entry.path())?;
                anyhow::ensure!(
                    info.is_file() && info.uid() == unsafe { libc::getuid() },
                    "Index files must be regular files owned by you: {}",
                    entry.path().display()
                );
                report.bytes += info.len();
                if let Ok(time) = info.modified() {
                    latest = Some(
                        latest.map_or(time, |previous: std::time::SystemTime| previous.max(time)),
                    );
                }
            }
        }
        report.other_indexes.sort();
        report.last_updated = latest
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|t| smcli_core::time::format_iso8601(t.as_secs_f64(), false));
        if !path.try_exists()? {
            report.missing = corpus.len();
            return Ok(());
        }
        report.built = true;
        let _lock = lock_index_directory(&index_dir, 1)?.ok_or_else(|| {
            anyhow::anyhow!("Another smcli owns the index; check again when it finishes.")
        })?;
        // Never use HistorySearch::open_index here: it deletes unreadable caches instead of reporting them.
        let db = taladb::open(&path)
            .map_err(|e| anyhow::anyhow!("Index is unreadable ({e}); run `smcli reindex`."))?;
        let rows = db
            .collection("commands")?
            .aggregate(vec![taladb::Stage::Project {
                fields: vec!["doc".into()],
                include: true,
                keep_id: false,
            }])?;
        let indexed: HashSet<String> = rows
            .iter()
            .filter_map(|r| match r.get("doc") {
                Some(taladb::Value::Str(doc)) => Some(doc.clone()),
                _ => None,
            })
            .collect();
        let expected: HashSet<String> = corpus
            .values()
            .map(|entry| search_document(entry, false))
            .collect();
        report.indexed = indexed.len();
        report.missing = expected.difference(&indexed).count();
        report.stale = indexed.difference(&expected).count();
        Ok(())
    })();
    if let Err(error) = result {
        report.problems.push(format!("{error:#}"));
    }
    report.healthy = report.problems.is_empty();
    report
}

#[derive(Debug, Serialize)]
pub struct IndexInfo {
    pub name: String,
    pub bytes: u64,
    pub current: bool,
}

/// List recognized caches by name, optionally pruning all but the supplied current names.
/// Include the typed-history and command-example names in `current_names` to retain them too.
pub fn list_indexes(
    directory: &Path,
    current_names: &[String],
    prune: bool,
) -> Result<Vec<IndexInfo>> {
    let _lock = if prune {
        Some(
            lock_index_directory(directory, 20)?
                .ok_or_else(|| anyhow::anyhow!("Search index is busy; try again."))?,
        )
    } else {
        None
    };
    if prune {
        remove_legacy_indexes(directory)?;
    }
    let mut found: std::collections::BTreeMap<String, IndexInfo> = Default::default();
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries.collect::<std::io::Result<Vec<_>>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let filename = entry.file_name().to_string_lossy().into_owned();
        // Index names have no dots; the first dot separates every sidecar from its index name.
        let name = if filename.starts_with("rs-commands-") || filename.starts_with("rs-examples-") {
            filename.split_once('.').map(|(name, _)| name.to_string())
        } else if filename.starts_with("tldr-") && filename.ends_with(".vec") {
            Some(filename.clone())
        } else {
            None
        };
        let Some(name) = name else { continue };
        let current = !filename.starts_with("tldr-") && current_names.contains(&name);
        let info = found.entry(name.clone()).or_insert_with(|| IndexInfo {
            name,
            bytes: 0,
            current,
        });
        info.bytes += entry.metadata()?.len();
        if prune && !info.current {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(found.into_values().collect())
}

/// Remove history caches after pruning rows, leaving command examples and unrelated files alone.
/// The caller holds the directory writer lock across the history prune and this cleanup.
pub fn remove_history_indexes(directory: &Path) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("rs-commands-v")
        {
            if entry.file_type()?.is_dir() {
                std::fs::remove_dir_all(entry.path())?;
            } else {
                std::fs::remove_file(entry.path())?;
            }
        }
    }
    Ok(())
}

/// Remove unversioned command-example caches without touching any history index.
pub fn remove_legacy_indexes(directory: &Path) -> Result<()> {
    remove_indexes_matching(directory, |name| {
        name.starts_with("rs-commands-potion-shell-16m-")
    })
}

pub(crate) fn remove_old_indexes(directory: &Path) -> Result<()> {
    remove_legacy_indexes(directory)?;
    remove_indexes_matching(directory, |name| {
        name.starts_with("rs-commands-v") && !name.starts_with(&format!("{VERSION}-"))
    })
}

fn remove_indexes_matching(directory: &Path, obsolete: impl Fn(&str) -> bool) -> Result<()> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        if obsolete(&entry.file_name().to_string_lossy()) {
            if entry.file_type()?.is_dir() {
                std::fs::remove_dir_all(entry.path())?;
            } else {
                std::fs::remove_file(entry.path())?;
            }
        }
    }
    Ok(())
}

/// Locks the shared index directory without loading an embedding model.
pub fn lock_index_directory(directory: &Path, attempts: u32) -> Result<Option<File>> {
    prepare_directory(directory)?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory.join("lock"))?;
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
pub(crate) fn prepare_directory(directory: &Path) -> Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)?;
    let info = std::fs::symlink_metadata(directory)?;
    // SAFETY: getuid has no preconditions.
    if !info.is_dir() || info.uid() != unsafe { libc::getuid() } || info.mode() & 0o077 != 0 {
        bail!(
            "Search index must be a private directory owned by you (chmod 700): {}",
            directory.display()
        );
    }
    Ok(())
}

/// Each indexed command's document identity and metadata fingerprint. Projected, since decoding every embedding is slow.
fn fingerprints(
    commands: &TypedCollection<IndexedCommand>,
) -> Result<HashMap<String, (String, String)>> {
    let rows = commands.raw().aggregate(vec![Stage::Project {
        fields: vec!["command".into(), "doc".into(), "meta".into()],
        include: true,
        keep_id: false,
    }])?;
    Ok(rows
        .iter()
        .filter_map(
            |row| match (row.get("command"), row.get("doc"), row.get("meta")) {
                (Some(Value::Str(command)), Some(Value::Str(doc)), Some(Value::Str(meta))) => {
                    Some((command.clone(), (doc.clone(), meta.clone())))
                }
                _ => None,
            },
        )
        .collect())
}

fn has_constraint(filter: &SearchFilter) -> bool {
    filter.program.is_some() || filter.subcommand.is_some() || filter.directory.is_some()
}

/// Picks and project additions keep a resolved `$word` marker even when retrieval is empty.
/// An automatic parse is kept only when it actually matched. Unresolved marker text is not a constraint.
fn addition_filter<'a>(
    explicit: &'a SearchFilter,
    merged: &'a SearchFilter,
    has_markers: bool,
    parsed_any: bool,
    retrieved: bool,
) -> &'a SearchFilter {
    if (has_markers && parsed_any) || retrieved {
        merged
    } else {
        explicit
    }
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

#[cfg(test)]
#[test]
fn lexical_search_matches_text_fallback() {
    let corpus: HashMap<_, _> = ["git status", "docker ps", "git log"]
        .into_iter()
        .map(|s| (s.into(), SearchEntry::new(s, 1, "native")))
        .collect();
    let signals = SearchSignals::default();
    for query in ["git", "show git status", "$git status"] {
        let filter = SearchFilter::default();
        let plan = plan_query(query, &corpus, 10, &filter, None, &signals)
            .unwrap()
            .unwrap_or_else(|_| panic!("expected query plan"));
        let expected = text_fallback(
            &plan.stripped,
            &plan.corpus,
            10,
            &filter,
            plan.literal,
            &signals,
            0.0,
            &plan.parsed_filter,
            plan.has_markers,
        );
        let actual = lexical_search(query, &corpus, 10, &filter, &signals, 0.0).unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap(),
            "{query}"
        );
    }
    let filter = SearchFilter {
        program: Some("git".into()),
        ..Default::default()
    };
    let matches = lexical_search("", &corpus, 10, &filter, &signals, 0.0).unwrap();
    assert_eq!(matches.len(), 2);
    assert!(matches.iter().all(|entry| filter.allows(entry)));
    assert!(lexical_search("", &corpus, 10, &SearchFilter::default(), &signals, 0.0).is_err());
    let mut context = smcli_core::ranking::RankingContext::default();
    context.candidates.push(smcli_core::ranking::Candidate::new(
        "make test".into(),
        smcli_core::ranking::CandidateKind::Script,
        "Makefile target",
    ));
    let signals = SearchSignals {
        context: Some(context),
        ..Default::default()
    };
    let corpus = HashMap::new();
    let filter = SearchFilter::default();
    let expected = text_fallback(
        "make test",
        &corpus,
        10,
        &filter,
        true,
        &signals,
        0.0,
        &filter,
        false,
    );
    let actual = lexical_search("make test", &corpus, 10, &filter, &signals, 0.0).unwrap();
    assert_eq!(actual.first().unwrap().command, "make test");
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

/// Search without the index: commands sharing the query's words, by how much of the query they cover.
/// `query` is already stripped of `$word` markers; `parsed_filter` and `has_markers` come from the
/// same parsing done on the indexed path so a held lock does not change ranking behavior.
#[cfg(test)]
fn text_fallback(
    query: &str,
    corpus: &HashMap<String, SearchEntry>,
    limit: usize,
    filter: &SearchFilter,
    literal: bool,
    signals: &SearchSignals,
    now: f64,
    parsed_filter: &SearchFilter,
    has_markers: bool,
) -> Vec<SearchEntry> {
    text_fallback_with_evidence(
        query,
        corpus,
        limit,
        filter,
        literal,
        signals,
        now,
        parsed_filter,
        has_markers,
        None,
    )
}

fn text_fallback_with_evidence(
    query: &str,
    corpus: &HashMap<String, SearchEntry>,
    limit: usize,
    filter: &SearchFilter,
    literal: bool,
    signals: &SearchSignals,
    now: f64,
    parsed_filter: &SearchFilter,
    has_markers: bool,
    evidence: Option<&dyn crate::ProjectEvidence>,
) -> Vec<SearchEntry> {
    let parsed_any = has_constraint(parsed_filter);
    let mut merged_filter = filter.clone();
    if parsed_any {
        merged_filter.merge_parsed(parsed_filter);
    }
    let terms: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(String::from)
        .collect();
    let mut hits: Vec<Hit> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    if parsed_any {
        let mut filtered: Vec<Hit> = corpus
            .values()
            .filter(|e| merged_filter.allows(e))
            .filter_map(|e| {
                Some((
                    e.clone(),
                    Some(entry_coverage(e, &terms)).filter(|c| *c > 0.0)?,
                ))
            })
            .collect();
        filtered.sort_by(|a, b| {
            b.1.total_cmp(&a.1)
                .then_with(|| a.0.command.cmp(&b.0.command))
        });
        filtered.truncate((limit * 4).max(40));
        for (entry, score) in filtered {
            if seen.insert(entry.key()) {
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
            .filter_map(|e| {
                Some((
                    e.clone(),
                    Some(entry_coverage(e, &terms)).filter(|c| *c > 0.0)?,
                ))
            })
            .collect();
        fallback.sort_by(|a, b| {
            b.1.total_cmp(&a.1)
                .then_with(|| a.0.command.cmp(&b.0.command))
        });
        fallback.truncate((limit * 4).max(40));
        let scale = if parsed_useful { 0.5 } else { 1.0 };
        for (entry, score) in fallback {
            if seen.insert(entry.key()) {
                hits.push((entry, score * scale));
            }
        }
    }
    let final_filter = addition_filter(
        filter,
        &merged_filter,
        has_markers,
        parsed_any,
        parsed_useful,
    );
    let expansion = expansion_terms(&hits, query, signals, evidence);
    if !expansion.is_empty() {
        for entry in corpus.values().filter(|e| final_filter.allows(e)) {
            merge_expansion(&mut hits, entry, entry_coverage(entry, &expansion));
        }
    }
    let existing: HashSet<String> = hits.iter().map(|h| h.0.key()).collect();
    hits.extend(signals.additions(query, corpus, &existing, final_filter, now));
    crate::corpus::rerank_with_evidence(hits, literal, query, now, signals, evidence)
        .into_iter()
        .take(limit)
        .collect()
}

fn expansion_terms(
    hits: &[Hit],
    query: &str,
    signals: &SearchSignals,
    evidence: Option<&dyn crate::ProjectEvidence>,
) -> Vec<String> {
    if !uses(signals.context.as_ref(), Signal::Expand) {
        return vec![];
    }
    let terms: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(String::from)
        .collect();
    // Normalized retrieval always gives the best hit 1.0; lexical coverage is an absolute confidence gate.
    if terms.is_empty() || hits.iter().any(|(e, _)| entry_coverage(e, &terms) >= 0.75) {
        return vec![];
    }
    let Some(evidence) = evidence else {
        return vec![];
    };
    let mut seen = HashSet::new();
    evidence
        .grep_terms(&terms, std::time::Duration::from_millis(20))
        .into_iter()
        .filter(|t| {
            (3..=128).contains(&t.len())
                && t.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
        })
        .filter(|t| !terms.contains(t) && seen.insert(t.clone()))
        .take(16)
        .collect()
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let dot = a
        .iter()
        .zip(b)
        .map(|(x, y)| f64::from(*x) * f64::from(*y))
        .sum::<f64>();
    let norm = |v: &[f32]| v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
    let denominator = norm(a) * norm(b);
    if denominator > 0.0 {
        (dot / denominator).clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

fn merge_expansion(hits: &mut Vec<Hit>, entry: &SearchEntry, score: f64) {
    let bonus = 0.15 * score.clamp(0.0, 1.0);
    if bonus == 0.0 {
        return;
    }
    if let Some(hit) = hits.iter_mut().find(|h| h.0.key() == entry.key()) {
        hit.1 = (hit.1 + bonus).min(1.0);
    } else {
        hits.push((entry.clone(), bonus));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_bridge_affinity_uses_program_subcommand_and_flags() -> Result<()> {
        let root = tempfile::tempdir()?;
        let db = taladb::open(root.path().join("fixture.taladb"))?;
        let commands = db.typed::<IndexedCommand>("commands")?;
        let hits = [
            "tar -xzf a.tgz",
            "tar -czf b.tgz",
            "git reset --soft HEAD~1",
            "git status",
            "ls -la",
        ]
        .into_iter()
        .map(|text| {
            let mut entry = SearchEntry::new(text, 1, "native");
            entry.flags = smcli_core::parse::command_flags(text);
            (entry, 0.5)
        })
        .collect::<Vec<_>>();
        commands.insert_many(
            &hits
                .iter()
                .map(|(entry, _)| IndexedCommand::new(entry, String::new()))
                .collect::<Vec<_>>(),
        )?;
        let matches =
            ["tar -xzf", "git reset --soft"].map(|template| crate::commands::DescriptionMatch {
                template: template.into(),
                similarity: 1.0,
            });
        let affinity = command_affinity(&commands, &hits, &matches)?;
        assert!(affinity["tar -xzf a.tgz"] > affinity["tar -czf b.tgz"]);
        assert!(affinity["git reset --soft HEAD~1"] > affinity["git status"]);
        assert_eq!(affinity["ls -la"], 0.0);
        let prepared =
            crate::corpus::prepare_intent_query("unpack an archive", &HashSet::new(), None);
        let ranked = crate::corpus::rerank_intent_with_affinity(
            hits[..2].to_vec(),
            &prepared,
            &SearchSignals::default(),
            &affinity,
        );
        assert_eq!(ranked[0].command, "tar -xzf a.tgz");
        let literal = crate::corpus::rerank_with_affinity(
            hits[..2].to_vec(),
            true,
            "tar -czf b.tgz",
            0.0,
            &SearchSignals::default(),
            None,
            &affinity,
        );
        assert_eq!(literal[0].command, "tar -czf b.tgz");
        Ok(())
    }

    fn report_model() -> crate::ModelId {
        crate::ModelId {
            identity: "report-fixture".into(),
            dimension: 2,
        }
    }

    #[test]
    fn health_checks_coverage_without_rebuilding() -> Result<()> {
        let root = tempfile::tempdir()?;
        let directory = root.path().join("index");
        let model = report_model();
        let corpus = HashMap::from([(
            "git status".into(),
            SearchEntry::new("git status", 1, "native"),
        )]);
        let missing = inspect(&directory, &model, &corpus);
        assert!(missing.healthy && !missing.built);
        assert_eq!(missing.commands, 1);
        assert_eq!(missing.missing, 1);
        assert!(
            !directory.exists(),
            "inspection must not create a cache directory"
        );
        drop(lock_index_directory(&directory, 1)?);
        let name = index_name(&model);
        let path = directory.join(format!("{name}.taladb"));
        let db = taladb::open(&path)?;
        let entry = corpus.values().next().unwrap();
        db.typed::<IndexedCommand>("commands")?
            .insert_many(&[IndexedCommand::new(entry, search_document(entry, false))])?;
        drop(db);
        let sidecar = directory.join(format!("{name}.sidecar"));
        std::fs::write(&sidecar, b"sidecar")?;
        let other_history = "rs-commands-v4-potion-othermodel-d2.taladb";
        let other_commands = "rs-examples-v1-potion-shell-16m-othermodel-stub@1-d2.taladb";
        for filename in [other_history, other_commands] {
            std::fs::write(directory.join(filename), b"other")?;
        }
        let sources = crate::commands::bundled_sources().unwrap();
        let refs = crate::commands::source_refs(&sources);
        let commands =
            crate::commands::CommandIndex::reader(&directory, model.clone(), &refs).path();
        std::fs::write(&commands, b"current examples")?;
        let bytes_before_inspection = path.metadata()?.len() + sidecar.metadata()?.len();
        let built = inspect(&directory, &model, &corpus);
        assert!(built.healthy && built.built, "{:?}", built.problems);
        assert_eq!(built.path, path.to_string_lossy());
        assert_eq!((built.indexed, built.missing, built.stale), (1, 0, 0));
        assert_eq!(built.bytes, bytes_before_inspection);
        assert!(built.last_updated.is_some());
        assert_eq!(built.other_indexes, [other_history, other_commands]);
        assert_eq!(inspect(&directory, &model, &HashMap::new()).stale, 1);
        let enlarged = HashMap::from([
            ("git status".into(), entry.clone()),
            ("git log".into(), SearchEntry::new("git log", 1, "native")),
        ]);
        assert_eq!(inspect(&directory, &model, &enlarged).missing, 1);
        let lock = lock_index_directory(&directory, 1)?.unwrap();
        let busy = inspect(&directory, &model, &corpus);
        assert!(!busy.healthy);
        assert_eq!(
            busy.problems,
            ["Another smcli owns the index; check again when it finishes."]
        );
        drop(lock);
        std::fs::write(&path, b"broken cache")?;
        let broken = inspect(&directory, &model, &corpus);
        assert!(!broken.healthy && broken.built);
        assert!(broken.problems[0].contains("Index is unreadable"));
        assert_eq!(
            std::fs::read(&path)?,
            b"broken cache",
            "inspection must not repair a corrupt cache"
        );
        Ok(())
    }

    #[test]
    fn inspect_checks_private_directory_and_regular_owned_files() -> Result<()> {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir()?;
        let directory = root.path().join("index");
        let model = report_model();
        drop(lock_index_directory(&directory, 1)?);
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755))?;
        let public = inspect(&directory, &model, &HashMap::new());
        assert!(!public.healthy && public.problems[0].contains("private directory owned by you"));
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        let outside = root.path().join("outside");
        std::fs::write(&outside, b"keep")?;
        let path = directory.join(format!("{}.taladb", index_name(&model)));
        symlink(&outside, &path)?;
        let linked = inspect(&directory, &model, &HashMap::new());
        assert!(!linked.healthy && linked.problems[0].contains("regular files owned by you"));
        assert_eq!(std::fs::read(&outside)?, b"keep");
        let link_dir = root.path().join("linked-index");
        symlink(&directory, &link_dir)?;
        assert!(!inspect(&link_dir, &model, &HashMap::new()).healthy);
        Ok(())
    }

    #[test]
    fn sync_compacts_after_removing_most_commands() -> Result<()> {
        let root = tempfile::tempdir()?;
        let directory = root.path().join("index");
        prepare_directory(&directory)?;
        let engine = HistorySearch::new(&directory, Potion::bundled()?);
        let corpus: HashMap<_, _> = (0..300)
            .map(|i| {
                let entry = SearchEntry::new(&format!("synthetic command {i}"), 1, "native");
                (entry.key(), entry)
            })
            .collect();
        let name = engine.index_name();
        let db = engine.open_index(&name)?;
        let commands = db.typed::<IndexedCommand>("commands")?;
        engine.sync(&db, &commands, &corpus)?;
        drop(commands);
        drop(db);
        let path = directory.join(format!("{name}.taladb"));
        let before = path.metadata()?.len();
        let remaining = corpus
            .iter()
            .take(10)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let db = engine.open_index(&name)?;
        let commands = db.typed::<IndexedCommand>("commands")?;
        engine.sync(&db, &commands, &remaining)?;
        drop(commands);
        drop(db);
        assert!(path.metadata()?.len() < before);
        Ok(())
    }

    #[test]
    fn indexes_prune_respects_lock_and_keeps_current_typed() -> Result<()> {
        let root = tempfile::tempdir()?;
        let directory = root.path().join("index");
        let model = report_model();
        let name = index_name(&model);
        let typed = format!("{name}-typed");
        let examples = crate::commands::CommandIndex::reader(&directory, model, &[]).index_name();
        let current = vec![name.clone(), typed.clone(), examples.clone()];
        assert!(list_indexes(&directory, &current, false)?.is_empty());
        assert!(!directory.exists());
        let lock = lock_index_directory(&directory, 1)?.unwrap();
        let normal = directory.join(format!("{name}.taladb"));
        let sidecar = directory.join(format!("{name}.sidecar"));
        let typed_path = directory.join(format!("{typed}.taladb"));
        let command_path = directory.join(format!("{examples}.taladb"));
        let old = directory.join("rs-commands-v3-old.taladb");
        let old_tldr = directory.join("tldr-old.vec");
        let other_commands = directory.join("rs-examples-v1-potion-othermodel-stub@1-d2.taladb");
        let legacy_commands = directory.join("rs-commands-potion-shell-16m-bundled-old.taladb");
        let unrelated = directory.join("keep.txt");
        for path in [
            &normal,
            &sidecar,
            &typed_path,
            &command_path,
            &old,
            &old_tldr,
            &other_commands,
            &legacy_commands,
            &unrelated,
        ] {
            std::fs::write(path, b"index")?;
        }
        let listed = list_indexes(&directory, &current, false)?;
        assert_eq!(
            listed.iter().find(|info| info.name == name).unwrap().bytes,
            10
        );
        assert_eq!(listed.iter().filter(|info| info.current).count(), 3);
        assert!(listed.windows(2).all(|pair| pair[0].name < pair[1].name));
        assert!(list_indexes(&directory, &current, true).is_err());
        assert!(old.exists() && legacy_commands.exists());
        drop(lock);
        let pruned = list_indexes(&directory, &current, true)?;
        assert!(
            pruned
                .iter()
                .any(|info| info.name == "rs-commands-v3-old" && !info.current)
        );
        assert!(
            !pruned
                .iter()
                .any(|info| info.name.starts_with("rs-commands-potion-shell-16m-"))
        );
        for path in [&normal, &sidecar, &typed_path, &command_path, &unrelated] {
            assert!(path.exists());
        }
        for path in [&old, &old_tldr, &other_commands, &legacy_commands] {
            assert!(!path.exists());
        }
        Ok(())
    }

    #[test]
    fn prune_removes_index_directories_without_loading_model() -> Result<()> {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir()?;
        let directory = root.path().join("index");
        let lock = lock_index_directory(&directory, 1)?.unwrap();
        let old = directory.join("rs-commands-v1-old");
        std::fs::create_dir(&old)?;
        std::fs::write(old.join("secret"), b"secret-cmd")?;
        let current = directory.join("rs-commands-v4-current.taladb");
        let commands = directory.join("rs-examples-v1-potion-shell-16m-test-stub@1-d2.taladb");
        let legacy_commands = directory.join("rs-commands-potion-shell-16m-bundled-old.taladb");
        let unrelated = directory.join("keep.txt");
        for path in [&current, &commands, &legacy_commands, &unrelated] {
            std::fs::write(path, b"keep")?;
        }
        let outside = root.path().join("outside");
        std::fs::create_dir(&outside)?;
        std::fs::write(outside.join("keep"), b"keep")?;
        let link = directory.join("rs-commands-v1-link");
        symlink(&outside, &link)?;
        remove_history_indexes(&directory)?;
        assert!(!old.exists() && !current.exists() && !link.exists());
        assert!(outside.join("keep").exists());
        for path in [&commands, &legacy_commands, &unrelated] {
            assert!(path.exists());
        }
        assert!(remove_history_indexes(&root.path().join("missing")).is_err());
        drop(lock);
        Ok(())
    }

    #[test]
    #[ignore]
    fn blocked_search_timing() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = HistorySearch::new(root.path().join("index"), Potion::bundled()?);
        let corpus = (0..100_000)
            .map(|i| {
                let entry = SearchEntry::new(&format!("cargo test synthetic_{i}"), 1, "native");
                (entry.key(), entry)
            })
            .collect();
        let start = std::time::Instant::now();
        let hits = engine.search_with_evidence_using_lock(
            "run the tests",
            &corpus,
            10,
            &SearchFilter::default(),
            None,
            &SearchSignals::default(),
            0.0,
            None,
            None,
        )?;
        let elapsed = start.elapsed();
        eprintln!("blocked_search_timing: 100000 synthetic commands, no index lock: {elapsed:?}");
        assert!(!hits.is_empty());
        Ok(())
    }

    #[test]
    fn vector_intent_uses_no_grep_expansion_and_keeps_absolute_cosine() {
        struct NoExpansion;
        impl crate::ProjectEvidence for NoExpansion {
            fn exists(&self, _: &[PathBuf]) -> Vec<bool> {
                panic!("intent must not consult indexed file evidence");
            }
            fn grep_terms(&self, _: &[String], _: std::time::Duration) -> Vec<String> {
                panic!("intent must not expand with BM25");
            }
        }
        let root = tempfile::tempdir().unwrap();
        let engine = HistorySearch::new(root.path().join("index"), Potion::bundled().unwrap());
        let corpus = ["make test", "bun run"]
            .into_iter()
            .map(|c| (c.into(), SearchEntry::new(c, 0, "native")))
            .collect();
        let indexed = engine
            .search_with_evidence(
                "run the tests",
                &corpus,
                2,
                &SearchFilter::default(),
                None,
                &SearchSignals::default(),
                0.0,
                Some(&NoExpansion),
            )
            .unwrap();
        assert_eq!(indexed[0].command, "make test");
        let locked = engine
            .search_with_evidence_using_lock(
                "run the tests",
                &corpus,
                2,
                &SearchFilter::default(),
                None,
                &SearchSignals::default(),
                0.0,
                Some(&NoExpansion),
                None,
            )
            .unwrap();
        assert_eq!(
            indexed.iter().map(|e| &e.command).collect::<Vec<_>>(),
            locked.iter().map(|e| &e.command).collect::<Vec<_>>()
        );
        assert_eq!(cosine(&[2.0, 0.0], &[3.0, 0.0]), 1.0);
        assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
    }

    #[test]
    fn typo_recovery_works_with_existing_index_lock_and_empty_history() {
        use smcli_core::ranking::{Candidate, CandidateKind, RankingContext};
        let root = tempfile::tempdir().unwrap();
        let engine = HistorySearch::new(root.path().join("index"), Potion::bundled().unwrap());
        let entry = SearchEntry::new("make install", 1, "native");
        let corpus = HashMap::from([(entry.key(), entry)]);
        let signals = SearchSignals::default();
        engine
            .search(
                "install",
                &corpus,
                10,
                &SearchFilter::default(),
                None,
                &signals,
                0.0,
            )
            .unwrap();
        assert_eq!(
            engine
                .search(
                    "insall",
                    &corpus,
                    10,
                    &SearchFilter::default(),
                    None,
                    &signals,
                    0.0
                )
                .unwrap()[0]
                .command,
            "make install"
        );
        let lock = engine.lock_index(20).unwrap().unwrap();
        let hits = engine
            .search_with_evidence_using_lock(
                "insall",
                &corpus,
                10,
                &SearchFilter::default(),
                None,
                &signals,
                0.0,
                None,
                None,
            )
            .unwrap();
        assert_eq!(hits[0].command, "make install");
        drop(lock);
        let strict = SearchFilter {
            program: Some("git".into()),
            ..Default::default()
        };
        assert!(
            engine
                .search("insall", &corpus, 10, &strict, None, &signals, 0.0)
                .unwrap()
                .is_empty()
        );
        let mut context = RankingContext::default();
        context.candidates.push(Candidate {
            command: "make install".into(),
            kind: CandidateKind::Script,
            reason: "Makefile target".into(),
            detail: None,
        });
        let signals = SearchSignals {
            context: Some(context),
            ..Default::default()
        };
        assert_eq!(
            engine
                .search(
                    "insall",
                    &HashMap::new(),
                    10,
                    &SearchFilter::default(),
                    None,
                    &signals,
                    0.0
                )
                .unwrap()[0]
                .command,
            "make install"
        );
    }

    #[test]
    fn expansion_finds_code_concepts_but_skips_strong_history_hits() {
        VECTOR_FIRST.with(|v| v.set(false));
        struct Evidence(std::cell::Cell<usize>);
        impl crate::ProjectEvidence for Evidence {
            fn exists(&self, _: &[std::path::PathBuf]) -> Vec<bool> {
                vec![]
            }
            fn grep_terms(&self, terms: &[String], budget: std::time::Duration) -> Vec<String> {
                assert!(terms.iter().any(|t| t == "migration"));
                assert_eq!(budget, std::time::Duration::from_millis(20));
                self.0.set(self.0.get() + 1);
                vec!["migrate".into()]
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let engine = HistorySearch::new(dir.path().join("index"), Potion::bundled().unwrap());
        let corpus = ["cargo test --test migrate", "echo unrelated"]
            .into_iter()
            .map(|c| (c.into(), SearchEntry::new(c, 1, "native")))
            .collect();
        let evidence = Evidence(std::cell::Cell::new(0));
        let filter = SearchFilter::default();
        let signals = SearchSignals::default();
        for locked in [false, true] {
            let _lock = locked.then(|| engine.lock_index(1).unwrap().unwrap());
            if locked {
                assert!(
                    engine
                        .search("migration tests", &corpus, 10, &filter, None, &signals, 0.0)
                        .unwrap()
                        .is_empty()
                );
            }
            let hits = engine
                .search_with_evidence(
                    "migration tests",
                    &corpus,
                    10,
                    &filter,
                    None,
                    &signals,
                    0.0,
                    Some(&evidence),
                )
                .unwrap();
            assert!(
                hits.iter()
                    .any(|e| e.command == "cargo test --test migrate")
            );
            let calls = evidence.0.get();
            let strong = engine
                .search_with_evidence(
                    "cargo test --test migrate",
                    &corpus,
                    10,
                    &filter,
                    None,
                    &signals,
                    0.0,
                    Some(&evidence),
                )
                .unwrap();
            assert_eq!(evidence.0.get(), calls);
            assert_eq!(
                strong,
                engine
                    .search(
                        "cargo test --test migrate",
                        &corpus,
                        10,
                        &filter,
                        None,
                        &signals,
                        0.0
                    )
                    .unwrap()
            );
            let disabled = SearchSignals {
                context: Some(smcli_core::ranking::RankingContext {
                    off: [Signal::Expand].into(),
                    ..Default::default()
                }),
                ..Default::default()
            };
            assert_eq!(
                engine
                    .search_with_evidence(
                        "migration tests",
                        &corpus,
                        10,
                        &filter,
                        None,
                        &disabled,
                        0.0,
                        Some(&evidence)
                    )
                    .unwrap(),
                engine
                    .search(
                        "migration tests",
                        &corpus,
                        10,
                        &filter,
                        None,
                        &disabled,
                        0.0
                    )
                    .unwrap()
            );
            assert_eq!(evidence.0.get(), calls);
        }
        assert_eq!(evidence.0.get(), 2);
    }

    #[test]
    fn text_fallback_prefers_exact_tokens_and_identifier_parts() {
        for (query, target, near) in [
            (
                "main-branch",
                "git checkout main-branch",
                "git checkout main",
            ),
            ("HEAD~1", "git reset HEAD~1", "git reset HEAD~10"),
            (
                "git reset --soft",
                "git reset --soft HEAD",
                "git reset --hard HEAD",
            ),
            ("bar tests", "xcodebuild -scheme FooBarTests", "run Bar"),
            ("FooBar", "run FooBar", "run Foo"),
        ] {
            let corpus = [
                (
                    target.into(),
                    SearchEntry {
                        frecency: 1.0,
                        successes: 1,
                        ..SearchEntry::new(target, 1, "captured")
                    },
                ),
                (
                    near.into(),
                    SearchEntry {
                        frecency: 25.0,
                        successes: 25,
                        ..SearchEntry::new(near, 25, "captured")
                    },
                ),
            ]
            .into_iter()
            .collect();
            let literal = is_literal_query(query, &program_arguments(&corpus));
            let hits = text_fallback(
                query,
                &corpus,
                3,
                &SearchFilter::default(),
                literal,
                &SearchSignals::default(),
                0.0,
                &SearchFilter::default(),
                false,
            );
            assert_eq!(hits[0].command, target, "{query}: {hits:?}");
        }
    }

    #[test]
    fn map_search_filters_to_index_filters() {
        assert_eq!(index_filter(&SearchFilter::default()), None);
        assert_eq!(
            index_filter(&SearchFilter {
                program: Some("git".into()),
                ..Default::default()
            }),
            Some(json!({"program": "git"}))
        );
        let filter = SearchFilter {
            min_runs: Some(2),
            directory: Some("/repo/".into()),
            flag: Some("-m".into()),
            ..Default::default()
        };
        assert_eq!(
            index_filter(&filter),
            Some(
                json!({"$and": [{"flags": {"$in": ["-m"]}}, {"dirs": {"$in": ["/repo"]}}, {"runs": {"$gte": 2}}]})
            )
        );
        let range = SearchFilter {
            since: Some(10.0),
            before: Some(20.0),
            ..Default::default()
        };
        assert_eq!(
            index_filter(&range),
            Some(json!({"lastRun": {"$gte": 10.0, "$lt": 20.0}}))
        );
        let entry = SearchEntry {
            last_directory: Some("/repo/app".into()),
            ..SearchEntry::new("git status", 1, "captured")
        };
        assert_eq!(
            IndexedCommand::new(&entry, "git status".into()).dirs,
            ["/repo/app", "/repo", "/"]
        );
        let entry = SearchEntry {
            last_directory: Some("/repo/".into()),
            ..entry
        };
        assert_eq!(
            IndexedCommand::new(&entry, "git status".into()).dirs,
            ["/repo", "/"]
        );
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
        let hits = engine
            .search("docker", &corpus, 3, &filter, None, &none, 0.0)
            .unwrap();
        assert_eq!(hits[0].command, "docker ps");
        // Removing a command from history removes it from the index.
        let smaller: HashMap<String, SearchEntry> = corpus
            .iter()
            .filter(|(k, _)| *k != "docker ps")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        assert!(
            !engine
                .search("docker", &smaller, 3, &filter, None, &none, 0.0)
                .unwrap()
                .iter()
                .any(|e| e.command == "docker ps")
        );
        // Another process holding the lock gets word matching instead of waiting.
        let held = engine.lock_index(1).unwrap().unwrap();
        let fallback = engine
            .search("git", &corpus, 3, &filter, None, &none, 0.0)
            .unwrap();
        assert_eq!(
            fallback
                .iter()
                .map(|e| e.command.as_str())
                .collect::<Vec<_>>(),
            ["git status"]
        );
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
        for field in [
            "command",
            "doc",
            "program",
            "subcommand",
            "author",
            "lastRun",
        ] {
            assert!(
                info.btree.contains(&field.to_string()),
                "missing btree index on {field}"
            );
        }
        assert!(
            info.fts.contains(&"terms".to_string()),
            "missing fts index on terms"
        );
        assert!(
            info.vector.contains(&"embedding".to_string()),
            "missing vector index on embedding"
        );
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
        engine
            .search(
                "docker",
                &corpus,
                1,
                &SearchFilter::default(),
                None,
                &none,
                0.0,
            )
            .unwrap();

        let _lock = engine.lock_index(1).unwrap().unwrap();
        let name = engine.index_name();
        let first = engine.open_index(&name).unwrap();
        let commands = first.typed::<IndexedCommand>("commands").unwrap();
        let info = commands.raw().list_indexes().unwrap();
        let count = commands.count(json!({})).unwrap();
        // Simulate an index built before `command` was indexed; reopening must add it back.
        commands.raw().drop_index("command").unwrap();
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
        let corpus: HashMap<String, SearchEntry> = [(
            "docker ps".to_string(),
            SearchEntry::new("docker ps", 1, "native"),
        )]
        .into_iter()
        .collect();
        engine
            .search(
                "docker",
                &corpus,
                1,
                &SearchFilter::default(),
                None,
                &SearchSignals::default(),
                0.0,
            )
            .unwrap();

        let name = engine.index_name();
        let db = engine.open_index(&name).unwrap();
        let commands = db.typed::<IndexedCommand>("commands").unwrap();
        let indexed = commands
            .find_one(json!({"command": "docker ps"}))
            .unwrap()
            .unwrap();
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
        let hits = engine
            .search(
                "ps docker",
                &corpus,
                3,
                &SearchFilter::default(),
                None,
                &SearchSignals::default(),
                0.0,
            )
            .unwrap();
        assert!(
            hits.iter().any(|e| e.command == "docker ps"),
            "hybrid query missed docker ps: {:?}",
            hits
        );
    }

    #[test]
    fn trailing_slash_directory_matches_index_and_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let engine = HistorySearch::new(dir.path().join("index"), Potion::bundled().unwrap());
        let entry = SearchEntry {
            last_directory: Some("/repo/".into()),
            ..SearchEntry::new("git status", 1, "captured")
        };
        let corpus = HashMap::from([(entry.command.clone(), entry)]);
        let filter = SearchFilter {
            directory: Some("/repo".into()),
            ..Default::default()
        };
        let signals = SearchSignals::default();
        let indexed = engine
            .search("status", &corpus, 3, &filter, None, &signals, 0.0)
            .unwrap();
        assert_eq!(
            indexed
                .iter()
                .map(|e| e.command.as_str())
                .collect::<Vec<_>>(),
            ["git status"]
        );
        let lock = engine.lock_index(1).unwrap().unwrap();
        let fallback = engine
            .search("status", &corpus, 3, &filter, None, &signals, 0.0)
            .unwrap();
        assert_eq!(
            fallback
                .iter()
                .map(|e| e.command.as_str())
                .collect::<Vec<_>>(),
            ["git status"]
        );
        drop(lock);
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
        let corpus: HashMap<String, SearchEntry> =
            [(git.command.clone(), git), (docker.command.clone(), docker)]
                .into_iter()
                .collect();
        let none = SearchSignals::default();

        let only_git = SearchFilter {
            program: Some("git".into()),
            ..Default::default()
        };
        let hits = engine
            .search("status", &corpus, 3, &only_git, None, &none, 0.0)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].command, "git status");

        let in_repo = SearchFilter {
            directory: Some("/repo".into()),
            ..Default::default()
        };
        let hits = engine
            .search("status", &corpus, 3, &in_repo, None, &none, 0.0)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].command, "git status");

        let with_flag = SearchFilter {
            flag: Some("-v".into()),
            ..Default::default()
        };
        let hits = engine
            .search("status", &corpus, 3, &with_flag, None, &none, 0.0)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].command, "git status");

        let by_author = SearchFilter {
            author: Some("bob".into()),
            ..Default::default()
        };
        let hits = engine
            .search("ps", &corpus, 3, &by_author, None, &none, 0.0)
            .unwrap();
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
        let unresolved = engine
            .search(
                "echo $HOME",
                &corpus,
                5,
                &SearchFilter::default(),
                None,
                &SearchSignals::default(),
                0.0,
            )
            .unwrap();
        assert!(
            unresolved.iter().any(|e| e.command == "echo hello"),
            "{unresolved:?}"
        );
        let strict = engine
            .search(
                "$git ps",
                &corpus,
                5,
                &SearchFilter::default(),
                None,
                &SearchSignals::default(),
                0.0,
            )
            .unwrap();
        assert!(
            strict.iter().any(|e| e.command == "git status"),
            "{strict:?}"
        );
        assert!(
            !strict.iter().any(|e| e.command == "docker ps"),
            "{strict:?}"
        );
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
        let corpus: HashMap<String, SearchEntry> =
            [(git.command.clone(), git), (docker.command.clone(), docker)]
                .into_iter()
                .collect();
        let mut picks = smcli_core::ranking::SelectionLog::default();
        picks.add("git other", "docker ps", 0.0);
        let signals = SearchSignals {
            picks,
            ..Default::default()
        };
        let query = "$git $other";
        let indexed = engine
            .search(
                query,
                &corpus,
                5,
                &SearchFilter::default(),
                None,
                &signals,
                0.0,
            )
            .unwrap();
        assert!(indexed.is_empty(), "indexed: {indexed:?}");
        let held = engine.lock_index(1).unwrap().unwrap();
        let locked = engine
            .search(
                query,
                &corpus,
                5,
                &SearchFilter::default(),
                None,
                &signals,
                0.0,
            )
            .unwrap();
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
        let corpus = HashMap::from([
            (alpha.clone(), first.clone()),
            (beta.clone(), second.clone()),
        ]);
        engine
            .search(
                "x",
                &corpus,
                5,
                &SearchFilter::default(),
                None,
                &SearchSignals::default(),
                0.0,
            )
            .unwrap();
        let rows = indexed_commands(&engine);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(
            rows.iter().find(|r| r.command == alpha).unwrap().embedding,
            rows.iter().find(|r| r.command == beta).unwrap().embedding
        );

        first.runs = 9;
        let corpus = HashMap::from([(alpha.clone(), first), (beta.clone(), second)]);
        engine
            .search(
                "x",
                &corpus,
                5,
                &SearchFilter::default(),
                None,
                &SearchSignals::default(),
                0.0,
            )
            .unwrap();
        let rows = indexed_commands(&engine);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows.iter().find(|r| r.command == alpha).unwrap().runs, 9);
        assert_eq!(rows.iter().find(|r| r.command == beta).unwrap().runs, 2);
    }

    #[test]
    fn setup_failure_keeps_the_cache_and_corrupt_cache_is_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        let engine = HistorySearch::new(dir.path().join("index"), Potion::bundled().unwrap());
        let corpus: HashMap<String, SearchEntry> = [(
            "docker ps".into(),
            SearchEntry::new("docker ps", 1, "native"),
        )]
        .into_iter()
        .collect();
        engine
            .search(
                "docker",
                &corpus,
                1,
                &SearchFilter::default(),
                None,
                &SearchSignals::default(),
                0.0,
            )
            .unwrap();
        let name = engine.index_name();
        let err = match engine.open_cached(&name, |_| bail!("transient setup I/O failure")) {
            Err(err) => err,
            Ok(_) => panic!("setup failure was ignored"),
        };
        assert!(
            err.to_string().contains("transient setup I/O failure"),
            "{err:#}"
        );
        assert_eq!(indexed_commands(&engine).len(), 1);

        let path = engine.directory.join(format!("{name}.taladb"));
        std::fs::write(&path, b"this is not a taladb file").unwrap();
        let hits = engine
            .search(
                "docker",
                &corpus,
                1,
                &SearchFilter::default(),
                None,
                &SearchSignals::default(),
                0.0,
            )
            .unwrap();
        assert_eq!(hits[0].command, "docker ps");
    }

    #[test]
    fn long_tail_terms_are_searchable() {
        let dir = tempfile::tempdir().unwrap();
        let engine = HistorySearch::new(dir.path().join("index"), Potion::bundled().unwrap());
        let prefix = "x".repeat(1_500);
        let alpha = format!("{prefix} ALPHA");
        let beta = format!("{prefix} BETA");
        let corpus: HashMap<String, SearchEntry> = [
            (alpha.clone(), SearchEntry::new(&alpha, 1, "captured")),
            (beta.clone(), SearchEntry::new(&beta, 1, "captured")),
        ]
        .into_iter()
        .collect();
        let hits = engine
            .search(
                "ALPHA",
                &corpus,
                3,
                &SearchFilter::default(),
                None,
                &SearchSignals::default(),
                0.0,
            )
            .unwrap();
        assert_eq!(
            hits[0].command, alpha,
            "ALPHA tail did not retrieve the right command: {hits:?}"
        );
        let hits = engine
            .search(
                "BETA",
                &corpus,
                3,
                &SearchFilter::default(),
                None,
                &SearchSignals::default(),
                0.0,
            )
            .unwrap();
        assert_eq!(
            hits[0].command, beta,
            "BETA tail did not retrieve the right command: {hits:?}"
        );
    }

    #[test]
    #[ignore = "manual timing measurement; run explicitly for comparable warm/cold measurements"]
    fn measure_long_entry_sync_overhead() {
        let dir = tempfile::tempdir().unwrap();
        let engine = HistorySearch::new(dir.path().join("index"), Potion::bundled().unwrap());
        let mut corpus: HashMap<String, SearchEntry> = HashMap::new();
        for index in 0..5 {
            // Each command is 8,000 characters of unique text so the lexical terms grow with the ceiling.
            let body: String = (0usize..8_000)
                .map(|i| (((i + index * 7) % 26) as u8 + b'a') as char)
                .collect();
            let command = format!("cmd {body}");
            corpus.insert(command.clone(), SearchEntry::new(&command, 1, "captured"));
        }
        let started = std::time::Instant::now();
        engine
            .search(
                "x",
                &corpus,
                1,
                &SearchFilter::default(),
                None,
                &SearchSignals::default(),
                0.0,
            )
            .unwrap();
        let sync_ms = started.elapsed().as_millis();
        let size: u64 = engine
            .files(&engine.index_name())
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
            .sum();
        println!("long-entry sync: {sync_ms} ms, index size: {size} bytes");
    }

    fn indexed_commands(engine: &HistorySearch) -> Vec<IndexedCommand> {
        let db = engine.open_index(&engine.index_name()).unwrap();
        db.typed::<IndexedCommand>("commands")
            .unwrap()
            .find(json!({}))
            .unwrap()
    }
}
