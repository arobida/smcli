use std::collections::HashMap;

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
        if self.last_start.get(session).is_some_and(|&t| started_at < t) {
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

#[derive(Debug)]
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
#[derive(Debug, Default)]
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
    /// Reference time for stored weights; moved forward before 2^((t − base)/h) could overflow.
    base: f64,
    commands: HashMap<String, Tally>,
    directories: HashMap<String, HashMap<String, DirectoryTally>>,
    answering: HashMap<String, bool>,
    history: Vec<Run>,
    affinity: CommandAffinity,
    picks: SelectionLog,
}

impl SuggestionIndex {
    pub fn new(runs: Vec<Run>, picks: SelectionLog) -> Self {
        let mut index = Self {
            base: runs.last().map_or_else(now, |r| r.started_at),
            commands: HashMap::new(),
            directories: HashMap::new(),
            answering: HashMap::new(),
            history: vec![],
            affinity: CommandAffinity::new(),
            picks,
        };
        for run in runs {
            index.add(&run);
        }
        index
    }

    /// Remembers that `command` was picked for `query`, as `record_pick` stored it.
    pub fn add_pick(&mut self, query: &str, command: &str, time: f64) {
        self.picks.add(query, command, time);
    }

    /// Adds one run. Runs are expected in (started_at, id) order; an older one still counts, but doesn't
    /// replace the last-run fields. Returns false when the run is older than its session's last run,
    /// signalling that the index should be rebuilt from the database.
    pub fn add(&mut self, run: &Run) -> bool {
        if is_smcli_command(&run.command) {
            return true;
        }
        let started_at = run.started_at.min(now());
        if !self.affinity.add(&run.session, &run.command, started_at) {
            return false;
        }
        let mut run = run.clone();
        run.started_at = started_at;
        self.history.push(run.clone());
        if (started_at - self.base) / USAGE_HALF_LIFE > 500.0 {
            self.base = started_at;
            self.commands.clear();
            self.directories.clear();
            for old in self.history.clone() {
                self.tally(&old);
            }
        } else {
            self.tally(&run);
        }
        true
    }

    fn tally(&mut self, run: &Run) {
        let mut weight = 2f64.powf((run.started_at - self.base) / USAGE_HALF_LIFE);
        if run.exit_code != 0 && !INTERRUPTED_EXIT_CODES.contains(&run.exit_code) {
            let answers = *self
                .answering
                .entry(run.command.clone())
                .or_insert_with(|| answers_with_exit_code(&run.command));
            if !(run.exit_code == 1 && answers) {
                weight *= 0.25;
            }
        }
        let tally = self.commands.entry(run.command.clone()).or_default();
        tally.score += weight;
        tally.runs += 1;
        if run.started_at >= tally.last_run {
            tally.last_run = run.started_at;
            tally.last_exit_code = run.exit_code;
        }
        let directory = self
            .directories
            .entry(run.directory.clone())
            .or_default()
            .entry(run.command.clone())
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
        let here = if directory.len() > 1 {
            directory.strip_suffix('/').unwrap_or(directory)
        } else {
            directory
        };
        let scale = 2f64.powf((self.base - now) / USAGE_HALF_LIFE);
        let mut scores: HashMap<String, f64> = self
            .commands
            .iter()
            .filter(|(c, _)| has_prefix(c, prefix))
            .map(|(c, t)| (c.clone(), t.score))
            .collect();
        let mut project_runs: HashMap<String, i64> = HashMap::new();
        for (directory, tallies) in &self.directories {
            let in_project = context.is_some_and(|c| c.in_project(directory));
            let bonus = if directory == here {
                3.0
            } else if in_project {
                2.0
            } else if here != "/"
                && directory != "/"
                && (directory.starts_with(&format!("{here}/"))
                    || here.starts_with(&format!("{directory}/")))
            {
                1.0
            } else {
                continue;
            };
            for (command, tally) in tallies {
                if let Some(score) = scores.get_mut(command) {
                    *score += bonus * tally.score;
                    if in_project {
                        *project_runs.entry(command.clone()).or_default() += tally.runs;
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
        let why = apply_ranking_signals(
            &mut scores,
            prefix,
            context,
            previous.map(String::as_str),
            &followers,
            &self.picks.weights(prefix, now),
        );
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
                    why.get(&command),
                );
                Suggestion {
                    score,
                    runs: tally.map_or(0, |t| t.runs),
                    runs_here,
                    last_run: tally.map_or_else(String::new, |t| format_iso8601(t.last_run, false)),
                    last_exit_code: tally.map_or(0, |t| t.last_exit_code),
                    why: explanation,
                    command,
                }
            })
            .collect()
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
            }],
            ..Default::default()
        };
        for directory in ["/p", "/p/", "/p/sub", "/", "/else"] {
            for prefix in ["", "git", "cargo", "missing"] {
                for context in [None, Some(&context)] {
                    let scan =
                        history.suggestions(&user_name(), directory, prefix, 20, now, context)?;
                    let indexed = index.suggestions_at(directory, prefix, 20, now, context);
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
        let mut index = SuggestionIndex::new(vec![], SelectionLog::default());
        let mut run = Run {
            command: "echo hi".into(),
            directory: "/p".into(),
            started_at: 0.0,
            exit_code: 0,
            session: "s".into(),
        };
        index.add(&run);
        run.started_at = 1000.0 * USAGE_HALF_LIFE;
        index.add(&run);
        // Older than the last run, from another session (the same session would ask for a rebuild).
        run.started_at -= 1.0;
        run.exit_code = 2;
        run.session = "t".into();
        assert!(index.add(&run));
        index.add_pick("echo", "echo hi", run.started_at);
        assert_eq!(index.picks().picks.len(), 1);
        let result = index.suggestions_at("/p", "", 1, run.started_at + 1.0, None);
        assert_eq!(result[0].runs, 3);
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
            Run { command: "first".into(), directory: "/p".into(), started_at: 100.0, exit_code: 0, session: "s".into() },
            Run { command: "second".into(), directory: "/p".into(), started_at: 200.0, exit_code: 0, session: "s".into() },
            Run { command: "third".into(), directory: "/p".into(), started_at: 300.0, exit_code: 0, session: "s".into() },
        ];
        let fresh = SuggestionIndex::new(sorted, SelectionLog::default());
        assert_eq!(fresh.previous_command("s").as_deref(), Some("third"));
    }
}
