#!/usr/bin/env python3
"""Converts a pinned cheat/cheatsheets checkout into Data/cheat/cheat.jsonl.

Usage: python3 scripts/gen-cheat.py CHEAT=<checkout>
Requires a local checkout of https://github.com/cheat/cheatsheets (pass CHEAT=path);
it never downloads anything. The checkout must be clean so the pinned commit is honest.

Reads only top-level regular files (dotfiles and directories are skipped). A file's
optional YAML front matter (--- ... ---) is stripped. Blocks are `# description`
lines followed by command lines, one example per line; the description is the block's
first comment line (further consecutive comment lines before the command are ignored,
and comment lines between commands of the same block do not change it). A command line
ending in a backslash continues on the next line: the lines are joined into one template
with a single space (the backslash-newline is dropped) and count as one example.
Placeholders are literal text, kept as written.

Writes Data/cheat/cheat.jsonl:
  - header line: {"version": 1, "cheat_commit": "<sha>", "license": "CC0-1.0"}
  - one line per example: {"program", "platform": "common", "description", "template"}
Fails if the output exceeds 10 MB. The parsing rules are regression-tested by
scripts/test-gen-cheat.py.
"""
import json
import os
import subprocess
import sys

MAX_BYTES = 10 * 1024 * 1024

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
out_dir = os.path.join(root, "Data", "cheat")


def parse_sheet(text):
    """One sheet's text to (description, template) examples; the parsing rules in design.md."""
    lines = text.splitlines()
    # Strip optional YAML front matter.
    if lines and lines[0].strip() == "---":
        end = next((i for i, line in enumerate(lines[1:], 1) if line.strip() == "---"), None)
        if end is not None:
            lines = lines[end + 1:]
    # Join backslash continuations into single logical lines (backslash-newline -> one space).
    logical = []
    pending = None
    for raw in lines:
        stripped = raw.strip()
        if pending is not None:
            if not stripped:
                continue
            stripped = pending + " " + stripped
            pending = None
        if stripped.endswith("\\"):
            pending = stripped[:-1].rstrip()
            continue
        logical.append(stripped)
    if pending is not None:
        logical.append(pending)
    entries = []
    description = None
    for line in logical:
        if not line:
            description = None  # a blank line ends the block
            continue
        if line.startswith("#"):
            # The first comment line of a block is its description; further consecutive comment
            # lines, and comment lines between commands of the same block, are ignored.
            if description is None:
                description = line.lstrip("#").strip()
        elif description:
            entries.append((description, line))
    return entries


def convert(cheat, out_path):
    """A clean checkout to (pinned commit, entries); writes out_path."""
    commit = subprocess.check_output(["git", "-C", cheat, "rev-parse", "HEAD"], text=True).strip()
    status = subprocess.run(["git", "-C", cheat, "status", "--porcelain"], capture_output=True, text=True).stdout.strip()
    if status:
        sys.exit(f"{cheat} is not clean; the pinned commit {commit} would not be reproducible")

    entries = []
    for name in sorted(os.listdir(cheat)):
        path = os.path.join(cheat, name)
        if name.startswith(".") or not os.path.isfile(path):
            continue
        for description, template in parse_sheet(open(path, errors="replace").read()):
            entries.append({
                "program": name,
                "platform": "common",
                "description": description,
                "template": template,
            })

    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    with open(out_path, "w") as handle:
        handle.write(json.dumps({"version": 1, "cheat_commit": commit, "license": "CC0-1.0"}) + "\n")
        for entry in entries:
            handle.write(json.dumps(entry) + "\n")
    return commit, entries


def main():
    cheat = next((arg.split("=", 1)[1] for arg in sys.argv[1:] if arg.startswith("CHEAT=")), None)
    if not cheat:
        sys.exit("usage: python3 scripts/gen-cheat.py CHEAT=<path to cheat/cheatsheets checkout>")
    out_path = os.path.join(out_dir, "cheat.jsonl")
    commit, entries = convert(cheat, out_path)
    size = os.path.getsize(out_path)
    if size > MAX_BYTES:
        os.remove(out_path)
        sys.exit(f"{out_path} is {size} bytes, over the 10 MB budget; not kept")
    programs = {entry["program"] for entry in entries}
    print(f"wrote {out_path}: {len(entries)} examples for {len(programs)} programs from cheat@{commit[:12]}, {size / 1e6:.2f} MB")


if __name__ == "__main__":
    main()
