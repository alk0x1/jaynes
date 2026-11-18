#!/usr/bin/env python3
"""Reads what the registry lists, and when: the reading a check then judges. Changes nothing: it only asks."""
import datetime
import json
import os
import sys
import urllib.request
from pathlib import Path

call = json.load(sys.stdin)
registry = {g["step"]: g["output"] for g in call["given"]}["survey"]["registry"]
try:
    with urllib.request.urlopen(f"{registry}/announcements", timeout=20) as answer:
        listed = json.load(answer)
except OSError as e:
    print(f"the registry at {registry} did not answer: {e}")
    sys.exit(3)

read_at = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
Path(os.environ["JAYNES_OUTPUT"]).write_text(json.dumps({"listed": listed, "read_at": read_at}))
