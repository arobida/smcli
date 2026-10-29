use std::collections::HashMap;
use std::ffi::{CStr, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Home directory: `$HOME`, then the current user's `pw_dir`, then `/`.
pub fn home_dir(environment: &HashMap<String, String>) -> PathBuf {
    environment
        .get("HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| unsafe {
            let user = libc::getpwuid(libc::getuid());
            if user.is_null() {
                PathBuf::from("/")
            } else {
                PathBuf::from(OsStr::from_bytes(CStr::from_ptr((*user).pw_dir).to_bytes()))
            }
        })
}

/// User settings written by `smcli init`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmcliConfig {
    /// Apple's built-in embedding instead of Potion (`smcli init --builtin`). Not ported: Rust always uses Potion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub builtin: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctrl_g_key: Option<bool>,
    /// Picker keys bound by shell-init, chosen in `smcli init`: Ctrl-R and Ctrl-T file search (on unless false).
    /// On an empty line, Left opens that same history search and Right opens directory suggestions (Bash and Zsh;
    /// each off unless true). Up is always bound; SMCLI_NO_KEYS=1 still skips them all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctrl_r_key: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctrl_t_key: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd_filter: Option<Vec<String>>,
    /// Regexes; commands matching any, or run in a directory matching any of `cwd_filter`, are not recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_filter: Option<Vec<String>>,
    /// `@`-Tab opens the files picker inline under the prompt; false uses the fullscreen picker. On unless false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_completion_inline: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left_key: Option<bool>,
    /// Right arrow on an empty line opens directory suggestions (Bash and Zsh; off unless true).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right_key: Option<bool>,
    /// The model's vector size, recorded at init so the index can be opened without a network call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ollama_dimension: Option<i64>,
    /// Ollama embedding model for search. Not ported: kept so the file round-trips.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ollama_model: Option<String>,
    /// Directory of a potion-code-16M-v2 model (safetensors + tokenizer) to use instead of the built-in copy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub potion_path: Option<String>,
    /// Skip commands containing likely API keys and tokens; on unless set to false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secrets_filter: Option<bool>,
}

impl SmcliConfig {
    pub fn location(home: &Path, environment: &HashMap<String, String>) -> PathBuf {
        if let Some(path) = environment.get("SMCLI_CONFIG").filter(|p| !p.is_empty()) {
            return PathBuf::from(path);
        }
        let base = environment
            .get("XDG_CONFIG_HOME")
            .filter(|p| p.starts_with('/'))
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        base.join("smcli/config.json")
    }

    /// A missing file means defaults; a malformed one is an error rather than silently ignored.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(anyhow!("Invalid config {}: {e}", path.display())),
            Ok(data) => serde_json::from_slice(&data).with_context(|| format!("Invalid config {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        use std::os::unix::fs::DirBuilderExt;
        if let Some(parent) = path.parent() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    }
}

/// Decides which commands are never stored, following Atuin's capture filters:
/// a leading space (the shell convention for "keep this out of history"), likely secrets,
/// and user regexes over the command (`history_filter`) or its directory (`cwd_filter`).
#[derive(Debug, Clone)]
pub struct CaptureFilter {
    pub history_filter: Vec<Regex>,
    pub cwd_filter: Vec<Regex>,
    pub secrets_filter: bool,
}

/// Token shapes adapted from Atuin's default secrets filter.
const SECRET_PATTERNS: [&str; 10] = [
    "AKIA[0-9A-Z]{16}",                        // AWS access key ID
    "gh[pousr]_[A-Za-z0-9]{36}",               // GitHub tokens
    "github_pat_[A-Za-z0-9_]{82}",             // GitHub fine-grained PAT
    "glpat-[A-Za-z0-9_-]{20}",                 // GitLab PAT
    "xox[abposr]-[A-Za-z0-9-]{10,}",           // Slack tokens
    r"https://hooks\.slack\.com/services/\S+", // Slack webhook
    "[sr]k_live_[A-Za-z0-9]{24,}",             // Stripe live keys
    "AIza[0-9A-Za-z_-]{35}",                   // Google API key
    "npm_[A-Za-z0-9]{36}",                     // npm token
    "sk-(ant|proj)-[A-Za-z0-9_-]{20,}",        // Anthropic and OpenAI keys
];

fn secret_patterns() -> &'static [Regex] {
    static PATTERNS: std::sync::OnceLock<Vec<Regex>> = std::sync::OnceLock::new();
    PATTERNS.get_or_init(|| SECRET_PATTERNS.iter().map(|p| Regex::new(p).unwrap()).collect())
}

impl CaptureFilter {
    /// Secrets filter on, no user patterns.
    pub fn standard() -> Self {
        Self { history_filter: vec![], cwd_filter: vec![], secrets_filter: true }
    }

    pub fn new(config: &SmcliConfig) -> Result<Self> {
        // shortcut: Rust `regex` syntax, not NSRegularExpression (no lookaround/backreferences); fine for Atuin-style filters.
        let compile = |patterns: &Option<Vec<String>>, name: &str| -> Result<Vec<Regex>> {
            patterns
                .iter()
                .flatten()
                .map(|p| Regex::new(p).map_err(|_| anyhow!("Invalid {name} pattern: {p}")))
                .collect()
        };
        Ok(Self {
            history_filter: compile(&config.history_filter, "historyFilter")?,
            cwd_filter: compile(&config.cwd_filter, "cwdFilter")?,
            secrets_filter: config.secrets_filter.unwrap_or(true),
        })
    }

    pub fn allows_run(&self, command: &str, expanded: Option<&str>, directory: &str) -> bool {
        self.allows(command, directory) && expanded.is_none_or(|text| self.allows(text, directory))
    }

    pub fn allows(&self, command: &str, directory: &str) -> bool {
        if command.starts_with(' ') {
            return false;
        }
        if self.secrets_filter && secret_patterns().iter().any(|p| p.is_match(command)) {
            return false;
        }
        !self.history_filter.iter().any(|p| p.is_match(command)) && !self.cwd_filter.iter().any(|p| p.is_match(directory))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_secrets_spaces_and_patterns() {
        let config = SmcliConfig {
            history_filter: Some(vec!["^secret".into()]),
            cwd_filter: Some(vec!["/private".into()]),
            ..Default::default()
        };
        let filter = CaptureFilter::new(&config).unwrap();
        assert!(filter.allows("git status", "/repo"));
        assert!(!filter.allows(" git status", "/repo"));
        assert!(!filter.allows("export KEY=AKIAABCDEFGHIJKLMNOP", "/repo"));
        assert!(!filter.allows("secret thing", "/repo"));
        assert!(!filter.allows("ls", "/private/x"));
        assert!(CaptureFilter::new(&SmcliConfig { history_filter: Some(vec!["(".into()]), ..Default::default() }).is_err());
    }

    #[test]
    fn config_round_trips_swift_keys() {
        let text = r#"{"potionPath":"/m","ctrlRKey":false,"historyFilter":["x"]}"#;
        let config: SmcliConfig = serde_json::from_str(text).unwrap();
        assert_eq!(config.potion_path.as_deref(), Some("/m"));
        assert_eq!(config.ctrl_r_key, Some(false));
        let saved = serde_json::to_string(&config).unwrap();
        assert_eq!(saved, r#"{"ctrlRKey":false,"historyFilter":["x"],"potionPath":"/m"}"#);
        let inline: SmcliConfig = serde_json::from_str(r#"{"atCompletionInline":false}"#).unwrap();
        assert_eq!(inline.at_completion_inline, Some(false));
        assert_eq!(serde_json::to_string(&inline).unwrap(), r#"{"atCompletionInline":false}"#);
        let arrows: SmcliConfig = serde_json::from_str(r#"{"leftKey":false,"rightKey":true}"#).unwrap();
        assert_eq!(arrows.left_key, Some(false));
        assert_eq!(arrows.right_key, Some(true));
        assert_eq!(
            serde_json::to_string(&arrows).unwrap(),
            r#"{"leftKey":false,"rightKey":true}"#
        );
        let environment = HashMap::from([("XDG_CONFIG_HOME".to_string(), "/x".to_string())]);
        assert_eq!(SmcliConfig::location(Path::new("/h"), &environment), PathBuf::from("/x/smcli/config.json"));
        assert_eq!(SmcliConfig::location(Path::new("/h"), &HashMap::new()), PathBuf::from("/h/.config/smcli/config.json"));
    }
}
