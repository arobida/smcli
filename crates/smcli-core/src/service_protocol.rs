//! Swift ServiceProtocol wire DTOs; no socket, dispatch, or binary identity logic.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const VERSION: i64 = 3;
pub const MAX_SHELL_PATH_BYTES: usize = 8_192;

/// Oversized snapshots are unknown, never truncated into a different PATH.
pub fn shell_path_snapshot(path: Option<&str>) -> Option<String> {
    path.filter(|p| p.len() <= MAX_SHELL_PATH_BYTES && !p.contains('\0')).map(String::from)
}
pub const MAX_MESSAGE_BYTES: usize = 1 << 20;
pub const MAX_RESPONSE_BYTES: usize = 64 << 20;
pub const IO_TIMEOUT: f64 = 2.0;
pub const MAX_CONNECTIONS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    Ping,
    Record,
    Suggest,
    Pick,
    Search,
    Context,
    Files,
    Grep,
    Complete,
    Select,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceRequest {
    #[serde(deserialize_with = "numbers::integer")]
    pub v: i64,
    pub op: Operation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "numbers::optional_integer"
    )]
    pub exit_code: Option<i64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "numbers::optional_finite"
    )]
    pub started_at: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "numbers::optional_finite"
    )]
    pub ended_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_file: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "numbers::optional_integers"
    )]
    pub pipestatus: Option<Vec<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "numbers::optional_integer"
    )]
    pub limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<SearchFilter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub histfile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_shell: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranking_off: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirs_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

impl ServiceRequest {
    pub fn new(op: Operation) -> Self {
        Self {
            v: VERSION,
            op,
            event: None,
            command: None,
            expanded_command: None,
            shell: None,
            session: None,
            exit_code: None,
            started_at: None,
            ended_at: None,
            native_file: None,
            pipestatus: None,
            directory: None,
            prefix: None,
            limit: None,
            query: None,
            filter: None,
            histfile: None,
            login_shell: None,
            shell_path: None,
            ranking_off: None,
            path: None,
            mode: None,
            dirs_only: None,
            config: None,
            scope: None,
            cwd: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceResponse {
    #[serde(deserialize_with = "numbers::integer")]
    pub v: i64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Failure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestions: Option<Vec<Suggestion>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_entries: Option<Vec<SearchEntry>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hits: Option<Vec<SearchEntry>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ProjectContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<FileHit>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "numbers::optional_integer"
    )]
    pub instance: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "numbers::optional_pid"
    )]
    pub pid: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexes: Option<Vec<FileIndexInfo>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
}

impl ServiceResponse {
    pub fn new(ok: bool) -> Self {
        Self {
            v: VERSION,
            ok,
            error: None,
            event: None,
            duplicate: None,
            suggestions: None,
            recent: None,
            recent_entries: None,
            hits: None,
            context: None,
            files: None,
            root: None,
            instance: None,
            partial: None,
            pid: None,
            indexes: None,
            build: None,
        }
    }

    pub fn failure(code: impl Into<String>, message: impl Into<String>) -> Self {
        let mut response = Self::new(false);
        response.error = Some(Failure {
            code: code.into(),
            message: message.into(),
        });
        response
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHit {
    pub path: String,
    pub insert: String,
    pub provenance: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "numbers::optional_integer"
    )]
    pub line: Option<i64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "numbers::optional_integer"
    )]
    pub column: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileIndexInfo {
    #[serde(deserialize_with = "numbers::integer")]
    pub id: i64,
    pub root: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subcommand: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "numbers::optional_finite"
    )]
    pub since: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "numbers::optional_finite"
    )]
    pub before: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "numbers::optional_integer"
    )]
    pub min_runs: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded_command: Option<String>,
    #[serde(serialize_with = "numbers::finite")]
    pub score: f64,
    #[serde(deserialize_with = "numbers::integer")]
    pub runs: i64,
    #[serde(deserialize_with = "numbers::integer")]
    pub runs_here: i64,
    pub last_run: String,
    #[serde(deserialize_with = "numbers::integer")]
    pub last_exit_code: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<Vec<String>>,
}

// Swift initializer defaults are not decoding defaults: all nonoptional keys remain required (`Default` is for Rust callers only).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchEntry {
    #[serde(skip)]
    pub alias_off: bool,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded_command: Option<String>,
    #[serde(deserialize_with = "numbers::integer")]
    pub runs: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_directory: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "numbers::optional_integer"
    )]
    pub last_exit_code: Option<i64>,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    pub flags: Vec<String>,
    #[serde(serialize_with = "numbers::finite")]
    pub frecency: f64,
    #[serde(deserialize_with = "numbers::integer")]
    pub successes: i64,
    #[serde(deserialize_with = "numbers::integer")]
    pub runs_here: i64,
    #[serde(deserialize_with = "numbers::integer")]
    pub runs_project: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContext {
    pub directory: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<Repository>,
    pub projects: Vec<Project>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_relative: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_relative: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<GitStatus>,
    pub status_pending: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub root: String,
    pub git_dir: String,
    pub common_dir: String,
    pub linked_worktree: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub root: String,
    pub kind: String,
    pub manifests: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_manager: Option<String>,
    pub scripts: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(deserialize_with = "numbers::integer")]
    pub ahead: i64,
    #[serde(deserialize_with = "numbers::integer")]
    pub behind: i64,
    #[serde(deserialize_with = "numbers::integer")]
    pub staged: i64,
    #[serde(deserialize_with = "numbers::integer")]
    pub unstaged: i64,
    #[serde(deserialize_with = "numbers::integer")]
    pub untracked: i64,
    #[serde(deserialize_with = "numbers::integer")]
    pub conflicted: i64,
    #[serde(serialize_with = "numbers::finite")]
    pub fetched_at: f64,
}

/// Encodes one compact JSON value followed by LF, including escaped embedded newlines.
/// The transport owns size limits, deadlines, and the one-request-per-connection rule.
pub fn encode_line<T: Serialize>(message: &T) -> serde_json::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(message)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Decodes the JSON payload returned by a line reader (a trailing LF is also accepted).
/// A different version is preserved for dispatch to return a `version` failure.
pub fn decode_line<T: serde::de::DeserializeOwned>(line: &[u8]) -> serde_json::Result<T> {
    serde_json::from_slice(line)
}

mod numbers {
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
    use std::fmt;

    struct Integer(i64);

    // Accept ordinary integral floats; Foundation's noncanonical decimal quirks are not emulated.
    impl<'de> Deserialize<'de> for Integer {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> de::Visitor<'de> for Visitor {
                type Value = Integer;
                fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                    f.write_str("a signed 64-bit integer")
                }
                fn visit_i64<E: de::Error>(self, value: i64) -> Result<Integer, E> {
                    Ok(Integer(value))
                }
                fn visit_u64<E: de::Error>(self, value: u64) -> Result<Integer, E> {
                    i64::try_from(value).map(Integer).map_err(E::custom)
                }
                fn visit_f64<E: de::Error>(self, value: f64) -> Result<Integer, E> {
                    if value.is_finite()
                        && value.fract() == 0.0
                        && value.abs() <= 9_007_199_254_740_992.0
                    {
                        Ok(Integer(value as i64))
                    } else {
                        Err(E::custom("integer out of range or fractional"))
                    }
                }
            }
            deserializer.deserialize_any(Visitor)
        }
    }

    pub fn integer<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
        Integer::deserialize(d).map(|value| value.0)
    }

    pub fn optional_integer<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
        Option::<Integer>::deserialize(d).map(|value| value.map(|value| value.0))
    }

    pub fn optional_integers<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Vec<i64>>, D::Error> {
        Option::<Vec<Integer>>::deserialize(d)
            .map(|values| values.map(|values| values.into_iter().map(|value| value.0).collect()))
    }

    pub fn optional_pid<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i32>, D::Error> {
        optional_integer(d)?
            .map(|value| i32::try_from(value).map_err(de::Error::custom))
            .transpose()
    }

    // serde_json otherwise turns nonfinite doubles into null; Swift JSONEncoder throws.
    pub fn finite<S: Serializer>(value: &f64, s: S) -> Result<S::Ok, S::Error> {
        if !value.is_finite() {
            return Err(serde::ser::Error::custom("nonfinite double"));
        }
        value.serialize(s)
    }

    pub fn optional_finite<S: Serializer>(value: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(value) => finite(value, s),
            None => s.serialize_none(),
        }
    }
}
