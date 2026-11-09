//! Path helpers shared by the CLI and the service: XDG base directories, insertion.
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// An XDG base directory: its `*_HOME` variable when absolute (the spec says to ignore a relative one), else the
/// spec's default under `home`.
#[derive(Clone, Copy, Debug)]
pub enum Xdg {
    Config,
    Data,
    State,
    Cache,
}

impl Xdg {
    pub const ALL: [Xdg; 4] = [Xdg::Config, Xdg::Data, Xdg::State, Xdg::Cache];
    pub fn variable(self) -> &'static str {
        match self {
            Xdg::Config => "XDG_CONFIG_HOME",
            Xdg::Data => "XDG_DATA_HOME",
            Xdg::State => "XDG_STATE_HOME",
            Xdg::Cache => "XDG_CACHE_HOME",
        }
    }
    fn default(self) -> &'static str {
        match self {
            Xdg::Config => ".config",
            Xdg::Data => ".local/share",
            Xdg::State => ".local/state",
            Xdg::Cache => ".cache",
        }
    }
    pub fn dir(self, home: &Path, environment: &HashMap<String, String>) -> PathBuf {
        environment
            .get(self.variable())
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(self.default()))
    }
}

/// Folders fff skips only on non-git roots, as relative `**/` patterns (its `IGNORED_DIRS`).
// Checked against fff rev 95fd777 (0.11.0): fff-core ignore.rs:24-26, applied only when !is_git_repo (walk/zlob.rs:39-41, walk/ripgrep.rs:31).
const FFF_IGNORED: &[&str] = &[
    ".local/state",
    #[cfg(target_os = "macos")]
    "Library/Application Support",
];

/// Whether fff, indexing `root`, is known to skip `database`: `root` is outside any git work tree and the database
/// sits under one of fff's ignored folders. Anything else may be opened, and closing it drops SQLite's POSIX locks.
pub fn fff_skips(root: &Path, database: &Path) -> bool {
    let Ok(inside) = database.strip_prefix(root) else {
        return false;
    };
    if root.ancestors().any(|dir| dir.join(".git").exists()) {
        return false;
    }
    let parts: Vec<_> = inside.components().collect();
    FFF_IGNORED.iter().any(|d| {
        let d: Vec<_> = Path::new(d).components().collect();
        parts.windows(d.len()).any(|w| w == d.as_slice())
    })
}

/// XDG variables set to a relative path, which the spec says to ignore. Empty means fine.
pub fn check(home: &Path, environment: &HashMap<String, String>) -> Vec<String> {
    Xdg::ALL
        .into_iter()
        .filter_map(|base| {
            environment
                .get(base.variable())
                .filter(|v| !v.is_empty() && !Path::new(v).is_absolute())
                .map(|value| {
                    format!(
                        "{}={value} is not absolute; using {} as the XDG spec requires",
                        base.variable(),
                        base.dir(home, environment).display()
                    )
                })
        })
        .collect()
}

/// `path` (under `root`) as seen from `cwd`. Insertion stays absolute outside `cwd`.
pub fn relative_insert(path: &str, root: &str, cwd: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let dir = path.ends_with('/');
    let joined = Path::new(root).join(path);
    // Keep a final symlink as written; `..` has no file name, so it resolves fully.
    let target = match (joined.parent(), joined.file_name()) {
        (Some(parent), Some(name)) if joined.symlink_metadata().is_ok() => {
            fs::canonicalize(parent).map(|p| p.join(name))
        }
        _ => fs::canonicalize(&joined),
    };
    let Ok(target) = target else {
        return path.to_owned();
    };
    let cwd = fs::canonicalize(cwd).unwrap_or_else(|_| PathBuf::from(cwd));
    let is_root = |p: &Path| p.as_os_str() == "/";
    let result = if !is_root(&cwd)
        && let Ok(rel) = target.strip_prefix(&cwd)
    {
        if rel.as_os_str().is_empty() {
            ".".into()
        } else {
            rel.to_string_lossy().into_owned()
        }
    } else {
        target.to_string_lossy().into_owned()
    };
    if dir && !result.ends_with('/') {
        format!("{result}/")
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn paths_fff_skips_only_non_git_roots_with_ignored_folder() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        assert!(!fff_skips(home, &home.join("history.db")));
        assert!(fff_skips(home, &home.join(".local/state/smcli/history.db")));
        assert!(!fff_skips(home, Path::new("/elsewhere/history.db")));
        assert!(fff_skips(home, &home.join("x/.local/state/history.db")));
        assert!(!fff_skips(
            &home.join(".local"),
            &home.join(".local/state/history.db")
        ));
        fs::create_dir(home.join(".git")).unwrap();
        assert!(!fff_skips(
            home,
            &home.join(".local/state/smcli/history.db")
        ));
    }

    #[test]
    fn xdg_dirs_ignore_relative_values_and_check_reports_them() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let env = HashMap::from([
            ("XDG_STATE_HOME".to_string(), "relative".to_string()),
            ("XDG_DATA_HOME".to_string(), "/d".to_string()),
        ]);
        assert_eq!(Xdg::State.dir(home, &env), home.join(".local/state"));
        assert_eq!(Xdg::Data.dir(home, &env), PathBuf::from("/d"));
        assert_eq!(Xdg::Config.dir(home, &HashMap::new()), home.join(".config"));
        assert!(check(home, &HashMap::new()).is_empty());
        let problems = check(home, &env);
        assert!(
            problems.len() == 1 && problems[0].contains("XDG_STATE_HOME=relative"),
            "{problems:?}"
        );
    }

    #[test]
    fn relative_insert_cases() {
        let temporary = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temporary.path()).unwrap();
        fs::create_dir(root.join("x")).unwrap();
        fs::create_dir(root.join("y")).unwrap();
        fs::write(root.join("x/a.swift"), "").unwrap();
        fs::write(root.join("x/b.swift"), "").unwrap();
        let root = root.to_str().unwrap();
        assert_eq!(
            relative_insert("x/a.swift", root, &format!("{root}/x")),
            "a.swift"
        );
        let abs = fs::canonicalize(Path::new(root).join("x/b.swift"))
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            relative_insert("x/b.swift", root, &format!("{root}/y")),
            abs
        );
        assert_eq!(relative_insert("x/b.swift", root, "/"), abs);
        assert_eq!(relative_insert("x/b.swift", root, root), "x/b.swift");
        assert_eq!(relative_insert("", root, root), "");
        symlink(format!("{root}/x"), format!("{root}/alias")).unwrap();
        assert_eq!(
            relative_insert("x/a.swift", root, &format!("{root}/alias")),
            "a.swift"
        );
    }

    #[test]
    fn relative_insert_keeps_final_symlink() {
        let temporary = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temporary.path()).unwrap();
        fs::write(root.join("real.txt"), "").unwrap();
        symlink(root.join("real.txt"), root.join("link.txt")).unwrap();
        let root = root.to_str().unwrap();
        assert_eq!(relative_insert("link.txt", root, root), "link.txt");
    }

    #[test]
    fn relative_insert_is_absolute_outside_cwd() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        fs::create_dir_all(home.join("Dev/other")).unwrap();
        fs::create_dir_all(home.join("elsewhere")).unwrap();
        let root = home.join("Dev/project");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("file.rs"), "").unwrap();
        fs::write(home.join("elsewhere/out.rs"), "").unwrap();
        let home = fs::canonicalize(&home).unwrap();
        let root = fs::canonicalize(&root).unwrap();
        let root = root.to_str().unwrap();
        let home = home.to_str().unwrap();
        assert_eq!(relative_insert("file.rs", root, root), "file.rs");
        let project_file = fs::canonicalize(Path::new(root).join("file.rs")).unwrap();
        let insert = relative_insert("file.rs", root, &format!("{home}/Dev/other"));
        assert_eq!(insert, project_file.to_string_lossy());
        assert!(Path::new(&insert).is_file());
        let elsewhere = fs::canonicalize(Path::new(home).join("elsewhere/out.rs")).unwrap();
        assert_eq!(
            relative_insert("../../elsewhere/out.rs", root, root),
            elsewhere.to_string_lossy()
        );
        fs::write(temporary.path().join("outside.rs"), "").unwrap();
        let outside = fs::canonicalize(temporary.path().join("outside.rs")).unwrap();
        assert_eq!(
            relative_insert("../../../outside.rs", root, root),
            outside.to_string_lossy()
        );
    }
}
