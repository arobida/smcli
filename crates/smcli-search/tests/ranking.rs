//! Port of `RankingQualityTests.swift`: a fixed query set over a deterministic synthetic history, scored by mean
//! reciprocal rank and top-1/top-3. `make ranking` prints the scores; the test fails below the floors.
//! The fixture never touches real history: it builds real directories (manifests, `.git` markers) under a
//! temporary root, since project and Git context are read from disk.

use std::collections::{BTreeMap, HashMap};

use rusqlite::Connection;
use smcli_core::config::CaptureFilter;
use smcli_core::context::{ProjectContext, real_path};
use smcli_core::entry::{SearchEntry, SearchFilter};
use smcli_core::history::{CapturedHistory, NewRun, user_name};
use smcli_core::ranking::RankingContext;
use smcli_search::corpus::search_corpus;
use smcli_search::setup::search_signals;
use smcli_search::{HistorySearch, Potion};

const NOW: f64 = 1_800_000_000.0;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Cohort {
    Original,
    Adversarial,
    HeldOut,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Kind {
    Suggest,
    Search,
}

#[derive(Clone, Copy)]
struct Case {
    cohort: Cohort,
    group: &'static str,
    kind: Kind,
    directory: &'static str,
    query: &'static str,
    expected: &'static str,
    forbidden: Option<&'static str>,
    session: Option<&'static str>,
}

const fn case(cohort: Cohort, group: &'static str, kind: Kind, directory: &'static str, query: &'static str, expected: &'static str) -> Case {
    Case { cohort, group, kind, directory, query, expected, forbidden: None, session: None }
}

const fn in_session(c: Case, session: &'static str) -> Case {
    Case { session: Some(session), ..c }
}

const fn with_forbidden(c: Case, forbidden: &'static str) -> Case {
    Case { forbidden: Some(forbidden), ..c }
}

use Cohort::{Adversarial, HeldOut, Original};
use Kind::{Search, Suggest};

const ORIGINAL_CASES: [Case; 34] = [
    // Directory: the command used where you are.
    case(Original, "directory", Suggest, "webapp", "npm run ", "npm run dev"),
    case(Original, "directory", Suggest, "api/Sources/Lib", "swift b", "swift build"),
    case(Original, "directory", Suggest, "tools", "make r", "make release"),
    case(Original, "directory", Search, "api/Sources/Lib", "build", "swift build"),
    case(Original, "directory", Search, "webapp", "npm test", "npm test"),
    // Project: commands from sibling directories of the same package outrank unrelated, more frequent ones.
    case(Original, "project", Suggest, "api/Sources/App", "swift t", "swift test --filter Lib"),
    case(Original, "project", Suggest, "mono/packages/ui/src", "npm run ", "npm run docs"),
    case(Original, "project", Search, "api/Sources/App", "swift test", "swift test --filter Lib"),
    case(Original, "project", Search, "mono/packages/ui/src", "npm run", "npm run docs"),
    // Same signal, other queries: a sibling build habit, the named in-project test, and the other package's script.
    case(Original, "project", Suggest, "api/Sources/App", "swift b", "swift build"),
    case(Original, "project", Suggest, "api/Sources/Lib", "swift test", "swift test --filter Lib"),
    case(Original, "project", Search, "api/Sources/App", "swift build", "swift build"),
    case(Original, "project", Search, "mono/packages/core", "npm run", "npm run bundle"),
    // Used here outweighs elsewhere in the project.
    case(Original, "project", Search, "api/Sources/Lib", "swift package", "swift package update"),
    // Transitions: what usually comes after the command you just ran.
    in_session(case(Original, "transition", Suggest, "notes", "git ", "git commit -m wip"), "live1"),
    in_session(case(Original, "transition", Suggest, "notes", "make ", "make lint"), "live2"),
    in_session(case(Original, "transition", Search, "notes", "git commit", "git commit -m wip"), "live1"),
    // Selections: what you picked for a similar query before.
    case(Original, "selection", Suggest, "other", "docker", "docker compose up -d"),
    case(Original, "selection", Suggest, "other", "git l", "git log --oneline -20"),
    case(Original, "selection", Search, "other", "start containers", "docker compose up -d"),
    // Project scripts and targets that exist in a manifest but were never run.
    case(Original, "scripts", Suggest, "webapp", "npm run sto", "npm run storybook"),
    case(Original, "scripts", Suggest, "tools", "make doc", "make docs"),
    case(Original, "scripts", Search, "webapp", "storybook", "npm run storybook"),
    case(Original, "scripts", Search, "tools", "docs", "make docs"),
    // Guards: context signals must not disturb queries they have nothing to say about.
    in_session(case(Original, "guard", Suggest, "notes", "ls", "ls -la"), "live1"),
    case(Original, "guard", Suggest, "rebasing", "git s", "git status"),
    case(Original, "guard", Suggest, "webapp", "npm run b", "npm run build"),
    case(Original, "guard", Search, "rebasing", "show running containers", "docker ps"),
    // Query-derived filters: program/subcommand and directory basenames are parsed into TalaDB filters.
    case(Original, "filter", Search, "bun", "how do i create a new bun project", "bun create react-app"),
    case(Original, "filter", Search, "smcli", "how do i build my smcli project", "cargo build"),
    // Git state: the way out of an operation in progress.
    case(Original, "git state", Suggest, "rebasing", "git ", "git rebase --continue"),
    case(Original, "git state", Suggest, "merging", "git m", "git merge --continue"),
    case(Original, "git state", Search, "rebasing", "continue", "git rebase --continue"),
    case(Original, "git state", Search, "merging", "continue", "git merge --continue"),
];

const ADVERSARIAL_CASES: [Case; 16] = [
    // Exact literal distinctions: a rare flag variant must beat a frequent near-match.
    case(Adversarial, "flag-variant", Search, "flags", "git reset --soft", "git reset --soft HEAD"),
    case(Adversarial, "revision", Search, "revisions", "HEAD~1", "git reset HEAD~1"),
    case(Adversarial, "rare-exact", Search, "rare-exact", "main-branch", "git checkout main-branch"),
    // Quoted paths and very long shared prefixes (the long-prefix cases are added at runtime).
    case(Adversarial, "quoted-path", Search, "quoted", r#""my file.txt""#, r#"cat "my file.txt""#),
    // Identifier parts: camelCase/PascalCase queries should reach the full identifier.
    case(Adversarial, "camelcase", Search, "camel", "bar tests", "xcodebuild -scheme FooBarTests"),
    case(Adversarial, "camelcase", Search, "camel", "FooBar", "run FooBar"),
    // Intent paraphrase without command tokens, and a never-run project script.
    case(Adversarial, "intent", Search, "intent", "undo the last commit but keep changes", "git reset --soft HEAD~1"),
    case(Adversarial, "intent", Search, "intent", "show recent commits", "git log --oneline -20"),
    case(Adversarial, "intent", Search, "intent", "what is using port 3000", "lsof -i :3000"),
    case(Adversarial, "intent", Search, "intent", "find the biggest files here", "du -sh * | sort -h"),
    case(Adversarial, "intent", Search, "intent", "delete branches that were merged", "git branch --merged | xargs git branch -d"),
    case(Adversarial, "intent", Search, "intent", "see what changed", "git diff"),
    case(Adversarial, "intent", Search, "intent", "download a file", "curl -O https://example.com/file.tgz"),
    case(Adversarial, "intent", Search, "intent", "unpack a tarball", "tar -xzf archive.tgz"),
    case(Adversarial, "intent", Search, "intent", "check github login", "gh auth status"),
    case(Adversarial, "cold-start", Suggest, "coldstart", "npm run sto", "npm run storybook"),
];

fn long_tail_cases() -> [Case; 2] {
    let alpha = ("x".repeat(1_500) + " ALPHA").into_boxed_str();
    let beta = ("x".repeat(1_500) + " BETA").into_boxed_str();
    // Leak to obtain 'static references; the allocation is tiny and fixed for the test process.
    let alpha: &'static str = Box::leak(alpha);
    let beta: &'static str = Box::leak(beta);
    [
        case(Adversarial, "long-tail", Search, "longtail", "ALPHA", alpha),
        case(Adversarial, "long-tail", Search, "longtail", "BETA", beta),
    ]
}

const HELD_OUT_CASES: [Case; 3] = [
    // Strict filter: resolved $word markers must reject everything outside the program/subcommand.
    with_forbidden(case(HeldOut, "strict-filter", Search, "heldout", "$git $status", "git status"), "git stash"),
    // Forbidden result: a literal whole-token match must keep an unrelated command out of the top.
    with_forbidden(case(HeldOut, "forbidden", Suggest, "heldout", "make release", "make release"), "make lint"),
    // Whole-token exactness via strict marker: a different subcommand must be filtered out.
    with_forbidden(case(HeldOut, "whole-token", Search, "heldout", "$make $release", "make release"), "make lint"),
];

const CASES: [Case; 53] = {
    let mut all: [Case; 53] = [ORIGINAL_CASES[0]; 53];
    let mut i = 0;
    while i < ORIGINAL_CASES.len() {
        all[i] = ORIGINAL_CASES[i];
        i += 1;
    }
    while i < ORIGINAL_CASES.len() + ADVERSARIAL_CASES.len() {
        all[i] = ADVERSARIAL_CASES[i - ORIGINAL_CASES.len()];
        i += 1;
    }
    while i < all.len() {
        all[i] = HELD_OUT_CASES[i - ORIGINAL_CASES.len() - ADVERSARIAL_CASES.len()];
        i += 1;
    }
    all
};

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
        for directory in ["notes", "other", "api/Sources/App", "api/Sources/Lib", "mono/packages/ui/src", "mono/packages/ui/stories", "bun", "smcli",
                          "flags", "revisions", "rare-exact", "quoted", "longtail", "camel", "intent", "coldstart", "heldout"] {
            std::fs::create_dir_all(self.path(directory)).unwrap();
        }
        // A never-run project script for the cold-start adversarial case.
        self.write(r#"{"name":"coldstart","scripts":{"storybook":"storybook dev","build":"vite build"}}"#,
                   "coldstart/package.json");
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
        self.habit("swift package update", "api/Sources/Lib", 6);
        self.habit("swift package resolve", "api/Sources/App", 6);
        self.habit("swift test --parallel", "tools", 16);
        self.habit("swift build -c release", "tools", 15);
        self.habit("make release", "other", 25);
        self.habit("ls -la", "notes", 30);
        // Query-derived filters: bun and smcli directory basenames become filters.
        self.habit("bun i", "bun", 10);
        self.habit("bun i usearch", "bun", 5);
        self.habit("bun create react-app", "bun", 3);
        self.habit("cargo build", "smcli", 8);
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

        // Adversarial history.
        self.habit_with("git reset --hard HEAD", "flags", 20, 0.5, 10.0);
        self.run("git reset --soft HEAD", "flags", NOW - 1.0, "flags1", 0);

        self.habit_with("git reset HEAD~10", "revisions", 10, 0.5, 10.0);
        self.run("git reset HEAD~1", "revisions", NOW - 1.0, "rev1", 0);

        self.habit_with("git checkout main", "rare-exact", 25, 0.5, 10.0);
        self.run("git checkout main-branch", "rare-exact", NOW - 5.0, "rare1", 0);

        self.habit_with("cat my-file.txt", "quoted", 15, 0.5, 10.0);
        self.run(r#"cat "my file.txt""#, "quoted", NOW - 1.0, "quoted1", 0);

        let prefix = "x".repeat(1_500);
        let alpha = format!("{prefix} ALPHA");
        let beta = format!("{prefix} BETA");
        self.run(&alpha, "longtail", NOW - 2.0, "long1", 0);
        self.run(&beta, "longtail", NOW - 1.0, "long2", 0);

        self.run("xcodebuild -scheme FooBarTests", "camel", NOW - 1.0, "camel1", 0);
        self.run("run FooBar", "camel", NOW - 2.0, "camel2", 0);
        self.run("run Foo", "camel", NOW - 3.0, "camel3", 0);
        self.run("run Bar", "camel", NOW - 4.0, "camel4", 0);

        self.habit_with("git commit --amend", "intent", 10, 0.5, 10.0);
        self.run("git reset --soft HEAD~1", "intent", NOW - 1.0, "intent1", 0);
        // Intent distractors share words with the queries (`find`, `branch`), not meaning.
        self.run("git log --oneline -20", "intent", NOW - 2.0, "intent2", 0);
        self.run("lsof -i :3000", "intent", NOW - 5.0, "intent5", 0);
        self.run("du -sh * | sort -h", "intent", NOW - 6.0, "intent6", 0);
        self.run("git branch --merged | xargs git branch -d", "intent", NOW - 7.0, "intent7", 0);
        self.run("git diff", "intent", NOW - 8.0, "intent8", 0);
        self.run("curl -O https://example.com/file.tgz", "intent", NOW - 9.0, "intent9", 0);
        self.run("tar -xzf archive.tgz", "intent", NOW - 10.0, "intent10", 0);
        self.run("gh auth status", "intent", NOW - 11.0, "intent11", 0);
        self.run("find . -name '*.md'", "intent", NOW - 14.0, "intent14", 0);
        self.run("git branch", "intent", NOW - 15.0, "intent15", 0);

        // Held-out history (kept small and using commands that do not move original cohort scores).
        self.run("git status", "heldout", NOW - 1.0, "held1", 0);
        self.run("git stash", "heldout", NOW - 2.0, "held2", 0);
        self.run("make release", "heldout", NOW - 1.0, "held3", 0);
        self.run("make lint", "heldout", NOW - 2.0, "held4", 0);
        self.run("python server.py", "heldout", NOW - 1.0, "held5", 0);
        self.run("ruby client.rb", "heldout", NOW - 2.0, "held6", 0);
        self.run("python test.py", "heldout", NOW - 3.0, "held7", 0);
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
        let signals = search_signals(&self.log, Some(context), &HashMap::from([("PWD".into(), directory)])).unwrap();
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
/// ranks at least as well. Only the original cohort is frozen; adversarial floors are set after the fixes land.
const ORIGINAL_FLOORS: [(Kind, f64, f64); 2] = [(Suggest, 1.0, 1.0), (Search, 0.954, 0.909)];

#[test]
fn rank_the_fixed_query_set_above_the_regression_floor() {
    let world = World::new();
    let mut scores: BTreeMap<(Cohort, Kind, String), Score> = BTreeMap::new();
    let mut totals: BTreeMap<(Cohort, Kind), Score> = BTreeMap::new();
    let mut misses = vec![];
    let mut held_out_violations = vec![];
    let long_tail = long_tail_cases();
    for item in CASES.iter().chain(long_tail.iter()) {
        let ranked = if item.kind == Suggest { world.suggest(item) } else { world.search(item) };
        let rank = ranked.iter().position(|r| r.0 == item.expected).map(|i| i + 1);
        let key = (item.cohort, item.kind, item.group.to_string());
        scores.entry(key).or_default().add(rank);
        totals.entry((item.cohort, item.kind)).or_default().add(rank);
        if rank != Some(1) {
            let why = |command: &str| ranked.iter().find(|r| r.0 == command).map_or("-".into(), |r| r.1.join(", "));
            misses.push(format!(
                "  {} {} [{}] {:?} in {}: wanted {:?}, rank {}; got {:?}\n      why: {}: {} | {}: {}",
                format!("{:?}", item.cohort).to_lowercase(),
                if item.kind == Suggest { "suggest" } else { "search" },
                item.group, item.query, item.directory, item.expected, rank.map_or("none".into(), |r| r.to_string()),
                ranked.iter().take(3).map(|r| &r.0).collect::<Vec<_>>(),
                ranked.first().map_or("-", |r| &r.0), ranked.first().map_or("-".into(), |r| why(&r.0)),
                item.expected, why(item.expected),
            ));
        }
        if std::env::var("SMCLI_RANKING_OFF").is_err() && item.cohort == Adversarial && item.group != "intent" {
            assert_eq!(rank, Some(1), "adversarial {} failed: {:?}", item.group, item.query);
        }
        if let Some(forbidden) = item.forbidden {
            let forbidden_rank = ranked.iter().position(|r| r.0 == forbidden).map(|i| i + 1);
            if forbidden_rank.map_or(false, |r| r <= 3) {
                held_out_violations.push(format!(
                    "  held-out {} [{}] {:?}: forbidden {:?} appeared at rank {}",
                    if item.kind == Suggest { "suggest" } else { "search" },
                    item.group, item.query, forbidden, forbidden_rank.unwrap()
                ));
            }
        }
    }
    println!("ranking:");
    for ((cohort, kind, group), score) in &scores {
        let cohort = format!("{:?}", cohort).to_lowercase();
        let kind = if *kind == Suggest { "suggest" } else { "search" };
        println!("  {cohort:<12} {kind:<7} {group:<24} {}", score.line());
    }
    for ((cohort, kind), score) in &totals {
        let cohort = format!("{:?}", cohort).to_lowercase();
        let kind = if *kind == Suggest { "suggest" } else { "search" };
        println!("  {cohort:<12} {kind:<7} {:<24} {}", "TOTAL", score.line());
    }
    if !misses.is_empty() {
        println!("ranking misses:\n{}", misses.join("\n"));
    }
    if !held_out_violations.is_empty() {
        println!("held-out violations:\n{}", held_out_violations.join("\n"));
    }
    // Floors only hold with every signal on; SMCLI_RANKING_OFF is for measuring one at a time.
    if std::env::var("SMCLI_RANKING_OFF").is_err() {
        for (kind, mrr, top1) in ORIGINAL_FLOORS {
            let score = &totals[&(Original, kind)];
            assert!(score.mrr() >= mrr, "original {kind:?} MRR {} fell below {mrr}", score.mrr());
            assert!(score.first_rate() >= top1, "original {kind:?} top-1 {} fell below {top1}", score.first_rate());
        }
        assert!(held_out_violations.is_empty(), "held-out forbidden results appeared:\n{}", held_out_violations.join("\n"));
    }
    rank_filesystem_signal_cohorts();
}

#[test]
fn rank_record_time_aliases() {
    use smcli_core::ranking::Signal;
    use smcli_search::corpus::SearchSignals;
    let root = tempfile::tempdir().unwrap();
    let log = CapturedHistory::new(root.path().join("history.db"));
    let db = log.open_connection().unwrap();
    for (typed, expanded, runs) in [
        ("gst", Some("git status"), 3),
        ("gl", Some("git log --oneline -20"), 3),
        ("gd", Some("git diff --stat"), 3),
        ("gst --short", Some("git status --short"), 3),
        ("gl", Some("git ls-files"), 1),
        ("git status", None, 1),
        ("git log --graph", None, 1),
        ("git diff --color", None, 1),
        ("docker ps", None, 1),
    ] {
        for i in 0..runs {
            let mut run = NewRun::new(typed, "zsh", "/alias-fixture", 0);
            run.expanded_command = expanded;
            run.started_at = Some(NOW - 10.0 + i as f64);
            run.session = typed;
            log.append(&run, "", Some(&db)).unwrap();
        }
    }
    let corpus = search_corpus(log.command_stats(Some("/alias-fixture"), None, NOW).unwrap(), &["nativegst".into(), "oldalias".into()]);
    let gl: Vec<_> = corpus.values().filter(|e| e.command == "gl").collect();
    assert_eq!(gl.len(), 2);
    assert_eq!(gl.iter().map(|e| e.runs).sum::<i64>(), 4);
    assert_eq!(corpus["git status"].runs, 1, "direct usage must not merge with the alias");
    assert!(corpus["nativegst"].expanded_command.is_none(), "native history never inherits today's aliases");
    let engine = HistorySearch::new(root.path().join("index"), Potion::bundled().unwrap());
    let on = SearchSignals::default();
    let mut off = on.clone();
    off.context = Some(RankingContext { off: [Signal::Alias].into_iter().collect(), ..Default::default() });
    let mut scores = [Score::default(), Score::default()];
    for (query, typed, expanded) in [
        ("gst", "gst", Some("git status")),
        ("gl", "gl", Some("git log --oneline -20")),
        ("git status", "gst", Some("git status")),
        ("git log --oneline", "gl", Some("git log --oneline -20")),
        ("git diff --stat", "gd", Some("git diff --stat")),
        ("git status --short", "gst --short", Some("git status --short")),
        ("docker ps", "docker ps", None),
        ("oldalias", "oldalias", None),
    ] {
        for (i, signals) in [&on, &off].into_iter().enumerate() {
            let hits = engine.search(query, &corpus, 20, &SearchFilter::default(), None, signals, NOW).unwrap();
            let rank = hits.iter().position(|e| e.command == typed && e.expanded_command.as_deref() == expanded).map(|i| i + 1);
            scores[i].add(rank);
            if i == 0 { assert_eq!(rank, Some(1), "alias {query}: {hits:?}"); }
            assert!(hits.iter().flat_map(|e| e.why.iter().flatten()).all(|why| !why.contains("git log --oneline") && !why.contains("git ls-files")));
        }
    }
    println!("  alias        search  TOTAL                    {}", scores[0].line());
    println!("  alias        search  signal-off               {}", scores[1].line());
    let filter = SearchFilter { program: Some("git".into()), subcommand: Some("status".into()), flag: Some("--short".into()), ..Default::default() };
    let strict = engine.search("$git $status", &corpus, 20, &filter, None, &on, NOW).unwrap();
    assert_eq!(strict.len(), 1);
    assert_eq!(strict[0].expanded_command.as_deref(), Some("git status --short"));
    let lock = std::fs::OpenOptions::new().read(true).write(true).open(engine.directory.join("lock")).unwrap();
    lock.try_lock().unwrap();
    let fallback = engine.search("$git $status", &corpus, 20, &filter, None, &on, NOW).unwrap();
    assert_eq!(fallback.iter().map(|e| e.key()).collect::<Vec<_>>(), strict.iter().map(|e| e.key()).collect::<Vec<_>>());
}

/// Alias evaluation: how each way of indexing an alias ranks it for plain-language queries, against competing commands.
/// `SMCLI_ALIAS_EVAL_DISTRACTORS=<file>` adds one command per line (a local export of real history; never commit it).
#[test]
fn rank_alias_eval() {
    use smcli_search::corpus::SearchSignals;
    let cases: Vec<(&str, &str, Vec<&str>)> = include_str!("fixtures/alias-eval.tsv").lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f[0], f[1], f[2].split(" ;; ").collect())
        })
        .collect();
    let mut distractors: Vec<String> = include_str!("fixtures/alias-eval-distractors.txt").lines().map(String::from).collect();
    if let Ok(path) = std::env::var("SMCLI_ALIAS_EVAL_DISTRACTORS") {
        distractors.extend(std::fs::read_to_string(path).unwrap().lines().filter(|l| !l.trim().is_empty()).map(String::from));
    }
    distractors.sort();
    distractors.dedup();
    let gated = cases.iter().filter(|c| !smcli_core::parse::safe_expansion(c.1)).count();
    println!("  alias-eval   {} aliases, {} queries, {} distractors, {} expansions refused by the gate",
        cases.len(), cases.iter().map(|c| c.2.len()).sum::<usize>(), distractors.len(), gated);
    // Typed only (before alias capture), then the expansion as written.
    let mut results = Vec::new();
    for (label, with_expansion) in [("typed-only", false), ("expanded", true)] {
        let root = tempfile::tempdir().unwrap();
        let log = CapturedHistory::new(root.path().join("history.db"));
        let db = log.open_connection().unwrap();
        let add = |typed: &str, expanded: Option<&str>| {
            for i in 0..2 {
                let mut run = NewRun::new(typed, "zsh", "/alias-eval", 0);
                run.expanded_command = expanded;
                run.started_at = Some(NOW - 10.0 + f64::from(i));
                log.append(&run, "", Some(&db)).unwrap();
            }
        };
        for d in &distractors {
            add(d, None);
        }
        for (typed, expanded, _) in &cases {
            add(typed, Some(*expanded).filter(|e| with_expansion && smcli_core::parse::safe_expansion(e)));
        }
        let corpus = search_corpus(log.command_stats(Some("/alias-eval"), None, NOW).unwrap(), &[]);
        let engine = HistorySearch::new(root.path().join("index"), Potion::bundled().unwrap());
        let (mut all, mut git, mut personal) = (Score::default(), Score::default(), Score::default());
        let mut ranks = Vec::new();
        for (typed, expanded, queries) in &cases {
            for query in queries {
                let hits = engine.search(query, &corpus, 20, &SearchFilter::default(), None, &SearchSignals::default(), NOW).unwrap();
                let rank = hits.iter().position(|e| e.command == *typed).map(|i| i + 1);
                all.add(rank);
                if expanded.starts_with("git ") { git.add(rank) } else { personal.add(rank) }
                ranks.push((*typed, *query, rank));
            }
        }
        println!("  {label:<16} all       {}", all.line());
        println!("  {label:<16} git       {}", git.line());
        println!("  {label:<16} personal  {}", personal.line());
        results.push((label, all.mrr(), ranks));
    }
    let shipped = &results[1].2;
    let missed: Vec<_> = shipped.iter().filter(|(_, _, r)| r.is_none_or(|r| r > 3)).collect();
    println!("  expanded misses (rank > 3 or not in top 20): {}", missed.len());
    for (typed, query, rank) in missed.iter().take(40) {
        println!("    {typed:<14} {query:<45} {rank:?}");
    }
    assert!(results[1].1 > results[0].1, "recording expansions must beat typed-only");
}

/// Controlled equal-relevance hits isolate the signals from model noise; the ordinary cohorts above
/// still exercise indexed retrieval, and index unit tests exercise the lock fallback.
fn rank_filesystem_signal_cohorts() {
    use smcli_core::ranking::Signal;
    use smcli_search::corpus::{SearchSignals, rerank};
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    for (name, body) in [("txbuild", "binary"), ("wrapper", "#!/bin/sh\n"), ("echo", "binary")] {
        let path = bin.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    std::fs::write(a.join("notes.md"), "").unwrap();
    std::fs::write(a.join("my notes.md"), "").unwrap();
    std::fs::write(b.join("todo.md"), "").unwrap();
    let entry = |command: &str, frecency| SearchEntry { frecency, ..SearchEntry::new(command, 1, "native") };
    let mut totals: BTreeMap<&str, Score> = BTreeMap::new();
    let mut off_totals: BTreeMap<&str, Score> = BTreeMap::new();
    let cases = [
        ("path", "present", "uxbuild app", "txbuild app", 1.0, 1.0, true),
        ("path", "missing", "uxbuild app", "missing app", 1.0, 1.0, false),
        ("path", "builtin", "uxbuild app", "echo app", 1.0, 1.0, false),
        ("path", "wrapper", "uxbuild app", "wrapper app", 1.0, 1.0, false),
        ("path", "absolute", "uxbuild app", "/bin/txbuild app", 1.0, 1.0, false),
        ("path", "unknown", "uxbuild app", "function app", 1.0, 1.0, false),
        ("path", "usage-dominates", "uxbuild app", "txbuild app", 25.0, 1.0, false),
        ("files", "present", "cat todo.md", "cat notes.md", 1.0, 1.0, true),
        ("files", "quoted", "cat missing.md", "cat 'my notes.md'", 1.0, 1.0, true),
        ("files", "head", "head -n 20 missing.md", "head -n 20 notes.md", 1.0, 1.0, true),
        ("files", "missing", "cat missing.md", "cat absent.md", 1.0, 1.0, false),
        ("files", "mkdir", "mkdir missing.md", "mkdir notes.md", 1.0, 1.0, false),
        ("files", "output", "tee missing.md", "tee notes.md", 1.0, 1.0, false),
        ("files", "glob", "cat missing.md", "cat *.md", 1.0, 1.0, false),
        ("files", "redirect", "cat missing.md", "cat notes.md > out", 1.0, 1.0, false),
        ("files", "usage-dominates", "cat missing.md", "cat notes.md", 25.0, 1.0, false),
    ];
    for (cohort, name, first, second, habit, rare, flip) in cases {
        let signal = if cohort == "path" { Signal::Path } else { Signal::Files };
        let signals = SearchSignals {
            shell_path: (cohort == "path").then(|| bin.to_string_lossy().into_owned()),
            directory: (cohort == "files").then(|| a.to_string_lossy().into_owned()),
            context: Some(RankingContext::default()),
            ..Default::default()
        };
        let hits = vec![(entry(first, habit), 1.0), (entry(second, rare), 1.0)];
        let on = rerank(hits.clone(), false, "inspect", NOW, &signals);
        let mut disabled = signals.clone();
        disabled.context.as_mut().unwrap().off.insert(signal);
        let off = rerank(hits.clone(), false, "inspect", NOW, &disabled);
        let absent = rerank(hits, false, "inspect", NOW, &SearchSignals::default());
        assert_eq!(off, absent, "{cohort}/{name}: disabled must be identical to absent");
        let expected = if flip { second } else { first };
        assert_eq!(on[0].command, expected, "{cohort}/{name}");
        totals.entry(cohort).or_default().add(on.iter().position(|e| e.command == expected).map(|i| i + 1));
        off_totals.entry(cohort).or_default().add(off.iter().position(|e| e.command == expected).map(|i| i + 1));
        let why = on.iter().find(|e| e.command == second).unwrap().why.as_ref().unwrap();
        assert!(why.iter().all(|reason| !reason.contains(root.path().to_str().unwrap())));
        if flip {
            assert!(why.iter().any(|reason| reason == if cohort == "path" { "program found in PATH" } else { "input file exists" }));
        }
    }
    // The same command's evidence follows cwd and current metadata, not service state.
    let signals = SearchSignals { directory: Some(b.to_string_lossy().into_owned()), ..Default::default() };
    let hits = vec![(entry("cat notes.md", 1.0), 1.0), (entry("cat todo.md", 1.0), 1.0)];
    assert_eq!(rerank(hits.clone(), false, "inspect", NOW, &signals)[0].command, "cat todo.md");
    std::fs::rename(b.join("todo.md"), b.join("new.md")).unwrap();
    assert_eq!(rerank(hits, false, "inspect", NOW, &signals)[0].command, "cat notes.md");
    // Hold the public index-directory lock to exercise the real text fallback at limit=1.
    let index_dir = root.path().join("locked-index");
    std::fs::create_dir(&index_dir).unwrap();
    std::fs::set_permissions(&index_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let lock = std::fs::File::create(index_dir.join("lock")).unwrap();
    lock.try_lock().unwrap();
    let engine = HistorySearch::new(&index_dir, Potion::bundled().unwrap());
    for (query, near, target, signals) in [
        ("artifact", "axbuild artifact", "txbuild artifact", SearchSignals {
            shell_path: Some(bin.to_string_lossy().into_owned()), ..Default::default()
        }),
        ("cat", "cat missing.md", "cat notes.md", SearchSignals {
            directory: Some(a.to_string_lossy().into_owned()), ..Default::default()
        }),
    ] {
        let corpus = [(near.into(), entry(near, 1.1)), (target.into(), entry(target, 1.0))].into_iter().collect();
        let hits = engine.search(query, &corpus, 1, &SearchFilter::default(), None, &signals, NOW).unwrap();
        assert_eq!(hits[0].command, target, "text fallback must boost before truncation: {query}");
        let off = engine.search(query, &corpus, 1, &SearchFilter::default(), None, &SearchSignals::default(), NOW).unwrap();
        assert_eq!(off[0].command, near, "text fallback control: {query}");
    }
    drop(lock);
    for (cohort, score) in totals {
        println!("  {cohort:<12} search  TOTAL                    {}", score.line());
        println!("  {cohort:<12} search  signal-off               {}", off_totals[cohort].line());
    }
}
