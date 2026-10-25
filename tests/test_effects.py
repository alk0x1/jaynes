"""Trusted steps declare their effect; an irreversible one runs once, on the payload a person approved, with a key."""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest
from pydantic import BaseModel

from jaynes.conductor import AnswerRejected, Conductor
from jaynes.role import StepContext, role, stop
from tests.fakes import Scripted


class Payment(BaseModel):
    to: str
    cents: int


class Receipt(BaseModel):
    key: str


class Crash(BaseException):
    """Leaves the process the way a kill does: the step started and nothing came back."""


def paying(calls: list[tuple[Payment, str]], crash: bool = False) -> tuple[Any, Any, Any]:
    r = role("pay", agent="fake")
    plan = r.step("plan", output=Payment, stop=stop(0.1, sampled=False))

    def pay(ctx: StepContext) -> Receipt:
        calls.append((ctx.needs[plan].value, ctx.idempotency_key))
        if crash:
            raise Crash()
        return Receipt(key=ctx.idempotency_key)

    send = r.step("send", output=Receipt, needs=[plan], run_by=pay, effect="irreversible")
    return r, plan, send


def payloads(c: Conductor, run_id: str, kind: str) -> list[dict[str, Any]]:
    return [e.payload for e in c.store.events(kind, run_id=run_id)]


def test_an_irreversible_step_runs_the_payload_the_person_approved_with_a_derived_key(tmp_path: Path) -> None:
    calls: list[tuple[Payment, str]] = []
    r, _, send = paying(calls)
    c = Conductor(tmp_path, runtimes={"fake": lambda: Scripted(default=lambda s: {"to": "acme", "cents": 900})})
    assert c.run(r, "pay acme", run_id="r1").status == "waiting"
    assert calls == []
    with pytest.raises(AnswerRejected, match="only a person"):
        c.answer(r, "r1", "plan", "accept", by="agent")
    c.answer(r, "r1", "plan", "change", output={"to": "acme", "cents": 90}, by="person")

    assert c.run(r, "pay acme", run_id="r1").status == "finished"

    [(paid, key)] = calls
    assert paid == Payment(to="acme", cents=90) and len(key) == 32
    started = [p for p in payloads(c, "r1", "StepStarted") if p["step"] == "send"][0]
    assert started["effect"] == "irreversible" and started["key"] == key and len(started["payload"]) == 1
    assert c.output("r1", send) == Receipt(key=key)


def test_each_run_and_each_payload_get_their_own_key(tmp_path: Path) -> None:
    keys: dict[str, list[str]] = {}
    for run, cents in (("r1", 90), ("r2", 90), ("r3", 91)):
        calls: list[tuple[Payment, str]] = []
        r, _, _ = paying(calls)
        c = Conductor(tmp_path, runtimes={"fake": lambda: Scripted(default=lambda s: {"to": "acme", "cents": cents})})
        c.run(r, "t", run_id=run)
        c.answer(r, run, "plan", "accept", by="person")
        c.run(r, "t", run_id=run)
        keys[run] = [k for _, k in calls]
    assert keys["r1"] != keys["r2"]  # each run is its own effect
    assert keys["r1"] != keys["r3"]


def test_an_irreversible_step_that_was_running_at_a_crash_blocks_instead_of_running_again(tmp_path: Path) -> None:
    calls: list[tuple[Payment, str]] = []
    r, _, _ = paying(calls, crash=True)
    c = Conductor(tmp_path, runtimes={"fake": lambda: Scripted(default=lambda s: {"to": "acme", "cents": 90})})
    c.run(r, "t", run_id="r1")
    c.answer(r, "r1", "plan", "accept", by="person")
    with pytest.raises(Crash):
        c.run(r, "t", run_id="r1")

    result = c.run(r, "t", run_id="r1")

    assert result.status == "blocked" and result.step == "send"
    [(_, key)] = calls
    assert "may have acted" in result.reasons[0] and key in result.reasons[0]


def test_an_irreversible_step_that_fails_is_not_retried(tmp_path: Path) -> None:
    r = role("pay", agent="fake")
    plan = r.step("plan", output=Payment, stop=stop(0.1, sampled=False))
    tries: list[str] = []

    def pay(ctx: StepContext) -> Receipt:
        tries.append(ctx.idempotency_key)
        raise TimeoutError("the bank did not answer")

    r.step("send", output=Receipt, needs=[plan], run_by=pay, effect="irreversible")
    c = Conductor(tmp_path, runtimes={"fake": lambda: Scripted(default=lambda s: {"to": "acme", "cents": 90})})
    c.run(r, "t", run_id="r1")
    c.answer(r, "r1", "plan", "accept", by="person")
    result = c.run(r, "t", run_id="r1")
    assert result.status == "blocked" and len(tries) == 1 and any(tries[0] in x for x in result.reasons)


def test_effects_that_cannot_be_kept_are_refused_when_the_role_is_written() -> None:
    def pay(ctx: StepContext) -> Receipt:
        return Receipt(key="")

    r = role("pay", agent="fake")
    sampled = r.step("sampled", output=Payment, stop=stop(0.1))
    floor = r.step("floor", output=Payment, stop=stop(0.1, sampled=False))
    with pytest.raises(ValueError, match="floor stop"):
        r.step("a", output=Receipt, run_by=pay, effect="irreversible")
    with pytest.raises(ValueError, match="floor stop"):
        r.step("b", output=Receipt, needs=[sampled], run_by=pay, effect="irreversible")
    with pytest.raises(ValueError, match="never sampled"):
        r.step("c", output=Receipt, needs=[floor], run_by=pay, effect="irreversible", stop=stop(0.1))
    with pytest.raises(ValueError, match="trusted"):
        r.step("d", output=Receipt, needs=[floor], effect="irreversible")
    with pytest.raises(ValueError, match="effect"):
        r.step("e", output=Receipt, run_by=pay, effect="explosive")  # type: ignore[arg-type]
    with pytest.raises(ValueError, match="every step it needs"):
        r.step("h", output=Receipt, needs=[floor, sampled], run_by=pay, effect="irreversible")
    ok = r.step("f", output=Receipt, needs=[floor], run_by=pay, effect="irreversible")
    assert ok.max_turns == 1 and ok.effect == "irreversible"
    assert r.step("g", output=Receipt, run_by=pay).effect == "pure"


def test_declaring_an_effect_other_than_pure_changes_the_fingerprint_and_pure_keeps_it() -> None:
    def fetch(ctx: StepContext) -> Receipt:
        return Receipt(key="")

    fps: list[str] = []
    for effect in ("pure", "web"):
        r = role("f", agent="fake")
        r.step("a", output=Receipt, run_by=fetch, effect=effect)  # type: ignore[arg-type]
        fps.append(r.fingerprint())
    plain = role("f", agent="fake")
    plain.step("a", output=Receipt, run_by=fetch)
    assert fps[0] == plain.fingerprint() != fps[1]
