/// BM25 terms for shell text: each whole shell word (so `--amend` and `HEAD~1` match exactly)
/// plus its alphanumeric pieces (so `amend` and `head` match too). Label lines from `searchDocument`
/// are skipped: they are for the embedding, and words like "last" or "run" in every document would
/// add keyword noise to queries that use them.
pub struct ShellTokenizer;

impl ShellTokenizer {
    pub const LABELS: [&str; 4] = ["command: ", "subcommand: ", "flags: ", "author: "];

    pub fn tokenize(text: &str) -> Vec<String> {
        let lines: Vec<&str> = text
            .split('\n')
            .enumerate()
            .filter(|(i, line)| *i == 0 || !Self::LABELS.iter().any(|l| line.to_lowercase().starts_with(l)))
            .map(|(_, line)| line)
            .collect();
        let joined = lines.join(" ");
        let mut tokens = Vec::new();
        for word in joined.split_whitespace() {
            let lower = word.to_lowercase();
            tokens.push(lower.clone());
            let pieces: Vec<&str> = word.split(|c: char| !c.is_alphanumeric()).filter(|p| !p.is_empty()).collect();
            if pieces.len() != 1 || pieces[0] != word {
                tokens.extend(pieces.iter().map(|p| p.to_lowercase()));
            }
            for piece in pieces {
                let chars: Vec<(usize, char)> = piece.char_indices().collect();
                let mut start = 0;
                for i in 1..chars.len() {
                    let (_, previous) = chars[i - 1];
                    let (at, current) = chars[i];
                    let next_lower = chars.get(i + 1).is_some_and(|(_, c)| c.is_lowercase());
                    if current.is_uppercase() && (previous.is_lowercase() || previous.is_numeric() || (previous.is_uppercase() && next_lower)) {
                        tokens.push(piece[start..at].to_lowercase());
                        start = at;
                    }
                }
                if start > 0 {
                    tokens.push(piece[start..].to_lowercase());
                }
            }
        }
        tokens
    }

    /// Terms as TalaDB full-text tokens: `t` plus lowercase hex of the UTF-8, which its alphanumeric splitter
    /// keeps whole, so BM25 scores exactly these terms (`--amend`, `-m`) rather than its own pieces.
    pub fn encoded(text: &str) -> String {
        Self::tokenize(text)
            .iter()
            .map(|term| format!("t{}", term.bytes().map(|b| format!("{b:02x}")).collect::<String>()))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::ShellTokenizer;

    #[test]
    fn tokenize_shell_words_and_their_pieces() {
        assert_eq!(
            ShellTokenizer::tokenize("git reset --soft HEAD~1"),
            ["git", "reset", "--soft", "soft", "head~1", "head", "1"]
        );
        assert_eq!(ShellTokenizer::tokenize("git push -f\ncommand: git\nflags: -f"), ["git", "push", "-f", "f"]);
        assert_eq!(
            ShellTokenizer::tokenize("ls\nflags: -a\nintent: list files"),
            ["ls", "intent:", "intent", "list", "files"]
        );
    }

    #[test]
    fn keeps_identifiers_and_emits_case_parts() {
        assert_eq!(ShellTokenizer::tokenize("FooBarTests camelCase HTTPServer"),
            ["foobartests", "foo", "bar", "tests", "camelcase", "camel", "case", "httpserver", "http", "server"]);
    }

    #[test]
    fn encode_terms_as_hex_tokens() {
        assert_eq!(ShellTokenizer::encoded("git -m"), "t676974 t2d6d t6d");
        assert_eq!(ShellTokenizer::encoded("ls\nflags: -a"), "t6c73");
    }
}
