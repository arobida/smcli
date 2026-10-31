use std::{collections::HashMap, panic::{catch_unwind, AssertUnwindSafe}, path::PathBuf, sync::{Arc, Condvar, Mutex}, time::{Duration, Instant}};
use anyhow::{Result, anyhow};
use fff_search::{FilePicker, FilePickerOptions, SharedFilePicker, SharedFrecency, QueryParser, FuzzySearchOptions, PaginationArgs, GrepSearchOptions, DirSearchConfig, MixedSearchConfig, Constraint};
use smcli_core::{context::ProjectContext, service_location::relative_insert, service_protocol::{FileHit, FileIndexInfo, ServiceRequest, ServiceResponse}};

type Scores = Box<dyn Fn(&str) -> HashMap<String, f64> + Send + Sync>;
type Record = Box<dyn Fn(&str, &str, &str, &str, &ServiceRequest) -> Result<()> + Send + Sync>;
/// Owns one fff instance for one root. An evicted index lives until its last request ends.
pub(crate) struct OwnedIndex(SharedFilePicker);
impl std::ops::Deref for OwnedIndex {
    type Target = SharedFilePicker;
    fn deref(&self) -> &Self::Target { &self.0 }
}
impl Drop for OwnedIndex {
    fn drop(&mut self) {
        self.0.shutdown_watches_and_wait();
        if let Ok(mut guard) = self.0.write() { guard.take(); }
    }
}
pub(crate) type Index = Arc<OwnedIndex>;
type Pending = Arc<(Mutex<Option<Result<Index, String>>>, Condvar)>;
struct Entry { id: i64, index: Index, used: Instant }
#[derive(Default)]
struct Pool { entries: HashMap<String, Entry>, pending: HashMap<String, Pending>, building: usize, next: i64 }

/// `files`, `grep` and `select` of a file: fff searches over one instance per root, with the root and Git worktree taken
/// from the directory context. Runs on the caller's connection thread, never on the state queue.
pub struct FileSearch {
    capacity: usize,
    idle: Duration,
    pool: Mutex<Pool>,
    available: Condvar,
    /// Decayed selection counts per path for a root; reads the history database on the service's state queue.
    scores: Scores,
    record: Record,
    home: PathBuf,
}
#[path = "evidence.rs"]
pub(crate) mod evidence;

impl FileSearch {
    pub fn new(capacity: usize, idle: Duration, scores: Scores, record: Record, home: PathBuf) -> Self {
        Self { capacity: capacity.max(1), idle, pool: Mutex::new(Pool::default()), available: Condvar::new(), scores, record, home }
    }

    pub(crate) fn capacity(&self) -> usize { self.capacity }

    /// The directory's Git worktree, or the directory itself outside Git. Real paths, so symlinks don't split instances.
    pub(crate) fn root(directory: &str) -> Option<String> {
        ProjectContext::discover(directory).repository.map(|r| r.root).or_else(|| std::fs::canonicalize(directory).ok().map(|p| p.to_string_lossy().into_owned()))
    }

    pub(crate) fn ready(&self, root: &str) -> Option<Index> {
        let mut pool = self.pool.lock().unwrap();
        let entry = pool.entries.get_mut(root)?;
        if entry.used.elapsed() > self.idle { return None; }
        entry.used = Instant::now();
        let index = entry.index.clone();
        drop(pool);
        let guard = index.read().ok()?;
        if guard.as_ref()?.is_scan_active() { return None; }
        drop(guard);
        Some(index)
    }

    /// The instance for `root`, created (and the least recently used one evicted) when needed.
    /// Creation waits for the first scan, so it happens outside the lock: other roots stay responsive.
    pub(crate) fn index(&self, root: &str) -> Result<(Index, i64)> {
        self.index_with_content(root, true)
    }

    fn index_with_content(&self, root: &str, enable_content_indexing: bool) -> Result<(Index, i64)> {
        loop {
            let mut pool = self.pool.lock().unwrap();
            pool.entries.retain(|_, e| e.used.elapsed() <= self.idle);
            if let Some(e) = pool.entries.get_mut(root) { e.used = Instant::now(); return Ok((e.index.clone(), e.id)); }
            if let Some(pending) = pool.pending.get(root).cloned() {
                drop(pool);
                let (lock, ready) = &*pending;
                let mut result = lock.lock().unwrap();
                while result.is_none() { result = ready.wait(result).unwrap(); }
                if let Err(error) = result.as_ref().unwrap() { return Err(anyhow!(error.clone())); }
                continue;
            }
            if pool.building >= self.capacity { drop(self.available.wait(pool).unwrap()); continue; }
            let pending = Arc::new((Mutex::new(None), Condvar::new()));
            pool.pending.insert(root.into(), pending.clone());
            pool.building += 1;
            drop(pool);
            // No LMDB stores: frecency lives in smcli's own database. The watcher keeps the index current.
            let result = catch_unwind(AssertUnwindSafe(|| -> Result<Index> {
                if root.contains('\0') { return Err(anyhow!("NUL in root")); }
                let picker = SharedFilePicker::default();
                FilePicker::new_with_shared_state(picker.clone(), SharedFrecency::default(), FilePickerOptions { base_path: root.into(), enable_content_indexing, ..Default::default() })?;
                // A slow scan is not an error: search what is indexed so far.
                if picker.wait_for_scan(Duration::from_secs(10)) { picker.wait_for_watcher(Duration::from_secs(10)); }
                Ok(Arc::new(OwnedIndex(picker)))
            }))
            .unwrap_or_else(|_| Err(anyhow!("file index build panicked")))
            .map_err(|e| e.to_string());
            let mut pool = self.pool.lock().unwrap();
            pool.building -= 1;
            pool.pending.remove(root);
            let answer = result.clone().map(|index| {
                while pool.entries.len() >= self.capacity {
                    let oldest = pool.entries.iter().min_by_key(|(_, e)| e.used).unwrap().0.clone();
                    pool.entries.remove(&oldest);
                }
                pool.next += 1;
                let id = pool.next;
                pool.entries.insert(root.into(), Entry { id, index: index.clone(), used: Instant::now() });
                (index, id)
            });
            *pending.0.lock().unwrap() = Some(result);
            pending.1.notify_all();
            self.available.notify_all();
            return answer.map_err(|e| anyhow!(e));
        }
    }

    fn ready_with_id(&self, root: &str) -> Option<(Index, i64)> {
        let mut pool = self.pool.lock().unwrap();
        let entry = pool.entries.get_mut(root)?;
        if entry.used.elapsed() > self.idle { return None; }
        entry.used = Instant::now();
        let index = entry.index.clone();
        let id = entry.id;
        drop(pool);
        let guard = index.read().ok()?;
        if guard.as_ref()?.is_scan_active() { return None; }
        drop(guard);
        Some((index, id))
    }

    pub fn files(&self, request: &ServiceRequest) -> ServiceResponse { self.search(request, false) }
    pub fn grep(&self, request: &ServiceRequest) -> ServiceResponse { self.search(request, true) }

    fn content_indexing_enabled(&self, scope: &str) -> bool {
        let in_repo = ProjectContext::discover(scope).repository.is_some();
        if in_repo { return true; }
        let Ok(scope) = std::fs::canonicalize(scope) else { return true };
        let home = std::fs::canonicalize(&self.home).unwrap_or_else(|_| self.home.clone());
        let Some(rest) = scope.strip_prefix(&home).ok().and_then(|p| p.to_str()) else { return false };
        rest.contains('/')
    }

    fn scoped_index(&self, scope: &str) -> Result<(Index, i64, Option<String>)> {
        let pool = self.pool.lock().unwrap();
        let mut roots: Vec<String> = pool.entries.keys()
            .filter(|root| *root == scope || scope.starts_with(&format!("{root}/")))
            .cloned()
            .collect();
        drop(pool);
        roots.sort_by_key(|root| std::cmp::Reverse(root.len()));
        for root in roots {
            if let Some((index, id)) = self.ready_with_id(&root) {
                let prefix = if root == scope { None } else {
                    let rest = scope.strip_prefix(&root).unwrap().trim_start_matches('/');
                    if rest.is_empty() { None } else { Some(format!("{rest}/")) }
                };
                return Ok((index, id, prefix));
            }
        }
        let (index, id) = self.index_with_content(scope, self.content_indexing_enabled(scope))?;
        Ok((index, id, None))
    }

    fn search(&self, request: &ServiceRequest, grep: bool) -> ServiceResponse {
        let name = if grep { "grep" } else { "files" };
        let (Some(directory), Some(query)) = (request.directory.as_deref().filter(|d| d.starts_with('/')), request.query.as_deref()) else {
            return ServiceResponse::failure("bad_request", format!("{name} needs a query and an absolute directory."));
        };
        let limit = request.limit.unwrap_or(50);
        if !(1..=500).contains(&limit) { return ServiceResponse::failure("bad_request", "limit must be 1 to 500."); }
        if grep && query.is_empty() { return ServiceResponse::failure("bad_request", "grep needs a query."); }
        let scoped = request.scope == Some(true);
        let cwd = request.cwd.as_deref().unwrap_or(directory);
        let home_str = self.home.to_string_lossy().into_owned();
        let result = (|| -> Result<ServiceResponse> {
            let (scope, (index, id, prefix)) = if scoped {
                let scope = std::fs::canonicalize(directory)
                    .map_err(|e| anyhow!("Cannot resolve {directory}: {e}"))?
                    .to_string_lossy()
                    .into_owned();
                (scope.clone(), self.scoped_index(&scope)?)
            } else {
                let root = Self::root(directory).ok_or_else(|| anyhow!("Cannot resolve {directory}."))?;
                let (index, id) = self.index_with_content(&root, true)?;
                (root.clone(), (index, id, None))
            };
            let picked = (self.scores)(&scope);
            if query.contains('\0') { return Err(anyhow!("NUL in query")); }
            let guard = index.read()?;
            let picker = guard.as_ref().ok_or_else(|| anyhow!("Missing instance"))?;
            let path_filter = request.path.as_deref();
            let glob = match (prefix.as_deref(), path_filter) {
                (None, None) => None,
                (Some(pre), None) => Some(format!("{}**", escape_glob(pre))),
                (pre, Some(path)) => {
                    let mut g = pre.map(escape_glob).unwrap_or_default();
                    g.push_str(&escape_glob(path));
                    Some(g)
                }
            };
            // Keep the post-filter: constrained grep that finds nothing retries as literal and drops our glob.
            let scope_relative = |fff_path: &str| -> Option<String> {
                let p = if let Some(prefix) = &prefix {
                    fff_path.strip_prefix(prefix)?
                } else { fff_path };
                if p.is_empty() { return None; }
                if let Some(want) = path_filter {
                    if p != want { return None; }
                }
                Some(p.to_owned())
            };
            let hit = |fff_path: &str, file: Option<&fff_search::FileItem>| -> Option<FileHit> {
                let path = scope_relative(fff_path)?;
                let mut provenance = vec![name.into()];
                if picked.contains_key(path.trim_end_matches('/')) { provenance.push("picked".into()); }
                let git = file.map(|f| fff_search::git::format_git_status(f.git_status)).as_deref().map(git).flatten();
                Some(FileHit { insert: relative_insert(&path, &scope, cwd, Some(&home_str)), path, provenance, git, line: None, column: None, text: None, definition: None })
            };
            let score_of = |fff_path: &str| {
                let p = prefix.as_deref().and_then(|pre| fff_path.strip_prefix(pre)).unwrap_or(fff_path);
                picked.get(p.trim_end_matches('/')).copied().unwrap_or(0.).min(3.)
            };
            let mut response = ServiceResponse::new(true);
            let hits = if grep {
                // fff checks the deadline between files, including before any match; not a hard I/O timeout.
                let mut parsed = fff_search::grep::parse_grep_query(query);
                if let Some(g) = glob.as_deref() { parsed.constraints.push(Constraint::Glob(g)); }
                let page = picker.grep(&parsed, &grep_options(limit as usize));
                let mut hits: Vec<_> = page.matches.iter().filter_map(|m| {
                    let file = page.files[m.file_index];
                    let mut h = hit(&file.relative_path(picker), Some(file))?;
                    h.line = Some(m.line_number as i64); h.column = Some(m.col as i64 + 1); h.text = Some(m.line_content.clone()); h.definition = Some(m.is_definition); Some(h)
                }).collect();
                if page.next_file_offset != 0 || hits.len() > limit as usize { response.partial = Some(true); }
                hits.truncate(limit as usize);
                hits
            } else if query.trim().is_empty() {
                // Blank: fff git/frecency as the base order, then SQLite pick scores; keep `picked`.
                let mut parsed = QueryParser::new(MixedSearchConfig).parse("");
                if let Some(g) = glob.as_deref() { parsed.constraints.push(Constraint::Glob(g)); }
                let found = picker.fuzzy_search_mixed(&parsed, None, FuzzySearchOptions { pagination: PaginationArgs { offset: 0, limit: (limit as usize).saturating_add(15) }, ..Default::default() });
                let rel = |i: usize| match &found.items[i] {
                    fff_search::MixedItemRef::File(f) => f.relative_path(picker),
                    fff_search::MixedItemRef::Dir(d) => d.relative_path(picker),
                };
                let key = |i: usize| i as f64 - 5. * score_of(&rel(i));
                let mut order: Vec<_> = (0..found.items.len()).collect();
                order.sort_by(|&a, &b| key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b)));
                let mut seen = std::collections::HashSet::new();
                let mut hits = Vec::new();
                for i in order {
                    let fff_path = rel(i);
                    let Some(mut h) = (match &found.items[i] {
                        fff_search::MixedItemRef::File(f) => hit(&fff_path, Some(f)),
                        fff_search::MixedItemRef::Dir(_) => hit(&fff_path, None),
                    }) else { continue };
                    if found.scores[i].frecency_boost + found.scores[i].git_status_boost + found.scores[i].git_recency_boost > 0 {
                        h.provenance.push("suggested".into());
                        // fff ranks dirs last on a blank query: a suggested file's folder is suggested just above it.
                        if let Some((dir, _)) = fff_path.rsplit_once('/') {
                            let dir = format!("{dir}/");
                            if let Some(mut d) = hit(&dir, None) {
                                if !d.path.is_empty() && seen.insert(d.path.clone()) {
                                    d.provenance.push("suggested".into());
                                    hits.push(d);
                                }
                            }
                        }
                    }
                    if seen.insert(h.path.clone()) { hits.push(h); }
                }
                hits.truncate(limit as usize);
                hits
            } else {
                let parser = QueryParser::default();
                let mut parsed = parser.parse(query);
                if let Some(g) = glob.as_deref() { parsed.constraints.push(Constraint::Glob(g)); }
                let found = picker.fuzzy_search(&parsed, None, FuzzySearchOptions { pagination: PaginationArgs { offset: 0, limit: (limit as usize * 3).min(500) }, ..Default::default() });
                let best = found.scores.first().map_or(0., |s| s.total as f64);
                // Exact name matches stay first; only files close to the best may be lifted by previous picks.
                let key = |i: usize| {
                    let score = &found.scores[i];
                    let picked_score = score_of(&found.items[i].relative_path(picker));
                    i as f64 - if !score.exact_match && score.total as f64 >= 0.8 * best { 5. * picked_score } else { 0. } - if score.exact_match { 1000. } else { 0. }
                };
                let mut order: Vec<_> = (0..found.items.len()).collect();
                order.sort_by(|&a, &b| key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b)));
                order.into_iter().take(limit as usize).filter_map(|i| hit(&found.items[i].relative_path(picker), Some(&found.items[i]))).collect()
            };
            response.files = Some(hits); response.root = Some(scope.clone()); response.instance = Some(id);
            Ok(response)
        })();
        result.unwrap_or_else(|e| ServiceResponse::failure("failed", e.to_string()))
    }

    pub fn complete(&self, request: &ServiceRequest) -> ServiceResponse {
        let (Some(directory), Some(query)) = (request.directory.as_deref().filter(|d| d.starts_with('/')), request.query.as_deref()) else {
            return ServiceResponse::failure("bad_request", "complete needs a query and an absolute directory.");
        };
        let limit = request.limit.unwrap_or(50);
        if !(1..=500).contains(&limit) { return ServiceResponse::failure("bad_request", "limit must be 1 to 500."); }
        let Some(root) = Self::root(directory) else { return ServiceResponse::failure("failed", format!("Cannot resolve {directory}.")); };
        let result = (|| -> Result<ServiceResponse> {
            let (index, id) = self.index(&root)?;
            let picked = (self.scores)(&root);
            if query.contains('\0') { return Err(anyhow!("NUL in query")); }
            let guard = index.read()?;
            let picker = guard.as_ref().ok_or_else(|| anyhow!("Missing instance"))?;
            let page_limit = (limit as usize * 3).min(500);
            let pagination = PaginationArgs { offset: 0, limit: page_limit };
            let dirs_only = request.dirs_only.unwrap_or(false);
            let has_glob = query.contains('*') || query.contains('?') || query.contains('[');
            let (paths, scores) = if dirs_only {
                if has_glob {
                    let fuzzy_query: String = query.chars().map(|c| if matches!(c, '*' | '?' | '[' | ']' | '/') { ' ' } else { c }).collect();
                    let parser = QueryParser::new(DirSearchConfig);
                    let parsed = parser.parse(&fuzzy_query);
                    let found = picker.fuzzy_search_directories(&parsed, FuzzySearchOptions { pagination, ..Default::default() });
                    let mut matched: Vec<(String, fff_search::Score)> = found.items.iter().zip(found.scores.iter())
                        .filter_map(|(d, s)| {
                            let p = d.relative_path(picker);
                            let match_p = p.strip_suffix('/').unwrap_or(&p);
                            let ok = smcli_core::parse::glob_match(query, match_p);
                            if ok { Some((p, s.clone())) } else { None }
                        })
                        .collect();
                    matched.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then(a.0.cmp(&b.0)));
                    matched.truncate(limit as usize);
                    let paths = matched.iter().map(|(p, _)| (p.clone(), None)).collect();
                    let scores = matched.iter().map(|(_, s)| s.clone()).collect();
                    (paths, scores)
                } else {
                    let parser = QueryParser::new(DirSearchConfig);
                    let parsed = parser.parse(query);
                    let found = picker.fuzzy_search_directories(&parsed, FuzzySearchOptions { pagination, ..Default::default() });
                    (found.items.iter().map(|d| (d.relative_path(picker), None)).collect::<Vec<(String, Option<_>)>>(), found.scores)
                }
            } else if has_glob {
                let found = picker.glob(query, FuzzySearchOptions { pagination, ..Default::default() });
                (found.items.iter().map(|f| (f.relative_path(picker), f.git_status)).collect(), found.scores)
            } else {
                let parser = QueryParser::new(MixedSearchConfig);
                let parsed = parser.parse(query);
                let found = picker.fuzzy_search_mixed(&parsed, None, FuzzySearchOptions { pagination, ..Default::default() });
                (found.items.iter().map(|m| match m {
                    fff_search::MixedItemRef::File(f) => (f.relative_path(picker), f.git_status),
                    fff_search::MixedItemRef::Dir(d) => (d.relative_path(picker), None),
                }).collect(), found.scores)
            };
            let best = scores.first().map_or(0., |s| s.total as f64);
            let key = |i: usize| {
                let score = &scores[i];
                let picked_score = picked.get(paths[i].0.trim_end_matches('/')).copied().unwrap_or(0.).min(3.);
                i as f64 - if !score.exact_match && score.total as f64 >= 0.8 * best { 5. * picked_score } else { 0. } - if score.exact_match { 1000. } else { 0. }
            };
            let mut order: Vec<_> = (0..paths.len()).collect();
            order.sort_by(|&a, &b| key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b)));
            let home_str = self.home.to_string_lossy().into_owned();
            let hits = order.into_iter().take(limit as usize).map(|i| FileHit {
                insert: relative_insert(&paths[i].0, &root, directory, Some(&home_str)),
                path: paths[i].0.clone(),
                provenance: vec!["complete".into()],
                git: git(fff_search::git::format_git_status(paths[i].1)),
                line: None,
                column: None,
                text: None,
                definition: None,
            }).collect();
            let mut response = ServiceResponse::new(true);
            response.files = Some(hits); response.root = Some(root.clone()); response.instance = Some(id);
            Ok(response)
        })();
        result.unwrap_or_else(|e| ServiceResponse::failure("failed", e.to_string()))
    }

    /// Records an explicit choice after lexical path validation; symlink containment is not checked.
    pub fn select(&self, request: &ServiceRequest) -> ServiceResponse {
        let (Some(directory), Some(path), Some(mode)) = (request.directory.as_deref().filter(|d| d.starts_with('/')), request.path.as_deref(), request.mode.as_deref().filter(|m| matches!(*m, "files" | "grep"))) else {
            return ServiceResponse::failure("bad_request", "select needs a directory, a path and a mode of files or grep.");
        };
        let path = path.trim_end_matches('/');
        if path.is_empty() || path.starts_with('/') || path.len() > 4096 || path.contains('\0') || path.split('/').any(|p| p == ".." || p.is_empty()) {
            return ServiceResponse::failure("bad_request", "select needs a path relative to the searched root.");
        }
        let root = if request.scope == Some(true) {
            std::fs::canonicalize(directory).ok().map(|p| p.to_string_lossy().into_owned())
        } else {
            Self::root(directory)
        };
        let Some(root) = root else { return ServiceResponse::failure("failed", format!("Cannot resolve {directory}.")); };
        let query: String = request.query.as_deref().unwrap_or("").chars().take(1000).collect();
        match (self.record)(&root, path, mode, &query, request) {
            Ok(()) => { let mut response = ServiceResponse::new(true); response.root = Some(root); response }
            Err(e) => ServiceResponse::failure("failed", e.to_string()),
        }
    }

    /// Drops instances unused for the idle timeout.
    pub fn evict_idle(&self) { self.pool.lock().unwrap().entries.retain(|_, e| e.used.elapsed() <= self.idle); }
    /// Live instances, oldest id first.
    pub fn instances(&self) -> Vec<FileIndexInfo> {
        let mut entries: Vec<_> = self.pool.lock().unwrap().entries.iter().map(|(root, e)| FileIndexInfo { id: e.id, root: root.clone() }).collect();
        entries.sort_by_key(|e| e.id); entries
    }
}

fn grep_options(limit: usize) -> GrepSearchOptions {
    GrepSearchOptions { page_limit: limit, max_matches_per_file: limit + 1, time_budget_ms: 1000, enforce_time_budget: true, max_file_size: 10 * 1024 * 1024, classify_definitions: true, ..Default::default() }
}

fn escape_glob(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '*' | '?' | '[' | ']' | '{' | '}') { out.push('\\'); }
        out.push(c);
    }
    out
}

/// fff names the state; shown like `git status --short`: index column, then work tree.
/// Clean files and anything outside a repository have none.
fn git(status: &str) -> Option<String> {
    match status {
        "modified" => Some(" M"), "untracked" => Some("??"), "deleted" => Some(" D"), "renamed" => Some(" R"),
        "staged_new" => Some("A "), "staged_modified" => Some("M "), "staged_deleted" => Some("D "), "ignored" => Some("!!"),
        "" | "clean" => None, _ => Some("~~"),
    }.map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use smcli_core::service_protocol::Operation;
    use std::{path::PathBuf, sync::atomic::{AtomicUsize, Ordering}};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Repo(PathBuf);
    impl Repo {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("smcli-file-search-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir_all(path.join("src")).unwrap();
            assert!(std::process::Command::new("git").args(["init", "-q"]).arg(&path).status().unwrap().success());
            std::fs::write(path.join("src/example.txt"), "first\nhello needle\n").unwrap();
            Self(std::fs::canonicalize(path).unwrap())
        }
        fn request(&self, op: Operation, query: &str) -> ServiceRequest {
            let mut r = ServiceRequest::new(op);
            r.directory = Some(self.0.join("src").to_string_lossy().into_owned());
            r.query = Some(query.into()); r
        }
    }
    impl Drop for Repo { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    fn search(capacity: usize) -> FileSearch {
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
        FileSearch::new(capacity, Duration::from_secs(600), Box::new(|_| HashMap::new()), Box::new(|_, _, _, _, _| Ok(())), home)
    }
    #[test]
    fn grep_budget_applies_before_the_first_match() {
        let options = grep_options(50);
        assert_eq!(options.time_budget_ms, 1000);
        assert!(options.enforce_time_budget);
    }

    #[test]
    #[ignore = "synthetic large-tree benchmark"]
    fn measure_zero_match_budget() {
        let repo = Repo::new();
        let content = "abcdefghij0123456789\n".repeat(1600);
        for i in 0..5000 {
            std::fs::write(repo.0.join(format!("src/{i}.txt")), &content).unwrap();
        }
        let search = search(1);
        let (index, _) = search.index(repo.0.to_str().unwrap()).unwrap();
        let guard = index.read().unwrap();
        let picker = guard.as_ref().unwrap();
        let query = fff_search::grep::parse_grep_query("abcdefghij0123456789_not_present");
        for budget in [1, 1000] {
            for enforce in [false, true] {
                let start = Instant::now();
                let page = picker.grep(&query, &GrepSearchOptions { time_budget_ms: budget, enforce_time_budget: enforce, ..grep_options(50) });
                eprintln!("5000 files, budget={budget}ms enforce={enforce}: {:?}, partial={}", start.elapsed(), page.next_file_offset != 0);
                assert!(page.matches.is_empty());
            }
        }
    }

    #[test]
    fn files_relative_insert_and_grep_coordinates() {
        fn send_sync<T: Send + Sync>() {} send_sync::<FileSearch>();
        let repo = Repo::new(); let search = search(4);
        let files = search.files(&repo.request(Operation::Files, "example.txt"));
        assert!(files.ok, "{:?}", files.error);
        let hit = &files.files.unwrap()[0];
        assert_eq!(hit.path, "src/example.txt"); assert_eq!(hit.insert, "example.txt");
        let grep = search.grep(&repo.request(Operation::Grep, "needle"));
        assert!(grep.ok, "{:?}", grep.error);
        let hits = grep.files.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].line, hits[0].column), (Some(2), Some(7)));
        assert_eq!(hits[0].text.as_deref(), Some("hello needle"));
    }
    #[test]
    fn complete_mixed_fuzzy_files_and_dirs() {
        let repo = Repo::new();
        std::fs::create_dir_all(repo.0.join("src/components")).unwrap();
        std::fs::write(repo.0.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(repo.0.join("src/components/mod.rs"), "").unwrap();
        let search = search(4);
        let response = search.complete(&repo.request(Operation::Complete, "comp"));
        assert!(response.ok, "{:?}", response.error);
        let files = response.files.unwrap();
        let paths: Vec<_> = files.iter().map(|h| h.path.clone()).collect();
        assert!(paths.contains(&"src/components/".into()), "mixed should include dir: {paths:?}");
        assert!(paths.contains(&"src/components/mod.rs".into()), "mixed should include file: {paths:?}");
        let dir = paths.iter().position(|p| p == "src/components/").unwrap();
        assert_eq!(files[dir].insert, "components/");
    }
    #[test]
    fn complete_dirs_only_fuzzy() {
        let repo = Repo::new();
        std::fs::create_dir_all(repo.0.join("src/components")).unwrap();
        std::fs::write(repo.0.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(repo.0.join("src/components/mod.rs"), "").unwrap();
        let search = search(4);
        let mut r = repo.request(Operation::Complete, "comp");
        r.dirs_only = Some(true);
        let response = search.complete(&r);
        assert!(response.ok, "{:?}", response.error);
        let paths: Vec<_> = response.files.unwrap().iter().map(|h| h.path.clone()).collect();
        assert!(paths.iter().all(|p| p.ends_with('/')), "dirs_only should return only directories: {paths:?}");
        assert!(paths.contains(&"src/components/".into()), "dirs_only should find components: {paths:?}");
    }
    #[test]
    fn complete_glob_files() {
        let repo = Repo::new();
        std::fs::create_dir_all(repo.0.join("src/components")).unwrap();
        std::fs::write(repo.0.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(repo.0.join("src/components/mod.rs"), "").unwrap();
        let search = search(4);
        let response = search.complete(&repo.request(Operation::Complete, "*.rs"));
        assert!(response.ok, "{:?}", response.error);
        let paths: Vec<_> = response.files.unwrap().iter().map(|h| h.path.clone()).collect();
        assert!(paths.contains(&"src/main.rs".into()), "glob should match main.rs: {paths:?}");
        assert!(paths.contains(&"src/components/mod.rs".into()), "glob should match mod.rs: {paths:?}");
        assert!(!paths.iter().any(|p| p.ends_with('/')), "glob should not return directories: {paths:?}");
    }
    #[test]
    fn complete_dirs_only_glob() {
        let repo = Repo::new();
        std::fs::create_dir_all(repo.0.join("src/foo/bar/service")).unwrap();
        std::fs::create_dir_all(repo.0.join("src/util")).unwrap();
        std::fs::write(repo.0.join("src/foo/bar/service/mod.rs"), "").unwrap();
        std::fs::write(repo.0.join("src/foo/bar/server.rs"), "").unwrap();
        std::fs::write(repo.0.join("src/main.rs"), "").unwrap();
        let search = search(4);
        let mut r = repo.request(Operation::Complete, "service");
        r.dirs_only = Some(true);
        let response = search.complete(&r);
        assert!(response.ok, "{:?}", response.error);
        let paths: Vec<_> = response.files.unwrap().iter().map(|h| h.path.clone()).collect();
        assert!(paths.contains(&"src/foo/bar/service/".into()), "fuzzy should find service dir: {paths:?}");
        let mut r = repo.request(Operation::Complete, "src/**/serv*");
        r.dirs_only = Some(true);
        let response = search.complete(&r);
        assert!(response.ok, "{:?}", response.error);
        let paths: Vec<_> = response.files.unwrap().iter().map(|h| h.path.clone()).collect();
        assert_eq!(paths, ["src/foo/bar/service/"], "dirs_only glob should return only matching directory: {paths:?}");
    }

    #[test]
    fn ready_never_builds_a_cold_root() {
        let repo = Repo::new(); let search = search(4);
        assert!(search.ready(&repo.0.to_string_lossy()).is_none());
        assert!(search.instances().is_empty());
        search.files(&repo.request(Operation::Files, "example.txt"));
        let index = search.ready(&repo.0.to_string_lossy()).unwrap();
        use smcli_search::ProjectEvidence;
        let evidence = evidence::IndexEvidence(index);
        assert_eq!(evidence.exists(&[repo.0.join("src/example.txt"), repo.0.join("missing.txt")]), vec![true, false]);
    }

    #[test]
    fn selection_records_and_rejects_escape() {
        let repo = Repo::new(); let recorded = Arc::new(Mutex::new(Vec::new())); let out = recorded.clone();
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
        let search = FileSearch::new(4, Duration::from_secs(600), Box::new(|_| HashMap::new()), Box::new(move |r, p, m, q, _| { out.lock().unwrap().push((r.to_string(), p.to_string(), m.to_string(), q.to_string())); Ok(()) }), home);
        let mut request = repo.request(Operation::Select, "example"); request.path = Some("src/example.txt".into()); request.mode = Some("files".into());
        assert!(search.select(&request).ok);
        assert_eq!(recorded.lock().unwrap()[0], (repo.0.to_string_lossy().into_owned(), "src/example.txt".into(), "files".into(), "example".into()));
        request.path = Some("folder/".into());
        assert!(search.select(&request).ok);
        assert_eq!(recorded.lock().unwrap()[1], (repo.0.to_string_lossy().into_owned(), "folder".into(), "files".into(), "example".into()));
        for path in ["../escape", "a//b", "/absolute", "a\0b", ""] { request.path = Some(path.into()); assert!(!search.select(&request).ok); }
        assert_eq!(recorded.lock().unwrap().len(), 2);
    }
    #[test]
    fn scoped_select_stores_under_scope_root() {
        let repo = Repo::new();
        std::fs::write(repo.0.join("src/needle.rs"), "").unwrap();
        let recorded = Arc::new(Mutex::new(Vec::new())); let out = recorded.clone();
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
        let search = FileSearch::new(4, Duration::from_secs(600), Box::new(|_| HashMap::new()), Box::new(move |r, p, m, q, _| { out.lock().unwrap().push((r.to_string(), p.to_string(), m.to_string(), q.to_string())); Ok(()) }), home);
        let mut request = repo.request(Operation::Select, "needle.rs");
        request.path = Some("needle.rs".into()); request.mode = Some("files".into()); request.scope = Some(true);
        let response = search.select(&request);
        assert!(response.ok, "{:?}", response.error);
        let got = recorded.lock().unwrap()[0].clone();
        assert_eq!(got.0, repo.0.join("src").to_string_lossy().into_owned());
        assert_eq!(got.1, "needle.rs");
    }
    #[test]
    fn blank_files_reorder_after_picks_and_keep_picked() {
        let repo = Repo::new();
        for name in ["aaa.txt", "mmm.txt", "zzz.txt"] {
            std::fs::write(repo.0.join("src").join(name), "x\n").unwrap();
        }
        let scores = Arc::new(Mutex::new(HashMap::<String, f64>::new()));
        let s = scores.clone();
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
        let search = FileSearch::new(4, Duration::from_secs(600), Box::new(move |_| s.lock().unwrap().clone()), Box::new(|_, _, _, _, _| Ok(())), home);
        let blank = |search: &FileSearch| {
            let r = search.files(&repo.request(Operation::Files, ""));
            assert!(r.ok, "{:?}", r.error);
            r.files.unwrap()
        };
        let before_all = blank(&search);
        let before: Vec<_> = before_all.iter().filter(|h| !h.path.ends_with('/')).collect();
        let target = before.iter().skip(1).map(|h| h.path.clone()).find(|p| p.ends_with(".txt")).expect("non-first txt in blank list");
        let before_pos = before.iter().position(|h| h.path == target).unwrap();
        assert!(!before[before_pos].provenance.iter().any(|p| p == "picked"));
        scores.lock().unwrap().insert(target.clone(), 3.0);
        let after_all = blank(&search);
        let after: Vec<_> = after_all.iter().filter(|h| !h.path.ends_with('/')).collect();
        let after_pos = after.iter().position(|h| h.path == target).expect("picked path still listed");
        assert!(after[after_pos].provenance.iter().any(|p| p == "picked"), "{:?}", after[after_pos].provenance);
        assert!(after_pos < before_pos, "before_pos={before_pos} after_pos={after_pos} before={:?} after={:?}", before.iter().map(|h| &h.path).collect::<Vec<_>>(), after.iter().map(|h| &h.path).collect::<Vec<_>>());
    }
    #[test]
    fn complete_directory_pick_boosts_nonexact() {
        let repo = Repo::new();
        std::fs::create_dir_all(repo.0.join("src/libalpha")).unwrap();
        std::fs::create_dir_all(repo.0.join("src/libomega")).unwrap();
        std::fs::write(repo.0.join("src/libalpha/mod.rs"), "").unwrap();
        std::fs::write(repo.0.join("src/libomega/mod.rs"), "").unwrap();
        let scores = Arc::new(Mutex::new(HashMap::<String, f64>::new()));
        let s = scores.clone();
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
        let search = FileSearch::new(4, Duration::from_secs(600), Box::new(move |_| s.lock().unwrap().clone()), Box::new(|_, _, _, _, _| Ok(())), home);
        let mut r = repo.request(Operation::Complete, "lib");
        r.dirs_only = Some(true);
        let before = search.complete(&r);
        assert!(before.ok, "{:?}", before.error);
        let files = before.files.unwrap();
        let target = files.iter().skip(1).map(|h| h.path.clone()).find(|p| p.contains("lib")).expect("non-first lib dir");
        assert!(target.ends_with('/'), "{target}");
        let before_pos = files.iter().position(|h| h.path == target).unwrap();
        scores.lock().unwrap().insert(target.trim_end_matches('/').to_string(), 3.0);
        let after = search.complete(&r);
        assert!(after.ok, "{:?}", after.error);
        let after_files = after.files.unwrap();
        let after_pos = after_files.iter().position(|h| h.path == target).expect("picked dir still listed");
        assert!(after_pos < before_pos, "before_pos={before_pos} after_pos={after_pos} before={:?} after={:?}", files.iter().map(|h| &h.path).collect::<Vec<_>>(), after_files.iter().map(|h| &h.path).collect::<Vec<_>>());
    }
    #[test]
    fn scoped_picks_boost_with_warm_parent() {
        let repo = Repo::new();
        std::fs::write(repo.0.join("src/aaa.txt"), "x\n").unwrap();
        std::fs::write(repo.0.join("src/zzz.txt"), "x\n").unwrap();
        std::fs::create_dir_all(repo.0.join("src/sub")).unwrap();
        std::fs::write(repo.0.join("src/sub/inside.txt"), "x\n").unwrap();
        let git = |args: &[&str]| {
            let st = std::process::Command::new("git").args(args).current_dir(&repo.0)
                .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
                .status().unwrap();
            assert!(st.success(), "git {args:?}");
        };
        git(&["add", "-A"]);
        git(&["commit", "-m", "t", "--no-gpg-sign"]);
        let scores = Arc::new(Mutex::new(HashMap::<String, f64>::new()));
        let s = scores.clone();
        let src = std::fs::canonicalize(repo.0.join("src")).unwrap().to_string_lossy().into_owned();
        let src_for_cb = src.clone();
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
        let search = FileSearch::new(4, Duration::from_secs(600), Box::new(move |root| {
            if root == src_for_cb { s.lock().unwrap().clone() } else { HashMap::new() }
        }), Box::new(|_, _, _, _, _| Ok(())), home);
        let mut base = repo.request(Operation::Files, "example.txt");
        base.directory = Some(repo.0.to_string_lossy().into_owned());
        assert!(search.files(&base).ok, "warm parent");
        let (index, _) = search.index(repo.0.to_str().unwrap()).unwrap();
        let inside = repo.0.join("src/sub/inside.txt");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let recency = {
                let guard = index.read().unwrap();
                guard.as_ref().and_then(|p| p.get_file_by_path(&inside)).map(|f| f.git_recency_score)
            };
            if recency.is_some_and(|s| s > 0) { break; }
            assert!(Instant::now() < deadline, "git recency not ready for inside.txt (score={recency:?})");
            std::thread::sleep(Duration::from_millis(10));
        }
        let scoped = |search: &FileSearch, query: &str| {
            let mut r = repo.request(Operation::Files, query);
            r.scope = Some(true);
            r.cwd = Some(repo.0.to_string_lossy().into_owned());
            let resp = search.files(&r);
            assert!(resp.ok, "{:?}", resp.error);
            resp.files.unwrap()
        };
        let before = scoped(&search, "txt");
        let files: Vec<_> = before.iter().filter(|h| h.path.ends_with(".txt")).collect();
        let target = files.iter().skip(1).map(|h| h.path.clone()).find(|p| p.ends_with(".txt")).expect("non-first scoped txt");
        assert!(!target.starts_with("src/"), "score key must be scope-relative, got {target}");
        let before_pos = files.iter().position(|h| h.path == target).unwrap();
        scores.lock().unwrap().insert(target.clone(), 3.0);
        let after = scoped(&search, "txt");
        let after_files: Vec<_> = after.iter().filter(|h| h.path.ends_with(".txt")).collect();
        let after_pos = after_files.iter().position(|h| h.path == target).expect("picked scoped file still listed");
        assert!(after_files[after_pos].provenance.iter().any(|p| p == "picked"), "{:?}", after_files[after_pos].provenance);
        assert!(after_pos < before_pos, "before_pos={before_pos} after_pos={after_pos} before={:?} after={:?}", files.iter().map(|h| &h.path).collect::<Vec<_>>(), after_files.iter().map(|h| &h.path).collect::<Vec<_>>());
        scores.lock().unwrap().clear();
        let blank_before = scoped(&search, "");
        let nested = blank_before.iter().position(|h| h.path == "sub/inside.txt").expect("sub/inside.txt");
        assert!(blank_before[nested].provenance.iter().any(|p| p == "suggested"), "{:?}", blank_before[nested].provenance);
        assert!(nested > 0, "expected synthesized parent above {blank_before:?}");
        assert_eq!(blank_before[nested - 1].path, "sub/");
        assert!(blank_before[nested - 1].provenance.iter().any(|p| p == "suggested"), "{:?}", blank_before[nested - 1].provenance);
        let dir_target = blank_before.iter().map(|h| h.path.clone()).find(|p| p.trim_end_matches('/') == "sub").expect("sub/");
        let dir_before = blank_before.iter().position(|h| h.path == dir_target).unwrap();
        scores.lock().unwrap().insert("sub".into(), 3.0);
        let blank_after = scoped(&search, "");
        let dir_hit = blank_after.iter().find(|h| h.path == dir_target).expect("picked dir still listed");
        assert!(dir_hit.provenance.iter().any(|p| p == "picked"), "{:?}", dir_hit.provenance);
        let dir_after = blank_after.iter().position(|h| h.path == dir_target).unwrap();
        if dir_before > 0 {
            assert!(dir_after < dir_before, "dir_before={dir_before} dir_after={dir_after} before={:?} after={:?}", blank_before.iter().map(|h| &h.path).collect::<Vec<_>>(), blank_after.iter().map(|h| &h.path).collect::<Vec<_>>());
        }
    }
    #[test]
    fn blank_pick_promotes_across_page_boundary() {
        let repo = Repo::new();
        for name in ["n0.txt", "n1.txt", "n2.txt", "n3.txt"] {
            std::fs::write(repo.0.join(name), "x\n").unwrap();
        }
        let scores = Arc::new(Mutex::new(HashMap::<String, f64>::new()));
        let s = scores.clone();
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
        let search = FileSearch::new(4, Duration::from_secs(600), Box::new(move |_| s.lock().unwrap().clone()), Box::new(|_, _, _, _, _| Ok(())), home);
        let mut wide_req = repo.request(Operation::Files, "");
        wide_req.limit = Some(50);
        let wide = search.files(&wide_req);
        assert!(wide.ok, "{:?}", wide.error);
        let wide = wide.files.unwrap();
        assert!(wide.len() >= 2, "{wide:?}");
        let outsider = wide.iter().skip(1).take(15).find(|h| !h.path.trim_end_matches('/').contains('/')).expect("top-level hit in boost window");
        let mut page = repo.request(Operation::Files, "");
        page.limit = Some(1);
        let before = search.files(&page);
        assert!(before.ok, "{:?}", before.error);
        let before_files = before.files.unwrap();
        assert!(!before_files.iter().any(|h| h.path == outsider.path), "outsider already on page: {before_files:?}");
        scores.lock().unwrap().insert(outsider.path.trim_end_matches('/').to_string(), 3.0);
        let after = search.files(&page);
        assert!(after.ok, "{:?}", after.error);
        let after_files = after.files.unwrap();
        assert!(after_files.iter().any(|h| h.path == outsider.path), "expected {} in {after_files:?}", outsider.path);
        assert!(after_files.iter().find(|h| h.path == outsider.path).unwrap().provenance.iter().any(|p| p == "picked"));
    }
    #[test]
    fn capacity_reuse_and_idle_eviction() {
        let a = Repo::new(); let b = Repo::new(); let search = search(1);
        let first = search.files(&a.request(Operation::Files, "")); assert!(first.ok);
        assert_eq!(search.files(&a.request(Operation::Files, "")).instance, first.instance);
        assert!(search.files(&b.request(Operation::Files, "")).ok);
        assert_eq!(search.instances().len(), 1);
        assert_eq!(search.instances()[0].root, b.0.to_string_lossy());
        for entry in search.pool.lock().unwrap().entries.values_mut() { entry.used = Instant::now() - Duration::from_secs(601); }
        search.evict_idle(); assert!(search.instances().is_empty());
    }
    #[test]
    fn validation_and_git_letters() {
        let search = search(4); let mut r = ServiceRequest::new(Operation::Files);
        assert_eq!(search.files(&r).error.unwrap().message, "files needs a query and an absolute directory.");
        r.directory = Some("/".into()); r.query = Some("".into()); r.limit = Some(501);
        assert_eq!(search.files(&r).error.unwrap().message, "limit must be 1 to 500.");
        r.limit = None;
        assert_eq!(search.grep(&r).error.unwrap().message, "grep needs a query.");
        assert_eq!(git("staged_new").as_deref(), Some("A ")); assert_eq!(git("clean"), None); assert_eq!(git("unknown").as_deref(), Some("~~"));
    }

    #[test]
    fn scoped_search_reuses_ancestor_root_and_builds_parent() {
        let repo = Repo::new();
        let search = search(2);
        // Non-scoped request indexes the whole repository.
        let mut base = repo.request(Operation::Files, "example.txt");
        base.directory = Some(repo.0.to_string_lossy().into_owned());
        let first = search.files(&base);
        assert!(first.ok, "{:?}", first.error);
        let first_id = first.instance.unwrap();
        assert_eq!(first.files.as_ref().unwrap()[0].path, "src/example.txt");
        // Scoped to a subdirectory reuses the repository instance and returns scope-relative paths.
        let mut sub = repo.request(Operation::Files, "example.txt");
        sub.directory = Some(repo.0.join("src").to_string_lossy().into_owned());
        sub.cwd = Some(repo.0.to_string_lossy().into_owned());
        sub.scope = Some(true);
        let second = search.files(&sub);
        assert!(second.ok, "{:?}", second.error);
        assert_eq!(second.instance, Some(first_id), "scoped search should reuse the ancestor index");
        let hit = &second.files.as_ref().unwrap()[0];
        assert_eq!(hit.path, "example.txt");
        assert_eq!(hit.insert, "src/example.txt");
        // Scoped to the parent directory builds a separate root.
        let parent = repo.0.parent().unwrap();
        let mut wide = ServiceRequest::new(Operation::Files);
        wide.directory = Some(parent.to_string_lossy().into_owned());
        wide.cwd = Some(repo.0.to_string_lossy().into_owned());
        wide.scope = Some(true);
        wide.query = Some("example.txt".into());
        wide.limit = Some(50);
        let third = search.files(&wide);
        assert!(third.ok, "{:?}", third.error);
        assert_ne!(third.instance, Some(first_id));
        assert!(third.files.as_ref().unwrap().iter().any(|h| h.path == format!("{}/src/example.txt", repo.0.file_name().unwrap().to_string_lossy())));
    }

    #[test]
    fn grep_path_filter_restricts_to_one_file() {
        let repo = Repo::new();
        std::fs::write(repo.0.join("src/other.txt"), "hello needle\n").unwrap();
        let search = search(2);
        let mut r = repo.request(Operation::Grep, "needle");
        r.directory = Some(repo.0.join("src").to_string_lossy().into_owned());
        r.cwd = Some(repo.0.to_string_lossy().into_owned());
        r.scope = Some(true);
        r.path = Some("example.txt".into());
        let response = search.grep(&r);
        assert!(response.ok, "{:?}", response.error);
        let hits = response.files.unwrap();
        assert_eq!(hits.len(), 1, "path filter should return only one file: {hits:?}");
        assert_eq!(hits[0].path, "example.txt");
        assert_eq!(hits[0].text.as_deref(), Some("hello needle"));
    }

    fn stamp(path: &std::path::Path, unix_secs: u64) {
        std::fs::File::options().write(true).open(path).unwrap()
            .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(unix_secs)).unwrap();
    }

    #[test]
    fn scoped_search_keeps_inside_hits_when_parent_is_warm() {
        let repo = Repo::new();
        std::fs::create_dir_all(repo.0.join("zzz/nested")).unwrap();
        std::fs::create_dir_all(repo.0.join("aaa")).unwrap();
        for i in 0..30 {
            let p = repo.0.join(format!("zzz/x{i:02}.txt"));
            std::fs::write(&p, "needle\n").unwrap();
            stamp(&p, 1_577_836_800);
        }
        stamp(&repo.0.join("zzz/x29.txt"), 1_262_304_000);
        std::fs::write(repo.0.join("zzz/nested/y.txt"), "x\n").unwrap();
        stamp(&repo.0.join("zzz/nested/y.txt"), 1_577_836_800);
        let inside: Vec<String> = (0..30).map(|i| format!("x{i:02}.txt")).chain(["nested/y.txt".into()]).collect();
        for i in 0..80 {
            let p = repo.0.join(format!("aaa/x{i:02}.txt"));
            std::fs::write(&p, "needle\n").unwrap();
            stamp(&p, 1_704_067_200);
        }
        let search = search(2);
        let mut warm = repo.request(Operation::Files, "x");
        warm.directory = Some(repo.0.to_string_lossy().into_owned());
        let warmed = search.files(&warm);
        assert!(warmed.ok, "{:?}", warmed.error);
        let parent = warmed.instance;
        let mut files = repo.request(Operation::Files, "x");
        files.directory = Some(repo.0.join("zzz").to_string_lossy().into_owned());
        files.scope = Some(true);
        files.limit = Some(10);
        let got = search.files(&files);
        assert!(got.ok, "{:?}", got.error);
        assert_eq!(got.instance, parent, "scoped files should reuse the warm parent index");
        let hits = got.files.unwrap();
        assert_eq!(hits.len(), 10, "{hits:?}");
        assert!(hits.iter().all(|h| inside.contains(&h.path)), "{hits:?}");
        let mut grep = repo.request(Operation::Grep, "needle");
        grep.directory = Some(repo.0.join("zzz").to_string_lossy().into_owned());
        grep.scope = Some(true);
        grep.limit = Some(10);
        let got = search.grep(&grep);
        assert!(got.ok, "{:?}", got.error);
        assert_eq!(got.instance, parent);
        let hits = got.files.unwrap();
        assert_eq!(hits.len(), 10, "{hits:?}");
        assert!(hits.iter().all(|h| inside.contains(&h.path)), "{hits:?}");
        grep.path = Some("x29.txt".into());
        let got = search.grep(&grep);
        assert!(got.ok, "{:?}", got.error);
        let hits = got.files.unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].path, "x29.txt");
        assert_eq!(hits[0].text.as_deref(), Some("needle"));
        let mut blank = repo.request(Operation::Files, "");
        blank.directory = Some(repo.0.join("zzz").to_string_lossy().into_owned());
        blank.scope = Some(true);
        let got = search.files(&blank);
        assert!(got.ok, "{:?}", got.error);
        assert_eq!(got.instance, parent);
        let hits = got.files.unwrap();
        assert!(hits.iter().all(|h| !h.path.is_empty()), "empty path: {hits:?}");
        let dirs: Vec<_> = hits.iter().filter(|h| h.path.ends_with('/')).map(|h| h.path.as_str()).collect();
        assert!(dirs.iter().any(|p| *p == "nested/"), "nested dir should keep trailing slash: {dirs:?}");
    }

    #[test]
    fn scoped_glob_escapes_metacharacters_in_scope_dir() {
        let repo = Repo::new();
        let scope = repo.0.join("a[b]*{c}");
        std::fs::create_dir_all(&scope).unwrap();
        std::fs::create_dir_all(repo.0.join("abc")).unwrap();
        std::fs::write(scope.join("hit.txt"), "uniq\n").unwrap();
        stamp(&scope.join("hit.txt"), 1_577_836_800);
        std::fs::write(repo.0.join("abc/decoy.txt"), "uniq\n").unwrap();
        stamp(&repo.0.join("abc/decoy.txt"), 1_704_067_200);
        let search = search(2);
        let mut warm = repo.request(Operation::Files, "hit");
        warm.directory = Some(repo.0.to_string_lossy().into_owned());
        let warmed = search.files(&warm);
        assert!(warmed.ok, "{:?}", warmed.error);
        let mut r = repo.request(Operation::Files, "hit");
        r.directory = Some(scope.to_string_lossy().into_owned());
        r.scope = Some(true);
        r.limit = Some(10);
        let got = search.files(&r);
        assert!(got.ok, "{:?}", got.error);
        assert_eq!(got.instance, warmed.instance);
        let hits = got.files.unwrap();
        assert!(hits.iter().any(|h| h.path == "hit.txt"), "{hits:?}");
        assert!(hits.iter().all(|h| h.path != "decoy.txt" && !h.path.contains("abc")), "{hits:?}");
        let mut g = repo.request(Operation::Grep, "uniq");
        g.directory = Some(scope.to_string_lossy().into_owned());
        g.scope = Some(true);
        g.limit = Some(10);
        let got = search.grep(&g);
        assert!(got.ok, "{:?}", got.error);
        let hits = got.files.unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].path, "hit.txt");
    }

    #[test]
    fn grep_caps_matches_and_marks_partial() {
        let repo = Repo::new();
        std::fs::write(repo.0.join("src/many.txt"), "onlyhere\n".repeat(11)).unwrap();
        let search = search(1);
        let mut r = repo.request(Operation::Grep, "onlyhere");
        r.path = Some("many.txt".into());
        r.scope = Some(true);
        r.limit = Some(10);
        let got = search.grep(&r);
        assert!(got.ok, "{:?}", got.error);
        let hits = got.files.unwrap();
        assert_eq!(hits.len(), 10, "{hits:?}");
        assert_eq!(got.partial, Some(true));
    }

    #[test]
    fn content_indexing_disabled_at_or_above_home_direct_children() {
        let old_home = std::env::var("HOME").ok();
        let home = std::env::temp_dir().join(format!("smcli-content-home-{}", std::process::id()));
        std::fs::create_dir_all(home.join("Dev/Apps/project")).unwrap();
        unsafe { std::env::set_var("HOME", &home); }
        let search = search(1);
        let home_str = home.to_string_lossy();
        assert!(search.content_indexing_enabled(&format!("{home_str}/Dev/Apps/project")));
        assert!(!search.content_indexing_enabled(&format!("{home_str}/Dev")));
        assert!(!search.content_indexing_enabled(&home_str));
        assert!(!search.content_indexing_enabled("/"));
        unsafe {
            match old_home {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}
