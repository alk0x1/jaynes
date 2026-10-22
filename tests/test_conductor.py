"""Conductor behaviour, observed through the event log and the checked outputs."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest
from pydantic import BaseModel

from jaynes.conductor import AnswerRejected, Conductor, RunResult, coin
from jaynes.gate import Window
from jaynes.role import Fail, Pass, Witnesses, role, stop
from tests.fakes import Scripted, fail, nothing, quota, raw, stray, tamper, timeout, write


class Note(BaseModel):
    text: str


def kinds(c: Conductor, run_id: str) -> list[str]:
    return [e.type for e in c.store.events(run_id=run_id)]


def payloads(c: Conductor, run_id: str, kind: str) -> list[dict[str, Any]]:
    return [e.payload for e in c.store.events(kind, run_id=run_id)]


def test_one_step_run_finishes_with_a_checked_output(tmp_path: Path) -> None:
    r = role("echo", agent="fake")
    draft = r.step("draft", output=Note, instructions="Write a note.")
    agent = Scripted({("draft", 1): write({"text": "hi"})})
    c = Conductor(tmp_path, runtimes={"fake": lambda: agent})

    result = c.run(r, "say hi", run_id="r1")

    assert result.status == "finished"
    assert kinds(c, "r1") == ["RunStarted", "StepStarted", "TurnFinished", "Checked", "RunFinished"]
    assert c.output("r1", draft) == Note(text="hi")


def chain(agent_name: str = "fake") -> tuple[Any, Any, Any]:
    r = role("chain", agent=agent_name)
    a = r.step("a", output=Note)
    b = r.step("b", output=Note, needs=[a], check=mentions_a)
    return r, a, b


def mentions_a(out: Note, needs: Witnesses) -> Pass | Fail:
    return Pass() if needs.names() == ["a"] and "a" in out.text else Fail("b must mention a")


def test_rejected_turn_is_retried_with_the_reasons_in_the_prompt(tmp_path: Path) -> None:
    r, _, b = chain()
    agent = Scripted({("a", 1): write({"text": "a"}), ("b", 1): write({"text": "zzz"}),
                      ("b", 2): write({"text": "about a"})})
    c = Conductor(tmp_path, runtimes={"fake": lambda: agent})

    assert c.run(r, "t", run_id="r1").status == "finished"

    rejected = payloads(c, "r1", "TurnRejected")
    assert [(p["step"], p["reason"], p["reasons"]) for p in rejected] == [("b", "check", ["b must mention a"])]
    assert "- b must mention a" in agent.prompts[-1]
    assert c.output("r1", b) == Note(text="about a")


def test_write_outside_the_output_rejects_the_turn(tmp_path: Path) -> None:
    r, _, _ = chain()
    agent = Scripted({("a", 1): write({"text": "a"}), ("b", 1): tamper({"text": "a"}),
                      ("b", 2): stray({"text": "a"}), ("b", 3): write({"text": "a"})})
    c = Conductor(tmp_path, runtimes={"fake": lambda: agent})

    assert c.run(r, "t", run_id="r1").status == "finished"

    assert [p["reason"] for p in payloads(c, "r1", "TurnRejected")] == ["write_boundary", "write_boundary"]
    assert [p["files_changed"] for p in payloads(c, "r1", "TurnFinished")] == [[], ["inputs/a.json"], ["notes.txt"], []]


def test_schema_errors_and_missing_output_are_rejections(tmp_path: Path) -> None:
    r, _, _ = chain()
    agent = Scripted({("a", 1): raw("not json"), ("a", 2): write({"txt": 1}), ("a", 3): nothing})
    c = Conductor(tmp_path, runtimes={"fake": lambda: agent})

    result = c.run(r, "t", run_id="r1")

    assert result.status == "blocked" and result.step == "a"
    assert [p["reason"] for p in payloads(c, "r1", "TurnRejected")] == ["schema"] * 3
    assert "StepStarted" not in [e.type for e in c.store.events(run_id="r1") if e.payload.get("step") == "b"]
    assert c.run(r, "t", run_id="r1").status == "blocked"  # blocked stays blocked


def test_runtime_failures_and_deadlines_count_as_turns(tmp_path: Path) -> None:
    r, _, _ = chain()
    agent = Scripted({("a", 1): fail, ("a", 2): timeout, ("a", 3): write({"text": "a"}),
                      ("b", 1): write({"text": "a"})})
    c = Conductor(tmp_path, runtimes={"fake": lambda: agent})

    assert c.run(r, "t", run_id="r1").status == "finished"
    assert [p["reason"] for p in payloads(c, "r1", "TurnRejected")] == ["runtime", "deadline"]


def test_quota_pauses_without_spending_an_attempt_and_resumes_after_reset(tmp_path: Path) -> None:
    now = [1000.0]
    r, _, _ = chain()
    agent = Scripted({("a", 1): quota(2000.0), ("a", 2): write({"text": "a"}), ("b", 1): write({"text": "a"})})
    c = Conductor(tmp_path, runtimes={"fake": lambda: agent}, clock=lambda: now[0])

    assert c.run(r, "t", run_id="r1") == RunResult("paused", "a", 2000.0)
    assert c.run(r, "t", run_id="r2").status == "paused"  # another run on the same agent waits too
    assert agent.counts == {"a": 1}
    now[0] = 2001.0
    assert c.run(r, "t", run_id="r1").status == "finished"
    assert payloads(c, "r1", "TurnRejected") == []
    assert [p["reason"] for p in payloads(c, "r1", "TurnInterrupted")] == ["quota"]


def test_trusted_executor_output_is_checked_and_never_written_by_the_agent(tmp_path: Path) -> None:
    r = role("t", agent="fake")
    snap = r.step("snapshot", output=Note, run_by=lambda ctx: Note(text=f"snap of {ctx.task}"))
    r.step("use", output=Note, needs=[snap])
    agent = Scripted({("use", 1): write({"text": "used"})})
    c = Conductor(tmp_path, runtimes={"fake": lambda: agent})

    assert c.run(r, "home", run_id="r1").status == "finished"
    assert c.output("r1", snap) == Note(text="snap of home")
    assert agent.counts == {"use": 1}
    assert [p["source"] for p in payloads(c, "r1", "Checked")] == ["jaynes", "fake"]
    ws = tmp_path / "runs/r1/use/turn-1/workspace"
    assert json.loads((ws / "inputs/snapshot.json").read_text()) == {"text": "snap of home"}


def reviewed(budget: float = 0.2, sampled: bool = True, audit_rate: float = 0.1) -> tuple[Any, Any, Any]:
    r = role("rev", agent="fake")
    a = r.step("a", output=Note, check=lambda out, _: Pass() if out.text else Fail("empty"),
               stop=stop(budget, sampled=sampled, audit_rate=audit_rate))
    b = r.step("b", output=Note, needs=[a])
    return r, a, b


def test_run_waits_at_a_stop_and_continues_after_accept(tmp_path: Path) -> None:
    r, a, _ = reviewed()
    agent = Scripted(default=lambda step: {"text": step})
    c = Conductor(tmp_path, runtimes={"fake": lambda: agent})

    assert c.run(r, "t", run_id="r1") == RunResult("waiting", "a")
    assert c.run(r, "t", run_id="r1") == RunResult("waiting", "a")  # still waiting, nothing new
    assert kinds(c, "r1").count("StopOpened") == 1
    c.answer(r, "r1", "a", "accept")
    assert c.run(r, "t", run_id="r1").status == "finished"

    [opened] = payloads(c, "r1", "StopOpened")
    assert (opened["status"], opened["n"], opened["k"]) == ("undetermined", 0, 0)
    [answered] = payloads(c, "r1", "StopAnswered")
    assert answered["label"] == "correct"
    assert c.window(opened["fingerprint"]) == Window(1, 0)
    assert c.output("r1", a) == Note(text="a")


def test_change_replaces_the_output_after_rechecking_it(tmp_path: Path) -> None:
    r, a, _ = reviewed()
    agent = Scripted(default=lambda step: {"text": step})
    c = Conductor(tmp_path, runtimes={"fake": lambda: agent})
    c.run(r, "t", run_id="r1")

    with pytest.raises(AnswerRejected, match="empty"):
        c.answer(r, "r1", "a", "change", output={"text": ""})
    c.answer(r, "r1", "a", "change", output=Note(text="better"))

    assert c.run(r, "t", run_id="r1").status == "finished"
    assert c.output("r1", a) == Note(text="better")
    assert [p["source"] for p in payloads(c, "r1", "Checked")] == ["fake", "person", "fake"]
    assert payloads(c, "r1", "StopAnswered")[0]["label"] == "error"
    ws = tmp_path / "runs/r1/b/turn-1/workspace"
    assert json.loads((ws / "inputs/a.json").read_text()) == {"text": "better"}


def test_return_reruns_the_step_with_the_note_and_context_is_not_counted(tmp_path: Path) -> None:
    r, a, _ = reviewed()
    agent = Scripted({("a", 1): write({"text": "one"}), ("a", 2): write({"text": "two"}),
                      ("a", 3): write({"text": "three"})}, default=lambda step: {"text": step})
    c = Conductor(tmp_path, runtimes={"fake": lambda: agent})
    c.run(r, "t", run_id="r1")

    c.answer(r, "r1", "a", "return", note="shorter please")
    assert c.run(r, "t", run_id="r1") == RunResult("waiting", "a")
    assert "shorter please" in agent.prompts[-1] and "inputs/previous.json" in agent.prompts[-1]
    c.answer(r, "r1", "a", "context", note="the data was stale")
    assert c.run(r, "t", run_id="r1") == RunResult("waiting", "a")
    c.answer(r, "r1", "a", "accept")

    assert c.run(r, "t", run_id="r1").status == "finished"
    assert c.output("r1", a) == Note(text="three")
    fp = payloads(c, "r1", "StopOpened")[0]["fingerprint"]
    assert c.window(fp) == Window(2, 1)  # return is an error, context is not counted, accept is correct


def test_answer_without_an_open_stop_is_rejected(tmp_path: Path) -> None:
    r, _, _ = reviewed()
    c = Conductor(tmp_path, runtimes={"fake": lambda: Scripted(default=lambda s: {"text": s})})
    with pytest.raises(AnswerRejected):
        c.answer(r, "r1", "a", "accept")


def certify(c: Conductor, r: Any, labels: int) -> None:
    for i in range(labels):
        assert c.run(r, "t", run_id=f"warm{i}").status == "waiting"
        c.answer(r, f"warm{i}", "a", "accept")
        c.run(r, "t", run_id=f"warm{i}")


def test_certified_stop_waits_only_on_the_audit_share(tmp_path: Path) -> None:
    r, _, _ = reviewed(budget=0.3, audit_rate=0.25)
    c = Conductor(tmp_path, runtimes={"fake": lambda: Scripted(default=lambda s: {"text": s})})
    certify(c, r, 13)  # 13 clean labels certify a 30% budget at delta 5% (SPEC 6.4)

    waited = 0
    for i in range(200):
        result = c.run(r, "t", run_id=f"x{i}")
        waited += result.status == "waiting"
        status = (payloads(c, f"x{i}", "StopOpened") or payloads(c, f"x{i}", "StopSkipped"))[0]["status"]
        assert status == "certified"
        assert (result.status == "waiting") == (coin(f"x{i}", "a") < 0.25)
    assert 25 <= waited <= 75


def test_floor_waits_every_run_whatever_the_labels(tmp_path: Path) -> None:
    r, _, _ = reviewed(budget=0.3, sampled=False)
    c = Conductor(tmp_path, runtimes={"fake": lambda: Scripted(default=lambda s: {"text": s})})
    certify(c, r, 20)
    assert c.run(r, "t", run_id="x").status == "waiting"
    assert payloads(c, "x", "StopOpened")[0]["status"] == "floor"


def test_changing_the_check_opens_a_new_window(tmp_path: Path) -> None:
    r, _, _ = reviewed(budget=0.3)
    c = Conductor(tmp_path, runtimes={"fake": lambda: Scripted(default=lambda s: {"text": s})})
    certify(c, r, 13)
    r2 = role("rev", agent="fake")
    r2.step("a", output=Note, check=lambda out, _: Pass() if len(out.text) < 99 else Fail("long"), stop=stop(0.3))
    assert c.run(r2, "t", run_id="new").status == "waiting"
    assert payloads(c, "new", "StopOpened")[0]["status"] == "undetermined"


def test_a_check_that_returns_neither_pass_nor_fail_fails(tmp_path: Path) -> None:
    r = role("sloppy", agent="fake")
    r.step("a", output=Note, check=lambda out, _: False, max_turns=1)  # type: ignore[arg-type, return-value]
    c = Conductor(tmp_path, runtimes={"fake": lambda: Scripted(default=lambda s: {"text": s})})
    assert c.run(r, "t", run_id="r1").status == "blocked"
    assert payloads(c, "r1", "TurnRejected")[0]["reasons"] == ["check <lambda> returned False, not Pass or Fail"]


def test_method_file_is_read_relative_to_the_role_module(tmp_path: Path) -> None:
    r = role("m", agent="fake", method=Path("method_fixture.md"))
    r.step("a", output=Note)
    agent = Scripted(default=lambda s: {"text": s})
    Conductor(tmp_path, runtimes={"fake": lambda: agent}).run(r, "t", run_id="r1")
    assert agent.prompts[0].startswith("Cite the snapshot for every claim.")


def test_a_stop_that_keeps_failing_is_refuted_and_waits(tmp_path: Path) -> None:
    r, _, _ = reviewed(budget=0.05)
    c = Conductor(tmp_path, runtimes={"fake": lambda: Scripted(default=lambda s: {"text": s})})
    for i in range(6):
        c.run(r, "t", run_id=f"w{i}")
        c.answer(r, f"w{i}", "a", "change", output={"text": "fixed"})
    assert c.run(r, "t", run_id="x").status == "waiting"
    assert payloads(c, "x", "StopOpened")[0]["status"] == "refuted"

