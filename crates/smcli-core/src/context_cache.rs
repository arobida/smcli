use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::service_protocol::{GitStatus, ProjectContext, Repository};
use crate::time::now;

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
    pub fn read(root: &str, timeout: f64) -> Option<Self> {
        Self::run_git(
            &["-C", root, "status", "--porcelain=v2", "--branch"],
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

type StatusReader = dyn Fn(&str) -> Option<GitStatus> + Send + Sync;
struct Entry {
    status: Option<GitStatus>,
    /// Running, or scheduled to start.
    refreshing: bool,
    /// Something changed since the running refresh started.
    dirty: bool,
    started: Option<Instant>,
    touched: Instant,
    generation: u64,
    _watcher: Option<watcher::FileWatcher>,
}
struct State {
    entries: HashMap<String, Entry>,
    generation: u64,
}
struct Inner {
    state: Mutex<State>,
    read: Arc<StatusReader>,
    max_age: f64,
    min_interval: Duration,
    limit: usize,
}

/// Serves `ProjectContext` for the service: discovery runs on the caller's thread (it only stats and reads small
/// files), while Git status is cached per worktree and refreshed on a background thread. A request never waits
/// for Git; it gets the last status and `status_pending` while a refresh is running.
///
/// A refresh starts when the status is missing, a file-system event touched the worktree or its Git directory,
/// or the status is older than `max_age` (events can be missed or coalesced, and `.git/index` timestamps miss
/// working-tree edits). Refreshes of one worktree start at least `min_interval` apart, so a noisy build
/// directory can't make the service run Git continuously.
#[derive(Clone)]
pub struct ContextCache {
    inner: Arc<Inner>,
}

impl Default for ContextCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextCache {
    /// Caps cached worktrees, and with them the file watchers, at 16; the least recently asked about goes first.
    pub fn new() -> Self {
        Self::with_reader(
            5.0,
            Duration::from_secs(1),
            16,
            Arc::new(|root| GitStatus::read(root, 10.0)),
        )
    }

    fn with_reader(
        max_age: f64,
        min_interval: Duration,
        limit: usize,
        read: Arc<StatusReader>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    entries: HashMap::new(),
                    generation: 0,
                }),
                read,
                max_age,
                min_interval,
                limit,
            }),
        }
    }

    pub fn context(&self, directory: &str) -> ProjectContext {
        let mut context = ProjectContext::discover(directory);
        let Some(repository) = context.repository.as_ref() else {
            return context;
        };
        let root = &repository.root;
        let mut state = self.inner.state.lock().unwrap();
        if !state.entries.contains_key(root) {
            self.make_entry(&mut state, repository);
        }
        let entry = state.entries.get_mut(root).unwrap();
        entry.touched = Instant::now();
        let stale = entry
            .status
            .as_ref()
            .is_none_or(|s| now() - s.fetched_at > self.inner.max_age);
        context.status = entry.status.clone();
        if (stale || entry.dirty) && !entry.refreshing {
            self.start_refresh(&mut state, root);
        }
        context.status_pending = state.entries[root].refreshing;
        context
    }

    /// Marks a worktree's status out of date, as a file-system event does.
    pub fn invalidate(&self, root: &str) {
        let mut state = self.inner.state.lock().unwrap();
        let Some(entry) = state.entries.get_mut(root) else {
            return;
        };
        entry.dirty = true;
        if !entry.refreshing {
            self.start_refresh(&mut state, root);
        }
    }

    /// Called with the lock held.
    fn make_entry(&self, state: &mut State, repository: &Repository) {
        if state.entries.len() >= self.inner.limit
            && let Some(oldest) = state
                .entries
                .iter()
                .min_by_key(|(_, e)| e.touched)
                .map(|(r, _)| r.clone())
            {
                state.entries.remove(&oldest);
            }
        state.generation += 1;
            let watcher = {
            let root = repository.root.clone();
            let weak = Arc::downgrade(&self.inner);
            let mut paths = vec![root.clone()];
            paths.extend(
                [&repository.git_dir, &repository.common_dir]
                    .into_iter()
                    .filter(|p| !format!("{p}/").starts_with(&format!("{root}/")))
                    .cloned(),
            );
            paths.sort();
            paths.dedup();
            watcher::FileWatcher::new(
                &paths,
                Box::new(move || {
                    if let Some(inner) = weak.upgrade() {
                        ContextCache { inner }.invalidate(&root);
                    }
                }),
            )
        };
        state.entries.insert(
            repository.root.clone(),
            Entry {
                status: None,
                refreshing: false,
                dirty: false,
                started: None,
                touched: Instant::now(),
                generation: state.generation,
                            _watcher: watcher,
            },
        );
    }

    /// Called with the lock held.
    fn start_refresh(&self, state: &mut State, root: &str) {
        let entry = state.entries.get_mut(root).unwrap();
        entry.refreshing = true;
        entry.dirty = false;
        let start = Instant::now();
        let delay = entry.started.map_or(Duration::ZERO, |s| {
            (s + self.inner.min_interval).saturating_duration_since(start)
        });
        entry.started = Some(start + delay);
        let started = entry.started;
        let generation = entry.generation;
        let cache = self.clone();
        let root = root.to_string();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            let status = (cache.inner.read)(&root);
            let mut state = cache.inner.state.lock().unwrap();
            // An entry evicted and recreated while this read ran belongs to a newer refresh.
            let Some(entry) = state
                .entries
                .get_mut(&root)
                .filter(|e| e.generation == generation && e.started == started)
            else {
                return;
            };
            // A failed or timed-out read keeps the previous status rather than erasing it.
            if status.is_some() {
                entry.status = status;
            }
            entry.refreshing = false;
            if entry.dirty {
                cache.start_refresh(&mut state, &root);
            }
        });
    }
}

mod watcher {
    use std::path::Path;

    use notify::{RecursiveMode, Watcher};

    type Handler = Box<dyn Fn() + Send + Sync>;

    /// File events on a set of directory trees (FSEvents on macOS, inotify on Linux); calls `handler` on
    /// notify's thread for each change. Bursts are cheap: `invalidate` only marks the entry, and refreshes are
    /// throttled by `min_interval`.
    // shortcut: inotify needs one watch per directory, so a huge tree can hit the user's limit; then there is no watcher
    // and status refreshes by age only. Watch just `.git` plus the index if that shows up.
    pub(super) struct FileWatcher(#[allow(dead_code)] notify::RecommendedWatcher);

    impl FileWatcher {
        pub(super) fn new(paths: &[String], handler: Handler) -> Option<Self> {
            let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                // inotify also reports opens and reads, and discovery itself reads `.git/HEAD`: only changes count.
                if event.is_ok_and(|e| !matches!(e.kind, notify::EventKind::Access(_))) {
                    handler();
                }
            })
            .ok()?;
            for path in paths {
                watcher.watch(Path::new(path), RecursiveMode::Recursive).ok()?;
            }
            Some(Self(watcher))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn repository(path: &std::path::Path) {
        std::fs::create_dir_all(path.join(".git/refs")).unwrap();
        std::fs::write(path.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    }
    fn wait(cache: &ContextCache, directory: &str) -> ProjectContext {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let context = cache.context(directory);
            if !context.status_pending {
                return context;
            }
            assert!(Instant::now() < deadline, "refresh did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
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
    fn refresh_is_nonblocking_and_failure_keeps_status() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<ContextCache>();
        let temp = tempfile::tempdir().unwrap();
        repository(temp.path());
        let directory = temp.path().to_str().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let cache = ContextCache::with_reader(
            100.0,
            Duration::from_millis(10),
            16,
            Arc::new(move |_| {
                // Far longer than the call may take, which includes starting the file watcher.
                std::thread::sleep(Duration::from_millis(600));
                (count.fetch_add(1, Ordering::SeqCst) == 0)
                    .then(|| GitStatus::parse("# branch.head main", now()))
            }),
        );
        let start = Instant::now();
        assert!(cache.context(directory).status_pending);
        assert!(start.elapsed() < Duration::from_millis(300));
        let context = wait(&cache, directory);
        assert_eq!(context.status.unwrap().branch.as_deref(), Some("main"));
        cache.invalidate(&context.repository.unwrap().root);
        assert!(cache.context(directory).status_pending);
        assert!(wait(&cache, directory).status.is_some());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn stale_status_and_lru_eviction() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        repository(&a);
        repository(&b);
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let cache = ContextCache::with_reader(
            100.0,
            Duration::ZERO,
            1,
            Arc::new(move |_| {
                count.fetch_add(1, Ordering::SeqCst);
                Some(GitStatus::parse("", now()))
            }),
        );
        let directory = a.to_str().unwrap();
        let context = wait(&cache, directory);
        let root = context.repository.unwrap().root;
        cache
            .inner
            .state
            .lock()
            .unwrap()
            .entries
            .get_mut(&root)
            .unwrap()
            .status
            .as_mut()
            .unwrap()
            .fetched_at = 0.0;
        assert!(cache.context(directory).status_pending);
        wait(&cache, directory);
        wait(&cache, b.to_str().unwrap());
        assert!(
            !cache
                .inner
                .state
                .lock()
                .unwrap()
                .entries
                .contains_key(&root)
        );
        wait(&cache, directory);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn dirty_refresh_retries_and_recreated_entries_have_new_generations() {
        use std::sync::mpsc;
        let temp = tempfile::tempdir().unwrap();
        repository(temp.path());
        let directory = temp.path().to_str().unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let cache = ContextCache::with_reader(
            100.0,
            Duration::ZERO,
            16,
            Arc::new(move |_| {
                let call = count.fetch_add(1, Ordering::SeqCst);
                if call == 0 {
                    started_tx.send(()).unwrap();
                    release_rx.lock().unwrap().recv().unwrap();
                }
                Some(GitStatus::parse(
                    &format!("# branch.head read{call}"),
                    now(),
                ))
            }),
        );
        let context = cache.context(directory);
        let root = context.repository.unwrap().root;
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        cache.invalidate(&root);
        release_tx.send(()).unwrap();
        assert_eq!(
            wait(&cache, directory).status.unwrap().branch.as_deref(),
            Some("read1")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // Removing and rediscovering a root gives it a distinct generation.
        let old_generation = cache.inner.state.lock().unwrap().entries[&root].generation;
        cache.inner.state.lock().unwrap().entries.remove(&root);
        wait(&cache, directory);
        assert!(cache.inner.state.lock().unwrap().entries[&root].generation > old_generation);
    }

    #[test]
    fn filesystem_events_refresh_status() {
        let temp = tempfile::tempdir().unwrap();
        repository(temp.path());
        let directory = temp.path().to_str().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let cache = ContextCache::with_reader(
            100.0,
            Duration::ZERO,
            16,
            Arc::new(move |_| {
                count.fetch_add(1, Ordering::SeqCst);
                Some(GitStatus::parse("", now()))
            }),
        );
        wait(&cache, directory);
        // Allow events from fixture creation to settle before measuring the edit.
        std::thread::sleep(Duration::from_millis(500));
        let before = calls.load(Ordering::SeqCst);
        std::fs::write(temp.path().join("tracked"), "edit").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while calls.load(Ordering::SeqCst) <= before {
            assert!(
                Instant::now() < deadline,
                "file event did not invalidate status"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn git_read_failure_and_timeout() {
        assert!(GitStatus::read("/smcli-does-not-exist", 1.0).is_none());
        assert!(GitStatus::run_git(&["--version"], 1.0).is_some());
        assert!(
            GitStatus::run_git(&["-c", "alias.smclisleep=!sleep 2", "smclisleep"], 0.01).is_none()
        );
    }
}
