#!/usr/bin/env python3
"""Lists `pub` items in crates/ that nothing uses, or that only tests use.

rustc's dead_code lint cannot see across workspace crates, so this compares identifiers instead.
shortcut: matches by name, so an unused `new` is hidden by any other `new`; use a resolver (rust-analyzer) if that hides real dead code.
"""
import re
import sys
from collections import Counter
from pathlib import Path

ITEM = re.compile(
    r"\bpub(?:\s*\([^)]*\))?\s+(?:(?:async|const|unsafe)\s+)*(?:fn|struct|enum|trait|type|const|static)\s+([A-Za-z_]\w*)"
)
WORD = re.compile(r"[A-Za-z_]\w*")
TEST_ATTRIBUTE = re.compile(r"#\[cfg\(test\)\]")
ATTRIBUTE_PATH = re.compile(r'\b(?:with|deserialize_with|serialize_with|default|skip_serializing_if)\s*=\s*"([\w:]+)"')
SKIPPED = {"main"}


def blank(text):
    """Replaces comments and string or char literals with spaces, keeping newlines, so only code is matched."""
    out, i, n = [], 0, len(text)

    def erase(end):
        nonlocal i
        out.append(re.sub(r"[^\n]", " ", text[i:end]))
        i = end

    while i < n:
        if text.startswith("//", i):
            end = text.find("\n", i)
            erase(n if end < 0 else end)
        elif text.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif text.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            erase(j)
        elif raw := re.compile(r'\b(?:b|c)?r(#*)"').match(text, i):
            end = text.find('"' + raw[1], raw.end())
            erase(n if end < 0 else end + 1 + len(raw[1]))
        elif text[i] == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            erase(min(j + 1, n))
        elif char := re.compile(r"'(?:\\.[^']*|[^\\'])'").match(text, i):
            erase(char.end())
        else:
            out.append(text[i])
            i += 1
    return "".join(out)


def split_tests(code):
    """Returns (production code, test code). Each `#[cfg(test)]` item is blanked in production, so lines still match."""
    production, tests, position = [], [], 0
    for match in TEST_ATTRIBUTE.finditer(code):
        if match.start() < position:
            continue
        production.append(code[position : match.start()])
        i, depth = match.end(), 0
        while i < len(code):
            if code[i] == "{":
                depth += 1
            elif code[i] == "}":
                depth -= 1
                if depth == 0:
                    i += 1
                    break
            elif code[i] == ";" and depth == 0:
                i += 1
                break
            i += 1
        tests.append(code[match.start() : i])
        production.append(re.sub(r"[^\n]", " ", code[match.start() : i]))
        position = i
    production.append(code[position:])
    return "".join(production), "".join(tests)


def scan(files):
    """Maps {path: source, ...} to (unused, test_only) lists of (path, line, name)."""
    production, tests, items = Counter(), Counter(), []
    for path, source in files.items():
        code = blank(source)
        if "/tests/" in f"/{path}":
            tests.update(WORD.findall(code))
            continue
        kept, tested = split_tests(code)
        production.update(WORD.findall(kept))
        # Blanking hides functions that attributes name in strings, such as serde's deserialize_with.
        production.update(name.rsplit("::", 1)[-1] for name in ATTRIBUTE_PATH.findall(source))
        tests.update(WORD.findall(tested))
        for match in ITEM.finditer(kept):
            items.append((path, kept.count("\n", 0, match.start()) + 1, match[1]))
    defined = Counter(name for _, _, name in items)
    unused, test_only = [], []
    for item in sorted(items):
        name = item[2]
        if name in SKIPPED or production[name] > defined[name]:
            continue
        (test_only if tests[name] else unused).append(item)
    return unused, test_only


def self_check():
    unused, test_only = scan(
        {
            "a/src/lib.rs": 'pub fn dead() {}\npub fn only_tested() {}\npub fn live() {}\n'
            '// pub fn commented() {}\nconst S: &str = r#"pub fn in_string() {}"#;\n'
            "#[cfg(test)]\nmod tests { pub fn helper() {} #[test] fn t() { only_tested(); helper(); } }\n",
            "b/src/main.rs": 'fn main() { live(); println!("dead"); }\n',
            "c/src/lib.rs": 'pub fn parse() {}\n#[serde(deserialize_with = "crate::parse")]\nstruct S;\n',
        }
    )
    assert [i[2] for i in unused] == ["dead"], unused
    assert [i[1:] for i in test_only] == [(2, "only_tested")], test_only


def main():
    self_check()
    root = Path(sys.argv[1] if len(sys.argv) > 1 else "crates")
    files = {str(p): p.read_text(errors="replace") for p in sorted(root.rglob("*.rs")) if "target" not in p.parts}
    unused, test_only = scan(files)
    for title, found in (("Unused", unused), ("Used only by tests", test_only)):
        print(f"{title}: {len(found)}")
        for path, line, name in found:
            print(f"  {path}:{line} {name}")
    print(f"Scanned {len(files)} Rust files.")


if __name__ == "__main__":
    main()
