"""A scripted agent for conductor tests: each turn applies a planned action to the workspace."""

from __future__ import annotations

import json
import os
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from jaynes.runtime import QuotaExhausted, RuntimeFailure, TurnResult, TurnTimeout

Action = Callable[[Path], None]


def write(value: Any) -> Action:
    def act(ws: Path) -> None:
        (ws / "output.json").write_text(json.dumps(value))
    return act


def raw(text: str) -> Action:
    def act(ws: Path) -> None:
        (ws / "output.json").write_text(text)
    return act


def stray(value: Any, name: str = "notes.txt") -> Action:
    """Writes a valid output and also a file outside the output path."""
    def act(ws: Path) -> None:
        write(value)(ws)
        (ws / name).write_text("x")
    return act


def tamper(value: Any) -> Action:
    """Writes a valid output and edits a read-only input."""
    def act(ws: Path) -> None:
        write(value)(ws)
        for p in sorted((ws / "inputs").glob("*.json")):
            os.chmod(p, 0o644)
            p.write_text("{}")
            return
        (ws / "inputs" / "new.json").write_text("{}")
    return act


def nothing(ws: Path) -> None:
    return None


def fail(ws: Path) -> None:
    raise RuntimeFailure("agent crashed")


def timeout(ws: Path) -> None:
    raise TurnTimeout()


def quota(until: float) -> Action:
    def act(ws: Path) -> None:
        raise QuotaExhausted(until)
    return act


def wait_for(sentinel: Path, then: Action) -> Action:
    """Blocks until ``sentinel`` exists, so a test can kill the process mid-turn."""
    def act(ws: Path) -> None:
        (ws.parent / "started").write_text("1")
        while not sentinel.exists():
            time.sleep(0.02)
        then(ws)
    return act


@dataclass
class Scripted:
    """Runtime whose n-th turn for a step runs ``plan[(step, n)]``; missing entries write ``default``."""

    plan: dict[tuple[str, int], Action] = field(default_factory=dict[tuple[str, int], Action])
    default: Callable[[str], Any] | None = None
    name: str = "fake"
    model: str = "fake-1"
    prompts: list[str] = field(default_factory=list[str])
    counts: dict[str, int] = field(default_factory=dict[str, int])

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        step = json.loads((workspace / "step.json").read_text())["step"]
        self.counts[step] = self.counts.get(step, 0) + 1
        self.prompts.append(prompt)
        action = self.plan.get((step, self.counts[step]))
        if action is None:
            if self.default is None:
                raise RuntimeFailure(f"no plan for {step} turn {self.counts[step]}")
            action = write(self.default(step))
        action(workspace)
        return TurnResult(tokens_in=100, tokens_out=10, model=self.model)
