"""Roles written in TOML: the steps in a file, their output models and checks in Python (SPEC section 4).

    name = "notes"
    agent = "claude"
    method = "Write short notes."          # or method_file = "method.md", next to this file

    [[step]]
    name = "draft"
    output = "notes_checks:Note"           # a pydantic model, as module:attribute
    check = ["notes_checks:not_empty"]
    stop = { budget = 0.2 }
    question = "Ship {out.text!r}?"         # a template over the step's output
    instructions = "Write the note."

Every key of ``Role.step`` is a step key; steps are named in ``needs``, and checks read them by name:
``needs["draft"]``. Modules are imported from the role file's folder and from the current folder.
"""

from __future__ import annotations

import importlib
import os
import string
import sys
import tomllib
from collections.abc import Callable
from pathlib import Path
from typing import Any

from pydantic import BaseModel

from jaynes.role import Role, Stop, Step, Witnesses, role, stop

ROLE_KEYS = {"name", "agent", "method", "method_file", "step"}
STEP_KEYS = {"name", "output", "needs", "check", "stop", "run_by", "agent", "instructions", "max_turns", "deadline",
             "question", "project", "may_change", "web", "effect", "ends",
             "max_messages"}
STOP_KEYS = {"budget", "sampled", "audit_rate", "delta"}


class RoleFileError(ValueError):
    pass


class Question:
    """A question template: ``{out}`` and its fields, formatted with the step's output."""

    def __init__(self, template: str) -> None:
        self.template = template

    def __call__(self, out: Any, needs: Witnesses) -> str:
        return self.template.format(out=out)

    def __repr__(self) -> str:  # what the stop fingerprint hashes, so an edited template opens a new window
        return f"Question({self.template!r})"


def load(path: Path) -> Role:
    path = Path(path).resolve()
    try:
        data = tomllib.loads(path.read_text())
    except FileNotFoundError:
        raise RoleFileError(f"{path}: no role file there") from None
    except tomllib.TOMLDecodeError as err:
        raise RoleFileError(f"{path}: {err}") from None
    for folder in (str(path.parent), os.getcwd()):
        if folder not in sys.path:
            sys.path.insert(0, folder)
    try:
        return _role(data, path.parent)
    except (ValueError, TypeError) as err:
        raise RoleFileError(f"{path}: {err}") from None


def _role(data: dict[str, Any], folder: Path) -> Role:
    _known("the role", data, ROLE_KEYS)
    if "name" not in data:
        raise ValueError("the role needs a name")
    method: str | Path = folder / data["method_file"] if "method_file" in data else data.get("method", "")
    r = role(data["name"], data.get("agent", "claude"), method)
    for s in data.get("step", []):
        if "name" not in s:
            raise ValueError("every step needs a name")
        _step(r, s)
    return r


def _step(r: Role, s: dict[str, Any]) -> Step[Any]:
    name = s["name"]
    _known(f"step {name}", s, STEP_KEYS)
    by_name = {x.name: x for x in r.steps}
    unknown = [n for n in s.get("needs", []) if n not in by_name]
    if unknown:
        raise ValueError(f"step {name} needs {', '.join(unknown)}, which is not an earlier step of {r.name}")
    output = _model(name, s["output"]) if "output" in s else None
    checks = s.get("check", [])
    return r.step(
        name, output, needs=[by_name[n] for n in s.get("needs", [])],
        check=[_callable(name, c) for c in ([checks] if isinstance(checks, str) else checks)],
        stop=_stop(name, s["stop"]) if "stop" in s else None,
        run_by=_callable(name, s["run_by"]) if "run_by" in s else None,
        agent=s.get("agent"), instructions=s.get("instructions", ""), max_turns=s.get("max_turns", 3),
        deadline=float(s.get("deadline", 900.0)),
        question=_question(name, s["question"], output) if "question" in s else None,
        project=s.get("project"), may_change=s.get("may_change", ()), web=s.get("web", False),
        effect=s.get("effect"), ends=_callable(name, s["ends"]) if "ends" in s else None,
        max_messages=s.get("max_messages"))


def _known(where: str, table: dict[str, Any], keys: set[str]) -> None:
    extra = sorted(set(table) - keys)
    if extra:
        raise ValueError(f"{where}: unknown key {', '.join(extra)}; the keys are {', '.join(sorted(keys))}")


def _stop(step: str, table: Any) -> Stop:
    if not isinstance(table, dict) or "budget" not in table or set(table) - STOP_KEYS:  # pyright: ignore[reportUnknownArgumentType]
        raise ValueError(f"step {step}: stop takes budget, and optionally sampled, audit_rate and delta, as "
                         "{ budget = 0.2 }")
    return stop(**table)  # pyright: ignore[reportUnknownArgumentType]


def _ref(step: str, ref: Any) -> Any:
    module, _, attr = str(ref).partition(":")
    if not module or not attr:
        raise ValueError(f"step {step}: expected module:attribute, got {ref!r}")
    try:
        found = getattr(importlib.import_module(module), attr)
    except (ImportError, AttributeError) as err:
        raise ValueError(f"step {step}: {ref} does not exist ({err})") from None
    return found


def _model(step: str, ref: str) -> type[BaseModel]:
    found = _ref(step, ref)
    if not (isinstance(found, type) and issubclass(found, BaseModel)):
        raise ValueError(f"step {step}: output {ref} is not a pydantic model")
    return found


def _callable(step: str, ref: str) -> Callable[..., Any]:
    found = _ref(step, ref)
    if not callable(found):
        raise ValueError(f"step {step}: {ref} cannot be called")
    return found


def _question(step: str, template: str, output: type[BaseModel] | None) -> Question:
    for _, field, _, _ in string.Formatter().parse(template):
        if field is None:
            continue
        root, *rest = field.replace("[", ".").split(".")
        if root != "out":
            raise ValueError(f"step {step}: the question may name only out and its fields, not {{{field}}}")
        if rest and output is not None and rest[0] not in output.model_fields:
            raise ValueError(f"step {step}: the question names out.{rest[0]}, but {output.__name__} has no field "
                             f"{rest[0]}")
    return Question(template)
