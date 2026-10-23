"""Role with two stops in a row, for returning an earlier step from the page; loaded by ``jaynes run``."""

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


class Agent:
    name = "quick"

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        fakes.write({"text": "words"})(workspace)
        return TurnResult(10, 1, "quick-1")
