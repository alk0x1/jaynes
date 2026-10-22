"""Role definitions: ordered steps with schemas, checks, executors and stops.

A role is plain Python. Each step declares the type of its output (a Pydantic
model), the earlier steps it needs, the checks its output must pass, who
produces it, and whether a person reviews it. Steps can only need steps defined
before them, so the definition order is a topological order of the graph.

Fingerprints hash everything that shapes a step's behaviour (method text,
instructions, schema, check code, stop parameters), so a change opens a new
window of labels.
"""

from __future__ import annotations

import hashlib
import inspect
import json
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Generic, TypeVar, cast

from pydantic import BaseModel

T = TypeVar("T", bound=BaseModel)

OUTPUT = "output.json"


class Pass:
    def __repr__(self) -> str:
        return "Pass()"


class Fail:
    def __init__(self, *reasons: str) -> None:
        if not reasons:
            raise ValueError("Fail needs at least one reason")
        self.reasons = tuple(reasons)

    def __repr__(self) -> str:
        return f"Fail{self.reasons!r}"


@dataclass(frozen=True)
class Checked(Generic[T]):
    """A step output that passed its checks. Only the conductor creates these."""

    step: str
    value: T
    output_hash: str
    check_hash: str


class Witnesses:
    """The checked outputs a step needs, read by step."""

    def __init__(self, by_name: Mapping[str, Checked[Any]]) -> None:
        self._by_name = dict(by_name)

    def __getitem__(self, step: Step[T]) -> Checked[T]:
        return cast(Checked[T], self._by_name[step.name])

    def names(self) -> list[str]:
        return sorted(self._by_name)

    def all(self) -> list[Checked[Any]]:
        return [self._by_name[n] for n in self.names()]


Check = Callable[[Any, Witnesses], Pass | Fail]


@dataclass(frozen=True)
class StepContext:
    """What a trusted executor receives."""

    run_id: str
    task: str
    needs: Witnesses
    workspace: Path


@dataclass(frozen=True)
class Stop:
    budget: float
    sampled: bool = True
    audit_rate: float = 0.1
    delta: float = 0.05


def stop(budget: float, sampled: bool = True, audit_rate: float = 0.1, delta: float = 0.05) -> Stop:
    if not 0 < budget < 1 or not 0 < delta < 1 or not 0 <= audit_rate <= 1:
        raise ValueError("budget and delta must be in (0, 1), audit_rate in [0, 1]")
    return Stop(budget, sampled, audit_rate, delta)


@dataclass(eq=False)
class Step(Generic[T]):
    role: Role
    name: str
    output: type[T] | None
    needs: tuple[Step[Any], ...]
    checks: tuple[Check, ...]
    stop: Stop | None
    run_by: Callable[[StepContext], T] | None  # a trusted executor; None means an agent
    agent: str
    instructions: str
    max_turns: int
    deadline: float  # seconds per turn
    question: Callable[[Any, Witnesses], str] | None = None  # what the person is asked at the stop

    @property
    def executor(self) -> str:
        if self.output is None:
            return "none"
        return "jaynes" if self.run_by is not None else self.agent

    def schema(self) -> dict[str, Any]:
        return {} if self.output is None else self.output.model_json_schema()

    def check_hash(self) -> str:
        return _sha("\0".join(code_hash(c) for c in self.checks))

    def prompt_hash(self) -> str:
        """Everything the agent is shown that belongs to the role rather than the task."""
        parts = [self.role.method_text(), self.instructions, json.dumps(self.schema(), sort_keys=True)]
        if self.run_by is not None:
            parts.append(code_hash(self.run_by))
        return _sha("\0".join(parts))

    def stop_fingerprint(self, model: str) -> str:
        assert self.stop is not None
        s = self.stop
        question = [code_hash(self.question)] if self.question else []  # absent, so older windows are kept
        return _sha("\0".join([self.role.name, self.name, self.prompt_hash(), self.check_hash(), *question,
                               repr(s.budget), repr(s.delta), repr(s.sampled), model]))[:16]


@dataclass(eq=False)
class Role:
    name: str
    agent: str = "claude"
    method: str | Path = ""
    steps: list[Step[Any]] = field(default_factory=list[Step[Any]])

    def method_text(self) -> str:
        return self.method.read_text() if isinstance(self.method, Path) else self.method

    def step(self, name: str, output: type[T] | None = None, *, needs: Sequence[Step[Any]] = (),
             check: Check | Sequence[Check] = (), stop: Stop | None = None,
             run_by: Callable[[StepContext], T] | None = None, agent: str | None = None,
             instructions: str = "", max_turns: int = 3, deadline: float = 900.0,
             question: Callable[[T, Witnesses], str] | None = None) -> Step[T]:
        if any(s.name == name for s in self.steps):
            raise ValueError(f"step {name!r} defined twice")
        for n in needs:
            if n.role is not self or n not in self.steps:
                raise ValueError(f"step {name!r} needs {n.name!r}, which is not an earlier step of {self.name!r}")
        if output is None and run_by is not None:
            raise ValueError(f"step {name!r} has an executor but no output type")
        if output is None and stop is None:
            raise ValueError(f"step {name!r} has no output and no stop; it would do nothing")
        if question is not None and stop is None:
            raise ValueError(f"step {name!r} has a question but no stop to ask it at")
        checks = tuple(cast(Sequence[Check], check)) if isinstance(check, Sequence) else (check,)
        s: Step[T] = Step(self, name, output, tuple(needs), checks, stop, run_by, agent or self.agent,
                          instructions, max_turns, deadline, cast(Callable[[Any, Witnesses], str] | None, question))
        self.steps.append(s)
        return s

    def fingerprint(self) -> str:
        parts = [self.name] + [
            f"{s.name}:{s.executor}:{s.prompt_hash()}:{s.check_hash()}:{s.stop}:{[n.name for n in s.needs]}"
            for s in self.steps
        ]
        return _sha("\0".join(parts))[:16]


def role(name: str, agent: str = "claude", method: str | Path = "") -> Role:
    """``method`` is the method text, or a ``Path`` to it, relative paths resolved next to the calling module."""
    if isinstance(method, Path) and not method.is_absolute():
        method = Path(inspect.stack()[1].filename).resolve().parent / method
    return Role(name, agent, method)


def code_hash(fn: Callable[..., Any]) -> str:
    try:
        src = inspect.getsource(fn)
    except (OSError, TypeError):
        src = getattr(fn, "__qualname__", repr(fn))
    return _sha(src)


def _sha(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()
