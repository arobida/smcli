use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::context::{KIND_CACHE_TTL, KindCache, directory_kind};
use crate::entry::{command_key, command_parts};
use crate::history::{
    INTERRUPTED_EXIT_CODES, Run, Suggestion, USAGE_HALF_LIFE, answers_with_exit_code,
};
use crate::parse::is_smcli_command;
use crate::ranking::{
    RankingContext, SelectionLog, Signal, apply_ranking_signals, has_prefix, suggestion_why, uses,
};
use crate::time::{format_iso8601, now};

/// Which command follows which within a shell session, as plain counts. The previous command is the session's
/// last one that wasn't smcli itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommandAffinity {
    pub followers: HashMap<String, HashMap<String, i64>>,
    pub last: HashMap<String, String>,
    pub last_start: HashMap<String, f64>,
}

impl CommandAffinity {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns false when `started_at` is older than the last run in this session, which means the
    /// caller's arrival order no longer matches chronological order and the index should be rebuilt.
    pub fn add(&mut self, session: &str, command: &str, started_at: f64) -> bool {
        if self
            .last_start
            .get(session)
            .is_some_and(|&t| started_at < t)
        {
            return false;
        }
        if let Some(previous) = self.last.get(session) {
            *self
                .followers
                .entry(previous.clone())
                .or_default()
                .entry(command.into())
                .or_default() += 1;
        }
        self.last.insert(session.into(), command.into());
        self.last_start.insert(session.into(), started_at);
        true
    }

    pub fn following(&self, previous: &str) -> HashMap<String, i64> {
        self.followers.get(previous).cloned().unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
struct Tally {
    score: f64,
    runs: i64,
    last_run: f64,
    last_exit_code: i64,
}
impl Default for Tally {
    fn default() -> Self {
        Self {
            score: 0.0,
            runs: 0,
            last_run: f64::NEG_INFINITY,
            last_exit_code: 0,
        }
    }
}
#[derive(Debug, Clone, Default)]
struct DirectoryTally {
    score: f64,
    runs: i64,
}

/// `CapturedHistory::suggestions` kept in memory and updated per record, so a query costs the number of distinct
/// commands (plus those run in related directories) rather than a scan of every run.
///
/// The score is the same sum: each run weighs 0.5^(age / half-life), ×4 in the query directory, ×3 elsewhere in its
/// project, ×2 above or below it, ×0.25 when it failed. Transitions (what follows what in a session) and picks are
/// kept here too, and `apply_ranking_signals` is shared with the scan. The decay factors out of the sum
/// (0.5^((now − t)/h) = 2^((t − base)/h) · 2^((base − now)/h)), so tallies store 2^((t − base)/h) and a query
/// multiplies by one factor. The directory bonus is added as (weight − 1) × that directory's tally.
/// One difference: a run stamped in the future counts slightly more than 1 here until its time passes,
/// where the scan clamps its age to zero.
#[derive(Debug)]
pub struct SuggestionIndex {
    /// Reference time for stored weights; rebuild before 2^((t − base)/h) could overflow.
    base: f64,
    commands: HashMap<String, Tally>,
    directories: HashMap<String, Arc<HashMap<String, DirectoryTally>>>,
    /// Project kind of each run directory, re-probed when an entry is older than its ttl (up to 60 s of lag).
    /// Shared with snapshots and behind a mutex because ranking only borrows the index.
    kinds: Arc<Mutex<KindCache>>,
    answering: HashMap<String, bool>,
    affinity: CommandAffinity,
    picks: SelectionLog,
}

impl SuggestionIndex {
    pub fn new(runs: Vec<Run>, picks: SelectionLog) -> Self {
        let mut index = Self {
            base: runs.last().map_or_else(now, |r| r.started_at),
            commands: HashMap::new(),
            directories: HashMap::new(),
            kinds: Arc::new(Mutex::new(KindCache::new(KIND_CACHE_TTL))),
            answering: HashMap::new(),
            affinity: CommandAffinity::new(),
            picks,
        };
        for run in runs {
            index.add(&run);
        }
        index
    }

    /// Request-local ranking data, without the run log; use only for reading suggestions.
    pub fn snapshot(&self, prefix: &str, context: Option<&RankingContext>) -> Self {
        let commands: HashMap<_, _> = self
            .commands
            .iter()
            .filter(|(key, _)| {
                let (typed, expanded) = command_parts(key);
                has_prefix(&typed, prefix)
                || (uses(context, Signal::Alias)
                    && expanded.is_some_and(|e| has_prefix(&e, prefix)))
                // Ranking can add candidates after the prefix and scope filters.
                || context.is_some_and(|c| c.candidates.iter().any(|c| &c.command == *key))
            })
            .map(|(key, tally)| (key.clone(), tally.clone()))
            .collect();
        // Clone only the outer map to preserve floating-point accumulation order.
        let mut directories = self.directories.clone();
        for tallies in directories.values_mut() {
            *tallies = Arc::new(
                tallies
                    .iter()
                    .filter(|(key, _)| commands.contains_key(*key))
                    .map(|(key, tally)| (key.clone(), tally.clone()))
                    .collect(),
            );
        }
        let mut affinity = CommandAffinity::new();
        if let Some(session) = context.and_then(|c| c.session.as_ref()) {
            if let Some(previous) = self.affinity.last.get(session) {
                affinity.last.insert(session.clone(), previous.clone());
                // Nonmatching followers still contribute to the transition denominator.
                if let Some(followers) = self.affinity.followers.get(previous) {
                    affinity
                        .followers
                        .insert(previous.clone(), followers.clone());
                }
            }
            if let Some(start) = self.affinity.last_start.get(session) {
                affinity.last_start.insert(session.clone(), *start);
            }
        }
        Self {
            base: self.base,
            commands,
            directories,
            kinds: Arc::clone(&self.kinds),
            answering: HashMap::new(),
            affinity,
            picks: self.picks.clone(),
        }
    }

    /// Remembers that `command` was picked for `query`, as `record_pick` stored it.
    pub fn add_pick(&mut self, query: &str, command: &str, time: f64) {
        self.picks.add(query, command, time);
    }

    /// Adds one run. Runs are expected in (started_at, id) order; an older one still counts, but doesn't
    /// replace the last-run fields. Returns false when the run is older than its session's last run,
    /// or the base time is too old, signalling a rebuild from the database.
    pub fn add(&mut self, run: &Run) -> bool {
        if is_smcli_command(run.expanded_command.as_deref().unwrap_or(&run.command)) {
            return true;
        }
        let started_at = run.started_at.min(now());
        if (started_at - self.base) / USAGE_HALF_LIFE > 500.0 {
            return false;
        }
        if !self.affinity.add(
            &run.session,
            &command_key(&run.command, run.expanded_command.as_deref()),
            started_at,
        ) {
            return false;
        }
        let mut run = run.clone();
        run.started_at = started_at;
        self.tally(&run);
        true
    }

    fn tally(&mut self, run: &Run) {
        let key = command_key(&run.command, run.expanded_command.as_deref());
        let primary = run.expanded_command.as_deref().unwrap_or(&run.command);
        let mut weight = 2f64.powf((run.started_at - self.base) / USAGE_HALF_LIFE);
        if run.exit_code != 0 && !INTERRUPTED_EXIT_CODES.contains(&run.exit_code) {
            let answers = *self
                .answering
                .entry(key.clone())
                .or_insert_with(|| answers_with_exit_code(primary));
            if !(run.exit_code == 1 && answers) {
                weight *= 0.25;
            }
        }
        let tally = self.commands.entry(key.clone()).or_default();
        tally.score += weight;
        tally.runs += 1;
        if run.started_at >= tally.last_run {
            tally.last_run = run.started_at;
            tally.last_exit_code = run.exit_code;
        }
        let directory = Arc::make_mut(self.directories.entry(run.directory.clone()).or_default())
            .entry(key)
            .or_default();
        directory.score += weight;
        directory.runs += 1;
    }

    /// Uses the current Unix time; use `suggestions_at` for deterministic queries.
    pub fn suggestions(
        &self,
        directory: &str,
        prefix: &str,
        limit: usize,
        context: Option<&RankingContext>,
    ) -> Vec<Suggestion> {
        self.suggestions_at(directory, prefix, limit, now(), context)
    }

    /// Ranks at the supplied Unix time, with the same signals as the database scan.
    pub fn suggestions_at(
        &self,
        directory: &str,
        prefix: &str,
        limit: usize,
        now: f64,
        context: Option<&RankingContext>,
    ) -> Vec<Suggestion> {
        self.suggestions_with_evidence(directory, prefix, limit, now, context, None, None, None)
    }

    pub fn suggestions_with_evidence(
        &self,
        directory: &str,
        prefix: &str,
        limit: usize,
        now: f64,
        context: Option<&RankingContext>,
        evidence: Option<&dyn Fn(&[String]) -> Vec<bool>>,
        identifiers: Option<&dyn Fn(&[String]) -> Vec<(bool, bool)>>,
        git_activity: Option<&dyn Fn(&[String]) -> Vec<f64>>,
    ) -> Vec<Suggestion> {
        let here = if directory.len() > 1 {
            directory.strip_suffix('/').unwrap_or(directory)
        } else {
            directory
        };
        let scale = 2f64.powf((self.base - now) / USAGE_HALF_LIFE);
        let mut scores: HashMap<String, f64> = self
            .commands
            .iter()
            .filter(|(c, _)| {
                let (typed, expanded) = command_parts(c);
                has_prefix(&typed, prefix)
                    || (uses(context, Signal::Alias)
                        && expanded.is_some_and(|e| has_prefix(&e, prefix)))
            })
            .map(|(c, t)| (c.clone(), t.score))
            .collect();
        let mut project_runs: HashMap<String, i64> = HashMap::new();
        let mut kind_runs: HashMap<String, i64> = HashMap::new();
        for (directory, tallies) in &self.directories {
            // Only a directory holding a command that matched the prefix can contribute anything.
            if !tallies.keys().any(|command| scores.contains_key(command)) {
                continue;
            }
            let in_project = context.is_some_and(|c| c.in_project(directory));
            let (bonus, same_kind) = if directory == here {
                (3.0, false)
            } else if in_project {
                (2.0, false)
            } else if here != "/"
                && directory != "/"
                && (directory.starts_with(&format!("{here}/"))
                    || here.starts_with(&format!("{directory}/")))
            {
                (1.0, false)
            } else if context.is_some_and(|c| {
                c.kind().is_some() && c.same_kind(self.kind_of(directory).as_deref())
            }) {
                (0.5, true)
            } else {
                continue;
            };
            for (command, tally) in tallies.iter() {
                if let Some(score) = scores.get_mut(command) {
                    *score += bonus * tally.score;
                    if in_project {
                        *project_runs.entry(command.clone()).or_default() += tally.runs;
                    }
                    if same_kind {
                        *kind_runs.entry(command.clone()).or_default() += tally.runs;
                    }
                }
            }
        }
        // Scores are stored relative to `base`; the signals work on real scores.
        for score in scores.values_mut() {
            *score *= scale;
        }
        let previous = context
            .filter(|_| uses(context, Signal::Transition))
            .and_then(|c| c.session.as_ref())
            .and_then(|s| self.affinity.last.get(s));
        let followers = previous.map(|p| self.following(p)).unwrap_or_default();
        let mut why = apply_ranking_signals(
            &mut scores,
            prefix,
            context,
            previous.map(String::as_str),
            &followers,
            &self.picks.weights(prefix, now),
        );
        if uses(context, Signal::Files)
            && uses(context, Signal::Index)
            && let Some(evidence) = evidence
        {
            let keys: Vec<String> = scores.keys().cloned().collect();
            let commands: Vec<String> = keys
                .iter()
                .map(|key| {
                    if uses(context, Signal::Alias) {
                        key.clone()
                    } else {
                        command_parts(key).0
                    }
                })
                .collect();
            for (command, present) in keys.iter().zip(evidence(&commands)) {
                if present {
                    *scores.get_mut(command).unwrap() += 0.08;
                    why.entry(command.clone())
                        .or_default()
                        .push("input file exists".into());
                }
            }
        }
        if uses(context, Signal::Git)
            && let Some(git_activity) = git_activity
        {
            let keys: Vec<String> = scores.keys().cloned().collect();
            let commands: Vec<String> = keys
                .iter()
                .map(|key| {
                    if uses(context, Signal::Alias) {
                        key.clone()
                    } else {
                        command_parts(key).0
                    }
                })
                .collect();
            for (command, activity) in keys.iter().zip(git_activity(&commands)) {
                if activity > 0.0 {
                    *scores.get_mut(command).unwrap() += 0.04 * activity;
                    why.entry(command.clone())
                        .or_default()
                        .push(if activity >= 1.0 {
                            "edited".into()
                        } else {
                            "recently changed".into()
                        });
                }
            }
        }
        if uses(context, Signal::Evidence)
            && let Some(identifiers) = identifiers
        {
            let mut keys: Vec<String> = scores.keys().cloned().collect();
            keys.sort_by(|a, b| scores[b].total_cmp(&scores[a]).then_with(|| a.cmp(b)));
            let commands: Vec<String> = keys
                .iter()
                .map(|key| {
                    let (typed, expanded) = command_parts(key);
                    if uses(context, Signal::Alias) {
                        expanded.unwrap_or(typed)
                    } else {
                        typed
                    }
                })
                .collect();
            for (key, (seen, defined)) in keys.into_iter().zip(identifiers(&commands)) {
                if defined {
                    *scores.get_mut(&key).unwrap() += 0.06;
                    why.entry(key)
                        .or_default()
                        .push("defined in project".into());
                } else if seen {
                    *scores.get_mut(&key).unwrap() += 0.03;
                    why.entry(key).or_default().push("found in project".into());
                }
            }
        }
        let last = |c: &str| {
            self.commands
                .get(c)
                .map_or(f64::NEG_INFINITY, |t| t.last_run)
        };
        let mut sorted: Vec<_> = scores.into_iter().collect();
        sorted.sort_by(|(a, x), (b, y)| {
            y.total_cmp(x)
                .then_with(|| last(b).total_cmp(&last(a)))
                .then_with(|| a.cmp(b))
        });
        sorted
            .into_iter()
            .take(limit)
            .map(|(command, score)| {
                let tally = self.commands.get(&command);
                let runs_here = self
                    .directories
                    .get(here)
                    .and_then(|d| d.get(&command))
                    .map_or(0, |t| t.runs);
                let explanation = suggestion_why(
                    runs_here,
                    *project_runs.get(&command).unwrap_or(&0),
                    *kind_runs.get(&command).unwrap_or(&0),
                    context.and_then(|c| c.project_kind.as_deref()),
                    why.get(&command),
                );
                Suggestion {
                    score,
                    runs: tally.map_or(0, |t| t.runs),
                    runs_here,
                    last_run: tally.map_or_else(String::new, |t| format_iso8601(t.last_run, false)),
                    last_exit_code: tally.map_or(0, |t| t.last_exit_code),
                    why: explanation,
                    expanded_command: command_parts(&command).1,
                    command: command_parts(&command).0,
                }
            })
            .collect()
    }

    /// The cached kind of a run directory; the kinds mutex is released while the disk is probed.
    fn kind_of(&self, directory: &str) -> Option<String> {
        if let Some(kind) = self.kinds.lock().unwrap().lookup(directory) {
            return kind;
        }
        let kind = directory_kind(directory);
        self.kinds.lock().unwrap().insert(directory, kind.clone());
        kind
    }

    /// The last command a session ran, which transitions out of the session's next command key on.
    pub fn previous_command(&self, session: &str) -> Option<String> {
        self.affinity.last.get(session).cloned()
    }
    /// How often each command directly followed `previous` in a session.
    pub fn following(&self, previous: &str) -> HashMap<String, i64> {
        self.affinity.following(previous)
    }
    /// Every distinct command, most recently run first (ties by command), as the picker lists full history.
    pub fn recent(&self) -> Vec<String> {
        let mut commands: Vec<_> = self.commands.iter().collect();
        commands.sort_by(|(a, x), (b, y)| y.last_run.total_cmp(&x.last_run).then_with(|| a.cmp(b)));
        commands.into_iter().map(|(c, _)| c.clone()).collect()
    }
    pub fn picks(&self) -> &SelectionLog {
        &self.picks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CaptureFilter;
    use crate::history::{CapturedHistory, NewRun, user_name};
    use crate::ranking::{Candidate, CandidateKind};

    #[test]
    fn snapshot_keeps_request_ranking_data() {
        let runs = [
            ("s", "previous"),
            ("s", "git status"),
            ("s", "previous"),
            ("s", "other follower"),
            ("s", "previous"),
            ("other", "unrelated"),
            ("other", "git log"),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (session, command))| Run {
            command: command.into(),
            expanded_command: None,
            directory: "/repo".into(),
            started_at: 1_700_000_000.0 + i as f64,
            exit_code: 0,
            session: session.into(),
        })
        .collect();
        let mut index = SuggestionIndex::new(runs, SelectionLog::default());
        index.add_pick("git", "git status", 1_700_000_007.0);
        let context = RankingContext {
            session: Some("s".into()),
            candidates: vec![Candidate::new(
                "other follower".into(),
                CandidateKind::Script,
                "script",
            )],
            ..Default::default()
        };
        for prefix in ["git", ""] {
            let snapshot = index.snapshot(prefix, Some(&context));
            assert_eq!(snapshot.affinity.last.len(), 1);
            assert_eq!(snapshot.previous_command("s").as_deref(), Some("previous"));
            assert_eq!(snapshot.previous_command("other"), None);
            assert_eq!(snapshot.affinity.last_start.len(), 1);
            assert_eq!(snapshot.affinity.followers.len(), 1);
            assert_eq!(snapshot.following("previous"), index.following("previous"));
            assert!(snapshot.commands.contains_key("other follower"));
            assert_eq!(
                snapshot.suggestions_at("/repo", prefix, 20, 1_700_000_008.0, Some(&context)),
                index.suggestions_at("/repo", prefix, 20, 1_700_000_008.0, Some(&context))
            );
        }
        let snapshot = index.snapshot("git", Some(&context));
        assert!(!snapshot.commands.contains_key("unrelated"));
        assert!(
            snapshot
                .directories
                .values()
                .all(|d| !d.contains_key("unrelated"))
        );
    }

    #[test]
    #[ignore]
    fn snapshot_timing() {
        let runs = (0..100_000)
            .map(|i| Run {
                command: format!("cargo test synthetic_{i}"),
                expanded_command: None,
                directory: format!("/synthetic/project_{}", i % 100),
                started_at: 1_700_000_000.0 + f64::from(i),
                exit_code: 0,
                session: format!("session_{}", i % 100),
            })
            .collect();
        let index = SuggestionIndex::new(runs, SelectionLog::default());
        let context = RankingContext {
            session: Some("session_0".into()),
            ..Default::default()
        };
        for (prefix, expected) in [("cargo test synthetic_999", 111), ("", 100_000)] {
            let start = std::time::Instant::now();
            let snapshot = index.snapshot(prefix, Some(&context));
            let elapsed = start.elapsed();
            std::hint::black_box(&snapshot);
            eprintln!(
                "snapshot_timing: 100000 synthetic commands, session_0, prefix {prefix:?}: {elapsed:?}"
            );
            assert_eq!(snapshot.commands.len(), expected);
        }
    }

    #[test]
    fn snapshots_share_kind_cache_and_disabled_tiers_do_not_probe() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let root = crate::context::real_path(temp.path().to_str().unwrap()).unwrap();
        let here = format!("{root}/here");
        let sibling = format!("{root}/sibling");
        std::fs::create_dir_all(&sibling)?;
        std::fs::write(format!("{sibling}/Cargo.toml"), "{}")?;
        let run = Run {
            command: "kt nextest".into(),
            expanded_command: None,
            directory: sibling.clone(),
            started_at: 1_700_000_000.0,
            exit_code: 0,
            session: "s".into(),
        };
        let index = SuggestionIndex::new(vec![run.clone()], SelectionLog::default());
        let on = RankingContext {
            project_kind: Some("rust".into()),
            ..Default::default()
        };
        let first = index.snapshot("kt ", Some(&on)).suggestions_at(
            &here,
            "kt ",
            10,
            run.started_at,
            Some(&on),
        );
        assert_eq!(first[0].why, Some(vec!["used in a rust project".into()]));
        std::fs::remove_dir_all(&sibling)?;
        let second = index.snapshot("kt ", Some(&on)).suggestions_at(
            &here,
            "kt ",
            10,
            run.started_at,
            Some(&on),
        );
        assert_eq!(
            second, first,
            "the second request must use the first snapshot's cached probe"
        );

        for context in [
            RankingContext::default(),
            RankingContext {
                project_kind: Some("rust".into()),
                off: [Signal::Kind].into(),
                ..Default::default()
            },
        ] {
            let cold = SuggestionIndex::new(vec![run.clone()], SelectionLog::default());
            cold.snapshot("kt ", Some(&context)).suggestions_at(
                &here,
                "kt ",
                10,
                run.started_at,
                Some(&context),
            );
            assert!(
                cold.kinds.lock().unwrap().lookup(&sibling).is_none(),
                "kind lookup must be skipped"
            );
        }
        Ok(())
    }

    /// A habit from a sibling project of the same kind outranks an equal one from another kind, the cwd's own
    /// project still wins, and the database scan and the index agree, also with `kind` switched off.
    #[test]
    fn same_kind_tier_agrees_between_scan_and_index() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let root = crate::context::real_path(temp.path().to_str().unwrap()).unwrap();
        for (dir, manifest) in [
            ("here", "Cargo.toml"),
            ("sibling", "Cargo.toml"),
            ("web", "package.json"),
        ] {
            std::fs::create_dir_all(format!("{root}/{dir}"))?;
            std::fs::write(format!("{root}/{dir}/{manifest}"), "{}")?;
        }
        let history = CapturedHistory::new(temp.path().join("history.db"));
        let db = history.open_connection()?;
        let now = 1_700_000_000.0;
        // The other-kind run is fresher than the sibling's, so only the tier can put the sibling's habit ahead of it.
        for (i, (command, directory)) in [
            ("kt gone", "gone"),
            ("kt nextest", "sibling"),
            ("kt own", "here"),
            ("kt newthing", "web"),
        ]
        .into_iter()
        .enumerate()
        {
            for n in 0..3 {
                let directory = format!("{root}/{directory}");
                let mut run = NewRun::new(command, "zsh", &directory, 0);
                run.started_at = Some(now - 1000.0 + (i * 3 + n) as f64);
                run.session = command;
                history.append(&run, "", Some(&db))?;
            }
        }
        let here = format!("{root}/here");
        let project = crate::context::ProjectContext::discover(&here);
        let ranked =
            |off: &str| -> anyhow::Result<(Vec<String>, Vec<String>, Option<Vec<String>>)> {
                let context = RankingContext::new(&project, None, Some(off));
                let scan =
                    history.suggestions(&user_name(), &here, "kt ", 10, now, Some(&context))?;
                let index = SuggestionIndex::new(
                    history.runs(&user_name(), &db)?,
                    history.picks(100, &db)?,
                )
                .suggestions_at(&here, "kt ", 10, now, Some(&context));
                let names = |v: &[crate::history::Suggestion]| {
                    v.iter().map(|s| s.command.clone()).collect::<Vec<_>>()
                };
                Ok((
                    names(&scan),
                    names(&index),
                    scan.iter()
                        .find(|s| s.command == "kt nextest")
                        .and_then(|s| s.why.clone()),
                ))
            };
        let (scan, index, why) = ranked("")?;
        assert_eq!(
            scan[..2],
            ["kt own", "kt nextest"],
            "here > same kind > other kinds"
        );
        assert_eq!(scan, index);
        assert_eq!(why, Some(vec!["used in a rust project".to_string()]));
        let (scan, index, why) = ranked("kind")?;
        assert_eq!(scan[0], "kt own");
        assert_ne!(
            scan[1], "kt nextest",
            "off: the fresher runs win; a gone directory gets no boost"
        );
        assert_eq!(scan, index);
        assert_eq!(why, None);
        Ok(())
    }

    #[test]
    fn same_kind_ancestor_outside_the_project_counts_as_ancestor_only() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let root = crate::context::real_path(temp.path().to_str().unwrap()).unwrap();
        std::fs::create_dir_all(format!("{root}/repo/crate/src"))?;
        std::fs::write(format!("{root}/repo/Cargo.toml"), "{}")?;
        std::fs::write(format!("{root}/repo/crate/Cargo.toml"), "{}")?;
        let history = CapturedHistory::new(temp.path().join("history.db"));
        let db = history.open_connection()?;
        let now = 1_700_000_000.0;
        let parent = format!("{root}/repo");
        let mut run = NewRun::new("kt up", "zsh", &parent, 0);
        run.started_at = Some(now - 10.0);
        history.append(&run, "", Some(&db))?;
        let here = format!("{root}/repo/crate/src");
        let project = crate::context::ProjectContext::discover(&here);
        let context = RankingContext::new(&project, None, None);
        let scan = history.suggestions(&user_name(), &here, "kt ", 10, now, Some(&context))?;
        let index =
            SuggestionIndex::new(history.runs(&user_name(), &db)?, history.picks(100, &db)?)
                .suggestions_at(&here, "kt ", 10, now, Some(&context));
        // The ancestor tier claims the run, so neither path explains it as a same-kind habit.
        assert_eq!(scan[0].why, index[0].why);
        assert_ne!(
            scan[0].why,
            Some(vec!["used in a rust project".to_string()])
        );
        Ok(())
    }

    /// A run directory deleted after the index was built: the scan sees it gone at once; the index keeps the cached
    /// kind (up to the ttl) and drops the boost once the entry expires. The fresher other-kind run wins without it.
    #[test]
    fn deleted_directory_loses_its_boost() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let root = crate::context::real_path(temp.path().to_str().unwrap()).unwrap();
        for (dir, manifest) in [
            ("here", "Cargo.toml"),
            ("sibling", "Cargo.toml"),
            ("web", "package.json"),
        ] {
            std::fs::create_dir_all(format!("{root}/{dir}"))?;
            std::fs::write(format!("{root}/{dir}/{manifest}"), "{}")?;
        }
        let history = CapturedHistory::new(temp.path().join("history.db"));
        let db = history.open_connection()?;
        let now = 1_700_000_000.0;
        for (i, (command, directory)) in [("kt nextest", "sibling"), ("kt newthing", "web")]
            .into_iter()
            .enumerate()
        {
            for n in 0..3 {
                let directory = format!("{root}/{directory}");
                let mut run = NewRun::new(command, "zsh", &directory, 0);
                run.started_at = Some(now - 1000.0 + (i * 3 + n) as f64);
                run.session = command;
                history.append(&run, "", Some(&db))?;
            }
        }
        let here = format!("{root}/here");
        let context =
            RankingContext::new(&crate::context::ProjectContext::discover(&here), None, None);
        let index =
            SuggestionIndex::new(history.runs(&user_name(), &db)?, history.picks(100, &db)?);
        let first = |v: Vec<crate::history::Suggestion>| {
            (
                v[0].command.clone(),
                v.iter()
                    .find(|s| s.command == "kt nextest")
                    .unwrap()
                    .why
                    .clone(),
            )
        };
        let boosted = Some(vec!["used in a rust project".to_string()]);
        assert_eq!(
            first(index.suggestions_at(&here, "kt ", 10, now, Some(&context))),
            ("kt nextest".into(), boosted.clone())
        );

        std::fs::remove_dir_all(format!("{root}/sibling"))?;
        let scan = history.suggestions(&user_name(), &here, "kt ", 10, now, Some(&context))?;
        assert_eq!(
            first(scan),
            ("kt newthing".into(), None),
            "in-process: no boost at once"
        );
        assert_eq!(
            first(index.suggestions_at(&here, "kt ", 10, now, Some(&context))),
            ("kt nextest".into(), boosted),
            "cached kind until it expires"
        );
        index.kinds.lock().unwrap().ttl = std::time::Duration::ZERO;
        assert_eq!(
            first(index.suggestions_at(&here, "kt ", 10, now, Some(&context))),
            ("kt newthing".into(), None),
            "expired: re-probed as gone"
        );
        Ok(())
    }

    #[test]
    fn matches_database_scan() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let history = CapturedHistory::new(temp.path().join("history.db"));
        let db = history.open_connection()?;
        let now = 1_700_000_000.0;
        for (i, (command, directory, exit_code, session)) in [
            ("git status", "/p", 0, "a"),
            ("cargo test", "/p/sub", 1, "a"),
            ("rg needle", "/p/other", 1, "a"),
            ("git status", "/else", 130, "b"),
            ("cargo test", "/p", 0, "a"),
            ("smcli suggest", "/p", 0, "a"),
            ("git diff", "/", 141, "b"),
            ("false", "/p", 1, "a"),
        ]
        .into_iter()
        .enumerate()
        {
            let mut run = NewRun::new(command, "zsh", directory, exit_code);
            run.session = session;
            run.started_at = Some(now - (8 - i) as f64 * USAGE_HALF_LIFE / 2.0);
            history.append(&run, "", Some(&db))?;
        }
        for expanded in ["git log", "git ls-files"] {
            let mut run = NewRun::new("gl", "zsh", "/p", 0);
            run.expanded_command = Some(expanded);
            run.started_at = Some(now - 1.0);
            run.session = expanded;
            history.append(&run, "", Some(&db))?;
        }
        history.record_pick(
            "git",
            "git diff",
            "suggest",
            "/p",
            &CaptureFilter::standard(),
            now,
            &db,
        )?;
        let index =
            SuggestionIndex::new(history.runs(&user_name(), &db)?, history.picks(100, &db)?);
        let context = RankingContext {
            project_root: Some("/p".into()),
            session: Some("a".into()),
            candidates: vec![Candidate {
                command: "cargo check".into(),
                kind: CandidateKind::Script,
                reason: "Cargo.toml project".into(),
                detail: None,
            }],
            ..Default::default()
        };
        let present = |commands: &[String]| vec![true; commands.len()];
        let plain = index.suggestions_at("/p", "", 100, now, None);
        let boosted =
            index.suggestions_with_evidence("/p", "", 100, now, None, Some(&present), None, None);
        for before in plain {
            let after = boosted
                .iter()
                .find(|s| {
                    s.command == before.command && s.expanded_command == before.expanded_command
                })
                .unwrap();
            assert!((after.score - before.score - 0.08).abs() < 1e-10);
        }
        let disabled = RankingContext {
            off: [Signal::Index].into(),
            ..Default::default()
        };
        assert_eq!(
            index.suggestions_with_evidence(
                "/p",
                "",
                100,
                now,
                Some(&disabled),
                Some(&present),
                None,
                None
            ),
            index.suggestions_at("/p", "", 100, now, Some(&disabled))
        );
        let alias_off = RankingContext {
            off: [Signal::Alias].into(),
            ..Default::default()
        };
        let typed_presence = |commands: &[String]| {
            assert_eq!(commands, ["gl", "gl"]);
            vec![false; commands.len()]
        };
        let typed_identifiers = |commands: &[String]| {
            assert_eq!(commands, ["gl", "gl"]);
            vec![(false, false); commands.len()]
        };
        index.suggestions_with_evidence(
            "/p",
            "gl",
            100,
            now,
            Some(&alias_off),
            Some(&typed_presence),
            Some(&typed_identifiers),
            None,
        );
        for directory in ["/p", "/p/", "/p/sub", "/", "/else"] {
            for prefix in ["", "git", "gl", "cargo", "missing"] {
                for context in [None, Some(&context)] {
                    let scan =
                        history.suggestions(&user_name(), directory, prefix, 20, now, context)?;
                    let indexed = index.suggestions_at(directory, prefix, 20, now, context);
                    let snapshot = index.snapshot(prefix, context);
                    assert_eq!(
                        indexed,
                        snapshot.suggestions_at(directory, prefix, 20, now, context)
                    );
                    let idents = |commands: &[String]| {
                        commands
                            .iter()
                            .map(|c| (true, c.contains("test")))
                            .collect()
                    };
                    let git = |commands: &[String]| vec![0.5; commands.len()];
                    assert_eq!(
                        index.suggestions_with_evidence(
                            directory,
                            prefix,
                            20,
                            now,
                            context,
                            Some(&present),
                            Some(&idents),
                            Some(&git)
                        ),
                        snapshot.suggestions_with_evidence(
                            directory,
                            prefix,
                            20,
                            now,
                            context,
                            Some(&present),
                            Some(&idents),
                            Some(&git)
                        )
                    );
                    assert_eq!(indexed.len(), scan.len());
                    for (mut a, b) in indexed.into_iter().zip(scan) {
                        assert!((a.score - b.score).abs() < 1e-10);
                        a.score = b.score;
                        assert_eq!(a, b);
                    }
                }
            }
        }
        assert_eq!(index.previous_command("a").as_deref(), Some("false"));
        assert_eq!(index.following("git status").get("cargo test"), Some(&1));
        Ok(())
    }

    #[test]
    fn rebases_and_keeps_latest_fields() {
        let mut run = Run {
            command: "echo hi".into(),
            expanded_command: None,
            directory: "/p".into(),
            started_at: 0.0,
            exit_code: 0,
            session: "s".into(),
        };
        let mut index = SuggestionIndex::new(vec![run.clone()], SelectionLog::default());
        run.started_at = 501.0 * USAGE_HALF_LIFE;
        assert!(!index.add(&run));
        // The caller rebuilds using the latest run as its base.
        index = SuggestionIndex::new(vec![run.clone()], SelectionLog::default());
        // Older than the last run, from another session (the same session would ask for a rebuild).
        run.started_at -= 1.0;
        run.exit_code = 2;
        run.session = "t".into();
        assert!(index.add(&run));
        index.add_pick("echo", "echo hi", run.started_at);
        assert_eq!(index.picks().picks.len(), 1);
        let result = index.suggestions_at("/p", "", 1, run.started_at + 1.0, None);
        assert_eq!(result[0].runs, 2);
        assert_eq!(result[0].last_exit_code, 0);
        assert!(result[0].score.is_finite());
        assert_eq!(index.recent(), vec!["echo hi"]);
    }

    #[test]
    fn far_future_timestamp_stays_finite() {
        let now = 1_700_000_000.0;
        let mut index = SuggestionIndex::new(vec![], SelectionLog::default());
        let mut run = Run {
            command: "old".into(),
            expanded_command: None,
            directory: "/p".into(),
            started_at: now - USAGE_HALF_LIFE,
            exit_code: 0,
            session: "s".into(),
        };
        index.add(&run);
        run.command = "future".into();
        run.started_at = 1e11;
        index.add(&run);
        let suggestions = index.suggestions_at("/p", "", 10, now, None);
        assert_eq!(suggestions.len(), 2);
        for s in &suggestions {
            assert!(s.score.is_finite(), "score for {} is not finite", s.command);
        }
        // The future run is treated as now, so it outranks the older run.
        assert_eq!(suggestions[0].command, "future");
        assert_eq!(suggestions[1].command, "old");
    }

    #[test]
    fn out_of_order_run_invalidates_index() {
        let mut index = SuggestionIndex::new(vec![], SelectionLog::default());
        let mut run = Run {
            command: "first".into(),
            expanded_command: None,
            directory: "/p".into(),
            started_at: 100.0,
            exit_code: 0,
            session: "s".into(),
        };
        assert!(index.add(&run));
        run.command = "third".into();
        run.started_at = 300.0;
        assert!(index.add(&run));
        run.command = "second".into();
        run.started_at = 200.0;
        assert!(!index.add(&run));

        // A fresh index built from chronologically sorted runs keeps the latest previous command.
        let sorted = vec![
            Run {
                command: "first".into(),
                expanded_command: None,
                directory: "/p".into(),
                started_at: 100.0,
                exit_code: 0,
                session: "s".into(),
            },
            Run {
                command: "second".into(),
                expanded_command: None,
                directory: "/p".into(),
                started_at: 200.0,
                exit_code: 0,
                session: "s".into(),
            },
            Run {
                command: "third".into(),
                expanded_command: None,
                directory: "/p".into(),
                started_at: 300.0,
                exit_code: 0,
                session: "s".into(),
            },
        ];
        let fresh = SuggestionIndex::new(sorted, SelectionLog::default());
        assert_eq!(fresh.previous_command("s").as_deref(), Some("third"));
    }

    #[test]
    fn defined_test_outranks_an_equal_habit_and_a_missed_budget_does_not() {
        let now = 1_700_000_000.0;
        let run = |command: &str, at: f64| Run {
            command: command.into(),
            expanded_command: None,
            directory: "/p".into(),
            started_at: at,
            exit_code: 0,
            session: "s".into(),
        };
        let index = SuggestionIndex::new(
            vec![
                run("cargo test dedup_keeps_latest", now - 10.0),
                run("cargo test old_removed_case", now - 1.0),
            ],
            SelectionLog::default(),
        );
        let defined = |commands: &[String]| {
            commands
                .iter()
                .map(|c| {
                    if c.contains("dedup_keeps_latest") {
                        (true, true)
                    } else {
                        (false, false)
                    }
                })
                .collect()
        };
        let plain =
            index.suggestions_with_evidence("/p", "cargo test", 10, now, None, None, None, None);
        let boosted = index.suggestions_with_evidence(
            "/p",
            "cargo test",
            10,
            now,
            None,
            None,
            Some(&defined),
            None,
        );
        assert_eq!(plain[0].command, "cargo test old_removed_case");
        assert_eq!(boosted[0].command, "cargo test dedup_keeps_latest");
        assert!(
            boosted[0]
                .why
                .as_ref()
                .unwrap()
                .iter()
                .any(|w| w == "defined in project")
        );
        let timed_out = |_: &[String]| Vec::<(bool, bool)>::new();
        assert_eq!(
            index.suggestions_with_evidence(
                "/p",
                "cargo test",
                10,
                now,
                None,
                None,
                Some(&timed_out),
                None
            ),
            plain
        );
        let off = RankingContext {
            off: [Signal::Evidence].into(),
            ..Default::default()
        };
        let boom = |_: &[String]| -> Vec<(bool, bool)> { panic!("evidence off") };
        assert_eq!(
            index.suggestions_with_evidence(
                "/p",
                "cargo test",
                10,
                now,
                Some(&off),
                None,
                Some(&boom),
                None
            ),
            index.suggestions_with_evidence(
                "/p",
                "cargo test",
                10,
                now,
                Some(&off),
                None,
                None,
                None
            )
        );
    }

    #[test]
    fn test_candidate_carries_project_test_label() {
        let index = SuggestionIndex::new(vec![], SelectionLog::default());
        let context = RankingContext {
            project_root: Some("/p".into()),
            candidates: vec![Candidate {
                command: "cargo test my_test".into(),
                kind: CandidateKind::Test,
                reason: "project test".into(),
                detail: None,
            }],
            ..Default::default()
        };
        let suggestions = index.suggestions_at("/p", "cargo test", 10, 0.0, Some(&context));
        let test = suggestions
            .iter()
            .find(|s| s.command == "cargo test my_test")
            .unwrap();
        assert!(
            test.why
                .as_ref()
                .unwrap()
                .iter()
                .any(|w| w == "project test"),
            "{test:?}"
        );
    }
}
