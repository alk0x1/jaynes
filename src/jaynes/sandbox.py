"""The box a project's own commands run in, such as its preview, when they run agent-changed code.

The agent works in its runtime's sandbox, but a preview or a test suite runs what it wrote on the person's
machine, outside that sandbox. On macOS Jaynes runs such commands under ``sandbox-exec`` with a profile that
allows the rest of the system, so toolchains work, and denies the ways out:

* writing anywhere but the box's folders (the copy and a temporary folder) and terminals;
* the network, except the local ports the run names (a backend the project's dev server talks to), or, for a
  test suite, any local port, since tests start their own servers and use local databases;
* reading where secrets usually live, and folders the box hides, such as Jaynes's home outside the copy;
* signalling processes outside the box, writing preferences, opening apps and sending Apple Events, which
  act outside any box.

Reading the rest of the disk is allowed; with no network, no writes outside and no other process to drive,
what is read has no way out but the named ports. Elsewhere there is no box yet: project commands refuse to
run unless ``JAYNES_UNBOXED=1`` accepts running them unboxed.

macOS opens no box inside another unless both are the same, as when Jaynes's own suite runs in a run's box and
its tests start previews. There project commands run in the box they are already in, which confines them.
"""

from __future__ import annotations

import ctypes
import functools
import os
import subprocess
import sys
from collections.abc import Iterable
from dataclasses import dataclass, field
from pathlib import Path

SANDBOX_EXEC = Path("/usr/bin/sandbox-exec")
SECRETS = (".ssh", ".aws", ".azure", ".gnupg", ".kube", ".docker", ".netrc", ".npmrc", ".pypirc",
           ".git-credentials", ".config/gh", ".config/gcloud", ".config/op", ".zsh_history", ".bash_history",
           ".claude", ".codex", "Library/Keychains", "Library/Cookies", "Library/Safari", "Library/Mail",
           "Library/Messages", "Library/Application Support/Google/Chrome", "Library/Application Support/Firefox")
DEVICES = ('(literal "/dev/null")', '(literal "/dev/zero")', '(literal "/dev/dtracehelper")',
           '(literal "/dev/ptmx")', '(regex #"^/dev/tty")')


class Unavailable(Exception):
    pass


@dataclass(frozen=True)
class Box:
    writable: tuple[Path, ...]
    hidden: tuple[Path, ...] = ()  # folders it may not read, apart from its writable folders inside them
    ports: tuple[int, ...] = ()  # local ports it may connect to
    local: bool = False  # any local port: a test suite's own servers and databases; never the internet
    home: Path = field(default_factory=Path.home)


def available() -> bool:
    """Jaynes can open its box here."""
    return sys.platform == "darwin" and SANDBOX_EXEC.exists() and not inside()


@functools.cache
def inside() -> bool:
    """Already in a box that macOS opens no other box inside, so what runs here is confined by it."""
    if sys.platform != "darwin" or not SANDBOX_EXEC.exists():
        return False
    try:
        check = ctypes.CDLL("/usr/lib/libSystem.dylib").sandbox_check
        check.restype, check.argtypes = ctypes.c_int, [ctypes.c_int, ctypes.c_char_p, ctypes.c_int]
        if check(os.getpid(), None, 0) != 1:
            return False
        trial = subprocess.run([str(SANDBOX_EXEC), "-p", "(version 1)(allow default)", "/usr/bin/true"],
                               capture_output=True, timeout=10)
    except (OSError, AttributeError, subprocess.SubprocessError):
        return False
    return trial.returncode != 0


def _lit(p: Path | str) -> str:
    return '"' + os.path.realpath(p).replace("\\", "\\\\").replace('"', '\\"') + '"'


def _subpaths(paths: Iterable[Path | str]) -> str:
    return " ".join(f"(subpath {_lit(p)})" for p in paths)


def profile(box: Box) -> str:
    """The sandbox-exec profile for ``box``. Where two rules match, an allow with a filter wins over a deny."""
    lines = ["(version 1)", "(allow default)",
             "(deny file-write*)", f"(allow file-write* {' '.join(DEVICES)})"]
    if box.writable:
        lines.append(f"(allow file-write* {_subpaths(box.writable)})")
    lines.append(f"(deny file-read* {_subpaths([box.home / s for s in SECRETS] + list(box.hidden))})")
    if box.writable:
        lines.append(f"(allow file-read* {_subpaths(box.writable)})")
    lines.append("(deny network-outbound)")
    lines += [f'(allow network-outbound (remote ip "localhost:{int(p)}"))' for p in box.ports]
    if box.local:
        lines.append('(allow network-outbound (remote ip "localhost:*"))')
        # a suite's own servers on socket files, such as the role tools: only in its own folders, a rule each, since
        # a socket rule with several paths holds only the first
        lines += [f"(allow network-outbound (remote unix-socket {_subpaths([p])}))" for p in box.writable]
    lines += ["(deny signal)", "(allow signal (target same-sandbox))", "(deny user-preference-write)",
              "(deny appleevent-send)", "(deny lsopen)"]
    return "\n".join(lines) + "\n"


def command(script: str, box: Box) -> list[str]:
    """The argv that runs the shell ``script`` inside ``box``."""
    if available():
        return [str(SANDBOX_EXEC), "-p", profile(box), "/bin/sh", "-c", script]
    if inside() or os.environ.get("JAYNES_UNBOXED") == "1":
        return ["/bin/sh", "-c", script]
    raise Unavailable("Jaynes has no box for project commands on this system yet; set JAYNES_UNBOXED=1 to run "
                      "them unboxed, with the agent's changes, on your machine")


def environment(tmp: Path) -> dict[str, str]:
    """Variables that keep temporary files and package caches inside the box's temporary folder."""
    (tmp / "npm").mkdir(parents=True, exist_ok=True)
    (tmp / "npmrc").touch()
    # Go ignores XDG_CACHE_HOME on macOS and writes its build cache to ~/Library/Caches unless told
    return {"TMPDIR": str(tmp), "XDG_CACHE_HOME": str(tmp / "cache"), "npm_config_cache": str(tmp / "npm"),
            "npm_config_userconfig": str(tmp / "npmrc"), "npm_config_update_notifier": "false",
            "GOCACHE": str(tmp / "go"), "GOPROXY": "off", "GOTOOLCHAIN": "local"}
