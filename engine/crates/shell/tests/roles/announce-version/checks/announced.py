#!/usr/bin/env python3
"""Holds the announcement to the world: the registry itself lists the version. A check reads its step's output and
what the step was given as JSON on standard input, and exits 0 to pass, 1 to fail with why, 3 when it cannot judge."""
import json
import sys
import urllib.request

call = json.load(sys.stdin)
given = {g["step"]: g["output"] for g in call["given"]}
registry, version = given["survey"]["registry"], given["survey"]["version"]

try:
    with urllib.request.urlopen(f"{registry}/announcements", timeout=20) as answer:
        listed = json.load(answer)
except OSError as e:
    print(f"the registry at {registry} did not answer: {e}")
    sys.exit(3)

if not any(a.get("version") == version and a.get("id") == call["output"].get("id") for a in listed):
    print(f"the registry does not list {version} under the id the step gave")
    sys.exit(1)
