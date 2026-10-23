"""Steps a role marks may read the web; every other step runs without it, and what was read is recorded."""

from __future__ import annotations

import json
from pathlib import Path

from pydantic import BaseModel

from jaynes.conductor import Conductor
from jaynes.role import role
from jaynes.runtime import TurnResult
from jaynes.runtimes import claude_result, claude_settings
from tests.fakes import Scripted, write


class Note(BaseModel):
    text: str


def two_steps():  # noqa: ANN201
    r = role("look", agent="fake")
    direction = r.step("direction", output=Note, web=True)
    r.step("apply", output=Note, needs=[direction])
    return r


def test_only_marked_steps_get_the_web_and_what_they_read_is_recorded(tmp_path: Path) -> None:
    agent = Scripted(default=lambda _: {"text": "ok"}, web_read={"direction": ("https://linear.app",)})
    c = Conductor(tmp_path / "home", runtimes={"fake": lambda: agent})
    assert c.run(two_steps(), "t", "r1").status == "finished"
    assert agent.webs == [True, False]
    assert "may read the web" in agent.prompts[0] and "may read the web" not in agent.prompts[1]
    finished = {e.payload["step"]: e.payload for e in c.store.events("TurnFinished", run_id="r1")}
    assert finished["direction"]["web"] == ["https://linear.app"] and "web" not in finished["apply"]


class Offline:
    name = "fake"

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        write({"text": "ok"})(workspace)
        return TurnResult(1, 1, "offline")


def test_a_runtime_that_cannot_open_the_web_is_refused_for_a_web_step(tmp_path: Path) -> None:
    c = Conductor(tmp_path / "home", runtimes={"fake": Offline})
    result = c.run(two_steps(), "t", "r1")
    assert result.status == "blocked"
    assert any("cannot open the web" in r for r in result.reasons)


def test_marking_a_step_for_the_web_changes_its_fingerprint_and_leaves_others_alone() -> None:
    a, b = role("x"), role("x")
    plain = a.step("s", output=Note)
    webbed = b.step("s", output=Note, web=True)
    assert plain.prompt_hash() != webbed.prompt_hash()
    assert plain.prompt_hash() == role("x").step("s", output=Note).prompt_hash()


def test_claude_gets_its_web_tools_only_on_a_web_step_and_bash_stays_offline(tmp_path: Path) -> None:
    off, on = claude_settings(tmp_path), claude_settings(tmp_path, web=True)
    assert "WebFetch" in off["permissions"]["deny"] and "WebSearch" in off["permissions"]["deny"]
    assert "WebFetch" not in on["permissions"]["deny"] and {"WebFetch", "WebSearch"} <= set(on["permissions"]["allow"])
    assert on["sandbox"]["network"]["allowedDomains"] == []


def test_the_web_addresses_a_claude_turn_read_come_from_its_stream() -> None:
    lines = [{"type": "system", "subtype": "init"},
             {"type": "assistant", "message": {"content": [
                 {"type": "tool_use", "name": "WebFetch", "input": {"url": "https://linear.app", "prompt": "p"}},
                 {"type": "tool_use", "name": "WebSearch", "input": {"query": "dark dashboard fonts"}},
                 {"type": "tool_use", "name": "Read", "input": {"file_path": "x"}}]}},
             {"type": "result", "is_error": False, "result": "done", "usage": {"output_tokens": 3}}]
    result, web = claude_result("\n".join(json.dumps(x) for x in lines))
    assert result["result"] == "done"
    assert web == ["https://linear.app", "search: dark dashboard fonts"]
    assert claude_result("not json")[0]["is_error"] is True
