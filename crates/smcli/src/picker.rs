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
use smcli_core::entry::{command_key, command_parts};
use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
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
        edit_paths: bool,
    },
    File {
        hit: FileHit,
        mode: Mode,
        query: String,
        scope: String,
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
    pub full: Vec<String>,
    pub suggested: Vec<String>,
    pub suggested_view: bool,
    pub query: String,
    pub mode: Mode,
    /// The current file/grep search scope (absolute directory). Set by the caller before `run`.
    pub scope: Option<String>,
    /// When in grep mode after Tab on a file, the file path (scope-relative) to restrict hits to.
    pub single_file_path: Option<String>,
    /// The scope to return to when leaving single-file grep with Shift+Tab.
    pub saved_file_scope: Option<String>,
    /// Replaces the "history" prompt label, e.g. for directory suggestions.
    pub title: Option<String>,
    /// Files mode shows only folders (1) or files (2) when set.
    pub kind: u8,
    /// Colour inline rows from `LS_COLORS` instead of only the terminal theme.
    pub ls_colors: bool,
}
impl Picker {
    /// Native history is oldest first; captured history is newest first.
    pub fn new(
        suggestions: Vec<String>,
        recent: Vec<String>,
        native: Vec<String>,
        query: String,
    ) -> Self {
        let add = |commands: Vec<String>| {
            let mut seen = HashSet::new();
            let mut list = Vec::new();
            for command in commands {
                let command = command.trim();
                if !command.is_empty() && !history_internal(command) && seen.insert(command.to_owned()) {
                    list.push(command.to_owned());
                }
            }
            list
        };
        let suggested = add(suggestions);
        let full = add(recent.into_iter().chain(native.into_iter().rev().map(|s| command_key(&s, None))).collect());
        Self {
            full,
            suggested,
            suggested_view: false,
            query,
            mode: Mode::History,
            scope: None,
            single_file_path: None,
            saved_file_scope: None,
            title: None,
            kind: 0,
            ls_colors: false,
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
        while let Ok(mut query) = incoming.recv() {
            // A running callback cannot be interrupted, but queued intermediate queries are obsolete.
            while let Ok(latest) = incoming.try_recv() { query = latest; }
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

fn stripe_line(width: u16, filled: u16, fade: Modifier, cell: &str) -> Line<'static> {
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
            cell.repeat(col - start),
            Style::new().fg(color).add_modifier(fade),
        ));
    }
    if filled < width {
        spans.push(Span::styled(cell.repeat(width - filled), MUTED));
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

/// Row colours from `LS_COLORS`: directories, the file default and `*.ext` entries. Backgrounds are dropped.
struct LsColors {
    dir: Style,
    file: Style,
    ext: Vec<(String, Style)>,
}
impl LsColors {
    fn parse(spec: &str) -> Self {
        // Unset: the terminal theme's own ANSI blue for directories and default foreground for files; LS_COLORS refines it.
        let mut colors = Self { dir: Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD), file: Style::new(), ext: Vec::new() };
        for entry in spec.split(':') {
            let Some((key, codes)) = entry.split_once('=') else { continue };
            match key {
                "di" => colors.dir = sgr(codes),
                "fi" => colors.file = sgr(codes),
                _ => if let Some(ext) = key.strip_prefix("*.") { colors.ext.push((format!(".{}", ext.to_lowercase()), sgr(codes))) },
            }
        }
        // Longest suffix wins: `.tar.gz` before `.gz`.
        colors.ext.sort_by_key(|(ext, _)| std::cmp::Reverse(ext.len()));
        colors
    }
    fn style(&self, path: &str) -> Style {
        if path.ends_with('/') { return self.dir; }
        let path = path.to_lowercase();
        self.ext.iter().find(|(ext, _)| path.ends_with(ext.as_str())).map_or(self.file, |(_, style)| *style)
    }
}

/// SGR codes to a foreground style. Unknown or truncated codes are skipped.
fn sgr(codes: &str) -> Style {
    let codes: Vec<u8> = codes.split(';').filter_map(|c| c.parse().ok()).collect();
    let mut style = Style::new();
    let mut i = 0;
    while i < codes.len() {
        match codes[i] {
            1 => style = style.add_modifier(Modifier::BOLD),
            2 => style = style.add_modifier(Modifier::DIM),
            3 => style = style.add_modifier(Modifier::ITALIC),
            4 => style = style.add_modifier(Modifier::UNDERLINED),
            n @ 30..=37 => style = style.fg(Color::Indexed(n - 30)),
            n @ 90..=97 => style = style.fg(Color::Indexed(n - 90 + 8)),
            38 => match codes.get(i + 1..) {
                Some([5, n, ..]) => { style = style.fg(Color::Indexed(*n)); i += 2; }
                Some([2, r, g, b, ..]) => { style = style.fg(Color::Rgb(*r, *g, *b)); i += 4; }
                _ => break,
            },
            48 => match codes.get(i + 1) { Some(5) => i += 2, Some(2) => i += 4, _ => break },
            _ => {}
        }
        i += 1;
    }
    style
}

/// Inline (`@`-Tab): header, search, sunset underline, list hints, results, global hints.
/// Slots match `picker_layout`; the sunset sits under the search and navigation is unused.
fn inline_layout(area: Rect) -> [Rect; 7] {
    let list = area.height.saturating_sub(5);
    let row = |y: u16, height: u16| Rect::new(area.x, area.y + y, area.width, height);
    [row(0, 1), row(3, 1), row(1, 1), row(4, 0), row(4, list), row(2, 1), row(4 + list, 1)]
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

/// Show paths under the home directory as `~/…`.
fn abbreviate(path: &str, home: &str) -> String {
    if path == home { return "~".into(); }
    if let Some(rest) = path.strip_prefix(&format!("{home}/")) {
        format!("~/{rest}")
    } else {
        path.into()
    }
}

/// Where inline Tab moves: a directory hit itself, a file's parent. `None` when that is `scope` or outside it.
fn tab_target(scope: &str, hit: &str) -> Option<String> {
    let path = std::path::Path::new(scope).join(hit);
    let dir = if hit.ends_with('/') { path } else { path.parent()?.to_path_buf() };
    let dir = dir.canonicalize().ok()?;
    let scope = std::path::Path::new(scope).canonicalize().ok()?;
    (dir != scope && dir.starts_with(&scope)).then(|| dir.to_string_lossy().into_owned())
}

/// The parent of `scope`, or `None` if widening would go above `home` or outside it.
fn widen_scope(scope: &str, home: &str) -> Option<String> {
    let scope = std::path::Path::new(scope);
    let parent = scope.parent()?;
    let parent = parent.to_str()?;
    if parent.is_empty() || parent.len() < home.len() || !parent.starts_with(home) {
        return None;
    }
    Some(parent.into())
}

/// Search is invoked only on Tab (history); files/grep receive the current scope and optional single-file path.
/// A file choice is unquoted: the CLI must validate the shell insertion context and quote it.
pub fn run(
    mut picker: Picker,
    search: Option<Box<dyn FnMut(&str) -> Result<Vec<String>> + Send>>,
    files: Option<Box<dyn FnMut(Mode, &str, u8, &str, Option<&str>) -> Result<FileResults> + Send>>,
    inline: bool,
) -> Result<Option<Choice>> {
    let search = search.map(|mut callback| worker(move |(q, _): &(String, u64)| callback(q)));
    let files = files.map(|mut callback| worker(move |(mode, q, kind, scope, path): &(Mode, String, u8, String, Option<String>)| {
        callback(*mode, q, *kind, scope, path.as_deref())
    }));
    let mut file_busy = false;
    let mut semantic_busy = false;
    let mut semantic_pending = None;
    let mut semantic_generation = 0u64;
    let mut spinner = 0;
    let mut spun = Instant::now();
    let rows = ratatui::crossterm::terminal::size().ok().filter(|&(_, rows)| rows > 0).map_or(24, |(_, rows)| rows);
    let mut terminal = TerminalSession::open(inline, 17, rows)?;
    let terminal_inline = terminal.inline.is_some();
    // The terminal theme by default; LS_COLORS only when the user opts in.
    let ls_colors = LsColors::parse(&if picker.ls_colors { std::env::var("LS_COLORS").unwrap_or_default() } else { String::new() });
    let mut saved = HashMap::new();
    let mut selected = 0usize;
    let mut semantic: Option<(String, Vec<String>)> = None;
    let mut kind = picker.kind;
    let mut fetched = None;
    let mut observed = None;
    let mut changed_at = Instant::now();
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
    let mut filtered: Option<((String, bool, bool), Vec<String>)> = None;
    let starred: HashSet<String> = picker.suggested.iter().cloned().collect();
    let home = std::env::var("HOME").unwrap_or_default();
    let cwd = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    if picker.scope.is_none() {
        picker.scope = Some(cwd.clone());
    }
    loop {
        let scope = picker.scope.clone().unwrap_or_else(|| cwd.clone());
        let file_path = picker.single_file_path.clone();
        let current = (picker.mode, picker.query.clone(), kind, scope.clone(), file_path.clone());
        if observed.as_ref() != Some(&current) {
            observed = Some(current.clone());
            changed_at = Instant::now();
        }
        if let Some((_, replies)) = &files {
            if let Ok((request, result)) = replies.try_recv() {
                file_busy = false;
                if request == current {
                    match result {
                        Ok(result) => {
                            hits = if picker.mode == Mode::Files {
                                let mut hits = filter_kind(result.hits, kind);
                                suggested_first(&mut hits, &picker.query);
                                hits
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
            if let Ok(((query, generation), result)) = replies.try_recv() {
                semantic_busy = false;
                if picker.mode == Mode::History && generation == semantic_generation && query == picker.query && semantic_pending.is_none() {
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
                        semantic_busy = requests.send((query, semantic_generation)).is_ok();
                    }
                }
            }
        }
        if picker.mode != Mode::History && fetched.as_ref() != Some(&current) {
            hits.clear();
            selected = 0;
            status.clear();
            if picker.mode == Mode::Grep && picker.query.is_empty() {
                fetched = Some(current.clone());
                status = "type to search file contents".into();
            } else if let Some((requests, _)) = &files {
                if !file_busy && changed_at.elapsed() >= Duration::from_millis(100)
                    && !event::poll(Duration::ZERO)? {
                    file_busy = requests.send(current.clone()).is_ok();
                    fetched = Some(current.clone());
                }
            } else {
                fetched = Some(current.clone());
                status = "files and grep need smcli-service".into();
            }
        }
        let file_pending = picker.mode != Mode::History && fetched.as_ref() != Some(&current);
        let busy = file_busy || semantic_busy || file_pending;
        if busy && spun.elapsed() >= Duration::from_millis(80) {
            spinner = (spinner + 1) % 10;
            spun = Instant::now();
        }
        let semantic_active = semantic.as_ref().is_some_and(|(q, _)| q == &picker.query);
        let key_now = (picker.query.clone(), semantic_active, picker.suggested_view);
        if filtered.as_ref().map(|(k, _)| k) != Some(&key_now) {
            let commands = if semantic_active {
                semantic.as_ref().unwrap().1.clone()
            } else {
                filter_history(if picker.suggested_view { &picker.suggested } else { &picker.full }, &picker.query)
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
            let label = if picker.mode == Mode::Files { match kind { 1 => "folders", 2 => "files", _ => "files+dirs" } } else if history { if picker.suggested_view { picker.title.as_deref().unwrap_or("history") } else { "history" } } else { picker.mode.as_str() };
            let compact = terminal_inline;
            let [header_area, filter_area, search_area, navigation_area, list_area, stripe_area, global_area] = if compact { inline_layout(area) } else { picker_layout(area) };
            let boxed = search_area.height == 3;
            // Header shows cwd and current scope; single-file grep names the file as its scope.
            let scope_label = if history {
                String::new()
            } else {
                let displayed = if picker.mode == Mode::Grep && picker.single_file_path.is_some() {
                    let file = picker.single_file_path.as_deref().unwrap();
                    format!("{}/{}", abbreviate(&scope, &home), file)
                } else {
                    abbreviate(&scope, &home)
                };
                format!("{}  →  {}", abbreviate(&cwd, &home), displayed)
            };
            let header = if compact && (!history || (picker.suggested_view && picker.title.is_some())) {
                // Only the searched directory, which grows and shrinks as Tab moves in and out; suggestions name the cwd.
                let (label, dir) = if history { ("suggested", &cwd) } else { (label, &scope) };
                let mut path = abbreviate(dir, &home);
                if !path.ends_with('/') { path.push('/'); }
                Line::from(vec![Span::styled(format!(" {label} @ "), ACCENT), Span::styled(visible(&path), ls_colors.dir)])
            } else {
                Line::from(vec![Span::styled(format!(" {label} "), ACCENT), Span::styled(scope_label, ACCENT.add_modifier(Modifier::BOLD)), Span::styled(" ›", ACCENT.add_modifier(Modifier::BOLD))])
            };
            let found = if compact { format!("{count} Results") } else { format!("{count} {} results", if picker.mode == Mode::Grep { "grep" } else { "files" }) };
            let summary = if history {
                if status.is_empty() && picker.suggested_view && picker.suggested.is_empty() {
                    "no suggestions for this directory".into()
                } else if compact && status.is_empty() { found } else { visible(&status) }
            } else if hits.is_empty() {
                if status.is_empty() { "no matches".into() } else { visible(&status) }
            } else if status.is_empty() { found } else { visible(&status) };
            let mut summary = Line::styled(format!(" {summary} "), MUTED);
            if busy {
                summary.spans.insert(0, Span::styled(format!(" {}", ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"][spinner]), ACCENT));
            }
            frame.render_widget(Paragraph::new(header), header_area);
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
            if compact && history {
                frame.render_widget(Paragraph::new(hint_line(&[("tab", "semantic ")]).right_aligned()), input);
            }
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
            // Inline marks the selection with a yellow cursor and nudges it right instead of a full-row highlight.
            let cursor = |sel: bool| if !compact { None } else if sel { Some(Span::styled(" ▌", Style::new().fg(Color::Yellow))) } else { Some(Span::raw("  ")) };
            let items: Vec<ListItem> = if history {
                // Rows past the selection plus one screen are never visible; skip building them.
                commands.iter().enumerate().take(selected + area.height as usize).map(|(i, c)| {
                    let sel = i == selected;
                    let mark = if picker.suggested_view && starred.contains(c) { "* " } else { "  " };
                    let spans = history_row(c, list_block.inner(list_area).width.saturating_sub(if compact { 5 } else { 2 }) as usize);
                    let mut line: Vec<Span> = cursor(sel).into_iter().collect();
                    // Inline matches the files rows: no leading mark, suggested rows end in ✨.
                    if !compact { line.push(Span::styled(mark, if sel { Style::new() } else { MUTED })); }
                    line.extend(if sel && !compact { plain(spans) } else { spans });
                    if compact && picker.suggested_view && starred.contains(c) { line.push(Span::raw(" ✨")); }
                    ListItem::new(Line::from(line))
                }).collect()
            } else {
                hits.iter().enumerate().map(|(i, h)| {
                    let name = &h.path;
                    let location = h.line.map(|n| format!(":{n}  {}", h.text.as_deref().unwrap_or(""))).unwrap_or_default();
                    ListItem::new(Line::from(cursor(i == selected).into_iter().chain([
                        Span::styled(if compact { String::new() } else { format!("{} ", h.git.as_deref().unwrap_or("")) }, ACCENT),
                        Span::styled(visible(name), if compact { ls_colors.style(name) } else { Style::new() }),
                        Span::styled(visible(&location), MUTED),
                        // Inline: colour already tells dirs from files; git status stays and suggested (picked) rows get ✨.
                        Span::styled(if !compact { format!("  {}", h.provenance.join("+")) } else {
                            h.git.as_deref().map(str::trim).filter(|g| !g.is_empty()).map(|g| format!("  {g}")).unwrap_or_default()
                        }, MUTED),
                        Span::raw(if compact && suggested(h) { " ✨" } else { "" }),
                    ]).collect::<Vec<_>>()))
                }).collect()
            };
            let list = List::new(items).block(list_block).highlight_style(if compact { Style::new().add_modifier(Modifier::BOLD) } else { SELECTED });
            let mut state = ListState::default().with_selected((count > 0).then_some(selected));
            frame.render_stateful_widget(list, list_area, &mut state);
            let filters = if history && compact {
                vec![("^R", "full/suggested")]
            } else if history {
                vec![("^R", "full/suggested"), ("tab", "semantic")]
            } else if picker.mode == Mode::Files {
                vec![("tab", "into"), ("⇧tab", "out"), ("^T", "dirs/files")]
            } else if picker.mode == Mode::Grep {
                vec![("⇧tab", "out")]
            } else {
                vec![]
            };
            let mut filters = filters;
            if compact { filters.extend([("⏎", "accept"), ("↑↓", "move")]); }
            frame.render_widget(Paragraph::new(hint_line(&filters)), filter_area);
            frame.render_widget(Paragraph::new(hint_line(&[("[ ⏎ ]", "accept"), ("↑↓", "move"), ("PgUp/PgDn", "page")])), navigation_area);
            let mut globals = vec![("esc", "cancel")];
            if picker.mode != Mode::Files { globals.push(("^T", "files")); }
            if picker.mode != Mode::History { globals.push(("^R", "history")); }
            if picker.mode == Mode::History {
                globals.push(("^E", "edit paths"));
            }
            if picker.mode != Mode::Grep { globals.push(("^G", "grep")); }
            globals.push(("^U", "clear"));
            let fade = if tick < 3 { Modifier::DIM } else { Modifier::empty() };
            if compact {
                // The search underline is the sunset; the result count ends it.
                let width = area.width.saturating_sub(summary.width() as u16);
                cols = width;
                let filled = shown.min(width);
                let mut line = if filled > 0 && filled < width {
                    // A `╸` tip in the edge cell's colour leads the bar.
                    let mut line = stripe_line(width, filled - 1, fade, "━");
                    let rest = line.spans.pop();
                    line.spans.push(Span::styled("╸", Style::new().fg(stripe_color(filled as usize - 1, width as usize)).add_modifier(fade)));
                    line.spans.extend(rest.map(|span| Span::styled("━".repeat(span.content.chars().count() - 1), span.style)));
                    line
                } else {
                    stripe_line(width, filled, fade, "━")
                };
                line.spans.extend(summary.spans);
                frame.render_widget(Paragraph::new(line), stripe_area);
            } else {
                frame.render_widget(Paragraph::new(stripe_line(area.width, shown, fade, "▀")), stripe_area);
            }
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
        let key = match event::read()? {
            // Inline: the next draw's autoresize re-anchors the viewport from the cursor; Drop still finds the prompt above it.
            Event::Resize(..) if terminal.inline.is_some() => continue,
            Event::Resize(columns, rows) => {
                terminal.terminal.resize(Rect::new(0, 0, columns, rows))?;
                continue;
            }
            Event::Key(key) => key,
            _ => continue,
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
            KeyCode::Down => selected = (selected + 1).min(count.saturating_sub(1)),
            KeyCode::Enter => {
                return Ok(if count == 0 {
                    None
                } else if picker.mode == Mode::History {
                    Some(Choice::Command {
                        command: commands[selected].clone(),
                        query: picker.query,
                        semantic: semantic_active,
                        edit_paths: false,
                    })
                } else {
                    Some(Choice::File {
                        hit: hits[selected].clone(),
                        mode: picker.mode,
                        query: picker.query,
                        scope,
                    })
                });
            }
            KeyCode::Char('r') if ctrl => {
                if picker.mode == Mode::History {
                    picker.suggested_view = !picker.suggested_view;
                    semantic = None;
                    semantic_pending = None;
                    semantic_generation += 1;
                    selected = 0;
                    status.clear();
                } else {
                    switch_mode(&mut picker, Mode::History, &mut saved, &mut selected, &mut status);
                }
            }
            KeyCode::Char('t') if ctrl => {
                if picker.mode == Mode::Files {
                    kind = (kind + 1) % 3;
                    selected = 0;
                } else {
                    switch_mode(&mut picker, Mode::Files, &mut saved, &mut selected, &mut status);
                    kind = picker.kind;
                }
            }
            KeyCode::Char('g') if ctrl && picker.mode != Mode::Grep => switch_mode(&mut picker, Mode::Grep, &mut saved, &mut selected, &mut status),
            KeyCode::Char('e') if ctrl && picker.mode == Mode::History && count > 0 => {
                return Ok(Some(Choice::Command {
                    command: commands[selected].clone(),
                    query: picker.query,
                    semantic: semantic_active,
                    edit_paths: true,
                }));
            }
            KeyCode::Char('u') if ctrl => {
                picker.query.clear();
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
            KeyCode::Tab if picker.mode == Mode::History && !picker.query.trim().is_empty() => {
                if search.is_some() {
                    semantic_pending = Some(picker.query.clone());
                }
            }
            KeyCode::Tab if picker.mode == Mode::Files && count > 0 && terminal_inline => {
                // Inline: a directory goes into itself, a file into its parent.
                if let Some(next) = tab_target(&scope, &hits[selected].path) {
                    picker.scope = Some(next);
                    picker.query.clear();
                    selected = 0;
                    status.clear();
                }
            }
            KeyCode::Tab if picker.mode == Mode::Files && count > 0 => {
                let hit = &hits[selected];
                if hit.path.ends_with('/') {
                    if let Ok(next) = std::path::Path::new(&scope).join(&hit.path).canonicalize() {
                        picker.scope = Some(next.to_string_lossy().into_owned());
                        picker.query.clear();
                        selected = 0;
                        status.clear();
                    }
                } else {
                    saved.insert(Mode::Files, picker.query.clone());
                    picker.saved_file_scope = Some(scope.clone());
                    picker.single_file_path = Some(hit.path.clone());
                    picker.mode = Mode::Grep;
                    picker.query = saved.get(&Mode::Grep).cloned().unwrap_or_default();
                    selected = 0;
                    status.clear();
                }
            }
            KeyCode::BackTab if picker.mode == Mode::Grep && picker.saved_file_scope.is_some() => {
                picker.scope = picker.saved_file_scope.take();
                picker.single_file_path = None;
                picker.mode = Mode::Files;
                picker.query = saved.get(&Mode::Files).cloned().unwrap_or_default();
                selected = 0;
                status.clear();
            }
            KeyCode::BackTab if picker.mode == Mode::Files => {
                if let Some(parent) = widen_scope(&scope, &home) {
                    picker.scope = Some(parent);
                    picker.query.clear();
                    selected = 0;
                    status.clear();
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

fn switch_mode(picker: &mut Picker, target: Mode, saved: &mut HashMap<Mode, String>, selected: &mut usize, status: &mut String) {
    saved.insert(picker.mode, std::mem::take(&mut picker.query));
    if picker.mode == Mode::Grep {
        picker.single_file_path = None;
        picker.saved_file_scope = None;
    }
    picker.mode = target;
    picker.query = saved.get(&picker.mode).cloned().unwrap_or_default();
    *selected = 0;
    status.clear();
}

fn history_internal(key: &str) -> bool {
    let (typed, expanded) = command_parts(key);
    smcli_core::parse::is_smcli_command(expanded.as_deref().unwrap_or(&typed))
}

pub(crate) fn history_label(key: &str) -> String {
    let (typed, expanded) = command_parts(key);
    visible(&expanded.map_or(typed.clone(), |e| format!("{typed}  ↦ {e}")))
}

/// The alias as typed: the user knows what it means, and the shell expands it again when it runs.
pub(crate) fn history_insert(key: &str) -> String {
    command_parts(key).0
}

/// An alias row: the typed alias, a `↦` (maps to) mark, then its expansion dimmed and cut to fit `width`. The alias is never cut first.
fn history_row(key: &str, width: usize) -> Vec<Span<'static>> {
    let (typed, expanded) = command_parts(key);
    let mut spans = highlight(&visible(&typed));
    let Some(expanded) = expanded else { return spans };
    let room = width.saturating_sub(Line::from(spans.clone()).width() + 4);
    let expanded = visible(&expanded);
    let shown = if Span::raw(expanded.as_str()).width() <= room {
        expanded
    } else {
        let mut cut = String::new();
        for c in expanded.chars() {
            if Span::raw(format!("{cut}{c}…")).width() > room { break; }
            cut.push(c);
        }
        if cut.is_empty() { String::new() } else { format!("{cut}…") }
    };
    spans.extend([Span::raw("  "), Span::styled("↦", ACCENT)]);
    if !shown.is_empty() { spans.push(Span::styled(format!(" {shown}"), MUTED)); }
    spans
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
            let (typed, expanded) = command_parts(c);
            let text = expanded.map_or(typed.clone(), |e| format!("{typed} {e}"));
            pattern
                .score(Utf32Str::new(&text, &mut buffer), &mut matcher)
                .is_some()
        })
        .cloned()
        .collect()
}

/// fff's blank-search suggestions, or files picked before.
fn suggested(hit: &FileHit) -> bool {
    hit.provenance.iter().any(|p| p == "picked" || p == "suggested")
}

/// Suggested rows that contain the query first; the stable sort keeps ranking within each group.
/// A picked row that only fuzzy-matched (a typo away) keeps its rank, so it never jumps over real matches.
fn suggested_first(hits: &mut [FileHit], query: &str) {
    let pattern = Pattern::new(query, CaseMatching::Ignore, Normalization::Smart, nucleo_matcher::pattern::AtomKind::Substring);
    let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT);
    let mut buffer = Vec::new();
    hits.sort_by_cached_key(|h| {
        !(suggested(h) && pattern.score(Utf32Str::new(&h.path, &mut buffer), &mut matcher).is_some())
    });
}

/// Narrows project hits to files (2) or folders (1); the index lists only files, so folders are their matches' parents.
// shortcut: folders only appear if a file inside them matches; add directory entries to the service index if that's not enough.
fn filter_kind(hits: Vec<FileHit>, kind: u8) -> Vec<FileHit> {
    // A blank search also returns dirs (trailing `/`): files mode drops them, folders mode keeps them as they are.
    if kind == 2 {
        return hits.into_iter().filter(|h| !h.path.ends_with('/')).collect();
    }
    if kind != 1 {
        return hits;
    }
    let parent = |p: &str| p.rsplit_once('/').map(|(d, _)| format!("{d}/"));
    let mut seen = HashSet::new();
    hits.into_iter()
        .filter_map(|h| {
            if h.path.ends_with('/') { return seen.insert(h.path.clone()).then_some(h); }
            let path = parent(&h.path)?;
            seen.insert(path.clone()).then(|| FileHit { insert: parent(&h.insert).unwrap_or_else(|| "./".to_string()), path, git: None, line: None, column: None, text: None, ..h })
        })
        .collect()
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

/// Inline height: `wanted` clamped under the prompt; `None` when too short to be useful.
fn inline_height(wanted: u16, rows: u16) -> Option<u16> {
    Some(wanted.min(rows.saturating_sub(1))).filter(|&height| height >= 6)
}

pub(crate) struct TerminalSession {
    pub terminal: Terminal<CrosstermBackend<File>>,
    /// The prompt's cursor column and the caller's stdout when drawn inline under it (`@`-Tab).
    inline: Option<(u16, std::os::fd::OwnedFd)>,
}
impl TerminalSession {
    /// Inline under the prompt when asked and it fits, else (or if the terminal won't say where the cursor is) fullscreen.
    // shortcut: a pty that never answers the cursor query waits crossterm's timeout on every open; cache the failure if one shows up.
    pub fn open(inline: bool, wanted: u16, rows: u16) -> Result<Self> {
        match inline.then(|| inline_height(wanted, rows)).flatten().and_then(|height| Self::inline(height).ok()) {
            Some(session) => Ok(session),
            None => Self::new(),
        }
    }
    /// A `height`-row viewport below the prompt line. Errors (e.g. a pty that never reports the cursor) leave the terminal as found.
    pub fn inline(height: u16) -> Result<Self> {
        let tty = OpenOptions::new().read(true).write(true).open("/dev/tty").context("pick needs a terminal")?;
        use std::os::fd::{AsRawFd, FromRawFd};
        // crossterm asks for the cursor on stdout, which the shell captures: point stdout at the tty until Drop.
        let saved = unsafe { libc::dup(1) };
        if saved < 0 { return Err(std::io::Error::last_os_error().into()); }
        let stdout = unsafe { std::os::fd::OwnedFd::from_raw_fd(saved) };
        let restore = |stdout: &std::os::fd::OwnedFd| unsafe { libc::dup2(stdout.as_raw_fd(), 1) };
        if unsafe { libc::dup2(tty.as_raw_fd(), 1) } < 0 { return Err(std::io::Error::last_os_error().into()); }
        if let Err(error) = enable_raw_mode() { restore(&stdout); return Err(error.into()); }
        let column = match cursor::position() {
            Ok((column, _)) => column,
            Err(error) => {
                let _ = disable_raw_mode();
                restore(&stdout);
                return Err(error.into());
            }
        };
        let mut display = tty.try_clone().context("pick needs a terminal")?;
        let terminal = std::io::Write::write_all(&mut display, b"\r\n").map_err(anyhow::Error::from).and_then(|_| {
            Ok(Terminal::with_options(CrosstermBackend::new(display), TerminalOptions { viewport: Viewport::Inline(height) })?)
        });
        match terminal {
            Ok(terminal) => Ok(Self { terminal, inline: Some((column, stdout)) }),
            Err(error) => {
                let _ = execute!(&tty, cursor::MoveUp(1), cursor::MoveToColumn(column));
                let _ = disable_raw_mode();
                restore(&stdout);
                Err(error)
            }
        }
    }

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
                let (columns, rows) = ratatui::crossterm::terminal::size().ok().filter(|&(_, rows)| rows > 0).unwrap_or((80, 24));
                let viewport = Viewport::Fixed(ratatui::layout::Rect::new(0, 0, columns, rows));
                Ok(Terminal::with_options(CrosstermBackend::new(display), TerminalOptions { viewport })?)
            });
        match terminal {
            Ok(terminal) => Ok(Self { terminal, inline: None }),
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
        if let Some((column, stdout)) = &self.inline {
            let column = *column;
            // The viewport may have scrolled the screen; the prompt is the line just above it.
            let row = self.terminal.get_frame().area().y.saturating_sub(1);
            let _ = self.terminal.clear();
            let _ = execute!(self.terminal.backend_mut(), cursor::MoveTo(column, row), cursor::Show);
            unsafe { libc::dup2(std::os::fd::AsRawFd::as_raw_fd(stdout), 1) };
        } else {
            let _ = execute!(self.terminal.backend_mut(), ratatui::crossterm::terminal::LeaveAlternateScreen, cursor::Show);
        }
        let _ = disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn picker_keeps_full_and_suggested_lists_separate() {
        let p = Picker::new(vec!["suggested".into(), "shared".into()], vec!["shared".into(), "recent".into()], vec!["native".into()], String::new());
        assert_eq!(p.full, ["shared", "recent", "native"]);
        assert_eq!(p.suggested, ["suggested", "shared"]);
    }

    #[test]
    fn switch_mode_saves_and_restores_query_and_clears_grep_scope() {
        let mut p = Picker::new(vec![], vec![], vec![], String::new());
        let mut saved = HashMap::new();
        let mut selected = 4;
        let mut status = "status".into();
        p.query = "history".into();
        switch_mode(&mut p, Mode::Files, &mut saved, &mut selected, &mut status);
        assert_eq!((p.mode, p.query.as_str(), selected, status.as_str()), (Mode::Files, "", 0, ""));
        p.query = "files".into();
        switch_mode(&mut p, Mode::History, &mut saved, &mut selected, &mut status);
        assert_eq!((p.mode, p.query.as_str()), (Mode::History, "history"));
        p.mode = Mode::Grep;
        p.single_file_path = Some("f".into());
        p.saved_file_scope = Some("scope".into());
        switch_mode(&mut p, Mode::History, &mut saved, &mut selected, &mut status);
        assert_eq!((p.single_file_path, p.saved_file_scope), (None, None));
    }

    #[test]
    fn file_kind_cycle_wraps_and_mode_entry_uses_picker_kind() {
        let mut kind = 0;
        let cycle = |kind: &mut u8| *kind = (*kind + 1) % 3;
        let mut values = Vec::new();
        for _ in 0..4 { values.push(kind); cycle(&mut kind); }
        assert_eq!(values, [0, 1, 2, 0]);
        let p = Picker { kind: 1, ..Picker::new(vec![], vec![], vec![], String::new()) };
        assert_eq!(p.kind, 1);
    }

    #[test]
    fn fixed_viewport_can_shrink_and_grow() {
        let mut terminal = Terminal::with_options(
            ratatui::backend::TestBackend::new(100, 30),
            TerminalOptions { viewport: Viewport::Fixed(Rect::new(0, 0, 100, 30)) },
        ).unwrap();
        for (columns, rows) in [(40, 8), (120, 40)] {
            terminal.backend_mut().resize(columns, rows);
            terminal.resize(Rect::new(0, 0, columns, rows)).unwrap();
            terminal.draw(|frame| {
                assert_eq!(frame.area(), Rect::new(0, 0, columns, rows));
                assert_eq!(picker_layout(frame.area())[6].bottom(), rows);
            }).unwrap();
        }
    }

    #[test]
    fn inline_viewport_sits_under_a_bottom_prompt() {
        assert_eq!(inline_height(17, 40), Some(17));
        assert_eq!(inline_height(17, 10), Some(9));
        assert_eq!(inline_height(17, 6), None);
        let mut backend = ratatui::backend::TestBackend::new(80, 24);
        // The cursor after `\r\n` from a prompt on the last row.
        ratatui::backend::Backend::set_cursor_position(&mut backend, (0, 23)).unwrap();
        let mut terminal = Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(17) }).unwrap();
        let area = terminal.get_frame().area();
        assert_eq!((area.y, area.height), (7, 17));
        assert_eq!(area.y.saturating_sub(1), 6);
        let [header, filters, search, _, list, sunset, globals] = inline_layout(area);
        let order: Vec<u16> = [header, search, sunset, filters, list, globals].iter().map(|r| r.y).collect();
        assert_eq!(order, [7, 8, 9, 10, 11, 23], "search, underline, list hints, 12 results, global hints");
        assert_eq!(list.height, 12);
    }

    #[test]
    fn ls_colors_match_dirs_and_longest_extension() {
        let unset = LsColors::parse("");
        assert_eq!(unset.style("src/"), Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD));
        assert_eq!(unset.style("a.rs"), Style::new());
        let colors = LsColors::parse("di=01;36:*.gz=31:*.tar.gz=38;5;208:*.RS=38;2;1;2;3:fi=00:x=bad:*.ok=38;5");
        assert_eq!(colors.style("lib.d/"), Style::new().fg(Color::Indexed(6)).add_modifier(Modifier::BOLD));
        assert_eq!(colors.style("a.TAR.GZ"), Style::new().fg(Color::Indexed(208)));
        assert_eq!(colors.style("a.gz"), Style::new().fg(Color::Indexed(1)));
        assert_eq!(colors.style("main.rs"), Style::new().fg(Color::Rgb(1, 2, 3)));
        assert_eq!(colors.style("x.ok"), Style::new());
        assert_eq!(sgr("41;32"), Style::new().fg(Color::Indexed(2)), "backgrounds dropped");
    }

    #[test]
    fn suggested_rows_lead_only_when_they_contain_the_query() {
        let hit = |path: &str, picked: bool| FileHit {
            insert: path.into(), path: path.into(), git: None, line: None, column: None, text: None, definition: None,
            provenance: if picked { vec!["fff".into(), "picked".into()] } else { vec!["fff".into()] },
        };
        let mut hits = vec![hit("t02 a", false), hit("t01 b", true), hit("t02 c", true)];
        suggested_first(&mut hits, "t02");
        let order: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(order, ["t02 c", "t02 a", "t01 b"]);
        suggested_first(&mut hits, "");
        let order: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(order, ["t02 c", "t01 b", "t02 a"], "empty query: every suggestion leads");
    }

    #[test]
    fn inline_tab_enters_dirs_and_file_parents_inside_scope() {
        let root = std::env::temp_dir().join(format!("smcli-tab-{}", std::process::id()));
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/f.txt"), "").unwrap();
        std::fs::write(root.join("top.txt"), "").unwrap();
        let scope = root.canonicalize().unwrap();
        let s = scope.to_str().unwrap();
        assert_eq!(tab_target(s, "a/"), Some(scope.join("a").to_string_lossy().into_owned()));
        assert_eq!(tab_target(s, "a/b/f.txt"), Some(scope.join("a/b").to_string_lossy().into_owned()));
        assert_eq!(tab_target(s, "top.txt"), None, "parent is the scope");
        assert_eq!(tab_target(s, "../"), None, "outside the scope");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn project_folders_from_file_parents() {
        let hit = |path: &str, insert: &str| FileHit { insert: insert.into(), path: path.into(), provenance: vec!["files".into()], git: None, line: None, column: None, text: None, definition: None };
        let dirs = filter_kind(vec![hit("src/a.rs", "../src/a.rs"), hit("src/b.rs", "../src/b.rs"), hit("top.rs", "../top.rs")], 1);
        assert_eq!(dirs.iter().map(|h| (h.path.as_str(), h.insert.as_str())).collect::<Vec<_>>(), [("src/", "../src/")]);
        let cwd = filter_kind(vec![hit("src/a.rs", "a.rs")], 1);
        assert_eq!(cwd.iter().map(|h| (h.path.as_str(), h.insert.as_str())).collect::<Vec<_>>(), [("src/", "./")]);
        assert_eq!(filter_kind(vec![hit("x/y", "y")], 2).len(), 1);
    }
    #[test]
    fn alias_identity_display_and_insertion_are_separate() {
        let first = command_key("gl", Some("git log"));
        let second = command_key("gl", Some("git ls-files"));
        let picker = Picker::new(vec![first.clone()], vec![second.clone()], vec!["gl".into()], String::new());
        assert_eq!(picker.full.len(), 2);
        assert_eq!(picker.suggested, [first.clone()]);
        assert_eq!(history_label(&first), "gl  ↦ git log");
        assert_eq!(history_insert(&first), "gl");
        assert_eq!(history_insert("gl"), "gl");
        assert_eq!(filter_history(&picker.full, "ls-files"), [second], "{:?}", picker.full.iter().map(|s| history_label(s)).collect::<Vec<_>>());
        assert_eq!(history_label(&command_key("g\x1b[31m", Some("git status"))), "g^[[31m  ↦ git status");
    }

    #[test]
    fn alias_rows_mark_the_alias_and_cut_only_the_expansion() {
        let text = |spans: Vec<Span>| spans.iter().map(|s| s.content.to_string()).collect::<String>();
        let key = command_key("mkal", Some(r#"$EDITOR $ZDOTDIR/conf.d/50-aliases.zsh && source "$ZDOTDIR/.zshrc""#));
        assert_eq!(text(history_row(&key, 200)), r#"mkal  ↦ $EDITOR $ZDOTDIR/conf.d/50-aliases.zsh && source "$ZDOTDIR/.zshrc""#);
        let narrow = text(history_row(&key, 20));
        assert_eq!(narrow, "mkal  ↦ $EDITOR $ZD…");
        assert_eq!(Span::raw(narrow).width(), 20);
        assert_eq!(text(history_row(&key, 4)), "mkal  ↦", "too narrow: the alias stays whole and the expansion goes");
        assert_eq!(text(history_row("git status", 4)), "git status");
        let row = history_row(&key, 200);
        assert!(row.iter().any(|s| s.content == "↦" && s.style == ACCENT));
        assert_eq!(row.last().unwrap().style, MUTED);
        assert_eq!(history_insert(&key), "mkal");
    }

    #[test]
    fn history_order_filter_and_safe_display() {
        let p = Picker::new(
            vec!["git status".into()],
            vec!["cargo test".into(), "git status".into()],
            vec!["old".into(), "new".into()],
            String::new(),
        );
        assert_eq!(p.full, ["cargo test", "git status", "new", "old"]);
        assert_eq!(p.suggested, ["git status"]);
        assert_eq!(filter_history(&p.full, "GIT STA"), ["git status"]);
        assert_eq!(filter_history(&p.full, "status git"), ["git status"]);
        assert_eq!(visible("a\x1b[2J\n\u{9b}"), "a^[[2J↵ <U+009B>");
        assert!(filter_history(&p.full, "unmatched").is_empty());
        assert!(FileResults::default().complete);
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
        let line = stripe_line(width, filled, Modifier::empty(), "▀");
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
        let dim = stripe_line(9, 9, Modifier::DIM, "▀");
        assert!(dim.spans.iter().all(|span| span.style.add_modifier.contains(Modifier::DIM)));
        assert_eq!(
            dim.spans.iter().map(|span| span.content.chars().count()).sum::<usize>(),
            9
        );
        assert!(stripe_line(8, 0, Modifier::empty(), "▀").spans.iter().all(|span| span.style == MUTED));
    }
}

#[cfg(test)]
mod worker_tests {
    use super::*;

    #[test]
    fn worker_skips_obsolete_queued_queries() {
        let (started, running) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let (requests, replies) = worker(move |query: &usize| {
            started.send(*query).unwrap();
            if *query == 0 { wait.recv().unwrap(); }
            *query
        });
        requests.send(0).unwrap();
        assert_eq!(running.recv_timeout(Duration::from_secs(2)).unwrap(), 0);
        for query in 1..=10 { requests.send(query).unwrap(); }
        release.send(()).unwrap();
        assert_eq!(replies.recv_timeout(Duration::from_secs(2)).unwrap(), (0, 0));
        assert_eq!(running.recv_timeout(Duration::from_secs(2)).unwrap(), 10);
        assert_eq!(replies.recv_timeout(Duration::from_secs(2)).unwrap(), (10, 10));
        assert!(replies.try_recv().is_err());
    }

    #[test]
    fn callback_runs_off_thread_and_preserves_request_identity() {
        let ui_thread = std::thread::current().id();
        let (release, wait) = mpsc::channel();
        let (requests, replies) = worker(move |query: &(Mode, String, String, Option<String>)| {
            assert_ne!(std::thread::current().id(), ui_thread);
            wait.recv().unwrap();
            query.1.clone()
        });
        let request = (Mode::Grep, "old query".to_owned(), "/scope".into(), Some("file.rs".into()));
        requests.send(request.clone()).unwrap();
        assert!(matches!(replies.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.send(()).unwrap();
        let (identity, result) = replies.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(identity, request);
        assert_ne!(identity, (Mode::Grep, "new query".to_owned(), "/scope".into(), Some("file.rs".into())));
        assert_eq!(result, "old query");
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    #[test]
    fn abbreviate_replaces_home_with_tilde() {
        assert_eq!(abbreviate("/Users/u/Dev", "/Users/u"), "~/Dev");
        assert_eq!(abbreviate("/Users/u", "/Users/u"), "~");
        assert_eq!(abbreviate("/etc", "/Users/u"), "/etc");
    }

    #[test]
    fn widen_scope_stops_at_home_and_outside_it() {
        let home = "/Users/u";
        assert_eq!(widen_scope("/Users/u/Dev/Apps", home), Some("/Users/u/Dev".into()));
        assert_eq!(widen_scope("/Users/u", home), None);
        assert_eq!(widen_scope("/Users/u/Dev", home), Some("/Users/u".into()));
        assert_eq!(widen_scope("/var/log", home), None);
        assert_eq!(widen_scope("/", home), None);
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
