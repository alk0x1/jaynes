"""What of Jaynes is off in this session: a Claude Code SessionStart hook shipped with the Jaynes plugin.

The plugin has two parts. The conducting part, the skill and the MCP tools, is off in a session started with
JAYNES_OFF set (to anything but nothing, 0, false, no or off, as ``jaynes mcp`` reads it). The guard keeps its own
switch, "on" in $JAYNES_HOME/guard.json (default ~/.jaynes/guard.json), and JAYNES_OFF never turns it off; with
"watch" beside it, the guard only watches what can be taken back and notes it (see guard.py).

A project may turn the conducting part off with its own switch, .jaynes.toml (see switch.py), and make the guard
stricter, never turn it off; the person may turn the guard off in a project in their settings ("off_in").

When something is off, the person reads which in the session, and the model is told not to use what is off. When
all is on it says nothing, so a session spends nothing on it. It only reads: no network, no file written, standard
library only, since it runs before jaynes may be installed.
"""

from __future__ import annotations

import json
import os
import sys

import switch


def conducting_off() -> bool:
    return os.environ.get("JAYNES_OFF", "").strip().lower() not in ("", "0", "false", "no", "off")


def home() -> str:
    return os.path.expanduser(os.environ.get("JAYNES_HOME") or os.path.join(os.path.expanduser("~"), ".jaynes"))


def settings() -> dict[str, object]:
    """The guard's settings, as the guard reads them; {} when missing or broken."""
    try:
        with open(os.path.join(home(), "guard.json"), encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def guard_on(data: dict[str, object]) -> bool:
    on = data.get("on", True)
    return on if isinstance(on, bool) else True


ACTIONS = "git and GitHub actions that cannot be undone"


def said(cwd: str) -> dict[str, object] | None:
    data, project = settings(), switch.read(cwd)
    if conducting_off():
        conduct: str | None = "Jaynes is off in this session (JAYNES_OFF)"
    elif not project["conduct"]:
        conduct = f"Jaynes is off in this project ({project['path']} says conduct = false)"
    else:
        conduct = None
    if not guard_on(data):
        guard = f"its guard is off (the Setup page of Jaynes turns it on): it does not ask before {ACTIONS}"
    elif switch.off_in(cwd, data):
        guard = f"its guard is off in this project (your settings, on the Setup page of Jaynes): it does not ask before {ACTIONS}"
    elif data.get("watch") is True:
        guard = (f"its guard only watches: a push or a pull request opened or merged it does not ask, and notes in "
                 f"{os.path.join(home(), 'guard-watch.jsonl')} what it would have asked; what cannot be undone it "
                 "still asks, and so does a change to its own files (the Setup page of Jaynes turns asking back on)")
    elif project["guard"] == "strict":
        guard = f"its guard is strict in this project ({project['path']}): it asks before every one of the {ACTIONS}, even on your own repositories"
    else:
        guard = f"its guard is on: it still asks before {ACTIONS}"
    refused = (f" {project['path']} says guard = false, which a project's file cannot do, since an agent or a cloned "
               "repository can write it: the guard stays on here. To turn it off in this project, use the Setup page "
               "of Jaynes." if project["asked_guard_off"] and guard_on(data) and not switch.off_in(cwd, data) else "")
    plain = guard.startswith("its guard is on")
    if conduct is None and plain and not refused:
        return None
    if conduct is None:
        return {"systemMessage": f"Jaynes is on here, and {guard}.{refused}"}
    model = (f"{conduct}: do not use the jaynes skill or the Jaynes MCP tools, and make agreed changes yourself.")
    return {"systemMessage": f"{conduct}: no skill, no tools; {guard}.{refused}",
            "hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": model}}


def main() -> int:
    try:
        event = json.loads(sys.stdin.read() or "null")
        cwd = event.get("cwd") if isinstance(event, dict) else None
        out = said(cwd if isinstance(cwd, str) and cwd else os.getcwd())
        if out:
            print(json.dumps(out))
    except Exception as err:  # never in the way of a session
        print(f"jaynes session: {err!r}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
