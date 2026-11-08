//! Typed requests over the unchanged wire bag. Validation messages stay the handler strings.
use std::path::PathBuf;

use crate::parse::{is_smcli_command, safe_expansion};
use crate::service_protocol::{FailureCode, Operation, SearchFilter, ServiceRequest};

#[derive(Debug)]
pub enum Request {
    Ping,
    Shutdown,
    Record(Record),
    Suggest(Ranked),
    Pick(Ranked),
    Search(Search),
    Context(Context),
    Files(FileQuery),
    Grep(FileQuery),
    Complete(Complete),
    Select(Select),
    Embed(Embed),
}

#[derive(Debug)]
pub struct Record {
    pub event: String,
    pub command: String,
    pub shell: String,
    pub session: String,
    pub directory: String,
    pub exit_code: i64,
    pub expanded_command: Option<String>,
    pub started_at: Option<f64>,
    pub ended_at: Option<f64>,
    pub pipestatus: Option<Vec<i64>>,
    pub native_file: Option<String>,
    pub config: PathBuf,
    pub shell_path: Option<String>,
}

#[derive(Debug)]
pub struct Ranked {
    pub directory: String,
    pub limit: i64,
    pub prefix: String,
    pub session: Option<String>,
    pub ranking_off: Option<String>,
}

#[derive(Debug)]
pub struct Search {
    pub query: String,
    pub directory: String,
    pub limit: i64,
    pub filter: SearchFilter,
    pub home: Option<String>,
    pub config: PathBuf,
    pub session: Option<String>,
    pub histfile: Option<String>,
    pub shell: Option<String>,
    pub path: Option<String>,
    pub ranking_off: Option<String>,
}

#[derive(Debug)]
pub struct Context {
    pub directory: String,
}

#[derive(Debug)]
pub struct FileQuery {
    pub directory: String,
    pub query: String,
    pub limit: i64,
    pub scope: bool,
    pub cwd: String,
    pub path: Option<String>,
    pub home: Option<PathBuf>,
    pub config: Option<PathBuf>,
}

#[derive(Debug)]
pub struct Complete {
    pub directory: String,
    pub query: String,
    pub limit: i64,
    pub dirs_only: bool,
    pub home: Option<PathBuf>,
    pub config: Option<PathBuf>,
}

#[derive(Debug)]
pub enum Select {
    Command {
        command: String,
        expanded_command: Option<String>,
        directory: String,
        mode: String,
        query: String,
        config: PathBuf,
    },
    File {
        cwd: Option<String>,
        directory: String,
        path: String,
        mode: String,
        query: String,
        scope: bool,
        config: PathBuf,
    },
}

#[derive(Debug)]
pub struct Embed {
    pub query: String,
    pub commands: bool,
    pub command: Option<String>,
    pub shell_path: Option<String>,
    pub config: PathBuf,
}

fn reject(message: impl Into<String>) -> (FailureCode, String) {
    (FailureCode::BadRequest, message.into())
}

fn config_of(request: &ServiceRequest) -> Result<PathBuf, (FailureCode, String)> {
    request
        .config
        .as_deref()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| reject("config is required"))
}

fn absolute<'a>(value: Option<&'a str>) -> Option<&'a str> {
    value.filter(|d| d.starts_with('/'))
}

impl TryFrom<&ServiceRequest> for Request {
    type Error = (FailureCode, String);

    fn try_from(request: &ServiceRequest) -> Result<Self, Self::Error> {
        if matches!(
            request.op,
            Operation::Record
                | Operation::Select
                | Operation::Search
                | Operation::Suggest
                | Operation::Pick
                | Operation::Embed
        ) {
            let _ = config_of(request)?;
        }
        Ok(match request.op {
            Operation::Ping => Self::Ping,
            Operation::Shutdown => Self::Shutdown,
            Operation::Record => Self::Record(record(request)?),
            Operation::Suggest => Self::Suggest(ranked(request, false)?),
            Operation::Pick => Self::Pick(ranked(request, true)?),
            Operation::Search => Self::Search(search(request)?),
            Operation::Context => Self::Context(Context {
                directory: absolute(request.directory.as_deref())
                    .ok_or_else(|| reject("context needs an absolute directory."))?
                    .to_owned(),
            }),
            Operation::Files => Self::Files(file_query(request, false)?),
            Operation::Grep => Self::Grep(file_query(request, true)?),
            Operation::Complete => Self::Complete(complete(request)?),
            Operation::Select => Self::Select(select(request)?),
            Operation::Embed => Self::Embed(embed(request)?),
        })
    }
}

fn record(request: &ServiceRequest) -> Result<Record, (FailureCode, String)> {
    let (Some(event), Some(command), Some(shell), Some(session), Some(directory), Some(exit_code)) = (
        request.event.clone(),
        request.command.clone(),
        request.shell.clone(),
        request.session.clone(),
        request.directory.clone(),
        request.exit_code,
    ) else {
        return Err(reject(
            "record needs event, command, shell, session, directory and exitCode.",
        ));
    };
    Ok(Record {
        event,
        command,
        shell,
        session,
        directory,
        exit_code,
        expanded_command: request.expanded_command.clone(),
        started_at: request.started_at,
        ended_at: request.ended_at,
        pipestatus: request.pipestatus.clone(),
        native_file: request.native_file.clone(),
        config: config_of(request)?,
        shell_path: request.shell_path.clone(),
    })
}

fn ranked(request: &ServiceRequest, pick: bool) -> Result<Ranked, (FailureCode, String)> {
    let name = if pick { "pick" } else { "suggest" };
    let Some(directory) = absolute(request.directory.as_deref()) else {
        return Err(reject(format!("{name} needs an absolute directory.")));
    };
    let limit = request.limit.unwrap_or(if pick { 50 } else { 10 });
    if !(1..=1000).contains(&limit) {
        return Err(reject("limit must be 1 to 1000."));
    }
    Ok(Ranked {
        directory: directory.to_owned(),
        limit,
        prefix: if pick {
            String::new()
        } else {
            request.prefix.clone().unwrap_or_default()
        },
        session: request.session.clone(),
        ranking_off: request.ranking_off.clone(),
    })
}

fn search(request: &ServiceRequest) -> Result<Search, (FailureCode, String)> {
    let (Some(query), Some(directory)) = (
        request.query.clone(),
        absolute(request.directory.as_deref()).map(str::to_owned),
    ) else {
        return Err(reject("search needs a query and an absolute directory."));
    };
    let limit = request.limit.unwrap_or(10);
    if !(1..=1000).contains(&limit) {
        return Err(reject("limit must be 1 to 1000."));
    }
    Ok(Search {
        query,
        directory,
        limit,
        filter: request.filter.clone().unwrap_or_default(),
        home: request.home.clone(),
        config: config_of(request)?,
        session: request.session.clone(),
        histfile: request.histfile.clone(),
        shell: request.login_shell.clone(),
        path: request.shell_path.clone(),
        ranking_off: request.ranking_off.clone(),
    })
}

fn file_query(request: &ServiceRequest, grep: bool) -> Result<FileQuery, (FailureCode, String)> {
    let name = if grep { "grep" } else { "files" };
    let (Some(directory), Some(query)) = (
        absolute(request.directory.as_deref()),
        request.query.as_deref(),
    ) else {
        return Err(reject(format!(
            "{name} needs a query and an absolute directory."
        )));
    };
    let limit = request.limit.unwrap_or(50);
    if !(1..=500).contains(&limit) {
        return Err(reject("limit must be 1 to 500."));
    }
    if grep && query.is_empty() {
        return Err(reject("grep needs a query."));
    }
    let cwd = request.cwd.clone().unwrap_or_else(|| directory.to_owned());
    Ok(FileQuery {
        directory: directory.to_owned(),
        query: query.to_owned(),
        limit,
        scope: request.scope == Some(true),
        cwd,
        path: request.path.clone(),
        home: request
            .home
            .clone()
            .filter(|p| !p.is_empty())
            .map(PathBuf::from),
        config: request
            .config
            .clone()
            .filter(|p| !p.is_empty())
            .map(PathBuf::from),
    })
}

fn complete(request: &ServiceRequest) -> Result<Complete, (FailureCode, String)> {
    let (Some(directory), Some(query)) = (
        absolute(request.directory.as_deref()),
        request.query.as_deref(),
    ) else {
        return Err(reject("complete needs a query and an absolute directory."));
    };
    let limit = request.limit.unwrap_or(50);
    if !(1..=500).contains(&limit) {
        return Err(reject("limit must be 1 to 500."));
    }
    Ok(Complete {
        directory: directory.to_owned(),
        query: query.to_owned(),
        limit,
        dirs_only: request.dirs_only.unwrap_or(false),
        home: request
            .home
            .clone()
            .filter(|p| !p.is_empty())
            .map(PathBuf::from),
        config: request
            .config
            .clone()
            .filter(|p| !p.is_empty())
            .map(PathBuf::from),
    })
}

fn select(request: &ServiceRequest) -> Result<Select, (FailureCode, String)> {
    let config = config_of(request)?;
    if matches!(request.mode.as_deref(), Some("suggest" | "search")) {
        let (Some(command), Some(directory), Some(mode)) = (
            request.command.as_deref(),
            request.directory.as_deref(),
            request.mode.as_deref(),
        ) else {
            return Err(reject(
                "select needs a command, a directory and a mode of suggest or search.",
            ));
        };
        if !directory.starts_with('/')
            || command.is_empty()
            || command.len() > 1 << 16
            || is_smcli_command(command)
        {
            return Err(reject(
                "select needs a command, a directory and a mode of suggest or search.",
            ));
        }
        if request
            .expanded_command
            .as_deref()
            .is_some_and(|e| !safe_expansion(e) || is_smcli_command(e))
        {
            return Err(reject("Invalid expanded command."));
        }
        return Ok(Select::Command {
            command: command.to_owned(),
            expanded_command: request.expanded_command.clone(),
            directory: directory.to_owned(),
            mode: mode.to_owned(),
            query: request
                .query
                .as_deref()
                .unwrap_or("")
                .chars()
                .take(1000)
                .collect(),
            config,
        });
    }
    let (Some(directory), Some(path), Some(mode)) = (
        absolute(request.directory.as_deref()),
        request.path.as_deref(),
        request
            .mode
            .as_deref()
            .filter(|m| matches!(*m, "files" | "grep")),
    ) else {
        return Err(reject(
            "select needs a directory, a path and a mode of files or grep.",
        ));
    };
    let path = path.trim_end_matches('/');
    if path.is_empty()
        || path.starts_with('/')
        || path.len() > 4096
        || path.contains('\0')
        || path.split('/').any(|p| p == ".." || p.is_empty())
    {
        return Err(reject("select needs a path relative to the searched root."));
    }
    Ok(Select::File {
        cwd: request.cwd.clone(),
        directory: directory.to_owned(),
        path: path.to_owned(),
        mode: mode.to_owned(),
        query: request
            .query
            .as_deref()
            .unwrap_or("")
            .chars()
            .take(1000)
            .collect(),
        scope: request.scope == Some(true),
        config,
    })
}

fn embed(request: &ServiceRequest) -> Result<Embed, (FailureCode, String)> {
    let Some(query) = request.query.clone() else {
        return Err(reject("embed needs a query."));
    };
    Ok(Embed {
        query,
        commands: request.mode.as_deref() == Some("commands"),
        command: request.command.clone(),
        shell_path: request.shell_path.clone(),
        config: config_of(request)?,
    })
}
