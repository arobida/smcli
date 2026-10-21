//! The long-lived owner of one history database: it keeps the database open and suggestions in memory, and
//! answers smcli clients over a private Unix-domain socket. Started on demand by clients; exits when idle.

use std::collections::HashMap;
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
        let fff_idle = Duration::from_secs_f64(environment.get("SMCLI_FFF_IDLE").and_then(|v| v.parse().ok()).unwrap_or(600.0));
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
                Box::new(move |root, path, mode, query| {
                    let Some(service) = record_handle.upgrade() else { return Ok(()) };
                    let mut state = service.lock();
                    state.last_activity = Instant::now();
                    service.history.record_selection(root, path, mode, query, now(), &state.db)
                }),
            );
            Service {
                build: std::env::current_exe().ok().and_then(|exe| build(&exe)),
                history,
                location,
                idle,
                user: user_name(),
                environment,
                state: Mutex::new(State { db, index: None, data_version: version, last_activity: Instant::now(), engine: None, corpus: None }),
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

    pub fn run(self: &Arc<Self>) -> Result<()> {
        let listener = listen_unix(&self.location.socket)?;
        let socket_inode = inode(&self.location.socket);
        signals::install(&self.location.socket, socket_inode);
        let watcher = Arc::clone(self);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(15));
            // Idle too long, or our socket was removed or replaced: let a fresh service take over.
            watcher.file_search.evict_idle();
            let idle = watcher.lock().last_activity.elapsed() > watcher.idle;
            if idle || inode(&watcher.location.socket) != socket_inode {
                watcher.stop(socket_inode);
            }
        });
        self.warm_search();
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let service = Arc::clone(self);
                    std::thread::spawn(move || service.serve(stream, socket_inode));
                }
                // Out of descriptors: back off instead of spinning until one frees up.
                Err(e) if matches!(e.raw_os_error(), Some(libc::EMFILE | libc::ENFILE)) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => {}
            }
        }
        Ok(())
    }

    /// Removes the socket only while it is still ours, then exits (which releases the lock).
    fn stop(&self, socket_inode: Option<u64>) -> ! {
        if inode(&self.location.socket).is_some() && inode(&self.location.socket) == socket_inode {
            let _ = std::fs::remove_file(&self.location.socket);
        }
        std::process::exit(0)
    }

    fn serve(&self, stream: UnixStream, socket_inode: Option<u64>) {
        let mut connection = SocketConnection::from(stream);
        // SAFETY: getuid has no preconditions.
        if connection.peer_uid() != Some(unsafe { libc::getuid() }) {
            return;
        }
        let admitted = self.active.fetch_add(1, Ordering::SeqCst) < MAX_CONNECTIONS;
        let _guard = Leave(&self.active);
        let timeout = Duration::from_secs_f64(IO_TIMEOUT);
        let mut shutdown = false;
        let mut response = if !admitted {
            ServiceResponse::failure("busy", "Too many connections.")
        } else {
            match connection.read_line(MAX_MESSAGE_BYTES, timeout) {
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
            }
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
            self.touch();
            return ServiceResponse::failure("version", format!("Protocol version {VERSION} required."));
        }
        let result = match request.op {
            Operation::Search => return self.search(request),
            Operation::Context => return self.context(request),
            Operation::Files => return self.files(|f| f.files(request)),
            Operation::Grep => return self.files(|f| f.grep(request)),
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
        RankingContext::new(&self.contexts.context(directory), request.session.clone(), &self.environment)
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
        let mut state = self.lock();
        state.last_activity = Instant::now();
        Self::refresh(&mut state)?;
        let started = request.started_at.unwrap_or_else(now);
        let run = NewRun {
            command, shell, directory, exit_code, session, started_at: Some(started), ended_at: request.ended_at,
            pipestatus: request.pipestatus.clone(), event: Some(event.clone()),
        };
        let inserted = self.history.append(&run, request.native_file.as_deref().unwrap_or(""), Some(&state.db))?;
        // Our own write moved data_version; keep the caches by updating them in place instead.
        state.data_version = data_version(&state.db)?;
        if inserted {
            let run = Run { command: command.clone(), directory: directory.clone(), started_at: started, exit_code, session: session.clone() };
            if let Some(index) = state.index.as_mut() {
                index.add(&run);
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
        let context = self.ranking_context(request, directory);
        let mut state = self.lock();
        state.last_activity = Instant::now();
        let index = self.current_index(&mut state)?;
        let prefix = if pick { "" } else { request.prefix.as_deref().unwrap_or("") };
        let mut response = ServiceResponse::new(true);
        response.suggestions = Some(index.suggestions(directory, prefix, limit as usize, Some(&context)));
        if pick {
            response.recent = Some(index.recent());
        }
        Ok(response)
    }

    /// Remembers which suggestion or search result the user chose for what they typed. Best effort for the picker.
    fn select_command(&self, request: &ServiceRequest) -> ServiceResponse {
        let (Some(command), Some(directory), Some(mode)) = (&request.command, &request.directory, &request.mode) else {
            return ServiceResponse::failure("bad_request", "select needs a command, a directory and a mode of suggest or search.");
        };
        if !directory.starts_with('/') || command.is_empty() || command.len() > 1 << 16 || is_smcli_command(command) {
            return ServiceResponse::failure("bad_request", "select needs a command, a directory and a mode of suggest or search.");
        }
        let query: String = request.query.as_deref().unwrap_or("").chars().take(1000).collect();
        let mut state = self.lock();
        state.last_activity = Instant::now();
        let result = (|| -> Result<()> {
            // An unreadable config records nothing rather than skipping the filter.
            let filter = CaptureFilter::new(&SmcliConfig::load(&self.config_path(request))?)?;
            Self::refresh(&mut state)?;
            let time = now();
            if self.history.record_pick(&query, command, mode, directory, &filter, time, &state.db)? {
                state.data_version = data_version(&state.db)?;
                if let Some(index) = state.index.as_mut() {
                    index.add_pick(&query, command, time);
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

    fn current_engine(&self, state: &mut State, request: &ServiceRequest) -> Result<Arc<HistorySearch>> {
        let path = self.config_path(request);
        let config = SmcliConfig::load(&path)?;
        if let Some((current, engine)) = &state.engine
            && *current == config
        {
            return Ok(Arc::clone(engine));
        }
        let mut environment = self.environment.clone();
        environment.insert("SMCLI_CONFIG".into(), path.to_string_lossy().into_owned());
        let engine = Arc::new(search_engine(&self.history, &self.home(), &environment)?);
        state.engine = Some((config, Arc::clone(&engine)));
        Ok(engine)
    }

    /// Loads the embedding model in the background so the first search doesn't pay for it; a search waits on `searching`.
    fn warm_search(self: &Arc<Self>) {
        let service = Arc::clone(self);
        std::thread::spawn(move || {
            let _searching = service.searching.lock();
            let mut state = service.lock();
            let _ = service.current_engine(&mut state, &ServiceRequest::new(Operation::Search));
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
            let (engine, entries, programs, signals) = {
                let mut state = self.lock();
                Self::refresh(&mut state)?;
                let index = self.current_index(&mut state)?;
                let mut signals = SearchSignals { picks: index.picks().clone(), ..Default::default() };
                if context.uses(Signal::Transition)
                    && let Some(previous) = request.session.as_deref().and_then(|s| index.previous_command(s))
                {
                    signals.followers = index.following(&previous);
                    signals.previous = Some(previous);
                }
                signals.context = Some(context);
                let engine = self.current_engine(&mut state, request)?;
                let corpus = self.current_corpus(&mut state, request, directory, project.as_deref())?;
                // shortcut: copies the corpus per search to search outside the lock; share it (Arc + copy-on-record) if it shows up in profiles.
                (engine, corpus.state.entries.clone(), corpus.state.programs.clone(), signals)
            };
            let filter = request.filter.clone().unwrap_or_default();
            let hits = engine.search(query, &entries, limit as usize, &filter, Some(&programs), &signals, now())?;
            let mut response = ServiceResponse::new(true);
            response.hits = Some(hits);
            Ok(response)
        })();
        result.unwrap_or_else(|e| ServiceResponse::failure("failed", e.to_string()))
    }

    /// The caller's view of the environment: its HISTFILE and SHELL pick the native history files.
    fn current_corpus<'a>(&self, state: &'a mut State, request: &ServiceRequest, directory: &str, project: Option<&str>)
        -> Result<&'a SearchCorpus> {
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
                format!("{} {} {}", f.path.display(), meta.as_ref().map_or(-1, |m| m.len() as i64), meta.map_or(0, |m| m.mtime()))
            })
            .collect();
        let fresh = state.corpus.as_ref().is_some_and(|c| {
            c.state.directory == directory && c.state.project.as_deref() == project && c.native == stamp
                && c.built.elapsed() < Duration::from_secs(300)
        });
        if !fresh {
            let native: Vec<String> =
                files.iter().filter_map(|f| f.read().ok()).flat_map(|t| t.split('\n').map(String::from).collect::<Vec<_>>()).collect();
            let as_of = now();
            let captured = self.history.command_stats(Some(directory), project, as_of)?;
            state.corpus = Some(SearchCorpus {
                native: stamp,
                built: Instant::now(),
                state: CorpusState::new(captured, &native, directory, project, as_of),
            });
        }
        Ok(state.corpus.as_ref().unwrap())
    }
}

struct Leave<'a>(&'a AtomicUsize);

impl Drop for Leave<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
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
