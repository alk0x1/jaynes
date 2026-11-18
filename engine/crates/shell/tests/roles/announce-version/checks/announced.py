#!/usr/bin/env python3
"""Judges a reading of the registry: it lists the version the project is at, under the id its announcement got.

A check reads {"output", "given", "project"} as JSON on standard input and exits 0 to pass, 1 to fail with why, 3
when it cannot judge. Its output is the reading (read-registry's): "listed", each an id and a version. It is given
survey's output (the version) and announce's (the id). It asks nothing of the world, so its cases stand for any
reading.
"""
import json
import sys

try:
    call = json.load(sys.stdin)
    given = {g["step"]: g["output"] for g in call.get("given") or []}
    version, announced = given["survey"]["version"], given["announce"]["id"]
    listed = (call.get("output") or {}).get("listed")
except (KeyError, TypeError, ValueError) as e:
    print(f"the check was not given what it judges: {e}")
    sys.exit(3)

if not isinstance(listed, list):
    print("the reading has no list of what the registry listed")
    sys.exit(1)
if not any(isinstance(a, dict) and a.get("version") == version and a.get("id") == announced for a in listed):
    print(f"the registry does not list {version} under the id the announcement got, {announced}")
    sys.exit(1)
