use std::{collections::HashSet, path::Path};

use smcli_search::{Corpus, Entry, Potion, corpus_vectors};
use smcli_search::tldr::{self, rank_with_vectors};

fn entry(program: &str, description: &str, template: &str) -> Entry {
    Entry { program: program.into(), description: description.into(), template: template.into() }
}

#[test]
fn tldr_placeholders_include_options_multiple_and_nested_braces() {
    assert_eq!(tldr::to_placeholders("cat {{file}}"), "cat @");
    assert_eq!(tldr::to_placeholders("mv {{source}} {{destination}}"), "mv @ @");
    assert_eq!(tldr::to_placeholders("bun add {{[-g|--global]}} {{package}}"), "bun add --global @");
    assert_eq!(tldr::to_placeholders("cmd {{[--one|--two|--three]}} {{[a|b|c]}}"), "cmd @ @");
    assert_eq!(tldr::to_placeholders("echo {{outer {{inner}} tail}} {{value}}"), "echo @ @");
    assert_eq!(tldr::to_placeholders("echo {{unterminated"), "echo {{unterminated");
}

#[test]
fn tldr_jsonl_loads_common_and_requested_platform() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("tldr.jsonl"), concat!(
        "{\"version\":1,\"tldr_commit\":\"abc123\",\"license\":\"CC-BY-4.0\"}\n",
        "{\"program\":\"git\",\"platform\":\"common\",\"description\":\"status\",\"template\":\"git status\"}\n",
        "{\"program\":\"mac\",\"platform\":\"osx\",\"description\":\"mac only\",\"template\":\"mac run\"}\n",
        "{\"program\":\"linux\",\"platform\":\"linux\",\"description\":\"linux only\",\"template\":\"linux run\"}\n",
    )).unwrap();
    let corpus = tldr::load(dir.path(), "osx").unwrap();
    assert_eq!(corpus.commit, "abc123");
    assert_eq!(corpus.entries.iter().map(|entry| entry.program.as_str()).collect::<Vec<_>>(), ["git", "mac"]);
    assert!(tldr::load(dir.path(), "linux").unwrap().entries.iter().any(|entry| entry.program == "linux"));
}

#[cfg(unix)]
#[test]
fn tldr_installed_requires_an_executable_file_on_path() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    for program in ["alpha", "beta"] {
        std::fs::write(bin.join(program), "").unwrap();
    }
    std::fs::set_permissions(bin.join("alpha"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(bin.join("beta"), std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::create_dir(bin.join("gamma")).unwrap();
    let programs = ["alpha", "beta", "gamma", "missing"].into_iter().map(str::to_owned).collect();
    let path = std::env::join_paths([&bin]).unwrap();
    assert_eq!(tldr::installed(&programs, &path.to_string_lossy()), HashSet::from(["alpha".to_owned()]));
}

#[test]
fn tldr_ranking_applies_prefix_bonus_and_stable_ties() {
    let entries = [entry("git", "show status", "git status"), entry("git", "continue", "git rebase --continue")];
    let vectors = vec![vec![1.0, 0.0], vec![1.0, 0.0]];
    let tied = rank_with_vectors(&[1.0, 0.0], &entries, &vectors, &[]);
    assert_eq!(tied.iter().map(|hit| hit.0).collect::<Vec<_>>(), [0, 1]);
    let prefixed = rank_with_vectors(&[1.0, 0.0], &entries, &vectors, &["git".into(), "rebase".into()]);
    assert_eq!(prefixed[0].0, 1);
    assert!((prefixed[0].1 - 1.05).abs() < 1e-6);
}

#[test]
fn tldr_cache_build_append_reread_and_model_identity() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus {
        commit: "abc123".into(),
        entries: vec![
            entry("beta", "first beta", "beta one"),
            entry("beta", "second beta", "beta two"),
            entry("alpha", "alpha entry", "alpha run"),
        ],
    };
    let index = dir.path().join("index");
    let potion = Potion::bundled().unwrap();
    let alpha = HashSet::from(["alpha".to_owned()]);
    let first = corpus_vectors(&potion, &corpus, &alpha, &index).unwrap();
    assert_eq!(first.len(), 1);
    let name = tldr::cache_name(&potion, &corpus.commit);
    assert!(index.join(&name).is_file());
    let both = HashSet::from(["alpha".to_owned(), "beta".to_owned()]);
    let appended = corpus_vectors(&potion, &corpus, &both, &index).unwrap();
    assert_eq!(appended.len(), 3);
    assert_eq!(corpus_vectors(&potion, &corpus, &both, &index).unwrap(), appended);
    let removed = corpus_vectors(&potion, &corpus, &HashSet::from(["beta".to_owned()]), &index).unwrap();
    assert_eq!(removed.len(), 2);
    drop(potion);
    let custom = Potion::from_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Models/potion-shell-16m").as_path()).unwrap();
    assert_ne!(name, tldr::cache_name(&custom, &corpus.commit));
}

const EVAL_ENTRIES: &[(&str, &str, &str)] = &[
    ("bun", "Install a package globally", "bun add --global {{package}}"),
    ("bun", "Install all dependencies", "bun install"),
    ("bun", "Add a development dependency", "bun add --dev {{package}}"),
    ("bun", "Remove a dependency", "bun remove {{package}}"),
    ("bun", "Run a package script", "bun run {{script}}"),
    ("bun", "Run tests", "bun test"),
    ("bun", "Create a new project from a template", "bun create {{template}} {{directory}}"),
    ("bun", "Update dependencies", "bun update"),
    ("bun", "Execute a package binary", "bun x {{command}}"),
    ("bun", "List installed packages", "bun pm ls"),
    ("git", "Show working tree status", "git status"),
    ("git", "Commit staged changes with a message", "git commit -m {{message}}"),
    ("git", "Amend the most recent commit", "git commit --amend"),
    ("git", "Continue a rebase after resolving conflicts", "git rebase --continue"),
    ("git", "Interactively rebase commits", "git rebase -i {{commit}}"),
    ("git", "Stash uncommitted changes", "git stash"),
    ("git", "Restore the latest stash", "git stash pop"),
    ("git", "Show compact commit history", "git log --oneline"),
    ("git", "Delete a local branch", "git branch -d {{branch}}"),
    ("git", "Undo the last commit and keep its changes", "git reset --soft HEAD~1"),
    ("tar", "Extract an archive", "tar -xf {{archive}}"),
    ("tar", "Extract a gzip archive", "tar -xzf {{archive.tar.gz}}"),
    ("tar", "Create a gzip archive", "tar -czf {{archive.tar.gz}} {{files}}"),
    ("tar", "List archive contents", "tar -tf {{archive}}"),
    ("tar", "Extract an archive to a directory", "tar -xf {{archive}} -C {{directory}}"),
    ("tar", "Create an archive from files", "tar -cf {{archive.tar}} {{files}}"),
    ("tar", "Extract a bzip2 archive", "tar -xjf {{archive.tar.bz2}}"),
    ("tar", "Append a file to an archive", "tar -rf {{archive.tar}} {{file}}"),
    ("tar", "Exclude a pattern while extracting", "tar -xf {{archive}} --exclude={{pattern}}"),
    ("tar", "Extract an archive verbosely", "tar -xvf {{archive}}"),
    ("docker", "List running containers", "docker ps"),
    ("docker", "List all containers including stopped containers", "docker ps -a"),
    ("docker", "Start compose services in detached mode", "docker compose up -d"),
    ("docker", "Stop a running container", "docker stop {{container}}"),
    ("docker", "Follow container logs", "docker logs -f {{container}}"),
    ("docker", "Build an image from a Dockerfile", "docker build -t {{image}} ."),
    ("docker", "Remove an image", "docker rmi {{image}}"),
    ("docker", "Run a container interactively", "docker run -it {{image}}"),
    ("docker", "Remove unused Docker data", "docker system prune"),
    ("docker", "Open a shell in a running container", "docker exec -it {{container}} sh"),
    ("find", "Find files by name", "find {{directory}} -name {{filename}}"),
    ("find", "Find files by case-insensitive name", "find {{directory}} -iname {{filename}}"),
    ("find", "Find regular files", "find {{directory}} -type f"),
    ("find", "Find directories", "find {{directory}} -type d"),
    ("find", "Find files modified less than a day ago", "find {{directory}} -mtime -1"),
    ("find", "Delete matching files", "find {{directory}} -name {{filename}} -delete"),
    ("find", "Execute a command for matching files", "find {{directory}} -name {{filename}} -exec {{command}} {} \\;"),
    ("find", "Find files larger than 100 megabytes", "find {{directory}} -size +100M"),
    ("find", "Find empty files", "find {{directory}} -empty"),
    ("find", "Skip a directory while searching", "find {{directory}} -path {{exclude}} -prune -o -print"),
];

#[test]
fn tldr_ranking_evaluation() {
    let potion = Potion::bundled().unwrap();
    let cases = include_str!("fixtures/tldr-eval.tsv").lines().skip(1).map(|line| {
        let fields = line.split('\t').collect::<Vec<_>>();
        assert_eq!(fields.len(), 3, "invalid evaluation row: {line}");
        fields
    }).collect::<Vec<_>>();
    assert_eq!(cases.len(), 50);
    let mut reciprocal_rank = 0.0;
    let mut top_three = 0usize;
    let mut misses = Vec::new();
    for case in cases {
        let entries = EVAL_ENTRIES.iter().filter(|entry| entry.0 == case[0])
            .map(|(program, description, template)| entry(program, description, template)).collect::<Vec<_>>();
        let ranked = smcli_search::rank(&potion, case[1], &entries, &[]);
        let position = ranked.iter().position(|(index, _)| entries[*index].template == case[2]).map(|position| position + 1).unwrap();
        if position <= 3 { top_three += 1; } else { misses.push(format!("{}: {} (rank {position})", case[0], case[1])); }
        reciprocal_rank += 1.0 / position as f64;
    }
    let accuracy = top_three as f64 / 50.0;
    let mrr = reciprocal_rank / 50.0;
    println!("tldr top-3: {top_three}/50 ({:.1}%), MRR: {mrr:.3}", accuracy * 100.0);
    if !misses.is_empty() { println!("misses:\n{}", misses.join("\n")); }
    assert!(accuracy >= 0.80, "top-3 accuracy {:.1}% is below 80%", accuracy * 100.0);
}
