"""Roles for returning an earlier step from the page: two stops in a row, a stop with no output after a step
with one, and the same as a floor. Loaded by ``jaynes run``."""

from __future__ import annotations

from pathlib import Path

from pydantic import BaseModel

from jaynes.role import role, stop
from jaynes.runtime import TurnResult
from tests import fakes


class Note(BaseModel):
    text: str


ROLE = role("two-stops", agent="quick")
OUTLINE = ROLE.step("outline", output=Note, stop=stop(0.2))
DRAFT = ROLE.step("draft", output=Note, needs=[OUTLINE], stop=stop(0.2))

PURE = role("pure-stop", agent="quick")
WRITE = PURE.step("write", output=Note)
PURE.step("accept", needs=[WRITE], stop=stop(0.2))

FLOOR = role("floor-stop", agent="quick")
FLOOR.step("accept", needs=[FLOOR.step("write", output=Note)], stop=stop(0.05, sampled=False))


class Agent:
    name = "quick"

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        fakes.write({"text": "words"})(workspace)
        return TurnResult(10, 1, "quick-1")
