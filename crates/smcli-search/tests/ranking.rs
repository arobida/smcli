//! Port of `RankingQualityTests.swift`: a fixed query set over a deterministic synthetic history, scored by mean
//! reciprocal rank and top-1/top-3. `make rs-ranking` prints the scores; the test fails below the floors.
//! The fixture never touches real history: it builds real directories (manifests, `.git` markers) under a
//! temporary root, since project and Git context are read from disk.

use std::collections::{BTreeMap, HashMap};

use rusqlite::Connection;
use smcli_core::config::CaptureFilter;
use smcli_core::context::{ProjectContext, real_path};
use smcli_core::entry::SearchFilter;
use smcli_core::history::{CapturedHistory, NewRun, user_name};
use smcli_core::ranking::RankingContext;
use smcli_search::corpus::search_corpus;
use smcli_search::setup::search_signals;
use smcli_search::{HistorySearch, Potion};

const NOW: f64 = 1_800_000_000.0;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Kind {
    Suggest,
    Search,
}

struct Case {
    group: &'static str,
    kind: Kind,
    directory: &'static str,
    query: &'static str,
    expected: &'static str,
    session: Option<&'static str>,
}

const fn case(group: &'static str, kind: Kind, directory: &'static str, query: &'static str, expected: &'static str) -> Case {
    Case { group, kind, directory, query, expected, session: None }
}

const fn in_session(c: Case, session: &'static str) -> Case {
    Case { session: Some(session), ..c }
}

use Kind::{Search, Suggest};

const CASES: [Case; 27] = [
    // Directory: the command used where you are.
    case("directory", Suggest, "webapp", "npm run ", "npm run dev"),
    case("directory", Suggest, "api/Sources/Lib", "swift b", "swift build"),
    case("directory", Suggest, "tools", "make r", "make release"),
    case("directory", Search, "api/Sources/Lib", "build", "swift build"),
    case("directory", Search, "webapp", "npm test", "npm test"),
    // Project: commands from sibling directories of the same package outrank unrelated, more frequent ones.
    case("project", Suggest, "api/Sources/App", "swift t", "swift test --filter Lib"),
    case("project", Suggest, "mono/packages/ui/src", "npm run ", "npm run docs"),
    case("project", Search, "api/Sources/App", "swift test", "swift test --filter Lib"),
    case("project", Search, "mono/packages/ui/src", "npm run", "npm run docs"),
    // Transitions: what usually comes after the command you just ran.
    in_session(case("transition", Suggest, "notes", "git ", "git commit -m wip"), "live1"),
    in_session(case("transition", Suggest, "notes", "make ", "make lint"), "live2"),
    in_session(case("transition", Search, "notes", "git commit", "git commit -m wip"), "live1"),
    // Selections: what you picked for a similar query before.
    case("selection", Suggest, "other", "docker", "docker compose up -d"),
    case("selection", Suggest, "other", "git l", "git log --oneline -20"),
    case("selection", Search, "other", "start containers", "docker compose up -d"),
    // Project scripts and targets that exist in a manifest but were never run.
    case("scripts", Suggest, "webapp", "npm run sto", "npm run storybook"),
    case("scripts", Suggest, "tools", "make doc", "make docs"),
    case("scripts", Search, "webapp", "storybook", "npm run storybook"),
    case("scripts", Search, "tools", "docs", "make docs"),
    // Guards: context signals must not disturb queries they have nothing to say about.
    in_session(case("guard", Suggest, "notes", "ls", "ls -la"), "live1"),
    case("guard", Suggest, "rebasing", "git s", "git status"),
    case("guard", Suggest, "webapp", "npm run b", "npm run build"),
    case("guard", Search, "rebasing", "show running containers", "docker ps"),
    // Git state: the way out of an operation in progress.
    case("git state", Suggest, "rebasing", "git ", "git rebase --continue"),
    case("git state", Suggest, "merging", "git m", "git merge --continue"),
    case("git state", Search, "rebasing", "continue", "git rebase --continue"),
    case("git state", Search, "merging", "continue", "git merge --continue"),
];

/// Picks the user made: what was typed, what was chosen, how many days ago.
const SELECTIONS: [(&str, &str, f64); 7] = [
    ("docker", "docker compose up -d", 1.0), ("docker", "docker compose up -d", 2.0), ("docker", "docker compose up -d", 4.0),
    ("git l", "git log --oneline -20", 1.0), ("git l", "git log --oneline -20", 3.0),
    ("start containers", "docker compose up -d", 2.0), ("start containers", "docker compose up -d", 5.0),
];

struct World {
    _base: tempfile::TempDir,
    root: String,
    log: CapturedHistory,
    db: Connection,
    sequence: u32,
    engine: HistorySearch,
}

impl World {
    fn new() -> Self {
        let base = tempfile::tempdir().unwrap();
        let root = real_path(base.path().to_str().unwrap()).unwrap();
        let log = CapturedHistory::new(format!("{root}/history.db"));
        let db = log.open_connection().unwrap();
        let engine = HistorySearch::new(format!("{root}/index"), Potion::bundled().unwrap());
        let mut world = Self { _base: base, root, log, db, sequence: 0, engine };
        world.layout();
        world.history();
        for (query, command, days) in SELECTIONS {
            world.log.record_pick(query, command, "suggest", &world.path("notes"), &CaptureFilter::standard(),
                                  NOW - days * 86_400.0, &world.db).unwrap();
        }
        world
    }

    fn path(&self, relative: &str) -> String {
        if relative.is_empty() { self.root.clone() } else { format!("{}/{relative}", self.root) }
    }

    fn write(&self, text: &str, file: &str) {
        let path = self.path(file);
        std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn repository(&self, directory: &str, marker: Option<&str>) {
        self.write("ref: refs/heads/main\n", &format!("{directory}/.git/HEAD"));
        if let Some(marker) = marker {
            self.write("x\n", &format!("{directory}/.git/{marker}"));
        }
    }

    /// The directories the history and queries refer to.
    fn layout(&self) {
        self.repository("webapp", None);
        self.write(r#"{"name":"webapp","scripts":{"dev":"vite","build":"vite build","test":"vitest","lint":"eslint .","storybook":"storybook dev"}}"#,
                   "webapp/package.json");
        self.repository("mono", None);
        self.write(r#"{"name":"mono","scripts":{"dev":"turbo dev"}}"#, "mono/package.json");
        self.write(r#"{"name":"ui","scripts":{"docs":"typedoc","test":"vitest"}}"#, "mono/packages/ui/package.json");
        self.write(r#"{"name":"core","scripts":{"bundle":"rollup -c"}}"#, "mono/packages/core/package.json");
        self.repository("api", None);
        self.write("// swift-tools-version: 6.0\nimport PackageDescription\nlet package = Package(name: \"api\", targets: [\n    \
                    .executableTarget(name: \"App\"), .target(name: \"Lib\"), .testTarget(name: \"AppTests\"),\n])",
                   "api/Package.swift");
        self.write(".PHONY: lint fmt release docs\nlint:\n\techo lint\nfmt:\n\techo fmt\nrelease:\n\techo release\ndocs:\n\techo docs\n",
                   "tools/Makefile");
        self.repository("rebasing", Some("rebase-merge/done"));
        self.repository("merging", Some("MERGE_HEAD"));
        for directory in ["notes", "other", "api/Sources/App", "api/Sources/Lib", "mono/packages/ui/src", "mono/packages/ui/stories"] {
            std::fs::create_dir_all(self.path(directory)).unwrap();
        }
    }

    fn run(&mut self, command: &str, directory: &str, at: f64, session: &str, exit: i64) {
        self.sequence += 1;
        // A distinct sub-millisecond offset per run keeps equal times from deciding rankings by insertion order.
        let directory = self.path(directory);
        let mut run = NewRun::new(command, "zsh", &directory, exit);
        run.session = session;
        run.started_at = Some(at + f64::from(self.sequence) * 1e-4);
        self.log.append(&run, "", Some(&self.db)).unwrap();
    }

    /// `count` runs spread evenly from `from` to `from + span` days before `NOW`, each in its own session so
    /// that habits create no command transitions of their own.
    fn habit_with(&mut self, command: &str, directory: &str, count: u32, from: f64, span: f64) {
        for index in 0..count {
            let age = from + span * f64::from(index) / f64::from(count.saturating_sub(1).max(1));
            let session = format!("x{}", self.sequence);
            self.run(command, directory, NOW - age * 86_400.0, &session, 0);
        }
    }

    fn habit(&mut self, command: &str, directory: &str, count: u32) {
        self.habit_with(command, directory, count, 0.5, 10.0);
    }

    /// `first` followed by `second` in one session, `count` times.
    fn pair(&mut self, first: &str, second: &str, directory: &str, count: u32) {
        for index in 0..count {
            let at = NOW - (0.5 + 10.0 * f64::from(index) / f64::from(count.saturating_sub(1).max(1))) * 86_400.0;
            let session = format!("p{}", self.sequence);
            self.run(first, directory, at, &session, 0);
            self.run(second, directory, at + 5.0, &session, 0);
        }
    }

    fn history(&mut self) {
        // Directory habits.
        self.habit("npm run dev", "webapp", 14);
        self.habit("npm run build", "webapp", 8);
        self.habit("npm test", "webapp", 12);
        self.habit("git status", "webapp", 50);
        self.habit("git diff", "webapp", 20);
        self.habit("swift build", "api/Sources/Lib", 50);
        self.habit("swift test --filter Lib", "api/Sources/Lib", 12);
        self.habit("swift test --parallel", "tools", 16);
        self.habit("swift build -c release", "tools", 15);
        self.habit("make release", "other", 25);
        self.habit("ls -la", "notes", 30);
        // Monorepo: root, the package being worked in, and a sibling package.
        self.habit("npm run dev", "mono", 14);
        self.habit_with("npm run docs", "mono/packages/ui/stories", 12, 0.5, 8.0);
        self.habit("npm run bundle", "mono/packages/core", 20);
        // Transitions: what follows what, within a session.
        self.pair("git add -A", "git commit -m wip", "other", 14);
        self.pair("git add -A", "git status", "other", 2);
        self.pair("git add -A", "git diff --staged", "other", 1);
        self.habit("git commit --amend", "other", 30);
        self.pair("make fmt", "make lint", "tools", 10);
        // Picks: the history behind what the user chose.
        self.habit("docker ps", "notes", 30);
        self.habit("docker logs -f web", "notes", 10);
        self.habit("docker compose up -d", "notes", 6);
        self.habit("git ls-files", "other", 20);
        self.habit("git log --oneline -20", "other", 8);
        // Git state: finished operations, and the habits a repository in the middle of one competes with.
        self.habit("git rebase -i HEAD~3", "other", 3);
        self.habit_with("git rebase --continue", "other", 2, 20.0, 5.0);
        self.habit("git merge main", "other", 9);
        self.habit("git mergetool", "other", 2);
        self.habit_with("git merge --abort", "other", 1, 25.0, 1.0);
        self.habit("git cherry-pick --continue", "other", 4);
        // The commands each live session just ran.
        self.run("git add -A", "notes", NOW - 30.0, "live1", 0);
        self.run("make fmt", "notes", NOW - 40.0, "live2", 0);
    }

    /// `SMCLI_RANKING_OFF=project,transition,...` switches signals off, to measure each one.
    fn context(&self, item: &Case) -> RankingContext {
        let environment: HashMap<String, String> = std::env::vars().collect();
        RankingContext::new(&ProjectContext::discover(&self.path(item.directory)), item.session.map(String::from), &environment)
    }

    fn suggest(&self, item: &Case) -> Vec<(String, Vec<String>)> {
        let context = self.context(item);
        self.log
            .suggestions(&user_name(), &self.path(item.directory), item.query, 10, NOW, Some(&context))
            .unwrap()
            .into_iter()
            .map(|s| (s.command, s.why.unwrap_or_default()))
            .collect()
    }

    fn search(&self, item: &Case) -> Vec<(String, Vec<String>)> {
        let directory = self.path(item.directory);
        let context = self.context(item);
        let stats = self.log.command_stats(Some(&directory), context.project_root.as_deref(), NOW).unwrap();
        let corpus = search_corpus(stats, &[]);
        let signals = search_signals(&self.log, Some(context)).unwrap();
        self.engine
            .search(item.query, &corpus, 10, &SearchFilter::default(), None, &signals, NOW)
            .unwrap()
            .into_iter()
            .map(|e| (e.command, e.why.unwrap_or_default()))
            .collect()
    }
}

#[derive(Default)]
struct Score {
    reciprocal: f64,
    top1: u32,
    top3: u32,
    count: u32,
}

impl Score {
    fn add(&mut self, rank: Option<usize>) {
        self.count += 1;
        let Some(rank) = rank else { return };
        self.reciprocal += 1.0 / rank as f64;
        self.top1 += u32::from(rank == 1);
        self.top3 += u32::from(rank <= 3);
    }

    fn mrr(&self) -> f64 { self.reciprocal / f64::from(self.count.max(1)) }
    fn first_rate(&self) -> f64 { f64::from(self.top1) / f64::from(self.count.max(1)) }
    fn third_rate(&self) -> f64 { f64::from(self.top3) / f64::from(self.count.max(1)) }

    fn line(&self) -> String {
        format!("MRR {:.3}  top1 {:.2}  top3 {:.2}  (n={})", self.mrr(), self.first_rate(), self.third_rate(), self.count)
    }
}

/// Floors set to the Swift build's scores on the same set (suggest 1.000/1.00, search 0.955/0.91), so the port
/// ranks at least as well.
const FLOORS: [(Kind, f64, f64); 2] = [(Suggest, 1.0, 1.0), (Search, 0.954, 0.909)];

#[test]
fn rank_the_fixed_query_set_above_the_regression_floor() {
    let world = World::new();
    let mut scores: BTreeMap<String, Score> = BTreeMap::new();
    let mut total: BTreeMap<Kind, Score> = BTreeMap::new();
    let mut misses = vec![];
    for item in &CASES {
        let ranked = if item.kind == Suggest { world.suggest(item) } else { world.search(item) };
        let rank = ranked.iter().position(|r| r.0 == item.expected).map(|i| i + 1);
        let kind = if item.kind == Suggest { "suggest" } else { "search" };
        scores.entry(format!("{kind} {}", item.group)).or_default().add(rank);
        total.entry(item.kind).or_default().add(rank);
        if rank != Some(1) {
            let why = |command: &str| ranked.iter().find(|r| r.0 == command).map_or("-".into(), |r| r.1.join(", "));
            misses.push(format!(
                "  {kind} [{}] {:?} in {}: wanted {:?}, rank {}; got {:?}\n      why: {}: {} | {}: {}",
                item.group, item.query, item.directory, item.expected, rank.map_or("none".into(), |r| r.to_string()),
                ranked.iter().take(3).map(|r| &r.0).collect::<Vec<_>>(),
                ranked.first().map_or("-", |r| &r.0), ranked.first().map_or("-".into(), |r| why(&r.0)),
                item.expected, why(item.expected),
            ));
        }
    }
    println!("ranking:");
    for (key, score) in &scores {
        println!("  {key:<24} {}", score.line());
    }
    for (kind, score) in &total {
        println!("  {:<24} {}", format!("{} TOTAL", if *kind == Suggest { "suggest" } else { "search" }), score.line());
    }
    if !misses.is_empty() {
        println!("ranking misses:\n{}", misses.join("\n"));
    }
    // Floors only hold with every signal on; SMCLI_RANKING_OFF is for measuring one at a time.
    if std::env::var("SMCLI_RANKING_OFF").is_err() {
        for (kind, mrr, top1) in FLOORS {
            let score = &total[&kind];
            assert!(score.mrr() >= mrr, "{kind:?} MRR {} fell below {mrr}", score.mrr());
            assert!(score.first_rate() >= top1, "{kind:?} top-1 {} fell below {top1}", score.first_rate());
        }
    }
}
