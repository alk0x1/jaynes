"""``jaynes run`` refuses bad input with a message instead of a traceback."""

from __future__ import annotations

from pathlib import Path

import pytest
from pydantic import BaseModel

from jaynes import cli
from jaynes.conductor import Conductor
from jaynes.role import role
from tests.fakes import Scripted


class Note(BaseModel):
    text: str


def test_runtime_override_without_equals_is_refused(tmp_path: Path) -> None:
    with pytest.raises(SystemExit, match="name=module:attribute"):
        cli.main(["run", "--home", str(tmp_path), "--role", "tests.durable:ROLE", "--task", "t", "--runtime", "slow"])


def test_resuming_a_run_started_from_python_is_refused_clearly(tmp_path: Path) -> None:
    r = role("py", agent="fake")
    r.step("a", output=Note)
    Conductor(tmp_path, runtimes={"fake": lambda: Scripted(default=lambda s: {"text": s})}).run(r, "t", run_id="r1")
    with pytest.raises(SystemExit, match="started from Python"):
        cli.main(["run", "--home", str(tmp_path), "r1"])
