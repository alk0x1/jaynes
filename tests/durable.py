"""Role and agent for the kill -9 test, loaded by ``jaynes run`` in a child process."""

from __future__ import annotations

import os
import time
from pathlib import Path

from pydantic import BaseModel

from jaynes.role import Fail, Pass, Witnesses, role
from jaynes.runtime import TurnResult
from tests import fakes


class Note(BaseModel):
    text: str


def has_text(out: Note, needs: Witnesses) -> Pass | Fail:
    return Pass() if out.text else Fail("empty")


ROLE = role("durable", agent="slow")
A = ROLE.step("a", output=Note, check=has_text)
B = ROLE.step("b", output=Note, needs=[A], check=has_text)
C = ROLE.step("c", output=Note, needs=[B], check=has_text)


class Agent:
    """Step ``a`` strays once; step ``b`` hangs until the file named by JAYNES_TEST_RELEASE exists."""

    name = "slow"

    def __init__(self) -> None:
        self.release = Path(os.environ["JAYNES_TEST_RELEASE"])

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        step = (workspace / "step.json").read_text()
        if '"a"' in step and "rejected" not in prompt:
            fakes.stray({"text": "a"})(workspace)
        elif '"b"' in step:
            (workspace.parent / "started").write_text("1")
            while not self.release.exists():
                time.sleep(0.02)
            fakes.write({"text": "b"})(workspace)
        else:
            fakes.write({"text": "ok"})(workspace)
        return TurnResult(10, 1, "slow-1")
