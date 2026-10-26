//! Plain identifiers from candidate arguments, for one project-evidence lookup.
use std::collections::HashSet;

use smcli_core::parse::shell_words;

const CAP: usize = 64;
const STOP: &[&str] = &[
    "main", "test", "tests", "build", "run", "debug", "release", "all", "clean", "install",
    "check", "help", "init", "start", "dev", "lint", "fmt", "update",
];

/// Plain-identifier arguments, first-seen order, at most 64.
/// Drops the program, flags, paths, numbers, tokens under 4 characters, stop words and shell syntax.
pub fn identifiers(commands: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for command in commands {
        // Pipes, substitutions and redirections are refused whole; nothing is guessed.
        let Some(words) = shell_words(command) else {
            continue;
        };
        for word in arguments(&words) {
            if !keep(word) {
                continue;
            }
            let word = word.to_string();
            if seen.insert(word.clone()) {
                out.push(word);
                if out.len() == CAP {
                    return out;
                }
            }
        }
    }
    out
}

fn arguments(words: &[String]) -> &[String] {
    let mut index = 0;
    while index < words.len() && assignment(&words[index]) {
        index += 1;
    }
    words.get(index + 1..).unwrap_or(&[])
}

fn assignment(word: &str) -> bool {
    let Some(equals) = word.find('=') else {
        return false;
    };
    let mut name = word[..equals].chars();
    matches!(name.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && name.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn keep(word: &str) -> bool {
    word.chars().count() >= 4
        && !is_flag(word)
        && !is_path(word)
        && !is_number(word)
        && !shell_syntax(word)
        && !is_stop(word)
        && plain(word)
}

fn is_flag(word: &str) -> bool {
    word.starts_with('-')
}

/// A slash, a dotfile or `~`, a file suffix (`main.rs`, `notes.bak`), or a bare name tools open.
fn is_path(word: &str) -> bool {
    word.contains('/')
        || word.contains('\\')
        || word.contains(char::is_whitespace)
        || word.starts_with('.')
        || word.starts_with('~')
        || file_suffix(word)
        || bare_file(word)
}

fn file_suffix(word: &str) -> bool {
    word.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty()
            && (1..=8).contains(&ext.len())
            && ext.bytes().all(|b| b.is_ascii_alphanumeric())
    })
}

fn bare_file(word: &str) -> bool {
    const BARE: &[&str] = &[
        "makefile",
        "gnumakefile",
        "dockerfile",
        "gemfile",
        "rakefile",
        "justfile",
        "license",
        "readme",
        "procfile",
    ];
    BARE.iter().any(|name| word.eq_ignore_ascii_case(name))
}

fn is_number(word: &str) -> bool {
    let body = word
        .strip_prefix("0x")
        .or_else(|| word.strip_prefix("0X"))
        .unwrap_or(word);
    if body != word {
        return !body.is_empty() && body.bytes().all(|b| b.is_ascii_hexdigit());
    }
    !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'_' || b == b'.')
        && word.bytes().any(|b| b.is_ascii_digit())
        && !word.contains("..")
        && !word.starts_with('_')
        && !word.ends_with('_')
}

fn shell_syntax(word: &str) -> bool {
    word.contains(|c: char| "$*?~![]{}<>|&;`'\"\\#=".contains(c))
}

fn is_stop(word: &str) -> bool {
    STOP.iter().any(|stop| word.eq_ignore_ascii_case(stop))
}

fn plain(word: &str) -> bool {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::identifiers;

    fn dropped(commands: &[&str]) {
        for command in commands {
            let got = identifiers(&[command]);
            assert!(got.is_empty(), "{command} -> {got:?}");
        }
    }

    #[test]
    fn extracts_symbols_and_skips_the_program() {
        assert_eq!(
            identifiers(&["cargo test dedup_keeps_latest"]),
            ["dedup_keeps_latest"]
        );
        assert_eq!(
            identifiers(&[
                "cargo test dedup_keeps_latest",
                "cargo test old_removed_case"
            ]),
            ["dedup_keeps_latest", "old_removed_case"]
        );
        assert_eq!(
            identifiers(&["pytest test_alpha test_beta"]),
            ["test_alpha", "test_beta"]
        );
        assert_eq!(
            identifiers(&["/usr/bin/cargo test dedup_keeps_latest"]),
            ["dedup_keeps_latest"]
        );
        assert_eq!(
            identifiers(&["my_custom_tool other_symbol"]),
            ["other_symbol"]
        );
        assert!(identifiers(&["dedup_keeps_latest"]).is_empty());
        assert_eq!(
            identifiers(&["cargo test 'dedup_keeps_latest'"]),
            ["dedup_keeps_latest"]
        );
        assert_eq!(
            identifiers(&["cargo test \"dedup_keeps_latest\""]),
            ["dedup_keeps_latest"]
        );
        assert_eq!(
            identifiers(&["cargo test dedup_keeps_latest # note"]),
            ["dedup_keeps_latest"]
        );
        assert_eq!(identifiers(&["make build-image"]), ["build-image"]);
        assert_eq!(identifiers(&["cargo run --bin my-tool"]), ["my-tool"]);
        assert_eq!(identifiers(&["tool _private_helper"]), ["_private_helper"]);
        assert_eq!(identifiers(&["echo foo1"]), ["foo1"]);
        assert_eq!(identifiers(&["echo deadbeef"]), ["deadbeef"]);
        assert_eq!(identifiers(&["cargo +nightly test foo_bar"]), ["foo_bar"]);
        assert_eq!(
            identifiers(&["cargo test --features serde_json"]),
            ["serde_json"]
        );
        assert_eq!(
            identifiers(&["cargo test -- dedup_keeps_latest"]),
            ["dedup_keeps_latest"]
        );
        assert_eq!(
            identifiers(&["FOO=1 BAR=2 /usr/bin/cargo test dedup_keeps_latest"]),
            ["dedup_keeps_latest"]
        );
        assert!(identifiers(&["FOO=1"]).is_empty());
        assert!(identifiers(&[]).is_empty());
    }

    #[test]
    fn drops_paths_flags_and_numbers() {
        dropped(&[
            "cat src/main.rs",
            "bat ./notes.txt ../other.md",
            "vim Cargo.toml",
            "code server.py",
            "open ~/Downloads",
            "nvim .gitignore",
            "cat 'my notes.md'",
            "python test.py",
            "ls Makefile",
            "cat Dockerfile",
            "echo data.custom",
            "echo notes.bak",
            "ls src/",
            "tool src/dedup_keeps_latest",
            "npm i @types/node",
            "ls -la",
            "tool --verbose",
            "tool -test",
            "tool --filter=dedup_keeps_latest",
            "sleep 1000",
            "echo 3.1415",
            "echo 0xDEAD",
            "echo 0xdead",
            "echo 1_000",
            "echo 1.2.3",
            "kill 12345",
            "echo +1234",
            "echo -1234",
            "tool v1.2.3",
            "head -n 20 file.rs",
        ]);
        for ext in [
            "rs", "py", "js", "ts", "go", "toml", "json", "md", "txt", "yaml", "yml", "lock",
            "swift",
        ] {
            dropped(&[&format!("tool file.{ext}")]);
        }
        assert_eq!(
            identifiers(&["cargo test --test ranking tests/ranking.rs"]),
            ["ranking"]
        );
        assert_eq!(
            identifiers(&["cargo test --release --exact dedup_keeps_latest"]),
            ["dedup_keeps_latest"]
        );
        assert_eq!(identifiers(&["git commit -m 'fix it' --amend"]), ["commit"]);
        assert_eq!(
            identifiers(&["cargo test --filter=dedup_keeps_latest other_name"]),
            ["other_name"]
        );
    }

    #[test]
    fn drops_short_tokens_stop_words_and_shell_syntax() {
        for word in [
            "main", "test", "tests", "build", "run", "debug", "release", "all", "clean", "install",
            "check", "help", "init", "start", "dev", "lint", "fmt", "update",
        ] {
            dropped(&[
                &format!("tool {word}"),
                &format!("tool {}", word.to_ascii_uppercase()),
            ]);
        }
        assert_eq!(identifiers(&["tool testing"]), ["testing"]);
        assert_eq!(identifiers(&["tool main_loop"]), ["main_loop"]);
        assert_eq!(identifiers(&["tool rust"]), ["rust"]);
        dropped(&[
            "tool foo",
            "tool ab",
            "tool",
            "",
            "   ",
            "tool 1test_name",
            "echo café_name",
            "echo 测试函数名",
        ]);
        dropped(&[
            "git status && cargo test dedup_keeps_latest",
            "ls | grep dedup_keeps_latest",
            "echo $(dedup_keeps_latest)",
            "echo `dedup_keeps_latest`",
            "cat < dedup_keeps_latest",
            "cmd > dedup_keeps_latest",
            "echo foo; cargo test dedup_keeps_latest",
            "echo 'unterminated",
            "echo foo*",
            "echo $HOME",
            "echo name=value_here",
            "echo foo_bar*",
            "cargo test module::dedup_keeps_latest",
            "echo $'dedup_keeps_latest'",
        ]);
        assert_eq!(
            identifiers(&[
                "ls | grep dedup_keeps_latest",
                "cargo test dedup_keeps_latest"
            ]),
            ["dedup_keeps_latest"]
        );
    }

    #[test]
    fn dedupes_in_order_and_caps_at_64() {
        assert_eq!(
            identifiers(&[
                "tool zebra_case alpha_case zebra_case",
                "tool alpha_case beta_case"
            ]),
            ["zebra_case", "alpha_case", "beta_case"]
        );
        assert_eq!(
            identifiers(&["tool Foo_bar", "tool foo_bar"]),
            ["Foo_bar", "foo_bar"]
        );

        let commands: Vec<String> = (0..80).map(|i| format!("tool ident_{i:04}")).collect();
        let refs: Vec<&str> = commands.iter().map(String::as_str).collect();
        let got = identifiers(&refs);
        assert_eq!(got.len(), 64);
        assert_eq!(got[0], "ident_0000");
        assert_eq!(got[63], "ident_0063");
        assert!(!got.iter().any(|id| id == "ident_0064"));

        let mut room = vec![
            "cat src/main.rs".into(),
            "ls -la".into(),
            "sleep 1000".into(),
        ];
        room.extend(std::iter::repeat_n("tool alpha_name".to_string(), 20));
        room.extend((0..62).map(|i| format!("tool more_{i:04}")));
        room.push("tool last_name_ok".into());
        room.push("tool one_past_cap".into());
        let refs: Vec<&str> = room.iter().map(String::as_str).collect();
        let got = identifiers(&refs);
        assert_eq!(got.len(), 64);
        assert_eq!(got[0], "alpha_name");
        assert_eq!(got[63], "last_name_ok");
        assert!(!got.iter().any(|id| id == "one_past_cap"));
    }
}
