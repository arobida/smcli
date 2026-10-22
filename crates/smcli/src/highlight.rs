use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};

/// A display lexer, not a shell parser: preserve every byte, including incomplete input.
pub(crate) fn highlight(cmd: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut i = 0;
    let mut command = true;
    let mut subcommand = false;
    let mut stack = Vec::new();
    let mut redirect = false;
    while i < cmd.len() {
        let start = i;
        let c = cmd[i..].chars().next().unwrap();
        let mut style = Style::default();
        if c.is_whitespace() {
            i += c.len_utf8();
        } else if c == '#' && (i == 0 || cmd[..i].ends_with(char::is_whitespace)) {
            i = cmd[i..].find('\n').map_or(cmd.len(), |n| i + n);
            style = style.add_modifier(Modifier::DIM | Modifier::ITALIC);
        } else if c == '\'' || c == '"' {
            i += 1;
            while i < cmd.len() {
                let ch = cmd[i..].chars().next().unwrap();
                i += ch.len_utf8();
                if ch == c {
                    break;
                }
                if ch == '\\' && c == '"' {
                    if let Some(next) = cmd[i..].chars().next() {
                        i += next.len_utf8();
                    }
                }
            }
            style = style.fg(Color::Green);
            if !redirect {
                command = false;
                subcommand = false;
            }
            redirect = false;
        } else if cmd[i..].starts_with("$(") {
            i += 2;
            stack.push((false, false, redirect));
            command = true;
            subcommand = false;
            redirect = false;
            style = style.fg(Color::Red);
        } else if c == '$' {
            i += 1;
            if cmd[i..].starts_with('{') {
                i += 1;
                while let Some(ch) = cmd[i..].chars().next() {
                    i += ch.len_utf8();
                    if ch == '}' {
                        break;
                    }
                }
            } else {
                while let Some(ch) = cmd[i..].chars().next() {
                    if !(ch.is_alphanumeric() || ch == '_') {
                        break;
                    }
                    i += ch.len_utf8();
                }
                if i == start + 1 {
                    if let Some(ch) = cmd[i..].chars().next() {
                        if "?@*#$!-".contains(ch) {
                            i += ch.len_utf8();
                        }
                    }
                }
            }
            style = style.fg(Color::Magenta);
            if !redirect {
                command = false;
                subcommand = false;
            }
            redirect = false;
        } else if "|&;><()".contains(c) {
            i += 1;
            if matches!(c, '|' | '&' | '>' | '<') && cmd[i..].starts_with(c) {
                i += 1;
            }
            style = style.fg(Color::Red);
            match c {
                ')' => {
                    if let Some(state) = stack.pop() {
                        (command, subcommand, redirect) = state;
                    }
                }
                '>' | '<' => redirect = true,
                _ => {
                    command = true;
                    subcommand = false;
                    redirect = false;
                }
            }
        } else {
            while let Some(ch) = cmd[i..].chars().next() {
                if ch.is_whitespace() || "'\"$|&;><()".contains(ch) {
                    break;
                }
                i += ch.len_utf8();
                if ch == '\\' {
                    if let Some(next) = cmd[i..].chars().next() {
                        i += next.len_utf8();
                    }
                }
            }
            let word = &cmd[start..i];
            style = if command && !redirect {
                Style::default().add_modifier(Modifier::BOLD)
            } else if word.starts_with('-') {
                style.fg(Color::Yellow)
            } else if word.contains('/') || word.starts_with('~') {
                style.fg(Color::Blue).add_modifier(Modifier::UNDERLINED)
            } else if subcommand
                && !redirect
                && word.chars().all(|ch| ch.is_alphanumeric() || ch == '-')
            {
                style.fg(Color::Cyan)
            } else {
                style
            };
            if !redirect {
                subcommand = command;
                command = false;
            }
            redirect = false;
        }
        spans.push(Span::styled(cmd[start..i].to_owned(), style));
    }
    spans
}

/// Let the selected row supply its foreground without losing token modifiers.
pub(crate) fn plain(mut spans: Vec<Span<'static>>) -> Vec<Span<'static>> {
    for span in &mut spans {
        span.style.fg = None;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(input: &str) -> Vec<Span<'static>> {
        let spans = highlight(input);
        assert_eq!(
            spans.iter().map(|s| s.content.as_ref()).collect::<String>(),
            input
        );
        let selected = plain(spans.clone());
        for (before, after) in spans.iter().zip(&selected) {
            let mut expected = before.style;
            expected.fg = None;
            assert_eq!(after.style, expected);
            assert_eq!(before.content, after.content);
        }
        spans
    }

    fn token(spans: &[Span<'_>], text: &str, color: Option<Color>, modifiers: Modifier) {
        let span = spans.iter().find(|s| s.content == text).unwrap();
        assert_eq!(span.style.fg, color, "{text}");
        assert_eq!(span.style.add_modifier, modifiers, "{text}");
    }

    #[test]
    fn git_status() {
        let s = check("git status --short");
        token(&s, "git", None, Modifier::BOLD);
        token(&s, "status", Some(Color::Cyan), Modifier::empty());
        token(&s, "--short", Some(Color::Yellow), Modifier::empty());
    }

    #[test]
    fn substitution() {
        let s = check("eval $(x shell-init zsh) -- /p history");
        token(&s, "x", None, Modifier::BOLD);
        token(&s, "shell-init", Some(Color::Cyan), Modifier::empty());
        token(&s, "$(", Some(Color::Red), Modifier::empty());
        token(&s, "/p", Some(Color::Blue), Modifier::UNDERLINED);
        token(&s, "history", None, Modifier::empty());
    }

    #[test]
    fn quotes_and_pipeline() {
        let s = check("printf '%s\\n' \"$HOME\" | rg --files");
        token(&s, "'%s\\n'", Some(Color::Green), Modifier::empty());
        token(&s, "\"$HOME\"", Some(Color::Green), Modifier::empty());
        token(&s, "rg", None, Modifier::BOLD);
        token(&s, "--files", Some(Color::Yellow), Modifier::empty());
    }

    #[test]
    fn quoted_search() {
        let s = check("semble search \"a b\" | jq");
        token(&s, "search", Some(Color::Cyan), Modifier::empty());
        token(&s, "\"a b\"", Some(Color::Green), Modifier::empty());
        token(&s, "jq", None, Modifier::BOLD);
    }

    #[test]
    fn incomplete_and_unicode_round_trip() {
        for input in [
            "",
            "écho 🦀/文件",
            "echo 'unfinished",
            "echo \"尾\\",
            "${尾",
            "echo $",
            r"echo \",
            "# café\necho ok",
            "echo $(x $(y)) && z; a || b",
            "cat > /tmp/x",
            "echo $HOME ${USER}",
        ] {
            // Also exercise every UTF-8-safe truncated input.
            for end in input
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(input.len()))
            {
                check(&input[..end]);
            }
        }
    }
}
