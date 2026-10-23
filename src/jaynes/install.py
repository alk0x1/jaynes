"""Install the action gate in Claude Code: ``jaynes hook`` on every event it needs, other hooks kept.

The previous ``settings.json`` is copied to ``settings.json.jaynes-backup`` before each write. The policy
is written once, to ``<home>/jaynes.toml``, and never overwritten, so edits survive a reinstall.
"""

from __future__ import annotations

import json
import shlex
import shutil
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

EVENTS = ("PreToolUse", "PostToolUse", "PostToolUseFailure", "Stop", "SessionStart")
TOOL_EVENTS = ("PreToolUse", "PostToolUse", "PostToolUseFailure")
BUILT_IN_POLICY = Path(__file__).with_name("agent-team.jaynes.toml")


@dataclass(frozen=True)
class Gate:
    settings: Path
    policy: Path
    hooked: tuple[str, ...]  # events whose hooks include jaynes hook

    @property
    def installed(self) -> bool:
        return set(self.hooked) == set(EVENTS)


def settings_path() -> Path:
    return Path.home() / ".claude" / "settings.json"


def temporary() -> bool:
    """Running from a cache that gets cleaned, as with uvx, so a hook pointing here would stop working."""
    return "/.cache/" in sys.prefix or "/archive-v" in sys.prefix


def is_jaynes(command: str) -> bool:
    words = command.split()
    return len(words) >= 2 and words[-1] == "hook" and "jaynes" in command


def hook_command(home: Path) -> str:
    """This ``jaynes`` by absolute path, so the hook does not depend on Claude Code's PATH."""
    here = Path(sys.executable).with_name("jaynes")
    base = shlex.quote(str(here)) if here.is_file() else f"{shlex.quote(sys.executable)} -m jaynes.cli"
    default = home.resolve() == (Path.home() / ".jaynes").resolve()  # Claude Code does not see this JAYNES_HOME
    return f"{'' if default else f'JAYNES_HOME={shlex.quote(str(home))} '}{base} hook"


def _read(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text()) if path.is_file() and path.read_text().strip() else {}


def _write(path: Path, data: dict[str, Any]) -> None:
    if data == _read(path):
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.is_file():
        shutil.copyfile(path, path.with_name(path.name + ".jaynes-backup"))
    path.write_text(json.dumps(data, indent=2) + "\n")


def status(home: Path) -> Gate:
    hooks: dict[str, Any] = _read(settings_path()).get("hooks", {})
    hooked = tuple(e for e in EVENTS if any(is_jaynes(str(h.get("command", "")))
                                            for g in hooks.get(e, []) for h in g.get("hooks", [])))
    return Gate(settings_path(), home / "jaynes.toml", hooked)


def install(home: Path) -> Gate:
    policy = home / "jaynes.toml"
    if not policy.is_file():
        policy.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(BUILT_IN_POLICY, policy)
    data = _read(settings_path())
    hooks: dict[str, Any] = data.setdefault("hooks", {})
    command = {"type": "command", "command": hook_command(home)}
    for event in set(EVENTS) - set(status(home).hooked):
        group: dict[str, Any] = {"matcher": "*", "hooks": [command]} if event in TOOL_EVENTS else {"hooks": [command]}
        hooks.setdefault(event, []).append(group)
    _write(settings_path(), data)
    return status(home)


def remove(home: Path) -> Gate:
    data = _read(settings_path())
    hooks: dict[str, Any] = data.get("hooks", {})
    for event in list(hooks):
        groups = [{**g, "hooks": [h for h in g.get("hooks", []) if not is_jaynes(str(h.get("command", "")))]}
                  for g in hooks[event]]
        hooks[event] = [g for g in groups if g["hooks"]]
        if not hooks[event]:
            del hooks[event]
    _write(settings_path(), data)
    return status(home)
