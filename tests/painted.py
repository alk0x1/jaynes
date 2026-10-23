"""Role that changes a project's CSS and stops for the person, loaded by ``jaynes run`` in a child process."""

from __future__ import annotations

import json
from pathlib import Path

from pydantic import BaseModel

from jaynes.role import role, stop
from jaynes.runtime import TurnResult


class Note(BaseModel):
    text: str


ROLE = role("painted", agent="painter")
PAINT = ROLE.step("paint", output=Note, project="write", may_change=["**/*.css"], stop=stop(0.2),
                  question=lambda out, _: "Keep this palette?")


class Agent:
    name = "painter"

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        (workspace / "project" / "style.css").write_text("body { background: #101418; color: #eef2f7; }\n")
        (workspace / "output.json").write_text(json.dumps({"text": "dark"}))
        return TurnResult(10, 1, "painter-1")
