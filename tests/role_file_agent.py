"""An agent for the role file test, loaded by ``jaynes run`` in a child process."""

from __future__ import annotations

import json
from pathlib import Path

from jaynes.runtime import TurnResult
from tests import fakes

OUTPUTS = {"draft": {"text": "buy milk"}, "plan": {"steps": ["buy milk today"]}}


class Agent:
    name = "quick"

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        fakes.write(OUTPUTS[json.loads((workspace / "step.json").read_text())["step"]])(workspace)
        return TurnResult(10, 1, "quick-1")
