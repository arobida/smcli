//! What one client request may depend on. The service keeps only [`LaunchSettings`] from its start.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::{SmcliConfig, home_dir};
use crate::service_protocol::{self, Operation, ServiceRequest};

/// Fields of [`CallerContext`] that an operation carries on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerField {
    Home,
    Config,
    Cwd,
    Session,
    Histfile,
    Shell,
    Path,
    RankingOff,
}

impl Operation {
    /// Which caller fields `apply` writes. Handlers must not read anything else from the launch environment.
    pub fn caller_fields(self) -> &'static [CallerField] {
        use CallerField::*;
        use Operation::*;
        match self {
            Search => &[Home, Config, Session, Histfile, Shell, Path, RankingOff],
            Suggest | Pick => &[Home, Config, Session, RankingOff],
            Embed => &[Home, Config, Path],
            Files | Grep | Complete => &[Home, Config, Cwd],
            Record => &[Home, Config, Path],
            Select => &[Home, Config, Cwd],
            Context | Ping | Shutdown => &[Home, Config],
        }
    }
}

/// The caller's home, config, cwd, session, history file, shell, PATH and ranking switches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerContext {
    pub home: PathBuf,
    pub config: PathBuf,
    pub cwd: String,
    pub session: Option<String>,
    pub histfile: Option<String>,
    pub shell: Option<String>,
    pub path: Option<String>,
    pub ranking_off: Option<String>,
}

/// Parsed once from the service's start environment. Request handlers do not read that environment.
#[derive(Debug, Clone)]
pub struct LaunchSettings {
    pub home: PathBuf,
    /// Config the launch environment names (`SMCLI_CONFIG`, `XDG_CONFIG_HOME`); used when a request names neither home nor config.
    pub config: PathBuf,
    pub idle: Duration,
    pub fff_max: usize,
    pub fff_idle: Duration,
}

impl LaunchSettings {
    pub fn from_env(env: &HashMap<String, String>) -> Self {
        let idle = env
            .get("SMCLI_SERVICE_IDLE")
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(1800.0);
        let fff_idle = env
            .get("SMCLI_FFF_IDLE")
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(600.0);
        let home = PathBuf::from(env.get("HOME").cloned().unwrap_or_else(|| "/".into()));
        Self {
            config: SmcliConfig::location(&home, env),
            home,
            idle: Duration::from_secs_f64(idle),
            fff_max: env
                .get("SMCLI_FFF_MAX")
                .and_then(|v| v.parse().ok())
                .unwrap_or(4),
            fff_idle: Duration::from_secs_f64(fff_idle),
        }
    }
}

impl CallerContext {
    /// Same inputs for the request and the in-process fallback.
    pub fn from_env(env: &HashMap<String, String>) -> Self {
        let home = home_dir(env);
        let cwd = env
            .get("PWD")
            .filter(|p| p.starts_with('/'))
            .cloned()
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|d| d.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| "/".into())
            });
        Self {
            home: home.clone(),
            config: SmcliConfig::location(&home, env),
            cwd,
            session: env.get("SMCLI_SESSION").filter(|s| !s.is_empty()).cloned(),
            histfile: env.get("HISTFILE").filter(|s| !s.is_empty()).cloned(),
            shell: env.get("SHELL").filter(|s| !s.is_empty()).cloned(),
            path: service_protocol::shell_path_snapshot(env.get("PATH").map(String::as_str)),
            ranking_off: env
                .get("SMCLI_RANKING_OFF")
                .filter(|s| s.len() <= 256)
                .cloned(),
        }
    }

    /// Rebuilds the context a client applied. Missing home uses the service's launch home.
    pub fn from_request(request: &ServiceRequest, launch: &LaunchSettings) -> Self {
        let caller_home = request
            .home
            .as_deref()
            .filter(|p| !p.is_empty())
            .map(PathBuf::from);
        let config = request
            .config
            .as_deref()
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| match &caller_home {
                Some(home) => SmcliConfig::location(home, &HashMap::new()),
                None => launch.config.clone(),
            });
        let home = caller_home.unwrap_or_else(|| launch.home.clone());
        Self {
            home,
            config,
            cwd: request
                .cwd
                .clone()
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| "/".into()),
            session: request.session.clone().filter(|s| !s.is_empty()),
            histfile: request.histfile.clone().filter(|s| !s.is_empty()),
            shell: request.login_shell.clone().filter(|s| !s.is_empty()),
            path: service_protocol::shell_path_snapshot(request.shell_path.as_deref()),
            ranking_off: request
                .ranking_off
                .as_ref()
                .filter(|s| !s.is_empty() && s.len() <= 256)
                .cloned(),
        }
    }

    /// Writes exactly `op.caller_fields()`. Home and config fill only when the request left them empty,
    /// so an explicit path is not replaced; the other fields are this caller's values.
    pub fn apply(&self, request: &mut ServiceRequest) {
        for field in request.op.caller_fields() {
            match field {
                CallerField::Home if request.home.is_none() => {
                    request.home = Some(self.home.to_string_lossy().into_owned());
                }
                CallerField::Config if request.config.is_none() => {
                    request.config = Some(self.config.to_string_lossy().into_owned());
                }
                CallerField::Cwd => request.cwd = Some(self.cwd.clone()),
                CallerField::Session => request.session = self.session.clone(),
                CallerField::Histfile => request.histfile = self.histfile.clone(),
                CallerField::Shell => request.login_shell = self.shell.clone(),
                CallerField::Path => request.shell_path = self.path.clone(),
                CallerField::RankingOff => {
                    request.ranking_off = Some(self.ranking_off.clone().unwrap_or_default());
                }
                CallerField::Home | CallerField::Config => {}
            }
        }
    }

    /// HISTFILE, SHELL, PATH and PWD only, for native history and in-process search.
    pub fn native_env(&self) -> HashMap<String, String> {
        let mut env = HashMap::new();
        env.insert("PWD".into(), self.cwd.clone());
        if let Some(histfile) = &self.histfile {
            env.insert("HISTFILE".into(), histfile.clone());
        }
        if let Some(shell) = &self.shell {
            env.insert("SHELL".into(), shell.clone());
        }
        if let Some(path) = &self.path {
            env.insert("PATH".into(), path.clone());
        }
        env
    }

    pub fn home(&self) -> &Path {
        &self.home
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> HashMap<String, String> {
        HashMap::from([
            ("HOME".into(), "/caller-home".into()),
            ("SMCLI_CONFIG".into(), "/caller-home/config.json".into()),
            ("PWD".into(), "/caller-home/proj".into()),
            ("SMCLI_SESSION".into(), "sess".into()),
            ("HISTFILE".into(), "/caller-home/.zsh_history".into()),
            ("SHELL".into(), "/bin/zsh".into()),
            ("PATH".into(), "/usr/bin:/caller/bin".into()),
            ("SMCLI_RANKING_OFF".into(), "path,files".into()),
        ])
    }

    #[test]
    fn caller_fields_table_is_applied() {
        let caller = CallerContext::from_env(&env());
        for op in [
            Operation::Ping,
            Operation::Record,
            Operation::Suggest,
            Operation::Pick,
            Operation::Search,
            Operation::Context,
            Operation::Files,
            Operation::Grep,
            Operation::Complete,
            Operation::Select,
            Operation::Shutdown,
            Operation::Embed,
        ] {
            let mut request = ServiceRequest::new(op);
            request.home = Some("sentinel-home".into());
            request.config = Some("sentinel-config".into());
            request.cwd = Some("sentinel-cwd".into());
            request.session = Some("sentinel-session".into());
            request.histfile = Some("sentinel-hist".into());
            request.login_shell = Some("sentinel-shell".into());
            request.shell_path = Some("sentinel-path".into());
            request.ranking_off = Some("sentinel-off".into());
            request.shell = Some("recorded-shell".into());
            let listed = op.caller_fields();
            // Home and config keep an explicit value; clear them so the table's write is visible.
            if listed.contains(&CallerField::Home) {
                request.home = None;
            }
            if listed.contains(&CallerField::Config) {
                request.config = None;
            }
            caller.apply(&mut request);
            let carries = |field| listed.contains(&field);
            assert_eq!(
                request.home.as_deref() == Some("/caller-home"),
                carries(CallerField::Home),
                "{op:?}"
            );
            assert_eq!(
                request.config.as_deref() == Some("/caller-home/config.json"),
                carries(CallerField::Config),
                "{op:?}"
            );
            assert_eq!(
                request.cwd.as_deref() == Some("/caller-home/proj"),
                carries(CallerField::Cwd),
                "{op:?}"
            );
            assert_eq!(
                request.session.as_deref() == Some("sess"),
                carries(CallerField::Session),
                "{op:?}"
            );
            assert_eq!(
                request.histfile.as_deref() == Some("/caller-home/.zsh_history"),
                carries(CallerField::Histfile),
                "{op:?}"
            );
            assert_eq!(
                request.login_shell.as_deref() == Some("/bin/zsh"),
                carries(CallerField::Shell),
                "{op:?}"
            );
            assert_eq!(
                request.shell_path.as_deref() == Some("/usr/bin:/caller/bin"),
                carries(CallerField::Path),
                "{op:?}"
            );
            assert_eq!(
                request.ranking_off.as_deref() == Some("path,files"),
                carries(CallerField::RankingOff),
                "{op:?}"
            );
            assert_eq!(request.shell.as_deref(), Some("recorded-shell"), "{op:?}");
        }
    }

    #[test]
    fn fallback_and_service_build_identical_context() {
        let caller = CallerContext::from_env(&env());
        let mut request = ServiceRequest::new(Operation::Search);
        caller.apply(&mut request);
        request.cwd = Some(caller.cwd.clone());
        let launch = LaunchSettings {
            home: PathBuf::from("/launch-home"),
            config: PathBuf::from("/launch-home/.config/smcli/config.json"),
            idle: Duration::from_secs(1),
            fff_max: 1,
            fff_idle: Duration::from_secs(1),
        };
        assert_eq!(CallerContext::from_request(&request, &launch), caller);
        assert_eq!(CallerContext::from_env(&env()), caller);
    }

    #[test]
    fn launch_config_honors_xdg_config_home() {
        let env = HashMap::from([
            ("HOME".to_string(), "/h".to_string()),
            ("XDG_CONFIG_HOME".to_string(), "/x".to_string()),
        ]);
        let launch = LaunchSettings::from_env(&env);
        assert_eq!(launch.config, PathBuf::from("/x/smcli/config.json"));
        assert_eq!(
            CallerContext::from_request(&ServiceRequest::new(Operation::Search), &launch).config,
            launch.config
        );
    }
}
