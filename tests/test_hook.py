"""The hook end to end: real stdin/stdout, real SQLite, a transcript written like Claude Code writes it."""

import io
import json
from collections.abc import Mapping
from pathlib import Path

import pytest

from jaynes import gate, hook
from jaynes.policy import load_policy
from jaynes.store import Store

ROOT = Path(__file__).resolve().parents[1]
PUSH = "git " + "push"


@pytest.fixture
def env(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    project = tmp_path / "helm"
    project.mkdir()
    (project / "jaynes.toml").write_text((ROOT / "examples" / "agent-team.jaynes.toml").read_text())
    monkeypatch.setenv("JAYNES_HOME", str(tmp_path / "home"))
    monkeypatch.setenv("HOME", str(tmp_path / "userhome"))
    return project


def run(event: dict[str, object]) -> dict[str, object] | None:
    out = io.StringIO()
    assert hook.main(io.StringIO(json.dumps(event)), out) == 0
    return json.loads(out.getvalue()) if out.getvalue() else None


def pre(project: Path, transcript: Path, tool_use_id: str, tool: str, tool_input: Mapping[str, object]) -> dict[str, object] | None:
    return run({"hook_event_name": "PreToolUse", "session_id": "s1", "cwd": str(project),
                "transcript_path": str(transcript), "tool_name": tool, "tool_input": dict(tool_input),
                "tool_use_id": tool_use_id})


def decision(out: dict[str, object] | None) -> str | None:
    return None if out is None else out["hookSpecificOutput"]["permissionDecision"]  # type: ignore[index]


def transcript_result(path: Path, tool_use_id: str, *, accept: bool, note: str = "") -> None:
    if accept:
        rec = {"type": "user", "permissionDecision": {"decision": "accept", "source": "user_temporary"},
               "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": tool_use_id, "content": "ok"}]}}
    else:
        text = "The user doesn't want to proceed with this tool use. The tool use was rejected."
        if note:
            text += f" To tell you how to proceed, the user said:\n{note}"
        rec = {"type": "user", "toolDenialKind": "user-rejected",
               "permissionDecision": {"decision": "reject", "source": "user_reject"},
               "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": tool_use_id,
                                                        "content": text, "is_error": True}]}}
    with open(path, "a") as fh:
        fh.write(json.dumps(rec) + "\n")


def audited_ids(prefix: str, policy_path: Path, want: bool, count: int) -> list[str]:
    pol = load_policy(policy_path)
    ids = (f"{prefix}{i}" for i in range(100000))
    return [i for i in ids if (gate.coin(pol, i) < pol.audit_rate) == want][:count]


def test_day_one_verdicts(env: Path, tmp_path: Path) -> None:
    t = tmp_path / "t.jsonl"
    t.write_text("")
    assert decision(pre(env, t, "x1", "Bash", {"command": f"{PUSH} origin main"})) == "deny"
    assert decision(pre(env, t, "x2", "Read", {"file_path": "README.md"})) == "allow"
    assert decision(pre(env, t, "x3", "Bash", {"command": "cat > notes.md <<'EOF'\n" + PUSH + " is prose\nEOF"})) != "deny"
    [quiet] = audited_ids("q", env / "jaynes.toml", False, 1)
    assert pre(env, t, quiet, "Edit", {"file_path": str(env / "work" / "a.md")}) is None  # host decides
    [loud] = audited_ids("l", env / "jaynes.toml", True, 1)
    out = pre(env, t, loud, "Edit", {"file_path": str(env / "work" / "a.md")})
    assert decision(out) == "ask"
    assert "ctx" in out["hookSpecificOutput"]["permissionDecisionReason"]  # type: ignore[index]


def test_labels_flow_from_the_transcript_into_the_window(env: Path, tmp_path: Path) -> None:
    t = tmp_path / "t.jsonl"
    t.write_text("")
    asked = audited_ids("e", env / "jaynes.toml", True, 3)
    for aid in asked:
        assert decision(pre(env, t, aid, "Edit", {"file_path": str(env / "work" / "a.md")})) == "ask"
    transcript_result(t, asked[0], accept=True)
    transcript_result(t, asked[1], accept=False)
    transcript_result(t, asked[2], accept=False, note="ctx changed my mind")
    run({"hook_event_name": "Stop", "session_id": "s1", "cwd": str(env)})
    store = Store(Path(hook.db_path()))
    got = {e.action_id: e.payload["label"] for e in store.events("HumanLabeled")}
    assert got == {asked[0]: "correct", asked[1]: "error", asked[2]: "context"}
    fp = next(e.payload["fingerprint"] for e in store.events("GateDecided") if e.action_id == asked[0])
    assert store.window(str(fp)) == gate.Window(2, 1)


def test_category_earns_autonomy_after_enough_correct_audits(env: Path, tmp_path: Path) -> None:
    t = tmp_path / "t.jsonl"
    t.write_text("")
    edit = {"file_path": str(env / "work" / "plan.md")}
    for aid in audited_ids("c", env / "jaynes.toml", True, 88):
        assert decision(pre(env, t, aid, "Edit", edit)) == "ask"
        transcript_result(t, aid, accept=True)
        run({"hook_event_name": "PostToolUse", "session_id": "s1", "cwd": str(env), "tool_use_id": aid})
    [quiet] = audited_ids("z", env / "jaynes.toml", False, 1)
    assert decision(pre(env, t, quiet, "Edit", edit)) == "allow"
    # an error takes it back to the host
    [loud] = audited_ids("w", env / "jaynes.toml", True, 1)
    pre(env, t, loud, "Edit", edit)
    transcript_result(t, loud, accept=False)
    run({"hook_event_name": "Stop", "session_id": "s1", "cwd": str(env)})
    [quiet2] = audited_ids("y", env / "jaynes.toml", False, 1)
    assert pre(env, t, quiet2, "Edit", edit) is None


def test_internal_failure_never_breaks_the_session(env: Path, tmp_path: Path) -> None:
    (env / "jaynes.toml").write_text("this is not toml [[[")
    assert pre(env, tmp_path / "t.jsonl", "f1", "Bash", {"command": "ls"}) is None
    assert (Path(hook.db_path()).parent / "errors.log").read_text()


def test_headless_auto_denial_is_not_a_human_label() -> None:
    # Exact shape recorded by Claude Code 2.1.306 when a hook asks in a -p run and nobody can answer.
    from jaynes.labels import label_of
    rec = {"toolDenialKind": "user-rejected", "permissionDecision": {"decision": "reject", "source": "config"}}
    assert label_of(rec, "Jaynes audit, edit_docs ...")[0] == "none"
    real_user = {"toolDenialKind": "user-rejected", "permissionDecision": {"decision": "reject", "source": "user_reject"}}
    assert label_of(real_user, "The user doesn't want to proceed")[0] == "error"
    hook_allow = {"permissionDecision": {"decision": "accept", "source": "hook", "reasonType": "hook"}}
    assert label_of(hook_allow, "ok")[0] == "none"


def test_first_call_of_a_session_reuses_the_session_model(env: Path, tmp_path: Path) -> None:
    t = tmp_path / "t.jsonl"
    t.write_text("")
    run({"hook_event_name": "SessionStart", "session_id": "s1", "cwd": str(env), "model": "claude-opus-5-5"})
    pre(env, t, "m1", "Bash", {"command": "ls"})
    store = Store(Path(hook.db_path()))
    [proposed] = [e for e in store.events("ActionProposed") if e.action_id == "m1"]
    assert proposed.payload["model"] == "claude-opus-5-5"
