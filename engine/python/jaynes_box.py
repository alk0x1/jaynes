"""The project's own commands for a role's checks: which command runs its tests, and a box to run them in.

Ported from the Python engine (``jaynes.runners``, ``jaynes.sandbox``, ``jaynes.suite``, ``jaynes.preview``), so
roles need nothing but this library and the standard library. It comes inside the ``jaynes`` program, written next
to ``jaynes_role.py``.

The box: on macOS a project's commands run under ``sandbox-exec`` with a profile that allows the rest of the
system, so toolchains work, and denies the ways out: writing anywhere but the box's folders, the network except
local ports, reading where secrets usually live, signalling outside the box, writing preferences, opening apps.
Elsewhere there is no box yet: commands refuse to run unless ``JAYNES_UNBOXED=1`` accepts running them unboxed.
"""

from __future__ import annotations

import ctypes
import functools
import json
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
from collections.abc import Iterable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, cast

DEPENDENCIES = {"node_modules", ".venv", "venv", "vendor", "bower_components"}
CACHES = {".vite", ".vite-temp", ".cache"}
TAIL = 3000  # characters of output kept, from the end, where test runners put their summary

# --- the box ---------------------------------------------------------------------------------------------------

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


def command_in_box(script: str, box: Box) -> list[str]:
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


# --- installed dependencies ------------------------------------------------------------------------------------

def link_dependencies(source: Path, dest: Path, depth: int = 4) -> list[str]:
    """Link installed dependency folders of ``source`` into the same places in ``dest``, entry by entry."""
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


# --- which command runs the tests -------------------------------------------------------------------------------

PYTHON = "python3"  # without a virtualenv in the project
PYTEST = "-m pytest -q -p no:cacheprovider"
NPM_DEFAULT = "no test specified"  # the test script npm init writes, which only fails


@dataclass(frozen=True)
class Runner:
    kind: str  # pytest, node, go, cargo, make or given
    suite: str  # runs the whole test suite, from the project folder
    each: str  # followed by test files, runs only them; empty when the runner cannot pick files
    found: str  # what it was found from

    def to_dict(self) -> dict[str, str]:
        return {"kind": self.kind, "suite": self.suite, "each": self.each, "found": self.found}

    @staticmethod
    def from_dict(d: dict[str, Any]) -> "Runner":
        return Runner(str(d.get("kind", "")), str(d.get("suite", "")), str(d.get("each", "")), str(d.get("found", "")))


def detect(root: Path, given: str = "") -> Runner | None:
    """The runner of the project at ``root``, or None. ``given``, the person's suite command, replaces the
    suite; the runner found still picks test files when it can."""
    found = _python(root) or _node(root) or _go(root) or _cargo(root) or _make(root)
    if not given:
        return found
    if found is None or found.kind == "make":
        return Runner(kind="given", suite=given, each="", found="given at start")
    return Runner(found.kind, given, found.each, f"suite given at start; files by {found.found}")


def for_files(runner: Runner, tests: list[str]) -> str:
    """The command that runs ``tests``, paths relative to the project folder."""
    files = [shlex.quote(t) for t in tests]
    if runner.kind == "go":  # go test takes packages, the folders of the files
        dirs = sorted({Path(t).parent.as_posix() for t in tests})
        return f"{runner.each} {' '.join(shlex.quote('.' if d == '.' else './' + d) for d in dirs)}"
    if runner.kind == "cargo":
        named = [f"--test {shlex.quote(Path(t).stem)}" for t in tests if Path(t).parts[:1] == ("tests",)]
        return f"{runner.each} {' '.join(named)}" if named else runner.suite
    if not runner.each or not files:
        return runner.suite
    return f"{runner.each} {' '.join(files)}"


def _python(root: Path) -> Runner | None:
    config = [n for n in ("pytest.ini", "conftest.py") if (root / n).is_file()]
    for name, mark in (("pyproject.toml", "pytest"), ("setup.cfg", "[tool:pytest]"), ("tox.ini", "[pytest]")):
        if (root / name).is_file() and mark in (root / name).read_text(errors="replace"):
            config.append(name)
    if not config and not _has(root, re.compile(r"(^test_.*|.*_test)\.py$")):
        return None
    python = ".venv/bin/python" if (root / ".venv" / "bin" / "python").exists() else PYTHON
    command = f"{python} {PYTEST}"
    return Runner(kind="pytest", suite=command, each=command, found=config[0] if config else "Python test files")


def _node(root: Path) -> Runner | None:
    try:
        package: object = json.loads((root / "package.json").read_text())
    except (OSError, ValueError):
        return None
    scripts = cast("dict[str, object]", package).get("scripts") if isinstance(package, dict) else None
    script = cast("dict[str, object]", scripts).get("test") if isinstance(scripts, dict) else None
    if not isinstance(script, str) or not script.strip() or NPM_DEFAULT in script:
        return None
    manager = next((m for lock, m in (("pnpm-lock.yaml", "pnpm"), ("yarn.lock", "yarn"), ("bun.lockb", "bun"),
                                      ("bun.lock", "bun")) if (root / lock).is_file()), "npm")
    suite = "bun run test" if manager == "bun" else f"{manager} test"
    return Runner(kind="node", suite=suite, each=suite if manager == "yarn" else f"{suite} --",
                  found="package.json scripts.test")


def _go(root: Path) -> Runner | None:
    if not (root / "go.mod").is_file():
        return None
    return Runner(kind="go", suite="go test ./...", each="go test", found="go.mod")


def _cargo(root: Path) -> Runner | None:
    if not (root / "Cargo.toml").is_file():
        return None
    return Runner(kind="cargo", suite="cargo test --offline", each="cargo test --offline", found="Cargo.toml")


def _make(root: Path) -> Runner | None:
    for name in ("Makefile", "GNUmakefile", "makefile"):
        path = root / name
        if path.is_file() and re.search(r"^test\s*:", path.read_text(errors="replace"), re.M):
            return Runner(kind="make", suite="make test", each="", found=f"{name} test target")
    return None


def _has(root: Path, name: re.Pattern[str]) -> bool:
    """Whether a file outside dependency folders and .git matches ``name``."""
    for _, dirs, files in os.walk(root):
        dirs[:] = [x for x in dirs if x not in DEPENDENCIES and x != ".git"]
        if any(name.match(f) for f in files):
            return True
    return False


# --- running a project's command in the box --------------------------------------------------------------------

@dataclass(frozen=True)
class Ran:
    code: int  # 124 when the command ran out of time
    output: str  # the end of what it printed, stdout and stderr together
    printed: str = ""  # all of it


def suite_environment(work: Path, scratch: Path) -> dict[str, str]:
    """What a project's own commands run with in ``work``: caches in ``scratch``, package managers offline, and
    the copy's code and virtualenv first."""
    venv = work / ".venv"
    env = {**environment(scratch), "CI": "1", "UV_NO_SYNC": "1", "UV_OFFLINE": "1",
           "UV_CACHE_DIR": str(scratch / "uv"), "npm_config_offline": "true", "PYTHONDONTWRITEBYTECODE": "1",
           "GIT_CONFIG_PARAMETERS": "'commit.gpgsign'='false' 'tag.gpgsign'='false'",
           "PYTHONPATH": os.pathsep.join([str(work / "src"), str(work)])}
    if venv.is_dir():
        env |= {"VIRTUAL_ENV": str(venv), "PATH": f"{venv / 'bin'}{os.pathsep}{os.environ.get('PATH', '')}"}
    return env


def run(project: Path, command: str, *, source: Path | None = None, timeout: float = 600.0,
        env: dict[str, str] | None = None, hidden: tuple[Path, ...] = ()) -> Ran:
    """Run ``command`` from a throwaway copy of ``project``, in the box, with the installed dependencies of
    ``source`` linked. Raises ``Unavailable`` where there is no box."""
    with tempfile.TemporaryDirectory(prefix="jaynes-suite-") as tmp:
        work, scratch = Path(tmp) / "project", Path(tmp) / "tmp"
        shutil.copytree(project, work, symlinks=True, ignore=shutil.ignore_patterns(".git", ".jaynes", *DEPENDENCIES))
        scratch.mkdir()
        if source is not None:
            link_dependencies(source, work)
        argv = command_in_box(command, Box((work, scratch), hidden=hidden, local=True))
        proc = subprocess.Popen(argv, cwd=work, env={**os.environ, **suite_environment(work, scratch), **(env or {})},
                                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                start_new_session=True)
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


# What the box refuses that no command needs: macOS's developer tools try to keep a cache outside the box and go on
# without it.
HARMLESS = ("xcrun_db",)


def refused(ran: Ran) -> list[str]:
    """What the box refused, when the command failed for it: then its failure says nothing about the project."""
    lines = [line.strip() for line in ran.printed.splitlines()
             if "operation not permitted" in line.lower() and not any(h in line for h in HARMLESS)]
    return lines if ran.code != 0 else []
