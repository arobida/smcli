//! History pruning; callers own the index lock and index-file cleanup.
use crate::config::CaptureFilter;
use crate::history::CapturedHistory;
use anyhow::{Result, bail};
use std::collections::HashSet;
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Default)]
pub struct PruneReport {
    pub database_exists: bool,
    /// Typed commands to display for a dry run; counts include every removed row.
    pub commands: Vec<String>,
    pub removed_commands: usize,
    pub removed_selections: usize,
    /// Kept copies (`.v7.bak`, `.pre-xdg`) which may still contain pruned history; callers should warn about each.
    pub backup: Vec<PathBuf>,
}

/// Deletes already committed; cleanup or compaction then failed.
/// Carries the same counts and backup the success report would, so the caller can warn before returning.
#[derive(Debug)]
pub struct PruneError {
    pub removed_commands: usize,
    pub removed_selections: usize,
    pub backup: Vec<PathBuf>,
    message: String,
}

impl fmt::Display for PruneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PruneError {}

impl CapturedHistory {
    /// The caller must hold its index lock until this returns; cleanup runs after committed deletes,
    /// before compaction. A dry run neither deletes rows nor invokes cleanup.
    pub fn prune(
        &self,
        filter: &CaptureFilter,
        dry: bool,
        cleanup: impl FnOnce() -> Result<()>,
    ) -> Result<PruneReport> {
        if !self.path.exists() {
            return Ok(PruneReport::default());
        }
        let mut db = self.open_connection()?;
        // Dry-run is read-only: Deferred avoids taking the write lock that blocks concurrent capture.
        let tx = db.transaction_with_behavior(if dry {
            rusqlite::TransactionBehavior::Deferred
        } else {
            rusqlite::TransactionBehavior::Immediate
        })?;
        let mut commands = HashSet::new();
        let mut ids = vec![];
        {
            let mut s = tx.prepare(
                "SELECT id,command,directory,expanded_command FROM commands ORDER BY started_at,id",
            )?;
            for row in s.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })? {
                let (id, command, d, expanded) = row?;
                if !filter.allows(&command, &d)
                    || expanded.as_deref().is_some_and(|e| !filter.allows(e, &d))
                {
                    ids.push(id);
                    commands.insert(crate::entry::command_key(&command, expanded.as_deref()));
                }
            }
        }
        let mut file_picks = vec![];
        {
            let mut s = tx.prepare("SELECT id,query,root,cwd FROM file_selections")?;
            for row in s.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })? {
                let (id, query, root, cwd) = row?;
                // Blank-query picks still carry a root; cwd filters must remove them too. NULL cwd predates v9.
                if !filter.allows(&query, &root)
                    || cwd.is_some_and(|cwd| !filter.allows(&query, &cwd))
                {
                    file_picks.push(id);
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
                let (typed, expanded) = crate::entry::command_parts(&command);
                if commands.contains(&command)
                    || !filter.allows_run(&typed, expanded.as_deref(), &d)
                    || (!q.is_empty() && !filter.allows(&q, &d))
                {
                    picks.push(id);
                }
            }
        }
        let selection_removed = picks.len() + file_picks.len();

        let removed_commands = ids.len();
        let mut backup = Vec::new();
        if dry {
            tx.rollback()?;
        } else {
            for id in ids {
                tx.execute("DELETE FROM commands WHERE id=?", [id])?;
            }
            for id in picks {
                tx.execute("DELETE FROM command_selections WHERE id=?", [id])?;
            }
            for id in file_picks {
                tx.execute("DELETE FROM file_selections WHERE id=?", [id])?;
            }
            tx.commit()?;
            for suffix in [".v7.bak", ".pre-xdg"] {
                let mut name = self.path.as_os_str().to_owned();
                name.push(suffix);
                let path = PathBuf::from(name);
                if path.exists() {
                    backup.push(path);
                }
            }
            // Commit stands. Return the backup with the maintenance error so the caller can still warn.
            if let Err(error) = finish_prune(&db, cleanup) {
                return Err(PruneError {
                    removed_commands,
                    removed_selections: selection_removed,
                    backup,
                    message: format!("{error:#}"),
                }
                .into());
            }
        }
        let commands = commands
            .iter()
            .map(|key| crate::entry::command_parts(key).0)
            .collect();
        Ok(PruneReport {
            database_exists: true,
            commands,
            removed_commands,
            removed_selections: selection_removed,
            backup,
        })
    }
}

fn finish_prune(db: &rusqlite::Connection, cleanup: impl FnOnce() -> Result<()>) -> Result<()> {
    let cleanup = cleanup();
    let maintenance = db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;");
    match (cleanup, maintenance) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(cleanup), Ok(())) => {
            bail!("History was pruned, but index cleanup failed: {cleanup:#}")
        }
        (Ok(()), Err(maintenance)) => {
            bail!("History was pruned, but database compaction failed: {maintenance}")
        }
        (Err(cleanup), Err(maintenance)) => bail!(
            "History was pruned, but index cleanup failed: {cleanup:#}; database compaction failed: {maintenance}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SmcliConfig;

    #[test]
    fn prune_reports_counts_backup_and_runs_cleanup_after_commit() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let c = CapturedHistory::new(temporary.path().join("history.db"));
        let filter = CaptureFilter::new(&SmcliConfig {
            history_filter: Some(vec!["^secret".into()]),
            ..Default::default()
        })?;
        let absent = c.prune(&filter, false, || {
            panic!("missing database needs no cleanup")
        })?;
        assert!(!absent.database_exists);
        assert!(!c.path.exists());
        let db = c.open_connection()?;
        db.execute_batch("INSERT INTO sessions(id,user,host,shell) VALUES('s','test','host','bash');
            INSERT INTO commands(session,started_at,directory,command,exit_code) VALUES('s',1,'/tmp','secret-command',0);
            INSERT INTO command_selections(query,command,kind,directory,selected_at) VALUES('secret','secret-command','search','/tmp',1);")?;
        let backup = c.path.with_file_name("history.db.v7.bak");
        std::fs::write(&backup, b"old history")?;
        let dry = c.prune(&filter, true, || panic!("dry run invoked cleanup"))?;
        assert!(dry.database_exists);
        assert_eq!(dry.removed_commands, 1);
        assert_eq!(dry.removed_selections, 1);
        assert_eq!(dry.commands, ["secret-command"]);
        drop(db);
        let report = c.prune(&filter, false, || {
            let db = c.open_connection()?;
            assert_eq!(
                db.query_row("SELECT count(*) FROM commands", [], |r| r.get::<_, i64>(0))?,
                0
            );
            drop(db);
            Ok(())
        })?;
        assert_eq!(report.removed_commands, 1);
        assert_eq!(report.removed_selections, 1);
        assert_eq!(report.backup, [backup]);
        Ok(())
    }

    #[test]
    fn prune_error_keeps_backup_after_committed_delete() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let c = CapturedHistory::new(temporary.path().join("history.db"));
        let db = c.open_connection()?;
        db.execute_batch("INSERT INTO sessions(id,user,host,shell) VALUES('s','test','host','bash');
            INSERT INTO commands(session,started_at,directory,command,exit_code) VALUES('s',1,'/tmp','secret-command',0);")?;
        drop(db);
        let backup = c.path.with_file_name("history.db.v7.bak");
        std::fs::write(&backup, b"old history")?;
        let filter = CaptureFilter::new(&SmcliConfig {
            history_filter: Some(vec!["^secret".into()]),
            ..Default::default()
        })?;
        let failed = c.prune(&filter, false, || bail!("boom")).unwrap_err();
        let committed = failed
            .downcast_ref::<PruneError>()
            .expect("post-commit error");
        assert_eq!(committed.removed_commands, 1);
        assert_eq!(committed.backup, [backup]);
        let message = failed.to_string();
        assert!(
            message.contains("index cleanup failed") && message.contains("boom"),
            "{message}"
        );
        assert!(
            !message.contains("migration backup"),
            "caller prints the warning; {message}"
        );
        assert_eq!(
            c.open_connection()?
                .query_row("SELECT count(*) FROM commands", [], |r| r.get::<_, i64>(0))?,
            0
        );
        Ok(())
    }
    #[test]
    fn prune_keeps_clean_selections_of_allowed_commands() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let dir = temporary.path().to_path_buf();
        let c = CapturedHistory::new(dir.join("history.db"));
        let db = c.open_connection()?;
        db.execute_batch(
            "INSERT INTO sessions(id,user,host,shell) VALUES('s','test','unknown','zsh');
             INSERT INTO commands(session,started_at,directory,command,exit_code) VALUES
               ('s',1700000000.0,'/tmp','git push',0),
               ('s',1700000001.0,'/tmp','secret-cmd',0);
             INSERT INTO file_selections(root,path,mode,query,selected_at) VALUES('/tmp','secret.txt','files','secret query',1700000005.0);
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
        c.prune(&filter, false, || Ok(()))?;
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
        assert_eq!(
            db.query_row("SELECT count(*) FROM file_selections", [], |r| r
                .get::<_, i64>(0))?,
            0
        );
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn prune_removes_file_selection_from_excluded_cwd() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let c = CapturedHistory::new(temporary.path().join("history.db"));
        c.open_connection()?.execute_batch(
            "INSERT INTO file_selections(root,path,mode,query,cwd,selected_at) VALUES
               ('/ok','a','files','q','/private',1.0), ('/ok','b','files','q','/ok',2.0), ('/ok','c','files','q',NULL,3.0)",
        )?;
        let filter = CaptureFilter::new(&SmcliConfig {
            cwd_filter: Some(vec!["/private".into()]),
            ..Default::default()
        })?;
        c.prune(&filter, false, || Ok(()))?;
        let paths: Vec<String> = c
            .open_connection()?
            .prepare("SELECT path FROM file_selections ORDER BY id")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        assert_eq!(paths, ["b", "c"]);
        Ok(())
    }

    #[test]
    fn prune_removes_blank_query_file_selections_in_excluded_directories() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let dir = temporary.path().to_path_buf();
        let c = CapturedHistory::new(dir.join("history.db"));
        let db = c.open_connection()?;
        db.execute_batch(
            "INSERT INTO file_selections(root,path,mode,query,selected_at) VALUES('/private','private.txt','files','',1700000005.0)",
        )?;
        drop(db);
        let filter = CaptureFilter::new(&SmcliConfig {
            cwd_filter: Some(vec!["/private".into()]),
            ..Default::default()
        })?;
        c.prune(&filter, false, || Ok(()))?;
        let db = c.open_connection()?;
        assert_eq!(
            db.query_row("SELECT count(*) FROM file_selections", [], |r| r
                .get::<_, i64>(0))?,
            0
        );
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn prune_cleanup_runs_when_database_compaction_fails() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let captured = CapturedHistory::new(temporary.path().join("history.db"));
        let db = captured.open_connection()?;
        let stale = temporary.path().join("stale");
        std::fs::write(&stale, b"secret-cmd")?;
        db.execute_batch(
            "BEGIN; INSERT INTO sessions(id,user,host,shell) VALUES('s','test','host','bash');",
        )?;
        let error = finish_prune(&db, || {
            std::fs::remove_file(&stale)?;
            Ok(())
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("database compaction failed"), "{error}");
        assert!(
            !stale.exists(),
            "stale index must be removed despite compaction failure"
        );
        let error = finish_prune(&db, || {
            std::fs::remove_file(&stale)?;
            Ok(())
        })
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("index cleanup failed") && error.contains("database compaction failed"),
            "{error}"
        );
        db.execute_batch("ROLLBACK")?;
        Ok(())
    }
}
