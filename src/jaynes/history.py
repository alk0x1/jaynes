"""Read past Claude Code tool calls from transcripts, for calibration of the policy file.

Each call is joined with the permission decision recorded next to its result,
so the report can say who approved what before Jaynes existed.
"""

from __future__ import annotations

import collections
import json
import os
from collections.abc import Iterator
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

from jaynes import bounds, labels
from jaynes.policy import Policy, classify


@dataclass(frozen=True)
class PastCall:
    action_id: str
    session: str
    ts: datetime
    cwd: str
    tool: str
    tool_input: dict[str, Any]
    decided_by: str  # e.g. accept:config/classifier, accept:user_temporary, reject:hook/hook, unrecorded
    label: str       # what labels.label_of would say: correct | error | context | none


def transcripts(root: Path, days: int) -> Iterator[Path]:
    cutoff = datetime.now(timezone.utc).timestamp() - days * 86400
    for d in sorted(root.iterdir()) if root.is_dir() else []:
        if not d.is_dir():
            continue
        for f in d.glob("*.jsonl"):
            try:
                if f.stat().st_mtime >= cutoff:
                    yield f
            except OSError:
                continue


def _parse_ts(raw: Any) -> datetime | None:
    if not isinstance(raw, str):
        return None
    try:
        return datetime.fromisoformat(raw.replace("Z", "+00:00"))
    except ValueError:
        return None


def calls_in(path: Path) -> list[PastCall]:
    uses: dict[str, tuple[str, datetime, str, str, dict[str, Any]]] = {}
    results: dict[str, tuple[str, str]] = {}
    try:
        fh = open(path, encoding="utf-8", errors="replace")
    except OSError:
        return []
    with fh:
        for line in fh:
            if '"tool_use"' not in line and '"tool_result"' not in line:
                continue
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(rec, dict):
                continue
            r: dict[str, Any] = rec  # pyright: ignore[reportUnknownVariableType]
            msg = r.get("message")
            content = msg.get("content") if isinstance(msg, dict) else None  # pyright: ignore[reportUnknownMemberType, reportUnknownVariableType]
            if not isinstance(content, list):
                continue
            ts = _parse_ts(r.get("timestamp"))
            for part in content:  # pyright: ignore[reportUnknownVariableType]
                if not isinstance(part, dict):
                    continue
                p: dict[str, Any] = part  # pyright: ignore[reportUnknownVariableType]
                if p.get("type") == "tool_use" and ts is not None:
                    inp = p.get("input")
                    uses[str(p.get("id"))] = (str(r.get("sessionId", "")), ts, str(r.get("cwd", "")),
                                              str(p.get("name", "")), inp if isinstance(inp, dict) else {})  # pyright: ignore[reportUnknownArgumentType]
                elif p.get("type") == "tool_result":
                    body = p.get("content")
                    text = body if isinstance(body, str) else json.dumps(body)
                    label, source, _ = labels.label_of(r, text)
                    pd = r.get("permissionDecision")
                    decision = str(pd.get("decision", "")) if isinstance(pd, dict) else ""  # pyright: ignore[reportUnknownMemberType, reportUnknownArgumentType]
                    who = f"{decision}:{source}" if decision else (f"denied:{r.get('toolDenialKind')}" if r.get("toolDenialKind") else "unrecorded")
                    results[str(p.get("tool_use_id", ""))] = (who, label)
    out: list[PastCall] = []
    for aid, (session, ts, cwd, tool, inp) in uses.items():
        who, label = results.get(aid, ("no-result", "none"))
        out.append(PastCall(aid, session, ts, cwd, tool, inp, who, label))
    return out


def past_calls(days: int = 30, root: Path | None = None) -> list[PastCall]:
    root = root or Path(os.path.expanduser("~/.claude/projects"))
    cutoff = datetime.now(timezone.utc) - timedelta(days=days)
    calls = [c for f in transcripts(root, days) for c in calls_in(f)]
    seen: set[str] = set()
    unique: list[PastCall] = []
    for c in sorted(calls, key=lambda c: c.ts):
        if c.ts >= cutoff and c.action_id not in seen:
            seen.add(c.action_id)
            unique.append(c)
    return unique


@dataclass(frozen=True)
class Row:
    category: str
    kind: str  # floor | pure | learns | unknown
    calls: int
    per_week: float
    audits_per_week: float
    weeks_to_certify: float | None  # assuming zero errors
    approvers: list[tuple[str, int]]  # who approved these calls before Jaynes, top three


@dataclass(frozen=True)
class Report:
    calls: int
    weeks: float
    answered: int  # calls a person answered
    rows: list[Row]
    audits_per_week: float
    labels_to_certify: int
    unknown: list[tuple[str, int]]  # the commonest calls no category matched


def report(policy: Policy, calls: list[PastCall], top: int = 25) -> Report | None:
    """Classify past calls by the policy: what each category would cost in audits and how long it takes to certify."""
    if not calls:
        return None
    weeks = max((calls[-1].ts - calls[0].ts).total_seconds() / (7 * 86400), 1 / 7)
    per_cat: collections.Counter[str] = collections.Counter()
    who: dict[str, collections.Counter[str]] = collections.defaultdict(collections.Counter)
    unknown: collections.Counter[str] = collections.Counter()
    for c in calls:
        cat = classify(policy, c.tool, c.tool_input, c.cwd or "/")
        per_cat[cat.name] += 1
        who[cat.name][c.decided_by.split("/")[0]] += 1
        if cat.name == "unknown":
            key = c.tool if c.tool != "Bash" else "Bash: " + str(c.tool_input.get("command", ""))[:70].replace("\n", " ")
            unknown[key] += 1
    need0 = bounds.labels_to_certify(0, policy.budget, policy.delta) or 0
    rows: list[Row] = []
    for name, count in per_cat.most_common():
        c = policy.category(name)
        kind = "floor" if c.floor else ("pure" if c.effect == 0 else ("unknown" if name == "unknown" else "learns"))
        rate = count / weeks
        aud = rate * policy.audit_rate if kind == "learns" else 0.0
        rows.append(Row(name, kind, count, rate, aud, need0 / aud if aud else None, who[name].most_common(3)))
    answered = sum(1 for c in calls if c.label in ("correct", "error", "context"))
    return Report(len(calls), weeks, answered, rows, sum(r.audits_per_week for r in rows), need0,
                  unknown.most_common(top))
