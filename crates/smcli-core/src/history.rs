use std::collections::HashMap;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, OpenFlags, params, params_from_iter, types::Value};

use crate::config::CaptureFilter;
use crate::entry::trim_slash;
use crate::parse::{CommandBreakdown, base_name, command_flags, is_smcli_command};
use crate::ranking::{RankingContext, SelectionLog, Signal, apply_ranking_signals, suggestion_why, uses};
use crate::time::{format_iso8601, now};

pub use crate::service_protocol::Suggestion;

/// Usage halves every two weeks, so habits outweigh one-offs without old habits winning forever.
pub const USAGE_HALF_LIFE: f64 = 14.0 * 86_400.0;
/// Exit codes that do not mean the command failed: stopped with Ctrl-C (130) or by a closed pipe (141).
pub const INTERRUPTED_EXIT_CODES: [i64; 2] = [130, 141];
/// Programs whose exit code 1 is an answer ("no match", "files differ", "false"), not an error.
const ANSWERING_PROGRAMS: [&str; 16] =
    ["grep", "egrep", "fgrep", "rg", "ag", "ack", "pgrep", "diff", "cmp", "colordiff", "test", "[", "which", "type", "command", "false"];

pub const VERSION: i64 = 7;
/// A short-lived build stored usage scores in a `usage` table at version 4. Usage is now computed when read,
/// so such databases are taken back to version 3, which every binary since flags can open.
/// Real schema changes therefore start at version 5.
const DISCARDED_USAGE_VERSION: i64 = 4;

const SCHEMA_V1: &str = "
    CREATE TABLE sessions (
        id TEXT PRIMARY KEY, user TEXT NOT NULL, host TEXT NOT NULL, shell TEXT NOT NULL
    ) WITHOUT ROWID;
    CREATE TABLE commands (
        id INTEGER PRIMARY KEY, session TEXT NOT NULL REFERENCES sessions(id),
        started_at REAL NOT NULL, duration_ms INTEGER, directory TEXT NOT NULL,
        command TEXT NOT NULL, exit_code INTEGER NOT NULL
    );
    CREATE INDEX commands_by_command ON commands(command, started_at);
    CREATE INDEX commands_by_directory ON commands(directory, started_at);
    ";
/// Fields borrowed from Atuin's history table: a UUID for deduplication, soft deletion, and authorship.
const MIGRATION_TO_V2: &str = "
    ALTER TABLE commands ADD COLUMN uuid TEXT;
    ALTER TABLE commands ADD COLUMN deleted_at REAL;
    ALTER TABLE commands ADD COLUMN author TEXT;
    ALTER TABLE commands ADD COLUMN intent TEXT;
    CREATE UNIQUE INDEX commands_by_uuid ON commands(uuid);
    PRAGMA user_version = 2;
    ";
/// Option names, space-separated (empty when none), so flags can be read without re-parsing.
/// Existing rows are filled in by `backfill_flags`.
const MIGRATION_TO_V3: &str = "
    ALTER TABLE commands ADD COLUMN flags TEXT;
    PRAGMA user_version = 3;
    ";
/// Space-separated exit status of each pipeline stage; NULL when the shell doesn't report it.
const MIGRATION_TO_V5: &str = "
    ALTER TABLE commands ADD COLUMN pipestatus TEXT;
    PRAGMA user_version = 5;
    ";
/// Files the user explicitly picked in the file or grep picker (never inferred from command text), for frecency.
/// `path` is relative to `root`; `query` is what was typed.
const MIGRATION_TO_V6: &str = "
    CREATE TABLE file_selections (
        id INTEGER PRIMARY KEY, root TEXT NOT NULL, path TEXT NOT NULL, mode TEXT NOT NULL,
        query TEXT NOT NULL, selected_at REAL NOT NULL
    );
    CREATE INDEX file_selections_by_root ON file_selections(root, path);
    PRAGMA user_version = 6;
    ";
/// Commands the user explicitly picked in the picker (from suggestions, history or search results), with what was
/// typed at the time, so a similar query later puts that command first. `kind` is `suggest` or `search`.
const MIGRATION_TO_V7: &str = "
    CREATE TABLE command_selections (
        id INTEGER PRIMARY KEY, query TEXT NOT NULL, command TEXT NOT NULL, kind TEXT NOT NULL,
        directory TEXT NOT NULL, selected_at REAL NOT NULL
    );
    PRAGMA user_version = 7;
    ";

pub fn answers_with_exit_code(command: &str) -> bool {
    let program = CommandBreakdown::parse(command).map(|b| b.main_command).or_else(|| command.split_whitespace().next().map(String::from));
    program.is_some_and(|p| ANSWERING_PROGRAMS.contains(&base_name(&p)))
}

/// The user smcli records runs under, like Foundation's `NSUserName()`.
pub fn user_name() -> String {
    // SAFETY: getpwuid returns a pointer into static storage or null; it is read before any other call.
    unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if !entry.is_null() && !(*entry).pw_name.is_null() {
            return std::ffi::CStr::from_ptr((*entry).pw_name).to_string_lossy().into_owned();
        }
    }
    std::env::var("USER").unwrap_or_else(|_| "unknown".into())
}

fn host_name() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is writable for its whole length.
    if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } == 0 {
        let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
        return String::from_utf8_lossy(&buffer[..end]).into_owned();
    }
    "unknown".into()
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CommandStats {
    pub runs: i64,
    pub last_run: f64,
    pub last_directory: String,
    pub last_exit_code: i64,
    pub author: Option<String>,
    pub intent: Option<String>,
    pub flags: Vec<String>,
    /// Runs weighted by age (a run two weeks old counts half), so current habits outrank old ones.
    pub frecency: f64,
    pub successes: i64,
    /// Runs in the directory passed to `command_stats`, or below it.
    pub runs_here: i64,
    /// Runs in the project root passed to `command_stats`, or below it.
    pub runs_project: i64,
}

/// One captured run, as the in-memory suggestion index and search corpus need it.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub command: String,
    pub directory: String,
    pub started_at: f64,
    pub exit_code: i64,
    /// The shell session, whose runs form the command transitions.
    pub session: String,
}

/// One finished command, as a shell hook reports it.
#[derive(Debug, Clone)]
pub struct NewRun<'a> {
    pub command: &'a str,
    pub shell: &'a str,
    pub directory: &'a str,
    pub exit_code: i64,
    pub session: &'a str,
    /// Unix epoch seconds supplied by the shell hook; without them the start is the save time and the duration is unknown.
    pub started_at: Option<f64>,
    pub ended_at: Option<f64>,
    pub pipestatus: Option<Vec<i64>>,
    /// The caller's ID for this record (stored as `uuid`); `None` makes a fresh one.
    pub event: Option<String>,
}

impl<'a> NewRun<'a> {
    pub fn new(command: &'a str, shell: &'a str, directory: &'a str, exit_code: i64) -> Self {
        Self { command, shell, directory, exit_code, session: "unknown", started_at: None, ended_at: None, pipestatus: None, event: None }
    }
}

/// Epoch seconds from 1970 up to year 5138: finite, and any difference of two in milliseconds fits an i64.
pub fn is_timestamp(time: f64) -> bool {
    time.is_finite() && (0.0..=1e11).contains(&time)
}

/// Event IDs are lowercase UUIDs, so a client can't collide with Atuin's IDs or smuggle in arbitrary text.
pub fn is_event_id(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => *b == b'-',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(b),
        })
}

fn new_event_id() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = file.read_exact(&mut bytes);
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// Parses a shell's `pipestatus` words ("0 1 0"). Empty means not reported.
pub fn parse_pipestatus(text: &str) -> Result<Option<Vec<i64>>> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Ok(None);
    }
    let codes: Vec<i64> = words.iter().filter_map(|w| w.parse().ok()).collect();
    if codes.len() != words.len() || codes.len() > 256 || !codes.iter().all(|c| (0..=255).contains(c)) {
        bail!("Invalid pipestatus: {text}");
    }
    Ok(Some(codes))
}

/// smcli's own SQLite log of captured commands. Native shell history is never written.
#[derive(Debug, Clone)]
pub struct CapturedHistory {
    pub path: PathBuf,
}

impl CapturedHistory {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn location(home: &Path, environment: &HashMap<String, String>) -> PathBuf {
        match environment.get("SMCLI_DB").filter(|p| !p.is_empty()) {
            Some(path) => PathBuf::from(path),
            None => home.join(".local/share/smcli/history.db"),
        }
    }

    fn exists(&self) -> bool {
        self.path.exists()
    }

    /// A long-lived writable connection, for the service and for repeated writes.
    pub fn open_connection(&self) -> Result<Connection> {
        self.connect(true)
    }

    /// Records one finished command. Returns false when its event ID was already stored.
    /// `db` reuses an open connection.
    pub fn append(&self, run: &NewRun, native_file: &str, db: Option<&Connection>) -> Result<bool> {
        let pipestatus = run.pipestatus.as_deref().unwrap_or(&[]);
        if !["bash", "zsh", "fish"].contains(&run.shell)
            || !(0..=255).contains(&run.exit_code)
            || !run.directory.starts_with('/')
            || run.command.is_empty()
            || run.session.is_empty()
            || pipestatus.len() > 256
            || !pipestatus.iter().all(|c| (0..=255).contains(c))
        {
            bail!("Invalid shell, directory, exit code, pipestatus, session, or empty command.");
        }
        self.check_separate(native_file)?;
        // Converting an unbounded float to an integer saturates silently, and these come from shells and service clients.
        if ![run.started_at, run.ended_at].iter().all(|t| t.is_none_or(is_timestamp)) {
            bail!("Invalid start or end time.");
        }
        let start = run.started_at.unwrap_or_else(now);
        let duration = run.ended_at.map(|end| (((end - start) * 1000.0).round() as i64).max(0));
        let event = run.event.clone().unwrap_or_else(new_event_id);
        if !is_event_id(&event) {
            bail!("Invalid event ID.");
        }
        let owned;
        let db = match db {
            Some(db) => db,
            None => {
                owned = self.connect(true)?;
                &owned
            }
        };
        insert(db, &Insert {
            session: run.session, user: &user_name(), host: &host_name(), shell: run.shell, started_at: start,
            duration_ms: duration, directory: run.directory, command: run.command, exit_code: run.exit_code,
            uuid: Some(&event), pipestatus: run.pipestatus.as_deref(),
        }, true)
    }

    /// Refuses a native history file that is this database, by path or by hard link. Checked before anything opens
    /// the database: opening it would create or write SQLite pages into the shell's history file.
    pub fn check_separate(&self, native_file: &str) -> Result<()> {
        if native_file.is_empty() {
            return Ok(());
        }
        let resolve = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        if resolve(&self.path) == resolve(Path::new(native_file)) {
            bail!("SMCLI_DB must be separate from the shell's native HISTFILE.");
        }
        if let (Ok(log), Ok(native)) = (std::fs::metadata(&self.path), std::fs::metadata(native_file))
            && log.dev() == native.dev() && log.ino() == native.ino() {
                bail!("SMCLI_DB must not be a hard link to native HISTFILE.");
            }
        Ok(())
    }

    /// Records one explicit file selection.
    pub fn record_selection(&self, root: &str, path: &str, mode: &str, query: &str, time: f64, db: &Connection) -> Result<()> {
        db.execute(
            "INSERT INTO file_selections (root, path, mode, query, selected_at) VALUES (?, ?, ?, ?, ?)",
            params![root, path, mode, query, time],
        )?;
        Ok(())
    }

    /// Selection count per path under `root`, decayed like command usage, so recent picks outrank old ones.
    pub fn selection_scores(&self, root: &str, now: f64, db: &Connection) -> Result<HashMap<String, f64>> {
        let mut statement =
            db.prepare("SELECT path, sum(smcli_decay(? - selected_at)) FROM file_selections WHERE root = ? GROUP BY path")?;
        let rows = statement.query_map(params![now, root], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Records that the user picked `command` after typing `query` (empty for the unfiltered list).
    /// A pick can come from native history, which never passed the capture filter, so it is applied here, to the
    /// command and to what was typed. Returns false when the filter kept it out.
    pub fn record_pick(&self, query: &str, command: &str, kind: &str, directory: &str, filter: &CaptureFilter, time: f64,
                       db: &Connection) -> Result<bool> {
        if !filter.allows(command, directory) || (!query.is_empty() && !filter.allows(query, directory)) {
            return Ok(false);
        }
        db.execute(
            "INSERT INTO command_selections (query, command, kind, directory, selected_at) VALUES (?, ?, ?, ?, ?)",
            params![query, command, kind, directory, time],
        )?;
        Ok(true)
    }

    /// The most recent picks, oldest first; older ones have faded anyway.
    pub fn picks(&self, limit: i64, db: &Connection) -> Result<SelectionLog> {
        let mut statement = db.prepare("SELECT query, command, selected_at FROM command_selections ORDER BY id DESC LIMIT ?")?;
        let rows: Vec<(String, String, f64)> =
            statement.query_map([limit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
        let mut log = SelectionLog::default();
        for (query, command, time) in rows.into_iter().rev() {
            log.add(&query, &command, time);
        }
        Ok(log)
    }

    /// The last command `session` ran (not counting smcli's own), for transitions.
    pub fn previous_command(&self, session: &str, user: &str, db: &Connection) -> Result<Option<String>> {
        let mut statement = db.prepare(
            "SELECT c.command FROM commands c JOIN sessions s ON s.id = c.session
             WHERE c.session = ? AND s.user = ? AND c.deleted_at IS NULL AND NOT smcli_internal(c.command)
             ORDER BY c.started_at DESC, c.id DESC LIMIT 1",
        )?;
        let mut rows = statement.query_map(params![session, user], |r| r.get(0))?;
        Ok(rows.next().transpose()?)
    }

    /// How often each command directly followed `previous` within one session.
    pub fn followers(&self, previous: &str, user: &str, db: &Connection) -> Result<HashMap<String, i64>> {
        let mut statement = db.prepare(
            "SELECT command, count(*) FROM (
                SELECT c.command AS command,
                       LAG(c.command) OVER (PARTITION BY c.session ORDER BY c.started_at, c.id) AS prev
                FROM commands c JOIN sessions s ON s.id = c.session
                WHERE s.user = ? AND c.deleted_at IS NULL AND NOT smcli_internal(c.command)
            ) WHERE prev = ? GROUP BY command",
        )?;
        let rows = statement.query_map(params![user, previous], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// `user`'s live runs in the order `suggestions` reads them.
    pub fn runs(&self, user: &str, db: &Connection) -> Result<Vec<Run>> {
        let mut statement = db.prepare(
            "SELECT c.command, c.directory, c.started_at, c.exit_code, c.session
             FROM commands c JOIN sessions s ON s.id = c.session
             WHERE s.user = ? AND c.deleted_at IS NULL
             ORDER BY c.started_at, c.id",
        )?;
        let rows = statement.query_map([user], |r| {
            Ok(Run { command: r.get(0)?, directory: r.get(1)?, started_at: r.get(2)?, exit_code: r.get(3)?, session: r.get(4)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// One row per distinct command: run count, its flags, and the most recent run's time, directory, exit code,
    /// author, and intent, plus usage aggregates from the same grouped query; `directory` scopes `runs_here`.
    pub fn command_stats(&self, directory: Option<&str>, project: Option<&str>, now: f64) -> Result<HashMap<String, CommandStats>> {
        if !self.exists() {
            return Ok(HashMap::new());
        }
        let db = self.connect(false)?;
        let here = directory.map(trim_slash).unwrap_or("");
        let root = project.unwrap_or("");
        let interrupted = INTERRUPTED_EXIT_CODES.map(|c| c.to_string()).join(", ");
        // SQLite returns the bare columns from the row holding MAX(started_at).
        let sql = format!(
            "SELECT c.command, COUNT(*), MAX(c.started_at), c.directory, c.exit_code, c.author, c.intent, c.flags,
                    SUM(smcli_decay(? - c.started_at)), SUM(c.exit_code IN (0, {interrupted})),
                    SUM(c.exit_code = 1),
                    SUM(? <> '' AND (c.directory = ? OR (? <> '/' AND substr(c.directory, 1, length(?) + 1) = ? || '/'))),
                    SUM(? <> '' AND (c.directory = ? OR (? <> '/' AND substr(c.directory, 1, length(?) + 1) = ? || '/')))
             FROM commands c
             WHERE c.deleted_at IS NULL GROUP BY c.command"
        );
        let mut values = vec![Value::Real(now)];
        values.extend([here; 5].iter().chain([root; 5].iter()).map(|s| Value::Text(s.to_string())));
        let mut statement = db.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(values))?;
        let mut stats = HashMap::new();
        while let Some(row) = rows.next()? {
            let command: String = row.get(0)?;
            let ones: i64 = row.get(10)?;
            let answers = ones > 0 && answers_with_exit_code(&command);
            let flags: Option<String> = row.get(7)?;
            stats.insert(command, CommandStats {
                runs: row.get(1)?,
                last_run: row.get(2)?,
                last_directory: row.get(3)?,
                last_exit_code: row.get(4)?,
                author: row.get(5)?,
                intent: row.get(6)?,
                flags: flags.unwrap_or_default().split(' ').filter(|f| !f.is_empty()).map(String::from).collect(),
                frecency: row.get(8)?,
                successes: row.get::<_, i64>(9)? + if answers { ones } else { 0 },
                runs_here: row.get(11)?,
                runs_project: row.get(12)?,
            });
        }
        Ok(stats)
    }

    /// Ranks `user`'s distinct commands for `directory`: each run counts more when it is recent, ran in this
    /// directory (or in the same project, or above/below it), and succeeded. Only commands starting with `prefix`
    /// are considered. `context` adds the signals of the moment (see `apply_ranking_signals`); picks apply always.
    pub fn suggestions(&self, user: &str, directory: &str, prefix: &str, limit: usize, now: f64,
                       context: Option<&RankingContext>) -> Result<Vec<Suggestion>> {
        if !self.exists() {
            return Ok(vec![]);
        }
        let db = self.connect(false)?;
        let here = trim_slash(directory);
        #[derive(Default)]
        struct Tally { score: f64, runs: i64, runs_here: i64, project_runs: i64, last_run: f64, last_exit_code: i64 }
        let mut tallies: HashMap<String, Tally> = HashMap::new();
        let mut answering: HashMap<String, bool> = HashMap::new();
        {
            let mut statement = db.prepare(
                "SELECT c.command, c.directory, c.started_at, c.exit_code
                 FROM commands c JOIN sessions s ON s.id = c.session
                 WHERE s.user = ? AND c.deleted_at IS NULL AND substr(c.command, 1, length(?)) = ?
                 ORDER BY c.started_at, c.id",
            )?;
            let mut rows = statement.query(params![user, prefix, prefix])?;
            while let Some(row) = rows.next()? {
                let command: String = row.get(0)?;
                if is_smcli_command(&command) {
                    continue;
                }
                let run_directory: String = row.get(1)?;
                let started: f64 = row.get(2)?;
                let exit_code: i64 = row.get(3)?;
                let in_project = context.is_some_and(|c| c.in_project(&run_directory));
                // Halves every two weeks, so habits outweigh one-offs without old habits winning forever.
                let mut weight = 0.5f64.powf((now - started).max(0.0) / USAGE_HALF_LIFE);
                if run_directory == here {
                    weight *= 4.0;
                } else if in_project {
                    weight *= 3.0;
                } else if here != "/" && run_directory != "/"
                    && (run_directory.starts_with(&format!("{here}/")) || here.starts_with(&format!("{run_directory}/")))
                {
                    weight *= 2.0;
                }
                if exit_code != 0 && !INTERRUPTED_EXIT_CODES.contains(&exit_code) {
                    let answers = *answering.entry(command.clone()).or_insert_with(|| answers_with_exit_code(&command));
                    if !(exit_code == 1 && answers) {
                        weight *= 0.25;
                    }
                }
                let tally = tallies.entry(command).or_default();
                tally.score += weight;
                tally.runs += 1;
                if run_directory == here { tally.runs_here += 1; }
                if in_project { tally.project_runs += 1; }
                tally.last_run = started;
                tally.last_exit_code = exit_code;
            }
        }
        let mut scores: HashMap<String, f64> = tallies.iter().map(|(k, t)| (k.clone(), t.score)).collect();
        let previous = match context.and_then(|c| c.session.as_deref()) {
            Some(session) if uses(context, Signal::Transition) => self.previous_command(session, user, &db)?,
            _ => None,
        };
        let followers = match &previous {
            Some(previous) => self.followers(previous, user, &db)?,
            None => HashMap::new(),
        };
        let picks = self.picks(5_000, &db)?.weights(prefix, now);
        let why = apply_ranking_signals(&mut scores, prefix, context, previous.as_deref(), &followers, &picks);
        let mut ranked: Vec<(String, f64)> = scores.into_iter().collect();
        let last = |command: &str| tallies.get(command).map_or(f64::NEG_INFINITY, |t| t.last_run);
        ranked.sort_by(|a, b| {
            b.1.total_cmp(&a.1).then_with(|| last(&b.0).total_cmp(&last(&a.0))).then_with(|| a.0.cmp(&b.0))
        });
        Ok(ranked
            .into_iter()
            .take(limit)
            .map(|(command, score)| match tallies.get(&command) {
                None => Suggestion {
                    why: suggestion_why(0, 0, why.get(&command)), command, score, runs: 0, runs_here: 0,
                    last_run: String::new(), last_exit_code: 0,
                },
                Some(t) => Suggestion {
                    why: suggestion_why(t.runs_here, t.project_runs, why.get(&command)), command, score, runs: t.runs,
                    runs_here: t.runs_here, last_run: format_iso8601(t.last_run, false), last_exit_code: t.last_exit_code,
                },
            })
            .collect())
    }

    // shortcut: `importAtuin`, `importLegacy`, `prune` and `records` are not ported yet; they come with their CLI commands in Phase 4.

    fn connect(&self, write: bool) -> Result<Connection> {
        let path = &self.path;
        if write {
            if let Some(parent) = path.parent() {
                std::fs::DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
            }
            // Create the file ourselves so it is private from the start; SQLite gives its WAL files the same mode.
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(path)
            {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
        let info = std::fs::symlink_metadata(path).with_context(|| format!("Cannot open history database {}", path.display()))?;
        // SAFETY: getuid has no preconditions.
        if !info.file_type().is_file() || info.uid() != unsafe { libc::getuid() } || info.mode() & 0o077 != 0 {
            bail!("History database must be a regular file owned by you with private permissions (chmod 600): {}", path.display());
        }
        // SQLite's NOFOLLOW refuses symlinks anywhere in the path (e.g. macOS /var), so resolve parent directories;
        // the file itself must still not be a symlink.
        let parent = std::fs::canonicalize(path.parent().unwrap_or(Path::new(".")))?;
        let resolved = parent.join(path.file_name().ok_or_else(|| anyhow!("Invalid history path"))?);
        let flags = if write { OpenFlags::SQLITE_OPEN_READ_WRITE } else { OpenFlags::SQLITE_OPEN_READ_ONLY }
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let db = Connection::open_with_flags(&resolved, flags)
            .map_err(|e| anyhow!("Cannot open history database {}: {e}", path.display()))?;
        // Concurrent terminals wait briefly for the writer instead of failing.
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let deterministic = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
        // SQLite has no portable pow(), so usage decay is a function of ours: smcli_decay(age in seconds).
        db.create_scalar_function("smcli_decay", 1, deterministic, |c| {
            Ok(0.5f64.powf(c.get::<f64>(0)?.max(0.0) / USAGE_HALF_LIFE))
        })?;
        // Lets a query skip smcli's own commands the way `is_smcli_command` does everywhere else.
        db.create_scalar_function("smcli_internal", 1, deterministic, |c| {
            Ok(is_smcli_command(&c.get::<Option<String>>(0)?.unwrap_or_default()))
        })?;
        let mut version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > VERSION && version != DISCARDED_USAGE_VERSION {
            bail!("History database was written by a newer smcli: {}", path.display());
        }
        if !write && version != 0 && version != VERSION {
            // Readers need the current columns; upgrade once through a writable connection.
            return self.connect(true);
        }
        if write {
            if version != VERSION {
                db.execute_batch("BEGIN IMMEDIATE")?;
                let migrated = (|| -> Result<()> {
                    // Another terminal may have created or upgraded the schema while this one waited for the lock.
                    let tables: i64 = db.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
                    version = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
                    if tables == 0 {
                        db.execute_batch(&[SCHEMA_V1, MIGRATION_TO_V2, MIGRATION_TO_V3, MIGRATION_TO_V5, MIGRATION_TO_V6, MIGRATION_TO_V7].concat())?;
                    } else {
                        if version == 1 { db.execute_batch(MIGRATION_TO_V2)?; }
                        if version <= 2 {
                            db.execute_batch(MIGRATION_TO_V3)?;
                            backfill_flags(&db)?;
                        }
                        if version == DISCARDED_USAGE_VERSION {
                            db.execute_batch("DROP TABLE IF EXISTS usage; PRAGMA user_version = 3;")?;
                        }
                        if version < 5 { db.execute_batch(MIGRATION_TO_V5)?; }
                        if version < 6 { db.execute_batch(MIGRATION_TO_V6)?; }
                        if version < 7 { db.execute_batch(MIGRATION_TO_V7)?; }
                    }
                    Ok(())
                })();
                if let Err(error) = migrated {
                    let _ = db.execute_batch("ROLLBACK");
                    return Err(error);
                }
                db.execute_batch("COMMIT")?;
                db.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
            }
            // WAL with NORMAL sync avoids an fsync per command; a power loss can drop only the last few records.
            db.execute_batch("PRAGMA synchronous = NORMAL")?;
        } else if version == 0 {
            bail!("Not an smcli history database: {}", path.display());
        }
        Ok(db)
    }
}

struct Insert<'a> {
    session: &'a str,
    user: &'a str,
    host: &'a str,
    shell: &'a str,
    started_at: f64,
    duration_ms: Option<i64>,
    directory: &'a str,
    command: &'a str,
    exit_code: i64,
    uuid: Option<&'a str>,
    pipestatus: Option<&'a [i64]>,
}

fn insert(db: &Connection, row: &Insert, transaction: bool) -> Result<bool> {
    // One transaction means one WAL commit per command.
    if transaction {
        db.execute_batch("BEGIN IMMEDIATE")?;
    }
    let result = (|| -> Result<bool> {
        db.execute("INSERT OR IGNORE INTO sessions (id, user, host, shell) VALUES (?, ?, ?, ?)",
                   params![row.session, row.user, row.host, row.shell])?;
        // OR IGNORE skips a row whose uuid is already stored.
        let inserted = db.execute(
            "INSERT OR IGNORE INTO commands
                (session, started_at, duration_ms, directory, command, exit_code, uuid, deleted_at, author, intent, flags, pipestatus)
             VALUES (?, ?, ?, ?, ?, ?, ?, NULL, NULL, NULL, ?, ?)",
            params![row.session, row.started_at, row.duration_ms, row.directory, row.command, row.exit_code, row.uuid,
                    command_flags(row.command).join(" "),
                    row.pipestatus.map(|p| p.iter().map(i64::to_string).collect::<Vec<_>>().join(" "))],
        )?;
        Ok(inserted > 0)
    })();
    if transaction {
        match &result {
            Ok(_) => db.execute_batch("COMMIT")?,
            Err(_) => { let _ = db.execute_batch("ROLLBACK"); }
        }
    }
    result
}

/// Fills the flags column for rows written before it existed. Runs inside the migration's transaction.
fn backfill_flags(db: &Connection) -> Result<()> {
    let rows: Vec<(i64, String)> = db
        .prepare("SELECT id, command FROM commands WHERE flags IS NULL")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (id, command) in rows {
        db.execute("UPDATE commands SET flags = ? WHERE id = ?", params![command_flags(&command).join(" "), id])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (tempfile::TempDir, CapturedHistory) {
        let dir = tempfile::tempdir().unwrap();
        let history = CapturedHistory::new(dir.path().join("sub/history.db"));
        (dir, history)
    }

    #[test]
    fn append_and_rank_suggestions() {
        let (_dir, log) = temp_db();
        let db = log.open_connection().unwrap();
        let run = |command, directory, at: f64, exit| {
            let mut new = NewRun::new(command, "zsh", directory, exit);
            new.started_at = Some(at);
            new.session = "s1";
            log.append(&new, "", Some(&db)).unwrap()
        };
        let now = 1_700_000_000.0;
        assert!(run("git status", "/repo", now - 100.0, 0));
        run("git status", "/repo", now - 50.0, 0);
        run("git stash", "/other", now - 10.0, 1);
        run("grep x y", "/repo", now - 5.0, 1);
        run("smcli search x", "/repo", now - 1.0, 0);
        let user = user_name();
        let suggestions = log.suggestions(&user, "/repo/", "git", 10, now, None).unwrap();
        assert_eq!(suggestions.iter().map(|s| s.command.as_str()).collect::<Vec<_>>(), ["git status", "git stash"]);
        assert_eq!(suggestions[0].runs_here, 2);
        assert_eq!(suggestions[0].why.as_deref(), Some(&["used here".to_string()][..]));
        let stats = log.command_stats(Some("/repo"), None, now).unwrap();
        assert_eq!(stats["git status"].runs, 2);
        assert_eq!(stats["grep x y"].successes, 1);
        assert_eq!(stats["git stash"].successes, 0);
        assert_eq!(log.previous_command("s1", &user, &db).unwrap().as_deref(), Some("grep x y"));
        assert_eq!(log.followers("git status", &user, &db).unwrap(), HashMap::from([("git status".into(), 1), ("git stash".into(), 1)]));
        // Retrying an event ID never adds a second row.
        let mut again = NewRun::new("ls", "zsh", "/", 0);
        again.event = Some("00000000-0000-4000-8000-000000000000".into());
        assert!(log.append(&again, "", Some(&db)).unwrap());
        assert!(!log.append(&again, "", Some(&db)).unwrap());
        assert!(log.append(&NewRun::new("ls", "tcsh", "/", 0), "", Some(&db)).is_err());
        assert!(!log.record_pick("x", "AKIAABCDEFGHIJKLMNOP", "suggest", "/", &CaptureFilter::standard(), now, &db).unwrap());
    }

    #[test]
    fn refuse_shared_or_public_files() {
        let (dir, log) = temp_db();
        log.open_connection().unwrap();
        assert!(log.check_separate(log.path.to_str().unwrap()).is_err());
        let public = dir.path().join("public.db");
        std::fs::write(&public, "").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&public, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(CapturedHistory::new(&public).open_connection().is_err());
        assert!(parse_pipestatus("0 1 x").is_err());
        assert_eq!(parse_pipestatus(" ").unwrap(), None);
    }

    #[test]
    fn upgrade_version_1_databases() {
        let (_dir, log) = temp_db();
        std::fs::create_dir_all(log.path.parent().unwrap()).unwrap();
        let old = Connection::open(&log.path).unwrap();
        old.execute_batch(SCHEMA_V1).unwrap();
        old.execute_batch("PRAGMA user_version = 1; INSERT INTO sessions VALUES ('s', 'u', 'h', 'zsh');
                           INSERT INTO commands (session, started_at, directory, command, exit_code) VALUES ('s', 1, '/', 'git commit -m x --amend', 0);").unwrap();
        drop(old);
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&log.path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let stats = log.command_stats(None, None, 10.0).unwrap();
        assert_eq!(stats["git commit -m x --amend"].flags, ["-m", "--amend"]);
    }
}
