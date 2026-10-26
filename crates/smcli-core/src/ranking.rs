use std::collections::{HashMap, HashSet};

use crate::context::{NODE_MANAGERS, Project, ProjectContext};
use crate::history::USAGE_HALF_LIFE;

/// The signals that can be switched off, for measuring each one (`SMCLI_RANKING_OFF=project,transition`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Signal {
    Project,
    Transition,
    Selection,
    Script,
    Tests,
    Git,
    Hint,
    Path,
    Files,
    Index,
    Expand,
    Evidence,
    Alias,
}

impl Signal {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "project" => Self::Project,
            "transition" => Self::Transition,
            "selection" => Self::Selection,
            "script" => Self::Script,
            "tests" => Self::Tests,
            "git" => Self::Git,
            "hint" => Self::Hint,
            "path" => Self::Path,
            "files" => Self::Files,
            "index" => Self::Index,
            "expand" => Self::Expand,
            "evidence" => Self::Evidence,
            "alias" => Self::Alias,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    Script,
    Test,
    Hint,
}

/// A command the project suggests whether or not it was ever run.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub command: String,
    pub kind: CandidateKind,
    /// Shown in `why`: `npm script`, `rebase in progress`.
    pub reason: String,
}

impl Candidate {
    pub fn new(command: String, kind: CandidateKind, reason: &str) -> Self {
        Self { command, kind, reason: reason.into() }
    }
}

/// Ranking evidence beyond a command's own runs: where the query comes from (project, session) and what the
/// project itself offers (scripts, Git state). Built per query from the directory context, never stored.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RankingContext {
    /// The nearest manifest's directory: runs below it count as the same project.
    pub project_root: Option<String>,
    /// The shell session asking, whose previous command selects transitions.
    pub session: Option<String>,
    pub candidates: Vec<Candidate>,
    pub off: HashSet<Signal>,
}

impl RankingContext {
    /// From directory context; `off` names come from the environment.
    pub fn new(project: &ProjectContext, session: Option<String>, environment: &HashMap<String, String>) -> Self {
        Self {
            project_root: project.projects.first().map(|p| p.root.clone()),
            session,
            candidates: candidates(project),
            off: Self::disabled(environment),
        }
    }

    pub fn disabled(environment: &HashMap<String, String>) -> HashSet<Signal> {
        environment.get("SMCLI_RANKING_OFF").map_or("", |s| s).split(',').filter_map(Signal::parse).collect()
    }

    pub fn uses(&self, signal: Signal) -> bool {
        !self.off.contains(&signal)
    }

    pub fn in_project(&self, directory: &str) -> bool {
        let Some(root) = self.project_root.as_deref().filter(|_| self.uses(Signal::Project)) else { return false };
        directory == root || (root != "/" && directory.starts_with(&format!("{root}/")))
    }

    pub fn candidates_of(&self, kind: CandidateKind) -> Vec<&Candidate> {
        if !self.uses(match kind { CandidateKind::Script => Signal::Script, CandidateKind::Test => Signal::Tests, CandidateKind::Hint => Signal::Hint }) {
            return vec![];
        }
        self.candidates.iter().filter(|c| c.kind == kind).collect()
    }
}

/// `context.map_or(true, |c| c.uses(signal))`: no context means every signal is on.
pub fn uses(context: Option<&RankingContext>, signal: Signal) -> bool {
    context.is_none_or(|c| c.uses(signal))
}

/// Commands the directory's project and Git state point to. Only names present in a manifest are produced
/// (read as text by `discover`; nothing is executed), and only plain identifiers, so a hostile `package.json`
/// cannot put shell syntax on someone's command line.
pub fn candidates(context: &ProjectContext) -> Vec<Candidate> {
    use CandidateKind::{Hint, Script};
    let mut found = vec![];
    if let Some(operation) = context.repository.as_ref().and_then(|r| r.operation.as_deref()) {
        let verbs: Vec<String> = match operation {
            "rebase" | "cherry-pick" | "revert" | "am" => vec!["--continue", "--abort", "--skip"],
            "merge" => vec!["--continue", "--abort"],
            "bisect" => vec!["good", "bad", "reset"],
            _ => vec![],
        }
        .into_iter()
        .map(String::from)
        .collect();
        for verb in verbs {
            found.push(Candidate::new(format!("git {operation} {verb}"), Hint, &format!("{operation} in progress")));
        }
    }
    let Some(project) = context.projects.first() else { return found };
    let valid = |name: &str| {
        let Some(first) = name.chars().next() else { return false };
        (first.is_alphabetic() || first.is_ascii_digit() || first == '_')
            && name.chars().all(|c| c.is_ascii() && (c.is_alphabetic() || c.is_ascii_digit() || "_.:@/-".contains(c)))
    };
    let manager = project.package_manager.as_deref().unwrap_or("");
    for (file, names) in &project.scripts {
        let lower = file.to_lowercase();
        for name in names.iter().filter(|n| valid(n)) {
            match lower.as_str() {
                "package.json" => {
                    // Checked again here: a cached or hand-built context may carry any text.
                    let manager = if NODE_MANAGERS.contains(&manager) { manager } else { "" };
                    let command = if manager == "yarn" {
                        format!("yarn {name}")
                    } else {
                        format!("{} run {name}", if manager.is_empty() { "npm" } else { manager })
                    };
                    found.push(Candidate::new(command, Script, "package.json script"));
                    if (manager.is_empty() || manager == "npm") && (name == "test" || name == "start") {
                        found.push(Candidate::new(format!("npm {name}"), Script, "package.json script"));
                    }
                }
                "makefile" | "gnumakefile" => found.push(Candidate::new(format!("make {name}"), Script, "Makefile target")),
                "cargo.toml" => found.push(Candidate::new(format!("cargo run --bin {name}"), Script, "Cargo binary")),
                "package.swift" if !name.ends_with("Tests") => {
                    found.push(Candidate::new(format!("swift build --target {name}"), Script, "Package.swift target"))
                }
                "pyproject.toml" if ["uv", "poetry", "pdm", "pipenv"].contains(&manager) => {
                    found.push(Candidate::new(format!("{manager} run {name}"), Script, "pyproject script"))
                }
                "deno.json" | "deno.jsonc" => found.push(Candidate::new(format!("deno task {name}"), Script, "deno task")),
                "composer.json" => found.push(Candidate::new(format!("composer run {name}"), Script, "composer script")),
                "justfile" | ".justfile" => found.push(Candidate::new(format!("just {name}"), Script, "justfile recipe")),
                "taskfile.yml" | "taskfile.yaml" => found.push(Candidate::new(format!("task {name}"), Script, "Taskfile task")),
                "rakefile" => {
                    let rake = if project.manifests.iter().any(|m| m == "Gemfile") { "bundle exec rake" } else { "rake" };
                    found.push(Candidate::new(format!("{rake} {name}"), Script, "Rakefile task"));
                }
                _ => {}
            }
        }
    }
    for file in &project.manifests {
        for command in standard_commands(&file.to_lowercase(), project) {
            found.push(Candidate::new(command, Script, &format!("{file} project")));
        }
    }
    found
}

/// The usual commands for a manifest, whatever it declares. Fixed text: only the tool varies, and only
/// between known names.
fn standard_commands(file: &str, project: &Project) -> Vec<String> {
    let manager = project.package_manager.as_deref().unwrap_or("");
    let has = |name: &str| project.manifests.iter().any(|m| m == name);
    let list = |items: &[&str]| items.iter().map(|s| s.to_string()).collect();
    match file {
        "package.swift" => list(&["swift build", "swift test", "swift run"]),
        "package.json" => vec![format!("{} install", if NODE_MANAGERS.contains(&manager) { manager } else { "npm" })],
        "deno.json" | "deno.jsonc" => list(&["deno test", "deno lint", "deno fmt"]),
        "cargo.toml" => list(&["cargo build", "cargo test", "cargo run", "cargo check", "cargo clippy", "cargo fmt"]),
        "pyproject.toml" => match manager {
            "uv" => list(&["uv sync", "uv run pytest"]),
            "poetry" => list(&["poetry install", "poetry run pytest"]),
            _ => list(&["pytest"]),
        },
        "go.mod" => list(&["go build ./...", "go test ./...", "go run .", "go vet ./...", "go mod tidy"]),
        "gemfile" => list(&["bundle install", "bundle exec rspec"]),
        "composer.json" => list(&["composer install", "composer update"]),
        "pom.xml" => {
            let mvn = if has("mvnw") { "./mvnw" } else { "mvn" };
            vec![format!("{mvn} clean install"), format!("{mvn} test"), format!("{mvn} package")]
        }
        "build.gradle.kts" | "build.gradle" => {
            let gradle = if has("gradlew") { "./gradlew" } else { "gradle" };
            vec![format!("{gradle} build"), format!("{gradle} test"), format!("{gradle} run")]
        }
        "mix.exs" => list(&["mix deps.get", "mix compile", "mix test", "iex -S mix"]),
        "pubspec.yaml" => list(&["dart pub get", "dart test", "dart run"]),
        "cmakelists.txt" => list(&["cmake -B build", "cmake --build build"]),
        "compose.yaml" | "compose.yml" | "docker-compose.yml" | "docker-compose.yaml" => list(&[
            "docker compose up", "docker compose up -d", "docker compose down", "docker compose build", "docker compose logs -f",
        ]),
        "dockerfile" => list(&["docker build ."]),
        _ => vec![],
    }
}

/// One pick: what the user typed when they chose a command.
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub query: String,
    pub command: String,
    pub time: f64,
}

/// What the user typed when they picked a command, kept to boost that command for similar queries.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SelectionLog {
    pub picks: Vec<Pick>,
}

impl SelectionLog {
    pub fn add(&mut self, query: &str, command: &str, time: f64) {
        self.picks.push(Pick { query: Self::normalize(query), command: command.into(), time });
    }

    pub fn normalize(query: &str) -> String {
        query.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// 1 for the same query, 0.6 when the current one is the start of the picked one (`doc` while typing toward
    /// `docker`), 0.25 when it extends the picked one (a more specific search than the pick answered), and half the
    /// word overlap when several words are shared (`start containers` and `start the containers`).
    /// An empty query only matches an empty one.
    pub fn similarity(a: &str, b: &str) -> f64 {
        if a == b {
            return 1.0;
        }
        if a.is_empty() || b.is_empty() {
            return 0.0;
        }
        // `a` is the current query, `b` the one the pick was made for.
        if a.chars().count() >= 2 && b.starts_with(a) {
            return 0.6;
        }
        if b.chars().count() >= 2 && a.starts_with(b) {
            return 0.25;
        }
        let left: HashSet<&str> = a.split(' ').collect();
        let right: HashSet<&str> = b.split(' ').collect();
        if left.len() <= 1 || right.len() <= 1 {
            return 0.0;
        }
        let overlap = left.intersection(&right).count() as f64 / left.union(&right).count() as f64;
        if overlap >= 0.5 { 0.5 * overlap } else { 0.0 }
    }

    /// Per command: picks for similar queries, each weighing 0.5^(age / half-life) × similarity.
    pub fn weights(&self, query: &str, now: f64) -> HashMap<String, f64> {
        let query = Self::normalize(query);
        let mut weights = HashMap::new();
        for pick in &self.picks {
            let similarity = Self::similarity(&query, &pick.query);
            if similarity > 0.0 {
                *weights.entry(pick.command.clone()).or_insert(0.0) +=
                    similarity * 0.5f64.powf((now - pick.time).max(0.0) / USAGE_HALF_LIFE);
            }
        }
        weights
    }
}

/// Applies the context signals to base scores (usage, recency, directory, project) and returns why each moved
/// (only the commands a signal touched).
///
/// - A command that often follows the session's previous one is multiplied by up to 7 (its share of that
///   command's followers, discounted until it has been seen three times).
/// - A command picked for a similar query is multiplied by up to 5 (each recent pick of the same query counts 2).
/// - A project script already in history is multiplied by 1.5; one that never ran enters at 0.3, below any habit.
/// - A Git-state hint is lifted just above the best score, so the way out of a rebase is the first suggestion.
pub fn apply_ranking_signals(
    scores: &mut HashMap<String, f64>,
    prefix: &str,
    context: Option<&RankingContext>,
    previous: Option<&str>,
    followers: &HashMap<String, i64>,
    picks: &HashMap<String, f64>,
) -> HashMap<String, Vec<String>> {
    let mut why: HashMap<String, Vec<String>> = HashMap::new();
    let empty = vec![];
    let scripts = context.map_or(empty.clone(), |c| c.candidates_of(CandidateKind::Script));
    for script in scripts.iter().filter(|s| has_prefix(&s.command, prefix)) {
        scores.entry(script.command.clone()).and_modify(|s| *s *= 1.5).or_insert(0.3);
        why.entry(script.command.clone()).or_default().push(script.reason.clone());
    }
    let tests = context.map_or(empty.clone(), |c| c.candidates_of(CandidateKind::Test));
    for test in tests.iter().filter(|t| has_prefix(&t.command, prefix)) {
        scores.entry(test.command.clone()).and_modify(|s| *s *= 1.5).or_insert(0.3);
        why.entry(test.command.clone()).or_default().push(test.reason.clone());
    }
    if let Some(previous) = previous.filter(|_| uses(context, Signal::Transition) && !followers.is_empty()) {
        let total = followers.values().sum::<i64>() as f64;
        for (command, count) in followers {
            if let Some(score) = scores.get_mut(command) {
                let share = *count as f64 / total;
                *score *= 1.0 + 6.0 * share * (*count as f64 / 3.0).min(1.0);
                why.entry(command.clone()).or_default().push(format!("often follows {}", crate::entry::command_parts(previous).0));
            }
        }
    }
    if uses(context, Signal::Selection) {
        for (command, weight) in picks {
            if let Some(score) = scores.get_mut(command) {
                *score *= 1.0 + (2.0 * weight).min(4.0);
                why.entry(command.clone()).or_default().push("picked for similar searches".into());
            }
        }
    }
    let hints: Vec<&Candidate> =
        context.map_or(empty, |c| c.candidates_of(CandidateKind::Hint)).into_iter().filter(|h| has_prefix(&h.command, prefix)).collect();
    if !hints.is_empty() {
        let best = scores
            .iter()
            .filter(|(command, _)| !hints.iter().any(|h| &h.command == *command))
            .map(|(_, s)| *s)
            .fold(0.0, f64::max)
            .max(1.0);
        for (index, hint) in hints.iter().enumerate() {
            let lifted = best * (1.1 - 0.01 * index as f64);
            let score = scores.entry(hint.command.clone()).or_insert(0.0);
            *score = score.max(lifted);
            why.entry(hint.command.clone()).or_default().push(hint.reason.clone());
        }
    }
    why
}

/// SQLite's `substr(command, 1, length(prefix)) = prefix` compares code points; so does `starts_with`.
pub fn has_prefix(command: &str, prefix: &str) -> bool {
    command.starts_with(prefix)
}

/// The `why` of a suggestion: where it was used, then what the context signals found.
pub fn suggestion_why(runs_here: i64, project_runs: i64, extra: Option<&Vec<String>>) -> Option<Vec<String>> {
    let mut lines = vec![];
    if runs_here > 0 {
        lines.push("used here".to_string());
    } else if project_runs > 0 {
        lines.push("used in this project".to_string());
    }
    lines.extend(extra.into_iter().flatten().cloned());
    (!lines.is_empty()).then_some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_pick_similarity() {
        assert_eq!(SelectionLog::similarity("docker", "docker"), 1.0);
        assert_eq!(SelectionLog::similarity("doc", "docker"), 0.6);
        assert_eq!(SelectionLog::similarity("docker ps", "docker"), 0.25);
        assert_eq!(SelectionLog::similarity("start containers", "start the containers"), 0.5 * 2.0 / 3.0);
        assert_eq!(SelectionLog::similarity("", "x"), 0.0);
    }

    #[test]
    fn evidence_switch_parses() {
        let off = RankingContext::disabled(&HashMap::from([("SMCLI_RANKING_OFF".into(), "evidence".into())]));
        assert!(off.contains(&Signal::Evidence));
        assert!(!RankingContext { off, ..Default::default() }.uses(Signal::Evidence));
    }
}
