use anyhow::{Result, bail, ensure};
use clap::{Args, Parser, Subcommand};
use smcli_core::{
    config::{CaptureFilter, SmcliConfig, home_dir},
    context::ProjectContext,
    history::{CapturedHistory, NewRun, parse_pipestatus, user_name},
    ranking::RankingContext,
    service_client::ServiceClient,
    service_location::relative_insert,
    service_protocol::{Operation, SearchEntry, SearchFilter, ServiceRequest},
    time::{format_iso8601, now, parse_iso8601},
};
use smcli_search::setup::{corpus, search_engine, search_signals, working_directory};
use std::{
    collections::HashMap,
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
        Command::Init(args) => crate::initialize::run(args, &config_path, &service)?,
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
            let dirs_only = matches!(command.as_deref(), Some("cd" | "pushd" | "z"));
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
                    let context = RankingContext::new(
                        &ProjectContext::discover(&working_directory(&env)),
                        env.get("SMCLI_SESSION").cloned(),
                        &env,
                    );
                    let corpus = corpus(
                        &captured,
                        &home,
                        &env,
                        context.project_root.as_deref(),
                        now(),
                    )?;
                    search_engine(&captured, &home, &env)?.search(
                        &query,
                        &corpus,
                        a.limit,
                        &filter,
                        None,
                        &search_signals(&captured, Some(context), &env)?,
                        now(),
                    )?
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
                None => captured.suggestions(
                    &user_name(),
                    &d,
                    &prefix,
                    limit,
                    now(),
                    Some(&RankingContext::new(
                        &ProjectContext::discover(&d),
                        request.session,
                        &env,
                    )),
                )?,
            };
            if json {
                print_json(&suggestions)?;
            } else {
                for s in suggestions {
                    println!("{}", crate::picker::history_label(&s.key()));
                }
            }
        }
        Command::Reindex => println!(
            "{}Reindexed {} commands.",
            if stop_service(&service) { "Stopped smcli-service to free its model. " } else { "" },
            search_engine(&captured, &home, &env)?.reindex(&corpus(
                &captured,
                &home,
                &env,
                None,
                now()
            )?)?
        ),
        Command::Indexes { prune, json } => indexes(&captured, &home, &env, prune, json)?,
        Command::Health { json } => {
            let corpus = corpus(&captured, &home, &env, None, now())?;
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
        Command::Prune { dry_run } => prune(&captured, &home, &env, &filter()?, dry_run)?,
        Command::Import {
            atuin,
            replace,
            source,
        } => import(&captured, &home, &env, atuin, replace, source)?,
        Command::Pick {
            suggest,
            files,
            grep,
            query,
        } => {
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

fn records(c: &CapturedHistory) -> Result<Vec<serde_json::Value>> {
    if !c.path.exists() {
        return Ok(vec![]);
    }
    let db = c.open_connection()?;
    let mut statement=db.prepare("SELECT c.started_at,c.duration_ms,s.user,s.host,s.id,s.shell,c.directory,c.command,c.exit_code,COUNT(*) OVER (PARTITION BY c.command,c.expanded_command ORDER BY c.started_at,c.id),c.uuid,c.author,c.intent,c.pipestatus,c.flags,c.expanded_command FROM commands c JOIN sessions s ON s.id=c.session WHERE c.deleted_at IS NULL ORDER BY c.started_at,c.id")?;
    let rows=statement.query_map([],|r| {
        let command:String=r.get(7)?;
        let mut value=serde_json::json!({"timestamp":format_iso8601(r.get(0)?,true),"user":r.get::<_,String>(2)?,"host":r.get::<_,String>(3)?,"session":r.get::<_,String>(4)?,"shell":r.get::<_,String>(5)?,"directory":r.get::<_,String>(6)?,"command":command,"exitCode":r.get::<_,i64>(8)?,"runCount":r.get::<_,i64>(9)?});
        let expanded: Option<String> = r.get(15)?;
        if let Some(breakdown)=smcli_core::parse::CommandBreakdown::parse(expanded.as_deref().unwrap_or(&command)) {value["breakdown"]=serde_json::to_value(breakdown).unwrap();}
        for (name,index) in [("id",10),("author",11),("intent",12),("expandedCommand",15)] {if let Some(v)=r.get::<_,Option<String>>(index)? {value[name]=v.into();}}
        if let Some(v)=r.get::<_,Option<i64>>(1)? {value["durationMs"]=v.into();}
        if let Some(v)=r.get::<_,Option<String>>(13)? {value["pipestatus"]=serde_json::json!(v.split_whitespace().filter_map(|s|s.parse::<i64>().ok()).collect::<Vec<_>>());}
        if let Some(v)=r.get::<_,Option<String>>(14)?.filter(|v| !v.trim().is_empty()) {value["flags"]=serde_json::json!(v.split_whitespace().collect::<Vec<_>>());}
        Ok(value)
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}
fn history(
    c: &CapturedHistory,
    home: &Path,
    env: &HashMap<String, String>,
    json: bool,
) -> Result<()> {
    let records = records(c)?;
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
fn prune(
    c: &CapturedHistory,
    home: &Path,
    env: &HashMap<String, String>,
    filter: &CaptureFilter,
    dry: bool,
) -> Result<()> {
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
    let mut db = c.open_connection()?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut commands = vec![];
    let mut ids = vec![];
    {
        let mut s =
            tx.prepare("SELECT id,command,directory,expanded_command FROM commands ORDER BY started_at,id")?;
        for row in s.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })? {
            let (id, command, d, expanded) = row?;
            if !filter.allows(&command, &d) || expanded.as_deref().is_some_and(|e| !filter.allows(e, &d)) {
                ids.push(id);
                let key = smcli_core::entry::command_key(&command, expanded.as_deref());
                if !commands.contains(&key) {
                    commands.push(key);
                }
            }
        }
    }
    let mut picks = vec![];
    {
        let mut s = tx.prepare(
            "SELECT id,command,query,directory FROM command_selections ORDER BY selected_at,id",
        )?;
        for row in s.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })? {
            let (id, command, q, d) = row?;
            // Only a rejected query removes a selection on its own; selections of a pruned command go with it.
            let (typed, expanded) = smcli_core::entry::command_parts(&command);
            if !filter.allows_run(&typed, expanded.as_deref(), &d) || (!q.is_empty() && !filter.allows(&q, &d)) {
                picks.push(id);
            }
        }
    }
    let mut selection_removed = picks.len();
    for command in &commands {
        selection_removed += tx.query_row(
            "SELECT count(*) FROM command_selections WHERE command = ?",
            [command],
            |r| r.get::<_, i64>(0),
        )? as usize;
    }
    if dry {
        for command in &commands {
            println!("{}", smcli_core::entry::command_parts(command).0);
        }
        tx.rollback()?;
        println!("{} commands and {} selections would be removed.", ids.len(), selection_removed);
    } else {
        let removed = ids.len();
        for id in ids {
            tx.execute("DELETE FROM commands WHERE id=?", [id])?;
        }
        for id in picks {
            tx.execute("DELETE FROM command_selections WHERE id=?", [id])?;
        }
        for command in &commands {
            tx.execute("DELETE FROM command_selections WHERE command=?", [command])?;
        }
        tx.commit()?;
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
        println!("Removed {removed} commands and {selection_removed} selections.");
        if removed > 0 {
            // The index still holds the pruned text until rebuilt.
            if let Err(e) = search_engine(c, home, env).and_then(|e| e.reindex(&corpus(c, home, env, None, now())?)) {
                eprintln!("smcli: search index not rebuilt ({e:#}); run `smcli reindex`.");
            }
        }
    }
    Ok(())
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
    ensure!(
        source != c.path
            && std::fs::canonicalize(&source).ok()
                != std::fs::canonicalize(&c.path)
                    .ok()
                    .filter(|_| c.path.exists()),
        "Cannot import history into itself"
    );
    let capture_filter = CaptureFilter::new(&SmcliConfig::load(&SmcliConfig::location(home, env))?)?;
    let mut db = c.open_connection()?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut count = 0;
    if atuin {
        let from = rusqlite::Connection::open_with_flags(
            &source,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let columns = from
            .prepare("PRAGMA table_info(history)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ensure!(
            [
                "id",
                "timestamp",
                "duration",
                "exit",
                "command",
                "cwd",
                "session",
                "hostname"
            ]
            .iter()
            .all(|s| columns.iter().any(|c| c == s)),
            "Not an Atuin history database"
        );
        let optional = ["deleted_at", "shell", "author", "intent"].map(|s| {
            if columns.iter().any(|c| c == s) {
                s
            } else {
                "NULL"
            }
        });
        let sql = format!(
            "SELECT id,timestamp,duration,exit,command,cwd,session,hostname,{} FROM history ORDER BY timestamp",
            optional.join(",")
        );
        let mut stmt = from.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        if replace {
            tx.execute_batch(
                "DELETE FROM commands; DELETE FROM sessions; DELETE FROM command_selections;",
            )?;
        }
        while let Some(r) = rows.next()? {
            let hostname: String = r.get(7)?;
            let (host, user) = hostname.split_once(':').unwrap_or((&hostname, "unknown"));
            let session = format!("atuin-{}", r.get::<_, String>(6)?);
            let shell = r
                .get::<_, Option<String>>(9)?
                .filter(|s| !s.is_empty())
                .unwrap_or("unknown".into());
            tx.execute(
                "INSERT OR IGNORE INTO sessions(id,user,host,shell) VALUES(?,?,?,?)",
                rusqlite::params![session, user, host, shell],
            )?;
            let command: String = r.get(4)?;
            let flags = smcli_core::parse::command_flags(&command).join(" ");
            let duration: i64 = r.get(2)?;
            count+=tx.execute("INSERT OR IGNORE INTO commands(session,started_at,duration_ms,directory,command,exit_code,uuid,deleted_at,author,intent,flags) VALUES(?,?,?,?,?,?,?,?,?,?,?)",rusqlite::params![session,r.get::<_,i64>(1)? as f64/1e9,if duration<0{None}else{Some(duration/1_000_000)},r.get::<_,String>(5)?,command,r.get::<_,i64>(3)?,r.get::<_,String>(0)?,r.get::<_,Option<i64>>(8)?.map(|n|n as f64/1e9),r.get::<_,Option<String>>(10)?,r.get::<_,Option<String>>(11)?,flags])?;
        }
    } else {
        for line in std::fs::read_to_string(&source)?
            .lines()
            .filter(|l| !l.is_empty())
        {
            let r: serde_json::Value = serde_json::from_str(line)?;
            let text = |key: &str| -> Result<&str> {
                r[key]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing {key} in legacy record"))
            };
            let user = text("user")?;
            let shell = text("shell")?;
            let session = format!("imported-{user}-{shell}");
            let command = text("command")?;
            let expanded = match r.get("expandedCommand") {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(text)) => Some(text.as_str()),
                _ => bail!("Invalid expanded command."),
            };
            ensure!(expanded.is_none_or(|e| smcli_core::parse::safe_expansion(e) && e != command), "Invalid expanded command.");
            let stamp = parse_iso8601(text("timestamp")?)
                .ok_or_else(|| anyhow::anyhow!("Invalid timestamp"))?;
            let directory = text("directory")?;
            let exit_code = r["exitCode"].as_i64().ok_or_else(|| anyhow::anyhow!("Missing exitCode"))?;
            if expanded.is_some() && !capture_filter.allows_run(command, expanded, directory) { continue; }
            tx.execute(
                "INSERT OR IGNORE INTO sessions(id,user,host,shell) VALUES(?,?,'unknown',?)",
                rusqlite::params![session, user, shell],
            )?;
            // Repeat imports must not double run counts: this import-only natural key skips a stored record.
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM commands WHERE session = ? AND started_at = ? AND command = ? AND directory = ? AND exit_code = ? AND expanded_command IS ?)",
                rusqlite::params![session, stamp, command, directory, exit_code, expanded],
                |r| r.get(0),
            )?;
            if !exists {
                tx.execute("INSERT INTO commands(session,started_at,directory,command,exit_code,flags,expanded_command) VALUES(?,?,?,?,?,?,?)",rusqlite::params![session,stamp,directory,command,exit_code,smcli_core::parse::command_flags(expanded.unwrap_or(command)).join(" "),expanded])?;
                count += 1;
            }
        }
    }
    tx.commit()?;
    if replace {
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
    }
    println!("Imported {count} records into {}.", c.path.display());
    Ok(())
}
fn command_directory(c: &CapturedHistory, user: &str, command_key: &str) -> Option<String> {
    if !c.path.exists() {
        return None;
    }
    let db = c.open_connection().ok()?;
    let mut stmt = db
        .prepare(
            "SELECT c.directory FROM commands c JOIN sessions s ON s.id = c.session
             WHERE smcli_key(c.command, c.expanded_command) = ? AND s.user = ?
             ORDER BY c.started_at DESC, c.id DESC LIMIT 1",
        )
        .ok()?;
    stmt.query_row(rusqlite::params![command_key, user], |r| r.get::<_, String>(0))
        .ok()
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
    let d = working_directory(env);
    let context = RankingContext::new(
        &ProjectContext::discover(&d),
        env.get("SMCLI_SESSION").cloned(),
        env,
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
        None => c.picker_recents(&user, 50)?,
    };
    let native = smcli_search::native::history_files(home, env)?
        .iter()
        .filter_map(|f| f.read().ok())
        .flat_map(|s| s.lines().map(String::from).collect::<Vec<_>>())
        .collect();
    let mut picker = if suggest {
        let mut p = Picker::new(suggestions, vec![], vec![], query);
        p.title = Some(format!("Suggested for {d}"));
        p
    } else {
        Picker::new(
            vec![],
            recent,
            native,
            // Ctrl-T's query is the whole command line, so files mode starts empty unless `@` completion names a word.
            if files { env.get("SMCLI_FILES_QUERY").cloned().unwrap_or_default() } else if grep { String::new() } else { query },
        )
    };
    if files {
        picker.kind = if env.contains_key("SMCLI_FILES_DIRS") { 1 } else { 0 };
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
        let home = home.to_owned();
        let env = env.clone();
        let d = d.clone();
        let context = context.clone();
        let service = ServiceClient::new(&c.path, &env, &std::env::current_exe()?);
        // The in-process fallback is built on first use and reused for the rest of the picker session.
        let fallback = std::cell::OnceCell::new();
        move |q: &str| -> Result<Vec<String>> {
        let mut r = ServiceRequest::new(Operation::Search);
        r.query = Some(q.into());
        r.directory = Some(d.clone());
        r.limit = Some(100);
        r.session = env.get("SMCLI_SESSION").cloned();
        r.histfile = env.get("HISTFILE").cloned();
        r.login_shell = env.get("SHELL").cloned();
        let hits = match service
            .send(&r, None, Duration::from_secs(600))
            .filter(|r| r.ok)
            .and_then(|r| r.hits)
        {
            Some(h) => h,
            None => {
                let (engine, corpus, signals) = match fallback.get() {
                    Some(built) => built,
                    None => {
                        let built = (
                            search_engine(&c, &home, &env)?,
                            corpus(&c, &home, &env, context.project_root.as_deref(), now())?,
                            search_signals(&c, Some(context.clone()), &env)?,
                        );
                        fallback.get_or_init(|| built)
                    }
                };
                engine.search(q, corpus, 100, &SearchFilter::default(), None, signals, now())?
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
                    let dir = command_directory(c, &user, &command).unwrap_or_else(|| d.clone());
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
                    let mut r = ServiceRequest::new(Operation::Select);
                    r.directory = Some(scope);
                    r.path = Some(hit.path.clone());
                    r.mode = Some(mode.as_str().into());
                    r.query = Some(query);
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
                let mut files = Vec::new();
                self.walk(&scope, &mut |p| {
                    let rel = p.strip_prefix(&scope).unwrap().to_string_lossy().into_owned();
                    if query.is_empty() || rel.to_lowercase().contains(&query.to_lowercase()) {
                        files.push(rel);
                    }
                });
                files.sort();
                files.truncate(100);
                if kind == 1 {
                    let mut seen = std::collections::HashSet::new();
                    for rel in files {
                        if let Some(parent) = Path::new(&rel).parent() {
                            let dir = parent.to_string_lossy().into_owned();
                            if !dir.is_empty() && seen.insert(dir.clone()) {
                                let path = format!("{dir}/");
                                hits.push(self.file_hit(&path, &scope_str, &cwd_str, &home_str, "files"));
                            }
                        }
                    }
                } else {
                    for rel in files {
                        hits.push(self.file_hit(&rel, &scope_str, &cwd_str, &home_str, "files"));
                    }
                }
            }
            Mode::Grep => {
                let target = path.map(|p| scope.join(p));
                let sources: Vec<PathBuf> = if let Some(target) = target {
                    vec![target]
                } else {
                    let mut files = Vec::new();
                    self.walk(&scope, &mut |p| files.push(p.to_path_buf()));
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

    fn walk(&self, dir: &Path, on_file: &mut dyn FnMut(&Path)) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let path = e.path();
            if path.file_name().is_some_and(|n| n == ".git") { continue; }
            if path.is_dir() {
                self.walk(&path, on_file);
            } else if path.is_file() {
                on_file(&path);
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

#[derive(serde::Serialize)]
struct IndexInfo {
    name: String,
    bytes: u64,
    current: bool,
}

/// Every `rs-commands-v*` index next to the history database, grouped by index name; `prune` deletes all but the current model's.
fn indexes(captured: &CapturedHistory, home: &Path, env: &HashMap<String, String>, prune: bool, json: bool) -> Result<()> {
    let engine = search_engine(captured, home, env)?;
    let current = engine.index_name();
    let mut found: std::collections::BTreeMap<String, IndexInfo> = Default::default();
    let entries = match std::fs::read_dir(&engine.directory) {
        Ok(entries) => entries.collect::<std::io::Result<Vec<_>>>()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    for entry in &entries {
        let filename = entry.file_name().to_string_lossy().into_owned();
        // Index names hold no dots, so everything before the first one groups the index with its side files.
        let Some((name, _)) = filename.split_once('.').filter(|_| filename.starts_with("rs-commands-v")) else { continue };
        let info = found.entry(name.to_string()).or_insert_with(|| IndexInfo { name: name.to_string(), bytes: 0, current: name == current });
        info.bytes += entry.metadata()?.len();
        if prune && !info.current {
            std::fs::remove_file(entry.path())?;
        }
    }
    let list: Vec<_> = found.into_values().collect();
    if json {
        return print_json(&list);
    }
    println!("Index directory: {}", engine.directory.display());
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
    let mut report = IndexHealth {
        path: format!("{}.search", captured.path.display()),
        commands: corpus.len(),
        ..Default::default()
    };
    let result = (|| -> Result<()> {
        use std::collections::HashSet;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let engine = search_engine(captured, home, env)?;
        report.embedder = format!(
            "{} ({} dimensions), {}",
            smcli_search::Potion::NAME,
            engine.potion.dimension(),
            engine.potion.identity
        );
        let vector = engine.potion.embed(&["smcli health check".into()]);
        ensure!(
            vector.len() == 1
                && vector[0].len() == engine.potion.dimension()
                && vector[0].iter().all(|x| x.is_finite()),
            "Embedding model returned an invalid vector; rerun `smcli init`."
        );
        let path = engine.directory.join(format!("{}.taladb", engine.index_name()));
        report.path = path.to_string_lossy().into_owned();
        let directory = match std::fs::symlink_metadata(&engine.directory) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                report.missing = corpus.len();
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        ensure!(
            directory.is_dir()
                && directory.uid() == unsafe { libc::getuid() }
                && directory.mode() & 0o077 == 0,
            "Search index must be a private directory owned by you (chmod 700): {}",
            engine.directory.display()
        );
        let mut latest = None;
        let name = engine.index_name();
        for entry in std::fs::read_dir(&engine.directory)? {
            let entry = entry?;
            let filename = entry.file_name().to_string_lossy().into_owned();
            if filename.starts_with("rs-commands-v")
                && filename.ends_with(".taladb")
                && filename != format!("{name}.taladb")
            {
                report.other_indexes.push(filename.clone());
            }
            if filename.starts_with(&format!("{name}.")) {
                let info = std::fs::symlink_metadata(entry.path())?;
                ensure!(
                    info.is_file() && info.uid() == unsafe { libc::getuid() },
                    "Index files must be regular files owned by you: {}",
                    entry.path().display()
                );
                report.bytes += info.len();
                if let Ok(time) = info.modified() {
                    latest = Some(
                        latest.map_or(time, |previous: std::time::SystemTime| previous.max(time)),
                    );
                }
            }
        }
        report.other_indexes.sort();
        report.last_updated = latest
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|t| format_iso8601(t.as_secs_f64(), false));
        if !path.try_exists()? {
            report.missing = corpus.len();
            return Ok(());
        }
        report.built = true;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(engine.directory.join("lock"))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                bail!("Another smcli owns the index; check again when it finishes.")
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
        // Never use HistorySearch::open_index here: it deletes unreadable caches instead of reporting them.
        let db = taladb::open(&path)
            .map_err(|e| anyhow::anyhow!("Index is unreadable ({e}); run `smcli reindex`."))?;
        let rows = db
            .collection("commands")?
            .aggregate(vec![taladb::Stage::Project {
                fields: vec!["doc".into()],
                include: true,
                keep_id: false,
            }])?;
        let indexed: HashSet<String> = rows
            .iter()
            .filter_map(|r| match r.get("doc") {
                Some(taladb::Value::Str(doc)) => Some(doc.clone()),
                _ => None,
            })
            .collect();
        let expected: HashSet<String> = corpus
            .values()
            .map(|entry| smcli_search::corpus::search_document(entry, false))
            .collect();
        report.indexed = indexed.len();
        report.missing = expected.difference(&indexed).count();
        report.stale = indexed.difference(&expected).count();
        Ok(())
    })();
    if let Err(error) = result {
        report.problems.push(format!("{error:#}"));
    }
    report.healthy = report.problems.is_empty();
    report
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn atuin_import_deduplicates_and_replace_is_atomic() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("smcli-atuin-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir)?;
        let source = dir.join("atuin.db");
        let db = rusqlite::Connection::open(&source)?;
        db.execute_batch("CREATE TABLE history(id TEXT,timestamp INTEGER,duration INTEGER,exit INTEGER,command TEXT,cwd TEXT,session TEXT,hostname TEXT); INSERT INTO history VALUES('atuin-id',1700000000000000000,1000000000,0,'git status','/tmp','session','host:user');")?;
        let c = CapturedHistory::new(dir.join("history.db"));
        for _ in 0..2 {
            import(&c, &dir, &HashMap::new(), true, false, Some(source.clone()))?;
        }
        let rows = records(&c)?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["durationMs"], 1000);
        assert_eq!(rows[0]["user"], "user");
        import(&c, &dir, &HashMap::new(), true, true, Some(source))?;
        assert_eq!(records(&c)?.len(), 1);
        drop(db);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn health_checks_coverage_without_rebuilding() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("smcli-health-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir)?;
        let captured = CapturedHistory::new(dir.join("history.db"));
        let env = HashMap::from([(
            "SMCLI_CONFIG".into(),
            dir.join("config.json").to_string_lossy().into_owned(),
        )]);
        let corpus = HashMap::from([(
            "git status".into(),
            SearchEntry::new("git status", 1, "native"),
        )]);
        let missing = index_health(&captured, &dir, &env, &corpus);
        assert!(missing.healthy && !missing.built);
        assert_eq!(missing.missing, 1);
        assert!(!Path::new(&missing.path).exists());
        let engine = search_engine(&captured, &dir, &env)?;
        engine.reindex(&corpus)?;
        let built = index_health(&captured, &dir, &env, &corpus);
        assert!(built.healthy, "{:?}", built.problems);
        assert!(built.built);
        assert_eq!(built.indexed, 1);
        assert_eq!(built.missing, 0);
        assert_eq!(
            index_health(&captured, &dir, &env, &HashMap::new()).stale,
            1
        );
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(engine.directory.join("lock"))?;
        lock.lock()?;
        assert!(!index_health(&captured, &dir, &env, &corpus).healthy);
        drop(lock);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn prune_keeps_clean_selections_of_allowed_commands() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("smcli-prune-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir)?;
        let c = CapturedHistory::new(dir.join("history.db"));
        let db = c.open_connection()?;
        db.execute_batch(
            "INSERT INTO sessions(id,user,host,shell) VALUES('s','test','unknown','zsh');
             INSERT INTO commands(session,started_at,directory,command,exit_code) VALUES
               ('s',1700000000.0,'/tmp','git push',0),
               ('s',1700000001.0,'/tmp','secret-cmd',0);
             INSERT INTO command_selections(query,command,kind,directory,selected_at) VALUES
               ('AKIAABCDEFGHIJKLMNOP','git push','search','/tmp',1700000002.0),
               ('push','git push','search','/tmp',1700000003.0),
               ('push','secret-cmd','search','/tmp',1700000004.0);",
        )?;
        drop(db);
        let filter = CaptureFilter::new(&SmcliConfig {
            history_filter: Some(vec!["^secret".into()]),
            ..Default::default()
        })?;
        prune(&c, &dir, &HashMap::new(), &filter, false)?;
        let db = c.open_connection()?;
        let commands: Vec<String> = db
            .prepare("SELECT command FROM commands ORDER BY id")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        assert_eq!(commands, ["git push"]);
        let selections: Vec<String> = db
            .prepare("SELECT query FROM command_selections ORDER BY id")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        assert_eq!(selections, ["push"]);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn alias_export_import_and_prune_keep_variant_identity() -> Result<()> {
        let root = std::env::temp_dir().join(format!("smcli-alias-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root)?;
        let home = root.as_path();
        let source = CapturedHistory::new(home.join("source.db"));
        for expanded in ["git log", "git ls-files"] {
            let mut run = NewRun::new("gl", "zsh", "/repo", 0);
            run.expanded_command = Some(expanded);
            run.started_at = Some(1_700_000_000.0);
            source.append(&run, "", None)?;
        }
        let exported = records(&source)?;
        assert!(exported.iter().all(|r| r["runCount"] == 1));
        let jsonl = home.join("export.jsonl");
        std::fs::write(&jsonl, exported.iter().map(|r| format!("{r}\n")).collect::<String>())?;
        let target = CapturedHistory::new(home.join("target.db"));
        let env = HashMap::from([("HISTFILE".into(), String::new())]);
        import(&target, home, &env, false, false, Some(jsonl.clone()))?;
        import(&target, home, &env, false, false, Some(jsonl))?;
        assert_eq!(records(&target)?.len(), 2);
        let db = target.open_connection()?;
        let filter = CaptureFilter::new(&SmcliConfig { history_filter: Some(vec!["ls-files".into()]), ..Default::default() })?;
        target.record_pick("gl", &smcli_core::entry::command_key("gl", Some("git log")), "search", "/repo", &CaptureFilter::standard(), now(), &db)?;
        target.record_pick("gl", &smcli_core::entry::command_key("gl", Some("git ls-files")), "search", "/repo", &CaptureFilter::standard(), now(), &db)?;
        prune(&target, home, &env, &filter, false)?;
        let kept = records(&target)?;
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["expandedCommand"], "git log");
        assert_eq!(db.query_row("SELECT count(*) FROM command_selections", [], |r| r.get::<_, i64>(0))?, 1);
        drop(db);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn legacy_import_is_idempotent() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("smcli-legacy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir)?;
        let c = CapturedHistory::new(dir.join("history.db"));
        let source = dir.join("legacy.jsonl");
        let record = serde_json::json!({"timestamp":"2024-01-01T00:00:00Z","user":"test","shell":"zsh","command":"git status","directory":"/tmp","exitCode":0}).to_string();
        std::fs::write(&source, record)?;
        for _ in 0..2 {
            import(&c, &dir, &HashMap::new(), false, false, Some(source.clone()))?;
        }
        let rows = records(&c)?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["runCount"], 1);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn imports_are_atomic_and_pruning_honors_dry_run() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("smcli-cli-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir)?;
        let c = CapturedHistory::new(dir.join("history.db"));
        let source = dir.join("legacy.jsonl");
        let good = serde_json::json!({"timestamp":"2024-01-01T00:00:00Z","user":"test","shell":"zsh","command":"secret-test","directory":"/tmp","exitCode":0}).to_string();
        std::fs::write(&source, format!("{good}\ninvalid\n"))?;
        assert!(
            import(
                &c,
                &dir,
                &HashMap::new(),
                false,
                false,
                Some(source.clone())
            )
            .is_err()
        );
        assert!(records(&c)?.is_empty());
        std::fs::write(&source, good)?;
        import(&c, &dir, &HashMap::new(), false, false, Some(source))?;
        assert_eq!(records(&c)?.len(), 1);
        let filter = CaptureFilter::new(&SmcliConfig {
            history_filter: Some(vec!["^secret".into()]),
            ..Default::default()
        })?;
        prune(&c, &dir, &HashMap::new(), &filter, true)?;
        assert_eq!(records(&c)?.len(), 1);
        prune(&c, &dir, &HashMap::new(), &filter, false)?;
        assert!(records(&c)?.is_empty());
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
