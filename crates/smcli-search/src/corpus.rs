use std::collections::{HashMap, HashSet};

use smcli_core::entry::{SearchEntry, SearchFilter, command_key, command_parts};
use smcli_core::history::{CommandStats, INTERRUPTED_EXIT_CODES, Run, USAGE_HALF_LIFE, answers_with_exit_code};
use smcli_core::parse::{CommandBreakdown, command_flags, is_smcli_command, program_of};
use smcli_core::ranking::{CandidateKind, RankingContext, SelectionLog, Signal, uses};
use smcli_core::time::{format_iso8601, now};

/// Native history lines after which a run counts half; about two weeks for a busy shell, like captured usage.
pub const NATIVE_HALF_LIFE: f64 = 1_000.0;

/// Characters of command+intent that are indexed lexically. SQLite `TEXT` has no practical limit, so this
/// ceiling keeps the keyword index bounded while still covering pasted scripts and heredocs.
pub const LEXICAL_LIMIT: usize = 8_192;

/// Merges captured stats and native history lines into one entry per distinct command.
/// Native history keeps no record boundaries we can trust, so each line is one entry.
pub fn search_corpus(captured: HashMap<String, CommandStats>, native: &[String]) -> HashMap<String, SearchEntry> {
    CorpusState::new(captured, native, "", None, now()).entries
}

/// The search corpus, which `add` updates for one new run instead of rebuilding from every run. Usage is as of
/// `as_of` (frecency drifts with time, so a holder rebuilds now and then), and `directory` and `project` fix what
/// `runs_here` and `runs_project` count. After the same runs, `entries` equals a rebuild from `command_stats`.
pub struct CorpusState {
    pub directory: String,
    pub project: Option<String>,
    pub as_of: f64,
    pub entries: HashMap<String, SearchEntry>,
    /// Each program in `entries` with the words that follow it, for `is_literal_query`.
    pub programs: HashMap<String, HashSet<String>>,
    captured: HashMap<String, CommandStats>,
    native_counts: HashMap<String, i64>,
    native_usage: HashMap<String, f64>,
}

impl CorpusState {
    pub fn new(captured: HashMap<String, CommandStats>, native: &[String], directory: &str, project: Option<&str>, as_of: f64) -> Self {
        let mut state = Self {
            directory: directory.into(),
            project: project.map(String::from),
            as_of,
            entries: HashMap::new(),
            programs: HashMap::new(),
            captured,
            native_counts: HashMap::new(),
            native_usage: HashMap::new(),
        };
        // Native lines have no times, but files are oldest first, so position stands in for age: a run's weight
        // halves every `NATIVE_HALF_LIFE` lines from the end. Captured commands keep whichever evidence is
        // stronger, so turning capture on does not throw away years of native habit.
        let lines: Vec<&str> = native.iter().map(|l| l.trim()).filter(|l| !l.is_empty() && !is_smcli_command(l)).collect();
        for (index, command) in lines.iter().enumerate() {
            *state.native_counts.entry(command_key(command, None)).or_insert(0) += 1;
            *state.native_usage.entry(command_key(command, None)).or_insert(0.0) +=
                0.5f64.powf((lines.len() - 1 - index) as f64 / NATIVE_HALF_LIFE);
        }
        let commands: HashSet<String> = state.captured.keys().chain(state.native_counts.keys()).cloned().collect();
        for command in commands {
            if let Some(entry) = state.make_entry(&command) {
                state.entries.insert(command, entry);
            }
        }
        state.programs = program_arguments(&state.entries);
        state
    }

    /// Adds one run the way `command_stats` would count it (see its columns), as the service does per record.
    pub fn add(&mut self, run: &Run) {
        let key = command_key(&run.command, run.expanded_command.as_deref());
        let primary = run.expanded_command.as_deref().unwrap_or(&run.command);
        if is_smcli_command(primary) {
            return;
        }
        let mut stats = self.captured.get(&key).cloned().unwrap_or_else(|| CommandStats {
            last_run: f64::NEG_INFINITY,
            last_directory: run.directory.clone(),
            last_exit_code: run.exit_code,
            flags: command_flags(primary),
            ..Default::default()
        });
        stats.runs += 1;
        if run.started_at >= stats.last_run {
            (stats.last_run, stats.last_directory, stats.last_exit_code) = (run.started_at, run.directory.clone(), run.exit_code);
            (stats.author, stats.intent) = (None, None);
        }
        stats.frecency += 0.5f64.powf((self.as_of - run.started_at).max(0.0) / USAGE_HALF_LIFE);
        if INTERRUPTED_EXIT_CODES.contains(&run.exit_code) || run.exit_code == 0
            || (run.exit_code == 1 && answers_with_exit_code(primary))
        {
            stats.successes += 1;
        }
        if within(&run.directory, &self.directory) {
            stats.runs_here += 1;
        }
        if self.project.as_deref().is_some_and(|p| within(&run.directory, p)) {
            stats.runs_project += 1;
        }
        self.captured.insert(key.clone(), stats);
        if let Some(entry) = self.make_entry(&key) {
            add_program(&entry, &mut self.programs);
            self.entries.insert(key, entry);
        }
    }

    fn make_entry(&self, command: &str) -> Option<SearchEntry> {
        let (typed, expanded) = command_parts(command);
        let mut entry = self.captured.get(command).filter(|_| !is_smcli_command(expanded.as_deref().unwrap_or(&typed))).map(|stats| SearchEntry {
            command: typed.clone(), expanded_command: expanded.clone(), alias_off: false,
            runs: stats.runs,
            last_run: Some(format_iso8601(stats.last_run, true)),
            last_directory: Some(stats.last_directory.clone()),
            last_exit_code: Some(stats.last_exit_code),
            source: "captured".into(),
            author: stats.author.clone(),
            intent: stats.intent.clone(),
            flags: stats.flags.clone(),
            frecency: stats.frecency,
            successes: stats.successes,
            runs_here: stats.runs_here,
            runs_project: stats.runs_project,
            why: None,
        });
        let Some(count) = self.native_counts.get(command) else { return entry };
        let usage = self.native_usage.get(command).copied().unwrap_or(0.0);
        match &mut entry {
            Some(entry) => entry.frecency = entry.frecency.max(usage),
            None => {
                entry = Some(SearchEntry {
                    flags: command_flags(&typed),
                    frecency: usage,
                    ..SearchEntry::new(&typed, *count, "native")
                })
            }
        }
        entry
    }
}

/// `runs_here` in SQL terms: the directory itself or below it; a root of "/" matches only itself.
fn within(path: &str, root: &str) -> bool {
    !root.is_empty() && (path == root || (root != "/" && path.starts_with(&format!("{root}/"))))
}

/// Index identity for an entry: the full command plus intent when `labeled` is false, which health
/// compares to the stored `doc`. Labeled text is the contextual document. Static models do not embed
/// this string; they embed `embedding_text`, so a 1,500-character cut never becomes the identity.
pub fn search_document(entry: &SearchEntry, labeled: bool) -> String {
    render_document(entry, labeled, if labeled { 1_500 } else { usize::MAX })
}

/// Bounded command plus intent. The only text a static model should see.
pub(crate) fn embedding_text(entry: &SearchEntry) -> String {
    // Typed aliases are an exact lexical surface, not semantic noise beside the executable text.
    let mut text = entry.primary().to_string();
    if let Some(intent) = entry.intent.as_deref().filter(|i| !i.is_empty()) {
        text.push('\n');
        text.push_str(&intent.replace('\n', " "));
    }
    text.chars().take(1_500).collect()
}

/// Command plus intent for the keyword index, capped at `LEXICAL_LIMIT`.
pub(crate) fn lexical_text(entry: &SearchEntry) -> String {
    render_document(entry, false, LEXICAL_LIMIT)
}

fn render_document(entry: &SearchEntry, labeled: bool, command_limit: usize) -> String {
    // Long commands (heredocs, pasted scripts) are cut so they fit embedding contexts with their labels intact.
    let mut lines = vec![entry.primary().chars().take(command_limit).collect::<String>()];
    lines.extend(entry.secondary().map(String::from));
    let intent = entry.intent.as_deref().filter(|i| !i.is_empty()).map(|i| i.replace('\n', " "));
    if !labeled {
        lines.extend(intent);
        return lines.join("\n").chars().take(command_limit).collect();
    }
    let parsed = CommandBreakdown::parse(entry.primary());
    if let Some(program) = program_of(entry.primary(), parsed.as_ref()) {
        lines.push(format!("command: {program}"));
    }
    if let Some(subcommand) = parsed.and_then(|p| p.subcommand) {
        lines.push(format!("subcommand: {subcommand}"));
    }
    if !entry.flags.is_empty() {
        lines.push(format!("flags: {}", entry.flags.join(" ")));
    }
    if let Some(author) = entry.author.as_deref().filter(|a| !a.is_empty()) {
        lines.push(format!("author: {author}"));
    }
    // Intent stays out of the label list in ShellTokenizer: its words are worth matching as keywords.
    lines.extend(intent.map(|i| format!("intent: {i}")));
    lines.join("\n")
}

/// How literal a query is. Flags, paths, revisions, assignments, variables, and commands from the history
/// carry exact constraints the keyword leg honors; plain words describe intent for embeddings.
/// `programs` maps each program in the history to the words seen right after it (`git` → `stash`, `log`).
pub fn is_literal_query(query: &str, programs: &HashMap<String, HashSet<String>>) -> bool {
    let words: Vec<&str> = query.split_whitespace().collect();
    let Some(first) = words.first() else { return false };
    let literal_word = |word: &&str| {
        let chars: Vec<char> = word.chars().collect();
        let inner = if chars.len() > 2 { &chars[1..chars.len() - 1] } else { &[][..] };
        word.starts_with('-') || word.contains('/') || word.contains('=') || word.starts_with('~')
            || word.contains(|c| "~^@|*{}".contains(c)) || *word == "HEAD"
            || (word.starts_with('$') && chars.len() > 1) || inner.contains(&':')
            || (word.contains('.') && !word.ends_with('.'))
    };
    if words.iter().any(literal_word) || (words.len() == 1 && first.contains('-')) {
        return true;
    }
    // A short query that reads as a command from the history (`git stash`, `make test`) is literal. Many
    // programs are also verbs (find, make, open), so the next word must be one seen after that program:
    // "find large files" is a request.
    let Some(next) = programs.get(&first.to_lowercase()).filter(|_| words.len() <= 4) else { return false };
    words.len() == 1 || next.contains(&words[1].to_lowercase())
}

/// Each program in `corpus` with the words that follow it, for `is_literal_query`.
pub fn program_arguments(corpus: &HashMap<String, SearchEntry>) -> HashMap<String, HashSet<String>> {
    let mut programs = HashMap::new();
    for entry in corpus.values() {
        add_program(entry, &mut programs);
    }
    programs
}

fn is_stopword(word: &str) -> bool {
    matches!(word, "how" | "do" | "i" | "a" | "an" | "the" | "to" | "my" | "new" | "in" | "for" | "of" | "with" | "and" | "what" | "is" | "me")
}

/// Derives index filters from the words in a natural-language query. Stopwords and ambiguity are
/// treated conservatively: a word only becomes a filter when it matches exactly one known program
/// or directory basename, and a subcommand is only inferred when a single program is unambiguous.
pub fn query_filter(query: &str, programs: &HashMap<String, HashSet<String>>, directories: &HashSet<String>) -> SearchFilter {
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
    let mut filter = SearchFilter::default();

    let program_candidates: Vec<&str> = words.iter().map(String::as_str).filter(|w| !is_stopword(w) && programs.contains_key(*w)).collect();
    if program_candidates.len() == 1 {
        filter.program = Some(program_candidates[0].to_string());
    }

    if let Some(program) = filter.program.as_deref() {
        if let Some(args) = programs.get(program) {
            let sub_candidates: Vec<&str> = words
                .iter()
                .map(String::as_str)
                .filter(|w| *w != program && !is_stopword(w) && w.len() >= 3 && args.contains(*w))
                .collect();
            if sub_candidates.len() == 1 {
                filter.subcommand = Some(sub_candidates[0].to_string());
            }
        }
    }

    let mut dir_matches = HashSet::new();
    for word in words.iter().map(String::as_str) {
        if is_stopword(word) || filter.program.as_deref() == Some(word) {
            continue;
        }
        for dir in directories {
            if std::path::Path::new(dir).file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(word)) {
                dir_matches.insert(dir.clone());
            }
        }
    }
    if dir_matches.len() == 1 {
        filter.directory = dir_matches.into_iter().next();
    }

    filter
}

fn resolve_marker(word: &str, programs: &HashMap<String, HashSet<String>>, directories: &HashSet<String>, filter: &mut SearchFilter) {
    if programs.contains_key(word) {
        filter.program = Some(word.to_string());
        return;
    }
    if let Some(program) = filter.program.as_deref() {
        if let Some(args) = programs.get(program) {
            if args.contains(word) {
                filter.subcommand = Some(word.to_string());
                return;
            }
        }
    }
    let mut matches = HashSet::new();
    for dir in directories {
        if std::path::Path::new(dir).file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(word)) {
            matches.insert(dir.clone());
        }
    }
    if matches.len() == 1 {
        filter.directory = matches.into_iter().next();
    }
}

/// Strips `$word` filter markers out of `query` and resolves them into a `SearchFilter`. Markers are
/// only recognized as complete whitespace-delimited tokens whose body is plain (letters, digits, `-`,
/// `_`, `.`), so `$HOME/x`, `${x}` and `$(...)` are left untouched. Returns the stripped query, the
/// filter derived from markers, and whether any marker was found.
pub fn parse_markers(query: &str, programs: &HashMap<String, HashSet<String>>, directories: &HashSet<String>) -> (String, SearchFilter, bool) {
    let mut stripped = String::with_capacity(query.len());
    let mut filter = SearchFilter::default();
    let mut has_markers = false;
    let mut chars = query.chars().peekable();
    let mut at_boundary = true;

    while let Some(c) = chars.next() {
        if c == '$' && at_boundary {
            let mut word = String::new();
            while let Some(&next) = chars.peek() {
                if next.is_alphanumeric() || next == '-' || next == '_' || next == '.' {
                    word.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            if !word.is_empty() && chars.peek().map_or(true, |c| c.is_whitespace()) {
                let lower = word.to_lowercase();
                has_markers = true;
                resolve_marker(&lower, programs, directories, &mut filter);
                stripped.push_str(&word);
                at_boundary = false;
                continue;
            }
            stripped.push('$');
            stripped.push_str(&word);
            at_boundary = false;
            continue;
        }
        stripped.push(c);
        at_boundary = c.is_whitespace();
    }

    (stripped, filter, has_markers)
}

fn add_program(entry: &SearchEntry, programs: &mut HashMap<String, HashSet<String>>) {
    let Some(program) = entry.program().map(|p| p.to_lowercase()) else { return };
    let words: Vec<&str> = entry.primary().split_whitespace().collect();
    let index = words.iter().position(|w| w.split('/').rfind(|p| !p.is_empty()).is_some_and(|p| p.to_lowercase() == program));
    let next = index.and_then(|i| words.get(i + 1)).map(|w| w.to_lowercase());
    programs.entry(program).or_default().extend(next);
}

/// What search knows about the moment of the query besides the corpus: the project and its candidate commands,
/// the session's previous command with what usually follows it, and earlier picks. All of it is usage and context
/// metadata applied after retrieval; none of it enters the embedded text or the keyword index.
#[derive(Debug, Clone, Default)]
pub struct SearchSignals {
    pub context: Option<RankingContext>,
    pub shell_path: Option<String>,
    pub directory: Option<String>,
    pub previous: Option<String>,
    pub followers: HashMap<String, i64>,
    pub picks: SelectionLog,
}

pub type Hit = (SearchEntry, f64);

impl SearchSignals {
    /// Relevance a pick lends a command: a repeated identical query is strong evidence, even for a command the
    /// embedding finds only loosely related.
    fn pick_relevance(weight: f64) -> f64 {
        (0.5 + 0.25 * weight).min(1.0)
    }

    /// Commands that belong in the pool although retrieval did not return them: ones picked for a similar query,
    /// and project scripts or Git-state commands whose words all appear in the query. They enter with a lexical
    /// relevance and still go through `rerank` like any hit; nothing is added when it does not match.
    pub fn additions(&self, query: &str, corpus: &HashMap<String, SearchEntry>, existing: &HashSet<String>,
                     filter: &SearchFilter, now: f64) -> Vec<Hit> {
        let context = self.context.as_ref();
        let mut added = vec![];
        let mut seen = existing.clone();
        if uses(context, Signal::Selection) {
            let mut weights: Vec<(String, f64)> = self.picks.weights(query, now).into_iter().collect();
            weights.sort_by(|a, b| a.0.cmp(&b.0));
            for (command, weight) in weights {
                let Some(entry) = corpus.get(&command).filter(|e| !seen.contains(&command) && filter.allows(e)) else { continue };
                seen.insert(command);
                added.push((entry.clone(), Self::pick_relevance(weight)));
            }
        }
        let terms: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
        if terms.is_empty() {
            return added;
        }
        let candidates = context.map(|c| [c.candidates_of(CandidateKind::Hint), c.candidates_of(CandidateKind::Script)].concat());
        for candidate in candidates.unwrap_or_default() {
            if seen.contains(&candidate.command) || coverage(&candidate.command, &terms) != 1.0 {
                continue;
            }
            let entry = corpus.get(&candidate.command).cloned().unwrap_or_else(|| SearchEntry {
                flags: command_flags(&candidate.command),
                ..SearchEntry::new(&candidate.command, 0, "project")
            });
            if !filter.allows(&entry) {
                continue;
            }
            seen.insert(candidate.command.clone());
            added.push((entry, 1.0));
        }
        added
    }
}

/// The share of query words that are a word of the command or start one (`sto` for `storybook`), counting the
/// pieces of `--continue`; 1 when all of them are.
pub fn entry_coverage(entry: &SearchEntry, terms: &[String]) -> f64 {
    coverage(entry.primary(), terms).max(entry.secondary().map_or(0.0, |s| coverage(s, terms)))
}

pub fn coverage(command: &str, terms: &[String]) -> f64 {
    if terms.is_empty() {
        return 0.0;
    }
    let tokens: HashSet<String> = crate::ShellTokenizer::tokenize(command).into_iter().collect();
    let matched = terms
        .iter()
        .filter(|term| tokens.iter().any(|t| t == *term || (term.chars().count() >= 2 && t.starts_with(term.as_str()))))
        .count();
    matched as f64 / terms.len() as f64
}

/// Reorders hybrid hits with usage evidence. `relevance` is each hit's fused score scaled so the best is 1.
/// Usage multiplies it as `boost ^ relevance`, so a habit lifts a strong match nearly fully but barely moves a
/// weak one: rank fusion leaves even distant dense neighbours a third of the best score, and a plain product
/// would let frequent unrelated commands outrank real matches.
/// Failures lower the score without excluding: the failed command may be exactly what is wanted.
/// `signals` adds the project, the previous command, earlier picks, project scripts and Git state, each with a `why`.
pub fn rerank(hits: Vec<Hit>, literal: bool, query: &str, now: f64, signals: &SearchSignals) -> Vec<SearchEntry> {
    let query_terms: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
    let terms = smcli_core::parse::shell_words(query).unwrap_or_default();
    let context = signals.context.as_ref();
    let picks = if uses(context, Signal::Selection) { signals.picks.weights(query, now) } else { HashMap::new() };
    let first_of = |kind| {
        let mut map: HashMap<String, String> = HashMap::new();
        for candidate in context.map(|c| c.candidates_of(kind)).unwrap_or_default() {
            map.entry(candidate.command.clone()).or_insert_with(|| candidate.reason.clone());
        }
        map
    };
    let (scripts, hints) = (first_of(CandidateKind::Script), first_of(CandidateKind::Hint));
    let empty = HashMap::new();
    let followed = if uses(context, Signal::Transition) { &signals.followers } else { &empty };
    let follow_total = followed.values().sum::<i64>() as f64;
    // An extra typed spelling must not impose a BM25 length penalty on identical executable text.
    let mut primary_relevance: HashMap<String, f64> = HashMap::new();
    for (entry, relevance) in &hits {
        primary_relevance.entry(entry.primary().into()).and_modify(|r| *r = r.max(*relevance)).or_insert(*relevance);
    }
    let evidence = crate::filesystem::evidence(&hits, signals,
        uses(context, Signal::Path) && signals.shell_path.is_some(),
        uses(context, Signal::Files) && signals.directory.is_some());
    let mut scored: Vec<(usize, SearchEntry, f64)> = hits
        .into_iter()
        .enumerate()
        .map(|(offset, (mut entry, mut relevance))| {
            relevance = primary_relevance.get(entry.primary()).copied().unwrap_or(relevance);
            let mut why: Vec<String> = vec![];
            let exact = !terms.is_empty() && [Some(entry.primary()), entry.secondary()].into_iter().flatten().any(|text| {
                let words = smcli_core::parse::shell_words(text).unwrap_or_default();
                terms.iter().all(|term| words.iter().any(|word| word.eq_ignore_ascii_case(term)))
            });
            why.push(if exact { "exact words" } else if literal { "keyword match" } else { "semantic match" }.into());
            let mut boost = 1.0 + 0.15 * entry.frecency.ln_1p();
            // Here outweighs elsewhere in the project. ranking.rs passes with project >= 0.9 and here in (project, 1.3].
            if entry.runs_here > 0 {
                boost += 1.1 * (entry.runs_here as f64 / entry.runs.max(1) as f64 + 0.25).min(1.0);
                why.push("used here".into());
            } else if entry.runs_project > 0 && uses(context, Signal::Project) {
                boost += 1.0 * (entry.runs_project as f64 / entry.runs.max(1) as f64 + 0.25).min(1.0);
                why.push("used in this project".into());
            }
            if entry.last_run_epoch().is_some_and(|last| now - last < 7.0 * 86_400.0) {
                boost += 0.1;
                why.push("recent".into());
            }
            if entry.runs >= 5 {
                why.push(format!("used {} times", entry.runs));
            }
            if exact {
                boost += if literal { 1.0 } else { 0.2 };
            }
            // A project command counts in proportion to how much of the query it matches, so an unrelated script does
            // not ride along on a hint or a habit.
            let covered = if hints.contains_key(&entry.command) || scripts.contains_key(&entry.command) {
                entry_coverage(&entry, &query_terms)
            } else {
                0.0
            };
            if covered > 0.0 && let Some(reason) = hints.get(&entry.command) {
                boost += 1.5 * covered;
                why.push(reason.clone());
            } else if covered > 0.0 && let Some(reason) = scripts.get(&entry.command) {
                boost += 0.8 * covered;
                why.push(reason.clone());
            }
            if let (Some(previous), Some(count)) = (&signals.previous, followed.get(&entry.key()))
                && follow_total > 0.0 {
                    boost += *count as f64 / follow_total * (*count as f64 / 3.0).min(1.0);
                    why.push(format!("often follows {}", command_parts(previous).0));
                }
            if let Some(weight) = picks.get(&entry.key()) {
                boost += (0.75 * weight).min(1.5);
                relevance = relevance.max(SearchSignals::pick_relevance(*weight));
                why.push("picked for similar searches".into());
            }
            if entry.source == "captured" && entry.runs > 0 {
                let success_rate = entry.successes as f64 / entry.runs as f64;
                boost *= 0.75 + 0.25 * success_rate;
                if entry.successes == 0 {
                    why.push("always failed".into());
                }
            }
            let (available, present) = evidence.get(offset).copied().unwrap_or_default();
            let mut score = relevance * boost.powf(relevance);
            if available {
                score += 0.1 * relevance;
                why.push("program found in PATH".into());
            }
            if present {
                score += 0.08 * relevance;
                why.push("input file exists".into());
            }
            entry.why = Some(why);
            (offset, entry, score)
        })
        .collect();
    scored.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)));
    scored.into_iter().map(|(_, entry, _)| entry).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(command: &str, runs: i64, frecency: f64, successes: Option<i64>, runs_here: i64) -> SearchEntry {
        SearchEntry {
            last_run: Some("2023-11-14T22:13:20Z".into()),
            last_directory: Some("/repo".into()),
            last_exit_code: Some(0),
            frecency,
            successes: successes.unwrap_or(runs),
            runs_here,
            ..SearchEntry::new(command, runs, "captured")
        }
    }

    #[test]
    fn alias_documents_preserve_provenance_intent_caps_and_incremental_variants() {
        let mut entry = SearchEntry { expanded_command: Some("git status".into()), intent: Some("authored purpose".into()), ..SearchEntry::new("gst", 1, "captured") };
        assert_eq!(embedding_text(&entry), "git status\nauthored purpose");
        assert_eq!(lexical_text(&entry), "git status\ngst\nauthored purpose");
        assert_eq!(entry.program().as_deref(), Some("git"));
        assert_eq!(entry.subcommand().as_deref(), Some("status"));
        let key = entry.key();
        assert_eq!(command_parts(&key), ("gst".into(), Some("git status".into())));
        assert_ne!(key, command_key(&key, None));
        entry.expanded_command = Some("x".repeat(9_000));
        assert_eq!(embedding_text(&entry).chars().count(), 1_500);
        assert_eq!(lexical_text(&entry).chars().count(), LEXICAL_LIMIT);
        let root = tempfile::tempdir().unwrap();
        let history = smcli_core::history::CapturedHistory::new(root.path().join("history.db"));
        let db = history.open_connection().unwrap();
        let mut state = CorpusState::new(HashMap::new(), &[], "/repo", None, 100.0);
        for expanded in ["git log", "git ls-files"] {
            let mut run = smcli_core::history::NewRun::new("gl", "zsh", "/repo", 0);
            run.expanded_command = Some(expanded);
            run.started_at = Some(99.0);
            history.append(&run, "", Some(&db)).unwrap();
            state.add(&Run { command: "gl".into(), expanded_command: Some(expanded.into()), directory: "/repo".into(), started_at: 99.0, exit_code: 0, session: "unknown".into() });
        }
        let rebuilt = CorpusState::new(history.command_stats(Some("/repo"), None, 100.0).unwrap(), &[], "/repo", None, 100.0);
        assert_eq!(state.entries, rebuilt.entries);
        assert_eq!(state.entries.len(), 2);
    }

    #[test]
    fn classify_literal_and_intent_queries() {
        let corpus: HashMap<String, SearchEntry> =
            ["git stash", "git reset --hard", "docker ps", "find . -name x", "make test", "/usr/bin/head -n 5 log"]
                .iter()
                .map(|c| (c.to_string(), SearchEntry::new(c, 1, "native")))
                .collect();
        let programs = program_arguments(&corpus);
        assert_eq!(programs["git"], HashSet::from(["stash".to_string(), "reset".to_string()]));
        assert_eq!(programs["head"], HashSet::from(["-n".to_string()]));
        for query in ["--soft", "git reset --soft HEAD~1", "HEAD~1", "src/main.swift", "git stash", "FOO=1", "a | b",
                      "make test", "git", "echo $HOME", "scp host:file"] {
            assert!(is_literal_query(query, &programs), "{query}");
        }
        for query in ["undo the last commit but keep the changes", "list running containers", "git rid of my changes please now",
                      "find large files", "show head of log", "make it faster", "error: not found", "cost in $"] {
            assert!(!is_literal_query(query, &programs), "{query}");
        }
    }

    #[test]
    fn rerank_by_usage_and_context() {
        let now = 1_700_000_000.0;
        let none = SearchSignals::default();
        let ranked = rerank(vec![
            (entry("git reset --hard", 1, 0.0, None, 0), 1.0),
            (entry("git reset --soft HEAD~1", 20, 15.0, None, 10), 0.9),
            (entry("git log", 500, 400.0, None, 0), 0.1),
        ], false, "undo commit", now, &none);
        assert_eq!(ranked.iter().map(|e| e.command.as_str()).collect::<Vec<_>>(), ["git reset --soft HEAD~1", "git reset --hard", "git log"]);
        assert_eq!(ranked[0].why.as_deref().unwrap(), ["semantic match", "used here", "recent", "used 20 times"]);
        // Failures are demoted, not dropped.
        let failing = rerank(vec![(entry("make deploy", 4, 0.0, Some(0), 0), 1.0), (entry("make test", 4, 0.0, None, 0), 0.9)],
                             true, "make", now, &none);
        assert_eq!(failing[0].command, "make test");
        assert!(failing[1].why.as_ref().unwrap().contains(&"always failed".to_string()));
        // A heavy habit that is only a distant neighbour stays below a real match.
        let habit = rerank(vec![(entry("rm -rf build", 1, 0.0, None, 0), 0.49), (entry("ls -la", 80, 60.0, None, 80), 0.34)],
                           false, "undo last commit", now, &none);
        assert_eq!(habit[0].command, "rm -rf build");
    }

    #[test]
    fn embed_only_command_and_intent_for_static_models() {
        let mut e = SearchEntry::new("git commit --amend", 1, "captured");
        e.intent = Some("fix the\ntypo".into());
        assert_eq!(search_document(&e, false), "git commit --amend\nfix the typo");
        e.flags = vec!["--amend".into()];
        assert_eq!(search_document(&e, true), "git commit --amend\ncommand: git\nsubcommand: commit\nflags: --amend\nintent: fix the typo");
    }

    #[test]
    fn add_runs_like_a_rebuild() {
        let mut state = CorpusState::new(HashMap::new(), &["ls".into(), "smcli x".into()], "/repo", Some("/repo"), 100.0);
        assert_eq!(state.entries["ls"].source, "native");
        assert!(!state.entries.contains_key("smcli x"));
        state.add(&Run { command: "ls".into(), expanded_command: None, directory: "/repo/a".into(), started_at: 100.0, exit_code: 0, session: "s".into() });
        let ls = &state.entries["ls"];
        assert_eq!((ls.source.as_str(), ls.runs, ls.runs_here, ls.runs_project, ls.frecency), ("captured", 1, 1, 1, 1.0));
    }

    #[test]
    fn query_filter_extracts_program_and_subcommand() {
        let programs = HashMap::from([
            ("bun".to_string(), HashSet::from(["i".to_string(), "create".to_string(), "init".to_string()])),
        ]);
        let filter = query_filter("how do i create a new bun project", &programs, &HashSet::new());
        assert_eq!(filter.program, Some("bun".to_string()));
        assert_eq!(filter.subcommand, Some("create".to_string()));
        assert_eq!(filter.directory, None);
    }

    #[test]
    fn query_filter_does_not_treat_english_i_as_bun_subcommand() {
        let programs = HashMap::from([
            ("bun".to_string(), HashSet::from(["i".to_string(), "create".to_string()])),
        ]);
        let filter = query_filter("how do i install usearch with bun", &programs, &HashSet::new());
        assert_eq!(filter.program, Some("bun".to_string()));
        assert_eq!(filter.subcommand, None);
    }

    #[test]
    fn query_filter_matches_directory_basename() {
        let directories = HashSet::from(["/Users/arobida/Dev/Apps/smcli".to_string()]);
        let filter = query_filter("how do i build my smcli project", &HashMap::new(), &directories);
        assert_eq!(filter.directory, Some("/Users/arobida/Dev/Apps/smcli".to_string()));
        assert_eq!(filter.program, None);
        assert_eq!(filter.subcommand, None);
    }

    #[test]
    fn query_filter_is_conservative_with_ambiguity() {
        let programs = HashMap::from([
            ("bun".to_string(), HashSet::from(["create".to_string()])),
            ("git".to_string(), HashSet::from(["status".to_string()])),
        ]);
        let directories = HashSet::from(["/home/bun".to_string(), "/home/git".to_string()]);
        let filter = query_filter("bun git create status", &programs, &directories);
        assert_eq!(filter.program, None);
        assert_eq!(filter.directory, None);
    }

    #[test]
    fn query_filter_skips_directory_word_that_is_the_program() {
        let programs = HashMap::from([
            ("smcli".to_string(), HashSet::from(["build".to_string()])),
        ]);
        let directories = HashSet::from(["/Users/arobida/Dev/Apps/smcli".to_string()]);
        let filter = query_filter("how do i build my smcli project", &programs, &directories);
        assert_eq!(filter.program, Some("smcli".to_string()));
        assert_eq!(filter.directory, None);
    }

    #[test]
    fn explicit_filter_values_win_over_parsed() {
        let mut explicit = SearchFilter { program: Some("git".into()), ..Default::default() };
        let parsed = SearchFilter {
            program: Some("bun".into()),
            subcommand: Some("create".into()),
            directory: Some("/x".into()),
            ..Default::default()
        };
        explicit.merge_parsed(&parsed);
        assert_eq!(explicit.program, Some("git".into()));
        assert_eq!(explicit.subcommand, Some("create".into()));
        assert_eq!(explicit.directory, Some("/x".into()));
    }

    #[test]
    fn marker_parsing_extracts_program_and_subcommand() {
        let programs = HashMap::from([
            ("bun".to_string(), HashSet::from(["i".to_string(), "create".to_string()])),
        ]);
        let (stripped, filter, has_markers) = parse_markers("$bun command to $create new project", &programs, &HashSet::new());
        assert!(has_markers);
        assert_eq!(stripped, "bun command to create new project");
        assert_eq!(filter.program, Some("bun".to_string()));
        assert_eq!(filter.subcommand, Some("create".to_string()));
    }

    #[test]
    fn marker_allows_short_subcommand() {
        let programs = HashMap::from([
            ("bun".to_string(), HashSet::from(["i".to_string(), "create".to_string()])),
        ]);
        let (_, filter, _) = parse_markers("$bun $i usearch", &programs, &HashSet::new());
        assert_eq!(filter.program, Some("bun".to_string()));
        assert_eq!(filter.subcommand, Some("i".to_string()));
    }

    #[test]
    fn unknown_markers_are_left_as_words() {
        let programs = HashMap::from([
            ("bun".to_string(), HashSet::from(["create".to_string()])),
        ]);
        let (stripped, filter, has_markers) = parse_markers("$bun $unknown stuff", &programs, &HashSet::new());
        assert!(has_markers);
        assert_eq!(stripped, "bun unknown stuff");
        assert_eq!(filter.program, Some("bun".to_string()));
        assert_eq!(filter.subcommand, None);
    }

    #[test]
    fn shell_like_dollar_expressions_stay_intact() {
        let programs = HashMap::<String, HashSet<String>>::new();
        let directories = HashSet::new();
        for query in ["echo $HOME/x", "echo ${x}", "echo $(date)"] {
            let (stripped, filter, has_markers) = parse_markers(query, &programs, &directories);
            assert!(!has_markers, "{query}");
            assert_eq!(stripped, query);
            assert!(filter.is_empty(), "{query}");
        }
    }

    #[test]
    fn subsecond_last_run_survives_since_before_filters() {
        let mut captured = HashMap::new();
        captured.insert("cat".into(), CommandStats {
            runs: 1,
            last_run: 1_700_000_000.5,
            last_directory: "/tmp".into(),
            last_exit_code: 0,
            successes: 1,
            ..Default::default()
        });
        let entry = &search_corpus(captured, &[])["cat"];
        assert_eq!(entry.last_run.as_deref(), Some("2023-11-14T22:13:20.500Z"));
        assert!(SearchFilter { since: Some(1_700_000_000.25), ..Default::default() }.allows(entry));
        assert!(!SearchFilter { before: Some(1_700_000_000.25), ..Default::default() }.allows(entry));
    }

    #[test]
    fn stripped_marker_text_is_literal_when_it_reads_as_command() {
        let programs = HashMap::from([
            ("bun".to_string(), HashSet::from(["create".to_string()])),
        ]);
        let (stripped, _, _) = parse_markers("$bun $create", &programs, &HashSet::new());
        assert!(is_literal_query(&stripped, &programs));
    }
}
