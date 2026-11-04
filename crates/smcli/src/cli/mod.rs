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
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

// Printed for no arguments, --help and parse errors.
const USAGE: &str = include_str!("../usage.txt");

mod history;
mod index;
mod pick;
mod search;
mod tldr;

pub(crate) use tldr::captured_programs;

/// Stops a running service so the model it holds is freed; never starts one.
pub(crate) fn stop_service(service: &ServiceClient) -> bool {
    service.send(&ServiceRequest::new(Operation::Shutdown), Some(false), Duration::from_secs(10)).is_some_and(|r| r.ok)
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
    #[arg(long, conflicts_with_all=["potion","builtin","default"])]
    pub model: Option<String>,
    #[arg(long, num_args=0..=1, default_missing_value="", conflicts_with_all=["builtin","default"])]
    pub potion: Option<String>,
    #[arg(long, conflicts_with = "default")]
    pub builtin: bool,
    #[arg(long)]
    pub default: bool,
    #[arg(long, num_args=0..=1, default_missing_value="")]
    pub keys: Option<String>,
}
impl Init {
    pub fn has_model(&self) -> bool {
        self.model.is_some() || self.potion.is_some() || self.builtin || self.default
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
    let captured = CapturedHistory::new(CapturedHistory::location(&home, &env));
    let executable = std::env::current_exe()?;
    let service = ServiceClient::new(&captured.path, &env, &executable);
    let config_path = SmcliConfig::location(&home, &env);
    let filter = || CaptureFilter::new(&SmcliConfig::load(&config_path)?);
    let mut args: Vec<String> = std::env::args().collect();
    if args.len() == 1 && crate::menu::is_interactive() {
        let Some(chosen) = crate::menu::run(SmcliConfig::load(&config_path).unwrap_or_default().at_completion_inline != Some(false))? else {
            return Ok(());
        };
        args.extend(chosen);
    }
    if args.len() == 1 || (args.len() == 2 && (args[1] == "--help" || args[1] == "-h")) {
        print!("{USAGE}");
        return Ok(());
    }
    let cli = Cli::try_parse_from(args).unwrap_or_else(|_| {
        eprint!("{USAGE}");
        std::process::exit(2)
    });
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
        Command::Record { fields, expanded } => history::record(&fields, expanded.as_deref(), &captured, &service, &filter()?)?,
        Command::Complete { word, command, limit } => {
            let word = word.strip_prefix('@').unwrap_or(&word);
            let dirs_only = command.as_deref().is_some_and(pick::is_directory_command);
            let cwd = working_directory(&env);
            let shell = env.get("SMCLI_SHELL").map(String::as_str).unwrap_or("bash");
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
                        .map(|h| if h.insert.is_empty() { h.path } else { h.insert })
                        .collect();
                    (paths, chosen)
                }
                None => (crate::complete::complete_local(Path::new(&cwd), word, dirs_only, limit), None),
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
        Command::Suggest { dir, limit, json, prefix } => {
            search::suggest(dir, limit, json, prefix, &captured, &service, &env)?;
        }
        Command::Reindex => crate::initialize::reindex(&captured, &home, &env, &service)?,
        Command::Indexes { prune, json } => index::indexes(&captured, &home, &env, prune, json)?,
        Command::Health { json } => index::health(&captured, &home, &env, json)?,
        Command::Service { action } if action == "stop" => println!(
            "{}",
            if stop_service(&service) { "smcli-service stopped." } else { "smcli-service was not running." }
        ),
        Command::Service { action } => {
            let response = service
                .send(&ServiceRequest::new(Operation::Ping), Some(action == "start"), Duration::from_secs(10))
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

    #[test]
    fn only_initialize_loads_the_model() -> Result<()> {
        fn scan(directory: &Path) -> Result<()> {
            for file in std::fs::read_dir(directory)? {
                let path = file?.path();
                if path.is_dir() { scan(&path)?; }
                else if path.extension().is_some_and(|ext| ext == "rs") {
                    let text = std::fs::read_to_string(&path)?;
                    let production = text.split("#[cfg(test)]").next().unwrap();
                    for forbidden in ["rusqlite::", "taladb::"] {
                        assert!(!production.contains(forbidden), "{} accesses storage via {forbidden}", path.display());
                    }
                    if path.file_name().unwrap() != "initialize.rs" {
                        for forbidden in ["search_engine(", "Potion::bundled(", "Potion::from_dir(", ".embed("] {
                            assert!(!production.contains(forbidden), "{} loads the model via {forbidden}", path.display());
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
            if line.starts_with('[') { dependencies = line == "[dependencies]"; }
            else if dependencies && let Some((name, _)) = line.split_once('=') {
                assert!(!["rusqlite", "taladb"].contains(&name.trim()), "{name} must not be a production dependency");
            }
        }
    }

    #[test]
    fn parse_flags_and_times() {
        assert!(Cli::try_parse_from(["smcli", "search", "git", "--flag", "--all"]).is_ok());
        assert!(Cli::try_parse_from(["smcli", "complete", "--word", "@foo", "--command", "cd", "--limit", "5"]).is_ok());
        assert!(Cli::try_parse_from(["smcli", "suggest", "--limit", "0"]).is_err());
        assert!(Cli::try_parse_from(["smcli", "pick", "--tldr"]).is_ok());
        for flag in ["--suggest", "--files", "--grep"] {
            assert!(Cli::try_parse_from(["smcli", "pick", "--tldr", flag]).is_err());
        }
        assert_eq!(time("2024-01-01").unwrap(), parse_iso8601("2024-01-01T00:00:00Z").unwrap());
        assert!((now() - time("30m").unwrap() - 1800.0).abs() < 1.0);
        assert!(history::epoch("NaN").is_err());
        assert!(time("2024-02-31").is_err());
        assert!(time("2024-01-01T00:00:00♥").is_err());
    }
}
