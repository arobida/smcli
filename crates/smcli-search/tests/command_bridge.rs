//! Synthetic dual-index integration checks; never read user history.
use smcli_core::entry::SearchEntry;
use smcli_core::entry::SearchFilter;
use smcli_search::corpus::SearchSignals;
use smcli_search::{
    Potion,
    commands::{CommandIndex, IndexedExample},
    index::{HistorySearch, IndexedCommand},
};
use std::collections::HashMap;
use std::time::Instant;

#[test]
fn command_bridge_description_lookup_and_silent_fallbacks() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let potion = Potion::bundled().unwrap();
    let engine = HistorySearch::new(root.path(), potion.clone());
    let corpus = [
        "git reset --soft HEAD~1",
        "git status",
        "tar -xzf a.tgz",
        "tar -czf b.tgz",
    ]
    .into_iter()
    .map(|s| (s.to_owned(), SearchEntry::new(s, 1, "native")))
    .collect();
    let index = CommandIndex::new(root.path(), potion.clone(), &[]);
    let search = |index| {
        let lock = engine.lock_index(20).unwrap();
        engine
            .search_with_command_index(
                "undo last commit",
                &corpus,
                10,
                &SearchFilter::default(),
                None,
                &SearchSignals::default(),
                0.0,
                None,
                lock.as_ref(),
                index,
            )
            .unwrap()
    };
    let baseline = search(None);
    let fingerprint = |hits: Vec<SearchEntry>| {
        hits.into_iter()
            .map(|e| (e.command, e.why))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        fingerprint(search(Some(&index))),
        fingerprint(baseline.clone())
    );
    assert!(!index.path().exists(), "search must never build an index");
    std::fs::write(index.path().with_extension("building"), "incomplete").unwrap();
    assert_eq!(
        fingerprint(search(Some(&index))),
        fingerprint(baseline.clone())
    );
    index
        .add_all(&[smcli_core::command_source::CommandExample {
            program: "git".into(),
            description: "undo last commit".into(),
            template: "git reset --soft {{revision}}".into(),
            source: "fixture".into(),
        }])
        .unwrap();
    let vector = potion.embed(&["undo last commit".into()]).remove(0);
    let matched = index.description_matches(&vector, 1, 0.60).unwrap();
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].template, "git reset --soft");
    assert!(
        index
            .description_matches(&vector, 3, 1.1)
            .unwrap()
            .is_empty()
    );
    let bridged = search(Some(&index));
    assert_eq!(bridged[0].command, "git reset --soft HEAD~1");
    assert!(
        bridged.iter().all(|e| corpus.contains_key(&e.command)),
        "templates are never results"
    );
    assert_eq!(
        fingerprint(search(None)),
        fingerprint(baseline.clone()),
        "in-process API ignores a present index"
    );

    let ready = root.path().join("ready");
    let release = root.path().join("release");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "command_bridge_lock_holder", "--nocapture"])
        .env("BRIDGE_LOCK_PATH", index.path())
        .env("BRIDGE_LOCK_READY", &ready)
        .env("BRIDGE_LOCK_RELEASE", &release)
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !ready.exists() && start.elapsed().as_secs() < 10 {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let locked = ready.exists();
    let fallback = search(Some(&index));
    std::fs::write(release, "release").unwrap();
    assert!(child.wait().unwrap().success());
    assert!(locked, "child failed to acquire command-index lock");
    assert_eq!(fingerprint(fallback), fingerprint(baseline));
}

#[test]
fn command_bridge_lock_holder() {
    let Some(path) = std::env::var_os("BRIDGE_LOCK_PATH") else {
        return;
    };
    let _db = taladb::open(path).unwrap();
    std::fs::write(std::env::var_os("BRIDGE_LOCK_READY").unwrap(), "ready").unwrap();
    let release = std::path::PathBuf::from(std::env::var_os("BRIDGE_LOCK_RELEASE").unwrap());
    let start = Instant::now();
    while !release.exists() && start.elapsed().as_secs() < 15 {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn bridge_dual_index_realistic_history() {
    let root = tempfile::tempdir().unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let potion = Potion::bundled().unwrap();
    let engine = HistorySearch::new(root.path(), potion.clone());
    let mut corpus: HashMap<_, _> = [
        "git reset --soft HEAD~1",
        "git log --oneline",
        "git status",
        "git commit -m update",
        "rm old.txt",
        "tar -xzf a.tgz",
    ]
    .into_iter()
    .map(|s| (s.to_owned(), SearchEntry::new(s, 1, "native")))
    .collect();
    for i in 0..4994 {
        let command = match i % 6 {
            0 => format!("git -C /tmp/project-{i} log --oneline"),
            1 => format!("cargo test --package package-{i}"),
            2 => format!("find /tmp/project-{i} -name '*.rs'"),
            3 => format!("tar -czf backup-{i}.tgz /tmp/project-{i}"),
            4 => format!("curl -o /tmp/file-{i} https://example.invalid/{i}"),
            _ => format!("rg pattern-{i} /tmp/project-{i}"),
        };
        corpus.insert(command.clone(), SearchEntry::new(&command, 1, "native"));
    }
    assert_eq!(corpus.len(), 5000);
    engine.reindex(&corpus).unwrap();
    let index = CommandIndex::new(root.path(), potion.clone(), &[]);

    let sources = smcli_search::commands::bundled_sources().unwrap();
    let refs = smcli_search::commands::source_refs(&sources);
    let bundled = [
        "git",
        "git-reset",
        "git-log",
        "tar",
        "docker",
        "docker-ps",
        "curl",
        "find",
        "rg",
        "cargo",
        "ls",
        "rm",
        "du",
        "lsof",
    ]
    .into_iter()
    .flat_map(|program| {
        CommandIndex::gather(&refs, program, smcli_search::tldr::current_platform())
            .unwrap_or_default()
    })
    .collect::<Vec<_>>();
    index.add_all(&bundled).unwrap();
    println!("bundled descriptions added: {}", bundled.len());
    let _lock = engine.lock_index(20).unwrap().unwrap();
    let history_db =
        taladb::open(root.path().join(format!("{}.taladb", engine.index_name()))).unwrap();
    let history = history_db.typed::<IndexedCommand>("commands").unwrap();
    let command_db = taladb::open(index.path()).unwrap();
    let examples = command_db.typed::<IndexedExample>("examples").unwrap();
    let query = potion.embed(&["undo last commit".into()]).remove(0);
    let direct = history
        .raw()
        .find_nearest("embedding", &query, 40, None)
        .unwrap();
    println!(
        "direct top scores: {:?}",
        direct
            .iter()
            .take(5)
            .map(|h| (h.document.get("command"), h.score))
            .collect::<Vec<_>>()
    );
    let mut best_recalled = 0.0_f32;
    let mut elapsed = Vec::new();
    for run in 0..21 {
        let start = Instant::now();
        let matches = examples
            .raw()
            .find_nearest("embedding", &query, 3, None)
            .unwrap();
        let templates: Vec<String> = matches
            .iter()
            .map(|h| match h.document.get("template").unwrap() {
                taladb::Value::Str(s) => s.clone(),
                _ => panic!(),
            })
            .collect();
        let vectors = potion.embed(&templates);
        for (matched, vector) in matches.iter().zip(vectors) {
            let recalled = history
                .raw()
                .find_nearest("embedding", &vector, 40, None)
                .unwrap();
            assert!(!recalled.is_empty());
            for hit in &recalled {
                if hit.document.get("command")
                    == Some(&taladb::Value::Str("git reset --soft HEAD~1".into()))
                {
                    best_recalled = best_recalled.max(matched.score * hit.score);
                }
            }
            if run == 0 {
                println!(
                    "bridge description score={} template={:?} history={:?}",
                    matched.score,
                    matched.document.get("template"),
                    recalled
                        .iter()
                        .take(5)
                        .map(|h| (h.document.get("command"), h.score, matched.score * h.score))
                        .collect::<Vec<_>>()
                );
            }
        }
        elapsed.push(start.elapsed().as_secs_f64() * 1000.);
    }
    assert!(
        best_recalled > direct[0].score,
        "damped bridge must beat the incorrect direct leader"
    );
    elapsed.remove(0);
    elapsed.sort_by(f64::total_cmp);
    println!(
        "bridge dual-index + description lookup + batch embedding + 3 history queries: median={:.3}ms p95={:.3}ms",
        elapsed[10], elapsed[18]
    );
}
