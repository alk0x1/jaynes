"""Returning an earlier step from a later stop: the earlier step runs again, and so does everything after it."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest
from pydantic import BaseModel

from jaynes.conductor import AnswerRejected, Conductor
from jaynes.role import role, stop
from tests.fakes import Scripted, write


class Note(BaseModel):
    text: str


def git(cwd: Path, *args: str) -> str:
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    r = tmp_path / "repo"
    r.mkdir()
    (r / "app.css").write_text(":root { --bg: #ffffff; }\n")
    git(r, "init", "-q")
    git(r, "add", "-A")
    git(r, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "start")
    return r


def paint(color: str):  # noqa: ANN201
    def act(ws: Path) -> None:
        palette = json.loads((ws / "inputs" / "direction.json").read_text())["text"]
        (ws / "project" / "app.css").write_text(f":root {{ --bg: {palette}; }}\n")
        write({"text": f"applied {color}"})(ws)
    return act


def three_steps():  # noqa: ANN201
    r = role("paint", agent="fake")
    read = r.step("read", output=Note)
    direction = r.step("direction", output=Note, needs=[read], stop=stop(0.2))
    r.step("apply", output=Note, needs=[direction], project="write", may_change=["**"], stop=stop(0.2))
    return r


def test_returning_an_earlier_step_reruns_it_with_the_note_and_every_step_after_it(
        repo: Path, tmp_path: Path) -> None:
    r = three_steps()
    seen: dict[str, object] = {}

    def second_direction(ws: Path) -> None:
        seen["previous"] = json.loads((ws / "inputs" / "previous.json").read_text())
        write({"text": "#202020"})(ws)

    agent = Scripted({("read", 1): write({"text": "white"}), ("direction", 1): write({"text": "#ff5a1f"}),
                      ("apply", 1): paint("orange"), ("direction", 2): second_direction, ("apply", 2): paint("grey")})
    c = Conductor(tmp_path / "home", runtimes={"fake": lambda: agent})
    assert c.run(r, "t", "r1", project=repo).step == "direction"
    c.answer(r, "r1", "direction", "accept", by="person")
    assert c.run(r, "t", "r1").step == "apply"

    c.answer(r, "r1", "apply", "return", note="the orange is too loud", to="direction", by="person")

    result = c.run(r, "t", "r1")
    assert result.status == "waiting" and result.step == "direction"
    assert seen["previous"] == {"text": "#ff5a1f"}
    assert agent.counts["read"] == 1  # steps before the returned one keep their output
    assert "the orange is too loud" in agent.prompts[-1]
    c.answer(r, "r1", "direction", "accept", by="person")
    assert c.run(r, "t", "r1").step == "apply"
    c.answer(r, "r1", "apply", "accept", by="person")
    assert c.run(r, "t", "r1").status == "finished"
    assert git(repo, "show", "jaynes/r1:app.css") == ":root { --bg: #202020; }"  # only the second apply


def test_the_earlier_step_takes_the_label_and_the_later_stop_takes_none(repo: Path, tmp_path: Path) -> None:
    r = three_steps()
    agent = Scripted({("apply", 1): paint("a"), ("apply", 2): paint("b")}, default=lambda _: {"text": "#101010"})
    c = Conductor(tmp_path / "home", runtimes={"fake": lambda: agent})
    c.run(r, "t", "r1", project=repo)
    c.answer(r, "r1", "direction", "accept", by="person")
    c.run(r, "t", "r1")
    direction_fp = next(iter(c.store.events("StopOpened", run_id="r1"))).payload["fingerprint"]
    apply_fp = list(c.store.events("StopOpened", run_id="r1"))[-1].payload["fingerprint"]
    assert (c.window(direction_fp).n, c.window(direction_fp).k) == (1, 0)

    c.answer(r, "r1", "apply", "return", note="not this palette", to="direction", by="person")

    assert (c.window(direction_fp).n, c.window(direction_fp).k) == (1, 1)  # the accept turned out wrong
    assert c.window(apply_fp).n == 0  # apply did what it was given


def test_context_for_an_earlier_step_withdraws_its_label(repo: Path, tmp_path: Path) -> None:
    r = three_steps()
    agent = Scripted({("apply", 1): paint("a")}, default=lambda _: {"text": "#101010"})
    c = Conductor(tmp_path / "home", runtimes={"fake": lambda: agent})
    c.run(r, "t", "r1", project=repo)
    c.answer(r, "r1", "direction", "accept", by="person")
    c.run(r, "t", "r1")
    direction_fp = next(iter(c.store.events("StopOpened", run_id="r1"))).payload["fingerprint"]
    c.answer(r, "r1", "apply", "context", note="it must also work in light mode", to="direction", by="person")
    assert c.window(direction_fp).n == 0


def test_only_an_earlier_step_with_an_output_can_be_returned(repo: Path, tmp_path: Path) -> None:
    r = three_steps()
    c = Conductor(tmp_path / "home", runtimes={"fake": lambda: Scripted({}, default=lambda _: {"text": "#101010"})})
    c.run(r, "t", "r1", project=repo)
    c.answer(r, "r1", "direction", "accept", by="person")
    c.run(r, "t", "r1")
    for to in ("apply", "nowhere"):
        with pytest.raises(AnswerRejected, match="earlier step"):
            c.answer(r, "r1", "apply", "return", note="x", to=to, by="person")
    with pytest.raises(AnswerRejected, match="return or context"):
        c.answer(r, "r1", "apply", "accept", to="direction", by="person")
