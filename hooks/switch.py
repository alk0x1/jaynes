"""A project's own switch for Jaynes, read by the plugin's hooks: ``.jaynes.toml`` at the root of the project's git
repository, found by going up from the folder a session or a command is in until a folder holds ``.git``. A file
above that root is not the project's, and a folder in no repository has no switch.

    conduct = false     # the skill and the MCP tools stand aside in this project
    guard = "strict"    # the guard asks before every action it holds here, even on the person's own repositories

A project's file may turn the conducting part off and make the guard stricter, never turn the guard off: a file
in the project is what an agent can write and what a cloned repository brings. ``guard = false`` is read only to
say that it does nothing. Turning the guard off in a project is the person's, in their settings (guard.json
"off_in"), which only the Setup page of Jaynes writes.

Only the top-level ``key = value`` lines are read (true, false, or a quoted string), since python3 before 3.11 has
no TOML reader and the hooks use the standard library alone. It only reads: no network, no file written.
"""

from __future__ import annotations

import os
import re

NAME = ".jaynes.toml"
LINE = re.compile(r"""\s*([A-Za-z_][A-Za-z0-9_-]*)\s*=\s*(true|false|"[^"]*"|'[^']*')\s*(?:#.*)?""")


def root_of(cwd: str) -> str | None:
    """The root of the git repository holding cwd, or None."""
    here = os.path.realpath(cwd)
    while True:
        if os.path.exists(os.path.join(here, ".git")):
            return here
        up = os.path.dirname(here)
        if up == here:
            return None
        here = up


def is_switch(path: str) -> bool:
    """Whether path is a project's switch, wherever it is: the guard asks before any change of one."""
    return os.path.basename(path) == NAME


def read(cwd: str) -> dict[str, object]:
    """The project's switch for cwd: its path (None when there is none), whether Jaynes conducts there, how the
    guard is ("on" or "strict"), and whether the file asked for the guard off, which a project's file cannot do."""
    root = root_of(cwd)
    out: dict[str, object] = {"path": None, "conduct": True, "guard": "on", "asked_guard_off": False}
    if root is None:
        return out
    path = os.path.join(root, NAME)
    try:
        with open(path, encoding="utf-8") as f:
            text = f.read()
    except (OSError, ValueError):
        return out
    out["path"] = path
    for line in text.splitlines():
        if line.strip().startswith("["):
            break  # a table: its keys are not the switch's
        m = LINE.fullmatch(line)
        if not m:
            continue
        key, raw = m.group(1), m.group(2)
        value: object = raw == "true" if raw in ("true", "false") else raw[1:-1]
        if key == "conduct" and value is False:
            out["conduct"] = False
        elif key == "guard" and value == "strict":
            out["guard"] = "strict"
        elif key == "guard" and value in (False, "off", "false"):
            out["asked_guard_off"] = True
    return out


def off_in(cwd: str, settings: dict[str, object]) -> bool:
    """Whether the person turned the guard off for the folder cwd is in, in their settings' "off_in"."""
    folders = settings.get("off_in")
    if not isinstance(folders, list) or not folders:
        return False
    here = os.path.realpath(cwd)
    for folder in folders:
        if isinstance(folder, str) and folder.strip():
            f = os.path.realpath(os.path.expanduser(folder))
            if here == f or here.startswith(f.rstrip(os.sep) + os.sep):
                return True
    return False
