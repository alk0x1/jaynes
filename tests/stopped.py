"""Role with a stop and an agent for the CLI answer test, loaded by ``jaynes run`` in a child process."""

from __future__ import annotations

import time
from pathlib import Path

from pydantic import BaseModel

from jaynes.role import role, stop
from jaynes.runtime import QuotaExhausted, Ran, TurnResult
from tests import fakes


class Note(BaseModel):
    text: str


ROLE = role("stopped", agent="quick")
DRAFT = ROLE.step("draft", output=Note, stop=stop(0.2), question=lambda out, _: f"Ship {out.text!r}?")
FINAL = ROLE.step("final", output=Note, needs=[DRAFT])


class Agent:
    name = "quick"

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        fakes.write({"text": "draft one"})(workspace)
        return TurnResult(10, 1, "quick-1")


class Found(BaseModel):
    found: bool
    why: str


ENDING = role("ending", agent="quick")
LOOK = ENDING.step("look", output=Found, stop=stop(0.2), ends=lambda out: not out.found)
ENDING.step("fix", output=Note, needs=[LOOK])


class Nothing:
    name = "quick"

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        fakes.write({"found": False, "why": "it works"})(workspace)
        return TurnResult(10, 1, "quick-1")


class Tested:
    """Runs the tests twice: they fail, then they pass."""
    name = "quick"

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        fakes.write({"text": "draft one"})(workspace)
        return TurnResult(10, 1, "quick-1", commands=(Ran("uv run pytest -q", False, "Exit code 1\n1 failed"),
                                                      Ran("uv run pytest -q", True, "3 passed in 0.02s")))


class Exhausted:
    """Out of quota for two seconds, once per home; then writes as Agent does."""
    name = "quick"

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        once = workspace.parents[3] / "paused-once"  # runs/, under the home
        if not once.exists():
            once.write_text("1")
            raise QuotaExhausted(time.time() + 2)
        return Agent().turn(workspace, scratch, prompt, deadline)
