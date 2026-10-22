"""``jaynes run`` killed with SIGKILL mid-turn resumes to the same events as an uninterrupted run."""

from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

from jaynes.store import Store

ROOT = Path(__file__).resolve().parents[1]


def jaynes(home: Path, release: Path, *args: str) -> subprocess.Popen[str]:
    env = dict(os.environ, PYTHONPATH=str(ROOT), JAYNES_TEST_RELEASE=str(release))
    return subprocess.Popen([sys.executable, "-m", "jaynes.cli", "run", "--home", str(home), *args],
                            env=env, stdout=subprocess.PIPE, text=True)


def start(home: Path, release: Path) -> subprocess.Popen[str]:
    return jaynes(home, release, "--role", "tests.durable:ROLE", "--task", "t", "--runtime", "slow=tests.durable:Agent")


def normalized(home: Path) -> list[tuple[str, dict[str, Any]]]:
    store = Store(home / "jaynes.sqlite3")
    out = [(e.type, {k: v for k, v in e.payload.items() if k != "seconds"}) for e in store.events()]
    store.close()
    return out


def test_resume_after_kill_reproduces_the_uninterrupted_events(tmp_path: Path) -> None:
    clean_home, release = tmp_path / "clean", tmp_path / "release"
    release.write_text("go")
    p = start(clean_home, release)
    out, _ = p.communicate(timeout=60)
    assert json.loads(out)["status"] == "finished"
    release.unlink()

    home = tmp_path / "killed"
    p = start(home, release)
    marker = home / "runs"
    deadline = time.monotonic() + 30
    while not list(marker.glob("*/b/turn-1/started")):
        assert p.poll() is None and time.monotonic() < deadline
        time.sleep(0.02)
    os.kill(p.pid, signal.SIGKILL)
    p.wait()
    [run] = [d.name for d in marker.iterdir() if d.is_dir()]
    release.write_text("go")
    p = jaynes(home, release, run)
    out, _ = p.communicate(timeout=60)
    assert json.loads(out)["status"] == "finished"

    killed = normalized(home)
    interrupted = [i for i, (t, _) in enumerate(killed) if t == "TurnInterrupted"]
    assert [killed[i][1] for i in interrupted] == [{"step": "b", "attempt": 1, "reason": "crash"}]
    i = interrupted[0]
    assert killed[i - 1][0] == "StepStarted" and killed[i - 1][1]["step"] == "b"
    assert killed[: i - 1] + killed[i + 1:] == normalized(clean_home)
    assert (home / "runs" / run / "b/turn-1/workspace").is_dir()  # the interrupted workspace is kept
