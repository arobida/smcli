//! Interactive no-argument command menu; returns argv for the normal CLI dispatcher.
use crate::picker::{TerminalSession, visible};
use anyhow::Result;
use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    layout::Rect,
    widgets::{List, ListItem, ListState, Paragraph},
};
use std::io::{self, IsTerminal, Write};

struct Item {
    title: &'static str,
    detail: &'static str,
    prompt: Option<&'static str>,
    command: &'static str,
}
const ITEMS: &[Item] = &[
    Item {
        title: "⌕ Search history",
        detail: "Keyword + semantic search across every command",
        prompt: Some("Search for"),
        command: "search",
    },
    Item {
        title: "✦ Suggest",
        detail: "Top commands for this directory, live as you type",
        prompt: None,
        command: "pick",
    },
    Item {
        title: "☰ Show history",
        detail: "Native shell files plus captured metadata",
        prompt: None,
        command: "history",
    },
    Item {
        title: "◆ Choose embedding model",
        detail: "Configure the embedded Potion model",
        prompt: None,
        command: "init",
    },
    Item {
        title: "⚙ Shell integration",
        detail: "Print capture hooks for your shell",
        prompt: None,
        command: "shell-init",
    },
    Item {
        title: "⇪ Import legacy history",
        detail: "Load records from a .jsonl file",
        prompt: Some("Path to .jsonl"),
        command: "import",
    },
    Item {
        title: "↻ Rebuild search index",
        detail: "Re-embed every command with the current model",
        prompt: None,
        command: "reindex",
    },
    Item {
        title: "♥ Check index health",
        detail: "Index coverage, size, and embedding model status",
        prompt: None,
        command: "health",
    },
    Item {
        title: "≡ Manage indexes",
        detail: "List search indexes per model and their size",
        prompt: None,
        command: "indexes",
    },
    Item {
        title: "? Help",
        detail: "Show command-line usage",
        prompt: None,
        command: "--help",
    },
];

pub fn is_interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

/// Returns command arguments (without argv[0]), or None on cancellation.
pub fn run(inline: bool) -> Result<Option<Vec<String>>> {
    let mut selected = 0;
    loop {
        let Some(index) = choose(selected, inline)? else {
            return Ok(None);
        };
        selected = index;
        let item = &ITEMS[index];
        let mut input = String::new();
        if let Some(prompt) = item.prompt {
            print!("\n  ❯ {prompt}: ");
            io::stdout().flush()?;
            if io::stdin().read_line(&mut input)? == 0 {
                return Ok(None);
            }
            input = input.trim().to_owned();
            if input.is_empty() {
                continue;
            }
        }
        let shell = std::env::var("SHELL").unwrap_or_default();
        return Ok(Some(arguments(item.command, &input, &shell)));
    }
}

fn arguments(command: &str, input: &str, shell: &str) -> Vec<String> {
    let mut args = vec![command.to_owned()];
    match command {
        "search" => args.extend(input.split_whitespace().map(str::to_owned)),
        "import" => {
            let path = if input == "~" || input.starts_with("~/") {
                std::env::var("HOME")
                    .map(|home| format!("{home}{}", &input[1..]))
                    .unwrap_or_else(|_| input.to_owned())
            } else {
                input.to_owned()
            };
            args.push(path);
        }
        "shell-init" => args.push(
            match std::path::Path::new(shell)
                .file_name()
                .and_then(|s| s.to_str())
            {
                Some("bash") => "bash",
                Some("fish") => "fish",
                _ => "zsh",
            }
            .into(),
        ),
        _ => {}
    }
    args
}

fn choose(mut selected: usize, inline: bool) -> Result<Option<usize>> {
    let rows = ratatui::crossterm::terminal::size().ok().filter(|&(_, rows)| rows > 0).map_or(24, |(_, rows)| rows);
    // Title, two rows per item, hints.
    let mut terminal = TerminalSession::open(inline, ITEMS.len() as u16 * 2 + 2, rows)?;
    loop {
        terminal.terminal.draw(|frame| {
            let area = frame.area();
            frame.render_widget(
                Paragraph::new("smcli — shell history, remembered")
                    .style(crate::picker::ACCENT),
                Rect::new(area.x, area.y, area.width, 1),
            );
            let items: Vec<_> = ITEMS
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    ListItem::new(format!(
                        "{}. {}\n   {}",
                        i + 1,
                        visible(item.title),
                        visible(item.detail)
                    ))
                })
                .collect();
            let list = List::new(items)
                .highlight_style(crate::picker::SELECTED)
                .highlight_symbol("  ");
            let mut state = ListState::default().with_selected(Some(selected));
            frame.render_stateful_widget(
                list,
                Rect::new(
                    area.x,
                    area.y + 1,
                    area.width,
                    area.height.saturating_sub(2),
                ),
                &mut state,
            );
            frame.render_widget(
                Paragraph::new("↑↓/jk move · enter select · 1-9 jump · q quit"),
                Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
            );
        })?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
            KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(None);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                selected = (selected + ITEMS.len() - 1) % ITEMS.len()
            }
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1) % ITEMS.len(),
            KeyCode::Enter => return Ok(Some(selected)),
            KeyCode::Char(c @ '1'..='9') => return Ok(Some(c as usize - '1' as usize)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dispatch_arguments() {
        assert_eq!(
            arguments("search", " git   status ", ""),
            ["search", "git", "status"]
        );
        assert_eq!(
            arguments("shell-init", "", "/bin/fish"),
            ["shell-init", "fish"]
        );
        assert_eq!(
            arguments("shell-init", "", "/bin/unknown"),
            ["shell-init", "zsh"]
        );
        assert_eq!(
            arguments("import", "a file.jsonl", ""),
            ["import", "a file.jsonl"]
        );
    }
}
