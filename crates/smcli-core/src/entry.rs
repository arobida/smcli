use crate::parse::{CommandBreakdown, program_of};
use crate::time::parse_iso8601;

/// Wire types from the service protocol; search and filtering behavior lives here.
pub use crate::service_protocol::{SearchEntry, SearchFilter};

/// Ordinary commands keep their historic key; NUL-prefixed JSON makes variant keys unambiguous.
/// Escaping the prefix also keeps arbitrary native-file input collision-free.
pub fn command_key(command: &str, expanded: Option<&str>) -> String {
    if expanded.is_none() && !command.starts_with('\0') { return command.into(); }
    format!("\0{}", serde_json::to_string(&(command, expanded)).unwrap())
}

pub fn command_parts(key: &str) -> (String, Option<String>) {
    key.strip_prefix('\0').and_then(|s| serde_json::from_str(s).ok()).unwrap_or_else(|| (key.into(), None))
}

impl SearchEntry {
    pub fn new(command: &str, runs: i64, source: &str) -> Self {
        Self { command: command.into(), runs, source: source.into(), ..Default::default() }
    }

    pub fn from_key(key: &str) -> Self {
        let (command, expanded_command) = command_parts(key);
        Self { command, expanded_command, source: "captured".into(), ..Default::default() }
    }

    pub fn key(&self) -> String { command_key(&self.command, self.expanded_command.as_deref()) }

    pub fn primary(&self) -> &str {
        if self.alias_off { &self.command } else { self.expanded_command.as_deref().unwrap_or(&self.command) }
    }

    pub fn secondary(&self) -> Option<&str> {
        (!self.alias_off && self.expanded_command.is_some()).then_some(self.command.as_str())
    }

    pub fn program(&self) -> Option<String> {
        program_of(self.primary(), CommandBreakdown::parse(self.primary()).as_ref())
    }

    pub fn subcommand(&self) -> Option<String> {
        CommandBreakdown::parse(self.primary())?.subcommand
    }

    pub fn last_run_epoch(&self) -> Option<f64> {
        self.last_run.as_deref().and_then(parse_iso8601)
    }
}

impl crate::service_protocol::Suggestion {
    pub fn key(&self) -> String { command_key(&self.command, self.expanded_command.as_deref()) }
}

/// `directory` without a trailing slash, keeping "/" itself.
pub fn trim_slash(directory: &str) -> &str {
    if directory.len() > 1 { directory.strip_suffix('/').unwrap_or(directory) } else { directory }
}

// shortcut: no `time()` parser for `30m`/`YYYY-MM-DD` yet; port it with the CLI flags in Phase 4.
impl SearchFilter {
    /// Fills unset fields from `parsed`; values already present in `self` win.
    pub fn merge_parsed(&mut self, parsed: &SearchFilter) {
        if self.program.is_none() { self.program = parsed.program.clone(); }
        if self.subcommand.is_none() { self.subcommand = parsed.subcommand.clone(); }
        if self.author.is_none() { self.author = parsed.author.clone(); }
        if self.flag.is_none() { self.flag = parsed.flag.clone(); }
        if self.directory.is_none() { self.directory = parsed.directory.clone(); }
        if self.min_runs.is_none() { self.min_runs = parsed.min_runs; }
        if self.since.is_none() { self.since = parsed.since; }
        if self.before.is_none() { self.before = parsed.before; }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn allows(&self, entry: &SearchEntry) -> bool {
        if self.program.is_some() && entry.program() != self.program {
            return false;
        }
        if self.subcommand.is_some() && entry.subcommand() != self.subcommand {
            return false;
        }
        if self.min_runs.is_some_and(|min| entry.runs < min) {
            return false;
        }
        if self.author.is_some() && entry.author != self.author {
            return false;
        }
        if self.flag.as_ref().is_some_and(|flag| !entry.flags.contains(flag)) {
            return false;
        }
        if let Some(directory) = &self.directory {
            let base = trim_slash(directory);
            let inside = entry.last_directory.as_deref().is_some_and(|last| {
                last == base || last.starts_with(&if base == "/" { "/".to_string() } else { format!("{base}/") })
            });
            if !inside {
                return false;
            }
        }
        if self.since.is_some() || self.before.is_some() {
            let Some(last) = entry.last_run_epoch() else { return false };
            if self.since.is_some_and(|since| last < since) || self.before.is_some_and(|before| last >= before) {
                return false;
            }
        }
        true
    }
}
