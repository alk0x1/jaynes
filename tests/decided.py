"""A role whose stop shows its output as decisions, accepted or rejected one by one. Loaded by ``jaynes run``."""

from __future__ import annotations

from pathlib import Path

from jaynes.runtime import TurnResult
from tests import fakes
from tests.test_decisions import FIRST, LOOKS

ROLE = LOOKS


class Agent:
    name = "fake"

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        fakes.write(FIRST)(workspace)
        return TurnResult(10, 1, "fake-1")
