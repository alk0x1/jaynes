#!/usr/bin/env python3
"""Announces the version to the project's registry. This cannot be undone.

The registry is given the run's key, so the same announcement sent twice counts once. The script exits 1 only when
it is sure nothing was sent; any other failure may have acted, and Jaynes says so to the person.
"""
import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path

call = json.load(sys.stdin)
given = {g["step"]: g["output"] for g in call["given"]}
registry, version = given["survey"]["registry"], given["survey"]["version"]

body = json.dumps({"version": version, "notes": given["stamp"]["entry"]}).encode()
ask = urllib.request.Request(f"{registry}/announcements", data=body, method="POST",
                             headers={"Content-Type": "application/json", "Idempotency-Key": call["key"]})
try:
    with urllib.request.urlopen(ask, timeout=20) as answer:
        said = json.load(answer)
except urllib.error.URLError as e:
    if isinstance(e.reason, ConnectionRefusedError):
        print(f"the registry at {registry} does not answer, so nothing was announced")
        sys.exit(1)
    print(f"the registry at {registry} was asked and its answer did not arrive: {e}")
    sys.exit(2)

Path(os.environ["JAYNES_OUTPUT"]).write_text(json.dumps({"id": str(said["id"])}))
