//! Request-scoped, positive-only filesystem evidence.
use std::collections::HashMap;
use std::ffi::CString;
use std::io::Read;
use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, mpsc};
use std::time::Duration;

use smcli_core::parse::shell_words;

use crate::corpus::{Hit, SearchSignals};

#[cfg(test)]
mod index_tests {
    use super::*;
    use smcli_core::entry::SearchEntry;

    struct All;
    impl crate::ProjectEvidence for All {
        fn exists(&self, paths: &[PathBuf]) -> Vec<bool> { vec![true; paths.len()] }
    }

    #[test]
    fn index_and_probe_share_presence_rules() {
        struct Disk;
        impl crate::ProjectEvidence for Disk {
            fn exists(&self, paths: &[PathBuf]) -> Vec<bool> { paths.iter().map(|p| p.exists()).collect() }
        }
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("input.txt"), "data").unwrap();
        let commands: Vec<String> = ["cat input.txt", "vim ./input.txt", "cat missing.txt", "touch input.txt", "cat '*.txt'", "cat -n input.txt"]
            .into_iter().map(String::from).collect();
        let hits = commands.iter().map(|c| (SearchEntry::new(c, 1, "native"), 1.0)).collect::<Vec<_>>();
        let signals = SearchSignals { directory: Some(root.path().to_string_lossy().into_owned()), ..Default::default() };
        assert_eq!(evidence_with_index(&hits, &signals, false, true, Some(&Disk)).into_iter().map(|(a, p, _)| (a, p)).collect::<Vec<_>>(),
            evaluate(&commands, None, signals.directory.as_deref()));
    }

    #[test]
    fn index_covers_the_whole_pool_and_keeps_path_rules() {
        let signals = SearchSignals { directory: Some("/project".into()), ..Default::default() };
        let mut hits: Vec<Hit> = (0..200).map(|i| (SearchEntry::new(&format!("cat src/{i}.rs"), 1, "native"), 1.0)).collect();
        let result = evidence_with_index(&hits, &signals, false, true, Some(&All));
        assert_eq!(result.len(), 200);
        assert!(result.iter().all(|r| r.1));
        hits.push((SearchEntry::new("touch output.txt", 1, "native"), 1.0));
        hits.push((SearchEntry::new("cat '$HOME/x.rs'", 1, "native"), 1.0));
        assert!(evidence_with_index(&hits, &signals, false, true, Some(&All))[200..].iter().all(|r| !r.1));
        let mut disabled = signals;
        disabled.context = Some(smcli_core::ranking::RankingContext { off: [smcli_core::ranking::Signal::Index, smcli_core::ranking::Signal::Git].into(), ..Default::default() });
        assert_eq!(evidence_with_index(&hits, &disabled, false, true, Some(&All)), evidence(&hits, &disabled, false, true));
    }
}

const POOL: usize = 40;
const PATHS: usize = 4;
const WAIT: Duration = Duration::from_millis(8);
/// One probe thread. A second request waits for a healthy probe instead of being dropped.
const PROBES: usize = 1;

struct Gate {
    inflight: usize,
    hung: bool,
}
static GATE: Mutex<Gate> = Mutex::new(Gate { inflight: 0, hung: false });
static GATE_CV: Condvar = Condvar::new();

fn lock_gate() -> std::sync::MutexGuard<'static, Gate> {
    GATE.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Take the probe slot. A probe still running after the wait is hung: do not start another.
fn acquire_probe() -> bool {
    let mut gate = lock_gate();
    loop {
        if gate.hung {
            return false;
        }
        if gate.inflight < PROBES {
            gate.inflight += 1;
            return true;
        }
        let (next, _) = GATE_CV.wait_timeout(gate, WAIT).unwrap_or_else(|poison| poison.into_inner());
        gate = next;
    }
}

fn release_probe() {
    let mut gate = lock_gate();
    gate.inflight = gate.inflight.saturating_sub(1);
    if gate.inflight == 0 {
        gate.hung = false;
    }
    drop(gate);
    GATE_CV.notify_all();
}

/// A stuck filesystem keeps its one thread. A concurrent request waits out a healthy probe, then runs.
pub(crate) fn evidence(hits: &[Hit], signals: &SearchSignals, path: bool, files: bool) -> Vec<(bool, bool, f64)> {
    evidence_with_index(hits, signals, path, files, None)
}

pub(crate) fn evidence_with_index(hits: &[Hit], signals: &SearchSignals, path: bool, files: bool,
                                  index: Option<&dyn crate::ProjectEvidence>) -> Vec<(bool, bool, f64)> {
    let use_index = files && smcli_core::ranking::uses(signals.context.as_ref(), smcli_core::ranking::Signal::Index);
    let use_git = files && smcli_core::ranking::uses(signals.context.as_ref(), smcli_core::ranking::Signal::Git);
    if let Some(index) = index.filter(|_| use_index || use_git) {
        let mut result = evidence(hits, signals, path, false);
        result.resize(hits.len(), (false, false, 0.0));
        if let Some(cwd) = signals.directory.as_deref().map(Path::new).filter(|p| p.is_absolute()) {
            let paths: Vec<Vec<PathBuf>> = hits.iter().map(|(entry, _)| {
                shell_words(entry.primary()).map(|words| presence_paths(&words, cwd)).unwrap_or_default()
            }).collect();
            let flat: Vec<PathBuf> = paths.iter().flatten().cloned().collect();
            if use_index {
                let found = index.exists(&flat);
                let mut offset = 0;
                for (result, paths) in result.iter_mut().zip(&paths) {
                    result.1 = (offset..offset + paths.len()).any(|i| found.get(i).copied().unwrap_or(false));
                    offset += paths.len();
                }
            }
            if use_git {
                let activity = index.git_activity(&flat);
                let mut offset = 0;
                for (result, paths) in result.iter_mut().zip(&paths) {
                    result.2 = (offset..offset + paths.len()).filter_map(|i| activity.get(i).copied()).fold(0.0, f64::max);
                    offset += paths.len();
                }
            }
        }
        return result;
    }
    let count = hits.len().min(POOL);
    let neutral = vec![(false, false, 0.0); count];
    if count == 0 || (!path && !files) || !acquire_probe() {
        return neutral;
    }
    let commands: Vec<String> = hits.iter().take(POOL)
        .map(|(e, _)| if e.primary().len() <= 32_768 { e.primary().to_string() } else { String::new() }).collect();
    let snapshot = signals.shell_path.clone();
    let directory = signals.directory.clone();
    let (send, receive) = mpsc::sync_channel(1);
    if std::thread::Builder::new().name("ranking-fs".into()).spawn(move || {
        struct Release;
        impl Drop for Release {
            fn drop(&mut self) { release_probe(); }
        }
        let _release = Release;
        let result = evaluate(&commands, snapshot.as_deref().filter(|_| path), directory.as_deref().filter(|_| files));
        let _ = send.send(result);
    }).is_err() {
        release_probe();
        return neutral;
    }
    match receive.recv_timeout(WAIT) {
        Ok(result) => result.into_iter().map(|(a, p)| (a, p, 0.0)).collect(),
        Err(_) => {
            let mut gate = lock_gate();
            if gate.inflight == 0 {
                drop(gate);
                return match receive.try_recv() {
                    Ok(result) => result.into_iter().map(|(a, p)| (a, p, 0.0)).collect(),
                    Err(_) => neutral,
                };
            }
            gate.hung = true;
            drop(gate);
            GATE_CV.notify_all();
            neutral
        }
    }
}

fn evaluate(commands: &[String], snapshot: Option<&str>, directory: Option<&str>) -> Vec<(bool, bool)> {
    let mut programs = HashMap::new();
    let mut local_ok = HashMap::new();
    commands.iter().take(POOL).map(|command| {
        let Some(words) = shell_words(command) else { return (false, false) };
        let Some(program) = words.first() else { return (false, false) };
        let available = if let Some(known) = programs.get(program) {
            *known
        } else {
            let known = snapshot.is_some_and(|p| available_in(program, p, &mut local_ok));
            programs.insert(program.clone(), known);
            known
        };
        let present = directory.is_some_and(|cwd| present_in(&words, Path::new(cwd), &mut local_ok));
        (available, present)
    }).collect()
}

#[cfg(test)]
fn available(program: &str, snapshot: &str) -> bool {
    available_in(program, snapshot, &mut HashMap::new())
}

fn available_in(program: &str, snapshot: &str, local_ok: &mut HashMap<PathBuf, bool>) -> bool {
    if program.contains('/') || !program.chars().all(|c| c.is_ascii_alphanumeric() || "-_+.".contains(c)) || builtin(program) {
        return false;
    }
    // Empty entries (trailing ':' or '::') are skipped. A relative or oversized snapshot is unknown.
    let dirs: Vec<&str> = snapshot.split(':').filter(|dir| !dir.is_empty()).collect();
    if snapshot.len() > smcli_core::service_protocol::MAX_SHELL_PATH_BYTES || dirs.len() > 64 || dirs.iter().any(|dir| !Path::new(dir).is_absolute()) {
        return false;
    }
    for dir in dirs {
        let dir = Path::new(dir);
        if !is_local(dir, local_ok) {
            if dir.symlink_metadata().is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) { continue; }
            return false;
        }
        let candidate = dir.join(program);
        if !is_local(&candidate, local_ok) {
            if candidate.symlink_metadata().is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) { continue; }
            return false;
        }
        let Ok(meta) = candidate.metadata() else { continue };
        if !meta.is_file() { continue; }
        let Ok(name) = CString::new(candidate.as_os_str().as_bytes()) else { continue };
        // SAFETY: name is a valid C string; access checks this user's executable permission.
        if unsafe { libc::access(name.as_ptr(), libc::X_OK) } != 0 { continue; }
        // The first executable wins: do not look past it to another binary.
        let Ok(mut file) = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(&candidate) else { return false };
        let mut buf = [0; 256];
        let Ok(n) = file.read(&mut buf) else { return false };
        if n < 2 { return false; }
        // An empty shebang is a shim. A script with a body is a real tool (npm, pip, brew).
        return !buf.starts_with(b"#!") || script_body(&buf[..n], n == buf.len());
    }
    false
}

/// True when a shebang file has a payload past its first line. A full buffer with no newline may still have one.
fn script_body(buf: &[u8], full: bool) -> bool {
    match buf.iter().position(|b| *b == b'\n') {
        Some(i) => buf[i + 1..].iter().any(|b| !b.is_ascii_whitespace()),
        None => full,
    }
}

fn builtin(program: &str) -> bool {
    matches!(program, "." | ":" | "alias" | "bg" | "bind" | "break" | "builtin" | "cd" | "command" | "continue" |
        "declare" | "dirs" | "disown" | "echo" | "enable" | "eval" | "exec" | "exit" | "export" | "false" |
        "fc" | "fg" | "getopts" | "hash" | "help" | "history" | "jobs" | "kill" | "let" | "local" | "logout" |
        "popd" | "printf" | "pushd" | "pwd" | "read" | "readonly" | "return" | "set" | "shift" | "source" |
        "test" | "times" | "trap" | "true" | "type" | "typeset" | "ulimit" | "umask" | "unalias" | "unset" | "wait")
}

#[cfg(test)]
fn present(words: &[String], cwd: &Path) -> bool {
    present_in(words, cwd, &mut HashMap::new())
}

fn present_in(words: &[String], cwd: &Path, local_ok: &mut HashMap<PathBuf, bool>) -> bool {
    presence_paths(words, cwd).into_iter().any(|path| is_local(&path, local_ok) && path.metadata().is_ok())
}

fn presence_paths(words: &[String], cwd: &Path) -> Vec<PathBuf> {
    if words.is_empty() || !cwd.is_absolute() || !matches!(words[0].as_str(), "cat" | "bat" | "less" | "more" | "head" | "tail" | "wc" |
        "file" | "vim" | "nvim" | "vi" | "nano" | "emacs" | "hx" | "code" | "open" | "source" | ".") {
        return vec![];
    }
    words.iter().skip(1)
        .filter(|word| !word.starts_with('-') && !word.contains(|c: char| "$*?~![]{}<>|&;`\\".contains(c)))
        .filter(|word| word.contains('/') || word.contains('.') || word.contains(' '))
        .take(PATHS)
        .map(|word| cwd.join(word)).collect()
}

fn is_local(path: &Path, cache: &mut HashMap<PathBuf, bool>) -> bool {
    if let Some(known) = cache.get(path) {
        return *known;
    }
    let known = local(path);
    cache.insert(path.to_path_buf(), known);
    known
}

/// Check mount locality before probing metadata. Unknown filesystem types are neutral.
/// This check itself runs only on the bounded worker: even statfs can hang on a dead mount.
fn local(path: &Path) -> bool {
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else { return false };
    let mut info = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: the C string and output buffer are valid for statfs; read only after success.
    if unsafe { libc::statfs(path.as_ptr(), info.as_mut_ptr()) } != 0 { return false; }
    let info = unsafe { info.assume_init() };
    #[cfg(target_os = "macos")]
    { info.f_flags & libc::MNT_LOCAL as u32 != 0 }
    #[cfg(target_os = "linux")]
    { matches!(info.f_type as u64, 0xef53 | 0x58465342 | 0x9123683e | 0x01021994 | 0x794c7630 | 0x2fc12fc1) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn bounded_pool_paths_and_busy_worker_are_neutral() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("exists.md"), "").unwrap();
        let commands = vec!["cat exists.md".to_string(); POOL + 1];
        assert_eq!(evaluate(&commands, None, root.path().to_str()).len(), POOL);
        assert!(!present(&shell_words("cat a.md b.md c.md d.md exists.md").unwrap(), root.path()));
        #[cfg(target_os = "linux")]
        assert!(!local(Path::new("/proc/self/status")));
    }

    #[test]
    fn path_and_file_guards_are_positive_and_request_scoped() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        for (name, body) in [("tool", "binary"), ("cat", "binary"), ("wrapper", "#!/bin/sh\n"), ("echo", "binary")] {
            let p = bin.join(name);
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let snapshot = bin.to_str().unwrap();
        assert!(available("tool", snapshot));
        assert!(available("tool", &format!("{}:{snapshot}", root.path().join("missing-bin").display())));
        assert!(!available("tool", &format!(".:{snapshot}")));
        assert!(!available("tool", &"x".repeat(8_193)));
        for name in ["wrapper", "echo", "unknown", "/bin/tool", "function"] { assert!(!available(name, snapshot), "{name}"); }
        let a = root.path().join("a");
        let b = root.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        std::fs::write(a.join("my notes.md"), "").unwrap();
        let check = |command: &str, cwd: &Path| shell_words(command).is_some_and(|words| present(&words, cwd));
        assert!(check("cat 'my notes.md'", &a));
        assert!(!check("cat 'my notes.md'", &b));
        for command in ["mkdir 'my notes.md'", "cat *.md", "cat $HOME", "cat ~/x.md", "cat x.md > out", "cat $(echo x)", "cat x.md | wc"] {
            assert!(!check(command, &a), "{command}");
        }
        std::fs::rename(a.join("my notes.md"), a.join("new.md")).unwrap();
        assert!(!check("cat 'my notes.md'", &a));
        assert!(check("head -n 20 new.md", &a));
        std::fs::remove_file(a.join("new.md")).unwrap();
        assert!(!check("cat new.md", &a));

        std::fs::write(a.join("signal.md"), "").unwrap();
        let hits = vec![(smcli_core::entry::SearchEntry::new("cat signal.md", 1, "native"), 1.0)];
        let signals = SearchSignals {
            shell_path: Some(snapshot.into()), directory: Some(a.to_string_lossy().into_owned()), ..Default::default()
        };
        let ranked = crate::corpus::rerank(hits, false, "inspect", 0.0, &signals);
        assert_eq!(ranked[0].why.as_ref().unwrap(), &["semantic match", "program found in PATH", "input file exists"]);
    }

    #[test]
    fn scripts_are_available_and_empty_path_entries_are_skipped() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        for (name, body) in [("tool", "binary"), ("wrapper", "#!/bin/sh\n"), ("npm-cli", "#!/usr/bin/env node\nconsole.log(1)\n")] {
            let path = bin.join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let snapshot = bin.to_str().unwrap();
        assert!(available("tool", snapshot));
        assert!(available("npm-cli", snapshot), "a shebang with a body is a tool, not a shim");
        assert!(!available("wrapper", snapshot), "an empty shebang is a shim");
        assert!(available("tool", &format!("{snapshot}:")));
        assert!(available("tool", &format!(":{snapshot}:")));
        assert!(available("tool", &format!("{snapshot}::{snapshot}")));
        assert!(!available("tool", ":"));
        assert!(!available("tool", &format!(".:{snapshot}")));
    }

    #[test]
    fn concurrent_evidence_calls_agree() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let tool = bin.join("tool");
        std::fs::write(&tool, "binary").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(root.path().join("notes.md"), "").unwrap();
        let hits = vec![
            (smcli_core::entry::SearchEntry::new("tool notes.md", 1, "native"), 1.0),
            (smcli_core::entry::SearchEntry::new("cat notes.md", 1, "native"), 1.0),
        ];
        let signals = SearchSignals {
            shell_path: Some(bin.to_string_lossy().into_owned()),
            directory: Some(root.path().to_string_lossy().into_owned()),
            ..Default::default()
        };
        let one = evidence(&hits, &signals, true, true);
        assert_eq!(one, vec![(true, false, 0.0), (false, true, 0.0)]);
        let (left, right) = std::thread::scope(|scope| {
            let a = scope.spawn(|| evidence(&hits, &signals, true, true));
            let b = scope.spawn(|| evidence(&hits, &signals, true, true));
            (a.join().unwrap(), b.join().unwrap())
        });
        assert_eq!(left, one);
        assert_eq!(right, one);
    }
}
