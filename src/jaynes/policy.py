"""Categories, effects and the policy file (``jaynes.toml``).

A tool call falls into exactly one category. Resolution order:

1. Floor categories: for Bash, a regex or glob matching any simple command
   (heredoc bodies are skipped unless fed to a shell); for other tools, tool
   name and path. The first floor that matches wins.
2. For Bash, every simple command must match some non-floor category; the call
   takes the matching category with the strongest effect (ties: file order).
   One unmatched command makes the whole call ``unknown``.
3. For other tools, the first category whose tool and path patterns match.
4. Otherwise ``unknown``.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import tomllib
from dataclasses import dataclass, field
from enum import IntEnum
from fnmatch import fnmatchcase
from pathlib import Path
from typing import Any, Literal, cast

from jaynes.shell import split_commands


class Effect(IntEnum):
    PURE = 0
    REVERSIBLE = 1
    IRREVERSIBLE = 2

    @classmethod
    def parse(cls, value: str) -> "Effect":
        return cls[value.upper()]

    def join(self, other: "Effect") -> "Effect":
        return max(self, other)


Floor = Literal["deny", "ask"]
Undetermined = Literal["defer", "ask"]


@dataclass(frozen=True)
class Category:
    name: str
    effect: Effect
    floor: Floor | None = None
    tools: tuple[str, ...] = ()
    paths: tuple[str, ...] = ()
    bash: tuple[str, ...] = ()
    bash_regex: tuple[str, ...] = ()
    _regex: tuple[re.Pattern[str], ...] = field(default=(), compare=False, repr=False)
    _paths: tuple[re.Pattern[str], ...] = field(default=(), compare=False, repr=False)

    def definition(self) -> dict[str, Any]:
        return {
            "name": self.name, "effect": self.effect.name.lower(), "floor": self.floor,
            "tools": list(self.tools), "paths": list(self.paths),
            "bash": list(self.bash), "bash_regex": list(self.bash_regex),
        }

    def matches_tool(self, tool: str) -> bool:
        return any(fnmatchcase(tool, t) for t in self.tools)

    def matches_path(self, path: str | None) -> bool:
        if not self.paths:
            return True
        return path is not None and any(p.match(path) for p in self._paths)

    def matches_simple_command(self, cmd: str) -> bool:
        return any(fnmatchcase(cmd, g) for g in self.bash) or any(r.search(cmd) for r in self._regex)


UNKNOWN = Category(name="unknown", effect=Effect.IRREVERSIBLE)


@dataclass(frozen=True)
class Policy:
    budget: float = 0.05
    delta: float = 0.05
    audit_rate: float = 0.10
    undetermined: Undetermined = "defer"
    seed: str = "jaynes"
    categories: tuple[Category, ...] = ()
    source: str = "<memory>"

    def category(self, name: str) -> Category:
        for c in self.categories:
            if c.name == name:
                return c
        return UNKNOWN

    def fingerprint(self, category: Category, extra: str = "") -> str:
        """Labels count only inside the window of the current fingerprint."""
        blob = json.dumps(
            {"category": category.definition(), "budget": self.budget, "delta": self.delta, "extra": extra},
            sort_keys=True,
        )
        return hashlib.sha256(blob.encode()).hexdigest()[:16]


def glob_to_regex(pattern: str) -> re.Pattern[str]:
    """Path glob with ``**`` (any depth), ``*`` (within a segment) and ``?``."""
    out: list[str] = []
    i = 0
    while i < len(pattern):
        if pattern.startswith("**/", i):
            out.append("(?:.*/)?")
            i += 3
        elif pattern.startswith("**", i):
            out.append(".*")
            i += 2
        elif pattern[i] == "*":
            out.append("[^/]*")
            i += 1
        elif pattern[i] == "?":
            out.append("[^/]")
            i += 1
        else:
            out.append(re.escape(pattern[i]))
            i += 1
    return re.compile("^" + "".join(out) + "$")


def _tuple(raw: dict[str, Any], key: str) -> tuple[str, ...]:
    value = raw.get(key, [])
    if isinstance(value, str):
        return (value,)
    if not isinstance(value, list):
        raise ValueError(f"{key} must be a list of strings")
    items: list[str] = []
    for v in cast(list[Any], value):
        if not isinstance(v, str):
            raise ValueError(f"{key} must be a list of strings")
        items.append(v)
    return tuple(items)


def make_category(raw: dict[str, Any]) -> Category:
    floor = raw.get("floor")
    if floor not in (None, "deny", "ask"):
        raise ValueError(f"category {raw.get('name')}: floor must be 'deny' or 'ask'")
    bash_regex = _tuple(raw, "bash_regex")
    paths = _tuple(raw, "paths")
    return Category(
        name=str(raw["name"]),
        effect=Effect.parse(str(raw.get("effect", "irreversible"))),
        floor=floor,
        tools=_tuple(raw, "tools"),
        paths=paths,
        bash=_tuple(raw, "bash"),
        bash_regex=bash_regex,
        _regex=tuple(re.compile(r) for r in bash_regex),
        _paths=tuple(glob_to_regex(p) for p in paths),
    )


def load_policy(path: str | os.PathLike[str]) -> Policy:
    with open(path, "rb") as fh:
        raw = tomllib.load(fh)
    pol = raw.get("policy", {})
    cats = tuple(make_category(c) for c in raw.get("category", []))
    names = [c.name for c in cats]
    if len(set(names)) != len(names) or "unknown" in names:
        raise ValueError("category names must be unique and must not be 'unknown'")
    undetermined = pol.get("undetermined", "defer")
    if undetermined not in ("defer", "ask"):
        raise ValueError("policy.undetermined must be 'defer' or 'ask'")
    policy = Policy(
        budget=float(pol.get("budget", 0.05)),
        delta=float(pol.get("delta", 0.05)),
        audit_rate=float(pol.get("audit_rate", 0.10)),
        undetermined=undetermined,
        seed=str(pol.get("seed", "jaynes")),
        categories=cats,
        source=str(path),
    )
    for name, v in (("budget", policy.budget), ("delta", policy.delta)):
        if not 0 < v < 1:
            raise ValueError(f"policy.{name} must be in (0, 1)")
    if not 0 <= policy.audit_rate <= 1:
        raise ValueError("policy.audit_rate must be in [0, 1]")
    return policy


def find_policy(cwd: str) -> Path | None:
    """``jaynes.toml`` in cwd or any parent, else ``$JAYNES_HOME/jaynes.toml``."""
    here = Path(cwd).resolve()
    for d in (here, *here.parents):
        candidate = d / "jaynes.toml"
        if candidate.is_file():
            return candidate
    fallback = jaynes_home() / "jaynes.toml"
    return fallback if fallback.is_file() else None


def jaynes_home() -> Path:
    return Path(os.environ.get("JAYNES_HOME", Path.home() / ".jaynes"))


# --- classification -----------------------------------------------------------

def call_path(tool_input: dict[str, Any], cwd: str) -> str | None:
    raw = tool_input.get("file_path") or tool_input.get("notebook_path") or tool_input.get("path")
    if not isinstance(raw, str) or not raw:
        return None
    p = Path(raw) if os.path.isabs(raw) else Path(cwd) / raw
    try:
        return p.resolve().relative_to(Path(cwd).resolve()).as_posix()
    except ValueError:
        return p.resolve().as_posix()


def classify(policy: Policy, tool: str, tool_input: dict[str, Any], cwd: str) -> Category:
    if tool == "Bash":
        command = tool_input.get("command")
        return classify_bash(policy, command if isinstance(command, str) else "")
    path = call_path(tool_input, cwd)
    for c in policy.categories:
        if c.floor and c.matches_tool(tool) and c.matches_path(path):
            return c
    for c in policy.categories:
        if not c.floor and c.matches_tool(tool) and c.matches_path(path):
            return c
    return UNKNOWN


def classify_bash(policy: Policy, command: str) -> Category:
    simple = split_commands(command)
    for c in policy.categories:
        if c.floor and any(c.matches_simple_command(s) for s in simple):
            return c
    if not simple:
        return UNKNOWN
    best: Category | None = None
    for s in simple:
        hit = next((c for c in policy.categories if not c.floor and c.matches_simple_command(s)), None)
        if hit is None:
            return UNKNOWN
        if best is None or hit.effect > best.effect:
            best = hit
    return best or UNKNOWN
