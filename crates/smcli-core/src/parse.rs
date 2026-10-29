use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandFlag {
    pub name: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandBreakdown {
    pub main_command: String,
    pub subcommand: Option<String>,
    pub flags: Vec<CommandFlag>,
    pub targets: Vec<String>,
    pub arguments: Vec<String>,
}

/// A first argument that reads as a verb (`create`, `i`, `run-script`), not a path, package or value.
fn is_subcommand(word: &str) -> bool {
    word.starts_with(|c: char| c.is_ascii_alphabetic()) && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

// shortcut: value options for common tools only; use complete tool schemas for semantic parsing.

fn value_options(key: &str) -> &'static [&'static str] {
    match key {
        "git" => &["-C", "-c", "--git-dir", "--work-tree"],
        "git commit" => &["-m", "--message", "-F", "--file", "--author"],
        "docker" => &["--context"],
        "docker build" => &["-f", "--file", "-t", "--tag"],
        "docker run" => &["--name", "-p", "--publish", "-v", "--volume", "-e", "--env"],
        "npm" => &["--prefix", "--registry"],
        "cargo" => &["--manifest-path", "--target", "--features", "-p", "--package"],
        "swift" => &["-c", "--configuration", "--package-path", "--target", "--product", "--filter"],
        "kubectl" => &["-n", "--namespace", "--context", "-f", "--filename", "-o", "--output"],
        "ls" => &["-I", "--ignore"],
        "grep" => &["-e", "--regexp", "-f", "--file", "-A", "-B", "-C"],
        _ => &[],
    }
}

/// The last path component (`git` for `/usr/bin/git`).
pub(crate) fn base_name(path: &str) -> &str {
    path.rsplit('/').find(|p| !p.is_empty()).unwrap_or(path)
}

impl CommandBreakdown {
    pub fn parse(command: &str) -> Option<Self> {
        let mut words = shell_words(command)?;
        // Assignment prefixes are not executables. Keep the unmodified command in the record.
        while let Some(first) = words.first() {
            let Some(equals) = first.find('=') else { break };
            let head = first.chars().next().unwrap();
            if !(head == '_' || head.is_ascii_alphabetic())
                || !first[..equals].chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
            {
                break;
            }
            words.remove(0);
        }
        if words.is_empty() {
            return None;
        }
        let main = words.remove(0);
        let tool = base_name(&main).to_string();
        let mut subcommand: Option<String> = None;
        let mut flags = Vec::new();
        let mut targets = Vec::new();
        let mut options = true;
        let mut index = 0;
        while index < words.len() {
            let word = &words[index];
            if options && word == "--" {
                options = false;
            } else if options && word.starts_with('-') && word != "-" {
                if let Some(equals) = word.find('=') {
                    flags.push(CommandFlag { name: word[..equals].into(), value: Some(word[equals + 1..].into()) });
                } else if (value_options(&tool).contains(&word.as_str())
                    || value_options(&format!("{tool} {}", subcommand.as_deref().unwrap_or(""))).contains(&word.as_str()))
                    && index + 1 < words.len()
                {
                    index += 1;
                    flags.push(CommandFlag { name: word.clone(), value: Some(words[index].clone()) });
                } else {
                    flags.push(CommandFlag { name: word.clone(), value: None });
                }
            } else if options && subcommand.is_none() && targets.is_empty() && is_subcommand(word) {
                subcommand = Some(word.clone());
            } else {
                targets.push(word.clone());
            }
            index += 1;
        }
        Some(Self { main_command: main, subcommand, flags, targets, arguments: words })
    }
}

/// Option names in a command (`--amend`, `-m`), without their values. Lines the parser refuses
/// (pipes, substitutions) fall back to whitespace-separated words that start with `-`.
pub fn command_flags(command: &str) -> Vec<String> {
    if let Some(breakdown) = CommandBreakdown::parse(command) {
        return breakdown.flags.into_iter().map(|f| f.name).collect();
    }
    command
        .split_whitespace()
        .filter(|w| w.starts_with('-') && *w != "-" && *w != "--")
        .map(|w| w.split('=').next().unwrap_or(w).to_string())
        .collect()
}

/// Tokenize without executing or expanding anything. Compound shell syntax is deliberately not guessed.
pub fn shell_words(command: &str) -> Option<Vec<String>> {
    let characters: Vec<char> = command.chars().collect();
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if quote == Some('\'') {
            if character == '\'' { quote = None } else { word.push(character) }
        } else if character == '\\' {
            index += 1;
            let next = *characters.get(index)?;
            if next != '\n' {
                if quote == Some('"') && !['$', '`', '"', '\\'].contains(&next) {
                    word.push('\\');
                }
                word.push(next);
            }
            started = true;
        } else if character == '`'
            || (character == '$'
                && index + 1 < characters.len()
                && (characters[index + 1] == '(' || (quote.is_none() && ['\'', '"'].contains(&characters[index + 1]))))
        {
            return None;
        } else if let Some(current) = quote {
            if character == current { quote = None } else { word.push(character) }
        } else if character == '\'' || character == '"' {
            quote = Some(character);
            started = true;
        } else if "|&;<>\n(){}".contains(character) {
            return None;
        } else if character.is_whitespace() {
            if started {
                words.push(std::mem::take(&mut word));
                started = false;
            }
        } else if character == '#' && !started {
            break;
        } else {
            word.push(character);
            started = true;
        }
        index += 1;
    }
    if quote.is_some() {
        return None;
    }
    if started {
        words.push(word);
    }
    Some(words)
}

/// Record-time alias vocabulary: variables, quotes and `&&`, `||`, `|`, `;` are kept as text; nothing that runs code
/// (substitution, redirection, background) or assigns (`NAME=`, `${NAME:=}`) passes.
pub fn safe_expansion(text: &str) -> bool {
    let assigns = |word: &str| word.split_once('=').is_some_and(|(name, _)| {
        let name = name.strip_suffix('+').unwrap_or(name);
        name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
    });
    let braces_plain = text.split("${").skip(1).all(|rest| {
        rest.split_once('}').is_some_and(|(name, _)| !name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'))
    });
    !text.is_empty() && text.len() <= 8_192 && text.trim() == text
        && text.bytes().all(|c| (b' '..=b'~').contains(&c))
        && !["$(", "`", "<", ">"].iter().any(|bad| text.contains(bad))
        && !text.replace("&&", "").contains('&')
        && braces_plain
        && text.split(|c: char| c.is_whitespace() || ['(', '{', ';', '&', '|'].contains(&c)).all(|word| !assigns(word.trim_start_matches(['"', '\'', '(', '{', ';', '&', '|'])))
        && text.split_whitespace().next().is_some_and(|word| !word.starts_with('-'))
}

/// True when a command line runs smcli itself (`smcli …`, `/path/to/smcli …`, `swift run smcli …`),
/// so searches and suggestions are not about the tool. Leading `VAR=value` assignments are skipped.
pub fn is_smcli_command(command: &str) -> bool {
    let mut words: Vec<&str> = command.split_whitespace().map(|w| w.trim_matches(|c| c == '\'' || c == '"')).collect();
    while let Some(first) = words.first() {
        match first.find('=') {
            Some(equals) if equals != 0 && !first[..equals].contains('/') => {
                words.remove(0);
            }
            _ => break,
        }
    }
    // `swift run` options may take values (`-c release`), so look for the product among the next few words.
    if words.first() == Some(&"swift") && words.get(1) == Some(&"run") {
        return words.iter().skip(2).take(5).any(|w| *w == "smcli");
    }
    words.first().is_some_and(|w| base_name(w) == "smcli")
}

/// The executable's name without its path (`git` for `/usr/bin/git git status`); for compound
/// shell lines the parser refuses, the first word.
pub fn program_of(command: &str, parsed: Option<&CommandBreakdown>) -> Option<String> {
    if let Some(main) = parsed.map(|p| &p.main_command) {
        return Some(base_name(main).to_string());
    }
    command.split_whitespace().next().map(|w| base_name(w).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_arguments_use_placeholder_detection() {
        assert_eq!(path_arguments("/bin/tool --out=a.txt 'some dir' | cat b.md", "bash", |p| p == "some dir"), ["a.txt", "some dir", "b.md"]);
    }

    #[test]
    fn last_command_segments() {
        assert_eq!(last_command_word("echo 'a|b'; cd @", "bash").as_deref(), Some("cd"));
        assert_eq!(last_command_word("cat \"a;b\"", "bash").as_deref(), Some("cat"));
        assert_eq!(last_command_word("cat a && mkdir @", "bash").as_deref(), Some("mkdir"));
        assert_eq!(last_command_word("cat a | ", "bash"), None);
    }

    #[test]
    fn parse_commands_and_flags() {
        let parsed = CommandBreakdown::parse("FOO=1 /usr/bin/git commit -m 'fix it' --amend file").unwrap();
        assert_eq!(parsed.main_command, "/usr/bin/git");
        assert_eq!(parsed.subcommand.as_deref(), Some("commit"));
        assert_eq!(parsed.flags, [
            CommandFlag { name: "-m".into(), value: Some("fix it".into()) },
            CommandFlag { name: "--amend".into(), value: None },
        ]);
        assert_eq!(parsed.targets, ["file"]);
        assert_eq!(CommandBreakdown::parse("bun create cloudflare@latest").unwrap().subcommand.as_deref(), Some("create"));
        assert_eq!(CommandBreakdown::parse("bun i -g x").unwrap().subcommand.as_deref(), Some("i"));
        assert_eq!(CommandBreakdown::parse("cat ./notes.txt").unwrap().subcommand, None);
        assert_eq!(CommandBreakdown::parse("npx @scope/pkg init").unwrap().subcommand, None);
        assert!(CommandBreakdown::parse("ls | wc -l").is_none());
        assert!(CommandBreakdown::parse("echo $(date)").is_none());
        assert_eq!(command_flags("ls -la | grep --color=auto x"), ["-la", "--color"]);
        assert_eq!(shell_words(r#"echo "a\$b\c" # comment"#).unwrap(), ["echo", r"a$b\c"]);
    }

    #[test]
    fn expansion_gate_keeps_text_but_rejects_code_and_assignments() {
        for text in ["git status", "git log --oneline -20", "git reset --soft HEAD~1", "./tool plain-argument",
            r#"$EDITOR $ZDOTDIR/conf.d/50-aliases.zsh && source "$ZDOTDIR/.zshrc""#, "git status | cat", "git status; pwd",
            "make || true", "git 'status'", "echo $HOME", "nvim ~/.zshrc", "cd ${HOME}/x", "git log --format=%h"] {
            assert!(safe_expansion(text), "{text}");
        }
        for text in ["", " git status", "git status ", "TOKEN=abc123 ./deploy.sh", "a && TOKEN=x ./b", "env 'X=1' a",
            "echo $(id)", "echo `id`", "git status > file", "cat < file", "diff <(a) b", "sleep 1 &", "echo ${X:=1}",
            "echo ${X:-$(id)}", "echo\nsecret", "echo\tx", "-rf /", "(TOKEN=abc printf ok)",
            "TOKEN+=abc printf ok", "printf ok;TOKEN=abc", "printf ok &&(TOKEN+=abc printf ok)"] {
            assert!(!safe_expansion(text), "{text}");
        }
        assert!(!safe_expansion(&"x".repeat(8_193)));
    }

    #[test]
    fn detect_smcli_commands() {
        assert!(is_smcli_command("smcli search x"));
        assert!(is_smcli_command("A=1 /opt/bin/smcli"));
        assert!(is_smcli_command("swift run -c release smcli history"));
        assert!(!is_smcli_command("git status"));
        assert_eq!(program_of("/usr/bin/git status", CommandBreakdown::parse("/usr/bin/git status").as_ref()).as_deref(), Some("git"));
    }

    #[test]
    fn path_placeholders_replace_paths_and_preserve_non_paths() {
        let exists = |w: &str| w == "src";
        let run = |cmd| path_placeholders(cmd, "bash", exists).map(|(s, _)| s);
        assert_eq!(run("cp notes/a.md backup/").as_deref(), Some("cp @ @"));
        assert_eq!(run(r#"rm -rf "old file.md""#).as_deref(), Some("rm -rf @"));
        assert_eq!(run("vim ~/x.toml").as_deref(), Some("vim @"));
        assert_eq!(run("git log"), None);
        assert_eq!(run("ls src").as_deref(), Some("ls @"));
        assert_eq!(run("npm i @scope/pkg ./a.md").as_deref(), Some("npm i @scope/pkg @"));
        assert_eq!(run("FOO=./a.md cat b.txt").as_deref(), Some("FOO=./a.md cat @"));
        assert_eq!(run("cargo build --manifest-path=x/Cargo.toml").as_deref(), Some("cargo build --manifest-path=@"));
        assert_eq!(run("git clone https://x.com/r.git"), None);
        assert_eq!(run("echo hi > out.log").as_deref(), Some("echo hi > @"));
        assert_eq!(run("curl --url=https://x.com/a"), None);
        assert_eq!(run("find --name=*.rs"), None);
        assert_eq!(path_placeholders("ls -a", "bash", |_| true), None);
        assert_eq!(run("cmd --out=\"x/a\"").as_deref(), Some("cmd --out=@"));
    }

    #[test]
    fn path_placeholders_mixed_word_substitution() {
        let result = path_placeholders(r#"echo prefix"$(cat a.md)""#, "bash", |_| false);
        assert_eq!(result.as_ref().map(|v| v.0.as_str()), Some(r#"echo prefix"$(cat @)""#));
    }

    #[test]
    fn path_placeholders_restores_outer_redirection() {
        let result = path_placeholders("echo > $(echo a.md) b.md", "bash", |_| false);
        assert_eq!(result.as_ref().map(|v| v.0.as_str()), Some("echo > $(echo @) @"));
    }

    #[test]
    fn path_placeholders_escaped_dollar_is_literal() {
        let result = path_placeholders(r"cat \$HOME/x.md $HOME/y.md", "bash", |_| false);
        assert_eq!(result.as_ref().map(|v| v.0.as_str()), Some("cat @ $HOME/y.md"));
        let result = path_placeholders(r"cmd --out=\$HOME/x.md", "bash", |_| false);
        assert_eq!(result.as_ref().map(|v| v.0.as_str()), Some("cmd --out=@"));
    }

    #[test]
    fn path_placeholders_command_substitutions_and_groups() {
        let exists = |w: &str| w == "src";
        let run = |cmd, shell| path_placeholders(cmd, shell, exists).map(|(s, _)| s);
        assert_eq!(run(r#"eval "$(/a/b/smcli shell-init zsh)""#, "bash").as_deref(), Some(r#"eval "$(@ shell-init zsh)""#));
        assert_eq!(run(r#"cat "$(ls notes/)""#, "bash").as_deref(), Some(r#"cat "$(ls @)""#));
        assert_eq!(run("cat `ls notes/`", "bash").as_deref(), Some("cat `ls @`"));
        assert_eq!(run("diff <(sort a.txt) <(sort b.txt)", "bash").as_deref(), Some("diff <(sort @) <(sort @)"));
        assert_eq!(run(r#"rm "old file.md""#, "bash").as_deref(), Some("rm @"));
        assert_eq!(run("cp $HOME/x.md dir/", "bash").as_deref(), Some("cp $HOME/x.md @"));
        assert_eq!(run("(cd src && make)", "bash").as_deref(), Some("(cd @ && make)"));
        assert_eq!(run("./run.sh a.md", "bash").as_deref(), Some("@ @"));
        assert_eq!(run("git log", "bash"), None);
        assert_eq!(run(r#"echo "$(ls notes/""#, "bash"), None);
        assert_eq!(run(r#"eval "$(/a/b/smcli shell-init zsh)""#, "zsh").as_deref(), Some(r#"eval "$(@ shell-init zsh)""#));
        assert_eq!(run("cat (ls notes/)", "fish").as_deref(), Some("cat (ls @)"));
        assert_eq!(run(r#"echo "a/b""#, "fish").as_deref(), Some("echo @"));
        assert_eq!(run(r#"echo "(ls notes/)""#, "fish").as_deref(), Some(r#"echo "(ls @)""#));
        assert_eq!(run("echo abc\"unfinished", "bash"), None);
        assert_eq!(run("echo a.md )", "bash"), None);
        assert_eq!(run(r#"echo "prefix/a $(cat b.md) suffix/c""#, "bash").as_deref(), Some(r#"echo "prefix/a $(cat @) suffix/c""#));
        assert_eq!(run(r#"eval "$(/a/b/smcli shell-init zsh)""#, "fish").as_deref(), Some(r#"eval "$(@ shell-init zsh)""#));
    }
}

/// Shell-like glob match supporting `**`, `*`, `?` and `[abc]` / `[a-z]` / `[!x]` classes.
/// Both `pat` and `text` use `/` separators.
pub fn glob_match(pat: &str, text: &str) -> bool {
    if pat.starts_with("**") {
        let rest = pat[2..].strip_prefix('/').unwrap_or(&pat[2..]);
        if rest.is_empty() {
            return true;
        }
        let mut s = text;
        loop {
            if glob_match(rest, s) {
                return true;
            }
            let Some(c) = s.chars().next() else { return false };
            s = &s[c.len_utf8()..];
        }
    }
    if pat.starts_with('*') {
        let rest = &pat[1..];
        let mut s = text;
        loop {
            if glob_match(rest, s) {
                return true;
            }
            match s.chars().next() {
                Some('/') | None => return false,
                Some(c) => s = &s[c.len_utf8()..],
            }
        }
    }
    let Some(pc) = pat.chars().next() else {
        return text.is_empty();
    };
    if pc == '?' {
        return match text.chars().next() {
            Some('/') | None => false,
            Some(c) => glob_match(&pat[pc.len_utf8()..], &text[c.len_utf8()..]),
        };
    }
    if pc == '[' {
        let Some(tc) = text.chars().next() else { return false };
        if tc == '/' {
            return false;
        }
        return match glob_class_eat(pat, tc) {
            Some((true, rest)) => glob_match(rest, &text[tc.len_utf8()..]),
            Some((false, _)) => false,
            None => tc == '[' && glob_match(&pat[1..], &text[tc.len_utf8()..]),
        };
    }
    match text.chars().next() {
        Some(tc) if tc == pc => glob_match(&pat[pc.len_utf8()..], &text[tc.len_utf8()..]),
        _ => false,
    }
}

fn glob_class_eat(pat: &str, c: char) -> Option<(bool, &str)> {
    let b = pat.as_bytes();
    if b.first() != Some(&b'[') {
        return None;
    }
    let mut i = 1;
    let neg = i < b.len() && matches!(b[i], b'!' | b'^');
    if neg {
        i += 1;
    }
    let start = i;
    let mut found = false;
    while i < b.len() {
        if b[i] == b']' && i > start {
            return Some((found != neg, &pat[i + 1..]));
        }
        if i + 2 < b.len() && b[i + 1] == b'-' && b[i + 2] != b']' {
            if c.is_ascii() && b[i] as char <= c && c <= b[i + 2] as char {
                found = true;
            }
            i += 3;
        } else {
            if c == b[i] as char {
                found = true;
            }
            i += 1;
        }
    }
    None
}

/// Replace a command's path arguments with `@` placeholders so they can be re-filled in the shell.
/// Returns `Some((rewritten_command, first_placeholder_byte_offset))` when at least one path was
/// found; otherwise the command is returned unchanged as `None`.
pub fn path_placeholders<F: FnMut(&str) -> bool>(command: &str, shell: &str, exists: F) -> Option<(String, usize)> {
    path_placeholders_with(command, shell, exists, |_| {})
}

pub fn path_arguments(command: &str, shell: &str, exists: impl FnMut(&str) -> bool) -> Vec<String> {
    let mut paths = Vec::new();
    path_placeholders_with(command, shell, exists, |path| paths.push(path.to_string()));
    paths
}

fn path_placeholders_with(command: &str, shell: &str, mut exists: impl FnMut(&str) -> bool, mut argument: impl FnMut(&str)) -> Option<(String, usize)> {
    let tokens = tokenize(command, shell)?;
    let mut replacements: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut command_expected = true;
    let mut redirection_expected = false;
    let mut frames = Vec::new();

    for token in tokens {
        match token {
            Token::Open => {
                frames.push((command_expected, redirection_expected));
                command_expected = true;
                redirection_expected = false;
            }
            Token::Close => {
                (command_expected, redirection_expected) = frames.pop()?;
                if redirection_expected { redirection_expected = false; } else { command_expected = false; }
            }
            Token::Opaque => { command_expected = false; redirection_expected = false; }
            Token::Op(range) => {
                let op = &command[range.clone()];
                if op != "\"" && op != "'" {
                    if is_command_separator(op) || is_command_frame_opener(op) {
                        command_expected = true;
                    } else {
                        command_expected = false;
                    }
                }
                redirection_expected = is_redirection(op);
            }
            Token::Whitespace => {}
            Token::Word { full, text } => {
                let was_command = command_expected;
                let raw = &command[full.clone()];
                let variable = |value: &str| {
                    let raw_value = raw.split_once('=').map_or(raw, |(_, value)| value);
                    value.starts_with('$') && !raw_value.starts_with('\\') && !raw_value.starts_with('\'') && !raw_value.starts_with("\"\\$")
                };
                let mut replacement = "@".to_string();
                let candidate = if redirection_expected {
                    Some(full.clone())
                } else if command_expected {
                    if is_assignment(&text) {
                        None
                    } else if text.starts_with('-') || text.starts_with('@') || variable(&text) || is_url(&text) || is_remote(&text) || is_glob(&text) {
                        command_expected = false;
                        None
                    } else if text.contains('/') && is_path(&text, &mut exists) {
                        command_expected = false;
                        replacement = "@".to_string();
                        Some(full.clone())
                    } else {
                        command_expected = false;
                        None
                    }
                } else if let Some((name_len, value)) = flag_value(&text) {
                    // Replace only the value part of `--flag=value`.
                    if !value.starts_with('@') && !variable(value) && !is_url(value) && !is_remote(value) && !is_glob(value) && is_path(value, &mut exists) {
                        replacement = format!("{}=@", &text[..name_len]);
                        Some(full.clone())
                    } else {
                        None
                    }
                } else if text.starts_with('-') || text.starts_with('@') || variable(&text) || is_url(&text) || is_remote(&text) || is_glob(&text) {
                    None
                } else if is_path(&text, &mut exists) {
                    Some(full.clone())
                } else {
                    None
                };
                if let Some(range) = candidate {
                    if !was_command {
                        argument(flag_value(&text).map_or(text.as_str(), |(_, value)| value));
                    }
                    replacements.push((range, replacement));
                }
                redirection_expected = false;
            }
        }
    }

    let mut out = String::new();
    let mut last = 0;
    let mut first_placeholder = None;
    for (range, repl) in replacements {
        out.push_str(&command[last..range.start]);
        out.push_str(&repl);
        if first_placeholder.is_none() {
            first_placeholder = Some(out.len() - 1);
        }
        last = range.end;
    }
    if first_placeholder.is_none() {
        return None;
    }
    out.push_str(&command[last..]);
    Some((out, first_placeholder.unwrap()))
}

/// First word of the last command segment, ignoring separators inside quoted words.
pub fn last_command_word(command: &str, shell: &str) -> Option<String> {
    let mut first = None;
    for token in tokenize(command, shell)? {
        match token {
            Token::Op(range) if is_command_separator(&command[range.clone()]) => first = None,
            Token::Word { text, .. } if first.is_none() => first = Some(text),
            _ => {}
        }
    }
    first
}

#[derive(Debug)]
enum Token {
    Open,
    Close,
    Opaque,
    Op(std::ops::Range<usize>),
    Whitespace,
    Word {
        full: std::ops::Range<usize>,
        text: String,
    },
}

fn tokenize(command: &str, shell: &str) -> Option<Vec<Token>> {
    let mut i = 0;
    tokenize_context(command, shell, &mut i, None)
}

fn tokenize_context(
    command: &str,
    shell: &str,
    i: &mut usize,
    stop: Option<char>,
) -> Option<Vec<Token>> {
    let stop_str = stop.map(|c| if c == ')' { ")" } else { "`" });
    let mut tokens = Vec::new();
    while *i < command.len() {
        if let Some(s) = stop_str {
            if command.get(*i..*i + 1) == Some(s) {
                break;
            }
        }
        let c = command[*i..].chars().next()?;
        if c.is_whitespace() {
            *i += c.len_utf8();
            while *i < command.len() && command[*i..].chars().next().unwrap().is_whitespace() {
                *i += command[*i..].chars().next().unwrap().len_utf8();
            }
            tokens.push(Token::Whitespace);
            continue;
        }
        if let Some(op_len) = command_opener_len(command, *i, shell) {
            let op_end = *i + op_len;
            tokens.push(Token::Open);
            *i = op_end;
            let inner_stop = if command.get(op_end - 1..op_end) == Some("`") {
                '`'
            } else {
                ')'
            };
            let inner = tokenize_context(command, shell, i, Some(inner_stop))?;
            tokens.extend(inner);
            let inner_stop_str = if inner_stop == ')' { ")" } else { "`" };
            if command.get(*i..*i + 1) != Some(inner_stop_str) {
                return None;
            }
            tokens.push(Token::Close);
            *i += 1;
            continue;
        }
        if c == ')' && stop.is_none() {
            return None;
        }
        if is_operator_char(c) {
            let start = *i;
            *i += c.len_utf8();
            if *i < command.len() {
                let c2 = command[*i..].chars().next().unwrap();
                if (c == '&' && c2 == '&')
                    || (c == '|' && c2 == '|')
                    || (c == '>' && c2 == '>')
                    || (c == '<' && c2 == '<')
                {
                    *i += c2.len_utf8();
                }
            }
            tokens.push(Token::Op(start..*i));
            continue;
        }
        if c == '"' {
            if let Some((full, text, new_i)) = read_double_quoted_word(command, shell, *i) {
                tokens.push(Token::Word { full, text });
                *i = new_i;
                continue;
            } else {
                tokens.push(Token::Op(*i..*i + 1));
                *i += 1;
                let inner = parse_double_quoted_inner(command, shell, i)?;
                tokens.extend(inner);
                if command.get(*i..*i + 1) != Some("\"") {
                    return None;
                }
                tokens.push(Token::Op(*i..*i + 1));
                *i += 1;
                continue;
            }
        }
        if c == '\'' {
            let (full, text, new_i) = read_single_quoted_word(command, *i)?;
            tokens.push(Token::Word { full, text });
            *i = new_i;
            continue;
        }
        let (full, text, new_i) = read_plain_word(command, *i, shell)?;
        if command.get(new_i..new_i + 1) == Some("\"") {
            tokens.push(Token::Opaque);
        } else {
            tokens.push(Token::Word { full, text });
        }
        *i = new_i;
    }
    Some(tokens)
}

fn command_opener_len(command: &str, i: usize, _shell: &str) -> Option<usize> {
    if command.get(i..i + 2) == Some("$(")
        || command.get(i..i + 2) == Some("<(")
        || command.get(i..i + 2) == Some(">(")
    {
        return Some(2);
    }
    if command.get(i..i + 1) == Some("(") || command.get(i..i + 1) == Some("`") {
        return Some(1);
    }
    None
}

fn read_plain_word(
    command: &str,
    start: usize,
    shell: &str,
) -> Option<(std::ops::Range<usize>, String, usize)> {
    let mut text = String::new();
    let mut i = start;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    while i < command.len() {
        let c = command[i..].chars().next().unwrap();
        if escaped {
            if quote != Some('\'') {
                text.push(c);
            }
            escaped = false;
            i += c.len_utf8();
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escaped = true;
            i += 1;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                text.push(c);
            }
            i += c.len_utf8();
            continue;
        }
        if c == '"' && read_double_quoted_word(command, shell, i).is_none() {
            break;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
            i += c.len_utf8();
            continue;
        }
        if c.is_whitespace() || is_operator_char(c) {
            break;
        }
        if c == '$' && command.get(i + 1..i + 2) == Some("(") {
            break;
        }
        text.push(c);
        i += c.len_utf8();
    }
    if quote.is_some() || escaped { return None; }
    Some((start..i, text, i))
}

fn read_single_quoted_word(
    command: &str,
    start: usize,
) -> Option<(std::ops::Range<usize>, String, usize)> {
    let mut i = start + 1;
    let mut text = String::new();
    while i < command.len() {
        let c = command[i..].chars().next()?;
        if c == '\'' {
            return Some((start..i + 1, text, i + 1));
        }
        text.push(c);
        i += c.len_utf8();
    }
    None
}

fn read_double_quoted_word(
    command: &str,
    shell: &str,
    start: usize,
) -> Option<(std::ops::Range<usize>, String, usize)> {
    let mut i = start + 1;
    let mut text = String::new();
    let mut escaped = false;
    while i < command.len() {
        let c = command[i..].chars().next()?;
        if escaped {
            text.push(c);
            escaped = false;
            i += c.len_utf8();
            continue;
        }
        if c == '\\' {
            escaped = true;
            i += 1;
            continue;
        }
        if c == '$' && command.get(i + 1..i + 2) == Some("(") {
            return None;
        }
        if c == '<' && command.get(i + 1..i + 2) == Some("(") {
            return None;
        }
        if c == '>' && command.get(i + 1..i + 2) == Some("(") {
            return None;
        }
        if c == '`' || (shell == "fish" && c == '(') {
            return None;
        }
        if c == '"' {
            return Some((start..i + 1, text, i + 1));
        }
        text.push(c);
        i += c.len_utf8();
    }
    None
}

fn parse_double_quoted_inner(
    command: &str,
    shell: &str,
    i: &mut usize,
) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    while *i < command.len() {
        let c = command[*i..].chars().next()?;
        if c == '"' {
            break;
        }
        if command.get(*i..*i + 2) == Some("$(")
            || command.get(*i..*i + 2) == Some("<(")
            || command.get(*i..*i + 2) == Some(">(")
            || c == '`'
            || (shell == "fish" && c == '(')
        {
            let op_end = if c == '`' || c == '(' { *i + 1 } else { *i + 2 };
            tokens.push(Token::Open);
            *i = op_end;
            let stop = if c == '`' { '`' } else { ')' };
            let inner = tokenize_context(command, shell, i, Some(stop))?;
            tokens.extend(inner);
            let stop_str = if stop == ')' { ")" } else { "`" };
            if command.get(*i..*i + 1) != Some(stop_str) {
                return None;
            }
            tokens.push(Token::Close);
            *i += 1;
            continue;
        }
        if c == '\\' {
            *i += 1;
            let next = command[*i..].chars().next()?;
            *i += next.len_utf8();
            continue;
        }
        *i += c.len_utf8();
    }
    Some(tokens)
}

fn is_operator_char(c: char) -> bool {
    matches!(c, '|' | '&' | ';' | '(' | ')' | '>' | '<' | '`')
}

fn is_command_separator(op: &str) -> bool {
    matches!(op, "|" | "||" | "&&" | ";" | "&")
}

fn is_command_frame_opener(op: &str) -> bool {
    op == "$(" || op == "<(" || op == ">(" || op == "(" || op == "`"
}

fn is_redirection(op: &str) -> bool {
    op == ">" || op == ">>" || op == "<" || op == "<<" || op == "&>"
}

fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else { return false };
    let name = name.strip_suffix('+').unwrap_or(name);
    if name.is_empty() {
        return false;
    }
    let mut ch = name.chars();
    let first = ch.next().unwrap();
    (first == '_' || first.is_ascii_alphabetic()) && ch.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn flag_value(word: &str) -> Option<(usize, &str)> {
    if !word.starts_with('-') || word == "-" {
        return None;
    }
    let (name, value) = word.split_once('=')?;
    Some((name.len(), value))
}

fn is_path<F: FnMut(&str) -> bool>(text: &str, exists: &mut F) -> bool {
    looks_like_path(text) || exists(text)
}

fn looks_like_path(text: &str) -> bool {
    text.contains('/')
        || text.starts_with('.')
        || text == "~"
        || text.starts_with("~/")
        || has_extension(text)
}

fn has_extension(text: &str) -> bool {
    let Some((before, after)) = text.rsplit_once('.') else { return false };
    if before.is_empty() || after.is_empty() {
        return false;
    }
    if after.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    after.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

fn is_url(text: &str) -> bool {
    text.contains("://")
}

fn is_remote(text: &str) -> bool {
    text.split_once('@')
        .and_then(|(_, after)| after.split_once(':').map(|(host, _)| !host.contains('/')))
        .unwrap_or(false)
}

fn is_glob(text: &str) -> bool {
    text.chars().any(|c| matches!(c, '*' | '?' | '['))
}
