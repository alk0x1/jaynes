#!/usr/bin/env python3
"""Writes the version's entry in the changelog, in the run's copy of the project: Jaynes measures the change and
delivers it. The entry says what the person asked for."""
import json
import os
import sys
from pathlib import Path

call = json.load(sys.stdin)
given = {g["step"]: g["output"] for g in call["given"]}
version = given["survey"]["version"]

changelog = Path("CHANGELOG.md")
before = changelog.read_text() if changelog.is_file() else "# Changelog\n"
entry = f"## {version}\n\n{call['request'].strip()}\n"
if f"## {version}\n" not in before:
    changelog.write_text(f"{before}\n{entry}")

output = {"entry": entry.strip(), "files": ["CHANGELOG.md"]}
Path(os.environ["JAYNES_OUTPUT"]).write_text(json.dumps(output))
