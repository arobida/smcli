//! The long-lived owner of one history database: it keeps the database open and suggestions in memory, and
//! answers smcli clients over a private Unix-domain socket. Started on demand by clients; exits when idle.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::Result;
use rusqlite::Connection;
use smcli_core::config::{CaptureFilter, SmcliConfig};
use smcli_core::context_cache::ContextCache;
use smcli_core::entry::SearchEntry;
use smcli_core::history::{CapturedHistory, NewRun, Run, user_name};
use smcli_core::parse::is_smcli_command;
use smcli_core::ranking::{RankingContext, Signal};
use smcli_core::service_location::{ServiceLocation, build};
use smcli_core::service_protocol::{
    IO_TIMEOUT, MAX_CONNECTIONS, MAX_MESSAGE_BYTES, Operation, ServiceRequest, ServiceResponse,
    VERSION, decode_line, encode_line,
};
use smcli_core::suggestion_index::SuggestionIndex;
use smcli_core::time::now;
use smcli_core::unix_socket::{SocketConnection, SocketError, listen_unix};
use smcli_search::HistorySearch;
use smcli_search::corpus::{CorpusState, SearchSignals};
use smcli_search::native::history_files;
use smcli_search::setup::search_engine;

use crate::file_search::FileSearch;

/// Reused for the same directory, project and native files, for up to five minutes (frecency drifts with time).
struct SearchCorpus {
    /// Path, size and modification time of each native history file read.
    native: Vec<String>,
    built: Instant,
    state: CorpusState,
}

/// All history state, behind one lock.
struct State {
    db: Connection,
    /// Built on the first suggest and dropped whenever another process changes the database.
    index: Option<SuggestionIndex>,
    data_version: i64,
    history_revision: u64,
    last_activity: Instant,
    /// The corpus is rebuilt after any outside write, the engine when the config changes.
    engine: Option<(SmcliConfig, Arc<HistorySearch>)>,
    corpus: Option<SearchCorpus>,
}

pub struct Service {
    history: CapturedHistory,
    location: ServiceLocation,
    idle: Duration,
    user: String,
    /// Stamped on every response so clients can tell this service is from an older build.
    build: Option<String>,
    environment: HashMap<String, String>,
    state: Mutex<State>,
    /// Searches run one at a time, outside `state`, so records aren't held up by one.
    searching: Mutex<()>,
    active: AtomicUsize,
    /// Directory context from cache: discovery is a few stats, and Git status refreshes in the background.
    contexts: ContextCache,
    file_search: FileSearch,
}

fn data_version(db: &Connection) -> Result<i64> {
    Ok(db.query_row("PRAGMA data_version", [], |r| r.get(0))?)
}

fn inode(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.ino())
}

impl Service {
    pub fn new(history: CapturedHistory, location: ServiceLocation, idle: Duration, environment: HashMap<String, String>) -> Result<Arc<Self>> {
        let db = history.open_connection()?;
        let version = data_version(&db)?;
        let capacity = environment.get("SMCLI_FFF_MAX").and_then(|v| v.parse().ok()).unwrap_or(4);
        let fff_idle = Duration::from_secs_f64(environment.get("SMCLI_FFF_IDLE").and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(600.0));
        let home = PathBuf::from(environment.get("HOME").cloned().unwrap_or_else(|| "/".into()));
        // The file search reaches back into `state` through a weak handle, set once the service exists.
        let service = Arc::new_cyclic(|weak: &std::sync::Weak<Service>| {
            let (scores_handle, record_handle) = (weak.clone(), weak.clone());
            let file_search = FileSearch::new(
                capacity,
                fff_idle,
                Box::new(move |root| {
                    let Some(service) = scores_handle.upgrade() else { return HashMap::new() };
                    let state = service.lock();
                    service.history.selection_scores(root, now(), &state.db).unwrap_or_default()
                }),
                Box::new(move |root, path, mode, query, request| {
                    let Some(service) = record_handle.upgrade() else { return Ok(()) };
                    let filter = CaptureFilter::new(&SmcliConfig::load(&service.config_path(request))?)?;
                    let directory = request.directory.as_deref().unwrap_or("");
                    if !filter.allows(query, directory) {
                        return Ok(());
                    }
                    let mut state = service.lock();
                    state.last_activity = Instant::now();
                    service.history.record_selection(root, path, mode, query, now(), &state.db)
                }),
                home.clone(),
            );
            Service {
                build: std::env::current_exe().ok().and_then(|exe| build(&exe)),
                history,
                location,
                idle,
                user: user_name(),
                environment,
                state: Mutex::new(State { db, index: None, data_version: version, history_revision: 0, last_activity: Instant::now(), engine: None, corpus: None }),
                searching: Mutex::new(()),
                active: AtomicUsize::new(0),
                contexts: ContextCache::new(),
                file_search,
            }
        });
        Ok(service)
    }

    /// A poisoned lock only means a handler panicked mid-request; the state itself is still usable.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn touch(&self) {
        self.lock().last_activity = Instant::now();
    }

    fn claim_idle_shutdown(&self) -> bool {
        let state = self.lock();
        if state.last_activity.elapsed() <= self.idle {
            return false;
        }
        self.active.compare_exchange(0, MAX_CONNECTIONS, Ordering::SeqCst, Ordering::SeqCst).is_ok()
    }

    pub fn run(self: &Arc<Self>) -> Result<()> {
        let listener = listen_unix(&self.location.socket)?;
        let socket_inode = inode(&self.location.socket);
        signals::install(&self.location.socket, socket_inode);
        let watcher = Arc::clone(self);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(15));
            // Idle too long, or our socket was removed or replaced: let a fresh service take over.
            watcher.file_search.evict_idle();
            if watcher.claim_idle_shutdown() || inode(&watcher.location.socket) != socket_inode {
                watcher.stop(socket_inode);
            }
        });
        self.warm_search();
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    if let Some(guard) = Leave::admit(Arc::clone(self)) {
                        let service = Arc::clone(self);
                        if let Err(e) = std::thread::Builder::new().spawn(move || {
                            let _guard = guard;
                            service.serve(stream, socket_inode);
                        }) {
                            eprintln!("smcli-service: could not spawn connection thread: {e}");
                        }
                    } else {
                        self.reject_busy(stream);
                    }
                }
                // Out of descriptors: back off instead of spinning until one frees up.
                Err(e) if matches!(e.raw_os_error(), Some(libc::EMFILE | libc::ENFILE)) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => {}
            }
        }
        Ok(())
    }

    fn reject_busy(&self, mut stream: UnixStream) {
        // Never let a rejected client block the accept loop.
        if stream.set_nonblocking(true).is_ok() {
            let mut response = ServiceResponse::failure("busy", "Too many connections.");
            response.build = self.build.clone();
            if let Ok(data) = encode_line(&response) {
                let _ = stream.write_all(&data);
            }
        }
    }

    /// Removes the socket only while it is still ours, then exits without running libc exit handlers.
    fn stop(&self, socket_inode: Option<u64>) -> ! {
        if inode(&self.location.socket).is_some() && inode(&self.location.socket) == socket_inode {
            let _ = std::fs::remove_file(&self.location.socket);
        }
        // SAFETY: _exit is async-signal-safe and avoids running libc exit handlers while other threads work.
        unsafe { libc::_exit(0) }
    }

    fn serve(&self, stream: UnixStream, socket_inode: Option<u64>) {
        let mut connection = SocketConnection::from(stream);
        // SAFETY: getuid has no preconditions.
        if connection.peer_uid() != Some(unsafe { libc::getuid() }) {
            return;
        }
        let timeout = Duration::from_secs_f64(IO_TIMEOUT);
        let mut shutdown = false;
        let mut response = match connection.read_line(MAX_MESSAGE_BYTES, timeout) {
                Ok(None) => return,
                Ok(Some(line)) => match decode_line::<ServiceRequest>(&line) {
                    Ok(request) => {
                        shutdown = request.op == Operation::Shutdown && request.v == VERSION;
                        self.dispatch(&request)
                    }
                    Err(_) => ServiceResponse::failure("bad_request", "Malformed request."),
                },
                Err(SocketError::TooLarge) => ServiceResponse::failure("too_large", format!("Request exceeds {MAX_MESSAGE_BYTES} bytes.")),
                Err(SocketError::Timeout) => ServiceResponse::failure("timeout", format!("Request not received within {IO_TIMEOUT} s.")),
                Err(_) => return,
            };
        response.build = self.build.clone();
        if let Ok(data) = encode_line(&response) {
            let _ = connection.write(&data, timeout);
        }
        if shutdown {
            let _state = self.lock();
            self.stop(socket_inode);
        }
    }

    fn dispatch(&self, request: &ServiceRequest) -> ServiceResponse {
        if request.v != VERSION {
            // A rejected version must not refresh idle time, or an old service never exits.
            return ServiceResponse::failure("version", format!("Protocol version {VERSION} required."));
        }
        let result = match request.op {
            Operation::Search => return self.search(request),
            Operation::Context => return self.context(request),
            Operation::Files => return self.files(|f| f.files(request)),
            Operation::Grep => return self.files(|f| f.grep(request)),
            Operation::Complete => return self.files(|f| f.complete(request)),
            Operation::Select if matches!(request.mode.as_deref(), Some("suggest" | "search")) => return self.select_command(request),
            Operation::Select => return self.files(|f| f.select(request)),
            Operation::Ping | Operation::Shutdown => {
                self.touch();
                let mut response = ServiceResponse::new(true);
                response.pid = Some(std::process::id() as i32);
                response.indexes = Some(self.file_search.instances());
                return response;
            }
            Operation::Record => self.record(request),
            Operation::Suggest | Operation::Pick => self.ranked(request),
        };
        result.unwrap_or_else(|e| ServiceResponse::failure("failed", e.to_string()))
    }

    /// Runs a file operation outside `state`, keeping the service alive while it works.
    fn files(&self, work: impl FnOnce(&FileSearch) -> ServiceResponse) -> ServiceResponse {
        self.touch();
        let response = work(&self.file_search);
        self.touch();
        response
    }

    fn context(&self, request: &ServiceRequest) -> ServiceResponse {
        self.touch();
        let Some(directory) = request.directory.as_deref().filter(|d| d.starts_with('/')) else {
            return ServiceResponse::failure("bad_request", "context needs an absolute directory.");
        };
        let mut response = ServiceResponse::new(true);
        response.context = Some(self.contexts.context(directory));
        response
    }

    fn ranking_context(&self, request: &ServiceRequest, directory: &str) -> RankingContext {
        let mut environment = self.environment.clone();
        if let Some(off) = request.ranking_off.as_ref().filter(|s| s.len() <= 256) {
            environment.insert("SMCLI_RANKING_OFF".into(), off.clone());
        }
        RankingContext::new(&self.contexts.context(directory), request.session.clone(), &environment)
    }

    /// Another process (a fallback record, an import, prune) changed the database: rebuild on next use.
    fn refresh(state: &mut State) -> Result<()> {
        let version = data_version(&state.db)?;
        if version != state.data_version {
            state.data_version = version;
            state.index = None;
            state.corpus = None;
        }
        Ok(())
    }

    fn current_index<'a>(&self, state: &'a mut State) -> Result<&'a mut SuggestionIndex> {
        Self::refresh(state)?;
        if state.index.is_none() {
            let runs = self.history.runs(&self.user, &state.db)?;
            let picks = self.history.picks(5_000, &state.db)?;
            state.index = Some(SuggestionIndex::new(runs, picks));
        }
        Ok(state.index.as_mut().unwrap())
    }

    /// Acknowledged only after the insert commits. A retried event ID is acknowledged again without a new row.
    fn record(&self, request: &ServiceRequest) -> Result<ServiceResponse> {
        let (Some(event), Some(command), Some(shell), Some(session), Some(directory), Some(exit_code)) = (
            &request.event, &request.command, &request.shell, &request.session, &request.directory, request.exit_code,
        ) else {
            return Ok(ServiceResponse::failure("bad_request", "record needs event, command, shell, session, directory and exitCode."));
        };
        let filter = CaptureFilter::new(&SmcliConfig::load(&self.config_path(request))?)?;
        if !filter.allows_run(command, request.expanded_command.as_deref(), directory) {
            let mut response = ServiceResponse::new(true);
            response.event = Some(event.clone());
            response.duplicate = Some(false);
            return Ok(response);
        }
        let mut state = self.lock();
        state.last_activity = Instant::now();
        Self::refresh(&mut state)?;
        let started = request.started_at.unwrap_or_else(now);
        let run = NewRun {
            command, expanded_command: request.expanded_command.as_deref(), shell, directory, exit_code, session, started_at: Some(started), ended_at: request.ended_at,
            pipestatus: request.pipestatus.clone(), event: Some(event.clone()),
        };
        let inserted = self.history.append(&run, request.native_file.as_deref().unwrap_or(""), Some(&state.db))?;
        if inserted {
            state.history_revision += 1;
            let run = Run { command: command.clone(), expanded_command: request.expanded_command.clone(), directory: directory.clone(), started_at: started, exit_code, session: session.clone() };
            if let Some(index) = state.index.as_mut() {
                if !index.add(&run) {
                    state.index = None;
                }
            }
            // The corpus is updated in place (a rebuild is slow at 100k runs); an unused one just stays valid.
            if let Some(corpus) = state.corpus.as_mut() {
                corpus.state.add(&run);
            }
        }
        let mut response = ServiceResponse::new(true);
        response.event = Some(event.clone());
        response.duplicate = Some(!inserted);
        Ok(response)
    }

    /// Suggest, and the picker's first draw (this directory's suggestions, then all history by last run).
    /// Directory context is read before taking `state`, so a record never waits behind discovery.
    fn ranked(&self, request: &ServiceRequest) -> Result<ServiceResponse> {
        let pick = request.op == Operation::Pick;
        let name = if pick { "pick" } else { "suggest" };
        let Some(directory) = request.directory.as_deref().filter(|d| d.starts_with('/')) else {
            return Ok(ServiceResponse::failure("bad_request", format!("{name} needs an absolute directory.")));
        };
        let limit = request.limit.unwrap_or(if pick { 50 } else { 10 });
        if !(1..=1000).contains(&limit) {
            return Ok(ServiceResponse::failure("bad_request", "limit must be 1 to 1000."));
        }
        let mut context = self.ranking_context(request, directory);
        if context.uses(Signal::Tests) {
            if let Some(root) = FileSearch::root(directory) {
                context.candidates.extend(crate::test_candidates::test_candidates(&self.file_search, &root));
            }
        }
        let evidence = FileSearch::root(directory).and_then(|root| self.file_search.ready(&root))
            .map(crate::file_search::evidence::IndexEvidence);
        let presence = |commands: &[String]| smcli_search::command_file_presence(commands, directory, evidence.as_ref().unwrap());
        let git = |commands: &[String]| smcli_search::command_git_activity(commands, directory, evidence.as_ref().unwrap());
        let idents = |commands: &[String]| crate::file_search::evidence::identifier_flags(evidence.as_ref().unwrap(), commands);
        let prefix = if pick { "" } else { request.prefix.as_deref().unwrap_or("") };
        let (index, recent) = self.suggestion_snapshot(prefix, &context, pick)?;
        let mut response = ServiceResponse::new(true);
        response.suggestions = Some(index.suggestions_with_evidence(directory, prefix, limit as usize, now(), Some(&context),
            evidence.as_ref().map(|_| &presence as &dyn Fn(&[String]) -> Vec<bool>),
            evidence.as_ref().map(|_| &idents as &dyn Fn(&[String]) -> Vec<(bool, bool)>),
            evidence.as_ref().map(|_| &git as &dyn Fn(&[String]) -> Vec<f64>)));
        if let Some(recent) = recent {
            response.recent_entries = Some(recent.iter().map(|key| smcli_core::entry::SearchEntry::from_key(key)).collect());
            response.recent = Some(recent.iter().map(|key| smcli_core::entry::command_parts(key).0).collect());
        }
        Ok(response)
    }

    fn suggestion_snapshot(&self, prefix: &str, context: &RankingContext, pick: bool) -> Result<(SuggestionIndex, Option<Vec<String>>)> {
        let mut state = self.lock();
        state.last_activity = Instant::now();
        let index = self.current_index(&mut state)?;
        Ok((index.snapshot(prefix, Some(context)), pick.then(|| index.recent())))
    }

    /// Remembers which suggestion or search result the user chose for what they typed. Best effort for the picker.
    fn select_command(&self, request: &ServiceRequest) -> ServiceResponse {
        let (Some(command), Some(directory), Some(mode)) = (&request.command, &request.directory, &request.mode) else {
            return ServiceResponse::failure("bad_request", "select needs a command, a directory and a mode of suggest or search.");
        };
        if !directory.starts_with('/') || command.is_empty() || command.len() > 1 << 16 || is_smcli_command(command) {
            return ServiceResponse::failure("bad_request", "select needs a command, a directory and a mode of suggest or search.");
        }
        if request.expanded_command.as_deref().is_some_and(|e| !smcli_core::parse::safe_expansion(e) || is_smcli_command(e)) {
            return ServiceResponse::failure("bad_request", "Invalid expanded command.");
        }
        let command = smcli_core::entry::command_key(command, request.expanded_command.as_deref());
        let query: String = request.query.as_deref().unwrap_or("").chars().take(1000).collect();
        let mut state = self.lock();
        state.last_activity = Instant::now();
        let result = (|| -> Result<()> {
            // An unreadable config records nothing rather than skipping the filter.
            let filter = CaptureFilter::new(&SmcliConfig::load(&self.config_path(request))?)?;
            Self::refresh(&mut state)?;
            let time = now();
            if self.history.record_pick(&query, &command, mode, directory, &filter, time, &state.db)? {
                state.data_version = data_version(&state.db)?;
                if let Some(index) = state.index.as_mut() {
                    index.add_pick(&query, &command, time);
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => ServiceResponse::new(true),
            Err(e) => ServiceResponse::failure("failed", e.to_string()),
        }
    }

    fn home(&self) -> PathBuf {
        PathBuf::from(self.environment.get("HOME").cloned().unwrap_or_else(|| "/".into()))
    }

    /// The caller's config file; requests from older clients fall back to the one this service started with.
    fn config_path(&self, request: &ServiceRequest) -> PathBuf {
        match request.config.as_deref().filter(|p| p.starts_with('/')) {
            Some(path) => PathBuf::from(path),
            None => SmcliConfig::location(&self.home(), &self.environment),
        }
    }

    fn current_engine(&self, request: &ServiceRequest) -> Result<Arc<HistorySearch>> {
        self.load_engine(request, || {})
    }

    fn load_engine(&self, request: &ServiceRequest, before_build: impl FnOnce()) -> Result<Arc<HistorySearch>> {
        let path = self.config_path(request);
        let config = SmcliConfig::load(&path)?;
        {
            let mut state = self.lock();
            if let Some((current, engine)) = &state.engine
                && *current == config
            {
                return Ok(Arc::clone(engine));
            }
            // Free the old model before loading the new one, so two are never resident; callers hold `searching`.
            state.engine = None;
        }
        let mut environment = self.environment.clone();
        environment.insert("SMCLI_CONFIG".into(), path.to_string_lossy().into_owned());
        before_build();
        let engine = Arc::new(search_engine(&self.history, &self.home(), &environment)?);
        if SmcliConfig::load(&path)? == config {
            self.lock().engine = Some((config, Arc::clone(&engine)));
        }
        Ok(engine)
    }

    /// Loads the embedding model in the background so the first search doesn't pay for it; a search waits on `searching`.
    fn warm_search(self: &Arc<Self>) {
        let service = Arc::clone(self);
        std::thread::spawn(move || {
            let _searching = service.searching.lock();
            let request = ServiceRequest::new(Operation::Search);
            let _ = service.current_engine(&request);
        });
    }

    /// Blocks this connection's thread, not `state`, for the length of the search.
    fn search(&self, request: &ServiceRequest) -> ServiceResponse {
        let _searching = self.searching.lock().unwrap_or_else(|e| e.into_inner());
        self.touch();
        let (Some(query), Some(directory)) = (&request.query, request.directory.as_deref().filter(|d| d.starts_with('/'))) else {
            return ServiceResponse::failure("bad_request", "search needs a query and an absolute directory.");
        };
        let limit = request.limit.unwrap_or(10);
        if !(1..=1000).contains(&limit) {
            return ServiceResponse::failure("bad_request", "limit must be 1 to 1000.");
        }
        let result = (|| -> Result<ServiceResponse> {
            let context = self.ranking_context(request, directory);
            let project = context.project_root.clone().filter(|_| context.uses(Signal::Project));
            let engine = self.current_engine(request)?;
            let signals = {
                let mut state = self.lock();
                Self::refresh(&mut state)?;
                let index = self.current_index(&mut state)?;
                let mut signals = SearchSignals {
                    picks: index.picks().clone(),
                    shell_path: smcli_core::service_protocol::shell_path_snapshot(request.shell_path.as_deref()),
                    directory: Some(directory.into()),
                    ..Default::default()
                };
                if context.uses(Signal::Transition)
                    && let Some(previous) = request.session.as_deref().and_then(|s| index.previous_command(s))
                {
                    signals.followers = index.following(&previous);
                    signals.previous = Some(previous);
                }
                signals.context = Some(context.clone());
                signals
            };
            // Lock before reading the corpus so prune cannot leave a stale snapshot in the index.
            let lock = engine.lock_index(20)?;
            // Read native history files and stats outside `state` so records don't wait on I/O.
            let (entries, programs) = self.current_corpus(request, directory, project.as_deref())?;
            let filter = request.filter.clone().unwrap_or_default();
            let evidence = FileSearch::root(directory).and_then(|root| self.file_search.ready(&root))
                .map(crate::file_search::evidence::IndexEvidence);
            let hits = engine.search_with_evidence_using_lock(query, &entries, limit as usize, &filter, Some(&programs), &signals, now(),
                evidence.as_ref().map(|e| e as &dyn smcli_search::ProjectEvidence), lock.as_ref())?;
            let mut response = ServiceResponse::new(true);
            response.hits = Some(hits);
            Ok(response)
        })();
        result.unwrap_or_else(|e| ServiceResponse::failure("failed", e.to_string()))
    }

    /// The caller's view of the environment: its HISTFILE and SHELL pick the native history files.
    /// Copies of the corpus entries and programs, so the search runs outside `state`.
    // shortcut: copies the corpus per search; share it (Arc + copy-on-record) if it shows up in profiles.
    fn current_corpus(&self, request: &ServiceRequest, directory: &str, project: Option<&str>)
        -> Result<(HashMap<String, SearchEntry>, HashMap<String, HashSet<String>>)> {
        self.build_corpus(request, directory, project, || {})
    }

    fn build_corpus(&self, request: &ServiceRequest, directory: &str, project: Option<&str>, before_publish: impl FnOnce())
        -> Result<(HashMap<String, SearchEntry>, HashMap<String, HashSet<String>>)> {
        let mut environment = self.environment.clone();
        environment.remove("HISTFILE");
        environment.remove("SHELL");
        environment.extend(request.histfile.clone().map(|h| ("HISTFILE".to_string(), h)));
        environment.extend(request.login_shell.clone().map(|s| ("SHELL".to_string(), s)));
        let files = history_files(&self.home(), &environment)?;
        let stamp: Vec<String> = files
            .iter()
            .map(|f| {
                let meta = std::fs::metadata(&f.path).ok();
                format!("{} {:?}", f.path.display(), meta.map(|m| (m.len(), m.mtime(), m.mtime_nsec(), m.dev(), m.ino())))
            })
            .collect();
        let version = {
            let mut state = self.lock();
            Self::refresh(&mut state)?;
            if let Some(c) = state.corpus.as_ref().filter(|c| {
                c.state.directory == directory && c.state.project.as_deref() == project && c.native == stamp
                    && c.built.elapsed() < Duration::from_secs(300)
            }) {
                return Ok((c.state.entries.clone(), c.state.programs.clone()));
            }
            (state.data_version, state.history_revision)
        };
        let native: Vec<String> =
            files.iter().filter_map(|f| f.read().ok()).flat_map(|t| t.split('\n').map(String::from).collect::<Vec<_>>()).collect();
        let as_of = now();
        let captured = self.history.command_stats(Some(directory), project, as_of)?;
        let corpus = CorpusState::new(captured, &native, directory, project, as_of);
        let copies = (corpus.entries.clone(), corpus.programs.clone());
        before_publish();
        let mut state = self.lock();
        Self::refresh(&mut state)?;
        // Never replace an updated cache with a snapshot taken before a committed record.
        if (state.data_version, state.history_revision) == version {
            state.corpus = Some(SearchCorpus { native: stamp, built: Instant::now(), state: corpus });
        }
        Ok(copies)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture { service: Arc<Service>, home: PathBuf }
    impl Fixture {
        fn new() -> Self { Self::with_idle(Duration::from_secs(600)) }
        fn with_idle(idle: Duration) -> Self {
            let home = std::env::temp_dir().join(format!("smcli-service-c-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir_all(&home).unwrap();
            let history = CapturedHistory::new(home.join("history.db"));
            let environment = HashMap::from([("HOME".into(), home.to_string_lossy().into_owned()), ("SMCLI_CONFIG".into(), home.join("config.toml").to_string_lossy().into_owned())]);
            let location = ServiceLocation::new(&history.path, &environment);
            let service = Service::new(history, location, idle, environment).unwrap();
            Self { service, home }
        }
        fn request(&self, op: Operation) -> ServiceRequest {
            let mut r = ServiceRequest::new(op);
            r.directory = Some(self.home.to_string_lossy().into_owned());
            r
        }
        fn record(&self) {
            let mut r = self.request(Operation::Record);
            r.event = Some("01234567-89ab-4cde-8012-3456789abcde".into());
            r.command = Some("echo concurrent".into());
            r.shell = Some("zsh".into());
            r.session = Some("test".into());
            r.exit_code = Some(0);
            self.service.record(&r).unwrap();
            assert_eq!(self.service.lock().history_revision, 1);
        }
    }
    impl Drop for Fixture { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.home); } }

    #[test]
    fn busy_response_includes_service_build() {
        use std::io::BufRead;
        let f = Fixture::new();
        assert!(f.service.build.is_some());
        let (server, client) = UnixStream::pair().unwrap();
        f.service.reject_busy(server);
        let mut line = String::new();
        std::io::BufReader::new(client).read_line(&mut line).unwrap();
        let response: ServiceResponse = decode_line(line.as_bytes()).unwrap();
        assert!(!response.ok);
        assert_eq!(response.build, f.service.build);
        assert!(line.contains("busy"));
    }

    #[test]
    fn idle_shutdown_waits_for_admitted_work() {
        let f = Fixture::with_idle(Duration::from_millis(30));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let service = Arc::clone(&f.service);
        let work = std::thread::spawn(move || {
            let _guard = Leave::admit(service).unwrap();
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        ready_rx.recv().unwrap();
        std::thread::sleep(Duration::from_millis(60));
        assert!(!f.service.claim_idle_shutdown());
        release_tx.send(()).unwrap();
        work.join().unwrap();
        assert!(f.service.claim_idle_shutdown());
        assert!(Leave::admit(Arc::clone(&f.service)).is_none());
    }

    #[test]
    fn admission_is_bounded_and_released_on_drop() {
        let f = Fixture::new();
        let mut guards: Vec<_> = (0..MAX_CONNECTIONS).map(|_| Leave::admit(Arc::clone(&f.service)).unwrap()).collect();
        assert!(Leave::admit(Arc::clone(&f.service)).is_none());
        drop(guards.pop());
        assert!(Leave::admit(Arc::clone(&f.service)).is_some());
        drop(guards);
        assert_eq!(f.service.active.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn file_selection_filters_query_and_actual_directory() {
        let f = Fixture::new();
        let mut r = f.request(Operation::Select);
        r.path = Some("example.txt".into());
        r.mode = Some("grep".into());
        r.query = Some("AKIAIOSFODNN7EXAMPLE".into());
        f.service.file_search.select(&r);
        let count = || f.service.lock().db.query_row("SELECT count(*) FROM file_selections", [], |row| row.get::<_, i64>(0)).unwrap();
        assert_eq!(count(), 0);
        std::fs::write(f.home.join("config.toml"), r#"{"cwdFilter":[".*"]}"#).unwrap();
        r.query = Some("ordinary".into());
        f.service.file_search.select(&r);
        assert_eq!(count(), 0);
        std::fs::write(f.home.join("config.toml"), "{}").unwrap();
        f.service.file_search.select(&r);
        assert_eq!(count(), 1);
    }

    #[test]
    fn record_during_suggestion_evidence_is_not_blocked() {
        let f = Fixture::new();
        let context = RankingContext::default();
        let (snapshot, _) = f.service.suggestion_snapshot("", &context, false).unwrap();
        let (entered, started) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let (done, recorded) = std::sync::mpsc::channel();
        let directory = f.home.to_string_lossy().into_owned();
        let completed = std::thread::scope(|scope| {
            scope.spawn(move || {
                let slow = |commands: &[String]| {
                    entered.send(()).unwrap();
                    wait.recv().unwrap();
                    vec![false; commands.len()]
                };
                snapshot.suggestions_with_evidence(&directory, "", 10, now(), Some(&context), Some(&slow), None, None);
            });
            started.recv_timeout(Duration::from_secs(2)).unwrap();
            scope.spawn(|| { f.record(); done.send(()).unwrap(); });
            let completed = recorded.recv_timeout(Duration::from_secs(2)).is_ok();
            release.send(()).unwrap();
            completed
        });
        assert!(completed, "record waited for suggestion evidence");
    }

    #[test]
    fn record_during_corpus_build_prevents_stale_publication() {
        let f = Fixture::new();
        let r = f.request(Operation::Search);
        let directory = r.directory.as_deref().unwrap();
        f.service.build_corpus(&r, directory, None, || f.record()).unwrap();
        assert!(f.service.lock().corpus.is_none());
        let (entries, _) = f.service.current_corpus(&r, directory, None).unwrap();
        assert!(entries.contains_key("echo concurrent"));
    }

    #[test]
    fn equal_length_native_rewrite_invalidates_corpus() {
        let f = Fixture::new();
        let path = f.home.join(".zsh_history");
        std::fs::write(&path, "echo before\n").unwrap();
        let first = std::fs::metadata(&path).unwrap().modified().unwrap();
        let mut r = f.request(Operation::Search);
        r.histfile = Some(path.to_string_lossy().into_owned());
        r.login_shell = Some("/bin/zsh".into());
        let directory = r.directory.as_deref().unwrap();
        f.service.current_corpus(&r, directory, None).unwrap();
        std::fs::write(&path, "echo after!\n").unwrap();
        let seconds = first.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let nanos = (first.duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos() + 1) % 1_000_000_000;
        let modified = std::time::UNIX_EPOCH + Duration::new(seconds, nanos);
        std::fs::File::options().write(true).open(&path).unwrap().set_times(std::fs::FileTimes::new().set_modified(modified)).unwrap();
        let (entries, _) = f.service.current_corpus(&r, directory, None).unwrap();
        assert!(entries.contains_key("echo after!"));
        assert!(!entries.contains_key("echo before"));
    }

    #[test]
    fn engine_reload_does_not_hold_state_lock() {
        let f = Fixture::new();
        let r = f.request(Operation::Search);
        f.service.load_engine(&r, || {
            assert!(f.service.state.try_lock().is_ok());
            f.record();
        }).unwrap();
        assert!(f.service.lock().engine.is_some());
    }

    #[test]
    fn engine_reload_drops_old_model_first() {
        let f = Fixture::new();
        let r = f.request(Operation::Search);
        let old = f.service.current_engine(&r).unwrap();
        f.service.lock().engine.as_mut().unwrap().0.ctrl_r_key = Some(false);
        f.service.load_engine(&r, || assert!(f.service.lock().engine.is_none())).unwrap();
        assert!(!Arc::ptr_eq(&old, &f.service.lock().engine.as_ref().unwrap().1));
    }

    #[test]
    fn suggest_includes_project_test_candidates() {
        let f = Fixture::new();
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("Cargo.toml"), "[package]\nname = \"testproj\"\nversion = \"0.1.0\"\n").unwrap();
        std::fs::create_dir(project.path().join("src")).unwrap();
        std::fs::write(project.path().join("src/lib.rs"), "#[test]\nfn my_test() {}\n").unwrap();
        assert!(std::process::Command::new("git").args(["init", "-q"]).arg(project.path()).status().unwrap().success());
        let mut warm = f.request(Operation::Files);
        warm.directory = Some(project.path().to_string_lossy().into_owned());
        warm.query = Some("".into());
        assert!(f.service.file_search.files(&warm).ok);
        let root = FileSearch::root(project.path().to_str().unwrap()).unwrap();
        let test_candidates = crate::test_candidates::test_candidates(&f.service.file_search, &root);
        assert!(test_candidates.iter().any(|c| c.command == "cargo test my_test"), "{test_candidates:?}");
        let mut r = f.request(Operation::Suggest);
        r.directory = Some(project.path().to_string_lossy().into_owned());
        r.prefix = Some("cargo test".into());
        let response = f.service.ranked(&r).unwrap();
        let suggestions: Vec<_> = response.suggestions.unwrap();
        let test = suggestions.iter().find(|s| s.command == "cargo test my_test").expect("{suggestions:?}");
        assert!(test.why.as_ref().unwrap().iter().any(|w| w == "project test"), "{test:?}");
        let mut off = f.request(Operation::Suggest);
        off.directory = Some(project.path().to_string_lossy().into_owned());
        off.prefix = Some("cargo test".into());
        off.ranking_off = Some("tests".into());
        let response = f.service.ranked(&off).unwrap();
        let suggestions: Vec<_> = response.suggestions.unwrap();
        assert!(!suggestions.iter().any(|s| s.command == "cargo test my_test"), "{suggestions:?}");
    }
}

struct Leave(Arc<Service>);

impl Leave {
    fn admit(service: Arc<Service>) -> Option<Self> {
        service.active.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n < MAX_CONNECTIONS).then_some(n + 1)).ok()?;
        Some(Self(service))
    }
}

impl Drop for Leave {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// SIGTERM, SIGINT and SIGHUP remove the socket while it is still ours and exit. The handler only calls
/// async-signal-safe functions (stat, unlink, _exit) on data fixed before it is installed.
mod signals {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::OnceLock;

    static SOCKET: OnceLock<(CString, u64)> = OnceLock::new();

    extern "C" fn stop(_: libc::c_int) {
        if let Some((path, inode)) = SOCKET.get() {
            // SAFETY: stat and unlink are async-signal-safe; `path` is a valid C string that lives forever.
            unsafe {
                let mut info: libc::stat = std::mem::zeroed();
                if libc::stat(path.as_ptr(), &mut info) == 0 && info.st_ino as u64 == *inode {
                    libc::unlink(path.as_ptr());
                }
            }
        }
        // SAFETY: _exit is async-signal-safe.
        unsafe { libc::_exit(0) }
    }

    pub fn install(socket: &Path, inode: Option<u64>) {
        if let (Ok(path), Some(inode)) = (CString::new(socket.as_os_str().as_bytes()), inode) {
            let _ = SOCKET.set((path, inode));
        }
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            // SAFETY: `stop` has the signature signal(3) expects.
            unsafe { libc::signal(signal, stop as extern "C" fn(libc::c_int) as libc::sighandler_t) };
        }
    }
}
