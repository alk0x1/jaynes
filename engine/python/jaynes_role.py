"""Write a Jaynes role in Python: the small library on the role's side of the protocol (version 1).

A role is a program the engine starts once per call: it reads one request from standard input and writes one
reply to standard output, both a line of JSON. This module does that part. The role only declares its steps and
writes the functions the engine calls::

    from jaynes_role import Role, Stop, Pass, Fail

    role = Role("small-change", version="1.0.0")
    role.step("change", executor="agent", reach={"project": "write", "write": ["**"], "commands": True},
              instructions="Make the change.", schema={"type": "object", "required": ["summary"]})

    @role.check("change", "suite", kind="open", about="the project's tests pass")
    def suite(call):
        return Pass()

    if __name__ == "__main__":
        role.serve()

The engine owns the order of the steps, and writes what each answer of the person will do. A role never decides
what runs next. Standard library only.
"""

from __future__ import annotations

import json
import sys
import traceback
from dataclasses import dataclass, field
from typing import Any, Callable

PROTOCOL = 1
VERDICTS = ("keep", "change", "redo", "drop", "skip")


@dataclass
class Pass:
    pass


@dataclass
class Fail:
    """A failed check. ``reasons`` is what the agent is told; ``detail`` only the person sees."""

    reasons: list[str]
    detail: str = ""

    def __init__(self, *reasons: str, detail: str = "") -> None:
        self.reasons = list(reasons)
        self.detail = detail


@dataclass
class Blocked:
    """The check could not judge at all, such as a tool it runs that did not start: the engine's fault, never the
    agent's, and no attempt is spent."""

    reason: str


@dataclass
class NeedsInput:
    """A role's own step asks the person before it can give its output. The engine asks, then calls the step again
    with the answer in ``call["answers"]``."""

    items: list[dict[str, Any]]
    offered: list[str]
    about: str = ""


@dataclass
class Refused:
    """The step cannot be done, for a reason of the role's."""

    reason: str


@dataclass
class Stop:
    verdicts: list[str] = field(default_factory=lambda: ["keep", "redo"])
    holds: bool = False
    from_risk: str = "low"
    # The stop only asks whether a result may go on, not a matter of taste or direction: an agent may let it go on.
    release: bool = False
    # The error rate the stop may have once it asks only a sample; None: it always asks.
    budget: float | None = None
    audit_rate: float = 0.1
    about: str = ""


@dataclass
class _Step:
    name: str
    executor: str
    needs: list[str]
    reach: dict[str, Any]
    effect: dict[str, Any]
    stop: Stop | None
    from_risk: str
    attempts: int
    fixes: int
    hidden: bool
    session: str
    about: str
    instructions: str
    schema: dict[str, Any]
    run: Callable[[dict[str, Any]], Any] | None = None
    items: Callable[[Any], list[dict[str, Any]]] | None = None
    checks: dict[str, tuple[str, str, list[str], Callable[[dict[str, Any]], Any]]] = field(default_factory=dict)


class Role:
    def __init__(self, name: str, version: str, delivery: str = "branch") -> None:
        self.name = name
        self.version = version
        self.delivery = delivery  # "branch", or "merge": Jaynes merges into the branch the run started from
        self.steps: dict[str, _Step] = {}

    def step(self, name: str, *, executor: str = "agent", needs: list[str] | None = None,
             reach: dict[str, Any] | None = None, effect: dict[str, Any] | None = None, stop: Stop | None = None,
             from_risk: str = "low", attempts: int = 3, fixes: int = 2, hidden: bool = False, session: str = "fresh",
             about: str = "", instructions: str = "", schema: dict[str, Any] | None = None,
             run: Callable[[dict[str, Any]], Any] | None = None) -> str:
        """Declare a step, in order. ``run`` does a step whose executor is the role."""
        if name in self.steps:
            raise ValueError(f"step {name} is declared twice")
        self.steps[name] = _Step(name, executor, list(needs or []), dict(reach or {}), dict(effect or {"kind": "none"}),
                                 stop, from_risk, attempts, fixes, hidden, session, about, instructions,
                                 dict(schema or {"type": "object"}), run)
        return name

    def check(self, step: str, name: str, *, kind: str = "open", about: str = "",
              reads: list[str] | None = None) -> Callable[[Callable[[dict[str, Any]], Any]], Callable[[dict[str, Any]], Any]]:
        """A check of ``step``'s output. ``open``: the agent knows it and fixes a failure in place. ``hidden``: the
        maker never sees it, and is told only the sentences of a failure. ``reads`` names earlier steps the check
        reads besides what its step uses, such as hidden cases; they come in ``call["given"]``."""
        def add(fn: Callable[[dict[str, Any]], Any]) -> Callable[[dict[str, Any]], Any]:
            self.steps[step].checks[name] = (kind, about, list(reads or []), fn)
            return fn
        return add

    def items(self, step: str) -> Callable[[Callable[[Any], list[dict[str, Any]]]], Callable[[Any], list[dict[str, Any]]]]:
        """What the person is shown of ``step``'s output: ``[{"id", "label", "content"}]``. Without it the whole
        output is one item."""
        def add(fn: Callable[[Any], list[dict[str, Any]]]) -> Callable[[Any], list[dict[str, Any]]]:
            self.steps[step].items = fn
            return fn
        return add

    def manifest(self) -> dict[str, Any]:
        steps = []
        for s in self.steps.values():
            d: dict[str, Any] = {"name": s.name, "needs": s.needs, "executor": s.executor, "session": s.session,
                                 "reach": s.reach, "effect": s.effect, "from_risk": s.from_risk,
                                 "attempts": s.attempts, "fixes": s.fixes, "hidden": s.hidden, "about": s.about,
                                 "checks": [{"name": n, "kind": k, "about": a, "reads": r} for n, (k, a, r, _) in s.checks.items()]}
            if s.stop is not None:
                d["stop"] = {"verdicts": s.stop.verdicts, "holds": s.stop.holds, "from_risk": s.stop.from_risk,
                             "release": s.stop.release, "budget": s.stop.budget,
                             "audit_rate": s.stop.audit_rate, "about": s.stop.about}
            steps.append(d)
        return {"role": self.name, "version": self.version, "protocol": PROTOCOL, "steps": steps, "delivery": self.delivery}

    # --- the calls ----------------------------------------------------------------------------------------------

    def handle(self, request: dict[str, Any]) -> dict[str, Any]:
        call = request.get("call")
        if call == "hello":
            return {"reply": "hello", "role": self.name, "lifetime": "call", "features": [], "required": []}
        if call == "describe":
            return {"reply": "manifest", "manifest": self.manifest()}
        step = self.steps.get(str(request.get("step")))
        if step is None:
            return {"reply": "error", "message": f"the role has no step {request.get('step')}"}
        if call == "prompt":
            return {"reply": "prompt", "text": step.instructions, "schema": step.schema}
        if call == "execute":
            if step.run is None:
                return {"reply": "error", "message": f"{step.name} is not the role's own step"}
            return _result(step.run(request))
        if call == "check":
            found = step.checks.get(str(request.get("check")))
            if found is None:
                return {"reply": "error", "message": f"{step.name} has no check {request.get('check')}"}
            return {"reply": "verdict", "verdict": _verdict(found[3](request))}
        if call == "items":
            output = request.get("output")
            listed = step.items(output) if step.items else [{"id": step.name, "label": f"what {step.name} made",
                                                             "content": output}]
            return {"reply": "items", "items": listed}
        return {"reply": "error", "message": f"unknown call {call}"}

    def serve(self) -> None:
        """Answer the one request on standard input."""
        line = sys.stdin.readline()
        request: dict[str, Any] = {}
        try:
            request = json.loads(line)
            body = self.handle(request)
        except Exception:  # the engine counts a role that cannot answer as its own fault, never the agent's
            body = {"reply": "error", "message": traceback.format_exc(limit=5)[-2000:]}
        reply = {"jaynes": PROTOCOL, "id": request.get("id", ""), **body}
        sys.stdout.write(json.dumps(reply) + "\n")
        sys.stdout.flush()


def _verdict(v: Any) -> dict[str, Any]:
    if isinstance(v, Pass):
        return {"verdict": "pass"}
    if isinstance(v, Fail):
        return {"verdict": "fail", "reasons": v.reasons, "detail": v.detail}
    if isinstance(v, Blocked):
        return {"verdict": "blocked", "reason": v.reason}
    return {"verdict": "blocked", "reason": f"the check returned {type(v).__name__}, not Pass, Fail or Blocked"}


def _result(r: Any) -> dict[str, Any]:
    if isinstance(r, NeedsInput):
        return {"reply": "needs_input", "items": r.items, "offered": r.offered, "about": r.about}
    if isinstance(r, Refused):
        return {"reply": "refused", "reason": r.reason}
    return {"reply": "result", "output": r}
