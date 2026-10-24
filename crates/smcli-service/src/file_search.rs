use std::{collections::HashMap, panic::{catch_unwind, AssertUnwindSafe}, sync::{Arc, Condvar, Mutex}, time::{Duration, Instant}};
use anyhow::{Result, anyhow};
use fff_search::{FilePicker, FilePickerOptions, SharedFilePicker, SharedFrecency, QueryParser, FuzzySearchOptions, PaginationArgs, GrepSearchOptions};
use smcli_core::{context::ProjectContext, service_location::relative_insert, service_protocol::{FileHit, FileIndexInfo, ServiceRequest, ServiceResponse}};

type Scores = Box<dyn Fn(&str) -> HashMap<String, f64> + Send + Sync>;
type Record = Box<dyn Fn(&str, &str, &str, &str, &ServiceRequest) -> Result<()> + Send + Sync>;
/// Owns one fff instance for one root. An evicted index lives until its last request ends.
struct OwnedIndex(SharedFilePicker);
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
type Index = Arc<OwnedIndex>;
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
}
impl FileSearch {
    pub fn new(capacity: usize, idle: Duration, scores: Scores, record: Record) -> Self {
        Self { capacity: capacity.max(1), idle, pool: Mutex::new(Pool::default()), available: Condvar::new(), scores, record }
    }

    /// The directory's Git worktree, or the directory itself outside Git. Real paths, so symlinks don't split instances.
    fn root(directory: &str) -> Option<String> {
        ProjectContext::discover(directory).repository.map(|r| r.root).or_else(|| std::fs::canonicalize(directory).ok().map(|p| p.to_string_lossy().into_owned()))
    }

    /// The instance for `root`, created (and the least recently used one evicted) when needed.
    /// Creation waits for the first scan, so it happens outside the lock: other roots stay responsive.
    fn index(&self, root: &str) -> Result<(Index, i64)> {
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
                FilePicker::new_with_shared_state(picker.clone(), SharedFrecency::default(), FilePickerOptions { base_path: root.into(), ..Default::default() })?;
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

    pub fn files(&self, request: &ServiceRequest) -> ServiceResponse { self.search(request, false) }
    pub fn grep(&self, request: &ServiceRequest) -> ServiceResponse { self.search(request, true) }

    fn search(&self, request: &ServiceRequest, grep: bool) -> ServiceResponse {
        let name = if grep { "grep" } else { "files" };
        let (Some(directory), Some(query)) = (request.directory.as_deref().filter(|d| d.starts_with('/')), request.query.as_deref()) else {
            return ServiceResponse::failure("bad_request", format!("{name} needs a query and an absolute directory."));
        };
        let limit = request.limit.unwrap_or(50);
        if !(1..=500).contains(&limit) { return ServiceResponse::failure("bad_request", "limit must be 1 to 500."); }
        if grep && query.is_empty() { return ServiceResponse::failure("bad_request", "grep needs a query."); }
        let Some(root) = Self::root(directory) else { return ServiceResponse::failure("failed", format!("Cannot resolve {directory}.")); };
        let result = (|| -> Result<ServiceResponse> {
            let (index, id) = self.index(&root)?;
            let picked = (self.scores)(&root);
            if query.contains('\0') { return Err(anyhow!("NUL in query")); }
            let guard = index.read()?;
            let picker = guard.as_ref().ok_or_else(|| anyhow!("Missing instance"))?;
            let hit = |file: &fff_search::FileItem| {
                let path = file.relative_path(picker);
                let mut provenance = vec![name.into()];
                if picked.contains_key(&path) { provenance.push("picked".into()); }
                FileHit { insert: relative_insert(&path, &root, directory), path, provenance, git: git(fff_search::git::format_git_status(file.git_status)), line: None, column: None, text: None }
            };
            let mut response = ServiceResponse::new(true);
            let hits = if grep {
                // Plain-text content search (smart case), up to `limit` matches within 1000 ms.
                let parsed = fff_search::grep::parse_grep_query(query);
                let page = picker.grep(&parsed, &GrepSearchOptions { page_limit: limit as usize, max_matches_per_file: 0, time_budget_ms: 1000, max_file_size: 10 * 1024 * 1024, ..Default::default() });
                if page.next_file_offset != 0 { response.partial = Some(true); }
                page.matches.iter().map(|m| {
                    let mut h = hit(page.files[m.file_index]);
                    h.line = Some(m.line_number as i64); h.column = Some(m.col as i64 + 1); h.text = Some(m.line_content.clone()); h
                }).collect()
            } else {
                let parser = QueryParser::default();
                let parsed = parser.parse(query);
                let found = picker.fuzzy_search(&parsed, None, FuzzySearchOptions { pagination: PaginationArgs { offset: 0, limit: (limit as usize * 3).min(500) }, ..Default::default() });
                let best = found.scores.first().map_or(0., |s| s.total as f64);
                // Exact name matches stay first; only files close to the best may be lifted by previous picks.
                let key = |i: usize| {
                    let score = &found.scores[i];
                    let picked_score = picked.get(&found.items[i].relative_path(picker)).copied().unwrap_or(0.).min(3.);
                    i as f64 - if !score.exact_match && score.total as f64 >= 0.8 * best { 5. * picked_score } else { 0. } - if score.exact_match { 1000. } else { 0. }
                };
                let mut order: Vec<_> = (0..found.items.len()).collect();
                order.sort_by(|&a, &b| key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b)));
                order.into_iter().take(limit as usize).map(|i| hit(found.items[i])).collect()
            };
            response.files = Some(hits); response.root = Some(root.clone()); response.instance = Some(id);
            Ok(response)
        })();
        result.unwrap_or_else(|e| ServiceResponse::failure("failed", e.to_string()))
    }

    /// Records an explicit choice. The path must stay inside the root; fff and the picker only ever send relative ones.
    pub fn select(&self, request: &ServiceRequest) -> ServiceResponse {
        let (Some(directory), Some(path), Some(mode)) = (request.directory.as_deref().filter(|d| d.starts_with('/')), request.path.as_deref(), request.mode.as_deref().filter(|m| matches!(*m, "files" | "grep"))) else {
            return ServiceResponse::failure("bad_request", "select needs a directory, a path and a mode of files or grep.");
        };
        if path.is_empty() || path.starts_with('/') || path.len() > 4096 || path.contains('\0') || path.split('/').any(|p| p == ".." || p.is_empty()) {
            return ServiceResponse::failure("bad_request", "select needs a path relative to the searched root.");
        }
        let Some(root) = Self::root(directory) else { return ServiceResponse::failure("failed", format!("Cannot resolve {directory}.")); };
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
    fn search(capacity: usize) -> FileSearch { FileSearch::new(capacity, Duration::from_secs(600), Box::new(|_| HashMap::new()), Box::new(|_, _, _, _, _| Ok(()))) }
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
    fn selection_records_and_rejects_escape() {
        let repo = Repo::new(); let recorded = Arc::new(Mutex::new(Vec::new())); let out = recorded.clone();
        let search = FileSearch::new(4, Duration::from_secs(600), Box::new(|_| HashMap::new()), Box::new(move |r, p, m, q, _| { out.lock().unwrap().push((r.to_string(), p.to_string(), m.to_string(), q.to_string())); Ok(()) }));
        let mut request = repo.request(Operation::Select, "example"); request.path = Some("src/example.txt".into()); request.mode = Some("files".into());
        assert!(search.select(&request).ok);
        assert_eq!(recorded.lock().unwrap()[0], (repo.0.to_string_lossy().into_owned(), "src/example.txt".into(), "files".into(), "example".into()));
        for path in ["../escape", "a//b", "/absolute", "a/", "a\0b", ""] { request.path = Some(path.into()); assert!(!search.select(&request).ok); }
        assert_eq!(recorded.lock().unwrap().len(), 1);
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
}
