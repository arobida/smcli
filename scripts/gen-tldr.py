#!/usr/bin/env python3
"""Converts a pinned tldr-pages checkout into Data/tldr/tldr.jsonl.

Usage: python3 scripts/gen-tldr.py TLDR=<checkout>
Requires a local checkout of https://github.com/tldr-pages/tldr (pass TLDR=path);
it never downloads anything. The checkout must be clean so the pinned commit is honest.

Writes Data/tldr/tldr.jsonl:
  - header line: {"version": 1, "tldr_commit": "<sha>", "license": "CC-BY-4.0"}
  - one line per example: {"program", "platform", "description", "template"}
Templates keep their raw {{...}} placeholders. Only pages/common, pages/osx and
pages/linux are read. Fails if the output exceeds 10 MB.
"""
import json
import os
import re
import subprocess
import sys

MAX_BYTES = 10 * 1024 * 1024
PLATFORMS = ("common", "osx", "linux")
EXAMPLE_RE = re.compile(r"^- (.+):\n\n`(.+)`\n", re.MULTILINE)

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
out_dir = os.path.join(root, "Data", "tldr")

tldr = next((arg.split("=", 1)[1] for arg in sys.argv[1:] if arg.startswith("TLDR=")), None)
if not tldr:
    sys.exit("usage: python3 scripts/gen-tldr.py TLDR=<path to tldr-pages checkout>")

commit = subprocess.check_output(["git", "-C", tldr, "rev-parse", "HEAD"], text=True).strip()
status = subprocess.run(["git", "-C", tldr, "status", "--porcelain"], capture_output=True, text=True).stdout.strip()
if status:
    sys.exit(f"{tldr} is not clean; the pinned commit {commit} would not be reproducible")

entries = []
for platform in PLATFORMS:
    pages = os.path.join(tldr, "pages", platform)
    for name in sorted(os.listdir(pages)):
        if not name.endswith(".md"):
            continue
        program = name[:-3]
        text = open(os.path.join(pages, name), errors="replace").read()
        for description, template in EXAMPLE_RE.findall(text):
            entries.append({
                "program": program,
                "platform": platform,
                "description": description.strip(),
                "template": template,
            })

os.makedirs(out_dir, exist_ok=True)
out_path = os.path.join(out_dir, "tldr.jsonl")
with open(out_path, "w") as handle:
    handle.write(json.dumps({"version": 1, "tldr_commit": commit, "license": "CC-BY-4.0"}) + "\n")
    for entry in entries:
        handle.write(json.dumps(entry) + "\n")

size = os.path.getsize(out_path)
if size > MAX_BYTES:
    os.remove(out_path)
    sys.exit(f"{out_path} is {size} bytes, over the 10 MB budget; not kept")
print(f"wrote {out_path}: {len(entries)} entries from tldr@{commit[:12]}, {size / 1e6:.2f} MB")
