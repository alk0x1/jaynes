"""Role whose first step leaves pictures and whose stop, which reads the web, needs them; for ``jaynes run``."""

from __future__ import annotations

from pathlib import Path

from pydantic import BaseModel

from jaynes.role import StepContext, role, stop
from jaynes.runtime import TurnResult
from tests import fakes

PNG = b"\x89PNG\r\n\x1a\nnot really"


class Note(BaseModel):
    text: str


def gather(ctx: StepContext) -> Note:
    (ctx.workspace / "files").mkdir()
    (ctx.workspace / "files" / "ref-1.png").write_bytes(PNG)
    (ctx.workspace / "files" / "ref-2.svg").write_text("<svg onload='alert(1)'/>")
    return Note(text="two files")


ROLE = role("pictured", agent="quick")
LOOK = ROLE.step("look", output=Note, run_by=gather)
ROLE.step("direction", output=Note, needs=[LOOK], stop=stop(0.2), web=True)


class Agent:
    name = "quick"

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float, web: bool = False) -> TurnResult:
        fakes.write({"text": "dark"})(workspace)
        return TurnResult(10, 1, "quick-1", ("https://linear.app", "search: dark fonts") if web else ())
