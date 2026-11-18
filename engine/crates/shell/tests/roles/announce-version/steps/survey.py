#!/usr/bin/env python3
"""Reads the project: the version it is at and the registry it announces to. Changes nothing.

A step's script reads its call as JSON on standard input, writes its output as JSON to the file named in
JAYNES_OUTPUT, and exits 0 when done, 1 when the step cannot be done (with why, printed), 3 when it could not run.
"""
import json
import os
import sys
from pathlib import Path

json.load(sys.stdin)  # the request and what earlier steps made: this step needs neither

missing = [name for name in ("VERSION", "registry") if not Path(name).is_file()]
if missing:
    print(f"the project has no {' and no '.join(missing)}: the version to announce and the registry's address, a line each")
    sys.exit(1)

output = {"version": Path("VERSION").read_text().strip(), "registry": Path("registry").read_text().strip()}
Path(os.environ["JAYNES_OUTPUT"]).write_text(json.dumps(output))
