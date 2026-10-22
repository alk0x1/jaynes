"""``jaynes run`` refuses bad input with a message instead of a traceback."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

import pytest
from pydantic import BaseModel

from jaynes import cli
from jaynes.conductor import Conductor
from jaynes.role import role
from jaynes.store import Store
from tests.fakes import Scripted

ROOT = Path(__file__).resolve().parents[1]


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


def jaynes(home: Path, *args: str) -> str:
    env = dict(os.environ, PYTHONPATH=str(ROOT))
    done = subprocess.run([sys.executable, "-m", "jaynes.cli", *args[:1], "--home", str(home), *args[1:]],
                          env=env, capture_output=True, text=True, timeout=60, cwd=ROOT)
    assert done.returncode == 0, done.stderr
    return done.stdout


def test_answer_from_the_cli_continues_the_run_in_the_background(tmp_path: Path) -> None:
    started = json.loads(jaynes(tmp_path, "run", "--role", "tests.stopped:ROLE", "--task", "t",
                                "--runtime", "quick=tests.stopped:Agent"))
    assert (started["status"], started["step"]) == ("waiting", "draft")
    run = started["run"]
    assert f"{run}  stop  draft" in jaynes(tmp_path, "queue")
    assert "Ship 'draft one'?" in jaynes(tmp_path, "queue")

    jaynes(tmp_path, "answer", run, "draft", "change", "--output", json.dumps({"text": "draft two"}),
           "--note", "warmer")

    store = Store(tmp_path / "jaynes.sqlite3")
    deadline = time.monotonic() + 30
    while not list(store.events("RunFinished", run_id=run)):
        assert time.monotonic() < deadline, (tmp_path / "runs" / run / "continue.log").read_text()
        time.sleep(0.05)
    answered = [e.payload for e in store.events("StopAnswered", run_id=run)]
    assert [(a["answer"], a["label"], a["note"]) for a in answered] == [("change", "error", "warmer")]
    assert jaynes(tmp_path, "queue").strip() == "nothing is waiting"
