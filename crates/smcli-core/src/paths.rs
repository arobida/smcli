//! Path insertion helpers shared by the CLI and the service.
use std::fs;
use std::path::{Path, PathBuf};

/// `path` (under `root`) as seen from `cwd`. Insertion stays absolute outside `cwd`.
pub fn relative_insert(path: &str, root: &str, cwd: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let dir = path.ends_with('/');
    let Ok(target) = fs::canonicalize(Path::new(root).join(path)) else {
        return path.to_owned();
    };
    let cwd = fs::canonicalize(cwd).unwrap_or_else(|_| PathBuf::from(cwd));
    let is_root = |p: &Path| p.as_os_str() == "/";
    let result = if !is_root(&cwd) && let Ok(rel) = target.strip_prefix(&cwd) {
        if rel.as_os_str().is_empty() { ".".into() } else { rel.to_string_lossy().into_owned() }
    } else {
        target.to_string_lossy().into_owned()
    };
    if dir && !result.ends_with('/') { format!("{result}/") } else { result }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn relative_insert_cases() {
        let temporary = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temporary.path()).unwrap();
        fs::create_dir(root.join("x")).unwrap();
        fs::create_dir(root.join("y")).unwrap();
        fs::write(root.join("x/a.swift"), "").unwrap();
        fs::write(root.join("x/b.swift"), "").unwrap();
        let root = root.to_str().unwrap();
        assert_eq!(relative_insert("x/a.swift", root, &format!("{root}/x")), "a.swift");
        let abs = fs::canonicalize(Path::new(root).join("x/b.swift")).unwrap().to_string_lossy().into_owned();
        assert_eq!(relative_insert("x/b.swift", root, &format!("{root}/y")), abs);
        assert_eq!(relative_insert("x/b.swift", root, "/"), abs);
        assert_eq!(relative_insert("x/b.swift", root, root), "x/b.swift");
        assert_eq!(relative_insert("", root, root), "");
        symlink(format!("{root}/x"), format!("{root}/alias")).unwrap();
        assert_eq!(relative_insert("x/a.swift", root, &format!("{root}/alias")), "a.swift");
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
        assert_eq!(relative_insert("../../elsewhere/out.rs", root, root), elsewhere.to_string_lossy());
        fs::write(temporary.path().join("outside.rs"), "").unwrap();
        let outside = fs::canonicalize(temporary.path().join("outside.rs")).unwrap();
        assert_eq!(relative_insert("../../../outside.rs", root, root), outside.to_string_lossy());
    }
}
