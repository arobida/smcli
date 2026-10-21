#!/usr/bin/env python3
"""Regenerate fixtures with the actual Swift Codable declarations, without building the app.
Run from any directory: python3 crates/smcli-core/tests/fixtures/service_protocol/generate.py
Only DTO declarations/initializers are retained; runtime methods are not exercised.
"""
from pathlib import Path
import subprocess
import sys

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
CORE = ROOT / "Sources/SmcliCore"


def declaration(filename, name, stop=None):
    text = (CORE / filename).read_text()
    start = text.index(f"package struct {name}:")
    if stop:
        return text[start:text.index(stop, start)] + "}\n"
    opening = text.index("{", start)
    depth = 1
    end = opening + 1
    while depth:
        depth += (text[end] == "{") - (text[end] == "}")
        end += 1
    return text[start:end] + "\n"


source = "import Foundation\n#if canImport(Darwin)\nimport Darwin\n#else\nimport Glibc\n#endif\n"
source += "enum ServiceProtocol { static let version = 1 }\n"
for name in ["ServiceRequest", "FileHit", "FileIndexInfo", "ServiceResponse"]:
    source += declaration("ServiceProtocol.swift", name)
source += declaration("SearchEntry.swift", "SearchEntry")
source += declaration("SearchEntry.swift", "SearchFilter", "    package func allows")
source += declaration("CapturedHistory.swift", "Suggestion")
source += declaration("DirectoryContext.swift", "ProjectContext", "    /// Languages first")
source += declaration("DirectoryContext.swift", "GitStatus", "    package var clean")
source = source.replace("package ", "")
source += (HERE / "generate.swift").read_text()
subprocess.run(["swift", "-", str(HERE), *sys.argv[1:]], input=source, text=True, check=True)
