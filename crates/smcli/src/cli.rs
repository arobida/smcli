use anyhow::{Result, bail, ensure};
use clap::{Args, Parser, Subcommand};
use smcli_core::{
    caller::CallerContext,
    config::{CaptureFilter, SmcliConfig, home_dir},
    context::ProjectContext,
    history::{CapturedHistory, NewRun, parse_pipestatus, user_name},
    ranking::RankingContext,
    service_client::ServiceClient,
    service_location::relative_insert,
    service_protocol::{Operation, SearchEntry, SearchFilter, ServiceRequest},
    time::{format_iso8601, now, parse_iso8601},
};
use smcli_search::setup::{configured_model, corpus, index_directory, search_signals, working_directory};
use smcli_search::lexical_search;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};
use crate::picker::{FileResults, Mode};
use smcli_core::service_protocol::FileHit;

// Swift-identical usage text, printed for no arguments, --help and parse errors.
const USAGE: &str = include_str!("usage.txt");

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
        Command::Record { fields, expanded } => record(&fields, expanded.as_deref(), &captured, &service, &filter()?)?,
        Command::Complete { word, command, limit } => {
            let word = word.strip_prefix('@').unwrap_or(&word);
            let dirs_only = command.as_deref().is_some_and(is_directory_command);
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
        Command::Search(a) => {
            let filter = SearchFilter {
                program: a.program,
                subcommand: a.subcommand,
                since: a.since.as_deref().map(time).transpose()?,
                before: a.before.as_deref().map(time).transpose()?,
                min_runs: a.min_runs.map(|n| n as i64),
                author: a.author,
                directory: a.dir.map(|d| directory(Some(d), &env)).transpose()?,
                flag: a.flag,
            };
            let query = a.query.join(" ");
            let mut request = ServiceRequest::new(Operation::Search);
            request.query = Some(query.clone());
            request.limit = Some(a.limit as i64);
            request.directory = Some(working_directory(&env));
            request.session = env.get("SMCLI_SESSION").cloned();
            request.filter = Some(filter.clone());
            request.histfile = env.get("HISTFILE").cloned();
            request.login_shell = env.get("SHELL").cloned();
            let hits = match service
                .send(&request, None, Duration::from_secs(600))
                .filter(|r| r.ok)
                .and_then(|r| r.hits)
            {
                Some(hits) => hits,
                None => {
                    let caller = CallerContext::from_env(&env);
                    let context = RankingContext::new(
                        &ProjectContext::discover(&caller.cwd),
                        caller.session.clone(),
                        caller.ranking_off.as_deref(),
                    );
                    eprintln!("smcli: search service unavailable; showing keyword matches only.");
                    let native = caller.native_env();
                    let corpus = corpus(
                        &captured,
                        caller.home(),
                        &native,
                        context.project_root.as_deref(),
                        context.kind(),
                        now(),
                    )?;
                    lexical_search(&query, &corpus, a.limit, &filter,
                        &search_signals(&captured, Some(context), &native)?, now())?
                }
            };
            if a.json {
                print_json(&hits)?;
            } else {
                print_hits(&hits);
            }
        }
        Command::Suggest {
            dir,
            limit,
            json,
            prefix,
        } => {
            let d = directory(dir, &env)?;
            let prefix = prefix.join(" ");
            let mut request = ServiceRequest::new(Operation::Suggest);
            request.directory = Some(d.clone());
            request.prefix = Some(prefix.clone());
            request.limit = Some(limit as i64);
            request.session = env.get("SMCLI_SESSION").cloned();
            let suggestions = match service
                .send(&request, None, Duration::from_secs(10))
                .filter(|r| r.ok)
                .and_then(|r| r.suggestions)
            {
                Some(s) => s,
                None => {
                    let caller = CallerContext::from_env(&env);
                    captured.suggestions(
                    &user_name(),
                    &d,
                    &prefix,
                    limit,
                    now(),
                    Some(&RankingContext::new(
                        &ProjectContext::discover(&d),
                        caller.session.clone(),
                        caller.ranking_off.as_deref(),
                    )),
                )?
                }
            };
            if json {
                print_json(&suggestions)?;
            } else {
                for s in suggestions {
                    println!("{}", crate::picker::history_label(&s.key()));
                }
            }
        }
        Command::Reindex => crate::initialize::reindex(&captured, &home, &env, &service)?,
        Command::Indexes { prune, json } => indexes(&captured, &home, &env, prune, json)?,
        Command::Health { json } => {
            let corpus = corpus(&captured, &home, &env, None, None, now())?;
            let report = index_health(&captured, &home, &env, &corpus);
            if json {
                print_json(&report)?;
            } else {
                println!(
                    "Embedder:     {}\nIndex:        {}",
                    report.embedder, report.path
                );
                if report.built {
                    println!(
                        "Commands:     {} indexed of {} in history",
                        report.indexed, report.commands
                    );
                    if report.missing > 0 {
                        println!(
                            "Missing:      {} (embedded on the next search)",
                            report.missing
                        );
                    }
                    if report.stale > 0 {
                        println!(
                            "Stale:        {} (removed on the next search)",
                            report.stale
                        );
                    }
                    println!("Size:         {} bytes", report.bytes);
                    if let Some(updated) = &report.last_updated {
                        println!("Updated:      {updated}");
                    }
                } else {
                    println!(
                        "Commands:     not built yet; the next search or `smcli reindex` embeds {}",
                        report.commands
                    );
                }
                if !report.other_indexes.is_empty() {
                    println!("Other models: {}", report.other_indexes.join(", "));
                }
                println!(
                    "Status:       {}",
                    if report.healthy {
                        "healthy"
                    } else {
                        "unhealthy"
                    }
                );
                for problem in &report.problems {
                    println!("  ✗ {problem}");
                }
            }
            if !report.healthy {
                std::process::exit(1);
            }
        }
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
        Command::History { json } => history(&captured, &home, &env, json)?,
        Command::Prune { dry_run } => prune(&captured, &filter()?, dry_run)?,
        Command::Import {
            atuin,
            replace,
            source,
        } => import(&captured, &home, &env, atuin, replace, source)?,
        Command::Pick {
            suggest,
            files,
            grep,
            tldr,
            query,
        } => {
            if tldr {
                pick_tldr(&captured, &service, &home, &env)?;
                return Ok(());
            }
            pick(
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
fn print_hits(hits: &[SearchEntry]) {
    if hits.is_empty() {
        println!("No matching commands.");
    }
    for h in hits {
        let mut details = vec![format!("runs={}", h.runs)];
        if let Some(last) = &h.last_run {
            details.push(format!("last={last}"));
        }
        if let Some(code) = h.last_exit_code {
            details.push(format!("exit={code}"));
        }
        if let Some(d) = &h.last_directory {
            details.push(d.clone());
        }
        if h.runs_here > 0 {
            details.push(format!("here={}", h.runs_here));
        }
        if h.source == "native" {
            details.push("native history".into());
        }
        println!("{}\n    {}", crate::picker::history_label(&h.key()), details.join("  "));
        if let Some(why) = &h.why {
            if !why.is_empty() {
                println!("    why: {}", why.join(", "));
            }
        }
    }
}
fn epoch(s: &str) -> Result<Option<f64>> {
    if s.is_empty() {
        return Ok(None);
    }
    let value: f64 = s.replace(',', ".").parse()?;
    ensure!(value.is_finite() && value > 0.0, "Invalid time: {s}");
    Ok(Some(value))
}
fn record(
    f: &[String],
    expanded: Option<&str>,
    captured: &CapturedHistory,
    service: &ServiceClient,
    filter: &CaptureFilter,
) -> Result<()> {
    ensure!([5, 8, 9].contains(&f.len()), "Invalid record arguments");
    // The hook only pre-checks; an expansion the full rule refuses is dropped, not the run: unsupported means typed-only.
    let command = &f[if f.len() == 5 { 3 } else { f.len() - 1 }];
    let expanded = expanded.filter(|e| smcli_core::parse::safe_expansion(e) && e != command);
    if f.len() == 5 {
        if filter.allows_run(&f[3], expanded, &f[1]) {
            let mut run = NewRun::new(&f[3], &f[0], &f[1], f[2].parse()?);
            run.expanded_command = expanded;
            captured.append(
                &run,
                &f[4],
                None,
            )?;
        }
        return Ok(());
    }
    let tail = f.len() - 4;
    if !filter.allows_run(&f[tail + 3], expanded, &f[2]) {
        return Ok(());
    }
    captured.check_separate(&f[tail + 2])?;
    let mut run = NewRun::new(&f[tail + 3], &f[0], &f[2], f[3].parse()?);
    run.expanded_command = expanded;
    run.session = &f[1];
    run.started_at = Some(epoch(&f[tail])?.unwrap_or_else(now));
    run.ended_at = epoch(&f[tail + 1])?;
    run.pipestatus = if f.len() == 9 {
        parse_pipestatus(&f[4])?
    } else {
        None
    };
    run.event = Some(uuid::Uuid::new_v4().to_string());
    let mut r = ServiceRequest::new(Operation::Record);
    r.command = Some(run.command.into());
    r.expanded_command = run.expanded_command.map(String::from);
    r.shell = Some(run.shell.into());
    r.directory = Some(run.directory.into());
    r.exit_code = Some(run.exit_code);
    r.session = Some(run.session.into());
    r.started_at = run.started_at;
    r.ended_at = run.ended_at;
    r.pipestatus = run.pipestatus.clone();
    r.event = run.event.clone();
    r.native_file = Some(f[tail + 2].clone());
    if !service
        .send(&r, None, Duration::from_secs(10))
        .is_some_and(|r| r.ok)
    {
        captured.append(&run, &f[tail + 2], None)?;
    }
    Ok(())
}

fn history(
    c: &CapturedHistory,
    home: &Path,
    env: &HashMap<String, String>,
    json: bool,
) -> Result<()> {
    let records = c.export_rows()?;
    if json {
        return print_json(&records);
    }
    let histories = smcli_search::native::history_files(home, env)?
        .into_iter()
        .map(|f| Ok((f.path.clone(), f.read()?)))
        .collect::<Result<Vec<_>>>()?;
    if records.is_empty() && histories.is_empty() {
        println!("No saved Bash or Zsh history found.");
    }
    for (path, text) in histories {
        println!("==> {} <==", path.display());
        print!("{text}");
        if !text.ends_with('\n') {
            println!();
        }
    }
    if !records.is_empty() {
        println!("==> {} <==", c.path.display());
    }
    for r in records {
        println!(
            "{} [{}@{} {}] {} exit={} runCount={}\n{}",
            r["timestamp"].as_str().unwrap_or(""),
            r["user"].as_str().unwrap_or(""),
            r["host"].as_str().unwrap_or(""),
            r["directory"].as_str().unwrap_or(""),
            r["shell"].as_str().unwrap_or(""),
            r["exitCode"],
            r["runCount"],
            r["command"].as_str().unwrap_or("")
        );
    }
    Ok(())
}
fn prune(c: &CapturedHistory, filter: &CaptureFilter, dry: bool) -> Result<()> {
    if !c.path.exists() {
        println!(
            "{}",
            if dry {
                "0 records would be removed."
            } else {
                "Removed 0 records."
            }
        );
        return Ok(());
    }
    let directory = index_directory(c);
    let _lock = if dry { None } else {
        Some(smcli_search::index::lock_index_directory(&directory, u32::MAX)?
            .ok_or_else(|| anyhow::anyhow!("Another smcli is updating the index."))?)
    };
    let pruned = c.prune(filter, dry, || smcli_search::index::remove_history_indexes(&directory));
    let report = match pruned {
        Ok(report) => report,
        Err(error) => {
            if let Some(failed) = error.downcast_ref::<smcli_core::prune::PruneError>() {
                println!("Removed {} commands and {} selections.", failed.removed_commands, failed.removed_selections);
                warn_migration_backup(failed.backup.as_deref());
            }
            return Err(error);
        }
    };
    if dry {
        for command in report.commands { println!("{command}"); }
        println!("{} commands and {} selections would be removed.", report.removed_commands, report.removed_selections);
    } else {
        println!("Removed {} commands and {} selections.", report.removed_commands, report.removed_selections);
        warn_migration_backup(report.backup.as_deref());
    }
    Ok(())
}

fn warn_migration_backup(backup: Option<&Path>) {
    if let Some(backup) = backup {
        eprintln!("smcli: migration backup {} may still contain pruned history; remove it manually if no longer needed.", backup.display());
    }
}
fn import(
    c: &CapturedHistory,
    home: &Path,
    env: &HashMap<String, String>,
    atuin: bool,
    replace: bool,
    source: Option<PathBuf>,
) -> Result<()> {
    let source = source.unwrap_or_else(|| {
        env.get("ATUIN_DB_PATH")
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                env.get("XDG_DATA_HOME")
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".local/share"))
                    .join("atuin/history.db")
            })
    });
    let capture_filter = CaptureFilter::new(&SmcliConfig::load(&SmcliConfig::location(home, env))?)?;
    let count = if atuin { c.import_atuin(&source, &capture_filter, replace)? }
        else { c.import_legacy(&source, &capture_filter)? };
    println!("Imported {count} records into {}.", c.path.display());
    Ok(())
}
const DIRECTORY_COMMANDS: &[&str] = &["cd", "pushd", "popd", "z", "rmdir", "mkdir"];

fn is_directory_command(command: &str) -> bool {
    DIRECTORY_COMMANDS.contains(&command)
}

fn files_kind(c: &CapturedHistory, env: &HashMap<String, String>) -> u8 {
    let line = env.get("SMCLI_LBUFFER").map(String::as_str).unwrap_or("");
    let Some(command) = smcli_core::parse::last_command_word(line, env.get("SMCLI_SHELL").map(String::as_str).unwrap_or("bash")) else { return 0 };
    let command = command.rsplit('/').next().unwrap_or(&command).to_string();
    if is_directory_command(&command) { return 1; }
    // Pre-C files_majority take(20) was on this same filtered stream, so the SQL cursor stops there too.
    let Ok(rows) = c.file_kind_rows(&user_name(), 20, |raw| {
        smcli_core::parse::program_of(raw, smcli_core::parse::CommandBreakdown::parse(raw).as_ref()).as_deref() == Some(command.as_str())
    }) else { return 0 };
    files_majority(rows.into_iter().map(|(raw, expanded, cwd, shell)| {
        let (mut dirs, mut files) = (0, 0);
        let command = expanded.as_deref().unwrap_or(&raw);
        for path in smcli_core::parse::path_arguments(command, &shell, |p| Path::new(&cwd).join(p).exists()) {
            let full = Path::new(&cwd).join(path);
            dirs += usize::from(full.is_dir());
            files += usize::from(full.is_file());
        }
        (dirs, files)
    }))
}

fn files_majority(rows: impl IntoIterator<Item = (usize, usize)>) -> u8 {
    let (dirs, files) = rows.into_iter().take(20).fold((0, 0), |(d, f), (dirs, files)| (d + dirs, f + files));
    if dirs > files { 1 } else if files > dirs { 2 } else { 0 }
}

#[derive(Debug, PartialEq, Eq)]
struct TldrContext {
    leading: String,
    program: Option<String>,
    prefix_words: Vec<String>,
}

fn tldr_context(line: &str) -> TldrContext {
    let words = line.split_whitespace().collect::<Vec<_>>();
    let mut program_index = 0;
    while words.get(program_index).is_some_and(|word| matches!(*word, "sudo" | "env" | "command")) {
        program_index += 1;
    }
    let program = words.get(program_index).map(|word| word.rsplit('/').next().unwrap_or(word).to_owned());
    let mut leading = words[..program_index].join(" ");
    if !leading.is_empty() {
        leading.push(' ');
    }
    let mut prefix_words = words.get(program_index..).unwrap_or_default().iter().map(|word| (*word).to_owned()).collect::<Vec<_>>();
    if let Some(program) = &program {
        prefix_words[0] = program.clone();
    }
    TldrContext { leading, program, prefix_words }
}

/// A failed prepare latches word ranking too; a complete index needs no service round trip.
fn prepare_tldr(index: &smcli_search::commands::CommandIndex, service: &ServiceClient, program: Option<&str>) -> Result<bool> {
    let timeout = if !index.path().exists() {
        Duration::from_secs(600)
    } else if let Some(program) = program {
        if index.examples(program)?.is_some() { return Ok(true); }
        Duration::from_secs(10)
    } else {
        return Ok(true);
    };
    let mut request = ServiceRequest::new(Operation::Embed);
    request.mode = Some("commands".into());
    request.command = program.map(str::to_owned);
    request.query = Some(String::new());
    Ok(service.send(&request, None, timeout).is_some_and(|response| response.ok))
}

fn tldr_ranker(mut service: Option<ServiceClient>, expected_model: String, entries: Vec<smcli_search::Entry>,
               vectors: Vec<Vec<f32>>, prefix: Vec<String>)
    -> impl FnMut(&str, bool) -> Result<Vec<String>> {
    let program = prefix.first().cloned();
    let mut page_subset: Option<Vec<usize>> = None;
    let mut all_subset: Option<Vec<usize>> = None;
    move |query: &str, all: bool| -> Result<Vec<String>> {
        let subset: &mut Vec<usize> = if all {
            if all_subset.is_none() {
                all_subset = Some((0..entries.len()).collect());
            }
            all_subset.as_mut().unwrap()
        } else {
            if page_subset.is_none() {
                page_subset = Some(
                    entries
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| Some(&entry.program) == program.as_ref())
                        .map(|(index, _)| index)
                        .collect(),
                );
            }
            page_subset.as_mut().unwrap()
        };
        let subset_entries: Vec<smcli_search::Entry> = subset.iter().map(|index| entries[*index].clone()).collect();
        let subset_vectors: Vec<Vec<f32>> = subset.iter().map(|index| vectors[*index].clone()).collect();
        // Empty embed queries prepare only, so they must not latch away later semantic queries.
        let query_vector = service.as_ref().filter(|_| !query.trim().is_empty()).and_then(|service| {
            let mut request = ServiceRequest::new(Operation::Embed);
            request.query = Some(query.to_owned());
            let response = service.send(&request, None, Duration::from_secs(2))?;
            if !response.ok || response.model.as_deref() != Some(expected_model.as_str()) { return None; }
            response.embedding.filter(|vector| !vector.is_empty() && vector.iter().all(|x| x.is_finite())
                && subset_vectors.iter().all(|stored| stored.len() == vector.len()))
        });
        if !query.trim().is_empty() && query_vector.is_none() { service = None; }
        let empty = Vec::new();
        let prefix = if all { &empty } else { &prefix };
        let ranked = match query_vector {
            Some(vector) => smcli_search::tldr::rank_with_vectors(&vector, &subset_entries, &subset_vectors, prefix),
            None => smcli_search::tldr::rank_words(query, &subset_entries, prefix),
        };
        Ok(ranked
            .into_iter()
            .map(|(position, _)| subset[position].to_string())
            .collect())
    }
}

fn pick_tldr(c: &CapturedHistory, service: &ServiceClient, home: &Path, env: &HashMap<String, String>) -> Result<()> {
    use crate::picker::{Choice, Mode, Picker};

    let context = tldr_context(env.get("SMCLI_LBUFFER").map(String::as_str).unwrap_or(""));
    let sources = smcli_search::commands::bundled_sources()
        .ok_or_else(|| anyhow::anyhow!("tldr data not found beside the smcli executable or in Data/tldr"))?;
    let refs = smcli_search::commands::source_refs(&sources);
    let index = smcli_search::commands::CommandIndex::reader(index_directory(c), configured_model(home, env)?, &refs);
    let prepared = prepare_tldr(&index, service, context.program.as_deref())?;
    let prefix = context.prefix_words.clone();
    // `@@` only reads: the service prepares and syncs the index.
    // The picker owns every indexed row; the closure ranks a view's subset and returns global row indices.
    let rows: Vec<(smcli_search::Entry, Vec<f32>, String)> = index
        .all()?
        .into_iter()
        .map(|example| {
            (
                smcli_search::Entry { program: example.program, description: example.description, template: example.template },
                example.embedding,
                example.source,
            )
        })
        .collect();
    let query = env.get("SMCLI_TLDR_QUERY").map(|query| query.trim_start().to_owned()).unwrap_or_default();
    let mut picker = Picker::new(Vec::new(), Vec::new(), Vec::new(), query);
    picker.mode = Mode::Tldr;
    picker.tldr_all = context.program.is_none();
    picker.tldr_program = context.program.clone();
    picker.title = Some(if picker.tldr_all { "all installed".into() } else { context.program.clone().unwrap_or_default() });
    picker.tldr_status = if !index.path().exists() {
        Some("command examples need smcli-service or `smcli reindex`".into())
    } else {
        context.program.as_ref().filter(|program| !rows.iter().any(|(entry, _, _)| &entry.program == *program))
            .map(|program| format!("no examples for {program}; Ctrl-A searches all installed"))
    };
    picker.tldr_entries = rows.iter().map(|(entry, _, _)| entry.clone()).collect();
    picker.tldr_sources = rows.iter().map(|(_, _, source)| source.clone()).collect();
    let entries: Vec<smcli_search::Entry> = rows.iter().map(|(entry, _, _)| entry.clone()).collect();
    let vectors: Vec<Vec<f32>> = rows.into_iter().map(|(_, vector, _)| vector).collect();
    let service = if prepared { Some(ServiceClient::new(&c.path, env, &std::env::current_exe()?)) } else { None };
    let search = tldr_ranker(service, index.index_name(), entries, vectors, prefix);
    let config = SmcliConfig::load(&SmcliConfig::location(home, env)).unwrap_or_default();
    if let Some(Choice::Command { command, .. }) = crate::picker::run(picker, Some(Box::new(search)), None, config.at_completion_inline != Some(false))? {
        println!("{}{}", context.leading, command);
    }
    Ok(())
}

/// First words of captured commands (smcli's own history, distinct).
pub(crate) fn captured_programs(c: &CapturedHistory) -> Result<HashSet<String>> {
    let stats = c.command_stats(None, None, None, now())?;
    Ok(stats
        .keys()
        .filter_map(|key| key.split_whitespace().next().map(|word| word.rsplit('/').next().unwrap_or(word).to_owned()))
        .collect())
}

fn pick(
    c: &CapturedHistory,
    service: &ServiceClient,
    home: &Path,
    env: &HashMap<String, String>,
    suggest: bool,
    files: bool,
    grep: bool,
    query: String,
) -> Result<()> {
    use crate::picker::{Choice, FileResults, Mode, Picker};
    let caller = CallerContext::from_env(env);
    let d = working_directory(env);
    let context = RankingContext::new(
        &ProjectContext::discover(&d),
        caller.session.clone(),
        caller.ranking_off.as_deref(),
    );
    let mut request = ServiceRequest::new(Operation::Pick);
    request.directory = Some(d.clone());
    request.limit = Some(50);
    request.session = env.get("SMCLI_SESSION").cloned();
    let served = service
        .send(&request, None, Duration::from_secs(10))
        .filter(|r| r.ok);
    let suggestions = match served.as_ref().and_then(|r| r.suggestions.clone()) {
        Some(s) => s,
        None => c.suggestions(&user_name(), &d, "", 50, now(), Some(&context))?,
    }
    .into_iter()
    .map(|s| s.key())
    .collect();
    let user = user_name();
    let recent = match served.and_then(|r| r.recent_entries) {
        Some(r) => r.into_iter().map(|entry| entry.key()).collect(),
        None => c.recent_distinct(&user)?,
    };
    let native = smcli_search::native::history_files(caller.home(), &caller.native_env())?
        .iter()
        .filter_map(|f| f.read().ok())
        .flat_map(|s| s.lines().map(String::from).collect::<Vec<_>>())
        .collect();
    let mut picker = Picker::new(
        suggestions,
        recent,
        native,
        // Ctrl-T's query is the whole command line, so files mode starts empty unless `@` completion names a word.
        if files { env.get("SMCLI_FILES_QUERY").cloned().unwrap_or_default() } else if grep { String::new() } else { query },
    );
    picker.suggested_view = suggest;
    picker.title = Some(format!("Suggested for {d}"));
    if files {
        picker.kind = files_kind(c, env);
        picker.files_line = env.get("SMCLI_FILES_LINE").cloned();
    }
    picker.mode = if files {
        Mode::Files
    } else if grep {
        Mode::Grep
    } else {
        Mode::History
    };
    let project = ProjectContext::discover(&d);
    let root = project
        .repository
        .as_ref()
        .map(|r| r.root.clone())
        .unwrap_or_else(|| d.clone());
    picker.scope = Some(root.clone());
    let search = {
        let c = CapturedHistory::new(c.path.clone());
        let env = env.clone();
        let d = d.clone();
        let context = context.clone();
        let caller = caller.clone();
        let service = ServiceClient::new(&c.path, &env, &std::env::current_exe()?);
        move |q: &str, _: bool| -> Result<Vec<String>> {
        let mut r = ServiceRequest::new(Operation::Search);
        r.query = Some(q.into());
        r.directory = Some(d.clone());
        r.limit = Some(100);
        r.session = caller.session.clone();
        r.histfile = caller.histfile.clone();
        r.login_shell = caller.shell.clone();
        let hits = match service
            .send(&r, None, Duration::from_secs(600))
            .filter(|r| r.ok)
            .and_then(|r| r.hits)
        {
            Some(h) => h,
            None => {
                let native = caller.native_env();
                let entries = corpus(&c, caller.home(), &native, context.project_root.as_deref(), context.kind(), now())?;
                let signals = search_signals(&c, Some(context.clone()), &native)?;
                lexical_search(q, &entries, 100, &SearchFilter::default(), &signals, now())?
            }
        };
        Ok(hits.into_iter().map(|h| h.key()).collect())
        }
    };
    let file_search = {
        let cwd = d.clone();
        let home = home.to_owned();
        let service = ServiceClient::new(&c.path, env, &std::env::current_exe()?);
        let fallback = std::cell::OnceCell::new();
        move |mode: Mode, q: &str, kind: u8, scope: &str, path: Option<&str>| -> Result<FileResults> {
        let mut r = ServiceRequest::new(if mode == Mode::Grep {
            Operation::Grep
        } else {
            Operation::Files
        });
        r.directory = Some(scope.into());
        r.query = Some(q.into());
        r.limit = Some(100);
        r.scope = Some(true);
        r.cwd = Some(cwd.clone());
        r.path = path.map(String::from);
        let response = service
            .send(&r, None, Duration::from_secs(10));
        if let Some(response) = response.filter(|r| r.ok) {
            return Ok(FileResults {
                hits: response.files.unwrap_or_default(),
                complete: !response.partial.unwrap_or(false),
            });
        }
        let local = fallback.get_or_init(|| LocalFileSearch { cwd: cwd.clone().into(), home: home.clone() });
        local.search(mode, q, kind, scope, path)
        }
    };
    let config = SmcliConfig::load(&SmcliConfig::location(home, env)).unwrap_or_default();
    let inline = config.at_completion_inline != Some(false);
    picker.ls_colors = config.at_completion_ls_colors == Some(true);
    if let Some(choice) = crate::picker::run(picker, Some(Box::new(search)), Some(Box::new(file_search)), inline)? {
        match choice {
            Choice::Command {
                command,
                query,
                semantic,
                edit_paths,
            } => {
                let mut r = ServiceRequest::new(Operation::Select);
                let (typed, expanded) = smcli_core::entry::command_parts(&command);
                r.command = Some(typed);
                r.expanded_command = expanded;
                r.query = Some(query.clone());
                r.directory = Some(d.clone());
                r.mode = Some(if semantic { "search" } else { "suggest" }.into());
                if !service
                    .send(&r, None, Duration::from_secs(10))
                    .is_some_and(|r| r.ok)
                {
                    // Ranking persistence must never prevent insertion of the chosen command.
                    let _ = (|| -> Result<()> {
                        let filter = CaptureFilter::new(&SmcliConfig::load(
                            &SmcliConfig::location(home, env),
                        )?)?;
                        let db = c.open_connection()?;
                        c.record_pick(
                            &query.chars().take(1000).collect::<String>(),
                            &command,
                            if semantic { "search" } else { "suggest" },
                            &d,
                            &filter,
                            now(),
                            &db,
                        )?;
                        Ok(())
                    })();
                }
                let typed = crate::picker::history_insert(&command);
                if edit_paths {
                    let shell = env.get("SMCLI_SHELL").map(String::as_str).unwrap_or("bash");
                    let dir = c.last_directory(&command, &user).unwrap_or_else(|| d.clone());
                    if let Some((rewritten, pos)) = smcli_core::parse::path_placeholders(&typed, shell, |path| {
                        std::path::Path::new(&dir).join(path).exists()
                    }) {
                        print!("{}\x1f{}", &rewritten[..pos], &rewritten[pos + 1..]);
                        std::process::exit(11);
                    }
                }
                println!("{typed}");
            }
            Choice::File { hit, mode, query, scope } => {
                if hit.provenance != ["*"] {
                    let r = file_selection_request(scope, hit.path.clone(), mode, query);
                    let _ = service.send(&r, None, Duration::from_secs(2));
                }
                let shell = env.get("SMCLI_SHELL").map(String::as_str).unwrap_or("bash");
                let before = env.get("SMCLI_LBUFFER");
                if !before.is_some_and(|b| crate::shell_quoting::insertable(b, shell)) {
                    eprintln!(
                        "smcli: not inserting a path inside an open quote or substitution, or after here-document syntax; finish the command first{}.",
                        if before.is_none() { " (or reload smcli's shell-init)" } else { "" }
                    );
                    std::process::exit(1);
                }
                println!("{}", crate::shell_quoting::quote(&hit.insert, shell));
                std::process::exit(10);
            }
        }
    } else {
        std::process::exit(1);
    }
    Ok(())
}

fn file_selection_request(scope: String, path: String, mode: Mode, query: String) -> ServiceRequest {
    let mut request = ServiceRequest::new(Operation::Select);
    request.directory = Some(scope);
    request.path = Some(path);
    request.mode = Some(mode.as_str().into());
    request.query = Some(query);
    request.scope = Some(true);
    request
}

struct LocalFileSearch {
    cwd: PathBuf,
    home: PathBuf,
}

impl LocalFileSearch {
    fn search(&self, mode: Mode, query: &str, kind: u8, scope: &str, path: Option<&str>) -> Result<FileResults> {
        let scope = Path::new(scope).canonicalize()?;
        let scope_str = scope.to_string_lossy().into_owned();
        let home_str = self.home.to_string_lossy().into_owned();
        let cwd_str = self.cwd.to_string_lossy().into_owned();
        let mut hits = Vec::new();
        match mode {
            Mode::Files => {
                let mut entries = Vec::new();
                let mut budget = 10_000;
                self.walk(&scope, 0, &mut budget, &mut |p, is_dir| {
                    let rel = p.strip_prefix(&scope).unwrap().to_string_lossy().into_owned();
                    if (query.is_empty() || rel.to_lowercase().contains(&query.to_lowercase()))
                        && (kind == 0 || is_dir == (kind == 1))
                    {
                        entries.push((rel, is_dir));
                    }
                });
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                entries.truncate(100);
                for (rel, is_dir) in entries {
                    let rel = if is_dir { format!("{rel}/") } else { rel };
                    hits.push(self.file_hit(&rel, &scope_str, &cwd_str, &home_str, "files"));
                }
            }
            Mode::Grep => {
                let target = path.map(|p| scope.join(p));
                let sources: Vec<PathBuf> = if let Some(target) = target {
                    vec![target]
                } else {
                    let mut files = Vec::new();
                    let mut budget = 10_000;
                    self.walk(&scope, 0, &mut budget, &mut |p, is_dir| {
                        if !is_dir { files.push(p.to_path_buf()); }
                    });
                    files
                };
                for file in sources {
                    let rel = file.strip_prefix(&scope).unwrap().to_string_lossy().into_owned();
                    let text = match std::fs::read_to_string(&file) {
                        Ok(t) => t,
                        Err(_) => continue,
                    };
                    for (i, line) in text.lines().enumerate() {
                        if let Some(pos) = line.to_lowercase().find(&query.to_lowercase()) {
                            let mut hit = self.file_hit(&rel, &scope_str, &cwd_str, &home_str, "grep");
                            hit.line = Some((i + 1) as i64);
                            hit.column = Some((pos + 1) as i64);
                            hit.text = Some(line.into());
                            hits.push(hit);
                            if hits.len() >= 100 { break; }
                        }
                    }
                    if hits.len() >= 100 { break; }
                }
            }
            _ => {}
        }
        Ok(FileResults { hits, complete: true })
    }

    fn walk(&self, dir: &Path, depth: usize, budget: &mut usize, on_entry: &mut dyn FnMut(&Path, bool)) {
        if depth >= 32 || *budget == 0 { return; }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries {
            if *budget == 0 { break; }
            *budget -= 1;
            let Ok(e) = entry else { continue };
            let path = e.path();
            if path.file_name().is_some_and(|n| n == ".git") { continue; }
            let Ok(file_type) = e.file_type() else { continue };
            if file_type.is_dir() {
                on_entry(&path, true);
                self.walk(&path, depth + 1, budget, on_entry);
            } else if file_type.is_file() {
                on_entry(&path, false);
            }
        }
    }

    fn file_hit(&self, path: &str, scope: &str, cwd: &str, home: &str, provenance: &str) -> FileHit {
        FileHit {
            insert: relative_insert(path, scope, cwd, Some(home)),
            path: path.into(),
            provenance: vec![provenance.into()],
            git: None,
            line: None,
            column: None,
            text: None,
            definition: None,
        }
    }
}

#[derive(Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexHealth {
    embedder: String,
    path: String,
    built: bool,
    indexed: usize,
    commands: usize,
    missing: usize,
    stale: usize,
    bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_updated: Option<String>,
    other_indexes: Vec<String>,
    problems: Vec<String>,
    healthy: bool,
}

/// Stops a running service so the model it holds is freed; never starts one.
pub(crate) fn stop_service(service: &ServiceClient) -> bool {
    service
        .send(&ServiceRequest::new(Operation::Shutdown), Some(false), Duration::from_secs(10))
        .is_some_and(|r| r.ok)
}

/// History and command-example indexes next to the database; `prune` keeps only the current model's.
fn indexes(captured: &CapturedHistory, home: &Path, env: &HashMap<String, String>, prune: bool, json: bool) -> Result<()> {
    let model = configured_model(home, env)?;
    let directory = index_directory(captured);
    let current = smcli_search::index_name(&model);
    // The command-completion index (bundled sources in priority order); absent tldr data just means it is not current.
    let command_current = smcli_search::commands::bundled_sources().map(|sources| {
        let refs = smcli_search::commands::source_refs(&sources);
        smcli_search::commands::CommandIndex::reader(directory.clone(), model.clone(), &refs).index_name()
    });
    let mut current_names = vec![current.clone(), format!("{current}-typed")];
    current_names.extend(command_current);
    let list = smcli_search::index::list_indexes(&directory, &current_names, prune)?;
    if json {
        return print_json(&list);
    }
    println!("Index directory: {}", directory.display());
    if list.is_empty() {
        println!("No indexes yet; the next search or `smcli reindex` builds {current}.");
    }
    for info in &list {
        let state = if info.current { "current" } else if prune { "deleted" } else { "unused" };
        println!("  {:<8} {:>12} bytes  {}", state, info.bytes, info.name);
    }
    if !prune && list.iter().any(|i| !i.current) {
        println!("Unused indexes belong to other models; `smcli indexes --prune` deletes them.");
    }
    Ok(())
}

fn index_health(
    captured: &CapturedHistory,
    home: &Path,
    env: &HashMap<String, String>,
    corpus: &HashMap<String, SearchEntry>,
) -> IndexHealth {
    let model = match configured_model(home, env) {
        Ok(model) => model,
        Err(error) => return IndexHealth {
            path: format!("{}.search", captured.path.display()),
            commands: corpus.len(),
            problems: vec![format!("{error:#}")],
            ..Default::default()
        },
    };
    let report = smcli_search::index::inspect(&index_directory(captured), &model, corpus);
    IndexHealth {
        embedder: format!("{} ({} dimensions), {}", smcli_search::Potion::NAME, model.dimension, model.identity),
        path: report.path,
        built: report.built,
        indexed: report.indexed,
        commands: report.commands,
        missing: report.missing,
        stale: report.stale,
        bytes: report.bytes,
        last_updated: report.last_updated,
        other_indexes: report.other_indexes,
        problems: report.problems,
        healthy: report.healthy,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn files_filter_uses_recent_captured_paths() {
        let root = std::env::temp_dir().join(format!("smcli-files-kind-{}", std::process::id()));
        std::fs::create_dir_all(root.join("folder")).unwrap();
        std::fs::write(root.join("file.txt"), "").unwrap();
        let history = super::CapturedHistory::new(root.join("history.db"));
        let env = std::collections::HashMap::from([("SMCLI_LBUFFER".into(), "echo hi; tool @".into())]);
        assert_eq!(super::files_kind(&history, &env), 0);
        let db = history.open_connection().unwrap();
        for i in 0..21 {
            let command = if i == 0 { "tool folder folder folder" } else { "tool file.txt" };
            let mut run = smcli_core::history::NewRun::new(command, "bash", root.to_str().unwrap(), 0);
            run.started_at = Some(1_700_000_000.0 + i as f64);
            history.append(&run, "", Some(&db)).unwrap();
        }
        assert_eq!(super::files_kind(&history, &env), 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn files_start_filter() {
        for command in ["cd", "pushd", "popd", "z", "rmdir", "mkdir"] {
            assert!(super::is_directory_command(command));
        }
        assert!(!super::is_directory_command("cat"));
        assert_eq!(super::files_majority([]), 0);
        assert_eq!(super::files_majority([(2, 2)]), 0);
        assert_eq!(super::files_majority([(2, 1)]), 1);
        assert_eq!(super::files_majority([(1, 2)]), 2);
        assert_eq!(super::files_majority(std::iter::repeat_n((1, 0), 20).chain([(0, 100)])), 1);
    }

    use super::*;
    use smcli_search::setup::search_engine;

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
        let manifest = include_str!("../Cargo.toml");
        let mut dependencies = false;
        for line in manifest.lines().map(str::trim) {
            if line.starts_with('[') { dependencies = line == "[dependencies]"; }
            else if dependencies && let Some((name, _)) = line.split_once('=') {
                assert!(!["rusqlite", "taladb"].contains(&name.trim()), "{name} must not be a production dependency");
            }
        }
    }

    #[test]
    fn health_json_preserves_cli_fields_and_model_errors() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("smcli-health-output-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir)?;
        let captured = CapturedHistory::new(dir.join("history.db"));
        let env = HashMap::from([("SMCLI_CONFIG".into(), dir.join("config.json").to_string_lossy().into_owned())]);
        let corpus = HashMap::from([("git status".into(), SearchEntry::new("git status", 1, "native"))]);
        let report = serde_json::to_value(index_health(&captured, &dir, &env, &corpus))?;
        assert_eq!(report["healthy"], true);
        assert_eq!(report["built"], false);
        assert_eq!(report["commands"], 1);
        assert_eq!(report["missing"], 1);
        assert_eq!(report["indexed"], 0);
        assert_eq!(report["stale"], 0);
        assert_eq!(report["bytes"], 0);
        assert_eq!(report["otherIndexes"], serde_json::json!([]));
        assert_eq!(report["problems"], serde_json::json!([]));
        assert!(report.get("lastUpdated").is_none());
        assert!(report["embedder"].as_str().unwrap().contains(smcli_search::Potion::NAME));
        assert!(report["path"].as_str().unwrap().starts_with(index_directory(&captured).to_str().unwrap()));
        assert!(!index_directory(&captured).exists());
        std::fs::write(dir.join("config.json"), r#"{"potionPath":"/missing/smcli-model"}"#)?;
        let failed = serde_json::to_value(index_health(&captured, &dir, &env, &corpus))?;
        assert_eq!(failed["healthy"], false);
        assert_eq!(failed["commands"], 1);
        assert_eq!(failed["problems"].as_array().unwrap().len(), 1);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    fn tldr_entries() -> Vec<smcli_search::Entry> {
        vec![
            smcli_search::Entry { program: "git".into(), description: "Show status".into(), template: "git status".into() },
            smcli_search::Entry { program: "git".into(), description: "Commit changes".into(), template: "git commit @".into() },
        ]
    }

    fn tldr_service(mode: &str) -> (PathBuf, CapturedHistory, ServiceClient) {
        use std::os::unix::fs::PermissionsExt;
        let dir = Path::new("/tmp").join(format!("smcli-a4-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let captured = CapturedHistory::new(dir.join("history.db"));
        let env = HashMap::from([
            ("HOME".into(), dir.to_string_lossy().into_owned()),
            ("SMCLI_RUNTIME_DIR".into(), dir.to_string_lossy().into_owned()),
            ("SMCLI_SERVICE".into(), mode.into()),
        ]);
        let service = ServiceClient::new(&captured.path, &env, Path::new("/missing/smcli"));
        (dir, captured, service)
    }

    #[test]
    fn tldr_ranker_falls_back_to_words_without_service() -> Result<()> {
        let (dir, _, service) = tldr_service("0");
        let mut rank = tldr_ranker(Some(service), "expected".into(), tldr_entries(), vec![vec![0.0, 1.0], vec![1.0, 0.0]], vec!["git".into()]);
        assert_eq!(rank("status", false)?, ["0", "1"]);
        assert_eq!(rank("commit", true)?, ["1", "0"]);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn tldr_ranker_rejects_vectors_for_another_model() -> Result<()> {
        use std::io::{BufRead, BufReader, Write};
        for (model, expected) in [("other", ["0", "1"]), ("expected", ["1", "0"])] {
            let (dir, _, service) = tldr_service("connect");
            let listener = std::os::unix::net::UnixListener::bind(service.socket())?;
            let worker = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
                let request: ServiceRequest = serde_json::from_str(&line).unwrap();
                assert_eq!(request.op, Operation::Embed);
                let reply = serde_json::json!({"v": smcli_core::service_protocol::VERSION, "ok": true,
                    "embedding": [1.0, 0.0], "model": model});
                writeln!(stream, "{reply}").unwrap();
            });
            let mut rank = tldr_ranker(Some(service), "expected".into(), tldr_entries(), vec![vec![0.0, 1.0], vec![1.0, 0.0]], vec!["git".into()]);
            assert_eq!(rank("status", false)?, expected);
            worker.join().unwrap();
            std::fs::remove_dir_all(dir)?;
        }
        Ok(())
    }

    #[test]
    fn tldr_ranker_latches_after_stalled_service() -> Result<()> {
        use std::time::Instant;
        let (dir, _, service) = tldr_service("connect");
        let listener = std::os::unix::net::UnixListener::bind(service.socket())?;
        let mut rank = tldr_ranker(Some(service), "expected".into(), tldr_entries(), vec![vec![1.0], vec![1.0]], vec!["git".into()]);
        let start = Instant::now();
        assert_eq!(rank("", false)?, ["0", "1"]);
        assert!(start.elapsed() < Duration::from_millis(50));
        listener.set_nonblocking(true)?;
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        listener.set_nonblocking(false)?;
        let start = Instant::now();
        assert_eq!(rank("status", false)?, ["0", "1"]);
        assert!(start.elapsed() >= Duration::from_millis(1800));
        assert!(start.elapsed() < Duration::from_secs(3));
        let (first, _) = listener.accept()?;
        for _ in 0..5 {
            let start = Instant::now();
            assert_eq!(rank("commit", false)?, ["1", "0"]);
            assert!(start.elapsed() < Duration::from_millis(50));
        }
        listener.set_nonblocking(true)?;
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        drop(first);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn pick_tldr_prepares_while_index_is_unpublished() -> Result<()> {
        use std::io::{BufRead, BufReader, Write};
        let (dir, captured, service) = tldr_service("connect");
        let listener = std::os::unix::net::UnixListener::bind(service.socket())?;
        let index = smcli_search::commands::CommandIndex::reader(index_directory(&captured),
            smcli_search::ModelId { identity: "test".into(), dimension: 2 }, &[]);
        std::fs::create_dir_all(&index.directory)?;
        let path = index.path();
        let pending = path.with_extension("building");
        std::fs::write(&pending, b"unpublished build")?;
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
            let request: ServiceRequest = serde_json::from_str(&line).unwrap();
            assert_eq!(request.op, Operation::Embed);
            assert_eq!(request.mode.as_deref(), Some("commands"));
            assert_eq!(request.query.as_deref(), Some(""));
            std::fs::rename(pending, path).unwrap();
            let reply = serde_json::json!({"v": smcli_core::service_protocol::VERSION, "ok": true});
            writeln!(stream, "{reply}").unwrap();
        });
        assert!(prepare_tldr(&index, &service, None)?);
        worker.join().unwrap();
        assert!(index.path().exists());
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn pick_tldr_skips_prepare_when_index_has_program() -> Result<()> {
        use smcli_core::command_source::CommandExample;
        let (dir, captured, service) = tldr_service("connect");
        let listener = std::os::unix::net::UnixListener::bind(service.socket())?;
        let index = smcli_search::commands::CommandIndex::new(index_directory(&captured), smcli_search::Potion::bundled()?, &[]);
        index.add("git", &[CommandExample { program: "git".into(), description: "Show status".into(), template: "git status".into(), source: "test".into() }])?;
        let reader = smcli_search::commands::CommandIndex::reader(index_directory(&captured), index.model.clone(), &[]);
        assert!(prepare_tldr(&reader, &service, Some("git"))?);
        assert!(prepare_tldr(&reader, &service, None)?);
        listener.set_nonblocking(true)?;
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
    #[test]
    fn fallback_walker_skips_directory_symlink_cycles_within_budget() -> Result<()> {
        let root = std::env::temp_dir().join(format!("smcli-walk-cycle-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("nested"))?;
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&root, root.join("nested/back-one"))?;
            std::os::unix::fs::symlink(&root, root.join("nested/back-two"))?;
        }
        let search = LocalFileSearch { cwd: root.clone(), home: root.clone() };
        let result = search.search(Mode::Files, "", 0, root.to_str().unwrap(), None)?;
        assert!(result.hits.iter().any(|hit| hit.path == "nested/"));
        assert!(!result.hits.iter().any(|hit| hit.path.contains("back-one") || hit.path.contains("back-two")));
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn fallback_walker_obeys_entry_budget() -> Result<()> {
        let root = std::env::temp_dir().join(format!("smcli-walk-budget-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        for name in ["one", "two", "three"] { std::fs::write(root.join(name), "")?; }
        let search = LocalFileSearch { cwd: root.clone(), home: root.clone() };
        let mut budget = 1;
        let mut seen = Vec::new();
        search.walk(&root, 0, &mut budget, &mut |path, _| seen.push(path.to_path_buf()));
        assert_eq!(seen.len(), 1);
        assert_eq!(budget, 0);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn fallback_walker_obeys_depth_limit() -> Result<()> {
        let root = std::env::temp_dir().join(format!("smcli-walk-depth-{}", uuid::Uuid::new_v4()));
        let mut leaf = root.clone();
        for _ in 0..33 { leaf.push("d"); }
        std::fs::create_dir_all(&leaf)?;
        let search = LocalFileSearch { cwd: root.clone(), home: root.clone() };
        let mut budget = 10_000;
        let mut deepest = 0;
        search.walk(&root, 0, &mut budget, &mut |path, _| {
            deepest = deepest.max(path.strip_prefix(&root).unwrap().components().count());
        });
        assert_eq!(deepest, 32);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn fallback_folder_picker_lists_empty_directories() -> Result<()> {
        let root = std::env::temp_dir().join(format!("smcli-walk-empty-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("empty"))?;
        let search = LocalFileSearch { cwd: root.clone(), home: root.clone() };
        std::fs::write(root.join("visible.txt"), "file")?;
        let result = search.search(Mode::Files, "", 1, root.to_str().unwrap(), None)?;
        assert!(result.hits.iter().any(|hit| hit.path == "empty/"));
        let mixed = search.search(Mode::Files, "", 0, root.to_str().unwrap(), None)?;
        assert!(mixed.hits.iter().any(|hit| hit.path == "visible.txt"));
        assert!(mixed.hits.iter().any(|hit| hit.path == "empty/"));
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn scoped_file_selection_request_sets_scope() {
        let request = file_selection_request("/repo/sub".into(), "needle.rs".into(), Mode::Files, "needle".into());
        assert_eq!(request.scope, Some(true));
        assert_eq!(request.directory.as_deref(), Some("/repo/sub"));
        assert_eq!(request.path.as_deref(), Some("needle.rs"));
    }

    #[test]
    fn parse_flags_and_times() {
        assert!(Cli::try_parse_from(["smcli", "search", "git", "--flag", "--all"]).is_ok());
        assert!(Cli::try_parse_from(["smcli", "complete", "--word", "@foo", "--command", "cd", "--limit", "5"]).is_ok());
        assert!(Cli::try_parse_from(["smcli", "suggest", "--limit", "0"]).is_err());
        assert_eq!(
            time("2024-01-01").unwrap(),
            parse_iso8601("2024-01-01T00:00:00Z").unwrap()
        );
        assert!((now() - time("30m").unwrap() - 1800.0).abs() < 1.0);
        assert!(epoch("NaN").is_err());
        assert!(time("2024-02-31").is_err());
        assert!(time("2024-01-01T00:00:00♥").is_err());
    }

    #[test]
    fn tldr_pick_arguments_and_lbuffer_context() {
        assert!(Cli::try_parse_from(["smcli", "pick", "--tldr"]).is_ok());
        for flag in ["--suggest", "--files", "--grep"] {
            assert!(Cli::try_parse_from(["smcli", "pick", "--tldr", flag]).is_err());
        }
        assert_eq!(tldr_context("sudo env git rebase "), TldrContext {
            leading: "sudo env ".into(),
            program: Some("git".into()),
            prefix_words: vec!["git".into(), "rebase".into()],
        });
        assert_eq!(tldr_context("command /usr/bin/bun "), TldrContext {
            leading: "command ".into(),
            program: Some("bun".into()),
            prefix_words: vec!["bun".into()],
        });
        assert_eq!(tldr_context("  "), TldrContext { leading: String::new(), program: None, prefix_words: Vec::new() });
    }

    #[test]
    fn prune_waits_for_search_with_pre_prune_corpus() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("smcli-prune-race-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir)?;
        let captured = CapturedHistory::new(dir.join("history.db"));
        let db = captured.open_connection()?;
        db.execute_batch("INSERT INTO sessions(id,user,host,shell) VALUES('s','test','unknown','zsh');
            INSERT INTO commands(session,started_at,directory,command,exit_code) VALUES('s',1,'/tmp','secret-cmd',0);")?;
        let engine = search_engine(&captured, &dir, &HashMap::new())?;
        let lock = engine.lock_index(1)?.unwrap();
        let entries = HashMap::from([("secret-cmd".into(), SearchEntry::new("secret-cmd", 1, "captured"))]);
        let c = captured.clone();
        let (started, ready) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let filter = CaptureFilter::new(&SmcliConfig { history_filter: Some(vec!["^secret".into()]), ..Default::default() })?;
            started.send(()).unwrap();
            prune(&c, &filter, false)
        });
        ready.recv()?;
        engine.search_with_evidence_using_lock("secret", &entries, 1, &SearchFilter::default(), None,
            &smcli_search::corpus::SearchSignals::default(), now(), None, Some(&lock))?;
        assert_eq!(db.query_row("SELECT count(*) FROM commands", [], |r| r.get::<_, i64>(0))?, 1);
        drop(lock);
        worker.join().unwrap()?;
        assert!(!std::fs::read_dir(&engine.directory)?.any(|e| e.unwrap().file_name().to_string_lossy().starts_with("rs-commands-v")));
        drop(db);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

}
