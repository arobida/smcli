pub fn quote(path: &str, shell: &str) -> String {
    let path = if path.starts_with('-') {
        format!("./{path}")
    } else {
        path.into()
    };
    if path.is_empty() {
        return "''".into();
    }
    if path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_./+,:@-".contains(&b))
    {
        return path;
    }
    let control = |c: char| (c as u32) < 32 || c == '\x7f';
    if shell != "fish" && !path.chars().any(control) {
        return format!("'{}'", path.replace('\'', "'\\''"));
    }
    let mut out = String::new();
    if shell != "fish" {
        out.push_str("$'");
    }
    let mut fish_run = false;
    for c in path.chars() {
        let escaped = match c {
            '\n' => "\\n".into(),
            '\t' => "\\t".into(),
            '\r' => "\\r".into(),
            c if control(c) => format!("\\x{:02x}", c as u32),
            '\\' => "\\\\".into(),
            '\'' => "\\'".into(),
            c => c.to_string(),
        };
        if shell == "fish" {
            if control(c) {
                if fish_run {
                    out.push('\'');
                    fish_run = false;
                }
                out.push_str(&escaped);
            } else {
                if !fish_run {
                    out.push('\'');
                    fish_run = true;
                }
                out.push_str(&escaped);
            }
        } else {
            out.push_str(&escaped);
        }
    }
    if shell != "fish" || fish_run {
        out.push('\'');
    }
    out
}

pub fn insertable(before: &str, shell: &str) -> bool {
    #[derive(Clone, Copy, PartialEq)]
    enum Frame {
        Command,
        Single,
        Double,
        DollarSingle,
        Backtick,
        Brace,
    }
    use Frame::*;
    let fish = shell == "fish";
    // shortcut: reject here-doc syntax; parse delimiters if insertion there is needed.
    if !fish && before.replace("\\\n", "").contains("<<") {
        return false;
    }
    let chars: Vec<char> = before.chars().collect();
    let mut stack = vec![Command];
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match *stack.last().unwrap() {
            Command | Brace => match c {
                '\\' => {
                    if next.is_none() {
                        return false;
                    }
                    i += 1;
                }
                '\'' => stack.push(Single),
                '"' => stack.push(Double),
                '`' if !fish => stack.push(Backtick),
                '$' if next == Some('\'') && !fish => {
                    stack.push(DollarSingle);
                    i += 1;
                }
                '$' if next == Some('(') => {
                    stack.push(Command);
                    i += 1;
                }
                '$' if next == Some('{') && !fish => {
                    stack.push(Brace);
                    i += 1;
                }
                '(' => stack.push(Command),
                ')' if stack.len() > 1 && stack.last() == Some(&Command) => {
                    stack.pop();
                }
                '}' if stack.last() == Some(&Brace) => {
                    stack.pop();
                }
                _ => {}
            },
            Single | DollarSingle => {
                if c == '\\' && next.is_some() && (fish || stack.last() == Some(&DollarSingle)) {
                    i += 1;
                } else if c == '\'' {
                    stack.pop();
                }
            }
            Double => {
                if c == '\\' && next.is_some() {
                    i += 1;
                } else if c == '"' {
                    stack.pop();
                } else if c == '$' && next == Some('(') {
                    stack.push(Command);
                    i += 1;
                } else if c == '`' && !fish {
                    stack.push(Backtick);
                } else if c == '$' && next == Some('{') && !fish {
                    stack.push(Brace);
                    i += 1;
                }
            }
            Backtick => {
                if c == '\\' && next.is_some() {
                    i += 1;
                } else if c == '`' {
                    stack.pop();
                }
            }
        }
        i += 1;
    }
    stack.last() == Some(&Command) && !stack.contains(&Backtick)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn safe_words() {
        assert_eq!(quote("-foo", "zsh"), "./-foo");
        assert_eq!(quote("a'b", "bash"), "'a'\\''b'");
        assert_eq!(quote("a\nb", "bash"), "$'a\\nb'");
        assert_eq!(quote("a b", "fish"), "'a b'");
        assert_eq!(quote("a\nb", "fish"), "'a'\\n'b'");
        assert!(!insertable("echo \"", "zsh"));
        assert!(insertable("echo \"$(", "zsh"));
        assert!(!insertable("echo `$(", "zsh"));
        assert!(!insertable("echo \\", "fish"));
    }
}
