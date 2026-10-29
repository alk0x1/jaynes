"""A project's own commands, such as its tests, run by a check on a copy of the step's project, in a box.

The command runs what the agent wrote, so it runs in a box (``jaynes.sandbox``): no internet, though any local
port (tests start their own servers), and no writes outside a throwaway copy of the step's project. Installed
dependencies are linked from the person's repository (``jaynes.preview.link_dependencies``), and the copy's
own code comes first: Python imports from the copy before an editable install that points at the person's
folder, and uv neither syncs nor downloads. Commits the tests make are not signed: the box hides the
person's keys.
"""

from __future__ import annotations

import os
import shutil
import signal
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path

from jaynes import preview, sandbox
from jaynes.role import Witnesses

TAIL = 3000  # characters of output kept, from the end, where test runners put their summary


@dataclass(frozen=True)
class Ran:
    code: int  # 124 when the command ran out of time
    output: str  # the end of what it printed, stdout and stderr together
    printed: str = ""  # all of it, to search: a long traceback can push a line out of the end


def environment(work: Path, scratch: Path) -> dict[str, str]:
    """What a project's own commands run with in ``work``, a copy with its dependencies linked: caches in
    ``scratch``, package managers offline, and the copy's code and virtualenv first. Agents get it too."""
    venv = work / ".venv"
    env = {**sandbox.environment(scratch), "CI": "1", "UV_NO_SYNC": "1", "UV_OFFLINE": "1",
           "UV_CACHE_DIR": str(scratch / "uv"), "npm_config_offline": "true", "PYTHONDONTWRITEBYTECODE": "1",
           # as git -c does; one variable, since Codex drops variables named like KEY from its commands
           "GIT_CONFIG_PARAMETERS": "'commit.gpgsign'='false' 'tag.gpgsign'='false'",
           "PYTHONPATH": os.pathsep.join([str(work / "src"), str(work)])}
    if venv.is_dir():
        env |= {"VIRTUAL_ENV": str(venv), "PATH": f"{venv / 'bin'}{os.pathsep}{os.environ.get('PATH', '')}"}
    return env


def reads(work: Path) -> list[Path]:
    """The real folders behind the dependencies linked into ``work``, and the Python each virtualenv runs: what an
    agent's box must let it read to run the project's commands."""
    found: set[Path] = set()
    for d, dirs, _ in os.walk(work):
        for name in [x for x in dirs if x in preview.DEPENDENCIES]:
            here = Path(d) / name
            found |= {Path(e.path).resolve().parent for e in os.scandir(here) if e.is_symlink()}
            cfg = here / "pyvenv.cfg"
            home = next((line.split("=", 1)[1].strip() for line in cfg.read_text().splitlines()
                         if line.split("=", 1)[0].strip() == "home"), "") if cfg.is_file() else ""
            if home:
                found.add(Path(home).resolve().parent)
        dirs[:] = [x for x in dirs if x not in preview.DEPENDENCIES and x != ".git"]
    return sorted(found)


def run(needs: Witnesses, command: str, timeout: float = 600.0) -> Ran:
    """Run ``command`` from a copy of ``needs.project``. Raises ``sandbox.Unavailable`` where there is no box."""
    assert needs.project is not None, "a project command needs a step that works on the project"
    with tempfile.TemporaryDirectory(prefix="jaynes-suite-") as tmp:
        work, scratch = Path(tmp) / "project", Path(tmp) / "tmp"
        shutil.copytree(needs.project, work, symlinks=True,
                        ignore=shutil.ignore_patterns(".git", *preview.DEPENDENCIES))
        scratch.mkdir()
        if needs.source is not None:
            preview.link_dependencies(needs.source, work)
        argv = sandbox.command(command, sandbox.Box((work, scratch), hidden=needs.hidden, local=True))
        proc = subprocess.Popen(argv, cwd=work, env={**os.environ, **environment(work, scratch)},
                                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, start_new_session=True)
        try:
            out, _ = proc.communicate(timeout=timeout)
            code = proc.returncode
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            out, _ = proc.communicate()
            out += f"\n(stopped after {timeout:.0f} s)".encode()
            code = 124
        text = out.decode(errors="replace")
        return Ran(code, text[-TAIL:], text)
