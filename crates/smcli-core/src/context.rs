use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

use regex::Regex;

/// What a directory is part of: its Git repository and worktree, and the projects around it. Discovery only
/// stats and reads small files (never runs Git, never executes a manifest), so it is cheap enough for the
/// typing path. The types are the service protocol's; `discover` leaves `status` unset.
// shortcut: no `GitStatus::read` (porcelain status via `git`); the ranking signals only need `operation`. Port with ContextCache.
pub use crate::service_protocol::{Project, ProjectContext, Repository};

/// The only package.json managers smcli builds commands for; anything else in the field is ignored.
pub const NODE_MANAGERS: [&str; 4] = ["npm", "pnpm", "yarn", "bun"];

/// Languages first, then task runners and containers, so a language decides the kind when there is one.
const MANIFESTS: [(&[&str], &str); 19] = [
    (&["Package.swift"], "swift"), (&["package.json"], "node"), (&["deno.json", "deno.jsonc"], "deno"),
    (&["Cargo.toml"], "rust"), (&["pyproject.toml"], "python"), (&["go.mod"], "go"), (&["Gemfile"], "ruby"),
    (&["composer.json"], "php"), (&["pom.xml"], "maven"), (&["build.gradle.kts", "build.gradle"], "gradle"),
    (&["mix.exs"], "elixir"), (&["pubspec.yaml"], "dart"), (&["CMakeLists.txt"], "cmake"),
    (&["Makefile", "makefile", "GNUmakefile"], "make"), (&["justfile", "Justfile", ".justfile"], "just"),
    (&["Taskfile.yml", "Taskfile.yaml"], "task"), (&["Rakefile"], "rake"),
    (&["compose.yaml", "compose.yml", "docker-compose.yml", "docker-compose.yaml"], "compose"), (&["Dockerfile"], "docker"),
];
const MAX_LEVELS: usize = 64;

fn exists(path: &str) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn parent(path: &str) -> String {
    match Path::new(path).parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_string_lossy().into_owned(),
        _ => path.to_string(),
    }
}

pub fn real_path(path: &str) -> Option<String> {
    std::fs::canonicalize(path).ok().map(|p| p.to_string_lossy().into_owned())
}

fn read_small_file(path: &str, limit: u64) -> Option<String> {
    let mut data = Vec::new();
    std::fs::File::open(path).ok()?.take(limit).read_to_end(&mut data).ok()?;
    Some(String::from_utf8_lossy(&data).trim().to_string())
}

fn relative(path: &str, root: &str) -> String {
    if path == root {
        return ".".into();
    }
    path.strip_prefix(&format!("{root}/")).unwrap_or(path).to_string()
}

impl ProjectContext {
    pub fn discover(directory: &str) -> Self {
        let mut context = Self { directory: directory.into(), ..Default::default() };
        let start = real_path(directory).unwrap_or_else(|| directory.to_string());
        let mut visited = vec![];
        let mut current = start.clone();
        for _ in 0..MAX_LEVELS {
            visited.push(current.clone());
            if let Some(repository) = Self::repository(&current) {
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
        context.repo_relative = context.repository.as_ref().map(|r| relative(&start, &r.root));
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
            if path.starts_with('/') { path.to_string() } else { format!("{directory}/{path}") }
        };
        let git_dir = real_path(&git_dir)?;
        let head = read_small_file(&format!("{git_dir}/HEAD"), 4096)?;
        let mut common_dir = git_dir.clone();
        if let Some(common) = read_small_file(&format!("{git_dir}/commondir"), 4096).filter(|c| !c.is_empty())
            && let Some(resolved) = real_path(&if common.starts_with('/') { common } else { format!("{git_dir}/{common}") }) {
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
            repository.branch = Some(reference.strip_prefix("refs/heads/").unwrap_or(reference).to_string());
        } else {
            repository.head = Some(head.chars().take(7).collect());
        }
        let markers = [("rebase-merge", "rebase"), ("rebase-apply/rebasing", "rebase"), ("rebase-apply", "am"),
                       ("MERGE_HEAD", "merge"), ("CHERRY_PICK_HEAD", "cherry-pick"), ("REVERT_HEAD", "revert"),
                       ("BISECT_LOG", "bisect")];
        repository.operation = markers.iter().find(|m| exists(&format!("{git_dir}/{}", m.0))).map(|m| m.1.to_string());
        Some(repository)
    }

    fn project(directory: &str) -> Option<Project> {
        let has = |file: &str| exists(&format!("{directory}/{file}"));
        let found: Vec<(&str, &str)> =
            MANIFESTS.iter().filter_map(|(names, kind)| names.iter().find(|n| has(n)).map(|n| (*n, *kind))).collect();
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
                "swift" => matches(r#"\.(?:executableTarget|target|testTarget|plugin|macro)\(\s*name:\s*"([^"]+)""#, text),
                "node" => node_scripts(text, &mut project, &has),
                "rust" => cargo_targets(text, &mut project),
                "python" => python_scripts(text, &mut project, &has),
                "go" => {
                    if project.name.is_none() {
                        project.name = text.and_then(|t| t.lines().find(|l| l.starts_with("module "))).map(|l| l[7..].trim().to_string());
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
                project.scripts.insert(file.to_string(), names.into_iter().take(200).collect());
            }
        }
        // Wrappers pin the build tool's version, so commands use them when present.
        project.manifests.extend(["gradlew", "mvnw"].into_iter().filter(|f| has(f)).map(String::from));
        if project.package_manager.is_none() {
            project.package_manager = match primary.1 {
                "swift" => Some("swiftpm"), "rust" => Some("cargo"), "go" => Some("go"), "make" => Some("make"),
                "ruby" => Some("bundler"), "php" => Some("composer"), "elixir" => Some("mix"), "deno" => Some("deno"),
                _ => None,
            }
            .map(String::from);
        }
        Some(project)
    }
}

fn matches(pattern: &str, text: Option<&str>) -> Vec<String> {
    let Ok(expression) = Regex::new(pattern) else { return vec![] };
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
    let mut keys: Vec<String> =
        json_object(text).and_then(|j| j.get(field)?.as_object().map(|o| o.keys().cloned().collect())).unwrap_or_default();
    keys.sort();
    keys
}

fn node_scripts(text: Option<&str>, project: &mut Project, has: &dyn Fn(&str) -> bool) -> Vec<String> {
    let json = json_object(text);
    project.name = json.as_ref().and_then(|j| j.get("name")?.as_str().map(String::from));
    // The field is repository-controlled text that ends up in suggested commands, so only known names count.
    let field = json.as_ref().and_then(|j| j.get("packageManager")?.as_str()).and_then(|f| f.split('@').next());
    project.package_manager = Some(if let Some(name) = field.filter(|n| NODE_MANAGERS.contains(n)) {
        name.to_string()
    } else {
        [("pnpm-lock.yaml", "pnpm"), ("yarn.lock", "yarn"), ("bun.lock", "bun"), ("bun.lockb", "bun"), ("package-lock.json", "npm")]
            .iter()
            .find(|l| has(l.0))
            .map_or("npm", |l| l.1)
            .to_string()
    });
    json_keys(text, "scripts")
}

fn toml_lines(text: Option<&str>) -> Vec<&str> {
    text.unwrap_or("").lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect()
}

/// `key = "value"`; with `any_value`, a key with any value (script tables map names to entry points).
fn toml_pair(line: &str, any_value: bool) -> Option<(String, String)> {
    let equals = line.find('=')?;
    let key = line[..equals].trim().trim_matches(|c| c == '"' || c == '\'').to_string();
    let value = line[equals + 1..].trim();
    if let Some(rest) = value.strip_prefix('"')
        && let Some(end) = rest.find('"') {
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
        let Some((_, value)) = toml_pair(line, false).filter(|(k, _)| k == "name") else { continue };
        if section == "[package]" {
            project.name = Some(value);
        } else if section == "[[bin]]" {
            names.push(value);
        }
    }
    project.package_manager = Some("cargo".into());
    names
}

fn python_scripts(text: Option<&str>, project: &mut Project, has: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut names = vec![];
    let mut section = "";
    let mut tools = HashSet::new();
    for line in toml_lines(text) {
        if line.starts_with('[') {
            section = line;
            if section.starts_with("[tool.poetry") { tools.insert("poetry"); }
            if section.starts_with("[tool.uv") { tools.insert("uv"); }
            continue;
        }
        let Some((key, value)) = toml_pair(line, true) else { continue };
        if section == "[project]" && key == "name" {
            project.name = Some(value);
        }
        if section == "[project.scripts]" || section == "[tool.poetry.scripts]" {
            names.push(key);
        }
    }
    let locks = [("uv.lock", "uv"), ("poetry.lock", "poetry"), ("Pipfile.lock", "pipenv"), ("pdm.lock", "pdm")];
    let manager = locks.iter().find(|l| has(l.0)).map(|l| l.1).or_else(|| ["uv", "poetry"].into_iter().find(|t| tools.contains(t)));
    project.package_manager = Some(manager.unwrap_or("pip").to_string());
    names
}

/// Recipe names: `name args:` at the start of a line (`@name:` is a quiet recipe), not `x := y` or a setting.
fn just_recipes(text: Option<&str>) -> Vec<String> {
    let mut names = vec![];
    for line in text.unwrap_or("").lines().filter(|l| !l.is_empty()) {
        let Some(first) = line.chars().next() else { continue };
        if !(first.is_alphabetic() || first == '_' || first == '@') || line.contains(":=") {
            continue;
        }
        let Some(colon) = line.find(':') else { continue };
        let head = line[..colon].trim_start_matches('@');
        let Some(name) = head.split(' ').find(|w| !w.is_empty()) else { continue };
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
            names.push(name[..name.len() - 1].trim().trim_matches(|c| c == '"' || c == '\'').to_string());
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
        let Some(colon) = line.find(':') else { continue };
        let after = &line[colon + 1..];
        if after.starts_with('=') || after.starts_with(":=") {
            continue;
        }
        for word in line[..colon].split(' ').filter(|w| !w.is_empty()) {
            if word.chars().all(|c| c.is_alphanumeric() || "_-./".contains(c)) && seen.insert(word.to_string()) {
                names.push(word.to_string());
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

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
        write("package.json", r#"{"name":"web","scripts":{"test":"x","dev":"y"}}"#);
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

    fn tempfile_dir() -> String {
        let path = std::env::temp_dir().join(format!("smcli-context-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&path).unwrap();
        real_path(path.to_str().unwrap()).unwrap()
    }
}
