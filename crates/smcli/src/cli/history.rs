use anyhow::{Result, ensure};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use smcli_core::config::{CaptureFilter, SmcliConfig};
use smcli_core::history::{CapturedHistory, NewRun, parse_pipestatus};
use smcli_core::service_client::ServiceClient;
use smcli_core::service_protocol::{Operation, ServiceRequest};
use smcli_core::time::now;
use smcli_search::setup::index_directory;

use super::print_json;

pub(super) fn epoch(s: &str) -> Result<Option<f64>> {
    if s.is_empty() {
        return Ok(None);
    }
    let value: f64 = s.replace(',', ".").parse()?;
    ensure!(value.is_finite() && value > 0.0, "Invalid time: {s}");
    Ok(Some(value))
}
pub(super) fn record(
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
            captured.append(&run, &f[4], None)?;
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

pub(super) fn history(
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
pub(super) fn prune(c: &CapturedHistory, filter: &CaptureFilter, dry: bool) -> Result<()> {
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
    let _lock = if dry {
        None
    } else {
        Some(
            smcli_search::index::lock_index_directory(&directory, u32::MAX)?
                .ok_or_else(|| anyhow::anyhow!("Another smcli is updating the index."))?,
        )
    };
    let pruned = c.prune(filter, dry, || {
        smcli_search::index::remove_history_indexes(&directory)
    });
    let report = match pruned {
        Ok(report) => report,
        Err(error) => {
            if let Some(failed) = error.downcast_ref::<smcli_core::prune::PruneError>() {
                println!(
                    "Removed {} commands and {} selections.",
                    failed.removed_commands, failed.removed_selections
                );
                warn_migration_backup(failed.backup.as_deref());
            }
            return Err(error);
        }
    };
    if dry {
        for command in report.commands {
            println!("{command}");
        }
        println!(
            "{} commands and {} selections would be removed.",
            report.removed_commands, report.removed_selections
        );
    } else {
        println!(
            "Removed {} commands and {} selections.",
            report.removed_commands, report.removed_selections
        );
        warn_migration_backup(report.backup.as_deref());
    }
    Ok(())
}

fn warn_migration_backup(backup: Option<&Path>) {
    if let Some(backup) = backup {
        eprintln!(
            "smcli: migration backup {} may still contain pruned history; remove it manually if no longer needed.",
            backup.display()
        );
    }
}
pub(super) fn import(
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
                smcli_core::paths::Xdg::Data
                    .dir(home, env)
                    .join("atuin/history.db")
            })
    });
    let capture_filter =
        CaptureFilter::new(&SmcliConfig::load(&SmcliConfig::location(home, env))?)?;
    let count = if atuin {
        c.import_atuin(&source, &capture_filter, replace)?
    } else {
        c.import_legacy(&source, &capture_filter)?
    };
    println!("Imported {count} records into {}.", c.path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use smcli_core::service_protocol::{SearchEntry, SearchFilter};
    use smcli_search::setup::search_engine;
    use std::collections::HashMap;

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
        let entries = HashMap::from([(
            "secret-cmd".into(),
            SearchEntry::new("secret-cmd", 1, "captured"),
        )]);
        let c = captured.clone();
        let (started, ready) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let filter = CaptureFilter::new(&SmcliConfig {
                history_filter: Some(vec!["^secret".into()]),
                ..Default::default()
            })?;
            started.send(()).unwrap();
            prune(&c, &filter, false)
        });
        ready.recv()?;
        engine.search_with_evidence_using_lock(
            "secret",
            &entries,
            1,
            &SearchFilter::default(),
            None,
            &smcli_search::corpus::SearchSignals::default(),
            now(),
            None,
            Some(&lock),
        )?;
        assert_eq!(
            db.query_row("SELECT count(*) FROM commands", [], |r| r.get::<_, i64>(0))?,
            1
        );
        drop(lock);
        worker.join().unwrap()?;
        assert!(!std::fs::read_dir(&engine.directory)?.any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("rs-commands-v")
        }));
        drop(db);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
