use std::collections::HashMap;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Where a service for one history database listens. Each database gets its own socket, so tests and
/// `SMCLI_DB` users never reach a service that owns another file.
#[derive(Debug, Clone)]
pub struct ServiceLocation {
    pub directory: PathBuf,
    pub socket: PathBuf,
    pub lock: PathBuf,
}

impl ServiceLocation {
    pub fn new(database: &Path, environment: &HashMap<String, String>) -> Self {
        let absolute = |key: &str| {
            environment.get(key).filter(|path| path.starts_with('/'))
        };
        let directory = if let Some(path) = absolute("SMCLI_RUNTIME_DIR") {
            PathBuf::from(path)
        } else if let Some(path) = absolute("XDG_RUNTIME_DIR") {
            Path::new(path).join("smcli")
        } else {
            // macOS's TMPDIR is already per-user; elsewhere the uid keeps users apart.
            let temporary = absolute("TMPDIR").map(String::as_str).unwrap_or("/tmp");
            Path::new(temporary).join(format!("smcli-{}", unsafe { libc::getuid() }))
        };
        // The path exactly as given: resolving it would split one database across two sockets.
        let name = format!("service-{}", Self::fingerprint(&database.to_string_lossy()));
        Self {
            socket: directory.join(format!("{name}.sock")),
            lock: directory.join(format!("{name}.lock")),
            directory,
        }
    }

    /// FNV-1a; only needs to tell database paths apart, not resist an attacker (the directory is private).
    pub fn fingerprint(text: &str) -> String {
        let mut hash = 0xcbf29ce484222325u64;
        for byte in text.bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
        format!("{hash:016x}")
    }

    /// Creates the directory private to this user, or checks an existing one is: a directory (not a symlink),
    /// owned by us, with no group or other access.
    pub fn prepare(&self) -> Result<()> {
        if let Err(error) = DirBuilder::new().mode(0o700).create(&self.directory)
            && error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error).with_context(|| format!("Cannot create {}", self.directory.display()));
            }
        let private = fs::symlink_metadata(&self.directory).is_ok_and(|info| {
            info.is_dir() && info.uid() == unsafe { libc::getuid() } && info.mode() & 0o077 == 0
        });
        if !private {
            bail!(
                "Service directory must be a private directory owned by you (chmod 700): {}",
                self.directory.display()
            );
        }
        Ok(())
    }
}

/// Identifies an executable by real path, modification time and size, so a rebuilt or replaced binary differs
/// from the copy a service started from. Nil when the file can't be examined.
pub fn build(executable: &Path) -> Option<String> {
    let resolved = fs::canonicalize(executable).ok()?;
    let info = fs::metadata(&resolved).ok()?;
    Some(format!(
        "{}|{}.{}|{}",
        resolved.to_string_lossy(),
        info.mtime(),
        info.mtime_nsec(),
        info.size()
    ))
}

/// `path` (under `root`) as seen from `directory`: `a.swift` from its own folder, `../x/b.swift` from a sibling.
pub fn relative_insert(path: &str, root: &str, directory: &str) -> String {
    let resolved = fs::canonicalize(directory).ok();
    let real = resolved.as_ref().map(|path| path.to_string_lossy());
    let real = real.as_deref().unwrap_or(directory);
    let root_parts: Vec<_> = root.split('/').filter(|part| !part.is_empty()).collect();
    let here: Vec<_> = real.split('/').filter(|part| !part.is_empty()).collect();
    // A directory outside the root can't be expressed relative to it; the root-relative path is the best we have.
    if !here.starts_with(&root_parts) {
        return path.to_owned();
    }
    let down = &here[root_parts.len()..];
    let target: Vec<_> = path.split('/').collect();
    let mut common = 0;
    while common < down.len() && common < target.len() - 1 && down[common] == target[common] {
        common += 1;
    }
    let mut parts = vec![".."; down.len() - common];
    parts.extend_from_slice(&target[common..]);
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn fingerprint_known_path() {
        // Independently computed with Python's bytewise FNV-1a and a 64-bit mask.
        assert_eq!(ServiceLocation::fingerprint("/tmp/history.db"), "e325ec6f80fdf18c");
        assert_eq!(ServiceLocation::fingerprint(""), "cbf29ce484222325");
    }

    #[test]
    fn socket_locations() {
        let uid = unsafe { libc::getuid() };
        let cases = [
            (vec![("SMCLI_RUNTIME_DIR", "/runtime"), ("XDG_RUNTIME_DIR", "/xdg")], "/runtime".to_owned()),
            (vec![("SMCLI_RUNTIME_DIR", "relative"), ("XDG_RUNTIME_DIR", "/xdg")], "/xdg/smcli".to_owned()),
            (vec![("XDG_RUNTIME_DIR", "relative"), ("TMPDIR", "/temporary")], format!("/temporary/smcli-{uid}")),
            (vec![("TMPDIR", "relative")], format!("/tmp/smcli-{uid}")),
        ];
        for (variables, base) in cases {
            let environment = variables.into_iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
            let location = ServiceLocation::new(Path::new("/tmp/history.db"), &environment);
            assert_eq!(location.directory, Path::new(&base));
            assert_eq!(location.socket, Path::new(&base).join("service-e325ec6f80fdf18c.sock"));
            assert_eq!(location.lock, Path::new(&base).join("service-e325ec6f80fdf18c.lock"));
        }
    }

    #[test]
    fn prepare_checks_private_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("runtime");
        let environment = HashMap::from([("SMCLI_RUNTIME_DIR".to_owned(), directory.to_str().unwrap().to_owned())]);
        let location = ServiceLocation::new(Path::new("/tmp/history.db"), &environment);
        location.prepare().unwrap();
        location.prepare().unwrap();
        assert_eq!(fs::metadata(&directory).unwrap().mode() & 0o777, 0o700);
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(location.prepare().is_err());
        fs::remove_dir(&directory).unwrap();
        symlink(temporary.path(), &directory).unwrap();
        assert!(location.prepare().is_err());
        fs::remove_file(&directory).unwrap();
        fs::write(&directory, "not a directory").unwrap();
        assert!(location.prepare().is_err());
    }

    #[test]
    fn build_format() {
        let temporary = tempfile::tempdir().unwrap();
        let executable = temporary.path().join("binary");
        fs::write(&executable, "abc").unwrap();
        let file = fs::File::open(&executable).unwrap();
        file.set_times(fs::FileTimes::new().set_modified(
            std::time::UNIX_EPOCH + std::time::Duration::new(1_700_000_000, 123),
        )).unwrap();
        let alias = temporary.path().join("alias");
        symlink(&executable, &alias).unwrap();
        assert_eq!(build(&alias), Some(format!("{}|1700000000.123|3", fs::canonicalize(&executable).unwrap().display())));
        assert_eq!(build(&temporary.path().join("missing")), None);
    }

    #[test]
    fn relative_insert_cases() {
        let temporary = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temporary.path()).unwrap();
        fs::create_dir(root.join("x")).unwrap();
        fs::create_dir(root.join("y")).unwrap();
        let root = root.to_str().unwrap();
        assert_eq!(relative_insert("x/a.swift", root, &format!("{root}/x")), "a.swift");
        assert_eq!(relative_insert("x/b.swift", root, &format!("{root}/y")), "../x/b.swift");
        assert_eq!(relative_insert("x/b.swift", root, "/"), "x/b.swift");
        assert_eq!(relative_insert("x/b.swift", root, root), "x/b.swift");
        assert_eq!(relative_insert("", root, root), "");
        symlink(format!("{root}/x"), format!("{root}/alias")).unwrap();
        assert_eq!(relative_insert("x/a.swift", root, &format!("{root}/alias")), "a.swift");
    }
}
