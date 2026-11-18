"""The project's own commands for a role's checks: which command runs its tests, and a box to run them in.

Ported from the Python engine (``jaynes.runners``, ``jaynes.sandbox``, ``jaynes.suite``, ``jaynes.preview``), so
roles need nothing but this library and the standard library. It comes inside the ``jaynes`` program, written next
to ``jaynes_role.py``.

The box: on macOS a project's commands run under ``sandbox-exec`` with a profile that allows the rest of the
system, so toolchains work, and denies the ways out: writing anywhere but the box's folders, the network except
local ports, reading where secrets usually live or what the engine keeps (the page's key and log, the record, other
runs), signalling outside the box, writing preferences, opening apps.

A known limit, said plainly: on macOS a project's commands reach ANY local TCP port, so a suite's own servers work
on fixed ports and on port 0, and so does everything else that listens on the machine: a browser's debugger, a
database without a password, the Jaynes page. The page's key and the record stay out of reach, so what comes through
the page from inside the box is not the person. sandbox-exec cannot cut one port out of "any local port": a deny of
one port under ``localhost:*`` holds for some port numbers and not for others (measured on macOS 26.5.2, the same
every time for the same profile), so no list of refused ports is written here. What holds is allowing listed ports
only, which breaks suites that serve on port 0; that is still to be measured. A network of the box's own, which
closes this, exists only on Linux.

Other known limits on macOS: a command may still bind a server on 0.0.0.0, which other machines on the network can
reach while it runs (the bind filter for localhost matches the wildcard too). The box sends no UDP, and reads and
writes none of the terminals (/dev/ttys*), so a command that needs a pty of its own fails there.
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
# What the engine keeps under its home that no project command reads: the page's key and log, the record. The runs'
# folders are hidden as well, but for the run's own copy of the project, whose installed dependencies the box links.
ENGINE_SECRETS = ("page.key", "page.log", "app-key.pub", "record.sqlite3", "record.sqlite3-wal", "record.sqlite3-shm")
# What the Python engine that came before kept in ~/.jaynes.
OLD_ENGINE = ("jaynes.sqlite3", "jaynes.sqlite3-wal", "jaynes.sqlite3-shm", "guard.json", "backups", "objects")
# Devices a command may write. Not the terminals (/dev/ttys*): a pty's other end has the same names as the person's
# own terminals, so the box reads and writes none of them, and a command that opens a pty of its own fails.
DEVICES = ('(literal "/dev/null")', '(literal "/dev/zero")', '(literal "/dev/dtracehelper")',
           '(literal "/dev/ptmx")', '(literal "/dev/tty")')


class Unavailable(Exception):
    pass


@dataclass(frozen=True)
class Box:
    writable: tuple[Path, ...]
    hidden: tuple[Path, ...] = ()  # folders it may not read, apart from its writable folders inside them
    ports: tuple[int, ...] = ()  # local ports it may connect to
    local: bool = False  # any local port: a test suite's own servers, and whatever else listens; never the internet
    readable: tuple[Path, ...] = ()  # folders inside hidden ones it may still read, such as the run's own copy
    home: Path = field(default_factory=Path.home)


def hidden_paths(box: Box) -> list[Path]:
    """Everything the box may not read: where secrets usually live, what the engine keeps, the runs, and the
    box's own hidden folders. One list for every system's box; ``box.readable`` and the writable folders inside
    these stay readable."""
    engine, old = engine_home(), box.home / ".jaynes"
    # the runs are hidden from a box that says what it reads in them; a photograph's box does not, and its page is
    # a step's output under the runs
    runs = [engine / "runs"] if box.local or box.readable else []
    return ([box.home / s for s in SECRETS] + [engine / f for f in ENGINE_SECRETS] + [old / f for f in OLD_ENGINE]
            + runs + list(box.hidden))


def engine_home() -> Path:
    """Where the engine keeps its record, roles and runs, as the engine itself finds it."""
    named = os.environ.get("JAYNES_ENGINE_HOME")
    return Path(named) if named else Path.home() / ".jaynes" / "engine"


def available() -> bool:
    """Jaynes can open its box here."""
    return sys.platform == "darwin" and SANDBOX_EXEC.exists() and not inside()


@functools.cache
def inside() -> bool:
    """Already in a box that macOS opens no other box inside: then the project's commands are refused (INSIDE)."""
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


@functools.cache
def user_temp() -> str:
    """The folder macOS gives this user for temporary files, apart from ``TMPDIR``: Chrome makes its lock there."""
    try:
        out = subprocess.run(["getconf", "DARWIN_USER_TEMP_DIR"], capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.SubprocessError):
        return ""
    return os.path.realpath(out.stdout.strip()) if out.returncode == 0 and out.stdout.strip() else ""


def _chrome_lock() -> list[str]:
    """Chrome, Chromium and the Chrome that tests use each make one folder of their own in the user's temporary
    folder, whatever ``TMPDIR`` says, and stop when they cannot: the box lets them write only those folders."""
    temp = user_temp()
    if not temp:
        return []
    pattern = re.escape(temp).replace('"', '\\"') + r"/\.?(com\.google\.[Cc]hrome|org\.chromium\.[Cc]hromium)[^/]*(/|$)"
    return [f'(allow file-write* (regex #"^{pattern}"))']


def profile(box: Box) -> str:
    """The sandbox-exec profile for ``box``. Where two rules match, an allow with a filter wins over a deny."""
    lines = ["(version 1)", "(allow default)",
             "(deny file-write*)", f"(allow file-write* {' '.join(DEVICES)})"]
    lines += _chrome_lock()
    if box.writable:
        lines.append(f"(allow file-write* {_subpaths(box.writable)})")
    lines.append(f"(deny file-read* {_subpaths(hidden_paths(box))})")
    # the last rule that matches wins: what it may read inside the hidden is read again, but for its hidden folders
    if box.readable:
        lines.append(f"(allow file-read* {_subpaths(box.readable)})")
        if box.hidden:
            lines.append(f"(deny file-read* {_subpaths(box.hidden)})")
    if box.writable:
        lines.append(f"(allow file-read* {_subpaths(box.writable)})")
    lines.append("(deny network-outbound)")
    # TCP only: the box sends no UDP, not even to its own machine
    lines += [f'(allow network-outbound (remote tcp "localhost:{int(p)}"))' for p in box.ports]
    if box.local:
        lines.append('(allow network-outbound (remote tcp "localhost:*"))')
        # a suite's own servers on socket files, such as the role tools: only in its own folders, a rule each, since
        # a socket rule with several paths holds only the first
        lines += [f"(allow network-outbound (remote unix-socket {_subpaths([p])}))" for p in box.writable]
    lines.append('(deny file-read* file-write* (regex #"^/dev/ttys[0-9]"))')
    lines += ["(deny signal)", "(allow signal (target same-sandbox))", "(deny user-preference-write)",
              "(deny appleevent-send)", "(deny lsopen)"]
    return "\n".join(lines) + "\n"


# Inside another program's sandbox (the shell of an agent whose box is on, as Claude Code's Bash) macOS opens no box,
# and that sandbox may be looser than Jaynes's: the project's commands are refused, never run under it.
INSIDE = ("Jaynes was started inside another program's sandbox and cannot put its own box around your project's "
          "commands. Start the run outside it: with the Jaynes plugin's MCP tool \"start\" "
          "(mcp__plugin_jaynes_jaynes__start), or by running \"jaynes start\" in an ordinary terminal")


def command_in_box(script: str, box: Box) -> list[str]:
    """The argv that runs the shell ``script`` inside ``box``. Inside another program's sandbox it refuses."""
    if inside():
        raise Unavailable(INSIDE)
    if available():
        return [str(SANDBOX_EXEC), "-p", profile(box), "/bin/sh", "-c", script]
    if os.environ.get("JAYNES_UNBOXED") == "1":
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
        env: dict[str, str] | None = None, hidden: tuple[Path, ...] = (), stdin: str | None = None,
        readable: tuple[Path, ...] = ()) -> Ran:
    """Run ``command`` from a throwaway copy of ``project``, in the box, with the installed dependencies of
    ``source`` linked, and ``stdin`` on its standard input. ``readable`` names more folders it may read, only read,
    inside the hidden ones, such as the run's own copy of the role a check's script is in. Raises ``Unavailable``
    where there is no box."""
    with tempfile.TemporaryDirectory(prefix="jaynes-suite-") as tmp:
        work, scratch = Path(tmp) / "project", Path(tmp) / "tmp"
        shutil.copytree(project, work, symlinks=True, ignore=shutil.ignore_patterns(".git", ".jaynes", *DEPENDENCIES))
        scratch.mkdir()
        # what a step installed in the run's copy first, then what the person's own folder has
        link_dependencies(project, work)
        if source is not None:
            link_dependencies(source, work)
        argv = command_in_box(command, Box((work, scratch), hidden=hidden, local=True, readable=(project, *readable)))
        proc = subprocess.Popen(argv, cwd=work, env={**os.environ, **suite_environment(work, scratch), **(env or {})},
                                stdin=subprocess.DEVNULL if stdin is None else subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, start_new_session=True)
        try:
            out, _ = proc.communicate(None if stdin is None else stdin.encode(), timeout=timeout)
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


def missing(project: Path, runner: Runner, *, source: Path | None = None) -> str | None:
    """Why the project's tests cannot run here at all, before any agent works on them, or None. A machine without
    the test runner is not the agent's to fix: the role refuses at once, with what to install. Raises
    ``Unavailable`` where there is no box: that is the machine's to fix, not the role's."""
    program = shlex.split(runner.suite)[0] if runner.suite.strip() else ""
    if not program:
        return None
    probe = f"command -v {shlex.quote(program)} >/dev/null || {{ echo 'not found: {program}'; exit 127; }}"
    if runner.kind == "pytest":
        probe += f" && {shlex.quote(program)} -c 'import pytest'"
    ran = run(project, probe, source=source, timeout=120)
    if ran.code == 0:
        return None
    said = ran.output.strip().splitlines()[-1] if ran.output.strip() else f"exit {ran.code}"
    hint = ("install pytest where the project's tests run (its .venv, or the python3 on the PATH)"
            if runner.kind == "pytest" else f"install {program}")
    return f"the project's tests cannot run on this machine ({runner.suite}: {said}); {hint}, then continue the run"


def refused(ran: Ran) -> list[str]:
    """What the box refused, when the command failed for it: then its failure says nothing about the project."""
    lines = [line.strip() for line in ran.printed.splitlines()
             if "operation not permitted" in line.lower() and not any(h in line for h in HARMLESS)]
    return lines if ran.code != 0 else []


# --- photographs ------------------------------------------------------------------------------------------------

CHROMES = ("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
           "/Applications/Chromium.app/Contents/MacOS/Chromium")
SIZE = (1440, 900)


# the names a Chrome or Chromium goes by on the PATH, as Linux installs them
CHROME_NAMES = ("google-chrome", "google-chrome-stable", "chromium", "chromium-browser")


def chrome() -> str | None:
    """The Chrome Jaynes photographs with: ``JAYNES_CHROME``, one installed where macOS keeps apps, or one on the
    PATH under the names Linux gives it."""
    named = os.environ.get("JAYNES_CHROME", "")
    for path in ([named] if named else []) + [*CHROMES, *(str(Path.home()) + p for p in CHROMES)]:
        if path and os.access(path, os.X_OK):
            return path
    return next((found for name in CHROME_NAMES if (found := shutil.which(name))), None)


def photograph(target: str, out: Path, size: tuple[int, int] = SIZE, ports: tuple[int, ...] = ()) -> str | None:
    """Photograph ``target``, a page file or an address on a local port in ``ports``, into the PNG ``out``, with a
    Chrome in the box: no internet, so a page shows only what it carries. None when done, else why not."""
    program = chrome()
    if program is None:
        return "there is no Chrome to photograph with: install Google Chrome, or name one in JAYNES_CHROME"
    page = target if re.match(r"^https?://(localhost|127\.0\.0\.1)(:\d+)?(/|$)", target) else Path(target).resolve().as_uri()
    out.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="jaynes-photo-") as tmp:
        shot = Path(tmp) / "shot.png"
        argv = [program, "--headless=new", "--no-sandbox", "--disable-gpu", "--use-mock-keychain", "--no-first-run",
                "--no-default-browser-check", "--disable-crash-reporter", "--disable-breakpad", "--hide-scrollbars",
                "--disable-extensions", "--disable-background-networking", "--disable-component-update",
                f"--user-data-dir={Path(tmp) / 'profile'}", f"--screenshot={shot}", f"--window-size={size[0]},{size[1]}",
                page]
        script = " ".join(shlex.quote(a) for a in argv)
        try:
            argv = command_in_box(script, Box((Path(tmp),), ports=ports))
            # Chrome keeps its default folders under the home macOS reports; the box lets it write none there, so
            # it is given one of its own, inside the temporary folder
            env = {**os.environ, "TMPDIR": tmp, "CFFIXED_USER_HOME": tmp}
            done = subprocess.run(argv, capture_output=True, text=True, timeout=90, env=env,
                                  start_new_session=True)
        except Unavailable as e:
            return str(e)
        except subprocess.TimeoutExpired:
            return f"Chrome did not photograph {target} within 90 seconds"
        if done.returncode != 0 or not shot.is_file():
            said = [line for line in done.stderr.splitlines() if "crashpad" not in line and line.strip()]
            return f"Chrome could not photograph {target} (exit {done.returncode}): " + " ".join(said[-3:])
        shutil.copyfile(shot, out)
    return None


if __name__ == "__main__":
    # jaynes_box.py photograph TARGET OUT [WIDTHxHEIGHT]: for the engine and for a role's scripts
    args = sys.argv[1:]
    if len(args) in (3, 4) and args[0] == "photograph":
        w, _, h = (args[3] if len(args) == 4 else f"{SIZE[0]}x{SIZE[1]}").partition("x")
        why = photograph(args[1], Path(args[2]), (int(w), int(h)))
        if why:
            print(why)
            sys.exit(1)
        sys.exit(0)
    print("usage: jaynes_box.py photograph TARGET OUT [WIDTHxHEIGHT]")
    sys.exit(2)
