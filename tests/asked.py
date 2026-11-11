"""A role that interviews the person before its work. Loaded by ``jaynes run``."""

from __future__ import annotations

from pathlib import Path

from jaynes.runtime import TurnResult
from tests import fakes
from tests.test_interview import ASKS, GUESSED

ROLE = ASKS


class Agent:
    name = "fake"

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        fakes.write(GUESSED if "# Step: interview" in prompt else {"summary": "a look"})(workspace)
        return TurnResult(10, 1, "fake-1")
