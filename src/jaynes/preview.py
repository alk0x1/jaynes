"""Previews at stops: the run's preview command serving a copy of the project, on a free local port.

Jaynes starts the command itself in copies it prepares, before the run's changes and after them, so what
the person sees is the project as changed, not a picture the agent made. Installed dependencies
(``node_modules`` and the like) are linked from the person's folder rather than copied. The command runs
the run's changes, so it runs in a box (``jaynes.sandbox``).
"""

from __future__ import annotations

import os
import signal
import socket
import subprocess
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

from jaynes import sandbox

DEPENDENCIES = {"node_modules", ".venv", "venv", "vendor", "bower_components"}
CACHES = {".vite", ".vite-temp", ".cache"}  # written by dev servers; each copy keeps its own
READY_WITHIN = 120.0  # seconds a preview has to start answering


@dataclass
class Preview:
    proc: subprocess.Popen[bytes]
    port: int
    path: str
    log: Path
    since: float


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


def link_dependencies(source: Path, dest: Path, depth: int = 4) -> list[str]:
    """Link installed dependency folders of ``source`` into the same places in ``dest``, entry by entry.

    Each folder is made anew in ``dest`` and holds a symlink per entry, so what a dev server writes there (its
    cache) stays in the copy, and the box can keep the person's own folder read-only.
    """
    linked: list[str] = []
    for d, dirs, _ in os.walk(source):
        here = Path(d)
        rel = here.relative_to(source)
        if len(rel.parts) >= depth:
            dirs[:] = []
            continue
        for name in [x for x in dirs if x in DEPENDENCIES]:
            target = dest / rel / name
            if target.parent.is_dir() and not target.exists():
                target.mkdir()
                for entry in os.scandir(here / name):
                    if entry.name not in CACHES:
                        (target / entry.name).symlink_to(entry.path, target_is_directory=entry.is_dir())
                linked.append((rel / name).as_posix())
        dirs[:] = [x for x in dirs if x not in DEPENDENCIES and x != ".git"]
    return linked


class Previews:
    """Running previews by key, at most ``limit`` at a time; the oldest stops first."""

    def __init__(self, limit: int = 6) -> None:
        self.limit = limit
        self.live: dict[str, Preview] = {}
        self.lock = threading.Lock()

    def ensure(self, key: str, prepare: Callable[[], Path], source: Path, command: str, path: str, *,
               hidden: tuple[Path, ...] = (), ports: tuple[int, ...] = ()) -> Preview:
        """The preview for ``key``, started in the folder ``prepare`` returns if it is not running.

        It runs in a box that writes only to that folder and its temporary folder, hides ``hidden`` and
        reaches no network but the local ``ports``. Raises ``sandbox.Unavailable`` where there is no box.
        """
        with self.lock:
            found = self.live.get(key)
            if found is not None and found.proc.poll() is None:
                return found
            folder = prepare()
            link_dependencies(source, folder)
            port = free_port()
            log = folder.parent / f"{folder.name}.log"
            tmp = folder.parent / f"{folder.name}.tmp"
            argv = sandbox.command(command.replace("{port}", str(port)),
                                   sandbox.Box((folder, tmp), hidden=hidden, ports=ports))
            with log.open("ab") as out:
                proc = subprocess.Popen(argv, cwd=folder, env={**os.environ, **sandbox.environment(tmp)},
                                        stdin=subprocess.DEVNULL, stdout=out, stderr=subprocess.STDOUT,
                                        start_new_session=True)
            self.live[key] = Preview(proc, port, path if path.startswith("/") else "/" + path, log, time.time())
            while len(self.live) > self.limit:
                oldest = min(self.live, key=lambda k: self.live[k].since)
                _stop(self.live.pop(oldest))
            return self.live[key]

    @staticmethod
    def state(p: Preview) -> tuple[Literal["ready", "starting", "failed"], str]:
        """Ready with its URL, starting, or failed with the end of its output."""
        if p.proc.poll() is not None:
            return "failed", f"the preview command exited with {p.proc.returncode}\n" + _tail(p.log)
        try:
            with socket.create_connection(("127.0.0.1", p.port), timeout=0.2):
                return "ready", f"http://127.0.0.1:{p.port}{p.path}"
        except OSError:
            if time.time() - p.since > READY_WITHIN:
                return "failed", f"nothing answered on port {p.port} after {READY_WITHIN:.0f} s\n" + _tail(p.log)
            return "starting", _tail(p.log)

    def stop_all(self) -> None:
        with self.lock:
            for p in self.live.values():
                _stop(p)
            self.live.clear()


def _stop(p: Preview) -> None:
    try:
        os.killpg(p.proc.pid, signal.SIGTERM)
    except (ProcessLookupError, PermissionError):
        pass


def _tail(log: Path, size: int = 3000) -> str:
    try:
        return log.read_bytes()[-size:].decode(errors="replace")
    except OSError:
        return ""
