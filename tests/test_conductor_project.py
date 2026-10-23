"""Steps that read or change a run's project: copies, write boundary, patches between steps, the final branch."""

from __future__ import annotations

import subprocess
from pathlib import Path
from typing import Any

import pytest
from pydantic import BaseModel

from jaynes.conductor import Conductor, StepState
from jaynes.role import Fail, Pass, Witnesses, role, stop
from tests.fakes import Action, Scripted, write


class Note(BaseModel):
    text: str


def git(cwd: Path, *args: str) -> str:
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    r = tmp_path / "repo"
    (r / "src").mkdir(parents=True)
    (r / "src" / "app.css").write_text(":root { --bg: #ffffff; }\n")
    (r / "src" / "app.ts").write_text("export const x = 1\n")
    git(r, "init", "-q")
    git(r, "add", "-A")
    git(r, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "start")
    return r


def edit(path: str, text: str, value: Any) -> Action:
    def act(ws: Path) -> None:
        (ws / "project" / path).write_text(text)
        write(value)(ws)
    return act


def payloads(c: Conductor, run_id: str, kind: str) -> list[dict[str, Any]]:
    return [e.payload for e in c.store.events(kind, run_id=run_id)]


def test_a_reading_step_sees_the_last_commit_and_its_check_can_open_the_copy(repo: Path, tmp_path: Path) -> None:
    seen: list[str] = []

    def cites_real_css(out: Note, needs: Witnesses) -> Pass | Fail:
        assert needs.project is not None
        css = (needs.project / "src" / "app.css").read_text()
        seen.append(css)
        return Pass() if out.text in css else Fail(f"{out.text} is not in src/app.css")

    r = role("read", agent="fake")
    r.step("current", output=Note, project="read", check=cites_real_css)
    agent = Scripted({("current", 1): write({"text": "#000000"}), ("current", 2): write({"text": "#ffffff"})})
    c = Conductor(tmp_path / "home", runtimes={"fake": lambda: agent})

    assert c.run(r, "t", "r1", project=repo).status == "finished"
    assert [p["reasons"] for p in payloads(c, "r1", "TurnRejected")] == [["#000000 is not in src/app.css"]]
    assert seen[-1] == ":root { --bg: #ffffff; }\n"
    assert payloads(c, "r1", "RunStarted")[0]["project"] == {"root": str(repo.resolve()),
                                                            "base": git(repo, "rev-parse", "HEAD")}
    assert payloads(c, "r1", "BranchCreated") == []  # nothing changed, so no branch


def test_a_reading_step_that_changes_the_project_is_rejected(repo: Path, tmp_path: Path) -> None:
    r = role("read", agent="fake")
    r.step("current", output=Note, project="read")
    agent = Scripted({("current", 1): edit("src/app.css", "x", {"text": "a"}), ("current", 2): write({"text": "b"})})
    c = Conductor(tmp_path / "home", runtimes={"fake": lambda: agent})
    c.run(r, "t", "r1", project=repo)
    [rejected] = payloads(c, "r1", "TurnRejected")
    assert rejected["reason"] == "write_boundary" and "src/app.css" in rejected["reasons"][0]


def test_a_changing_step_is_limited_to_its_globs_and_its_patch_reaches_the_next_step(
        repo: Path, tmp_path: Path) -> None:
    r = role("paint", agent="fake")
    apply = r.step("apply", output=Note, project="write", may_change=["**/*.css"])
    r.step("review", output=Note, needs=[apply], project="read")
    seen: dict[str, str] = {}

    def look(ws: Path) -> None:
        seen["css"] = (ws / "project" / "src" / "app.css").read_text()
        write({"text": "looks right"})(ws)

    agent = Scripted({
        ("apply", 1): edit("src/app.ts", "export const x = 2\n", {"text": "palette"}),
        ("apply", 2): edit("src/app.css", ":root { --bg: #101418; }\n", {"text": "palette"}),
        ("review", 1): look,
    })
    c = Conductor(tmp_path / "home", runtimes={"fake": lambda: agent})

    assert c.run(r, "make it dark", "r1", project=repo).status == "finished"

    [rejected] = payloads(c, "r1", "TurnRejected")
    assert rejected["reason"] == "write_boundary" and "src/app.ts" in rejected["reasons"][0]
    checked = {p["step"]: p for p in payloads(c, "r1", "Checked")}
    assert checked["apply"]["changed"] == ["src/app.css"] and checked["review"]["changed"] == []
    assert seen["css"] == ":root { --bg: #101418; }\n"
    [made] = payloads(c, "r1", "BranchCreated")
    assert made["branch"] == "jaynes/r1"
    assert git(repo, "show", "jaynes/r1:src/app.css") == ":root { --bg: #101418; }"
    assert git(repo, "log", "--format=%s", "-1", "jaynes/r1") == "paint, apply: make it dark"
    assert (repo / "src" / "app.css").read_text() == ":root { --bg: #ffffff; }\n"  # the checkout is untouched


def test_a_returned_step_starts_again_from_the_earlier_steps_without_its_own_patch(
        repo: Path, tmp_path: Path) -> None:
    r = role("paint", agent="fake")
    r.step("apply", output=Note, project="write", stop=stop(0.1))
    seen: list[str] = []

    def second(ws: Path) -> None:
        seen.append((ws / "project" / "src" / "app.css").read_text())
        edit("src/app.css", ":root { --bg: #202020; }\n", {"text": "warmer"})(ws)

    agent = Scripted({("apply", 1): edit("src/app.css", ":root { --bg: #000000; }\n", {"text": "dark"}),
                      ("apply", 2): second})
    c = Conductor(tmp_path / "home", runtimes={"fake": lambda: agent})
    assert c.run(r, "t", "r1", project=repo).status == "waiting"
    c.answer(r, "r1", "apply", "return", note="warmer")
    assert c.run(r, "t", "r1").status == "waiting"
    assert seen == [":root { --bg: #ffffff; }\n"]
    c.answer(r, "r1", "apply", "accept")
    assert c.run(r, "t", "r1").status == "finished"
    assert git(repo, "show", "jaynes/r1:src/app.css") == ":root { --bg: #202020; }"


def test_a_project_step_in_a_run_without_a_project_blocks_with_the_reason(tmp_path: Path) -> None:
    r = role("read", agent="fake")
    r.step("current", output=Note, project="read")
    c = Conductor(tmp_path / "home", runtimes={"fake": lambda: Scripted({})})
    result = c.run(r, "t", "r1")
    assert result.status == "blocked" and "project folder" in result.reasons[0]


def test_the_agent_is_told_where_the_project_is_and_what_it_may_change(tmp_path: Path) -> None:
    r = role("paint", agent="fake")
    step = r.step("apply", output=Note, project="write", may_change=["**/*.css"])
    c = Conductor(tmp_path / "home")
    prompt = c.prompt(r, step, "t", StepState())
    assert "project/" in prompt and "**/*.css" in prompt


def test_globs_without_a_changing_step_are_refused() -> None:
    with pytest.raises(ValueError, match="may_change"):
        role("x").step("s", output=Note, project="read", may_change=["*.css"])
