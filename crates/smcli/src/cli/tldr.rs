use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use smcli_core::config::SmcliConfig;
use smcli_core::history::CapturedHistory;
use smcli_core::time::now;
use smcli_core::service_client::ServiceClient;
use smcli_core::service_protocol::{Operation, ServiceRequest};
use smcli_search::setup::{configured_model, index_directory};

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

/// First words of captured commands (smcli's own history, distinct).
pub(crate) fn captured_programs(c: &CapturedHistory) -> Result<HashSet<String>> {
    let stats = c.command_stats(None, None, None, now())?;
    Ok(stats.keys().filter_map(|key| smcli_core::entry::SearchEntry::from_key(key).program()).collect())
}

pub(super) fn pick_tldr(c: &CapturedHistory, service: &ServiceClient, home: &Path, env: &HashMap<String, String>) -> Result<()> {
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


#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
    fn captured_programs_uses_expanded_alias() -> Result<()> {
        let (dir, captured, _) = tldr_service("0");
        let mut run = smcli_core::history::NewRun::new("j", "bash", "/tmp", 0);
        run.expanded_command = Some("jq . f.json");
        captured.append(&run, "", None)?;
        assert_eq!(captured_programs(&captured)?, HashSet::from(["jq".to_owned()]));
        std::fs::remove_dir_all(dir)?;
        Ok(())
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
    fn tldr_pick_arguments_and_lbuffer_context() {
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
}
