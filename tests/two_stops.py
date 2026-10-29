"""Roles for returning an earlier step from the page: two stops in a row, and a stop with no output after a step
with one. Loaded by ``jaynes run``."""

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


class Agent:
    name = "quick"

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        fakes.write({"text": "words"})(workspace)
        return TurnResult(10, 1, "quick-1")
