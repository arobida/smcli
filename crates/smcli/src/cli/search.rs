use anyhow::Result;
use std::collections::HashMap;

use smcli_core::caller::CallerContext;
use smcli_core::context::ProjectContext;
use smcli_core::history::{CapturedHistory, user_name};
use smcli_core::ranking::RankingContext;
use smcli_core::service_client::ServiceClient;
use smcli_core::service_protocol::{Operation, SearchEntry, SearchFilter, ServiceRequest};
use smcli_core::time::now;
use smcli_search::lexical_search;
use smcli_search::setup::{corpus, search_signals, working_directory};
use std::time::Duration;

use super::{directory, print_json, time, Search};

pub(super) fn search(a: Search, captured: &CapturedHistory, service: &ServiceClient, env: &HashMap<String, String>) -> Result<()> {
    let filter = SearchFilter {
        program: a.program,
        subcommand: a.subcommand,
        since: a.since.as_deref().map(time).transpose()?,
        before: a.before.as_deref().map(time).transpose()?,
        min_runs: a.min_runs.map(|n| n as i64),
        author: a.author,
        directory: a.dir.map(|d| directory(Some(d), env)).transpose()?,
        flag: a.flag,
    };
    let query = a.query.join(" ");
    let mut request = ServiceRequest::new(Operation::Search);
    request.query = Some(query.clone());
    request.limit = Some(a.limit as i64);
    request.directory = Some(working_directory(env));
    request.session = env.get("SMCLI_SESSION").cloned();
    request.filter = Some(filter.clone());
    request.histfile = env.get("HISTFILE").cloned();
    request.login_shell = env.get("SHELL").cloned();
    let hits = match service.send(&request, None, Duration::from_secs(600)).filter(|r| r.ok).and_then(|r| r.hits) {
        Some(hits) => hits,
        None => {
            let caller = CallerContext::from_env(env);
            let context = RankingContext::new(&ProjectContext::discover(&caller.cwd), caller.session.clone(), caller.ranking_off.as_deref());
            eprintln!("smcli: search service unavailable; showing keyword matches only.");
            let native = caller.native_env();
            let corpus = corpus(captured, caller.home(), &native, context.project_root.as_deref(), context.kind(), now())?;
            lexical_search(&query, &corpus, a.limit, &filter, &search_signals(captured, Some(context), &native)?, now())?
        }
    };
    if a.json { print_json(&hits)?; } else { print_hits(&hits); }
    Ok(())
}

pub(super) fn suggest(
    dir: Option<String>,
    limit: usize,
    json: bool,
    prefix: Vec<String>,
    captured: &CapturedHistory,
    service: &ServiceClient,
    env: &HashMap<String, String>,
) -> Result<()> {
    let d = directory(dir, env)?;
    let prefix = prefix.join(" ");
    let mut request = ServiceRequest::new(Operation::Suggest);
    request.directory = Some(d.clone());
    request.prefix = Some(prefix.clone());
    request.limit = Some(limit as i64);
    request.session = env.get("SMCLI_SESSION").cloned();
    let suggestions = match service.send(&request, None, Duration::from_secs(10)).filter(|r| r.ok).and_then(|r| r.suggestions) {
        Some(s) => s,
        None => {
            let caller = CallerContext::from_env(env);
            captured.suggestions(&user_name(), &d, &prefix, limit, now(), Some(&RankingContext::new(
                &ProjectContext::discover(&d), caller.session.clone(), caller.ranking_off.as_deref(),
            )))?
        }
    };
    if json {
        print_json(&suggestions)?;
    } else {
        for s in suggestions {
            println!("{}", crate::picker::history_label(&s.key()));
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
        if let Some(last) = &h.last_run { details.push(format!("last={last}")); }
        if let Some(code) = h.last_exit_code { details.push(format!("exit={code}")); }
        if let Some(d) = &h.last_directory { details.push(d.clone()); }
        if h.runs_here > 0 { details.push(format!("here={}", h.runs_here)); }
        if h.source == "native" { details.push("native history".into()); }
        println!("{}\n    {}", crate::picker::history_label(&h.key()), details.join("  "));
        if let Some(why) = &h.why {
            if !why.is_empty() { println!("    why: {}", why.join(", ")); }
        }
    }
}
