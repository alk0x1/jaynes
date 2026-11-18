"""The project's own commands for a role's checks: which command runs its tests, and a box to run them in.

Ported from the Python engine (``jaynes.runners``, ``jaynes.sandbox``, ``jaynes.suite``, ``jaynes.preview``), so
roles need nothing but this library and the standard library. It comes inside the ``jaynes`` program, written next
to ``jaynes_role.py``.

The box: on macOS a project's commands run under ``sandbox-exec`` with a profile that allows the rest of the
system, so toolchains work, and denies the ways out: writing anywhere but the box's folders, the network except
local ports, reading where secrets usually live or what the engine keeps (the page's key and log, the record, other
runs), signalling outside the box, writing preferences, opening apps.

Local ports, on macOS, are a list of what is allowed, closed by default: a project's commands bind and reach only
the ports the run reserved for them, handed over in ``JAYNES_PORTS`` (free ports, comma separated), and the ports
the step declares in its reach as ``local_ports``; never the Jaynes page's, through which an answer counts as the
person's. Unix sockets work only in the box's own folders. A suite that serves on port 0 (Go's httptest, supertest,
a test server on port 0) is refused the bind: it has to take its port from ``JAYNES_PORTS``, and until it does its
check is blocked, the machine's fault, naming what was refused. Only a list of allowed ports holds: a deny of one
port under ``localhost:*`` holds for some port numbers and not for others (measured on macOS 26.5.2).

The person may open one project's checks to any local port (``jaynes project open-ports DIR``, at a terminal): the
choice is kept in the engine's home, which no box reads or writes, fixed for a run when it starts and recorded in
its setup, and the engine says it in each call (``any_port``), never through the environment. Then a project's commands
bind and reach every local TCP port, the page's among them: sandbox-exec cannot keep one port shut under that.

Known limits on macOS: a command may bind an allowed port on 0.0.0.0, which other machines on the network can reach
while it runs (the bind filter for localhost matches the wildcard too). The box sends no UDP, and reads and writes
none of the terminals (/dev/ttys*), so a command that needs a pty of its own fails there.
Services of macOS reached by name (mach-lookup) are open but for the pasteboard's and the keychain's, which are
denied (``MACH_DENIED``): no project command reads or writes what the person copied, nor asks their keychain. Every
other service still answers; closing them all takes a profile that denies by default, with each service listed.
A service's name may change in a later macOS: the end-to-end test that runs a real run's suite as a probe (it fails
when it reads or writes the pasteboard or lists the keychains) is what notices, so it must keep passing.
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
import stat
import socket
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
# The Jaynes page: an answer through it counts as the person's, so no box ever reaches it.
PAGE_PORT = 8750
# How many local ports a run reserves for a project's commands, handed over in JAYNES_PORTS.
RESERVED = 8
OLD_ENGINE = ("jaynes.sqlite3", "jaynes.sqlite3-wal", "jaynes.sqlite3-shm", "guard.json", "backups", "objects")
# Devices a command may write. Not the terminals (/dev/ttys*): a pty's other end has the same names as the person's
# own terminals, so the box reads and writes none of them, and a command that opens a pty of its own fails.
# Services of macOS a command reaches by name, past every file and network rule, that no project command needs: the
# pasteboard (com.apple.pasteboard.1), through which a command read what the person copied and could write it; and
# the keychain's (com.apple.SecurityServer, com.apple.securityd, com.apple.securityd.xpc), through which it could ask
# the person's keychain for its items. Measured on real suites before they were denied: Go with httptest, with TLS
# and the system's verifier, Node with supertest, Python with a test server and its TLS context, a Go app's suite,
# and the Helm roles' checks with Chrome all pass the same without them.
MACH_DENIED = ("com.apple.pasteboard.1", "com.apple.SecurityServer", "com.apple.securityd", "com.apple.securityd.xpc")

DEVICES = ('(literal "/dev/null")', '(literal "/dev/zero")', '(literal "/dev/dtracehelper")',
           '(literal "/dev/ptmx")', '(literal "/dev/tty")')


class Unavailable(Exception):
    pass


@dataclass(frozen=True)
class Box:
    writable: tuple[Path, ...]
    hidden: tuple[Path, ...] = ()  # folders it may not read, apart from its writable folders inside them
    readonly: tuple[Path, ...] = ()  # folders inside its writable folders that it may read and not write
    ports: tuple[int, ...] = ()  # local ports it may connect to
    # a test suite's own servers: it binds and reaches ``reserved`` and ``local_ports``, never the page's; never
    # the internet
    local: bool = False
    reserved: tuple[int, ...] = ()  # the free ports the run reserved, handed over in JAYNES_PORTS
    local_ports: tuple[int, ...] = ()  # the ports the step declares in its reach
    any_port: bool = False  # with ``local``: every local TCP port, opened by the person for this project
    readable: tuple[Path, ...] = ()  # folders inside hidden ones it may still read, such as the run's own copy
    home: Path = field(default_factory=Path.home)
    runs: tuple[Path, ...] = ()  # when given, the only programs it may start (an agent program in a step without commands)


def hidden_paths(box: Box) -> list[Path]:
    """Everything the box may not read: where secrets usually live, what the engine keeps, the runs, and the
    box's own hidden folders. One list for every system's box; ``box.readable`` and the writable folders inside
    these stay readable."""
    engine, old = engine_home(), box.home / ".jaynes"
    # the runs are hidden from a box that says what it reads in them; a photograph's box does not, and its page is
    # a step's output under the runs
    runs = [engine / "runs"] if box.local or box.readable else []
    return ([box.home / s for s in SECRETS] + [engine / f for f in ENGINE_SECRETS] + [engine / "projects"]
            + [old / f for f in OLD_ENGINE]
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


def _mach_denied() -> str:
    return "(deny mach-lookup " + " ".join(f'(global-name "{n}")' for n in MACH_DENIED) + ")"


def _chrome_socket() -> list[str]:
    """In that same folder Chrome binds the socket file that keeps one browser to a profile, and stops when it
    cannot: the box lets it bind and reach socket files there and nowhere else outside its own folders."""
    temp = user_temp()
    if not temp:
        return []
    pattern = re.escape(temp).replace('"', '\\"') + r"/\.?(com\.google\.[Cc]hrome|org\.chromium\.[Cc]hromium)[^/]*/"
    return [f'(allow network-bind (local unix-socket (regex #"^{pattern}")))',
            f'(allow network-outbound (remote unix-socket (regex #"^{pattern}")))']


def profile(box: Box) -> str:
    """The sandbox-exec profile for ``box``. Where two rules match, an allow with a filter wins over a deny."""
    lines = ["(version 1)", "(allow default)",
             "(deny file-write*)", f"(allow file-write* {' '.join(DEVICES)})"]
    lines += _chrome_lock()
    if box.writable:
        lines.append(f"(allow file-write* {_subpaths(box.writable)})")
    if box.readonly:  # after the allow it narrows: of two rules that match, the later one holds
        lines.append(f"(deny file-write* {_subpaths(box.readonly)})")
    lines.append(f"(deny file-read* {_subpaths(hidden_paths(box))})")
    # the last rule that matches wins: what it may read inside the hidden is read again, but for its hidden folders
    if box.readable:
        lines.append(f"(allow file-read* {_subpaths(box.readable)})")
        if box.hidden:
            lines.append(f"(deny file-read* {_subpaths(box.hidden)})")
    if box.writable:
        lines.append(f"(allow file-read* {_subpaths(box.writable)})")
    lines.append("(deny network-outbound)")
    # TCP only: the box sends no UDP, not even to its own machine. Only listed ports: a deny under a wildcard does not
    # hold in sandbox-exec
    lines += [f'(allow network-outbound (remote tcp "localhost:{int(p)}"))' for p in box.ports if p != PAGE_PORT]
    if box.local and box.any_port:
        lines.append('(allow network-outbound (remote tcp "localhost:*"))')
        lines += [f"(allow network-outbound (remote unix-socket {_subpaths([p])}))" for p in box.writable]
    elif box.local:
        lines.append("(deny network-bind)")
        for p in sorted({*box.reserved, *box.local_ports} - {PAGE_PORT}):
            lines.append(f'(allow network-bind (local tcp "localhost:{int(p)}"))')
            lines.append(f'(allow network-outbound (remote tcp "localhost:{int(p)}"))')
        lines += [f"(allow network-bind (local unix-socket {_subpaths([p])}))" for p in box.writable]
        lines += _chrome_socket()
        # a suite's own servers on socket files, such as the role tools: only in its own folders, a rule each, since
        # a socket rule with several paths holds only the first
        lines += [f"(allow network-outbound (remote unix-socket {_subpaths([p])}))" for p in box.writable]
    if box.runs:
        lines += ["(deny process-exec)", "(allow process-exec " + " ".join(f"(literal {_lit(p)})" for p in box.runs) + ")"]
    lines.append('(deny file-read* file-write* (regex #"^/dev/ttys[0-9]"))')
    lines.append(_mach_denied())
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


def programs_of(program: Path) -> tuple[Path, ...]:
    """What starting ``program`` starts: itself and, for a script, its interpreter, and the shell macOS's /bin/sh starts
    in turn (which the box must let start too, or /bin/sh refuses: "Failed to exec /bin/bash as variant for /bin/sh")."""
    program = Path(os.path.realpath(program))
    out = [program]
    try:
        with open(program, "rb") as f:
            first = f.readline(512)
    except OSError:
        return tuple(out)
    if first.startswith(b"#!"):
        words = first[2:].decode("utf-8", "replace").split()
        if words:
            interpreter = words[0]
            out.append(Path(os.path.realpath(interpreter)))
            if os.path.basename(interpreter) == "env" and len(words) > 1 and (found := shutil.which(words[1])):
                out.append(Path(os.path.realpath(found)))
            if interpreter == "/bin/sh" and os.path.exists("/private/var/select/sh"):
                out.append(Path(os.path.realpath("/private/var/select/sh")))
    return tuple(dict.fromkeys(out))


def agent_in_box(work: Path, scratch: Path, argv: list[str], read_only: bool, no_commands: bool) -> list[str]:
    """The argv that runs an agent program inside the box, the program started directly, never through a shell: on
    macOS only, since no other system's box holds a list of programs yet. Never unboxed."""
    if inside():
        raise Unavailable(INSIDE)
    if not available():
        raise Unavailable("Jaynes has no box for an agent program on this system yet, and runs none without one")
    program = Path(os.path.realpath(argv[0]))
    writable = (work / ".jaynes", scratch) if read_only else (work, scratch)
    box = Box(writable, readable=(work,), runs=programs_of(program) if no_commands else ())
    return [str(SANDBOX_EXEC), "-p", profile(box), str(program), *argv[1:]]


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
    reserved: tuple[int, ...] = ()  # the ports it was given in JAYNES_PORTS
    local_ports: tuple[int, ...] = ()  # the ports its step declared
    any_port: bool = False  # it ran with every local port open, by the person's choice for the project


def _kill(proc: "subprocess.Popen[bytes]") -> None:
    """Stop a command that ran out of time, with everything it started. One that ended just now has no group left."""
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


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


def reserve(n: int = RESERVED) -> tuple[int, ...]:
    """``n`` local ports free now, none of them the page's."""
    socks = []
    try:
        while len(socks) < n:
            s = socket.socket()
            s.bind(("127.0.0.1", 0))
            if s.getsockname()[1] == PAGE_PORT:
                s.close()
                continue
            socks.append(s)
        return tuple(sorted(s.getsockname()[1] for s in socks))
    finally:
        for s in socks:
            s.close()


def run(project: Path, command: str, *, source: Path | None = None, timeout: float = 600.0,
        env: dict[str, str] | None = None, hidden: tuple[Path, ...] = (), stdin: str | None = None,
        readable: tuple[Path, ...] = (), local_ports: Iterable[int] = (), any_port: bool = False) -> Ran:
    """Run ``command`` from a throwaway copy of ``project``, in the box, with the installed dependencies of
    ``source`` linked, and ``stdin`` on its standard input. ``readable`` names more folders it may read, only read,
    inside the hidden ones, such as the run's own copy of the role a check's script is in. Its servers bind the ports
    in ``JAYNES_PORTS`` and the step's ``local_ports``. Raises ``Unavailable`` where there is no box."""
    with tempfile.TemporaryDirectory(prefix="jaynes-suite-") as tmp:
        work, scratch = Path(tmp) / "project", Path(tmp) / "tmp"
        shutil.copytree(project, work, symlinks=True, ignore=shutil.ignore_patterns(".git", ".jaynes", *DEPENDENCIES))
        scratch.mkdir()
        # what a step installed in the run's copy first, then what the person's own folder has
        link_dependencies(project, work)
        if source is not None:
            link_dependencies(source, work)
        ports, declared = reserve(), tuple(sorted({int(p) for p in local_ports} - {PAGE_PORT}))
        # the run's say, from its setup, in the engine's call: the person opened this project's local ports
        anyone = any_port is True
        box = Box((work, scratch), hidden=hidden, local=True, readable=(project, *readable), reserved=ports, local_ports=declared,
                  any_port=anyone)
        argv = command_in_box(command, box)
        given = {"JAYNES_PORTS": ",".join(map(str, ports))}
        proc = subprocess.Popen(argv, cwd=work, env={**os.environ, **suite_environment(work, scratch), **(env or {}), **given},
                                stdin=subprocess.DEVNULL if stdin is None else subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, start_new_session=True)
        try:
            out, _ = proc.communicate(None if stdin is None else stdin.encode(), timeout=timeout)
            code = proc.returncode
        except subprocess.TimeoutExpired:
            _kill(proc)
            out, _ = proc.communicate()
            out += f"\n(stopped after {timeout:.0f} s)".encode()
            code = 124
        text = out.decode(errors="replace")
        return Ran(code, text[-TAIL:], text, reserved=ports, local_ports=declared, any_port=anyone)


# --- a role's own step ------------------------------------------------------------------------------------------

OUTPUT_MOST = 1_000_000  # bytes of the output a step's script may write


@dataclass(frozen=True)
class Stepped:
    ran: Ran
    output: str | None  # what the script wrote to the file named in JAYNES_OUTPUT; None when it wrote nothing
    unread: str = ""  # why what it wrote there was not read


def _output(path: Path) -> tuple[str | None, str]:
    """What a script left at ``path``, read as data: a plain file of its own, never a link to another (by name or
    by a second name for the same file), of at most ``OUTPUT_MOST`` bytes. The text, or None and why it was not
    read (empty when there is nothing there)."""
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except FileNotFoundError:
        return None, ""
    except OSError:
        return None, "what is at JAYNES_OUTPUT is a link or cannot be opened, and the output is a plain file"
    try:  # what it is, is asked of the open file itself: a folder or a pipe opens too
        found = os.fstat(fd)
        if not stat.S_ISREG(found.st_mode) or found.st_nlink != 1:
            return None, "what is at JAYNES_OUTPUT is not a plain file of its own"
        data = b""
        while len(data) <= OUTPUT_MOST:
            more = os.read(fd, OUTPUT_MOST + 1 - len(data))
            if not more:
                break
            data += more
    finally:
        os.close(fd)
    if len(data) > OUTPUT_MOST:
        return None, f"the output is larger than {OUTPUT_MOST} bytes"
    return data.decode(errors="replace"), ""


def run_step(command: str, *, project: Path | None, write: bool = False, source: Path | None = None,
             ports: tuple[int, ...] = (), timeout: float = 600.0, stdin: str | None = None,
             role: Path | None = None) -> Stepped:
    """Run a role's own step, the shell ``command``, in the box with the reach its step declares, and read the
    output it wrote to the file named in ``JAYNES_OUTPUT``. Raises ``Unavailable`` where there is no box.

    ``project`` None: in an empty folder, seeing nothing of the project. A project it only reads: in a throwaway
    copy, as a check's command runs, so what it writes there is gone. A project it writes: in the run's copy
    itself, where Jaynes then measures what it changed; the copy's ``.git`` stays out of what it may write, since
    Jaynes runs git there later, outside any box, and a hook or a setting left in it would run then. (Where the
    box can only add what may be written and not take a folder out, new entries at the top of the copy are
    refused instead.) No network, and of this machine's ports only ``ports``: no local port is open unless named.
    Of what the engine keeps, it reads only ``role``, the role's own folder, where its script is: never the other
    runs, the record or the page's key.
    """
    with tempfile.TemporaryDirectory(prefix="jaynes-step-") as tmp:
        scratch, out = Path(tmp) / "tmp", Path(tmp) / "out"
        scratch.mkdir()
        out.mkdir()
        readonly: tuple[Path, ...] = ()
        if project is None:
            work = Path(tmp) / "empty"
            work.mkdir()
        elif write:
            work = project
            readonly = (project / ".git",)
        else:
            work = Path(tmp) / "project"
            shutil.copytree(project, work, symlinks=True, ignore=shutil.ignore_patterns(".git", ".jaynes", *DEPENDENCIES))
            link_dependencies(project, work)
            if source is not None:
                link_dependencies(source, work)
        # what it reads is said, so the engine's runs are hidden from it but for its role's folder
        reads = tuple(p for p in (role, None if write or project is None else project) if p is not None) or (scratch,)
        argv = command_in_box(command, Box((work, scratch, out), ports=ports, readonly=readonly, readable=reads))
        env = {**os.environ, **suite_environment(work, scratch), "JAYNES_OUTPUT": str(out / "output.json")}
        proc = subprocess.Popen(argv, cwd=work, env=env, stdin=subprocess.DEVNULL if stdin is None else subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            said, _ = proc.communicate(None if stdin is None else stdin.encode(), timeout=timeout)
            code = proc.returncode
        except subprocess.TimeoutExpired:
            _kill(proc)
            said, _ = proc.communicate()
            said += f"\n(stopped after {timeout:.0f} s)".encode()
            code = 124
        text = said.decode(errors="replace")
        output, unread = _output(out / "output.json")
        return Stepped(Ran(code, text[-TAIL:], text), output, unread)


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


def ports_refused(ran: Ran) -> list[str]:
    """The local ports a refused command names, outside what it was given: ``0`` for a server on a port of its own
    choosing, or one whose refused bind names no port. Read from each refusal and the few lines before it, where a
    traceback shows the call: macOS logs no denial of a sandbox-exec profile, so the output is all there is."""
    allowed = {*ran.reserved, *ran.local_ports}
    lines = ran.printed.splitlines()
    named: list[str] = []
    for i, line in enumerate(lines):
        if "not permitted" not in line.lower() and "eperm" not in line.lower():
            continue
        near = "\n".join(lines[max(0, i - 6):i + 1])
        ports = re.findall(r"(?:localhost|[\d.]+|\[[0-9a-f:]*\]|\*):(\d{1,5})\b", line)
        ports = ports or re.findall(r"\(\s*['\"](?:localhost|[\d.]+|::1?|)['\"]\s*,\s*(\d{1,5})\s*\)", near)
        if not ports and re.search(r"\b(listen|bind)\b", near, re.IGNORECASE):
            ports = ["0"]
        for port in ports:
            if int(port) not in allowed and port not in named:
                named.append(port)
    return named


def refused(ran: Ran) -> list[str]:
    """What the box refused, when the command failed for it: then its failure says nothing about the project."""
    lines = [line.strip() for line in ran.printed.splitlines()
             if ("operation not permitted" in line.lower() or "eperm" in line.lower())
             and not any(h in line for h in HARMLESS)]
    return lines if ran.code != 0 else []


# --- photographs ------------------------------------------------------------------------------------------------

CHROMES = ("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
           "/Applications/Chromium.app/Contents/MacOS/Chromium")
SIZE = (1440, 900)


# the names a Chrome or Chromium goes by on the PATH, as Linux installs them
CHROME_NAMES = ("google-chrome", "google-chrome-stable", "chromium", "chromium-browser")


def chrome() -> str | None:
    """The Chrome Jaynes photographs with: ``JAYNES_CHROME``, the only one looked at when it is set; else one
    installed where macOS keeps apps, or one on the PATH under the names Linux gives it."""
    named = os.environ.get("JAYNES_CHROME", "")
    # a browser named is the only one: when it is not there, none is, and no other is looked for
    if named:
        return named if os.access(named, os.X_OK) else None
    for path in [*CHROMES, *(str(Path.home()) + p for p in CHROMES)]:
        if os.access(path, os.X_OK):
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
    # jaynes_box.py agent WORK SCRATCH [--read-only] [--no-commands] -- PROGRAM ARGS...: the argv that runs an agent
    # program inside the box: writing its step's folder (only Jaynes's exchange in it when the step reads the project)
    # and scratch, reading nothing the engine keeps, reaching no port, and starting only itself without commands
    if len(args) >= 5 and args[0] == "agent" and "--" in args[3:]:
        cut = args.index("--")
        flags, argv = set(args[3:cut]), args[cut + 1:]
        try:
            print(json.dumps(agent_in_box(Path(args[1]), Path(args[2]), argv, "--read-only" in flags, "--no-commands" in flags)))
        except Unavailable as e:
            print(e)
            sys.exit(1)
        sys.exit(0)
    print("usage: jaynes_box.py photograph TARGET OUT [WIDTHxHEIGHT] | agent WORK SCRATCH -- PROGRAM ARGS...")
    sys.exit(2)
