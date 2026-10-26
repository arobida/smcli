use std::path::Path;

const MAX_DEPTH: usize = 6;
const MAX_ENTRIES: usize = 20_000;

/// In-process `@` completion. `word` has the leading `@` stripped by the caller.
pub(crate) fn complete_local(cwd: &Path, word: &str, dirs_only: bool, limit: usize) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let mut paths = Vec::new();
    let mut n = 0;
    walk(cwd, cwd, 0, dirs_only, &mut n, &mut paths);
    let glob = word.bytes().any(|b| matches!(b, b'*' | b'?' | b'['));
    let mut hits: Vec<String> = if word.is_empty() {
        paths
    } else if glob {
        paths.into_iter().filter(|p| smcli_core::parse::glob_match(word, p)).collect()
    } else {
        paths.into_iter().filter(|p| subsequence(word, p)).collect()
    };
    if glob {
        hits.sort_by(|a, b| a.len().cmp(&b.len()).then(a.cmp(b)));
    } else {
        hits.sort_by(|a, b| rank(word, a).cmp(&rank(word, b)).then(a.cmp(b)));
    }
    hits.truncate(limit);
    // Directories end in `/`, as the service's do.
    hits.into_iter().map(|p| if cwd.join(&p).is_dir() { p + "/" } else { p }).collect()
}

fn walk(cwd: &Path, dir: &Path, depth: usize, dirs_only: bool, n: &mut usize, out: &mut Vec<String>) {
    if depth >= MAX_DEPTH || *n >= MAX_ENTRIES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if *n >= MAX_ENTRIES {
            return;
        }
        *n += 1;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(ft) = entry.file_type() else { continue };
        let is_dir = ft.is_dir();
        if is_dir && (name.starts_with('.') || name == "target" || name == "node_modules") {
            continue;
        }
        let path = entry.path();
        if let Ok(rel) = path.strip_prefix(cwd)
            && let Some(rel) = rel.to_str()
            && (is_dir || !dirs_only)
        {
            out.push(rel.replace('\\', "/"));
        }
        if is_dir {
            walk(cwd, &path, depth + 1, dirs_only, n, out);
        }
    }
}

fn rank(word: &str, path: &str) -> (u8, usize) {
    let base = path.rsplit('/').next().unwrap_or(path);
    (if subsequence(word, base) { 0 } else { 1 }, path.len())
}

fn subsequence(q: &str, s: &str) -> bool {
    let mut chars = s.chars();
    q.chars().all(|qc| chars.any(|sc| sc.eq_ignore_ascii_case(&qc)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("smcli-complete-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(root: &Path, rel: &str) {
        let p = root.join(rel);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(p, []).unwrap();
    }

    fn fixture() -> PathBuf {
        let dir = tmp();
        touch(&dir, "crates/smcli-search/tests/ranking.rs");
        touch(&dir, "crates/smcli/src/main.rs");
        touch(&dir, "src/main.rs");
        touch(&dir, "src/lib.rs");
        touch(&dir, "README.md");
        std::fs::create_dir_all(dir.join("crates/smcli-service")).unwrap();
        touch(&dir, "target/debug/smcli");
        touch(&dir, "node_modules/pkg/index.js");
        touch(&dir, ".git/HEAD");
        touch(&dir, ".hidden/secret");
        touch(&dir, "l1/l2/l3/l4/l5/keep.txt");
        touch(&dir, "l1/l2/l3/l4/l5/l6/gone.txt");
        dir
    }

    #[test]
    fn fuzzy_subsequence_ranks_basename_then_shorter() {
        let dir = fixture();
        let hits = complete_local(&dir, "rnkng", false, 8);
        assert_eq!(hits, ["crates/smcli-search/tests/ranking.rs"]);
        let mains = complete_local(&dir, "main", false, 8);
        assert_eq!(mains[0], "src/main.rs");
        assert!(mains.contains(&"crates/smcli/src/main.rs".into()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dirs_only_skips_files_and_ignored_trees() {
        let dir = fixture();
        let hits = complete_local(&dir, "serv", true, 8);
        assert_eq!(hits, ["crates/smcli-service/"]);
        let all = complete_local(&dir, "", false, 100);
        assert!(all.iter().any(|p| p == "src/main.rs"));
        assert!(all.iter().all(|p| !p.starts_with("target")
            && !p.starts_with("node_modules")
            && !p.starts_with(".git")
            && !p.starts_with(".hidden")));
        assert!(all.iter().any(|p| p == "l1/l2/l3/l4/l5/keep.txt"));
        assert!(all.iter().any(|p| p == "l1/l2/l3/l4/l5/l6/"));
        assert!(all.iter().all(|p| p != "l1/l2/l3/l4/l5/l6/gone.txt"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn glob_star_star_question_and_class() {
        let dir = fixture();
        let nested = complete_local(&dir, "src/**/*.rs", false, 8);
        assert_eq!(nested, ["src/lib.rs", "src/main.rs"]);
        assert!(complete_local(&dir, "*.md", false, 8).contains(&"README.md".into()));
        assert!(!complete_local(&dir, "*.rs", false, 8).iter().any(|p| p.contains('/')));
        let ranking = complete_local(&dir, "crates/**/*.rs", false, 8);
        assert!(ranking.contains(&"crates/smcli-search/tests/ranking.rs".into()));
        assert_eq!(complete_local(&dir, "src/main.r[s]", false, 8), ["src/main.rs"]);
        assert_eq!(complete_local(&dir, "src/??b.rs", false, 8), ["src/lib.rs"]);
        assert_eq!(complete_local(&dir, "", false, 3).len(), 3);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dirs_only_with_glob() {
        let dir = tmp();
        std::fs::create_dir_all(dir.join("src/foo/bar/service")).unwrap();
        std::fs::create_dir_all(dir.join("src/util")).unwrap();
        touch(&dir, "src/foo/bar/service/mod.rs");
        touch(&dir, "src/foo/bar/server.rs");
        touch(&dir, "src/main.rs");
        let hits = complete_local(&dir, "src/**/serv*", true, 8);
        assert_eq!(hits, ["src/foo/bar/service/"]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
