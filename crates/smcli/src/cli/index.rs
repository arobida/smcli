use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

use smcli_core::history::CapturedHistory;
use smcli_core::service_protocol::SearchEntry;
use smcli_core::time::now;
use smcli_search::setup::{configured_model, corpus, index_directory};

use super::print_json;

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

/// `smcli health`: smcli's paths, then the search index.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Health {
    database: String,
    config: String,
    path_problems: Vec<String>,
    /// Its `healthy` covers the paths too.
    #[serde(flatten)]
    index: IndexHealth,
}

/// History and command-example indexes next to the database; `prune` keeps only the current model's.
pub(super) fn indexes(
    captured: &CapturedHistory,
    home: &Path,
    env: &HashMap<String, String>,
    prune: bool,
    json: bool,
) -> Result<()> {
    let model = configured_model(home, env)?;
    let directory = index_directory(captured);
    let current = smcli_search::index_name(&model);
    // The command-completion index (bundled sources in priority order); absent tldr data just means it is not current.
    let command_current = smcli_search::commands::bundled_sources().map(|sources| {
        let refs = smcli_search::commands::source_refs(&sources);
        smcli_search::commands::CommandIndex::reader(directory.clone(), model.clone(), &refs)
            .index_name()
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
        let state = if info.current {
            "current"
        } else if prune {
            "deleted"
        } else {
            "unused"
        };
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
        Err(error) => {
            return IndexHealth {
                path: format!("{}.search", captured.path.display()),
                commands: corpus.len(),
                problems: vec![format!("{error:#}")],
                ..Default::default()
            };
        }
    };
    let report = smcli_search::index::inspect(&index_directory(captured), &model, corpus);
    IndexHealth {
        embedder: format!(
            "{} ({} dimensions), {}",
            smcli_search::Potion::NAME,
            model.dimension,
            model.identity
        ),
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

pub(super) fn health(
    captured: &CapturedHistory,
    home: &Path,
    env: &HashMap<String, String>,
    json: bool,
) -> Result<()> {
    let config = smcli_core::config::SmcliConfig::location(home, env);
    let mut path_problems = smcli_core::paths::check(home, env);
    if let Err(error) = smcli_core::config::FileIgnore::load(&config, home) {
        path_problems.push(format!("{error:#}; file search fails until it is fixed"));
    }
    if let Some(old) = captured.stranded_legacy(home, env) {
        path_problems.push(format!(
            "open shells still use an old smcli build; restart them. {} holds commands not in the main history {}",
            old.display(),
            captured.path.display()
        ));
    }
    let corpus = corpus(captured, home, env, None, None, now())?;
    let mut report = index_health(captured, home, env, &corpus);
    report.healthy &= path_problems.is_empty();
    let healthy = report.healthy;
    if json {
        print_json(&Health {
            database: captured.path.display().to_string(),
            config: config.display().to_string(),
            path_problems,
            index: report,
        })?;
    } else {
        println!(
            "Database:     {}\nConfig:       {}",
            captured.path.display(),
            config.display()
        );
        println!(
            "Paths:        {}",
            if path_problems.is_empty() {
                "ok"
            } else {
                "problems"
            }
        );
        for problem in &path_problems {
            println!("  \u{2717} {problem}");
        }
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
            if healthy { "healthy" } else { "unhealthy" }
        );
        for problem in &report.problems {
            println!("  \u{2717} {problem}");
        }
    }
    if !healthy {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use smcli_core::service_protocol::SearchEntry;
    use smcli_search::setup::index_directory;
    use std::collections::HashMap;

    #[test]
    fn health_json_preserves_cli_fields_and_model_errors() -> Result<()> {
        let dir =
            std::env::temp_dir().join(format!("smcli-health-output-{}", uuid::Uuid::new_v4()));
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
        assert!(
            report["embedder"]
                .as_str()
                .unwrap()
                .contains(smcli_search::Potion::NAME)
        );
        assert!(
            report["path"]
                .as_str()
                .unwrap()
                .starts_with(index_directory(&captured).to_str().unwrap())
        );
        assert!(!index_directory(&captured).exists());
        std::fs::write(
            dir.join("config.json"),
            r#"{"potionPath":"/missing/smcli-model"}"#,
        )?;
        let failed = serde_json::to_value(index_health(&captured, &dir, &env, &corpus))?;
        assert_eq!(failed["healthy"], false);
        assert_eq!(failed["commands"], 1);
        assert_eq!(failed["problems"].as_array().unwrap().len(), 1);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
