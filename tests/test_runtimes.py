"""What a real agent's box lets it do: on a project step, run the project's own tests offline."""

from __future__ import annotations

import json
from pathlib import Path

from jaynes.preview import link_dependencies
from jaynes.runtime import Ran
from jaynes.runtimes import _project_env, _run, claude_commands, claude_settings, codex_command  # pyright: ignore[reportPrivateUsage]
from jaynes.suite import environment


def project_step(tmp_path: Path) -> tuple[Path, Path, Path, Path]:
    """A turn's workspace whose project copy has a virtualenv linked from the person's folder."""
    python = tmp_path / "pythons" / "cpython-3.12"
    (python / "bin").mkdir(parents=True)
    source = tmp_path / "person"
    (source / ".venv" / "bin").mkdir(parents=True)
    (source / ".venv" / "pyvenv.cfg").write_text(f"home = {python / 'bin'}\nversion_info = 3.12\n")
    ws, scratch = tmp_path / "turn" / "workspace", tmp_path / "turn" / "scratch"
    (ws / "project").mkdir(parents=True)
    scratch.mkdir()
    link_dependencies(source, ws / "project")
    return ws, scratch, source, python


def test_on_a_project_step_claude_may_read_the_dependencies_write_caches_and_use_local_ports(
        tmp_path: Path) -> None:
    ws, scratch, source, python = project_step(tmp_path)
    box = claude_settings(ws, scratch=scratch)["sandbox"]
    assert box["filesystem"]["allowWrite"] == [str(scratch)]
    assert {str(ws), str((source / ".venv").resolve()), str(python.resolve())} <= set(box["filesystem"]["allowRead"])
    assert box["network"]["allowLocalBinding"] is True and box["network"]["allowedDomains"] == []


def test_a_step_without_a_project_gets_no_local_ports_and_reads_only_its_workspace(tmp_path: Path) -> None:
    box = claude_settings(tmp_path, scratch=tmp_path / "scratch")["sandbox"]
    assert box["filesystem"]["allowRead"] == [str(tmp_path)] and "allowLocalBinding" not in box["network"]


def test_project_commands_keep_their_caches_in_scratch_and_find_the_copys_virtualenv(tmp_path: Path) -> None:
    ws, scratch, _, _ = project_step(tmp_path)
    env = environment(ws / "project", scratch)
    assert env["UV_CACHE_DIR"].startswith(str(scratch)) and env["XDG_CACHE_HOME"].startswith(str(scratch))
    assert env["PATH"].startswith(str(ws / "project" / ".venv" / "bin"))
    assert env["UV_OFFLINE"] == "1" and env["UV_NO_SYNC"] == "1"


def test_an_agent_on_a_project_step_starts_with_the_projects_environment(tmp_path: Path) -> None:
    ws, scratch, _, _ = project_step(tmp_path)
    code, out, _ = _run(["sh", "-c", "echo $TMPDIR $UV_OFFLINE"], ws, scratch, "", 10, _project_env(ws, scratch))
    assert (code, out.split()) == (0, [str(scratch), "1"])


def test_the_commands_a_claude_turn_ran_come_from_its_stream_with_how_each_ended() -> None:
    def use(i: str, name: str, **args: str) -> dict[str, object]:
        return {"type": "assistant", "message": {"content": [{"type": "tool_use", "id": i, "name": name, "input": args}]}}

    def result(i: str, content: object, error: bool = False) -> dict[str, object]:
        return {"type": "user", "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": i, "content": content, "is_error": error}]}}

    lines = [use("1", "Bash", command="uv run pytest -q", description="tests"), result("1", "Exit code 1\n1 failed", True),
             use("2", "Read", file_path="x"), result("2", "contents"),
             use("3", "Bash", command="uv run pytest -q"), result("3", [{"type": "text", "text": "a" * 5000 + "3 passed"}]),
             use("4", "Bash", command="sleep 999")]
    ran = claude_commands("\n".join(json.dumps(x) for x in lines))
    assert ran[0] == Ran("uv run pytest -q", False, "Exit code 1\n1 failed")
    assert ran[1].ok and ran[1].output.endswith("3 passed") and len(ran[1].output) <= 2000
    assert ran[2] == Ran("sleep 999", False, "")  # the turn ended before it did
    assert len(ran) == 3


def test_a_codex_command_item_says_what_ran_and_how_it_ended() -> None:
    item = {"id": "i1", "type": "command_execution", "command": "bash -lc 'pytest -q'",
            "aggregated_output": "1 failed\n", "exit_code": 1, "status": "failed"}
    assert codex_command(item) == Ran("bash -lc 'pytest -q'", False, "1 failed\n")
    assert codex_command({**item, "exit_code": 0, "status": "completed"}) == Ran("bash -lc 'pytest -q'", True, "1 failed\n")
    assert codex_command({"type": "agent_message", "text": "hi"}) is None
