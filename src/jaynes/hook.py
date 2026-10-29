"""Claude Code hook entry point: ``jaynes hook`` reads one event on stdin.

* PreToolUse: classify, decide, log, answer. ``defer`` prints nothing, so the
  host's own permission flow (rules, auto mode, prompt) decides as before.
* PostToolUse / PostToolUseFailure: log execution, then collect labels.
* Stop / SessionEnd: collect labels.

Any internal failure prints nothing and exits 0: the host decides as if Jaynes
were not installed. Keep a hard floor hook (such as an existing deny script)
installed alongside while dogfooding.
"""

from __future__ import annotations

import hashlib
import json
import os
import sys
import traceback
from pathlib import Path
from typing import Any, cast

from jaynes import gate, labels
from jaynes.policy import Policy, classify, find_policy, jaynes_home, load_policy
from jaynes.store import Store

INSTRUCTION_FILES = ("CLAUDE.md", "AGENTS.md")
_TAIL_BYTES = 256 * 1024


def db_path() -> Path:
    return jaynes_home() / "jaynes.sqlite3"


def instructions_digest(cwd: str) -> str:
    """Hash of the instruction files that shape the agent: global CLAUDE.md plus those up the tree."""
    h = hashlib.sha256()
    files = [Path.home() / ".claude" / "CLAUDE.md"]
    here = Path(cwd).resolve()
    for d in (here, *here.parents):
        files += [d / f for f in INSTRUCTION_FILES]
    for f in files:
        try:
            h.update(str(f).encode() + b"\0" + f.read_bytes() + b"\0")
        except OSError:
            continue
    return h.hexdigest()[:16]


def transcript_model(path: str) -> str:
    """Model of the most recent assistant message in the transcript tail."""
    try:
        with open(path, "rb") as fh:
            fh.seek(0, os.SEEK_END)
            size = fh.tell()
            fh.seek(max(0, size - _TAIL_BYTES))
            tail = fh.read().decode("utf-8", errors="replace").splitlines()
    except OSError:
        return "?"
    for line in reversed(tail):
        if '"model"' not in line or '"assistant"' not in line:
            continue
        try:
            rec = json.loads(line)
        except json.JSONDecodeError:
            continue
        msg = rec.get("message") if isinstance(rec, dict) else None  # pyright: ignore[reportUnknownMemberType, reportUnknownVariableType]
        model = msg.get("model") if isinstance(msg, dict) else None  # pyright: ignore[reportUnknownMemberType, reportUnknownVariableType]
        if isinstance(model, str) and model and not model.startswith("<"):
            return model
    return "?"


def summarize(tool: str, tool_input: dict[str, Any]) -> str:
    for key in ("command", "file_path", "notebook_path", "path", "pattern", "url", "query"):
        v = tool_input.get(key)
        if isinstance(v, str):
            return v[:300]
    return json.dumps(tool_input, sort_keys=True)[:300]


def pre_tool_use(event: dict[str, Any], policy: Policy, store: Store) -> dict[str, Any] | None:
    tool = str(event.get("tool_name", ""))
    tool_input: dict[str, Any] = event.get("tool_input") or {}
    cwd = str(event.get("cwd") or os.getcwd())
    session = str(event.get("session_id", ""))
    action_id = str(event.get("tool_use_id") or "")
    transcript = str(event.get("transcript_path") or "")
    if not action_id:
        return None
    collect(store, session)

    category = classify(policy, tool, tool_input, cwd)
    model = transcript_model(transcript) if transcript else "?"
    if model == "?":
        model = store.last_model(session) or "?"
    fingerprint = policy.fingerprint(category, extra=f"{model}|{instructions_digest(cwd)}")
    window = store.window(fingerprint)
    d = gate.decide(category, window, policy, gate.coin(policy, action_id))

    store.append("ActionProposed", {
        "tool": tool, "category": category.name, "effect": category.effect.name.lower(),
        "summary": summarize(tool, tool_input), "cwd": cwd, "transcript": transcript,
        "agent": event.get("agent_type"), "model": model,
    }, action_id=action_id, session=session)
    store.append("GateDecided", {
        "category": category.name, "fingerprint": fingerprint, "verdict": d.verdict, "status": d.status,
        "audit": d.audit, "n": d.n, "k": d.k, "log_e": round(d.log_e_certify, 6), "reason": d.reason,
        "policy": policy.source,
    }, action_id=action_id, session=session)

    if d.verdict == "defer":
        return None
    reason = gate.prompt_text(category, d, policy) if d.verdict == "ask" else f"Jaynes: {category.name}, {d.reason}."
    return {"hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": d.verdict,
        "permissionDecisionReason": reason,
    }}


def collect(store: Store, session: str) -> int:
    """Read labels for this session's asked actions from the transcript."""
    pending = store.pending_audits(session)
    if not pending:
        return 0
    by_file: dict[str, set[str]] = {}
    for action_id, transcript in pending:
        if transcript:
            by_file.setdefault(transcript, set()).add(action_id)
    count = 0
    for path, wanted in by_file.items():
        found, end = labels.scan_file(path, wanted, store.read_to(session, path))
        for lab in found:
            store.append("HumanLabeled", {"label": lab.label, "source": lab.source, "note": lab.note},
                         action_id=lab.action_id, session=session)
            count += 1
        store.set_read_to(session, path, end)  # after the labels, so a crash rereads rather than skips
    return count


def handle(event: dict[str, Any]) -> dict[str, Any] | None:
    name = event.get("hook_event_name")
    cwd = str(event.get("cwd") or os.getcwd())
    path = find_policy(cwd)
    if path is None:
        return None
    policy = load_policy(path)
    store = Store(db_path())
    try:
        session = str(event.get("session_id", ""))
        if name == "PreToolUse":
            return pre_tool_use(event, policy, store)
        if name in ("PostToolUse", "PostToolUseFailure"):
            aid = event.get("tool_use_id")
            if aid:
                store.append("Executed", {"ok": name == "PostToolUse"}, action_id=str(aid), session=session)
            collect(store, session)
        elif name in ("Stop", "SubagentStop", "SessionEnd"):
            collect(store, session)
        elif name in ("SessionStart", "PostModelSwitch"):
            model = event.get("model") or event.get("to_model")
            if isinstance(model, str) and model:
                store.append("ModelSeen", {"model": model}, session=session)
        return None
    finally:
        store.close()


def main(stdin: Any = None, stdout: Any = None) -> int:
    stdin = stdin or sys.stdin
    stdout = stdout or sys.stdout
    try:
        event = json.load(stdin)
        out = handle(cast(dict[str, Any], event)) if isinstance(event, dict) else None
        if out is not None:
            json.dump(out, stdout)
    except Exception:  # noqa: BLE001 - never break the host session
        try:
            log = jaynes_home() / "errors.log"
            log.parent.mkdir(parents=True, exist_ok=True)
            with open(log, "a", encoding="utf-8") as fh:
                fh.write(traceback.format_exc() + "\n")
        except OSError:
            pass
    return 0
