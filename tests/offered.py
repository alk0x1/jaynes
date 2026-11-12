"""A role whose stop offers three looks as decisions, where dropping chooses what is left. Loaded by ``jaynes run``."""

from __future__ import annotations

from pathlib import Path

from jaynes.runtime import TurnResult
from tests import fakes
from tests.test_decisions import OFFER, THREE

ROLE = OFFER


class Agent:
    name = "fake"

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        fakes.write(THREE)(workspace)
        return TurnResult(10, 1, "fake-1")
