"""The gate: a pure function from (category, window labels, policy, coin) to a verdict.

Category status, from the labels in its current window:

* ``floor``        configured never to run without a human (``floor = "ask"``) or never at all (``"deny"``)
* ``pure``         effect is pure: runs, no audit
* ``unknown``      no category matched: the host's own permission flow decides, no labels
* ``certified``    anytime-valid test rejects "error rate >= budget"
* ``refuted``      anytime-valid test rejects "error rate <= budget"
* ``undetermined`` neither yet

Verdicts map to Claude Code's PreToolUse decisions; ``defer`` means "return no
decision", leaving the call to the host (auto mode classifier, rules, prompt).
"""

from __future__ import annotations

import hashlib
import math
from dataclasses import dataclass
from typing import Literal

from jaynes import bounds
from jaynes.policy import UNKNOWN, Category, Effect, Policy

Status = Literal["floor", "pure", "unknown", "certified", "refuted", "undetermined"]
Verdict = Literal["allow", "ask", "deny", "defer"]


@dataclass(frozen=True)
class Window:
    n: int = 0  # human labels counted (correct + error)
    k: int = 0  # errors among them


@dataclass(frozen=True)
class Decision:
    verdict: Verdict
    status: Status
    audit: bool
    n: int
    k: int
    log_e_certify: float
    reason: str


def status_of(category: Category, window: Window, policy: Policy) -> Status:
    if category.floor:
        return "floor"
    if category is UNKNOWN or category.name == "unknown":
        return "unknown"
    if category.effect == Effect.PURE:
        return "pure"
    if bounds.certified(window.n, window.k, policy.budget, policy.delta):
        return "certified"
    if bounds.refuted(window.n, window.k, policy.budget, policy.delta):
        return "refuted"
    return "undetermined"


def coin(policy: Policy, action_id: str) -> float:
    """Deterministic uniform draw per action, so decisions replay exactly."""
    h = hashlib.sha256(f"{policy.seed}:{action_id}".encode()).digest()
    return int.from_bytes(h[:8], "big") / 2**64


def decide(category: Category, window: Window, policy: Policy, u: float) -> Decision:
    status = status_of(category, window, policy)
    log_e = bounds.log_certify_evalue(window.n, window.k, policy.budget) if window.n else 0.0
    audit = False
    if status == "floor":
        verdict: Verdict = "deny" if category.floor == "deny" else "ask"
        reason = f"floor {category.floor}"
    elif status == "pure":
        verdict, reason = "allow", "pure effect"
    elif status == "unknown":
        verdict, reason = "defer", "no category matched"
    elif status == "refuted":
        verdict, reason = "ask", "error rate shown above budget"
    else:
        audit = u < policy.audit_rate
        if audit:
            verdict, reason = "ask", "random audit"
        elif status == "certified":
            verdict, reason = "allow", "certified within budget"
        else:
            verdict = policy.undetermined
            reason = "not enough labels yet"
    return Decision(verdict, status, audit, window.n, window.k, log_e, reason)


def prompt_text(category: Category, d: Decision, policy: Policy) -> str:
    """What the person sees in the permission prompt when the gate asks."""
    if d.status == "floor":
        head = f"Jaynes: {category.name} always asks."
    elif d.audit:
        head = f"Jaynes audit, {category.name}: {d.k} errors in {d.n} labels."
    else:
        head = f"Jaynes: {category.name} needs review ({d.k} errors in {d.n} labels, budget {policy.budget:.0%})."
    return head + " Deny = the agent got this wrong. To deny without blaming the agent, start your note with 'ctx'."


def labels_needed(window: Window, policy: Policy) -> int | None:
    """Correct labels still needed, with no new error, to certify."""
    n = bounds.labels_to_certify(window.k, policy.budget, policy.delta)
    return None if n is None else max(0, n - window.n)


def evidence(window: Window, policy: Policy) -> float:
    """Certification evidence as a fraction of the threshold, on a log scale (1.0 = certified)."""
    if window.n == 0:
        return 0.0
    return bounds.log_certify_evalue(window.n, window.k, policy.budget) / math.log(1 / policy.delta)
