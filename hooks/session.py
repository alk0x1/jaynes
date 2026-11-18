"""What of Jaynes is off in this session: a Claude Code SessionStart hook shipped with the Jaynes plugin.

The plugin has two parts. The conducting part, the skill and the MCP tools, is off in a session started with
JAYNES_OFF set (to anything but nothing, 0, false, no or off, as ``jaynes mcp`` reads it). The guard keeps its own
switch, "on" in $JAYNES_HOME/guard.json (default ~/.jaynes/guard.json), and JAYNES_OFF never turns it off.

When something is off, the person reads which in the session, and the model is told not to use what is off. When
all is on it says nothing, so a session spends nothing on it. It only reads: no network, no file written, standard
library only, since it runs before jaynes may be installed.
"""

from __future__ import annotations

import json
import os
import sys


def conducting_off() -> bool:
    return os.environ.get("JAYNES_OFF", "").strip().lower() not in ("", "0", "false", "no", "off")


def guard_on() -> bool:
    """The guard's own switch, read as the guard reads it: on unless its settings say "on": false."""
    home = os.environ.get("JAYNES_HOME") or os.path.join(os.path.expanduser("~"), ".jaynes")
    try:
        with open(os.path.join(os.path.expanduser(home), "guard.json"), encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, ValueError):
        return True
    on = data.get("on", True) if isinstance(data, dict) else True
    return on if isinstance(on, bool) else True


GUARD_ON = "its guard is on: it still asks before git and GitHub actions that cannot be undone"
GUARD_OFF = "its guard is off (the Setup page of Jaynes turns it on): it does not ask before git and GitHub actions that cannot be undone"


def said() -> dict[str, object] | None:
    off, guard = conducting_off(), guard_on()
    if not off and guard:
        return None
    if off:
        person = f"Jaynes is off in this session (JAYNES_OFF): no skill, no tools; {GUARD_ON if guard else GUARD_OFF}."
        model = ("Jaynes is off in this session (JAYNES_OFF is set): do not use the jaynes skill or the Jaynes MCP "
                 "tools, and make agreed changes yourself.")
        return {"systemMessage": person,
                "hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": model}}
    return {"systemMessage": f"Jaynes is on in this session, and {GUARD_OFF}."}


def main() -> int:
    try:
        sys.stdin.read()
        out = said()
        if out:
            print(json.dumps(out))
    except Exception as err:  # never in the way of a session
        print(f"jaynes session: {err!r}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
