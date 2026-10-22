"""Properties of the conductor over generated roles, agent behaviours, answers and crashes (SPEC section 11, M2)."""

from __future__ import annotations

import hashlib
import json
import tempfile
from pathlib import Path
from typing import Any, cast

from hypothesis import HealthCheck, given, settings
from hypothesis import strategies as st
from pydantic import BaseModel

from jaynes.conductor import Answer, Conductor
from jaynes.role import Fail, Pass, Role, Witnesses, role, stop
from jaynes.runtime import TurnResult
from tests import fakes


class Note(BaseModel):
    text: str


def not_bad(out: Note, needs: Witnesses) -> Pass | Fail:
    return Fail("text is bad") if out.text == "bad" else Pass()


BEHAVIOURS = ["ok", "ok", "ok", "bad", "garbage", "stray", "tamper", "nothing", "fail", "timeout"]
ACTIONS: dict[str, fakes.Action] = {
    "ok": fakes.write({"text": "ok"}), "bad": fakes.write({"text": "bad"}), "garbage": fakes.raw("{"),
    "stray": fakes.stray({"text": "ok"}), "tamper": fakes.tamper({"text": "ok"}), "nothing": fakes.nothing,
    "fail": fakes.fail, "timeout": fakes.timeout}


class Script:
    """Behaviours and answers drawn by Hypothesis once per (step, round, attempt), so a rerun replays them."""

    def __init__(self, data: st.DataObject) -> None:
        self.data = data
        self.turns: dict[tuple[str, int, int], str] = {}
        self.answers: dict[tuple[str, int], str] = {}

    def behaviour(self, key: tuple[str, int, int]) -> str:
        if key not in self.turns:
            self.turns[key] = self.data.draw(st.sampled_from(BEHAVIOURS), label=f"turn {key}")
        return self.turns[key]

    def answer(self, step: str, round_: int) -> Answer:
        key = (step, round_)
        if key not in self.answers:
            choices = ["accept"] if round_ > 3 else ["accept", "change", "return", "context"]
            self.answers[key] = self.data.draw(st.sampled_from(choices), label=f"answer {key}")
        return cast(Answer, self.answers[key])


class Agent:
    name = "fake"

    def __init__(self, script: Script) -> None:
        self.script = script

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        info = json.loads((workspace / "step.json").read_text())
        ACTIONS[self.script.behaviour((info["step"], info["round"], info["attempt"]))](workspace)
        return TurnResult(10, 1, "fake-1")


class Crash(BaseException):
    """Stands in for SIGKILL: not an Exception, so nothing in the conductor catches it."""


@st.composite
def roles(draw: st.DrawFn) -> Role:
    r = role("gen", agent="fake")
    for i in range(draw(st.integers(1, 5))):
        needs = draw(st.lists(st.sampled_from(r.steps), unique=True)) if r.steps else []
        has_stop = draw(st.booleans())
        r.step(f"s{i}", output=Note, needs=needs, check=not_bad,
               stop=stop(0.2, sampled=draw(st.booleans())) if has_stop else None,
               max_turns=draw(st.integers(1, 3)))
    return r


def conductor(home: Path, script: Script) -> Conductor:
    return Conductor(home, runtimes={"fake": lambda: Agent(script)})


def drive(c: Conductor, r: Role, script: Script) -> str:
    """Run to the end, answering every stop with the script's answers."""
    for _ in range(200):
        result = c.run(r, "t", run_id="r")
        if result.status != "waiting":
            return result.status
        assert result.step is not None
        round_ = int(list(c.store.events("StopOpened", run_id="r"))[-1].payload["round"])
        a = script.answer(result.step, round_)
        c.answer(r, "r", result.step, a, note="n", output={"text": "edited"} if a == "change" else None)
    raise AssertionError("run did not settle")


def crash_after(c: Conductor, writes: int) -> None:
    """Make the log raise Crash on its next write after ``writes`` writes; a write is one append or one batch."""
    count = [0]
    append, append_all = c.store.append, c.store.append_all

    def guard() -> None:
        if count[0] >= writes:
            raise Crash()
        count[0] += 1

    def crashing_append(*args: Any, **kwargs: Any) -> None:
        guard()
        append(*args, **kwargs)

    def crashing_append_all(*args: Any, **kwargs: Any) -> None:
        guard()
        append_all(*args, **kwargs)

    c.store.append = crashing_append  # type: ignore[method-assign]
    c.store.append_all = crashing_append_all  # type: ignore[method-assign]


def normalized(c: Conductor) -> list[tuple[str, dict[str, Any]]]:
    """Events without timings, and without each interrupted turn (its StepStarted through TurnInterrupted)."""
    out: list[tuple[str, dict[str, Any]]] = []
    for e in c.store.events(run_id="r"):
        p = {k: v for k, v in e.payload.items() if k not in ("seconds", "seconds_waited")}
        if e.type == "TurnInterrupted":
            while out[-1][0] != "StepStarted" or out[-1][1]["step"] != p["step"]:
                out.pop()
            out.pop()
            continue
        out.append((e.type, p))
    return out


@given(r=roles(), data=st.data())
@settings(max_examples=300, deadline=None, suppress_health_check=[HealthCheck.too_slow])
def test_conductor_invariants(r: Role, data: st.DataObject) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        script = Script(data)
        c = conductor(Path(tmp), script)
        final = drive(c, r, script)
        events = list(c.store.events(run_id="r"))
        steps = {s.name: s for s in r.steps}

        done: set[str] = set()
        started: dict[tuple[str, int], str] = {}  # (step, attempt) -> drawn behaviour of the open turn
        turns: dict[tuple[str, int], int] = {}
        for e in events:
            p = e.payload
            if e.type == "StepStarted":
                # no step starts without the checked, reviewed outputs of the steps it needs
                assert all(n.name in done for n in steps[p["step"]].needs), (p, done)
                started[(p["step"], p["attempt"])] = script.turns[(p["step"], p["round"], p["attempt"])]
                key = (p["step"], p["round"])
                turns[key] = turns.get(key, 0) + 1
            elif e.type == "TurnRejected" and started.get((p["step"], p["attempt"])) in ("stray", "tamper"):
                assert p["reason"] == "write_boundary"
            elif e.type == "Checked":
                if p["source"] == "fake":
                    # no turn is accepted after the agent wrote outside its output
                    assert started[(p["step"], p["attempt"])] == "ok"
                data_bytes = (Path(tmp) / "objects" / f"{p['output_hash']}.json").read_bytes()
                assert hashlib.sha256(data_bytes).hexdigest() == p["output_hash"]
                assert isinstance(not_bad(Note.model_validate(json.loads(data_bytes)), Witnesses({})), Pass)
                if steps[p["step"]].stop is None:
                    done.add(p["step"])
            elif e.type == "StopSkipped" or (e.type == "StopAnswered" and p["answer"] in ("accept", "change")):
                done.add(p["step"])

        for (name, _), n in turns.items():
            assert n <= steps[name].max_turns
        if final == "finished":
            assert done == set(steps)
        else:
            assert final == "blocked"
            assert events[-1].type == "RunBlocked"
        before = len(events)
        assert c.run(r, "t", run_id="r").status == final  # a settled run is stable
        assert len(list(c.store.events(run_id="r"))) == before


@given(r=roles(), data=st.data())
@settings(max_examples=300, deadline=None, suppress_health_check=[HealthCheck.too_slow])
def test_resume_after_a_crash_at_any_write_reproduces_the_events(r: Role, data: st.DataObject) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        script = Script(data)
        clean = conductor(Path(tmp) / "clean", script)
        final = drive(clean, r, script)
        writes = data.draw(st.integers(0, len(list(clean.store.events(run_id="r")))), label="crash after")

        crashed = conductor(Path(tmp) / "crashed", script)
        crash_after(crashed, writes)
        try:
            drive(crashed, r, script)
        except Crash:
            pass
        resumed = conductor(Path(tmp) / "crashed", script)
        assert drive(resumed, r, script) == final
        assert normalized(resumed) == normalized(clean)
