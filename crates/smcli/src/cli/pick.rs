use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use smcli_core::caller::CallerContext;
use smcli_core::config::{CaptureFilter, SmcliConfig};
use smcli_core::context::ProjectContext;
use smcli_core::history::{CapturedHistory, user_name};
use smcli_core::paths::relative_insert;
use smcli_core::ranking::RankingContext;
use smcli_core::service_client::ServiceClient;
use smcli_core::service_protocol::{FileHit, Operation, SearchFilter, ServiceRequest};
use smcli_core::time::now;
use smcli_search::lexical_search;
use smcli_search::setup::{corpus, search_signals, working_directory};

use crate::picker::{FileResults, Mode};

const DIRECTORY_COMMANDS: &[&str] = &["cd", "pushd", "popd", "z", "rmdir", "mkdir"];

/// Fallback grep skips files past the 10 MiB cap the service's grep enforces (`grep_options`).
const MAX_GREP_FILE_BYTES: u64 = 10 << 20;

pub(super) fn is_directory_command(command: &str) -> bool {
    DIRECTORY_COMMANDS.contains(&command)
}

fn files_kind(c: &CapturedHistory, env: &HashMap<String, String>) -> u8 {
    let line = env.get("SMCLI_LBUFFER").map(String::as_str).unwrap_or("");
    let Some(command) = smcli_core::parse::last_command_word(line, env.get("SMCLI_SHELL").map(String::as_str).unwrap_or("bash")) else { return 0 };
    let command = command.rsplit('/').next().unwrap_or(&command).to_string();
    if is_directory_command(&command) { return 1; }
    // Pre-C files_majority take(20) was on this same filtered stream, so the SQL cursor stops there too.
    let Ok(rows) = c.file_kind_rows(&user_name(), 20, |raw| {
        smcli_core::parse::program_of(raw, smcli_core::parse::CommandBreakdown::parse(raw).as_ref()).as_deref() == Some(command.as_str())
    }) else { return 0 };
    files_majority(rows.into_iter().map(|(raw, expanded, cwd, shell)| {
        let (mut dirs, mut files) = (0, 0);
        let command = expanded.as_deref().unwrap_or(&raw);
        for path in smcli_core::parse::path_arguments(command, &shell, |p| Path::new(&cwd).join(p).exists()) {
            let full = Path::new(&cwd).join(path);
            dirs += usize::from(full.is_dir());
            files += usize::from(full.is_file());
        }
        (dirs, files)
    }))
}

fn files_majority(rows: impl IntoIterator<Item = (usize, usize)>) -> u8 {
    let (dirs, files) = rows.into_iter().take(20).fold((0, 0), |(d, f), (dirs, files)| (d + dirs, f + files));
    if dirs > files { 1 } else if files > dirs { 2 } else { 0 }
}


pub(super) fn pick(
    c: &CapturedHistory,
    service: &ServiceClient,
    home: &Path,
    env: &HashMap<String, String>,
    suggest: bool,
    files: bool,
    grep: bool,
    query: String,
) -> Result<()> {
    use crate::picker::{Choice, FileResults, Mode, Picker};
    let caller = CallerContext::from_env(env);
    let d = working_directory(env);
    let context = RankingContext::new(
        &ProjectContext::discover(&d),
        caller.session.clone(),
        caller.ranking_off.as_deref(),
    );
    let mut request = ServiceRequest::new(Operation::Pick);
    request.directory = Some(d.clone());
    request.limit = Some(50);
    request.session = env.get("SMCLI_SESSION").cloned();
    let served = service
        .send(&request, None, Duration::from_secs(10))
        .filter(|r| r.ok);
    let suggestions = match served.as_ref().and_then(|r| r.suggestions.clone()) {
        Some(s) => s,
        None => c.suggestions(&user_name(), &d, "", 50, now(), Some(&context))?,
    }
    .into_iter()
    .map(|s| s.key())
    .collect();
    let user = user_name();
    let recent = match served.and_then(|r| r.recent_entries) {
        Some(r) => r.into_iter().map(|entry| entry.key()).collect(),
        None => c.recent_distinct(&user)?,
    };
    let native = smcli_search::native::history_files(caller.home(), &caller.native_env())?
        .iter()
        .filter_map(|f| f.read().ok())
        .flat_map(|s| s.lines().map(String::from).collect::<Vec<_>>())
        .collect();
    let mut picker = Picker::new(
        suggestions,
        recent,
        native,
        // Ctrl-T's query is the whole command line, so files mode starts empty unless `@` completion names a word.
        if files { env.get("SMCLI_FILES_QUERY").cloned().unwrap_or_default() } else if grep { String::new() } else { query },
    );
    picker.suggested_view = suggest;
    picker.title = Some(format!("Suggested for {d}"));
    if files {
        picker.kind = files_kind(c, env);
        picker.files_line = env.get("SMCLI_FILES_LINE").cloned();
    }
    picker.mode = if files {
        Mode::Files
    } else if grep {
        Mode::Grep
    } else {
        Mode::History
    };
    let project = ProjectContext::discover(&d);
    let root = project
        .repository
        .as_ref()
        .map(|r| r.root.clone())
        .unwrap_or_else(|| d.clone());
    picker.scope = Some(root.clone());
    let search = {
        let c = CapturedHistory::new(c.path.clone());
        let env = env.clone();
        let d = d.clone();
        let context = context.clone();
        let caller = caller.clone();
        let service = ServiceClient::new(&c.path, &env, &std::env::current_exe()?);
        move |q: &str, _: bool| -> Result<Vec<String>> {
        let mut r = ServiceRequest::new(Operation::Search);
        r.query = Some(q.into());
        r.directory = Some(d.clone());
        r.limit = Some(100);
        r.session = caller.session.clone();
        r.histfile = caller.histfile.clone();
        r.login_shell = caller.shell.clone();
        let hits = match service
            .send(&r, None, Duration::from_secs(600))
            .filter(|r| r.ok)
            .and_then(|r| r.hits)
        {
            Some(h) => h,
            None => {
                let native = caller.native_env();
                let entries = corpus(&c, caller.home(), &native, context.project_root.as_deref(), context.kind(), now())?;
                let signals = search_signals(&c, Some(context.clone()), &native)?;
                lexical_search(q, &entries, 100, &SearchFilter::default(), &signals, now())?
            }
        };
        Ok(hits.into_iter().map(|h| h.key()).collect())
        }
    };
    let file_search = {
        let cwd = d.clone();
        let service = ServiceClient::new(&c.path, env, &std::env::current_exe()?);
        let fallback = std::cell::OnceCell::new();
        move |mode: Mode, q: &str, kind: u8, scope: &str, path: Option<&str>| -> Result<FileResults> {
        let mut r = ServiceRequest::new(if mode == Mode::Grep {
            Operation::Grep
        } else {
            Operation::Files
        });
        r.directory = Some(scope.into());
        r.query = Some(q.into());
        r.limit = Some(100);
        r.scope = Some(true);
        r.cwd = Some(cwd.clone());
        r.path = path.map(String::from);
        let response = service
            .send(&r, None, Duration::from_secs(10));
        if let Some(response) = response.filter(|r| r.ok) {
            return Ok(FileResults {
                hits: response.files.unwrap_or_default(),
                complete: !response.partial.unwrap_or(false),
            });
        }
        let local = fallback.get_or_init(|| LocalFileSearch { cwd: cwd.clone().into() });
        local.search(mode, q, kind, scope, path)
        }
    };
    let config = SmcliConfig::load(&SmcliConfig::location(home, env)).unwrap_or_default();
    let inline = config.at_completion_inline != Some(false);
    picker.ls_colors = config.at_completion_ls_colors == Some(true);
    if let Some(choice) = crate::picker::run(picker, Some(Box::new(search)), Some(Box::new(file_search)), inline)? {
        match choice {
            Choice::Command {
                command,
                query,
                semantic,
                edit_paths,
            } => {
                let mut r = ServiceRequest::new(Operation::Select);
                let (typed, expanded) = smcli_core::entry::command_parts(&command);
                r.command = Some(typed);
                r.expanded_command = expanded;
                r.query = Some(query.clone());
                r.directory = Some(d.clone());
                r.mode = Some(if semantic { "search" } else { "suggest" }.into());
                if !service
                    .send(&r, None, Duration::from_secs(10))
                    .is_some_and(|r| r.ok)
                {
                    // Ranking persistence must never prevent insertion of the chosen command.
                    let _ = (|| -> Result<()> {
                        let filter = CaptureFilter::new(&SmcliConfig::load(
                            &SmcliConfig::location(home, env),
                        )?)?;
                        let db = c.open_connection()?;
                        c.record_pick(
                            &query.chars().take(1000).collect::<String>(),
                            &command,
                            if semantic { "search" } else { "suggest" },
                            &d,
                            &filter,
                            now(),
                            &db,
                        )?;
                        Ok(())
                    })();
                }
                let typed = crate::picker::history_insert(&command);
                if edit_paths {
                    let shell = env.get("SMCLI_SHELL").map(String::as_str).unwrap_or("bash");
                    let dir = c.last_directory(&command, &user).unwrap_or_else(|| d.clone());
                    if let Some((rewritten, pos)) = smcli_core::parse::path_placeholders(&typed, shell, |path| {
                        std::path::Path::new(&dir).join(path).exists()
                    }) {
                        print!("{}\x1f{}", &rewritten[..pos], &rewritten[pos + 1..]);
                        std::process::exit(11);
                    }
                }
                println!("{typed}");
            }
            Choice::File { hit, mode, query, scope } => {
                if hit.provenance != ["*"] {
                    let r = file_selection_request(scope, hit.path.clone(), mode, query);
                    let _ = service.send(&r, None, Duration::from_secs(2));
                }
                let shell = env.get("SMCLI_SHELL").map(String::as_str).unwrap_or("bash");
                let before = env.get("SMCLI_LBUFFER");
                if !before.is_some_and(|b| crate::shell_quoting::insertable(b, shell)) {
                    eprintln!(
                        "smcli: not inserting a path inside an open quote or substitution, or after here-document syntax; finish the command first{}.",
                        if before.is_none() { " (or reload smcli's shell-init)" } else { "" }
                    );
                    std::process::exit(1);
                }
                println!("{}", crate::shell_quoting::quote(&hit.insert, shell));
                std::process::exit(10);
            }
        }
    } else {
        std::process::exit(1);
    }
    Ok(())
}

fn file_selection_request(scope: String, path: String, mode: Mode, query: String) -> ServiceRequest {
    let mut request = ServiceRequest::new(Operation::Select);
    request.directory = Some(scope);
    request.path = Some(path);
    request.mode = Some(mode.as_str().into());
    request.query = Some(query);
    request.scope = Some(true);
    request
}

struct LocalFileSearch {
    cwd: PathBuf,
}

impl LocalFileSearch {
    fn search(&self, mode: Mode, query: &str, kind: u8, scope: &str, path: Option<&str>) -> Result<FileResults> {
        let scope = Path::new(scope).canonicalize()?;
        let scope_str = scope.to_string_lossy().into_owned();
        let cwd_str = self.cwd.to_string_lossy().into_owned();
        let mut hits = Vec::new();
        match mode {
            Mode::Files => {
                let mut entries = Vec::new();
                let mut budget = 10_000;
                self.walk(&scope, 0, &mut budget, &mut |p, is_dir| {
                    let rel = p.strip_prefix(&scope).unwrap().to_string_lossy().into_owned();
                    if (query.is_empty() || rel.to_lowercase().contains(&query.to_lowercase()))
                        && (kind == 0 || is_dir == (kind == 1))
                    {
                        entries.push((rel, is_dir));
                    }
                });
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                entries.truncate(100);
                for (rel, is_dir) in entries {
                    let rel = if is_dir { format!("{rel}/") } else { rel };
                    hits.push(self.file_hit(&rel, &scope_str, &cwd_str, "files"));
                }
            }
            Mode::Grep => {
                let target = path.map(|p| scope.join(p));
                let sources: Vec<PathBuf> = if let Some(target) = target {
                    vec![target]
                } else {
                    let mut files = Vec::new();
                    let mut budget = 10_000;
                    self.walk(&scope, 0, &mut budget, &mut |p, is_dir| {
                        if !is_dir { files.push(p.to_path_buf()); }
                    });
                    files
                };
                for file in sources {
                    let rel = file.strip_prefix(&scope).unwrap().to_string_lossy().into_owned();
                    if file.metadata().map(|m| m.len() > MAX_GREP_FILE_BYTES).unwrap_or(true) { continue; }
                    let text = match std::fs::File::open(&file) {
                        Ok(f) => {
                            use std::io::Read as _;
                            let mut text = String::new();
                            // One byte past the cap detects a file that grew after the size check; skip it whole.
                            match f.take(MAX_GREP_FILE_BYTES + 1).read_to_string(&mut text) {
                                Ok(n) if n as u64 <= MAX_GREP_FILE_BYTES => text,
                                _ => continue,
                            }
                        }
                        Err(_) => continue,
                    };
                    for (i, line) in text.lines().enumerate() {
                        if let Some(pos) = line.to_lowercase().find(&query.to_lowercase()) {
                            let mut hit = self.file_hit(&rel, &scope_str, &cwd_str, "grep");
                            hit.line = Some((i + 1) as i64);
                            hit.column = Some((pos + 1) as i64);
                            hit.text = Some(line.into());
                            hits.push(hit);
                            if hits.len() >= 100 { break; }
                        }
                    }
                    if hits.len() >= 100 { break; }
                }
            }
            _ => {}
        }
        Ok(FileResults { hits, complete: true })
    }

    fn walk(&self, dir: &Path, depth: usize, budget: &mut usize, on_entry: &mut dyn FnMut(&Path, bool)) {
        if depth >= 32 || *budget == 0 { return; }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries {
            if *budget == 0 { break; }
            *budget -= 1;
            let Ok(e) = entry else { continue };
            let path = e.path();
            if path.file_name().is_some_and(|n| n == ".git") { continue; }
            let Ok(file_type) = e.file_type() else { continue };
            if file_type.is_dir() {
                on_entry(&path, true);
                self.walk(&path, depth + 1, budget, on_entry);
            } else if file_type.is_file() {
                on_entry(&path, false);
            }
        }
    }

    fn file_hit(&self, path: &str, scope: &str, cwd: &str, provenance: &str) -> FileHit {
        FileHit {
            insert: relative_insert(path, scope, cwd),
            path: path.into(),
            provenance: vec![provenance.into()],
            git: None,
            line: None,
            column: None,
            text: None,
            definition: None,
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn files_filter_uses_recent_captured_paths() {
        let root = std::env::temp_dir().join(format!("smcli-files-kind-{}", std::process::id()));
        std::fs::create_dir_all(root.join("folder")).unwrap();
        std::fs::write(root.join("file.txt"), "").unwrap();
        let history = super::CapturedHistory::new(root.join("history.db"));
        let env = std::collections::HashMap::from([("SMCLI_LBUFFER".into(), "echo hi; tool @".into())]);
        assert_eq!(super::files_kind(&history, &env), 0);
        let db = history.open_connection().unwrap();
        for i in 0..21 {
            let command = if i == 0 { "tool folder folder folder" } else { "tool file.txt" };
            let mut run = smcli_core::history::NewRun::new(command, "bash", root.to_str().unwrap(), 0);
            run.started_at = Some(1_700_000_000.0 + i as f64);
            history.append(&run, "", Some(&db)).unwrap();
        }
        assert_eq!(super::files_kind(&history, &env), 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn files_start_filter() {
        for command in ["cd", "pushd", "popd", "z", "rmdir", "mkdir"] {
            assert!(super::is_directory_command(command));
        }
        assert!(!super::is_directory_command("cat"));
        assert_eq!(super::files_majority([]), 0);
        assert_eq!(super::files_majority([(2, 2)]), 0);
        assert_eq!(super::files_majority([(2, 1)]), 1);
        assert_eq!(super::files_majority([(1, 2)]), 2);
        assert_eq!(super::files_majority(std::iter::repeat_n((1, 0), 20).chain([(0, 100)])), 1);
    }

}

#[cfg(test)]
mod walker_tests {
    use super::*;
    use crate::picker::Mode;

    #[test]
    fn fallback_walker_skips_directory_symlink_cycles_within_budget() -> Result<()> {
        let root = std::env::temp_dir().join(format!("smcli-walk-cycle-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("nested"))?;
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&root, root.join("nested/back-one"))?;
            std::os::unix::fs::symlink(&root, root.join("nested/back-two"))?;
        }
        let search = LocalFileSearch { cwd: root.clone() };
        let result = search.search(Mode::Files, "", 0, root.to_str().unwrap(), None)?;
        assert!(result.hits.iter().any(|hit| hit.path == "nested/"));
        assert!(!result.hits.iter().any(|hit| hit.path.contains("back-one") || hit.path.contains("back-two")));
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn fallback_walker_obeys_entry_budget() -> Result<()> {
        let root = std::env::temp_dir().join(format!("smcli-walk-budget-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        for name in ["one", "two", "three"] { std::fs::write(root.join(name), "")?; }
        let search = LocalFileSearch { cwd: root.clone() };
        let mut budget = 1;
        let mut seen = Vec::new();
        search.walk(&root, 0, &mut budget, &mut |path, _| seen.push(path.to_path_buf()));
        assert_eq!(seen.len(), 1);
        assert_eq!(budget, 0);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn fallback_walker_obeys_depth_limit() -> Result<()> {
        let root = std::env::temp_dir().join(format!("smcli-walk-depth-{}", uuid::Uuid::new_v4()));
        let mut leaf = root.clone();
        for _ in 0..33 { leaf.push("d"); }
        std::fs::create_dir_all(&leaf)?;
        let search = LocalFileSearch { cwd: root.clone() };
        let mut budget = 10_000;
        let mut deepest = 0;
        search.walk(&root, 0, &mut budget, &mut |path, _| {
            deepest = deepest.max(path.strip_prefix(&root).unwrap().components().count());
        });
        assert_eq!(deepest, 32);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn fallback_folder_picker_lists_empty_directories() -> Result<()> {
        let root = std::env::temp_dir().join(format!("smcli-walk-empty-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("empty"))?;
        let search = LocalFileSearch { cwd: root.clone() };
        std::fs::write(root.join("visible.txt"), "file")?;
        let result = search.search(Mode::Files, "", 1, root.to_str().unwrap(), None)?;
        assert!(result.hits.iter().any(|hit| hit.path == "empty/"));
        let mixed = search.search(Mode::Files, "", 0, root.to_str().unwrap(), None)?;
        assert!(mixed.hits.iter().any(|hit| hit.path == "visible.txt"));
        assert!(mixed.hits.iter().any(|hit| hit.path == "empty/"));
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn fallback_grep_skips_files_over_the_size_limit() -> Result<()> {
        let root = std::env::temp_dir().join(format!("smcli-grep-limit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        std::fs::write(root.join("small.txt"), "needle")?;
        std::fs::write(root.join("big.txt"), "needle")?;
        std::fs::File::options().write(true).open(root.join("big.txt"))?.set_len(MAX_GREP_FILE_BYTES + (1 << 20))?;
        let search = LocalFileSearch { cwd: root.clone() };
        let result = search.search(Mode::Grep, "needle", 0, root.to_str().unwrap(), None)?;
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].path, "small.txt");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn scoped_file_selection_request_sets_scope() {
        let request = file_selection_request("/repo/sub".into(), "needle.rs".into(), Mode::Files, "needle".into());
        assert_eq!(request.scope, Some(true));
        assert_eq!(request.directory.as_deref(), Some("/repo/sub"));
        assert_eq!(request.path.as_deref(), Some("needle.rs"));
    }

}
