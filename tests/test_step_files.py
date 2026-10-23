"""Files a trusted executor leaves next to its output reach the steps that need it, read-only."""

from __future__ import annotations

from pathlib import Path

from pydantic import BaseModel

from jaynes.conductor import Conductor
from jaynes.role import StepContext, role
from tests.fakes import Scripted, write


class Note(BaseModel):
    text: str


def gather(ctx: StepContext) -> Note:
    (ctx.workspace / "files").mkdir()
    (ctx.workspace / "files" / "ref-1.png").write_bytes(b"\x89PNG picture")
    return Note(text="one picture")


def test_files_of_a_trusted_step_are_inputs_of_the_steps_that_need_it(tmp_path: Path) -> None:
    r = role("look", agent="fake")
    refs = r.step("references", output=Note, run_by=gather)
    r.step("direction", output=Note, needs=[refs])
    seen: dict[str, bytes] = {}

    def direction(ws: Path) -> None:
        seen["picture"] = (ws / "inputs" / "references" / "ref-1.png").read_bytes()
        write({"text": "dark"})(ws)

    agent = Scripted({("direction", 1): direction})
    c = Conductor(tmp_path / "home", runtimes={"fake": lambda: agent})
    assert c.run(r, "t", "r1").status == "finished"
    assert seen["picture"] == b"\x89PNG picture"
    assert "inputs/references/ref-1.png" in agent.prompts[0]
    checked = [e.payload for e in c.store.events("Checked", run_id="r1") if e.payload["step"] == "references"]
    assert list(checked[0]["files"]) == ["ref-1.png"]
