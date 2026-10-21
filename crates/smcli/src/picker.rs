//! Inline, controlling-terminal picker. Callers own search, persistence and shell quoting.
use anyhow::{Context, Result};
use nucleo_matcher::{
    Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};
use ratatui::{
    Terminal, TerminalOptions, Viewport,
    backend::CrosstermBackend,
    crossterm::{
        cursor,
        event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
        execute,
        terminal::{disable_raw_mode, enable_raw_mode},
    },
    layout::Rect,
    style::{Color, Style},
    widgets::{List, ListItem, ListState, Paragraph},
};
use smcli_core::service_protocol::FileHit;
use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
    path::Path,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    #[default]
    History,
    Files,
    Grep,
    Directory,
}
impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::History => "history",
            Self::Files => "files",
            Self::Grep => "grep",
            Self::Directory => "directory",
        }
    }
}
#[derive(Debug, Clone)]
pub enum Choice {
    Command {
        command: String,
        query: String,
        semantic: bool,
    },
    File {
        hit: FileHit,
        mode: Mode,
        query: String,
    },
}
#[derive(Debug)]
pub struct FileResults {
    pub hits: Vec<FileHit>,
    pub complete: bool,
}
impl Default for FileResults {
    fn default() -> Self {
        Self {
            hits: Vec::new(),
            complete: true,
        }
    }
}

pub struct Picker {
    pub candidates: Vec<String>,
    pub suggestion_count: usize,
    pub query: String,
    pub mode: Mode,
    pub local_files: Vec<FileHit>,
}
impl Picker {
    /// Native history is oldest first; captured history is newest first.
    pub fn new(
        suggestions: Vec<String>,
        recent: Vec<String>,
        native: Vec<String>,
        query: String,
    ) -> Self {
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();
        let mut add = |command: String| {
            let command = command.trim();
            if !command.is_empty()
                && !smcli_core::parse::is_smcli_command(command)
                && seen.insert(command.to_owned())
            {
                candidates.push(command.to_owned());
            }
        };
        for command in suggestions {
            add(command);
        }
        let suggestion_count = seen.len();
        for command in recent.into_iter().chain(native.into_iter().rev()) {
            let command = command.trim();
            if !command.is_empty()
                && !smcli_core::parse::is_smcli_command(command)
                && seen.insert(command.to_owned())
            {
                candidates.push(command.to_owned());
            }
        }
        Self {
            candidates,
            suggestion_count,
            query,
            mode: Mode::History,
            local_files: Vec::new(),
        }
    }
}

/// Search is invoked only on Tab; files is invoked for project filenames and grep.
/// A file choice is unquoted: the CLI must validate the shell insertion context and quote it.
pub fn run(
    mut picker: Picker,
    mut search: Option<&mut dyn FnMut(&str) -> Result<Vec<String>>>,
    mut files: Option<&mut dyn FnMut(Mode, &str) -> Result<FileResults>>,
) -> Result<Option<Choice>> {
    let mut terminal = TerminalSession::new(14)?;
    let mut saved = HashMap::new();
    let mut selected = 0usize;
    let mut semantic: Option<(String, Vec<String>)> = None;
    let mut global_files = false;
    let mut kind = if picker.mode == Mode::Directory { 1 } else { 0 };
    let mut fetched = None;
    let mut hits = Vec::new();
    let mut status = String::new();
    loop {
        let local = matches!(picker.mode, Mode::Files | Mode::Directory) && !global_files;
        if local {
            hits = picker
                .local_files
                .iter()
                .filter(|h| {
                    (kind == 0 || h.insert.ends_with('/') == (kind == 1))
                        && contains_terms(&h.insert, &picker.query)
                })
                .cloned()
                .collect();
            fetched = None;
        } else if picker.mode != Mode::History
            && fetched.as_ref() != Some(&(picker.mode, picker.query.clone()))
        {
            fetched = Some((picker.mode, picker.query.clone()));
            hits.clear();
            selected = 0;
            if picker.mode == Mode::Grep && picker.query.is_empty() {
                status = "type to search file contents".into();
            } else if let Some(callback) = files.as_mut() {
                match callback(
                    if picker.mode == Mode::Directory {
                        Mode::Files
                    } else {
                        picker.mode
                    },
                    &picker.query,
                ) {
                    Ok(result) => {
                        hits = result.hits;
                        status = if result.complete {
                            String::new()
                        } else {
                            "partial grep results · type more to narrow".into()
                        };
                    }
                    Err(error) => status = error.to_string(),
                }
            } else {
                status = "files and grep need smcli-service".into();
            }
        }
        let semantic_active = semantic.as_ref().is_some_and(|(q, _)| q == &picker.query);
        let commands = if semantic_active {
            semantic.as_ref().unwrap().1.clone()
        } else {
            filter_history(&picker.candidates, &picker.query)
        };
        let count = if picker.mode == Mode::History {
            commands.len()
        } else {
            hits.len()
        };
        selected = selected.min(count.saturating_sub(1));
        // Repaint everything each frame like Swift (ESC[2J): diffed updates leave stale text between new words.
        // Terminal::clear would query the cursor position, which some ptys never answer; two swaps blank both buffers.
        execute!(terminal.terminal.backend_mut(), ratatui::crossterm::terminal::Clear(ratatui::crossterm::terminal::ClearType::All))?;
        terminal.terminal.swap_buffers();
        terminal.terminal.swap_buffers();
        terminal.terminal.draw(|frame| {
            let area = frame.area();
            let label = if local { match kind { 1 => "folders", 2 => "files", _ => "all" } } else { picker.mode.as_str() };
            frame.render_widget(Paragraph::new(format!("{label} ❯ {}", visible(&picker.query))).style(Style::default().fg(Color::Magenta)), Rect::new(area.x, area.y, area.width, 1));
            let history = picker.mode == Mode::History;
            // Dim like Swift; styled spaces also differ from blank cells, so ratatui writes lines whole.
            let dim = Style::default().add_modifier(ratatui::style::Modifier::DIM);
            let found = if local {
                format!("{count} in {} · tab searches the project", std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default())
            } else {
                format!("{count} {label} results")
            };
            let summary = if history {
                visible(&status)
            } else if hits.is_empty() && !local {
                if status.is_empty() { "no matches".into() } else { visible(&status) }
            } else if status.is_empty() { found } else { visible(&status) };
            frame.render_widget(Paragraph::new(format!(" {summary}")).style(dim), Rect::new(area.x, area.y + 1, area.width, 1));
            let items: Vec<ListItem> = if picker.mode == Mode::History {
                commands.iter().map(|c| {
                    let mark = if picker.candidates[..picker.suggestion_count].contains(c) { "★" } else { " " };
                    ListItem::new(format!("{mark} {}", visible(c)))
                }).collect()
            } else {
                hits.iter().map(|h| {
                    let name = if local { &h.insert } else { &h.path };
                    let location = h.line.map(|n| format!(":{n}  {}", h.text.as_deref().unwrap_or(""))).unwrap_or_default();
                    ListItem::new(visible(&format!("{} {name}{location}  {}", h.git.as_deref().unwrap_or(""), h.provenance.join("+"))))
                }).collect()
            };
            let list = List::new(items).highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White)).highlight_symbol("▌ ");
            let mut state = ListState::default().with_selected((count > 0).then_some(selected));
            frame.render_stateful_widget(list, Rect::new(area.x, area.y + 2, area.width, area.height.saturating_sub(3)), &mut state);
            frame.render_widget(Paragraph::new(if history {
                " type to search · ↑↓ move · enter select · tab semantic · ^T files · ^G grep · esc cancel".to_string()
            } else {
                format!(" type to search · ↑↓ move · enter insert path · {}{} · esc cancel",
                    if local { "tab search all · ^F dirs/files · " } else { "" },
                    if picker.mode == Mode::Files { "^T history · ^G grep" } else { "^G history · ^T files" })
            }).style(dim), Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1));
        })?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return Ok(None),
            KeyCode::Char('c' | 'd') if ctrl => return Ok(None),
            KeyCode::Up => selected = selected.saturating_sub(1),
            KeyCode::Char('p' | 'r') if ctrl => selected = selected.saturating_sub(1),
            KeyCode::Down => selected = (selected + 1).min(count.saturating_sub(1)),
            KeyCode::Char('n' | 's') if ctrl => {
                selected = (selected + 1).min(count.saturating_sub(1))
            }
            KeyCode::Enter => {
                return Ok(if count == 0 {
                    None
                } else if picker.mode == Mode::History {
                    Some(Choice::Command {
                        command: commands[selected].clone(),
                        query: picker.query,
                        semantic: semantic_active,
                    })
                } else {
                    Some(Choice::File {
                        hit: hits[selected].clone(),
                        mode: picker.mode,
                        query: picker.query,
                    })
                });
            }
            KeyCode::Char('t' | 'g') if ctrl => {
                saved.insert(picker.mode, picker.query);
                let target = if key.code == KeyCode::Char('t') {
                    Mode::Files
                } else {
                    Mode::Grep
                };
                picker.mode = if picker.mode == target
                    || (target == Mode::Files && picker.mode == Mode::Directory)
                {
                    Mode::History
                } else {
                    target
                };
                picker.query = saved.get(&picker.mode).cloned().unwrap_or_default();
                selected = 0;
                status.clear();
            }
            KeyCode::Char('f') if ctrl && local => {
                kind = (kind + 1) % 3;
                selected = 0;
            }
            KeyCode::Char('u') if ctrl => {
                picker.query.clear();
                global_files = false;
                selected = 0;
                status.clear();
            }
            KeyCode::Char('h') if ctrl => {
                picker.query.pop();
                selected = 0;
                status.clear();
            }
            KeyCode::Backspace => {
                picker.query.pop();
                selected = 0;
                status.clear();
            }
            KeyCode::Tab if local => {
                global_files = true;
                selected = 0;
            }
            KeyCode::Tab if picker.mode == Mode::History && !picker.query.trim().is_empty() => {
                if let Some(callback) = search.as_mut() {
                    match callback(&picker.query) {
                        Ok(results) => {
                            semantic = Some((picker.query.clone(), results));
                            selected = 0;
                            status = "semantic results · type to filter again".into();
                        }
                        Err(error) => status = error.to_string(),
                    }
                }
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                picker.query.push(c);
                selected = 0;
                status.clear();
            }
            _ => {}
        }
    }
}

fn contains_terms(text: &str, query: &str) -> bool {
    let text = text.to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|word| text.contains(word))
}

/// Preserve recency/suggestion order; nucleo does Unicode-aware substring matching.
fn filter_history(candidates: &[String], query: &str) -> Vec<String> {
    let pattern = Pattern::new(
        query,
        CaseMatching::Ignore,
        Normalization::Smart,
        nucleo_matcher::pattern::AtomKind::Substring,
    );
    let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT);
    let mut buffer = Vec::new();
    candidates
        .iter()
        .filter(|c| {
            pattern
                .score(Utf32Str::new(c, &mut buffer), &mut matcher)
                .is_some()
        })
        .cloned()
        .collect()
}

pub fn directory_entries(directory: &Path, limit: usize) -> Vec<FileHit> {
    let mut entries: Vec<_> = std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() != ".git" && e.file_name() != ".DS_Store")
        .map(|e| {
            let metadata = e.metadata().ok();
            (
                e.file_name().to_string_lossy().into_owned(),
                metadata.as_ref().is_some_and(|m| m.is_dir()),
                metadata.and_then(|m| m.modified().ok()),
            )
        })
        .collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
    entries
        .into_iter()
        .take(limit)
        .map(|(name, directory, _)| FileHit {
            insert: format!("{name}{}", if directory { "/" } else { "" }),
            path: name,
            provenance: vec!["here".into()],
            git: None,
            line: None,
            column: None,
            text: None,
        })
        .collect()
}

/// Previously selected paths that still exist lead the local directory entries.
pub fn with_picked(
    scores: &HashMap<String, f64>,
    root: &Path,
    directory: &Path,
    entries: Vec<FileHit>,
    limit: usize,
) -> Vec<FileHit> {
    let mut scores: Vec<_> = scores
        .iter()
        .filter(|(path, _)| root.join(path).exists())
        .collect();
    scores.sort_by(|a, b| b.1.total_cmp(a.1).then(a.0.cmp(b.0)));
    let directory = directory
        .canonicalize()
        .unwrap_or_else(|_| directory.to_path_buf());
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut picked: Vec<_> = scores
        .into_iter()
        .take(limit)
        .map(|(path, _)| {
            let absolute = root.join(path);
            let target: Vec<_> = absolute.components().collect();
            let from: Vec<_> = directory.components().collect();
            let common = target.iter().zip(&from).take_while(|(a, b)| a == b).count();
            let insert = std::iter::repeat_n(std::path::Component::ParentDir, from.len() - common)
                .chain(target[common..].iter().copied())
                .collect::<std::path::PathBuf>()
                .to_string_lossy()
                .into_owned();
            FileHit {
                path: path.clone(),
                insert,
                provenance: vec!["picked".into()],
                git: None,
                line: None,
                column: None,
                text: None,
            }
        })
        .collect();
    let shown: HashSet<_> = picked.iter().map(|hit| hit.insert.clone()).collect();
    picked.extend(
        entries
            .into_iter()
            .filter(|hit| !shown.contains(&hit.insert)),
    );
    picked
}

/// Never pass repository/history control characters through to the terminal.
pub(crate) fn visible(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\n' => "↵ ".into(),
            '\t' => "  ".into(),
            '\x00'..='\x1f' => format!("^{}", char::from_u32(c as u32 + 64).unwrap()),
            '\x7f' => "^?".into(),
            '\u{80}'..='\u{9f}' => format!("<U+{:04X}>", c as u32),
            _ => c.to_string(),
        })
        .collect()
}

pub(crate) struct TerminalSession {
    pub terminal: Terminal<CrosstermBackend<File>>,
}
impl TerminalSession {
    pub fn new(height: u16) -> Result<Self> {
        let tty = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .context("pick needs a terminal")?;
        enable_raw_mode()?;
        // Alternate screen like the Swift picker; an inline viewport queries the cursor position, which some ptys never answer.
        let mut tty = tty;
        let _ = height;
        let terminal = execute!(tty, ratatui::crossterm::terminal::EnterAlternateScreen)
            .map_err(anyhow::Error::from)
            .and_then(|_| {
                // Like Swift: a terminal reporting no size (some ptys) is drawn as 24x80.
                // shortcut: the size is read once, so a resize mid-pick is not followed; re-read per frame if that matters.
                let (columns, rows) = ratatui::crossterm::terminal::size().ok().filter(|&(_, rows)| rows > 0).unwrap_or((80, 24));
                let viewport = Viewport::Fixed(ratatui::layout::Rect::new(0, 0, columns, rows));
                Ok(Terminal::with_options(CrosstermBackend::new(tty), TerminalOptions { viewport })?)
            });
        match terminal {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                let _ = disable_raw_mode();
                Err(error)
            }
        }
    }
}
impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = execute!(self.terminal.backend_mut(), ratatui::crossterm::terminal::LeaveAlternateScreen, cursor::Show);
        let _ = disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_order_filter_and_safe_display() {
        let p = Picker::new(
            vec!["git status".into()],
            vec!["cargo test".into(), "git status".into()],
            vec!["old".into(), "new".into()],
            String::new(),
        );
        assert_eq!(p.candidates, ["git status", "cargo test", "new", "old"]);
        assert_eq!(filter_history(&p.candidates, "GIT STA"), ["git status"]);
        assert_eq!(filter_history(&p.candidates, "status git"), ["git status"]);
        assert_eq!(visible("a\x1b[2J\n\u{9b}"), "a^[[2J↵ <U+009B>");
        assert!(filter_history(&p.candidates, "unmatched").is_empty());
        assert!(FileResults::default().complete);
    }

    #[test]
    fn directory_and_picked_entries() {
        let root = std::env::temp_dir().join(format!(
            "smcli-picker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("folder")).unwrap();
        for name in ["file", ".hidden", ".git", ".DS_Store"] {
            std::fs::write(root.join(name), "").unwrap();
        }
        let entries = directory_entries(&root, 200);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].insert, "folder/");
        assert!(entries.iter().any(|h| h.path == ".hidden"));
        let scores = HashMap::from([("file".into(), 2.0), ("missing".into(), 100.0)]);
        let picked = with_picked(&scores, &root, &root, entries, 10);
        assert_eq!(picked[0].insert, "file");
        assert_eq!(picked[0].provenance, ["picked"]);
        assert_eq!(picked.len(), 3);
        let picked = with_picked(&scores, &root, &root.join("folder"), Vec::new(), 10);
        assert_eq!(picked[0].insert, "../file");
        assert!(contains_terms("Hello World/", "world HELLO"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
