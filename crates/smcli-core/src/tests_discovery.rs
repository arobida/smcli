/// Finds test definitions using intentionally small, language-specific patterns.
pub fn test_names(file_name: &str, text: &str) -> Vec<String> {
    let rust = file_name.ends_with(".rs");
    let python = file_name.ends_with(".py");
    let js = [".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs"].iter().any(|ext| file_name.ends_with(ext));
    let lines: Vec<&str> = text.lines().collect();
    let mut names = Vec::new();
    if rust {
        for (i, line) in lines.iter().enumerate() {
            let marker = line.trim();
            if marker == "#[test]" || marker == "#[tokio::test]" {
                if let Some(next) = lines.get(i + 1) {
                    if let Some(name) = fn_name(next.trim()) { push_valid(&mut names, name); }
                }
            }
        }
    } else if python {
        for line in lines {
            if let Some(rest) = line.trim().strip_prefix("def test_") {
                if let Some(name) = rest.split('(').next() { push_valid(&mut names, &format!("test_{name}")); }
            }
        }
    } else if js {
        for line in lines {
            let line = line.trim();
            for prefix in ["it('", "test('"] {
                if let Some(rest) = line.strip_prefix(prefix) {
                    if let Some(name) = rest.split('\'').next() { push_valid(&mut names, name); }
                }
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Makes a test-runner command for a recognized project manifest kind.
pub fn fn_test_command(manifest_kind: &str, name: &str) -> Option<String> {
    if !valid(name) { return None; }
    match manifest_kind {
        "cargo" => Some(format!("cargo test {name}")),
        "python" => Some(format!("pytest -k {name}")),
        "npm" => Some(format!("npm test -- -t {name}")),
        _ => None,
    }
}

fn fn_name(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("fn ")?;
    let end = rest.find('(')?;
    let name = &rest[..end];
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') { return None; }
    Some(name)
}

fn push_valid(names: &mut Vec<String>, name: &str) {
    if valid(name) { names.push(name.to_owned()); }
}

fn valid(name: &str) -> bool {
    let Some(first) = name.chars().next() else { return false };
    (first.is_alphabetic() || first.is_ascii_digit() || first == '_')
        && name.chars().all(|c| c.is_ascii() && (c.is_alphabetic() || c.is_ascii_digit() || "_.:@/-".contains(c)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_language_test_names_and_rejects_hostile_names() {
        assert_eq!(test_names("x.rs", "#[test]\nfn good_case() {}\n#[tokio::test]\nfn bad;$(id)() {}"), ["good_case"]);
        assert_eq!(test_names("x.py", "def test_ok():\ndef test_bad;():"), ["test_ok"]);
        assert_eq!(test_names("x.ts", "it('plain_name', () => {});\ntest('bad;$(id)', () => {});"), ["plain_name"]);
    }

    #[test]
    fn runner_commands_validate_names() {
        assert_eq!(fn_test_command("cargo", "test_name").as_deref(), Some("cargo test test_name"));
        assert_eq!(fn_test_command("python", "test_name").as_deref(), Some("pytest -k test_name"));
        assert_eq!(fn_test_command("npm", "test_name").as_deref(), Some("npm test -- -t test_name"));
        assert_eq!(fn_test_command("npm", "x;id").as_deref(), None);
    }
}
