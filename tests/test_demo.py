"""``jaynes demo`` works from an installed package: no repository, no PYTHONPATH, no agent quota."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path


JAYNES = Path(sys.executable).with_name("jaynes")  # the installed command, which unlike -m puts no cwd on sys.path


def jaynes(cwd: Path, *args: str, env: dict[str, str] | None = None) -> str:
    env = {k: v for k, v in os.environ.items() if k != "PYTHONPATH"} | (env or {})
    done = subprocess.run([str(JAYNES), *args], env=env, capture_output=True, text=True,
                          timeout=60, cwd=cwd)
    assert done.returncode == 0, done.stderr
    return done.stdout


def test_demo_leaves_a_stop_waiting_and_an_answer_continues_it(tmp_path: Path) -> None:
    home = tmp_path / "home"
    out = jaynes(tmp_path, "demo", "--no-serve", env={"JAYNES_HOME": str(home)})
    run = json.loads(out.splitlines()[0])["run"]
    queue = jaynes(tmp_path, "queue", "--home", str(home))  # the demo shares the home of real runs
    assert f"{run}  stop  requirements" in queue
    assert "Are they the right fixes?" in queue

    jaynes(tmp_path, "answer", run, "requirements", "accept", "--home", str(home))
    deadline = time.time() + 30
    while f"{run}  stop  accept" not in jaynes(tmp_path, "queue", "--home", str(home)):
        assert time.time() < deadline, (home / "runs" / run / "continue.log").read_text()
        time.sleep(0.2)


def test_a_new_run_can_be_given_its_id(tmp_path: Path) -> None:
    out = jaynes(tmp_path, "run", "r-chosen", "--home", str(tmp_path / "home"), "--role", "jaynes.demo:ROLE",
                 "--task", "t", "--runtime", "scripted=jaynes.demo:Agent")
    assert json.loads(out)["run"] == "r-chosen"


def test_read_only_commands_fall_back_to_the_built_in_policy(tmp_path: Path) -> None:
    env = {k: v for k, v in os.environ.items() if k != "PYTHONPATH"} | {"JAYNES_HOME": str(tmp_path)}
    done = subprocess.run([str(JAYNES), "autonomy", "--db", str(tmp_path / "x.sqlite3")],
                          env=env, capture_output=True, text=True, timeout=60, cwd=tmp_path)
    assert done.returncode == 0, done.stderr
    assert "agent-team.jaynes.toml" in done.stdout


def test_policy_prints_the_built_in_policy(tmp_path: Path) -> None:
    assert '[policy]' in jaynes(tmp_path, "policy")


def test_run_finds_a_role_in_the_current_folder(tmp_path: Path) -> None:
    (tmp_path / "my_role.py").write_text(
        "from pydantic import BaseModel\n"
        "from jaynes.role import role\n"
        "class Note(BaseModel):\n    text: str\n"
        "ROLE = role('mine', agent='claude')\n"
        "ROLE.step('note', output=Note, run_by=lambda ctx: Note(text=ctx.task))\n")
    out = jaynes(tmp_path, "run", "--home", str(tmp_path / "home"), "--role", "my_role:ROLE", "--task", "hi")
    assert json.loads(out)["status"] == "finished"
