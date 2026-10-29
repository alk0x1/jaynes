"""Turn Claude Code transcript records into human labels.

Claude Code fires no hook when a person answers a permission prompt, but the
session transcript (JSONL) records, next to each tool result, who decided:
``permissionDecision = {decision: accept|reject, source: ...}`` and, for
denials, ``toolDenialKind``. Observed sources include ``user_temporary``,
``user_reject``, ``config`` (rules, mode, auto mode classifier) and ``hook``.

Only a person's answer is a label:

* accepted by ``user_*``                  -> ``correct``
* rejected by the user, plain             -> ``error``
* rejected with a note starting ``ctx``   -> ``context`` (not counted)
* interrupted, classifier, rules, hooks   -> ``none`` (not counted)
"""

from __future__ import annotations

import json
from collections.abc import Iterable, Iterator
from dataclasses import dataclass
from typing import Any, cast

USER_SAID = "the user said:"


@dataclass(frozen=True)
class Label:
    action_id: str
    label: str  # correct | error | context | none
    source: str
    note: str = ""


def _results(record: dict[str, Any]) -> Iterator[tuple[str, str]]:
    msg = record.get("message")
    content = msg.get("content") if isinstance(msg, dict) else None  # pyright: ignore[reportUnknownMemberType, reportUnknownVariableType]
    if not isinstance(content, list):
        return
    for raw in cast(list[Any], content):
        if not isinstance(raw, dict):
            continue
        part = cast(dict[str, Any], raw)
        if part.get("type") == "tool_result":
            body = part.get("content")
            text = body if isinstance(body, str) else json.dumps(body)
            yield str(part.get("tool_use_id", "")), text


def label_of(record: dict[str, Any], text: str) -> tuple[str, str, str]:
    pd = record.get("permissionDecision")
    kind = record.get("toolDenialKind")
    decision = source = ""
    if isinstance(pd, dict):
        decision = str(pd.get("decision", ""))  # pyright: ignore[reportUnknownMemberType, reportUnknownArgumentType]
        source = str(pd.get("source", ""))  # pyright: ignore[reportUnknownMemberType, reportUnknownArgumentType]
        if pd.get("reasonType"):  # pyright: ignore[reportUnknownMemberType]
            source += "/" + str(pd.get("reasonType"))  # pyright: ignore[reportUnknownMemberType, reportUnknownArgumentType]
    if kind == "interrupted":
        return "none", "interrupted", ""
    if decision == "accept" and source.startswith("user"):
        return "correct", source, ""
    # A recorded decision says who decided; "user-rejected" alone is also used for automatic
    # denials in non-interactive runs (source "config"), so it only counts when nothing was recorded.
    if (decision == "reject" and source.startswith("user")) or (not decision and kind == "user-rejected"):
        note = ""
        i = text.find(USER_SAID)
        if i >= 0:
            note = text[i + len(USER_SAID):].strip()
        if note.lower().startswith("ctx"):
            return "context", source or str(kind), note[:500]
        return "error", source or str(kind), note[:500]
    return "none", source or str(kind or "unrecorded"), ""


def scan(lines: Iterable[str], wanted: set[str]) -> list[Label]:
    found: list[Label] = []
    for line in lines:
        if not wanted:
            break
        if '"tool_result"' not in line:
            continue
        try:
            record = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(record, dict):
            continue
        rec: dict[str, Any] = record  # pyright: ignore[reportUnknownVariableType]
        for tool_use_id, text in _results(rec):
            if tool_use_id in wanted:
                label, source, note = label_of(rec, text)
                found.append(Label(tool_use_id, label, source, note))
                wanted.discard(tool_use_id)
    return found


def scan_file(path: str, wanted: set[str], start: int = 0) -> tuple[list[Label], int]:
    """Labels in the complete lines after byte ``start``, and where those lines end, for the next call to
    start from. An action's result comes after it was asked, so a session that reads on from there misses none
    of its later answers. A file shorter than ``start`` was rewritten, and is read from the beginning."""
    try:
        with open(path, "rb") as fh:
            if fh.seek(0, 2) < start:
                start = 0
            fh.seek(start)
            data = fh.read()
    except OSError:
        return [], start
    end = data.rfind(b"\n") + 1  # a line still being written waits for the next call
    lines = data[:end].decode("utf-8", errors="replace").splitlines()
    return scan(lines, set(wanted)), start + end
