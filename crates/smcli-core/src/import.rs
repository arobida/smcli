//! Transactional imports into captured history; source files are read-only.
use crate::config::CaptureFilter;
use crate::history::CapturedHistory;
use crate::time::parse_iso8601;
use anyhow::{Result, bail, ensure};
use std::path::Path;

fn validate_source(c: &CapturedHistory, source: &Path) -> Result<()> {
    ensure!(
        source.try_exists()?,
        "Import source not found: {}",
        source.display()
    );
    ensure!(
        source != c.path
            && std::fs::canonicalize(&source).ok() != std::fs::canonicalize(&c.path).ok(),
        "Cannot import history into itself"
    );
    Ok(())
}

impl CapturedHistory {
    /// Import Atuin UUIDs and tombstones; replacement is atomic with the import.
    /// Rows the capture filter rejects are skipped, but their tombstones still apply.
    pub fn import_atuin(
        &self,
        source: &Path,
        filter: &CaptureFilter,
        replace: bool,
    ) -> Result<usize> {
        validate_source(self, source)?;
        let mut db = self.open_connection()?;
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut count = 0;
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
            let command: String = r.get(4)?;
            let cwd: String = r.get(5)?;
            let deleted = r.get::<_, Option<i64>>(8)?.map(|n| n as f64 / 1e9);
            if !filter.allows_run(&command, None, &cwd) {
                if deleted.is_some() {
                    tx.execute(
                        "UPDATE commands SET deleted_at=? WHERE uuid=?",
                        rusqlite::params![deleted, r.get::<_, String>(0)?],
                    )?;
                }
                continue;
            }
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
            let flags = crate::parse::command_flags(&command).join(" ");
            let duration: i64 = r.get(2)?;
            count+=tx.execute("INSERT INTO commands(session,started_at,duration_ms,directory,command,exit_code,uuid,deleted_at,author,intent,flags) VALUES(?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(uuid) DO UPDATE SET deleted_at=excluded.deleted_at",rusqlite::params![session,r.get::<_,i64>(1)? as f64/1e9,if duration<0{None}else{Some(duration/1_000_000)},cwd,command,r.get::<_,i64>(3)?,r.get::<_,String>(0)?,deleted,r.get::<_,Option<String>>(10)?,r.get::<_,Option<String>>(11)?,flags])?;
        }

        tx.commit()?;
        if replace {
            db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
        }
        Ok(count)
    }

    /// Import exported JSONL, skipping filtered rows and previously imported variants.
    pub fn import_legacy(&self, source: &Path, capture_filter: &CaptureFilter) -> Result<usize> {
        validate_source(self, source)?;
        let mut db = self.open_connection()?;
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut count = 0;
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
            ensure!(
                expanded.is_none_or(|e| crate::parse::safe_expansion(e) && e != command),
                "Invalid expanded command."
            );
            let stamp = parse_iso8601(text("timestamp")?)
                .ok_or_else(|| anyhow::anyhow!("Invalid timestamp"))?;
            let directory = text("directory")?;
            let exit_code = r["exitCode"]
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("Missing exitCode"))?;
            if !capture_filter.allows_run(command, expanded, directory) {
                continue;
            }
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
                tx.execute("INSERT INTO commands(session,started_at,directory,command,exit_code,flags,expanded_command) VALUES(?,?,?,?,?,?,?)",rusqlite::params![session,stamp,directory,command,exit_code,crate::parse::command_flags(expanded.unwrap_or(command)).join(" "),expanded])?;
                count += 1;
            }
        }

        tx.commit()?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SmcliConfig;
    use crate::history::NewRun;
    use crate::time::now;
    use std::collections::HashMap;

    fn capture_filter(home: &Path) -> Result<CaptureFilter> {
        CaptureFilter::new(&SmcliConfig::load(&SmcliConfig::location(
            home,
            &HashMap::new(),
        ))?)
    }

    #[test]
    fn atuin_import_deduplicates_and_replace_is_atomic() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let dir = temporary.path().to_path_buf();
        let source = dir.join("atuin.db");
        let db = rusqlite::Connection::open(&source)?;
        db.execute_batch("CREATE TABLE history(id TEXT,timestamp INTEGER,duration INTEGER,exit INTEGER,command TEXT,cwd TEXT,session TEXT,hostname TEXT,deleted_at INTEGER); INSERT INTO history VALUES('atuin-id',1700000000000000000,1000000000,0,'git status','/tmp','session','host:user',NULL);")?;
        let c = CapturedHistory::new(dir.join("history.db"));
        for _ in 0..2 {
            c.import_atuin(&source, &capture_filter(&dir)?, false)?;
        }
        let rows = c.export_rows()?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["durationMs"], 1000);
        assert_eq!(rows[0]["user"], "user");
        db.execute(
            "UPDATE history SET deleted_at=1700000001 WHERE id='atuin-id'",
            [],
        )?;
        c.import_atuin(&source, &capture_filter(&dir)?, false)?;
        assert!(c.export_rows()?.is_empty());
        db.execute("UPDATE history SET deleted_at=NULL WHERE id='atuin-id'", [])?;
        c.import_atuin(&source, &capture_filter(&dir)?, false)?;
        assert_eq!(c.export_rows()?.len(), 1);
        c.import_atuin(&source, &capture_filter(&dir)?, true)?;
        assert_eq!(c.export_rows()?.len(), 1);
        db.execute_batch("INSERT INTO history VALUES('bad-id',1700000001000000000,0,0,NULL,'/tmp','session','host:user',NULL);")?;
        assert!(
            c.import_atuin(&source, &capture_filter(&dir)?, true)
                .is_err()
        );
        assert_eq!(
            c.export_rows()?.len(),
            1,
            "failed replacement must roll back its deletes"
        );
        drop(db);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn atuin_import_applies_filters() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let dir = temporary.path().to_path_buf();
        let source = dir.join("atuin.db");
        let db = rusqlite::Connection::open(&source)?;
        db.execute_batch("CREATE TABLE history(id TEXT,timestamp INTEGER,duration INTEGER,exit INTEGER,command TEXT,cwd TEXT,session TEXT,hostname TEXT,deleted_at INTEGER); INSERT INTO history VALUES('a',1700000000000000000,1,0,'secret one','/tmp','s','h:u',NULL);")?;
        let filter = CaptureFilter::new(&SmcliConfig {
            history_filter: Some(vec!["^secret".into()]),
            ..Default::default()
        })?;
        let c = CapturedHistory::new(dir.join("history.db"));
        c.import_atuin(&source, &filter, false)?;
        assert!(c.export_rows()?.is_empty());
        let sessions: i64 =
            c.open_connection()?
                .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))?;
        assert_eq!(sessions, 0);
        Ok(())
    }

    #[test]
    fn atuin_import_applies_filters_to_tombstones() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let dir = temporary.path().to_path_buf();
        let source = dir.join("atuin.db");
        let db = rusqlite::Connection::open(&source)?;
        db.execute_batch("CREATE TABLE history(id TEXT,timestamp INTEGER,duration INTEGER,exit INTEGER,command TEXT,cwd TEXT,session TEXT,hostname TEXT,deleted_at INTEGER); INSERT INTO history VALUES('a',1700000000000000000,1,0,'secret one','/tmp','s','h:u',NULL);")?;
        let c = CapturedHistory::new(dir.join("history.db"));
        c.import_atuin(&source, &capture_filter(&dir)?, false)?;
        assert_eq!(c.export_rows()?.len(), 1);
        db.execute("UPDATE history SET deleted_at=1700000001", [])?;
        let filter = CaptureFilter::new(&SmcliConfig {
            history_filter: Some(vec!["^secret".into()]),
            ..Default::default()
        })?;
        assert_eq!(c.import_atuin(&source, &filter, false)?, 0);
        assert!(c.export_rows()?.is_empty());
        Ok(())
    }

    #[test]
    fn legacy_import_applies_filters_without_expanded_command() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let dir = temporary.path().to_path_buf();
        std::fs::create_dir_all(dir.join(".config/smcli"))?;
        std::fs::write(
            dir.join(".config/smcli/config.json"),
            r#"{"historyFilter":["^secret"]}"#,
        )?;
        let c = CapturedHistory::new(dir.join("history.db"));
        let source = dir.join("legacy.jsonl");
        let plain = serde_json::json!({"timestamp":"2024-01-01T00:00:00Z","user":"test","shell":"zsh","command":"secret data","directory":"/tmp","exitCode":0});
        let expanded = serde_json::json!({"timestamp":"2024-01-01T00:00:01Z","user":"test","shell":"zsh","command":"sd","expandedCommand":"secret data","directory":"/tmp","exitCode":0});
        let allowed = serde_json::json!({"timestamp":"2024-01-01T00:00:02Z","user":"test","shell":"zsh","command":"git status","directory":"/tmp","exitCode":0});
        std::fs::write(&source, format!("{plain}\n{expanded}\n{allowed}\n"))?;
        c.import_legacy(&source, &capture_filter(&dir)?)?;
        let rows = c.export_rows()?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["command"], "git status");
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn alias_export_import_and_prune_keep_variant_identity() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let root = temporary.path().to_path_buf();
        let home = root.as_path();
        let source = CapturedHistory::new(home.join("source.db"));
        for expanded in ["git log", "git ls-files"] {
            let mut run = NewRun::new("gl", "zsh", "/repo", 0);
            run.expanded_command = Some(expanded);
            run.started_at = Some(1_700_000_000.0);
            source.append(&run, "", None)?;
        }
        let exported = source.export_rows()?;
        assert!(exported.iter().all(|r| r["runCount"] == 1));
        let jsonl = home.join("export.jsonl");
        std::fs::write(
            &jsonl,
            exported
                .iter()
                .map(|r| format!("{r}\n"))
                .collect::<String>(),
        )?;
        let target = CapturedHistory::new(home.join("target.db"));
        target.import_legacy(&jsonl, &capture_filter(home)?)?;
        target.import_legacy(&jsonl, &capture_filter(home)?)?;
        assert_eq!(target.export_rows()?.len(), 2);
        let db = target.open_connection()?;
        let filter = CaptureFilter::new(&SmcliConfig {
            history_filter: Some(vec!["ls-files".into()]),
            ..Default::default()
        })?;
        target.record_pick(
            "gl",
            &crate::entry::command_key("gl", Some("git log")),
            "search",
            "/repo",
            &CaptureFilter::standard(),
            now(),
            &db,
        )?;
        target.record_pick(
            "gl",
            &crate::entry::command_key("gl", Some("git ls-files")),
            "search",
            "/repo",
            &CaptureFilter::standard(),
            now(),
            &db,
        )?;
        target.prune(&filter, false, || Ok(()))?;
        let kept = target.export_rows()?;
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["expandedCommand"], "git log");
        assert_eq!(
            db.query_row("SELECT count(*) FROM command_selections", [], |r| r
                .get::<_, i64>(0))?,
            1
        );
        drop(db);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn import_reports_missing_source() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let dir = temporary.path();
        let c = CapturedHistory::new(dir.join("history.db"));
        let err = c
            .import_legacy(&dir.join("missing.jsonl"), &capture_filter(dir)?)
            .unwrap_err();
        assert!(err.to_string().contains("Import source not found"));
        Ok(())
    }

    #[test]
    fn legacy_import_is_idempotent() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let dir = temporary.path().to_path_buf();
        let c = CapturedHistory::new(dir.join("history.db"));
        let source = dir.join("legacy.jsonl");
        let record = serde_json::json!({"timestamp":"2024-01-01T00:00:00Z","user":"test","shell":"zsh","command":"git status","directory":"/tmp","exitCode":0}).to_string();
        std::fs::write(&source, record)?;
        for _ in 0..2 {
            c.import_legacy(&source, &capture_filter(&dir)?)?;
        }
        let rows = c.export_rows()?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["runCount"], 1);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn imports_are_atomic_and_pruning_honors_dry_run() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let dir = temporary.path().to_path_buf();
        let c = CapturedHistory::new(dir.join("history.db"));
        let source = dir.join("legacy.jsonl");
        let good = serde_json::json!({"timestamp":"2024-01-01T00:00:00Z","user":"test","shell":"zsh","command":"secret-test","directory":"/tmp","exitCode":0}).to_string();
        std::fs::write(&source, format!("{good}\ninvalid\n"))?;
        assert!(c.import_legacy(&source, &capture_filter(&dir)?).is_err());
        assert!(c.export_rows()?.is_empty());
        std::fs::write(&source, good)?;
        c.import_legacy(&source, &capture_filter(&dir)?)?;
        assert_eq!(c.export_rows()?.len(), 1);
        let filter = CaptureFilter::new(&SmcliConfig {
            history_filter: Some(vec!["^secret".into()]),
            ..Default::default()
        })?;
        c.prune(&filter, true, || Ok(()))?;
        assert_eq!(c.export_rows()?.len(), 1);
        c.prune(&filter, false, || Ok(()))?;
        assert!(c.export_rows()?.is_empty());
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
