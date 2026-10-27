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
        venv = work / ".venv"
        env = {**os.environ, **sandbox.environment(scratch), "CI": "1", "UV_NO_SYNC": "1", "UV_OFFLINE": "1",
               "npm_config_offline": "true", "PYTHONDONTWRITEBYTECODE": "1", "GIT_CONFIG_COUNT": "2",
               "GIT_CONFIG_KEY_0": "commit.gpgsign", "GIT_CONFIG_VALUE_0": "false",
               "GIT_CONFIG_KEY_1": "tag.gpgsign", "GIT_CONFIG_VALUE_1": "false",
               "PYTHONPATH": os.pathsep.join([str(work / "src"), str(work)])}
        if venv.is_dir():
            env |= {"VIRTUAL_ENV": str(venv), "PATH": f"{venv / 'bin'}{os.pathsep}{os.environ.get('PATH', '')}"}
        proc = subprocess.Popen(argv, cwd=work, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, start_new_session=True)
        try:
            out, _ = proc.communicate(timeout=timeout)
            code = proc.returncode
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            out, _ = proc.communicate()
            out += f"\n(stopped after {timeout:.0f} s)".encode()
            code = 124
        return Ran(code, out.decode(errors="replace")[-TAIL:])
