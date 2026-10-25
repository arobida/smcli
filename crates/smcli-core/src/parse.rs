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

/// Conservative record-time alias vocabulary: no evaluation, assignments, quoting or shell operators.
pub fn safe_expansion(text: &str) -> bool {
    !text.is_empty() && text.len() <= 8_192 && text.trim() == text
        && text.bytes().all(|c| c.is_ascii_alphanumeric() || b" _-./,:@%+~^".contains(&c))
        && text.split(' ').all(|word| !word.is_empty() && !word.starts_with('~'))
        && text.split(' ').next().is_some_and(|word| !word.starts_with('-'))
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
    fn expansion_gate_never_accepts_shell_syntax_or_assignments() {
        for text in ["git status", "git log --oneline -20", "git reset --soft HEAD~1", "./tool plain-argument"] {
            assert!(safe_expansion(text), "{text}");
        }
        for text in ["", " git status", "git status ", "git  status", "TOKEN=abc123 ./deploy.sh", "echo $(id)", "echo `id`", "git status; pwd", "git status | cat", "git status > file", "git 'status'", "echo $HOME", "echo ~/secret", "echo\nsecret"] {
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
}
