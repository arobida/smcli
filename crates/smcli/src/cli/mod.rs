use anyhow::{Result, bail, ensure};
use clap::{Args, Parser, Subcommand};
use smcli_core::{
    config::{CaptureFilter, SmcliConfig, home_dir},
    history::CapturedHistory,
    service_client::ServiceClient,
    service_protocol::{Operation, ServiceRequest},
    time::{format_iso8601, now, parse_iso8601},
};
use smcli_search::setup::working_directory;
use std::{collections::HashMap, path::PathBuf, time::Duration};

// Printed for no arguments and top-level --help.
const USAGE: &str = include_str!("../usage.txt");

mod history;
mod index;
mod pick;
mod search;
mod tldr;

pub(crate) use tldr::captured_programs;

/// Stops a running service so the model it holds is freed; never starts one.
pub(crate) fn stop_service(service: &ServiceClient) -> bool {
    service
        .send(
            &ServiceRequest::new(Operation::Shutdown),
            Some(false),
            Duration::from_secs(10),
        )
        .is_some_and(|r| r.ok)
}

#[derive(Parser)]
#[command(
    name = "smcli",
    version,
    about = "Search and capture shell history",
    subcommand_required = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Init(Init),
    History {
        #[arg(long)]
        json: bool,
    },
    ShellInit {
        shell: String,
    },
    Search(Search),
    Suggest {
        #[arg(long)]
        dir: Option<String>,
        #[arg(long, short='n', default_value="10", value_parser=positive)]
        limit: usize,
        #[arg(long)]
        json: bool,
        prefix: Vec<String>,
    },
    Pick {
        #[arg(long, conflicts_with_all=["files","grep"])]
        suggest: bool,
        #[arg(long, conflicts_with = "grep")]
        files: bool,
        #[arg(long)]
        grep: bool,
        #[arg(long, conflicts_with_all=["suggest","files","grep"])]
        tldr: bool,
        query: Vec<String>,
    },
    Context {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        dir: Option<String>,
    },
    Reindex,
    Indexes {
        #[arg(long)]
        prune: bool,
        #[arg(long)]
        json: bool,
    },
    Health {
        #[arg(long)]
        json: bool,
    },
    Import {
        #[arg(long)]
        atuin: bool,
        #[arg(long, requires = "atuin")]
        replace: bool,
        #[arg(required_unless_present = "atuin")]
        source: Option<PathBuf>,
    },
    Prune {
        #[arg(long)]
        dry_run: bool,
    },
    Service {
        #[arg(value_parser=["start","stop","status"])]
        action: String,
    },
    #[command(hide = true)]
    Record {
        #[arg(long, allow_hyphen_values = true)]
        expanded: Option<String>,
        /// Read the expansion line and the command from stdin; the last field is `-`.
        #[arg(long, conflicts_with = "expanded")]
        stdin: bool,
        #[arg(num_args=5..=9, allow_hyphen_values=true)]
        fields: Vec<String>,
    },
    #[command(hide = true)]
    Complete {
        #[arg(long, allow_hyphen_values = true)]
        word: String,
        #[arg(long, allow_hyphen_values = true)]
        command: Option<String>,
        #[arg(long, default_value = "50", value_parser = positive)]
        limit: usize,
    },
}
#[derive(Args)]
pub struct Init {
    #[arg(long, num_args=0..=1, default_missing_value="", conflicts_with = "default")]
    pub potion: Option<String>,
    #[arg(long)]
    pub default: bool,
    #[arg(long, num_args=0..=1, default_missing_value="")]
    pub keys: Option<String>,
}
impl Init {
    pub fn has_model(&self) -> bool {
        self.potion.is_some() || self.default
    }
}
#[derive(Args)]
struct Search {
    query: Vec<String>,
    #[arg(long = "command")]
    program: Option<String>,
    #[arg(long)]
    subcommand: Option<String>,
    #[arg(long)]
    since: Option<String>,
    #[arg(long)]
    before: Option<String>,
    #[arg(long, value_parser=positive)]
    min_runs: Option<usize>,
    #[arg(long)]
    author: Option<String>,
    #[arg(long)]
    dir: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    flag: Option<String>,
    #[arg(long, short='n', default_value="10", value_parser=positive)]
    limit: usize,
    #[arg(long)]
    json: bool,
}
fn positive(s: &str) -> std::result::Result<usize, String> {
    s.parse::<usize>()
        .ok()
        .filter(|n| *n > 0 && *n <= i64::MAX as usize)
        .ok_or_else(|| "requires a positive number".into())
}
pub fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
/// `@` completion inserts a path, so it needs a known line the path can go into.
fn complete_allowed(lbuffer: Option<&str>, shell: &str) -> bool {
    lbuffer.is_some_and(|b| crate::shell_quoting::insertable(b, shell))
}
fn directory(value: Option<String>, env: &HashMap<String, String>) -> Result<String> {
    let d = value.unwrap_or_else(|| working_directory(env));
    ensure!(d.starts_with('/'), "--dir must be an absolute path.");
    Ok(d)
}
fn time(value: &str) -> Result<f64> {
    if let Some((n, unit)) = value
        .char_indices()
        .last()
        .and_then(|(i, u)| value[..i].parse::<u64>().ok().map(|n| (n, u)))
    {
        let scale = match unit {
            'm' => 60.0,
            'h' => 3600.0,
            'd' => 86400.0,
            'w' => 604800.0,
            _ => 0.0,
        };
        if n > 0 && scale > 0.0 {
            return Ok(now() - n as f64 * scale);
        }
    }
    ensure!(value.is_ascii(), "Invalid time: {value}");
    let day = value
        .get(..10)
        .ok_or_else(|| anyhow::anyhow!("Invalid time: {value}"))?;
    let date = format!("{day}T00:00:00Z");
    let midnight = parse_iso8601(&date).ok_or_else(|| anyhow::anyhow!("Invalid time: {value}"))?;
    ensure!(
        format_iso8601(midnight, false).starts_with(day),
        "Invalid date: {value}"
    );
    parse_iso8601(if value.len() == 10 { &date } else { value })
        .ok_or_else(|| anyhow::anyhow!("Invalid time: {value}"))
}
pub fn run() -> Result<()> {
    let env: HashMap<String, String> = std::env::vars().collect();
    let home = home_dir(&env);
    let mut captured = CapturedHistory::new(CapturedHistory::location(&home, &env));
    let executable = std::env::current_exe()?;
    let config_path = SmcliConfig::location(&home, &env);
    let first_start = !captured.path.exists();
    // First start (no database yet): say now if an XDG variable is ignored, before history goes somewhere unexpected.
    if first_start {
        for problem in smcli_core::paths::check(&home, &env) {
            eprintln!("smcli: {problem}");
        }
    }
    let filter = || CaptureFilter::new(&SmcliConfig::load(&config_path)?);
    let mut args: Vec<String> = std::env::args().collect();
    if args.len() == 1 && crate::menu::is_interactive() {
        let Some(chosen) = crate::menu::run(
            SmcliConfig::load(&config_path)
                .unwrap_or_default()
                .at_completion_inline
                != Some(false),
        )?
        else {
            return Ok(());
        };
        args.extend(chosen);
    }
    if args.len() == 1 || (args.len() == 2 && (args[1] == "--help" || args[1] == "-h")) {
        print!("{USAGE}");
        return Ok(());
    }
    let cli = Cli::parse_from(args);
    // Only commands that open the database move it; `record` must, or it would create the new one and strand history.
    if !matches!(cli.command, Command::ShellInit { .. }) {
        if let Err(error) = captured.migrate_legacy(&home, &env, |old| {
            stop_service(&ServiceClient::new(old, &env, &executable));
        }) {
            let old = CapturedHistory::legacy_location(&home);
            eprintln!(
                "smcli: could not move the database to {}: {error:#}; using {} for now",
                captured.path.display(),
                old.display()
            );
            captured = CapturedHistory::new(old);
        }
    }
    let service = ServiceClient::new(&captured.path, &env, &executable);
    match cli.command {
        Command::Init(args) => crate::initialize::run(args, &config_path, &service, &captured)?,
        Command::ShellInit { shell } => {
            let c = match SmcliConfig::load(&config_path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("smcli: {e:#}; using default keys");
                    SmcliConfig::default()
                }
            };
            println!(
                "{}",
                crate::shell_integration::render_config(&shell, &executable.to_string_lossy(), &c)?
            );
        }
        Command::Context { json, dir } => {
            crate::context::run(&directory(dir, &env)?, json, &service)?
        }
        Command::Record {
            mut fields,
            expanded,
            stdin,
        } => {
            let mut expanded = expanded;
            if stdin {
                ensure!(
                    [8, 9].contains(&fields.len()) && fields.last().is_some_and(|f| f == "-"),
                    "Invalid record arguments"
                );
                let mut text = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
                let Some((exp, command)) = history::split_record_stdin(&text) else {
                    bail!("Invalid record input");
                };
                expanded = exp.map(String::from);
                *fields.last_mut().unwrap() = command.to_string();
            }
            history::record(
                &fields,
                expanded.as_deref(),
                &captured,
                &service,
                &filter()?,
            )?
        }
        Command::Complete {
            word,
            command,
            limit,
        } => {
            let shell = env.get("SMCLI_SHELL").map(String::as_str).unwrap_or("bash");
            let lbuffer = env.get("SMCLI_LBUFFER").map(String::as_str);
            if !complete_allowed(lbuffer, shell) {
                if lbuffer.is_none() {
                    eprintln!("smcli: reload smcli's shell-init to enable @ completion.");
                }
                return Ok(());
            }
            let word = word.strip_prefix('@').unwrap_or(&word);
            let dirs_only = command.as_deref().is_some_and(pick::is_directory_command);
            let cwd = working_directory(&env);
            let mut request = ServiceRequest::new(Operation::Complete);
            request.query = Some(word.to_string());
            request.directory = Some(cwd.clone());
            request.limit = Some(limit as i64);
            request.dirs_only = Some(dirs_only);
            let (paths, chosen) = match service
                .send(&request, None, Duration::from_secs(10))
                .filter(|r| r.ok)
                .and_then(|r| r.files)
            {
                Some(hits) => {
                    let chosen = (hits.len() == 1).then(|| hits[0].path.clone());
                    let paths = hits
                        .into_iter()
                        .map(|h| {
                            if h.insert.is_empty() {
                                h.path
                            } else {
                                h.insert
                            }
                        })
                        .collect();
                    (paths, chosen)
                }
                None => {
                    let ignore = smcli_core::config::FileIgnore::load(&config_path, &home)?;
                    // Canonical, like the service scope, so anchored patterns match through symlinks.
                    let root = std::fs::canonicalize(&cwd).unwrap_or_else(|_| cwd.clone().into());
                    (
                        crate::complete::complete_local(
                            &root,
                            word,
                            dirs_only,
                            limit,
                            ignore.as_ref(),
                        ),
                        None,
                    )
                }
            };
            if let Some(path) = chosen {
                let mut r = ServiceRequest::new(Operation::Select);
                r.directory = Some(cwd);
                r.path = Some(path);
                r.mode = Some("files".into());
                r.query = Some(word.to_string());
                let _ = service.send(&r, None, Duration::from_secs(2));
            }
            for path in paths {
                println!("{}", crate::shell_quoting::quote(&path, shell));
            }
        }
        Command::Search(a) => search::search(a, &captured, &service, &env)?,
        Command::Suggest {
            dir,
            limit,
            json,
            prefix,
        } => {
            search::suggest(dir, limit, json, prefix, &captured, &service, &env)?;
        }
        Command::Reindex => crate::initialize::reindex(&captured, &home, &env, &service)?,
        Command::Indexes { prune, json } => index::indexes(&captured, &home, &env, prune, json)?,
        Command::Health { json } => index::health(&captured, &home, &env, json)?,
        Command::Service { action } if action == "stop" => println!(
            "{}",
            if stop_service(&service) {
                "smcli-service stopped."
            } else {
                "smcli-service was not running."
            }
        ),
        Command::Service { action } => {
            let response = service
                .send(
                    &ServiceRequest::new(Operation::Ping),
                    Some(action == "start"),
                    Duration::from_secs(10),
                )
                .filter(|r| r.ok);
            if let Some(pid) = response.and_then(|r| r.pid) {
                println!(
                    "smcli-service running (pid {pid}) at {} for {}",
                    service.socket().display(),
                    captured.path.display()
                );
            } else {
                bail!("smcli-service not running for {}", captured.path.display());
            }
        }
        Command::History { json } => history::history(&captured, &home, &env, json)?,
        Command::Prune { dry_run } => history::prune(&captured, &filter()?, dry_run)?,
        Command::Import {
            atuin,
            replace,
            source,
        } => history::import(&captured, &home, &env, atuin, replace, source)?,
        Command::Pick {
            suggest,
            files,
            grep,
            tldr,
            query,
        } => {
            if tldr {
                tldr::pick_tldr(&captured, &service, &home, &env)?;
                return Ok(());
            }
            pick::pick(
                &captured,
                &service,
                &home,
                &env,
                suggest,
                files,
                grep,
                query.join(" "),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn complete_allowed_needs_an_insertable_line() {
        assert!(complete_allowed(Some(""), "bash"));
        assert!(complete_allowed(Some("cat @"), "bash"));
        assert!(!complete_allowed(None, "bash"));
        assert!(!complete_allowed(Some("echo \"@"), "bash"));
    }

    #[test]
    fn only_initialize_loads_the_model() -> Result<()> {
        fn scan(directory: &Path) -> Result<()> {
            for file in std::fs::read_dir(directory)? {
                let path = file?.path();
                if path.is_dir() {
                    scan(&path)?;
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let text = std::fs::read_to_string(&path)?;
                    let production = text.split("#[cfg(test)]").next().unwrap();
                    for forbidden in ["rusqlite::", "taladb::"] {
                        assert!(
                            !production.contains(forbidden),
                            "{} accesses storage via {forbidden}",
                            path.display()
                        );
                    }
                    if path.file_name().unwrap() != "initialize.rs" {
                        for forbidden in [
                            "search_engine(",
                            "Potion::bundled(",
                            "Potion::from_dir(",
                            ".embed(",
                        ] {
                            assert!(
                                !production.contains(forbidden),
                                "{} loads the model via {forbidden}",
                                path.display()
                            );
                        }
                    }
                }
            }
            Ok(())
        }
        scan(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"))
    }

    #[test]
    fn smcli_has_no_storage_dependencies() {
        let manifest = include_str!("../../Cargo.toml");
        let mut dependencies = false;
        for line in manifest.lines().map(str::trim) {
            if line.starts_with('[') {
                dependencies = line == "[dependencies]";
            } else if dependencies && let Some((name, _)) = line.split_once('=') {
                assert!(
                    !["rusqlite", "taladb"].contains(&name.trim()),
                    "{name} must not be a production dependency"
                );
            }
        }
    }

    #[test]
    fn parse_flags_and_times() {
        assert!(Cli::try_parse_from(["smcli", "search", "git", "--flag", "--all"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "smcli",
                "complete",
                "--word",
                "@foo",
                "--command",
                "cd",
                "--limit",
                "5"
            ])
            .is_ok()
        );
        assert!(Cli::try_parse_from(["smcli", "suggest", "--limit", "0"]).is_err());
        assert!(Cli::try_parse_from(["smcli", "pick", "--tldr"]).is_ok());
        assert_eq!(
            Cli::try_parse_from(["smcli", "--version"])
                .err()
                .map(|e| e.kind()),
            Some(clap::error::ErrorKind::DisplayVersion)
        );
        assert!(Cli::try_parse_from(["smcli", "init", "--model", "x"]).is_err());
        assert!(Cli::try_parse_from(["smcli", "init", "--builtin"]).is_err());
        for flag in ["--suggest", "--files", "--grep"] {
            assert!(Cli::try_parse_from(["smcli", "pick", "--tldr", flag]).is_err());
        }
        assert_eq!(
            time("2024-01-01").unwrap(),
            parse_iso8601("2024-01-01T00:00:00Z").unwrap()
        );
        assert!((now() - time("30m").unwrap() - 1800.0).abs() < 1.0);
        assert!(history::epoch("NaN").is_err());
        assert!(time("2024-02-31").is_err());
        assert!(time("2024-01-01T00:00:00♥").is_err());
    }
}
