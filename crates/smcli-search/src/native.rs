use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

/// A native shell history file. Read only: smcli never writes one.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryFile {
    pub path: PathBuf,
    pub shell: Shell,
}

impl HistoryFile {
    pub fn read(&self) -> Result<String> {
        let mut bytes = std::fs::read(&self.path)?;
        if self.shell == Shell::Fish {
            let text = String::from_utf8_lossy(&bytes);
            return Ok(text
                .lines()
                .filter_map(|line| line.strip_prefix("- cmd: "))
                .map(unquote_fish)
                .collect::<Vec<_>>()
                .join("\n"));
        }
        if self.shell == Shell::Zsh {
            // Zsh escapes non-ASCII bytes with Meta (0x83), followed by byte XOR 32.
            let mut decoded = Vec::with_capacity(bytes.len());
            let mut index = 0;
            while index < bytes.len() {
                if bytes[index] == 0x83 && index + 1 < bytes.len() {
                    index += 1;
                    decoded.push(bytes[index] ^ 32);
                } else {
                    decoded.push(bytes[index]);
                }
                index += 1;
            }
            bytes = decoded;
        }
        let text = String::from_utf8_lossy(&bytes);
        let mut lines: Vec<String> = text.split('\n').map(String::from).collect();
        if self.shell == Shell::Bash {
            lines.retain(|l| !(l.len() > 1 && l.starts_with('#') && l[1..].bytes().all(|b| b.is_ascii_digit())));
        } else {
            let mut continued = false;
            let count = lines.len();
            for (index, line) in lines.iter_mut().enumerate() {
                if !continued {
                    *line = strip_extended_prefix(line).to_string();
                }
                continued = index < count - 1 && line.ends_with('\\');
                if continued {
                    line.pop();
                }
            }
        }
        Ok(lines.join("\n"))
    }
}

/// Fish history stores YAML-like records; accept only simple single-line `- cmd:` scalars.
fn unquote_fish(command: &str) -> String {
    let command = command.trim();
    if let Some(value) = command.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        value.replace("''", "'")
    } else if let Some(value) = command.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        value.replace("\\\\", "\\").replace("\\\"", "\"")
    } else {
        // Fish writes `\\` for a backslash and `\n` for a newline in a multi-line command.
        let mut out = String::with_capacity(command.len());
        let mut chars = command.chars();
        while let Some(c) = chars.next() {
            match (c, chars.clone().next()) {
                ('\\', Some('\\')) => { out.push('\\'); chars.next(); }
                ('\\', Some('n')) => { out.push('\n'); chars.next(); }
                _ => out.push(c),
            }
        }
        out
    }
}

/// Removes zsh's `: <start>:<elapsed>;` extended-history prefix.
fn strip_extended_prefix(line: &str) -> &str {
    let Some(rest) = line.strip_prefix(": ") else { return line };
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    let first = digits(rest);
    if first == 0 || rest.as_bytes().get(first) != Some(&b':') {
        return line;
    }
    let rest = &rest[first + 1..];
    let second = digits(rest);
    if second == 0 || rest.as_bytes().get(second) != Some(&b';') {
        return line;
    }
    &rest[second + 1..]
}

pub fn history_files(home: &Path, environment: &HashMap<String, String>) -> Result<Vec<HistoryFile>> {
    let mut files = vec![];
    for (name, shell) in [("bash", Shell::Bash), ("zsh", Shell::Zsh)] {
        let file = home.join(format!(".{name}_history"));
        if file.exists() {
            files.push(HistoryFile { path: file, shell });
        }
        let sessions = home.join(format!(".{name}_sessions"));
        if sessions.is_dir() {
            let Ok(entries) = std::fs::read_dir(&sessions) else { continue };
            let mut entries: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
            entries.sort_by_key(|p| p.file_name().map(|n| n.to_os_string()));
            for entry in entries {
                if matches!(entry.extension().and_then(|e| e.to_str()), Some("history" | "historynew")) {
                    files.push(HistoryFile { path: entry, shell });
                }
            }
        }
    }
    let fish = home.join(".local/share/fish/fish_history");
    if fish.is_file() {
        files.push(HistoryFile { path: fish, shell: Shell::Fish });
    }
    if let Some(path) = environment.get("HISTFILE").filter(|p| !p.is_empty()) {
        let shell = if environment.get("SHELL").is_some_and(|s| s.ends_with("/bash")) { Shell::Bash } else { Shell::Zsh };
        files.push(HistoryFile { path: PathBuf::from(path), shell });
    }
    let mut seen = HashSet::new();
    files.retain(|f| seen.insert(std::fs::canonicalize(&f.path).unwrap_or_else(|_| f.path.clone())));
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_optional_session_directory_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let history = dir.path().join(".bash_history");
        std::fs::write(&history, "echo usable\n").unwrap();
        let sessions = dir.path().join(".bash_sessions");
        std::fs::create_dir(&sessions).unwrap();
        std::fs::write(sessions.join("session.history"), "echo session\n").unwrap();
        std::fs::remove_dir_all(&sessions).unwrap();
        std::fs::write(&sessions, "not a directory").unwrap();
        let files = history_files(dir.path(), &HashMap::new()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].read().unwrap(), "echo usable\n");
    }

    #[test]
    fn reads_fish_commands_without_modifying_native_history() {
        let dir = tempfile::tempdir().unwrap();
        let fish_dir = dir.path().join(".local/share/fish");
        std::fs::create_dir_all(&fish_dir).unwrap();
        let fish = fish_dir.join("fish_history");
        let source = "- cmd: echo plain\n  when: 1700000000\n- cmd: 'echo it''s quoted'\n  when: 1700000001\n\
                      - cmd: printf a\\\\tb\\nls\n  when: 1700000002\n";
        std::fs::write(&fish, source).unwrap();
        let files = history_files(dir.path(), &HashMap::new()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].shell, Shell::Fish);
        assert_eq!(files[0].read().unwrap(), "echo plain\necho it's quoted\nprintf a\\tb\nls");
        assert_eq!(std::fs::read_to_string(fish).unwrap(), source);
    }

    #[test]
    fn read_bash_and_zsh_history() {
        let dir = tempfile::tempdir().unwrap();
        let bash = dir.path().join(".bash_history");
        std::fs::write(&bash, "#1700000000\nls -la\ngit status\n").unwrap();
        let zsh = dir.path().join(".zsh_history");
        std::fs::write(&zsh, b": 1700000000:0;echo \\\nnext\n: 1700000001:2;caf\xc3\x83\x89\n".as_slice()).unwrap();
        let files = history_files(dir.path(), &HashMap::new()).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].read().unwrap(), "ls -la\ngit status\n");
        assert_eq!(files[1].read().unwrap(), "echo \nnext\ncafé\n");
    }
}
