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
    style::{Color, Modifier, Style},
    symbols::border,
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState, Paragraph},
};
use crate::highlight::{highlight, plain};

// ANSI-16 roles only, so the user's terminal theme does the painting.
pub(crate) const MUTED: Style = Style::new().add_modifier(Modifier::DIM);
pub(crate) const ACCENT: Style = Style::new().fg(Color::Magenta);
pub(crate) const SELECTED: Style = Style::new().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD);
use smcli_core::service_protocol::FileHit;
use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    #[default]
    History,
    Files,
    Grep,
}
impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::History => "history",
            Self::Files => "files",
            Self::Grep => "grep",
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
    /// Replaces the "history" prompt label, e.g. for directory suggestions.
    pub title: Option<String>,
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
            title: None,
        }
    }
}

/// Each callback has one worker; dropping the picker never waits for a slow search.
fn worker<Q: Send + 'static, R: Send + 'static>(
    mut callback: impl FnMut(&Q) -> R + Send + 'static,
) -> (mpsc::Sender<Q>, mpsc::Receiver<(Q, R)>) {
    let (requests, incoming) = mpsc::channel::<Q>();
    let (outgoing, results) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(query) = incoming.recv() {
            let result = callback(&query);
            if outgoing.send((query, result)).is_err() { break; }
        }
    });
    (requests, results)
}

/// Cells of the sunset row that show progress. Nothing is filled when there is no row;
/// one row fills the width; otherwise `width * (selected + 1) / count`.
fn stripe_fill(width: u16, selected: usize, count: usize) -> u16 {
    if width == 0 || count == 0 {
        return 0;
    }
    if count <= 1 {
        return width;
    }
    let cells = u128::from(width) * (selected as u128).saturating_add(1) / count as u128;
    u16::try_from(cells).unwrap_or(width).min(width)
}

/// One third of the gap toward `target`, and at least one cell.
fn ease_stripe(shown: u16, target: u16) -> u16 {
    let gap = shown.abs_diff(target);
    if gap == 0 {
        return shown;
    }
    let step = (gap / 3).max(1);
    if shown < target {
        shown.saturating_add(step).min(target)
    } else {
        shown.saturating_sub(step)
    }
}

/// Yellow, red, then magenta across thirds of the full row. A narrow row is magenta, matching the old stripe.
fn stripe_color(col: usize, width: usize) -> Color {
    let third = width / 3;
    if col < third {
        Color::Yellow
    } else if col < third * 2 {
        Color::Red
    } else {
        Color::Magenta
    }
}

fn stripe_line(width: u16, filled: u16, fade: Modifier) -> Line<'static> {
    let width = width as usize;
    let filled = (filled as usize).min(width);
    let mut spans = Vec::new();
    let mut col = 0;
    while col < filled {
        let color = stripe_color(col, width);
        let start = col;
        col += 1;
        while col < filled && stripe_color(col, width) == color {
            col += 1;
        }
        spans.push(Span::styled(
            "▀".repeat(col - start),
            Style::new().fg(color).add_modifier(fade),
        ));
    }
    if filled < width {
        spans.push(Span::styled("▀".repeat(width - filled), MUTED));
    }
    Line::from(spans)
}

/// Keep at least three result rows when possible. As height shrinks, drop
/// decoration, filter hints, navigation hints, then global hints and the header.
fn picker_layout(area: Rect) -> [Rect; 7] {
    let h = area.height;
    let heights = [
        u16::from(h >= 5), // header
        u16::from(h >= 9), // filters
        if h >= 13 { 3 } else { u16::from(h >= 2) }, // search
        u16::from(h >= 8), // navigation
        0, // list takes the remainder
        u16::from(h >= 10), // sunset
        u16::from(h >= 6), // global hints
    ];
    let mut rows = [Rect::default(); 7];
    let mut y = area.y;
    for (i, height) in heights.into_iter().enumerate() {
        let height = if i == 4 { h.saturating_sub(heights.iter().sum()) } else { height };
        rows[i] = Rect::new(area.x, y, area.width, height);
        y += height;
    }
    rows
}

fn hint_line(hints: &[(&str, &str)]) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (i, (key, what)) in hints.iter().enumerate() {
        if i > 0 { spans.push(Span::styled(" · ", MUTED)); }
        spans.push(Span::raw((*key).to_owned()));
        spans.push(Span::styled(format!(" {what}"), MUTED));
    }
    Line::from(spans)
}

/// Search is invoked only on Tab (history) or Ctrl-A (files); files is invoked for project filenames and grep.
/// A file choice is unquoted: the CLI must validate the shell insertion context and quote it.
pub fn run(
    mut picker: Picker,
    search: Option<Box<dyn FnMut(&str) -> Result<Vec<String>> + Send>>,
    files: Option<Box<dyn FnMut(Mode, &str) -> Result<FileResults> + Send>>,
) -> Result<Option<Choice>> {
    let search = search.map(|mut callback| worker(move |q: &String| callback(q)));
    let files = files.map(|mut callback| worker(move |(mode, q, _): &(Mode, String, u8)| {
        callback(*mode, q)
    }));
    let mut file_busy = false;
    let mut semantic_busy = false;
    let mut semantic_pending = None;
    let mut spinner = 0;
    let mut spun = Instant::now();
    let mut terminal = TerminalSession::new()?;
    let mut saved = HashMap::new();
    let mut selected = 0usize;
    let mut semantic: Option<(String, Vec<String>)> = None;
    let mut global_files = false;
    let mut kind = 0;
    let mut fetched = None;
    let mut tick = 0u8;
    let mut fade_at = Instant::now();
    // Displayed sunset width. It eases toward the selection instead of jumping.
    let mut shown = 0u16;
    let mut next_bar = Instant::now() + Duration::from_millis(20);
    let mut cols = 0u16;
    let mut tall = false;
    let mut page_size = 1usize;
    let mut hits = Vec::new();
    let mut status = String::new();
    let mut filtered: Option<((String, bool), Vec<String>)> = None;
    let starred: HashSet<String> = picker.candidates[..picker.suggestion_count].iter().cloned().collect();
    loop {
        let local = picker.mode == Mode::Files && !global_files;
        let current = (picker.mode, picker.query.clone(), kind);
        if let Some((_, replies)) = &files {
            if let Ok((request, result)) = replies.try_recv() {
                file_busy = false;
                if !local && request == current {
                    match result {
                        Ok(result) => {
                            hits = if picker.mode == Mode::Files {
                                filter_kind(result.hits, kind)
                            } else { result.hits };
                            status = if result.complete { String::new() } else {
                                "partial grep results · type more to narrow".into()
                            };
                        }
                        Err(error) => status = error.to_string(),
                    }
                }
            }
        }
        if let Some((requests, replies)) = &search {
            if let Ok((query, result)) = replies.try_recv() {
                semantic_busy = false;
                if picker.mode == Mode::History && query == picker.query && semantic_pending.is_none() {
                    match result {
                        Ok(results) => {
                            semantic = Some((query, results));
                            selected = 0;
                            status = "semantic results · type to filter again".into();
                        }
                        Err(error) => status = error.to_string(),
                    }
                }
            }
            if !semantic_busy {
                if let Some(query) = semantic_pending.take() {
                    if picker.mode == Mode::History && query == picker.query {
                        semantic_busy = requests.send(query).is_ok();
                    }
                }
            }
        }
        if local {
            hits = picker.local_files.iter().filter(|h| {
                (kind == 0 || h.insert.ends_with('/') == (kind == 1))
                    && contains_terms(&h.insert, &picker.query)
            }).cloned().collect();
            fetched = None;
        } else if picker.mode != Mode::History && fetched.as_ref() != Some(&current) {
            hits.clear();
            selected = 0;
            status.clear();
            if picker.mode == Mode::Grep && picker.query.is_empty() {
                fetched = Some(current);
                status = "type to search file contents".into();
            } else if let Some((requests, _)) = &files {
                if !file_busy {
                    file_busy = requests.send(current.clone()).is_ok();
                    fetched = Some(current);
                }
            } else {
                fetched = Some(current);
                status = "files and grep need smcli-service".into();
            }
        }
        let busy = file_busy || semantic_busy;
        if busy && spun.elapsed() >= Duration::from_millis(80) {
            spinner = (spinner + 1) % 10;
            spun = Instant::now();
        }
        let semantic_active = semantic.as_ref().is_some_and(|(q, _)| q == &picker.query);
        let key_now = (picker.query.clone(), semantic_active);
        if filtered.as_ref().map(|(k, _)| k) != Some(&key_now) {
            let commands = if semantic_active {
                semantic.as_ref().unwrap().1.clone()
            } else {
                filter_history(&picker.candidates, &picker.query)
            };
            filtered = Some((key_now, commands));
        }
        let commands = &filtered.as_ref().unwrap().1;
        let count = if picker.mode == Mode::History {
            commands.len()
        } else {
            hits.len()
        };
        selected = selected.min(count.saturating_sub(1));
        // Coalesce key repeat: draw only once queued input is handled, else held arrows pile up behind frames.
        if !event::poll(std::time::Duration::ZERO)? { terminal.terminal.draw(|frame| {
            let area = frame.area();
            cols = area.width;
            let history = picker.mode == Mode::History;
            let label = if picker.mode == Mode::Files { match kind { 1 => "folders", 2 => "files", _ => "files+dirs" } } else if history { picker.title.as_deref().unwrap_or("history") } else { picker.mode.as_str() };
            let [header_area, filter_area, search_area, navigation_area, list_area, stripe_area, global_area] = picker_layout(area);
            let boxed = search_area.height == 3;
            // Files modes name their scope so directory and project search never look alike.
            let scope = match picker.mode {
                Mode::Files if local => format!("in {}", std::env::current_dir().map(|d| visible(&d.display().to_string())).unwrap_or_default()),
                Mode::Files => "across your system".into(),
                Mode::Grep => format!("in {}", std::env::current_dir().map(|d| visible(&d.display().to_string())).unwrap_or_default()),
                _ => String::new(),
            };
            frame.render_widget(Paragraph::new(Line::from(vec![Span::styled(format!(" {label} "), ACCENT), Span::styled(scope, ACCENT.add_modifier(Modifier::BOLD)), Span::styled(" ›", ACCENT.add_modifier(Modifier::BOLD))])), header_area);
            let found = if local {
                format!("{count} results")
            } else {
                format!("{count} {} results", if picker.mode == Mode::Grep { "grep" } else { "files" })
            };
            let summary = if history {
                visible(&status)
            } else if hits.is_empty() && !local {
                if status.is_empty() { "no matches".into() } else { visible(&status) }
            } else if status.is_empty() { found } else { visible(&status) };
            let mut summary = Line::styled(format!(" {summary} "), MUTED);
            if busy {
                summary.spans.insert(0, Span::styled(format!(" {}", ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"][spinner]), ACCENT));
            }
            let search_block = if boxed { Block::bordered().border_set(border::ROUNDED).border_style(MUTED) } else { Block::new() };
            let input = search_block.inner(search_area);
            let query = visible(&picker.query);
            let prompt = " ⌕ ";
            let text = if query.is_empty() {
                Span::styled(format!("type to search {}…", if history { "history" } else { picker.mode.as_str() }), MUTED)
            } else {
                Span::raw(query.clone())
            };
            frame.render_widget(Paragraph::new(Line::from(vec![Span::styled(prompt, ACCENT), text])).block(search_block), search_area);
            if input.width > 0 && input.height > 0 {
                let offset = (Span::raw(prompt).width() + Span::raw(query.as_str()).width()).min(input.width.saturating_sub(1) as usize) as u16;
                frame.set_cursor_position((input.x + offset, input.y));
            }
            tall = stripe_area.height > 0;
            let list_block = if boxed {
                Block::bordered().border_set(border::ROUNDED).border_style(MUTED)
                    .title_bottom(summary.clone().right_aligned())
            } else {
                Block::new()
            };
            page_size = list_block.inner(list_area).height.max(1) as usize;
            let items: Vec<ListItem> = if history {
                // Rows past the selection plus one screen are never visible; skip building them.
                commands.iter().enumerate().take(selected + area.height as usize).map(|(i, c)| {
                    let sel = i == selected;
                    let mark = if starred.contains(c) { "* " } else { "  " };
                    let spans = highlight(&visible(c));
                    let mut line = vec![Span::styled(mark, if sel { Style::new() } else { MUTED })];
                    line.extend(if sel { plain(spans) } else { spans });
                    ListItem::new(Line::from(line))
                }).collect()
            } else {
                hits.iter().map(|h| {
                    let name = if local { &h.insert } else { &h.path };
                    let location = h.line.map(|n| format!(":{n}  {}", h.text.as_deref().unwrap_or(""))).unwrap_or_default();
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("{} ", h.git.as_deref().unwrap_or("")), ACCENT),
                        Span::raw(visible(name)),
                        Span::styled(visible(&location), MUTED),
                        Span::styled(format!("  {}", h.provenance.join("+")), MUTED),
                    ]))
                }).collect()
            };
            let list = List::new(items).block(list_block).highlight_style(SELECTED);
            let mut state = ListState::default().with_selected((count > 0).then_some(selected));
            frame.render_stateful_widget(list, list_area, &mut state);
            let filters = if history {
                vec![("tab", "semantic")]
            } else if picker.mode == Mode::Files {
                vec![("^A", if local { "global search" } else { "cwd search" }), ("^F", "dirs/files")]
            } else {
                vec![]
            };
            frame.render_widget(Paragraph::new(hint_line(&filters)), filter_area);
            frame.render_widget(Paragraph::new(hint_line(&[("[ ⏎ ]", "accept"), ("↑↓", "move"), ("PgUp/PgDn", "page")])), navigation_area);
            let mut globals = vec![("esc", "cancel"), ("^T", if picker.mode == Mode::Files { "history" } else { "files" })];
            if grep_available(global_files) {
                globals.push(("^G", if picker.mode == Mode::Grep { "history" } else { "grep" }));
            }
            globals.push(("^U", "clear"));
            let fade = if tick < 3 { Modifier::DIM } else { Modifier::empty() };
            frame.render_widget(Paragraph::new(stripe_line(area.width, shown, fade)), stripe_area);
            frame.render_widget(Paragraph::new(hint_line(&globals)), global_area);
        })?; }
        // Soonest of bar ease (~20ms), fade-in (70ms), and spinner (80ms). A queued key
        // makes poll return at once, so held arrows still skip frames and coalesce.
        let fill = stripe_fill(cols, selected, count);
        let growing = tall && shown != fill;
        let wait = if growing {
            let mut wait = next_bar.saturating_duration_since(Instant::now());
            if tick < 3 {
                wait = wait.min(Duration::from_millis(70).saturating_sub(fade_at.elapsed()));
            }
            if busy {
                wait = wait.min(Duration::from_millis(80));
            }
            Some(wait)
        } else if tick < 3 {
            Some(Duration::from_millis(70))
        } else if busy {
            Some(Duration::from_millis(80))
        } else {
            None
        };
        if let Some(wait) = wait {
            if !event::poll(wait)? {
                if tick < 3 && (!growing || fade_at.elapsed() >= Duration::from_millis(70)) {
                    tick += 1;
                    fade_at = Instant::now();
                }
                if growing && Instant::now() >= next_bar {
                    shown = ease_stripe(shown, fill);
                    next_bar = Instant::now() + Duration::from_millis(20);
                }
                continue;
            }
        }
        tick = 3;
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
            KeyCode::PageUp => selected = selected.saturating_sub(page_size),
            KeyCode::PageDown => selected = selected.saturating_add(page_size).min(count.saturating_sub(1)),
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
            KeyCode::Char('t' | 'g') if ctrl && (key.code != KeyCode::Char('g') || grep_available(global_files)) => {
                saved.insert(picker.mode, picker.query);
                let target = if key.code == KeyCode::Char('t') {
                    Mode::Files
                } else {
                    Mode::Grep
                };
                picker.mode = if picker.mode == target
                    
                {
                    Mode::History
                } else {
                    target
                };
                picker.query = saved.get(&picker.mode).cloned().unwrap_or_default();
                selected = 0;
                status.clear();
            }
            KeyCode::Char('f') if ctrl && picker.mode == Mode::Files => {
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
            KeyCode::Char('a') if ctrl && picker.mode == Mode::Files => {
                global_files = !global_files;
                selected = 0;
            }
            KeyCode::Tab if picker.mode == Mode::History && !picker.query.trim().is_empty() => {
                if search.is_some() {
                    semantic_pending = Some(picker.query.clone());
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

fn grep_available(global_files: bool) -> bool {
    !global_files
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
            provenance: vec!["*".into()],
            git: None,
            line: None,
            column: None,
            text: None,
        })
        .collect()
}

/// Narrows project hits to files (2) or folders (1); the index lists only files, so folders are their matches' parents.
// shortcut: folders only appear if a file inside them matches; add directory entries to the service index if that's not enough.
fn filter_kind(hits: Vec<FileHit>, kind: u8) -> Vec<FileHit> {
    if kind != 1 {
        return hits;
    }
    let parent = |p: &str| p.rsplit_once('/').map(|(d, _)| format!("{d}/"));
    let mut seen = HashSet::new();
    hits.into_iter()
        .filter_map(|h| {
            let path = parent(&h.path)?;
            seen.insert(path.clone()).then(|| FileHit { insert: parent(&h.insert).unwrap_or_else(|| "./".to_string()), path, git: None, line: None, column: None, text: None, ..h })
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
                provenance: vec!["*".into()],
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
    pub fn new() -> Result<Self> {
        let mut tty = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .context("pick needs a terminal")?;
        enable_raw_mode()?;
        // Alternate screen like the Swift picker; an inline viewport queries the cursor position, which some ptys never answer.
        let mut display = tty.try_clone().context("pick needs a terminal")?;
        let terminal = execute!(display, ratatui::crossterm::terminal::EnterAlternateScreen)
            .map_err(anyhow::Error::from)
            .and_then(|_| {
                // Like Swift: a terminal reporting no size (some ptys) is drawn as 24x80.
                // shortcut: the size is read once, so a resize mid-pick is not followed; re-read per frame if that matters.
                let (columns, rows) = ratatui::crossterm::terminal::size().ok().filter(|&(_, rows)| rows > 0).unwrap_or((80, 24));
                let viewport = Viewport::Fixed(ratatui::layout::Rect::new(0, 0, columns, rows));
                Ok(Terminal::with_options(CrosstermBackend::new(display), TerminalOptions { viewport })?)
            });
        match terminal {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                let _ = execute!(tty, ratatui::crossterm::terminal::LeaveAlternateScreen);
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
    fn grep_is_only_available_outside_global_search() {
        assert!(grep_available(false));
        assert!(!grep_available(true));
    }
    #[test]
    fn project_folders_from_file_parents() {
        let hit = |path: &str, insert: &str| FileHit { insert: insert.into(), path: path.into(), provenance: vec!["files".into()], git: None, line: None, column: None, text: None };
        let dirs = filter_kind(vec![hit("src/a.rs", "../src/a.rs"), hit("src/b.rs", "../src/b.rs"), hit("top.rs", "../top.rs")], 1);
        assert_eq!(dirs.iter().map(|h| (h.path.as_str(), h.insert.as_str())).collect::<Vec<_>>(), [("src/", "../src/")]);
        let cwd = filter_kind(vec![hit("src/a.rs", "a.rs")], 1);
        assert_eq!(cwd.iter().map(|h| (h.path.as_str(), h.insert.as_str())).collect::<Vec<_>>(), [("src/", "./")]);
        assert_eq!(filter_kind(vec![hit("x/y", "y")], 2).len(), 1);
    }
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
        assert!(entries.iter().all(|hit| hit.provenance == ["*"]));
        assert_eq!(entries[0].insert, "folder/");
        assert!(entries.iter().any(|h| h.path == ".hidden"));
        let scores = HashMap::from([("file".into(), 2.0), ("missing".into(), 100.0)]);
        let picked = with_picked(&scores, &root, &root, entries, 10);
        assert_eq!(picked[0].insert, "file");
        assert!(picked.iter().all(|hit| hit.provenance == ["*"]));
        assert_eq!(picked.len(), 3);
        let picked = with_picked(&scores, &root, &root.join("folder"), Vec::new(), 10);
        assert_eq!(picked[0].insert, "../file");
        assert!(contains_terms("Hello World/", "world HELLO"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stripe_fill_eases_and_keeps_sunset_thirds() {
        assert_eq!(stripe_fill(80, 0, 0), 0);
        assert_eq!(stripe_fill(0, 0, 3), 0);
        assert_eq!(stripe_fill(80, 0, 1), 80);
        assert_eq!(stripe_fill(80, 0, 4), 20);
        assert_eq!(stripe_fill(80, 3, 4), 80);
        assert_eq!(stripe_fill(10, 0, 100), 0);
        assert_eq!(ease_stripe(0, 10), 3);
        assert_eq!(ease_stripe(9, 10), 10);
        assert_eq!(ease_stripe(10, 0), 7);
        assert_eq!(ease_stripe(4, 4), 4);
        let mut shown = 0u16;
        for _ in 0..64 {
            let next = ease_stripe(shown, 80);
            if shown != 80 {
                assert!(next > shown && next <= 80);
            }
            shown = next;
        }
        assert_eq!(shown, 80);
        shown = 80;
        for _ in 0..64 {
            shown = ease_stripe(shown, 0);
        }
        assert_eq!(shown, 0);

        let width = 12u16;
        let filled = 7u16;
        let line = stripe_line(width, filled, Modifier::empty());
        let mut colors = Vec::new();
        for span in &line.spans {
            for _ in 0..span.content.chars().count() {
                colors.push(span.style);
            }
        }
        assert_eq!(colors.len(), width as usize);
        for (col, style) in colors.iter().enumerate().take(filled as usize) {
            assert_eq!(style.fg, Some(stripe_color(col, width as usize)));
            assert!(!style.add_modifier.contains(Modifier::DIM));
        }
        assert!(colors[filled as usize..].iter().all(|style| *style == MUTED));
        let dim = stripe_line(9, 9, Modifier::DIM);
        assert!(dim.spans.iter().all(|span| span.style.add_modifier.contains(Modifier::DIM)));
        assert_eq!(
            dim.spans.iter().map(|span| span.content.chars().count()).sum::<usize>(),
            9
        );
        assert!(stripe_line(8, 0, Modifier::empty()).spans.iter().all(|span| span.style == MUTED));
    }
}

#[cfg(test)]
mod worker_tests {
    use super::*;

    #[test]
    fn callback_runs_off_thread_and_preserves_request_identity() {
        let ui_thread = std::thread::current().id();
        let (release, wait) = mpsc::channel();
        let (requests, replies) = worker(move |query: &(Mode, String, u8)| {
            assert_ne!(std::thread::current().id(), ui_thread);
            wait.recv().unwrap();
            query.1.clone()
        });
        let request = (Mode::Grep, "old query".to_owned(), 2);
        requests.send(request.clone()).unwrap();
        assert!(matches!(replies.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.send(()).unwrap();
        let (identity, result) = replies.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(identity, request);
        assert_ne!(identity, (Mode::Grep, "new query".to_owned(), 2));
        assert_eq!(result, "old query");
    }
}

#[cfg(test)]
mod hint_layout_tests {
    use super::*;

    #[test]
    fn hints_stay_ordered_and_leave_results_on_short_terminals() {
        for height in 0..=30 {
            let area = Rect::new(2, 3, 80, height);
            let rows = picker_layout(area);
            let mut bottom = area.y;
            for row in rows {
                assert_eq!(row.y, bottom);
                bottom = row.bottom();
            }
            assert_eq!(bottom, area.bottom());
            let content_height = rows[4].height.saturating_sub(if rows[2].height == 3 { 2 } else { 0 });
            assert!(content_height >= height.saturating_sub(1).min(3));
            if height >= 13 {
                assert_eq!(rows.map(|r| r.height), [1, 1, 3, 1, height - 8, 1, 1]);
            }
        }
    }
}
