"""Read past Claude Code tool calls from transcripts, for calibration of the policy file.

Each call is joined with the permission decision recorded next to its result,
so the report can say who approved what before Jaynes existed.
"""

from __future__ import annotations

import json
import os
from collections.abc import Iterator
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

from jaynes import labels


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
