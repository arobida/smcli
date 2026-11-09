use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use regex::Regex;

use crate::time::now;

/// What a directory is part of: its Git repository and worktree, and the projects around it. Discovery only
/// stats and reads small files (never runs Git, never executes a manifest), so it is cheap enough for the
/// typing path. The types are the service protocol's; `discover` leaves `status` unset.
use crate::service_protocol::GitStatus;
pub use crate::service_protocol::{Project, ProjectContext, Repository};

/// The only package.json managers smcli builds commands for; anything else in the field is ignored.
pub const NODE_MANAGERS: [&str; 4] = ["npm", "pnpm", "yarn", "bun"];

/// Languages first, then task runners and containers, so a language decides the kind when there is one.
const MANIFESTS: [(&[&str], &str); 19] = [
    (&["Package.swift"], "swift"),
    (&["package.json"], "node"),
    (&["deno.json", "deno.jsonc"], "deno"),
    (&["Cargo.toml"], "rust"),
    (&["pyproject.toml"], "python"),
    (&["go.mod"], "go"),
    (&["Gemfile"], "ruby"),
    (&["composer.json"], "php"),
    (&["pom.xml"], "maven"),
    (&["build.gradle.kts", "build.gradle"], "gradle"),
    (&["mix.exs"], "elixir"),
    (&["pubspec.yaml"], "dart"),
    (&["CMakeLists.txt"], "cmake"),
    (&["Makefile", "makefile", "GNUmakefile"], "make"),
    (&["justfile", "Justfile", ".justfile"], "just"),
    (&["Taskfile.yml", "Taskfile.yaml"], "task"),
    (&["Rakefile"], "rake"),
    (
        &[
            "compose.yaml",
            "compose.yml",
            "docker-compose.yml",
            "docker-compose.yaml",
        ],
        "compose",
    ),
    (&["Dockerfile"], "docker"),
];
const MAX_LEVELS: usize = 64;

fn exists(path: &str) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn regular_file(path: &str) -> bool {
    // metadata follows symlinks; opening a FIFO blocks until a writer connects.
    std::fs::metadata(path).is_ok_and(|info| info.is_file())
}

fn parent(path: &str) -> String {
    match Path::new(path).parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_string_lossy().into_owned(),
        _ => path.to_string(),
    }
}

pub fn real_path(path: &str) -> Option<String> {
    std::fs::canonicalize(path)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// The primary kind of the nearest project at or above `directory`: `discover(..).projects.first().kind` without
/// reading Git or parsing manifests (only `regular_file` stats), starting from the directory's real path like
/// `discover`. A directory that is gone or has no manifest has none, so its runs get no same-kind boost (and no
/// penalty).
pub fn directory_kind(directory: &str) -> Option<String> {
    if !Path::new(directory).is_dir() {
        return None;
    }
    let mut current = real_path(directory).unwrap_or_else(|| directory.to_string());
    for _ in 0..MAX_LEVELS {
        let has = |file: &&str| regular_file(&format!("{current}/{file}"));
        if let Some((_, kind)) = MANIFESTS.iter().find(|(names, _)| names.iter().any(has)) {
            return Some((*kind).into());
        }
        let up = parent(&current);
        // Like discover, projects above the Git repository root do not count.
        if up == current || ProjectContext::repository(&current).is_some() {
            return None;
        }
        current = up;
    }
    None
}

/// `directory_kind` remembered for `ttl`: an expired or missing entry is probed again, and a deleted directory
/// probes as unknown. The service keeps one so a long-lived index notices a manifest added or a directory removed
/// within the ttl. It can never hold more entries than the distinct directories the index keeps.
#[derive(Debug, Clone)]
pub struct KindCache {
    pub ttl: std::time::Duration,
    entries: HashMap<String, (Option<String>, std::time::Instant)>,
}

pub const KIND_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(60);

impl KindCache {
    pub fn new(ttl: std::time::Duration) -> Self {
        Self {
            ttl,
            entries: HashMap::new(),
        }
    }

    /// A fresh entry's kind, or None when `directory` must be probed (and then `insert`ed).
    pub fn lookup(&self, directory: &str) -> Option<Option<String>> {
        self.entries
            .get(directory)
            .filter(|(_, at)| at.elapsed() < self.ttl)
            .map(|(kind, _)| kind.clone())
    }

    pub fn insert(&mut self, directory: &str, kind: Option<String>) {
        self.entries
            .insert(directory.into(), (kind, std::time::Instant::now()));
    }

    pub fn get(&mut self, directory: &str) -> Option<String> {
        if let Some(kind) = self.lookup(directory) {
            return kind;
        }
        let kind = directory_kind(directory);
        self.insert(directory, kind.clone());
        kind
    }
}

/// `directory_kind` remembered in `memo`, for loops over many runs in few directories.
pub fn directory_kind_memo(
    memo: &mut HashMap<String, Option<String>>,
    directory: &str,
) -> Option<String> {
    if let Some(kind) = memo.get(directory) {
        return kind.clone();
    }
    let kind = directory_kind(directory);
    memo.insert(directory.into(), kind.clone());
    kind
}

pub(crate) fn read_small_file(path: &str, limit: u64) -> Option<String> {
    if !regular_file(path) {
        return None;
    }
    let mut data = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(limit)
        .read_to_end(&mut data)
        .ok()?;
    Some(String::from_utf8_lossy(&data).trim().to_string())
}

fn relative(path: &str, root: &str) -> String {
    if path == root {
        return ".".into();
    }
    path.strip_prefix(&format!("{root}/"))
        .unwrap_or(path)
        .to_string()
}

impl ProjectContext {
    pub fn discover(directory: &str) -> Self {
        let mut context = Self {
            directory: directory.into(),
            ..Default::default()
        };
        let start = real_path(directory).unwrap_or_else(|| directory.to_string());
        let mut visited = vec![];
        let mut current = start.clone();
        for _ in 0..MAX_LEVELS {
            visited.push(current.clone());
            if let Some(repository) = ProjectContext::repository(&current) {
                context.repository = Some(repository);
                break;
            }
            let up = parent(&current);
            if up == current {
                break;
            }
            current = up;
        }
        for level in &visited {
            if let Some(project) = Self::project(level) {
                context.projects.push(project);
                // Outside a repository there is no boundary to stop at, so only the nearest counts.
                if context.repository.is_none() {
                    break;
                }
            }
        }
        context.repo_relative = context
            .repository
            .as_ref()
            .map(|r| relative(&start, &r.root));
        context.project_relative = context.projects.first().map(|p| relative(&start, &p.root));
        context
    }

    /// Git's rules: `.git` is a directory, or a file `gitdir: <path>` (linked worktrees, submodules); a linked
    /// worktree's Git directory names the shared one in `commondir`.
    fn repository(directory: &str) -> Option<Repository> {
        let dot_git = format!("{directory}/.git");
        let info = std::fs::symlink_metadata(&dot_git).ok()?;
        let git_dir = if info.is_dir() {
            dot_git
        } else {
            let text = read_small_file(&dot_git, 4096)?;
            let path = text.strip_prefix("gitdir:")?.trim();
            if path.starts_with('/') {
                path.to_string()
            } else {
                format!("{directory}/{path}")
            }
        };
        let git_dir = real_path(&git_dir)?;
        let head = read_small_file(&format!("{git_dir}/HEAD"), 4096)?;
        let mut common_dir = git_dir.clone();
        if let Some(common) =
            read_small_file(&format!("{git_dir}/commondir"), 4096).filter(|c| !c.is_empty())
            && let Some(resolved) = real_path(&if common.starts_with('/') {
                common
            } else {
                format!("{git_dir}/{common}")
            })
        {
            common_dir = resolved;
        }
        let mut repository = Repository {
            root: real_path(directory).unwrap_or_else(|| directory.into()),
            linked_worktree: git_dir != common_dir,
            git_dir: git_dir.clone(),
            common_dir,
            ..Default::default()
        };
        if let Some(reference) = head.strip_prefix("ref:") {
            let reference = reference.trim();
            repository.branch = Some(
                reference
                    .strip_prefix("refs/heads/")
                    .unwrap_or(reference)
                    .to_string(),
            );
        } else {
            repository.head = Some(head.chars().take(7).collect());
        }
        let markers = [
            ("rebase-merge", "rebase"),
            ("rebase-apply/rebasing", "rebase"),
            ("rebase-apply", "am"),
            ("MERGE_HEAD", "merge"),
            ("CHERRY_PICK_HEAD", "cherry-pick"),
            ("REVERT_HEAD", "revert"),
            ("BISECT_LOG", "bisect"),
        ];
        repository.operation = markers
            .iter()
            .find(|m| exists(&format!("{git_dir}/{}", m.0)))
            .map(|m| m.1.to_string());
        Some(repository)
    }

    fn project(directory: &str) -> Option<Project> {
        let has = |file: &str| regular_file(&format!("{directory}/{file}"));
        let found: Vec<(&str, &str)> = MANIFESTS
            .iter()
            .filter_map(|(names, kind)| names.iter().find(|n| has(n)).map(|n| (*n, *kind)))
            .collect();
        let primary = *found.first()?;
        let mut project = Project {
            root: directory.into(),
            kind: primary.1.into(),
            manifests: found.iter().map(|f| f.0.to_string()).collect(),
            ..Default::default()
        };
        for (file, kind) in &found {
            let text = read_small_file(&format!("{directory}/{file}"), 1 << 20);
            let text = text.as_deref();
            let names = match *kind {
                // shortcut: literal name: "..." on target-like constructors only, upgrade to a structured reader for computed names and other declarations
                "swift" => matches(
                    r#"\.(?:executableTarget|target|testTarget|plugin|macro)\(\s*name:\s*"([^"]+)""#,
                    text,
                ),
                "node" => node_scripts(text, &mut project, &has),
                "rust" => cargo_targets(text, &mut project),
                "python" => python_scripts(text, &mut project, &has),
                "go" => {
                    if project.name.is_none() {
                        project.name = text
                            .and_then(|t| t.lines().find(|l| l.starts_with("module ")))
                            .map(|l| l[7..].trim().to_string());
                    }
                    vec![]
                }
                "deno" => json_keys(text, "tasks"),
                "php" => json_keys(text, "scripts"),
                "make" => make_targets(text),
                "just" => just_recipes(text),
                "task" => taskfile_tasks(text),
                "rake" => matches(r#"(?m)^\s*task\s+:?["']?([A-Za-z0-9_:-]+)"#, text),
                _ => vec![],
            };
            if !names.is_empty() {
                project
                    .scripts
                    .insert(file.to_string(), names.into_iter().take(200).collect());
            }
        }
        // Wrappers pin the build tool's version, so commands use them when present.
        project.manifests.extend(
            ["gradlew", "mvnw"]
                .into_iter()
                .filter(|f| has(f))
                .map(String::from),
        );
        if project.package_manager.is_none() {
            project.package_manager = match primary.1 {
                "swift" => Some("swiftpm"),
                "rust" => Some("cargo"),
                "go" => Some("go"),
                "make" => Some("make"),
                "ruby" => Some("bundler"),
                "php" => Some("composer"),
                "elixir" => Some("mix"),
                "deno" => Some("deno"),
                _ => None,
            }
            .map(String::from);
        }
        Some(project)
    }
}

fn matches(pattern: &str, text: Option<&str>) -> Vec<String> {
    let Ok(expression) = Regex::new(pattern) else {
        return vec![];
    };
    let mut seen = HashSet::new();
    expression
        .captures_iter(text.unwrap_or(""))
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .filter(|name| seen.insert(name.clone()))
        .collect()
}

fn json_object(text: Option<&str>) -> Option<serde_json::Map<String, serde_json::Value>> {
    match serde_json::from_str(text?).ok()? {
        serde_json::Value::Object(map) => Some(map),
        _ => None,
    }
}

fn json_keys(text: Option<&str>, field: &str) -> Vec<String> {
    let mut keys: Vec<String> = json_object(text)
        .and_then(|j| {
            j.get(field)?
                .as_object()
                .map(|o| o.keys().cloned().collect())
        })
        .unwrap_or_default();
    keys.sort();
    keys
}

fn node_scripts(
    text: Option<&str>,
    project: &mut Project,
    has: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    let json = json_object(text);
    project.name = json
        .as_ref()
        .and_then(|j| j.get("name")?.as_str().map(String::from));
    // The field is repository-controlled text that ends up in suggested commands, so only known names count.
    let field = json
        .as_ref()
        .and_then(|j| j.get("packageManager")?.as_str())
        .and_then(|f| f.split('@').next());
    project.package_manager = Some(
        if let Some(name) = field.filter(|n| NODE_MANAGERS.contains(n)) {
            name.to_string()
        } else {
            [
                ("pnpm-lock.yaml", "pnpm"),
                ("yarn.lock", "yarn"),
                ("bun.lock", "bun"),
                ("bun.lockb", "bun"),
                ("package-lock.json", "npm"),
            ]
            .iter()
            .find(|l| has(l.0))
            .map_or("npm", |l| l.1)
            .to_string()
        },
    );
    json_keys(text, "scripts")
}

fn toml_lines(text: Option<&str>) -> Vec<&str> {
    text.unwrap_or("")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

/// `key = "value"`; with `any_value`, a key with any value (script tables map names to entry points).
fn toml_pair(line: &str, any_value: bool) -> Option<(String, String)> {
    let equals = line.find('=')?;
    let key = line[..equals]
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_string();
    let value = line[equals + 1..].trim();
    if let Some(rest) = value.strip_prefix('"')
        && let Some(end) = rest.find('"')
    {
        return Some((key, rest[..end].to_string()));
    }
    any_value.then(|| (key, value.to_string()))
}

fn cargo_targets(text: Option<&str>, project: &mut Project) -> Vec<String> {
    let mut names = vec![];
    let mut section = "";
    for line in toml_lines(text) {
        if line.starts_with('[') {
            section = line;
            continue;
        }
        let Some((_, value)) = toml_pair(line, false).filter(|(k, _)| k == "name") else {
            continue;
        };
        if section == "[package]" {
            project.name = Some(value);
        } else if section == "[[bin]]" {
            names.push(value);
        }
    }
    project.package_manager = Some("cargo".into());
    names
}

fn python_scripts(
    text: Option<&str>,
    project: &mut Project,
    has: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    let mut names = vec![];
    let mut section = "";
    let mut tools = HashSet::new();
    for line in toml_lines(text) {
        if line.starts_with('[') {
            section = line;
            if section.starts_with("[tool.poetry") {
                tools.insert("poetry");
            }
            if section.starts_with("[tool.uv") {
                tools.insert("uv");
            }
            continue;
        }
        let Some((key, value)) = toml_pair(line, true) else {
            continue;
        };
        if section == "[project]" && key == "name" {
            project.name = Some(value);
        }
        if section == "[project.scripts]" || section == "[tool.poetry.scripts]" {
            names.push(key);
        }
    }
    let locks = [
        ("uv.lock", "uv"),
        ("poetry.lock", "poetry"),
        ("Pipfile.lock", "pipenv"),
        ("pdm.lock", "pdm"),
    ];
    let manager = locks
        .iter()
        .find(|l| has(l.0))
        .map(|l| l.1)
        .or_else(|| ["uv", "poetry"].into_iter().find(|t| tools.contains(t)));
    project.package_manager = Some(manager.unwrap_or("pip").to_string());
    names
}

/// Recipe names: `name args:` at the start of a line (`@name:` is a quiet recipe), not `x := y` or a setting.
fn just_recipes(text: Option<&str>) -> Vec<String> {
    let mut names = vec![];
    for line in text.unwrap_or("").lines().filter(|l| !l.is_empty()) {
        let Some(first) = line.chars().next() else {
            continue;
        };
        if !(first.is_alphabetic() || first == '_' || first == '@') || line.contains(":=") {
            continue;
        }
        let Some(colon) = line.find(':') else {
            continue;
        };
        let head = line[..colon].trim_start_matches('@');
        let Some(name) = head.split(' ').find(|w| !w.is_empty()) else {
            continue;
        };
        if !["set", "alias", "import", "mod", "export"].contains(&name) {
            names.push(name.to_string());
        }
    }
    names
}

/// Keys one level under the top-level `tasks:` of a Taskfile, read by indentation rather than a YAML parser.
fn taskfile_tasks(text: Option<&str>) -> Vec<String> {
    let mut names = vec![];
    let mut in_tasks = false;
    let mut indent = None;
    for line in text.unwrap_or("").lines().filter(|l| !l.trim().is_empty()) {
        let depth = line.chars().take_while(|c| *c == ' ').count();
        if depth == 0 {
            in_tasks = line.starts_with("tasks:");
            continue;
        }
        if !in_tasks || !line.ends_with(':') || line.contains('#') {
            continue;
        }
        if *indent.get_or_insert(depth) == depth {
            let name = line.trim();
            names.push(
                name[..name.len() - 1]
                    .trim()
                    .trim_matches(|c| c == '"' || c == '\'')
                    .to_string(),
            );
        }
    }
    names
}

/// Rule targets by simple parsing: `name:` at the start of a line, not an assignment, pattern or special target.
fn make_targets(text: Option<&str>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut names = vec![];
    for line in text.unwrap_or("").lines().filter(|l| !l.is_empty()) {
        if matches!(line.chars().next(), Some('\t' | '#' | '.' | ' ')) {
            continue;
        }
        let Some(colon) = line.find(':') else {
            continue;
        };
        let after = &line[colon + 1..];
        if after.starts_with('=') || after.starts_with(":=") {
            continue;
        }
        for word in line[..colon].split(' ').filter(|w| !w.is_empty()) {
            if word
                .chars()
                .all(|c| c.is_alphanumeric() || "_-./".contains(c))
                && seen.insert(word.to_string())
            {
                names.push(word.to_string());
            }
        }
    }
    names
}

impl GitStatus {
    pub fn clean(&self) -> bool {
        self.staged + self.unstaged + self.untracked + self.conflicted == 0
    }

    /// Header lines start with `# `; entries are `1`/`2` (changed; `XY` is index then work tree, `.` for none),
    /// `u` (unmerged) and `?` (untracked). Ignored entries (`!`) aren't requested.
    /// `fetched_at` is supplied explicitly so parsing is deterministic.
    pub fn parse(output: &str, fetched_at: f64) -> Self {
        let mut status = Self {
            oid: None,
            branch: None,
            upstream: None,
            ahead: 0,
            behind: 0,
            staged: 0,
            unstaged: 0,
            untracked: 0,
            conflicted: 0,
            fetched_at,
        };
        for line in output.lines().filter(|l| !l.is_empty()) {
            let mut rest = line.trim_start_matches(' ');
            let mut words = Vec::new();
            for _ in 0..2 {
                let (word, tail) = rest.split_once(' ').unwrap_or((rest, ""));
                if !word.is_empty() {
                    words.push(word);
                }
                rest = tail.trim_start_matches(' ');
            }
            if !rest.is_empty() {
                words.push(rest);
            }
            match line.chars().next() {
                Some('#') => {
                    if words.len() < 3 {
                        continue;
                    }
                    let key = words[1];
                    let value = words[2].to_string();
                    match key {
                        "branch.oid" => status.oid = (value != "(initial)").then_some(value),
                        "branch.head" => status.branch = (value != "(detached)").then_some(value),
                        "branch.upstream" => status.upstream = Some(value),
                        "branch.ab" => {
                            let parts: Vec<_> = value.split(' ').collect();
                            if parts.len() == 2 {
                                status.ahead =
                                    parts[0].get(1..).and_then(|s| s.parse().ok()).unwrap_or(0);
                                status.behind =
                                    parts[1].get(1..).and_then(|s| s.parse().ok()).unwrap_or(0);
                            }
                        }
                        _ => {}
                    }
                }
                Some('1' | '2') => {
                    let Some(codes) = words.get(1) else { continue };
                    let codes: Vec<_> = codes.chars().collect();
                    if codes.len() != 2 {
                        continue;
                    }
                    if codes[0] != '.' {
                        status.staged += 1;
                    }
                    if codes[1] != '.' {
                        status.unstaged += 1;
                    }
                }
                Some('u') => status.conflicted += 1,
                Some('?') => status.untracked += 1,
                _ => {}
            }
        }
        status
    }

    /// Runs Git without a shell or optional locks (so it never contends with the user's own Git), killed after
    /// `timeout` seconds. None on timeout or failure.
    /// Status can run repo-defined programs, so fsmonitor and submodules are off, and a repo with local or
    /// worktree `filter.*` config (clean/smudge programs) is skipped. Global filters such as LFS are fine.
    pub fn read(root: &str, timeout: f64) -> Option<Self> {
        let config = Self::run_git(
            &["-C", root, "config", "--show-scope", "--includes", "--list"],
            timeout,
        )?;
        let untrusted_filter = config.lines().any(|line| {
            let (scope, rest) = line.split_once('\t').unwrap_or_default();
            matches!(scope, "local" | "worktree") && rest.starts_with("filter.")
        });
        if untrusted_filter {
            return None;
        }
        Self::run_git(
            &[
                "-C",
                root,
                "-c",
                "core.fsmonitor=false",
                "status",
                "--porcelain=v2",
                "--branch",
                "--ignore-submodules=all",
            ],
            timeout,
        )
        .map(|output| Self::parse(&output, now()))
    }

    pub fn run_git(arguments: &[&str], timeout: f64) -> Option<String> {
        let mut command = Command::new("/usr/bin/env");
        command.args(["git", "--no-pager"]).args(arguments);
        for (key, _) in std::env::vars_os().filter(|(k, _)| k.to_string_lossy().starts_with("GIT_"))
        {
            command.env_remove(key);
        }
        command
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            // A partial clone must not lazy-fetch from status: that can run a repo-configured program.
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .stdout(Stdio::piped());
        let mut child = command.spawn().ok()?;
        let mut pipe = child.stdout.take()?;
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        // A thread of its own: the shared pool can be busy with callers blocked on this very read.
        std::thread::spawn(move || {
            let mut output = Vec::new();
            let result = pipe.read_to_end(&mut output).map(|_| output);
            let _ = sender.send(result);
        });
        let deadline = Instant::now()
            + Duration::from_secs_f64(if timeout.is_finite() {
                timeout.max(0.0)
            } else {
                0.0
            });
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        };
        // A daemon Git started (fsmonitor) can keep the pipe open; the output is complete once Git exits.
        let output = receiver.recv_timeout(Duration::from_secs(1)).ok()?.ok()?;
        status
            .success()
            .then(|| String::from_utf8_lossy(&output).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_kind_is_the_nearest_projects_primary_kind() {
        let root = tempfile_dir();
        let write = |file: &str| {
            let path = format!("{root}/{file}");
            std::fs::create_dir_all(parent(&path)).unwrap();
            std::fs::write(path, "{}").unwrap();
        };
        write("crate/Cargo.toml");
        write("crate/src/deep/x");
        write("crate/Makefile");
        write("mono/package.json");
        write("mono/pkg/Makefile");
        write("plain/x");
        write("mono/inner/.git/HEAD");
        write("mono/inner/src/x");
        let mut memo = HashMap::new();
        let kind =
            |memo: &mut HashMap<_, _>, d: &str| directory_kind_memo(memo, &format!("{root}/{d}"));
        assert_eq!(
            kind(&mut memo, "crate/src/deep").as_deref(),
            Some("rust"),
            "nested, and a language beats make"
        );
        assert_eq!(
            kind(&mut memo, "mono/pkg").as_deref(),
            Some("make"),
            "nearest project wins"
        );
        assert_eq!(kind(&mut memo, "mono").as_deref(), Some("node"));
        assert_eq!(
            kind(&mut memo, "gone/away"),
            None,
            "a missing directory is unknown"
        );
        assert_eq!(kind(&mut memo, "plain"), None, "no manifest is unknown");
        assert_eq!(memo.len(), 5);
        assert_eq!(
            kind(&mut memo, "mono/inner/src"),
            None,
            "a project above the Git root does not count"
        );
        // The cheap probe is exactly discover's first project kind.
        for d in [
            "crate/src/deep",
            "crate",
            "mono/pkg",
            "mono",
            "mono/inner/src",
            "plain",
            "gone/away",
        ] {
            let path = format!("{root}/{d}");
            let expected = Path::new(&path)
                .is_dir()
                .then(|| {
                    ProjectContext::discover(&path)
                        .projects
                        .first()
                        .map(|p| p.kind.clone())
                })
                .flatten();
            assert_eq!(directory_kind(&path), expected, "{d}");
        }
    }

    /// A symlinked directory is probed at its real path, so its kind equals discover's first kind for the link.
    #[test]
    fn symlinked_directory_probes_its_real_path() {
        let root = tempfile_dir();
        std::fs::create_dir_all(format!("{root}/real/crate/src")).unwrap();
        std::fs::write(
            format!("{root}/real/Cargo.toml"),
            "[package]\nname = \"x\"\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(format!("{root}/real/crate"), format!("{root}/link")).unwrap();
        let link = format!("{root}/link");
        let expected = ProjectContext::discover(&link)
            .projects
            .first()
            .map(|p| p.kind.clone());
        assert_eq!(directory_kind(&link), expected);
        assert_eq!(directory_kind(&link).as_deref(), Some("rust"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discover_projects_and_git_state() {
        let root = tempfile_dir();
        let write = |file: &str, text: &str| {
            let path = format!("{root}/{file}");
            std::fs::create_dir_all(parent(&path)).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(".git/HEAD", "ref: refs/heads/main\n");
        write(".git/rebase-merge/done", "x");
        write(
            "package.json",
            r#"{"name":"web","scripts":{"test":"x","dev":"y"}}"#,
        );
        write("yarn.lock", "");
        write("tools/Makefile", ".PHONY: a\nlint fmt:\n\techo\nX := 1\n");
        let context = ProjectContext::discover(&format!("{root}/tools"));
        let repository = context.repository.as_ref().unwrap();
        assert_eq!(repository.branch.as_deref(), Some("main"));
        assert_eq!(repository.operation.as_deref(), Some("rebase"));
        assert_eq!(context.projects.len(), 2);
        assert_eq!(context.projects[0].scripts["Makefile"], ["lint", "fmt"]);
        assert_eq!(context.projects[1].scripts["package.json"], ["dev", "test"]);
        assert_eq!(context.projects[1].package_manager.as_deref(), Some("yarn"));
        assert_eq!(context.repo_relative.as_deref(), Some("tools"));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Literal `name: "..."` on target-like constructors only. Dependencies, other constructors, and computed names stay out.
    #[test]
    fn swift_manifest_keeps_literal_target_names_only() {
        let root = tempfile_dir();
        std::fs::write(
            format!("{root}/Package.swift"),
            r#"
// swift-tools-version: 6.0
import PackageDescription
let libName = "Computed"
let package = Package(
    name: "api",
    dependencies: [
        .package(url: "https://github.com/example/CSQLite.git", from: "1.0.0"),
    ],
    targets: [
        .executableTarget(name: "App"),
        .target(name: "Lib", dependencies: [.product(name: "CSQLite", package: "CSQLite")]),
        .testTarget(name: "AppTests"),
        .plugin(name: "Gen"),
        .macro(
            name: "Str"
        ),
        .target(name: libName),
        .target(name: makeName("Nope")),
        .binaryTarget(name: "BinLib", path: "Bin.zip"),
        .systemLibrary(name: "CLib"),
    ]
)
"#,
        )
        .unwrap();
        let context = ProjectContext::discover(&root);
        let project = &context.projects[0];
        assert_eq!(project.kind, "swift");
        assert_eq!(
            project.scripts["Package.swift"],
            ["App", "Lib", "AppTests", "Gen", "Str"]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A FIFO named like a manifest must not block discovery; it is not a project.
    #[test]
    fn fifo_manifest_does_not_block() {
        let root = tempfile_dir();
        let manifest = format!("{root}/Cargo.toml");
        let c_path = std::ffi::CString::new(manifest).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o644) }, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        let dir = root.clone();
        std::thread::spawn(move || {
            tx.send(ProjectContext::discover(&dir).projects.len()).ok();
        });
        let count = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("FIFO Cargo.toml blocked discovery");
        assert_eq!(count, 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlinked_manifest_is_read() {
        let root = tempfile_dir();
        std::fs::write(
            format!("{root}/real.toml"),
            "[package]\nname = \"linked\"\n",
        )
        .unwrap();
        std::os::unix::fs::symlink("real.toml", format!("{root}/Cargo.toml")).unwrap();
        let project = &ProjectContext::discover(&root).projects[0];
        assert_eq!(project.kind, "rust");
        assert_eq!(project.name.as_deref(), Some("linked"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_porcelain() {
        let status = GitStatus::parse(
            "# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -3\n1 M. rest\n2 .M rest\nu UU rest\n? file\n! ignored\n1 broken\n",
            42.0,
        );
        assert_eq!(
            (
                status.ahead,
                status.behind,
                status.staged,
                status.unstaged,
                status.conflicted,
                status.untracked
            ),
            (2, 3, 1, 1, 1, 1)
        );
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.fetched_at, 42.0);
        assert!(!status.clean());
        let detached = GitStatus::parse("# branch.oid (initial)\n# branch.head (detached)\n", 0.0);
        assert!(detached.clean());
        assert_eq!((detached.oid, detached.branch), (None, None));
    }

    #[test]
    fn git_read_failure_and_timeout() {
        assert!(GitStatus::read("/smcli-does-not-exist", 1.0).is_none());
        assert!(GitStatus::run_git(&["--version"], 1.0).is_some());
        assert!(
            GitStatus::run_git(&["-c", "alias.smclisleep=!sleep 2", "smclisleep"], 0.01).is_none()
        );
    }

    fn git(root: &str, args: &[&str]) {
        let ok = Command::new("git")
            .args(["-C", root, "-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?} failed");
    }

    fn marker_script(root: &str) -> String {
        let script = format!("{root}/hook.sh");
        std::fs::write(&script, format!("#!/bin/sh\ntouch {root}/ran\ncat\n")).unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        script
    }

    #[test]
    fn git_status_ignores_repo_fsmonitor() {
        let root = tempfile_dir();
        git(&root, &["init", "-q"]);
        std::fs::write(format!("{root}/f"), "a").unwrap();
        git(&root, &["add", "f"]);
        git(&root, &["commit", "-qm", "x"]);
        let script = marker_script(&root);
        git(&root, &["config", "core.fsmonitor", &script]);
        assert!(GitStatus::read(&root, 5.0).is_some());
        assert!(!Path::new(&format!("{root}/ran")).exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn git_status_skips_repo_filters() {
        let root = tempfile_dir();
        git(&root, &["init", "-q"]);
        std::fs::write(format!("{root}/.gitattributes"), "f filter=evil\n").unwrap();
        std::fs::write(format!("{root}/f"), "a").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "x"]);
        let script = marker_script(&root);
        std::fs::write(
            format!("{root}/extra.cfg"),
            format!("[filter \"evil\"]\n\tclean = {script}\n"),
        )
        .unwrap();
        git(
            &root,
            &["config", "include.path", &format!("{root}/extra.cfg")],
        );
        std::fs::write(format!("{root}/f"), "b").unwrap(); // same size, dirty stat data
        assert!(GitStatus::read(&root, 5.0).is_none());
        assert!(!Path::new(&format!("{root}/ran")).exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn tempfile_dir() -> String {
        let path = std::env::temp_dir().join(format!(
            "smcli-context-{}-{}",
            std::process::id(),
            format!("{:?}", std::thread::current().id()).replace(['(', ')'], "")
        ));
        std::fs::create_dir_all(&path).unwrap();
        real_path(path.to_str().unwrap()).unwrap()
    }
}
