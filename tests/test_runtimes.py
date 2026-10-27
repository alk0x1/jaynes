"""What a real agent's box lets it do: on a project step, run the project's own tests offline."""

from __future__ import annotations

from pathlib import Path

from jaynes.preview import link_dependencies
from jaynes.runtimes import _project_env, _run, claude_settings  # pyright: ignore[reportPrivateUsage]
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
