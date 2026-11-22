use crate::file_search::{FileSearch, OwnedIndex};
use fff_search::{GrepSearchOptions, grep::parse_grep_query};
use smcli_core::{
    ranking::{Candidate, CandidateKind},
    tests_discovery::{fn_test_command, test_names},
};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

struct Cached {
    at: Instant,
    /// Every source path per command, so each caller's ignore policy applies after the read.
    items: Vec<(Candidate, Vec<String>)>,
    index: Weak<OwnedIndex>,
    dirty: Arc<AtomicBool>,
    watch: Option<fff_search::WatchId>,
}
impl Drop for Cached {
    fn drop(&mut self) {
        if let (Some(index), Some(watch)) = (self.index.upgrade(), self.watch) {
            index.unwatch(watch);
        }
    }
}
static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();

fn prune(cache: &mut HashMap<String, Cached>, capacity: usize, now: Instant) {
    cache.retain(|_, entry| {
        now.duration_since(entry.at) < Duration::from_secs(30)
            && !entry.dirty.load(Ordering::Acquire)
            && entry.index.strong_count() > 0
    });
    while cache.len() > capacity {
        let root = cache
            .iter()
            .min_by_key(|(_, entry)| entry.at)
            .unwrap()
            .0
            .clone();
        cache.remove(&root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smcli_core::service_protocol::{Operation, ServiceRequest};
    use std::path::PathBuf;

    fn warm(root: &Path) -> FileSearch {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        let search = FileSearch::new(
            2,
            Duration::from_secs(600),
            Box::new(|_| Default::default()),
            Box::new(|_, _, _, _, _, _, _| Ok(())),
            home,
        );
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
        for (name, age, dirty) in [
            ("expired", 31, false),
            ("dirty", 0, true),
            ("old", 3, false),
            ("middle", 2, false),
            ("new", 1, false),
        ] {
            cache.insert(
                name.into(),
                Cached {
                    at: now - Duration::from_secs(age),
                    items: vec![],
                    index: Arc::downgrade(&index),
                    dirty: Arc::new(AtomicBool::new(dirty)),
                    watch: None,
                },
            );
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
    fn double_quoted_test_candidates() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::write(root.join("it.js"), "it(\"double_it\", () => {});\n").unwrap();
        std::fs::write(root.join("test.ts"), "test(\"double_test\", () => {});\n").unwrap();
        let search = warm(&root);
        let found = test_candidates(&search, root.to_str().unwrap(), None);
        let commands: Vec<&str> = found.iter().map(|c| c.command.as_str()).collect();
        assert_eq!(
            commands,
            ["npm test -- -t double_it", "npm test -- -t double_test"]
        );
    }

    #[test]
    fn test_candidates_honor_ignore() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::write(
            root.join("skip.rs"),
            "#[test]\nfn both_case() {}\n#[test]\nfn only_skipped() {}\n",
        )
        .unwrap();
        std::fs::write(root.join("keep.rs"), "#[test]\nfn both_case() {}\n").unwrap();
        std::fs::write(root.join("ignore.txt"), "skip.rs\n").unwrap();
        let config = root.join("config.json");
        std::fs::write(&config, r#"{"ignoreFile":"ignore.txt"}"#).unwrap();
        let ignore = smcli_core::config::FileIgnore::load(&config, &root)
            .unwrap()
            .unwrap();
        let search = warm(&root);
        let r = root.to_str().unwrap();
        let cmds = |found: Vec<Candidate>| -> Vec<String> {
            found.into_iter().map(|c| c.command).collect()
        };
        for ignoring_first in [true, false] {
            let (a, b);
            if ignoring_first {
                a = cmds(test_candidates(&search, r, Some(&ignore)));
                b = cmds(test_candidates(&search, r, None));
            } else {
                b = cmds(test_candidates(&search, r, None));
                a = cmds(test_candidates(&search, r, Some(&ignore)));
            }
            assert_eq!(a, ["cargo test both_case"]);
            assert_eq!(b, ["cargo test both_case", "cargo test only_skipped"]);
        }
    }

    #[test]
    fn watcher_invalidates_test_definitions_before_ttl() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::write(root.join("lib.rs"), "#[test]\nfn before_case() {}\n").unwrap();
        let search = warm(&root);
        let root = root.to_str().unwrap();
        assert!(
            test_candidates(&search, root, None)
                .iter()
                .any(|c| c.command == "cargo test before_case")
        );
        assert!(CACHE.get().unwrap().lock().unwrap()[root].watch.is_some());
        std::fs::write(
            Path::new(root).join("lib.rs"),
            "#[test]\nfn after_case() {}\n",
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let found = test_candidates(&search, root, None);
            if found.iter().any(|c| c.command == "cargo test after_case") {
                assert!(found.iter().all(|c| c.command != "cargo test before_case"));
                break;
            }
            assert!(
                Instant::now() < deadline,
                "watcher did not invalidate cache"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// shortcut: if fff cannot subscribe yet, the bounded cache expires after 30s; retry watching on refresh.
pub fn test_candidates(
    search: &FileSearch,
    root: &str,
    ignore: Option<&smcli_core::config::FileIgnore>,
) -> Vec<Candidate> {
    let keep = |items: &[(Candidate, Vec<String>)]| -> Vec<Candidate> {
        items
            .iter()
            .filter(|(_, paths)| {
                !ignore.is_some_and(|i| {
                    paths
                        .iter()
                        .all(|p| i.ignores(Path::new(root), &Path::new(root).join(p), false))
                })
            })
            .map(|(c, _)| c.clone())
            .collect()
    };
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cached = cache.lock().unwrap();
    prune(&mut cached, search.capacity(), Instant::now());
    drop(cached);
    let Some(index) = search.ready(root) else {
        return vec![];
    };
    {
        let cached = cache.lock().unwrap();
        if let Some(entry) = cached
            .get(root)
            .filter(|entry| entry.index.ptr_eq(&Arc::downgrade(&index)))
        {
            return keep(&entry.items);
        }
    }
    let dirty = Arc::new(AtomicBool::new(false));
    let changed = dirty.clone();
    let watch = index
        .watch("", Default::default(), move |_, _| {
            changed.store(true, Ordering::Release);
        })
        .ok();
    let mut entry = Cached {
        at: Instant::now(),
        items: vec![],
        index: Arc::downgrade(&index),
        dirty,
        watch,
    };
    let Ok(guard) = index.read() else {
        return vec![];
    };
    let Some(picker) = guard.as_ref() else {
        return vec![];
    };
    let mut found: std::collections::BTreeMap<String, (Candidate, Vec<String>)> =
        Default::default();
    let mut read = std::collections::HashSet::new();
    for pattern in [
        "#[test]",
        "#[tokio::test]",
        "def test_",
        "it('",
        "test('",
        "it(\"",
        "test(\"",
    ] {
        let query = parse_grep_query(pattern);
        let page = picker.grep(
            &query,
            &GrepSearchOptions {
                page_limit: 200,
                max_matches_per_file: 1,
                time_budget_ms: 100,
                enforce_time_budget: true,
                ..Default::default()
            },
        );
        for hit in page.matches {
            let file = page.files[hit.file_index];
            let path = file.relative_path(picker);
            let kind = if path.ends_with(".rs") {
                "cargo"
            } else if path.ends_with(".py") {
                "python"
            } else if [".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs"]
                .iter()
                .any(|e| path.ends_with(e))
            {
                "npm"
            } else {
                continue;
            };
            if !read.insert(path.to_string()) {
                continue;
            }
            // Size cap only; the symlink-swap race stays deferred.
            let mut bytes = vec![];
            if let Ok(file) = std::fs::File::open(Path::new(root).join(&path)) {
                let _ =
                    std::io::Read::read_to_end(&mut std::io::Read::take(file, 1 << 20), &mut bytes);
            }
            let text = String::from_utf8_lossy(&bytes);
            for name in test_names(&path, &text) {
                if let Some(command) = fn_test_command(kind, &name) {
                    found
                        .entry(command.clone())
                        .or_insert_with(|| {
                            (
                                Candidate::new(command, CandidateKind::Test, "project test"),
                                vec![],
                            )
                        })
                        .1
                        .push(path.to_string());
                }
            }
        }
    }
    let found: Vec<_> = found.into_values().collect();
    drop(guard);
    entry.at = Instant::now();
    entry.items = found.clone();
    let mut cached = cache.lock().unwrap();
    cached.remove(root);
    prune(&mut cached, search.capacity() - 1, Instant::now());
    cached.insert(root.to_owned(), entry);
    keep(&found)
}
