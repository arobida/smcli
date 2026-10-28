use std::{collections::HashMap, path::Path, sync::{Arc, Mutex, OnceLock, Weak, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use fff_search::{GrepSearchOptions, grep::parse_grep_query};
use smcli_core::{ranking::{Candidate, CandidateKind}, tests_discovery::{fn_test_command, test_names}};
use crate::file_search::{FileSearch, OwnedIndex};

struct Cached {
    at: Instant,
    items: Vec<Candidate>,
    index: Weak<OwnedIndex>,
    dirty: Arc<AtomicBool>,
    watch: Option<fff_search::WatchId>,
}
impl Drop for Cached {
    fn drop(&mut self) {
        if let (Some(index), Some(watch)) = (self.index.upgrade(), self.watch) { index.unwatch(watch); }
    }
}
static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();

fn prune(cache: &mut HashMap<String, Cached>, capacity: usize, now: Instant) {
    cache.retain(|_, entry| now.duration_since(entry.at) < Duration::from_secs(30)
        && !entry.dirty.load(Ordering::Acquire) && entry.index.strong_count() > 0);
    while cache.len() > capacity {
        let root = cache.iter().min_by_key(|(_, entry)| entry.at).unwrap().0.clone();
        cache.remove(&root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smcli_core::service_protocol::{Operation, ServiceRequest};
    use std::path::PathBuf;

    fn warm(root: &Path) -> FileSearch {
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
        let search = FileSearch::new(2, Duration::from_secs(600), Box::new(|_| HashMap::new()), Box::new(|_, _, _, _, _| Ok(())), home);
        let mut request = ServiceRequest::new(Operation::Files);
        request.directory = Some(root.to_string_lossy().into_owned());
        request.query = Some("lib".into());
        assert!(search.files(&request).ok);
        search
    }

    #[test]
    fn cache_evicts_expired_dirty_and_excess_roots() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let search = warm(&root);
        let index = search.ready(root.to_str().unwrap()).unwrap();
        let now = Instant::now();
        let mut cache = HashMap::new();
        for (name, age, dirty) in [("expired", 31, false), ("dirty", 0, true), ("old", 3, false), ("middle", 2, false), ("new", 1, false)] {
            cache.insert(name.into(), Cached { at: now - Duration::from_secs(age), items: vec![],
                index: Arc::downgrade(&index), dirty: Arc::new(AtomicBool::new(dirty)), watch: None });
        }
        prune(&mut cache, 2, now);
        assert_eq!(cache.len(), 2);
        assert!(cache.contains_key("middle") && cache.contains_key("new"));
        drop(index);
        drop(search);
        prune(&mut cache, 2, now);
        assert!(cache.is_empty());
    }

    #[test]
    fn watcher_invalidates_test_definitions_before_ttl() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::write(root.join("lib.rs"), "#[test]\nfn before_case() {}\n").unwrap();
        let search = warm(&root);
        let root = root.to_str().unwrap();
        assert!(test_candidates(&search, root).iter().any(|c| c.command == "cargo test before_case"));
        assert!(CACHE.get().unwrap().lock().unwrap()[root].watch.is_some());
        std::fs::write(Path::new(root).join("lib.rs"), "#[test]\nfn after_case() {}\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let found = test_candidates(&search, root);
            if found.iter().any(|c| c.command == "cargo test after_case") {
                assert!(found.iter().all(|c| c.command != "cargo test before_case"));
                break;
            }
            assert!(Instant::now() < deadline, "watcher did not invalidate cache");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// shortcut: if fff cannot subscribe yet, the bounded cache expires after 30s; retry watching on refresh.
pub fn test_candidates(search: &FileSearch, root: &str) -> Vec<Candidate> {
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cached = cache.lock().unwrap();
    prune(&mut cached, search.capacity(), Instant::now());
    drop(cached);
    let Some(index) = search.ready(root) else { return vec![] };
    {
        let cached = cache.lock().unwrap();
        if let Some(entry) = cached.get(root).filter(|entry| entry.index.ptr_eq(&Arc::downgrade(&index))) {
            return entry.items.clone();
        }
    }
    let dirty = Arc::new(AtomicBool::new(false));
    let changed = dirty.clone();
    let watch = index.watch("", Default::default(), move |_, _| { changed.store(true, Ordering::Release); }).ok();
    let mut entry = Cached { at: Instant::now(), items: vec![], index: Arc::downgrade(&index), dirty, watch };
    let Ok(guard) = index.read() else { return vec![] };
    let Some(picker) = guard.as_ref() else { return vec![] };
    let mut found = vec![];
    for pattern in ["#[test]", "#[tokio::test]", "def test_", "it('", "test('"] {
        let query = parse_grep_query(pattern);
        let page = picker.grep(&query, &GrepSearchOptions { page_limit: 200, max_matches_per_file: 0, time_budget_ms: 100, enforce_time_budget: true, ..Default::default() });
        for hit in page.matches {
            let file = page.files[hit.file_index];
            let path = file.relative_path(picker);
            let kind = if path.ends_with(".rs") { "cargo" } else if path.ends_with(".py") { "python" } else if [".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs"].iter().any(|e| path.ends_with(e)) { "npm" } else { continue };
            let text = std::fs::read_to_string(Path::new(root).join(&path)).unwrap_or_default();
            for name in test_names(&path, &text) {
                if let Some(command) = fn_test_command(kind, &name) { found.push(Candidate::new(command, CandidateKind::Test, "project test")); }
            }
        }
    }
    found.sort_by(|a, b| a.command.cmp(&b.command));
    found.dedup_by(|a, b| a.command == b.command);
    drop(guard);
    entry.at = Instant::now();
    entry.items = found.clone();
    let mut cached = cache.lock().unwrap();
    cached.remove(root);
    prune(&mut cached, search.capacity() - 1, Instant::now());
    cached.insert(root.to_owned(), entry);
    found
}
