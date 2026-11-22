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
the step or the check that runs them names as ``local_ports`` (a check names its own, never its step's); never the
Jaynes page's, through which an answer counts as the person's. Unix sockets work only in the box's own folders. A suite that serves on port 0 (Go's httptest, supertest,
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
A role's own program has a box of its own, closed by default (``program_profile``): the system's own rules for what
any process needs (``system.sb``, Apple's, private, which may change with macOS: the test of this box runs a sample
program and the escapes, and must keep passing), then only its interpreter and declared commands to run, by the
declared and the real path; its folder, Jaynes's library, its interpreter's folders and the libraries they load
(``program_needs`` works them out with otool, never by hand) and what it declares, to read; its call's temporary
folder to write; its own children to signal, as when it stops one, and nothing outside its box; the metadata of
the paths it walks, link by link, and of nothing else, so it cannot even tell
whether ~/.ssh exists; one socket file, the broker that runs the project's commands for it (macOS opens no box inside
a box), and the local TCP ports it declares. The person's home, the engine's folder and the page's port stay shut,
and a declaration of them is refused before the program starts. A shim of /usr/bin is refused too: it finds its
program through xcrun, which reads the whole disk.
The box is proved on the person's machine before any program runs in it (``program-proof``, at install and when
macOS changes): a sample program does its work and tries twelve escapes in it, and what only an opened app could
try (lsopen, Apple events, the last denial of secrets) is read in the profile; the macOS, the box (this file's
sha256) and the interpreter it held with are kept in the engine's home. With another of the three the proof is made
again first; when it does not hold, no role's program runs, and the refusal says what got through.
Elsewhere there is no box yet, and a project's commands do not run there. Nothing opens them unboxed but the person,
with a proof only they can give, by starting the run unboxed (G18); never a variable.
"""

from __future__ import annotations

import contextlib
import ctypes
import errno
import functools
import hashlib
import json
import os
import re
import select
import shlex
import shutil
import signal
import stat
import socket
import subprocess
import sys
import tempfile
import threading
import time
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
           "Library/Messages", "Library/Application Support/Google", "Library/Application Support/Firefox",
           "Library/Application Support/Chromium", "Library/Containers/com.apple.Safari",
           "Library/Application Support/BraveSoftware", "Library/Application Support/Microsoft Edge",
           "Library/Application Support/Arc", "Library/Application Support/com.operasoftware.Opera",
           "Library/Application Support/Vivaldi", "Library/Application Support/zen", "Library/Application Support/librewolf",
           "Library/Application Support/Waterfox", "Library/Application Support/Floorp", "Library/Application Support/Orion",
           "Library/Application Support/Thorium")
# The person's browsers: no command in a box starts one (the tech-lead's rule, 22/11), only Jaynes's own headless
# Chrome (JAYNES_CHROME) and the one a photograph's box is made for. The agent's settings in agent.rs hide the same
# (BROWSER_APPS, BROWSER_PLACES there; a test checks both say the same).
BROWSER_APPS = ("Google Chrome.app", "Google Chrome Beta.app", "Google Chrome Dev.app", "Google Chrome Canary.app",
                "Chromium.app", "Safari.app", "Safari Technology Preview.app", "Firefox.app",
                "Firefox Developer Edition.app", "Firefox Nightly.app", "Microsoft Edge.app", "Brave Browser.app",
                "Arc.app", "Opera.app", "Vivaldi.app", "Zen.app", "Zen Browser.app", "Orion.app", "LibreWolf.app",
                "Waterfox.app", "Floorp.app", "Thorium.app")
# where macOS keeps apps for everyone, Safari's own place among them (/Applications/Safari.app links there)
BROWSER_PLACES = ("/Applications", "/System/Cryptexes/App/System/Applications",
                  "/System/Volumes/Preboot/Cryptexes/App/System/Applications")


def browsers(home: Path) -> list[Path]:
    """Every place the person's browsers' bundles are kept: for everyone, and in the person's own Applications."""
    return [Path(place) / app for place in (*BROWSER_PLACES, str(home / "Applications")) for app in BROWSER_APPS]
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
    local_ports: tuple[int, ...] = ()  # the ports the step, or the check, that runs the commands names
    any_port: bool = False  # with ``local``: every local TCP port, opened by the person for this project
    # with ``local``: the project's own test servers, let by the person for this project (macOS): its servers bind any
    # local port, a port the system picks among them, and it reaches these, the ones the system gives next
    window: tuple[int, ...] = ()
    readable: tuple[Path, ...] = ()  # folders inside hidden ones it may still read, such as the run's own copy
    home: Path = field(default_factory=Path.home)
    runs: tuple[Path, ...] = ()  # when given, the only programs it may start (an agent program in a step without commands)
    browser: Path | None = None  # the one browser it may start, with its bundle's helpers: a photograph's Chrome
    hide_home: bool = False  # the whole home hidden but for its readable and writable folders: an agent's commands


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
            + runs + list(box.hidden) + ([box.home] if box.hide_home else []))


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


def _browsers_denied(box: Box) -> list[str]:
    """No command starts the person's browsers, nor what their bundles hold (Chrome starts its helpers from inside
    its own). Let again: the browser the box was made for, with its bundle (a photograph's, a role's own script's:
    the Chrome Jaynes photographs with), and a headless Chrome named in ``JAYNES_CHROME`` that is in no browser's
    bundle, as the tests' own is; one named inside a browser's bundle starts only as the box's browser. A box that
    starts only its agent program lets nothing again."""
    held = browsers(box.home)
    lines = [f"(deny process-exec {_subpaths(held)})"]
    if box.runs:
        return lines
    def bundle(named: str) -> Path | None:
        real = Path(os.path.realpath(named))
        return next((Path(os.path.realpath(b)) for b in held if real.is_relative_to(os.path.realpath(b))), None)
    let = []
    headless = os.environ.get("JAYNES_CHROME", "")
    if headless and bundle(headless) is None:
        let.append(Path(os.path.realpath(headless)))
    if box.browser is not None:
        let.append(bundle(str(box.browser)) or Path(os.path.realpath(box.browser)))
    if let:
        lines.append("(allow process-exec " + " ".join(f"(subpath {_lit(p)})" if p.is_dir() else f"(literal {_lit(p)})"
                                                       for p in let) + ")")
    return lines


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
        if box.window:
            # a port the system picks binds only under "localhost:*", which every address and port matches, so the
            # bind is open (the page's port and the free ones of HELD_PORTS are held by `_holding` meanwhile) and what it
            # reaches stays a list
            lines.append('(allow network-bind (local tcp "localhost:*"))')
        for p in sorted({*box.reserved, *box.local_ports} - {PAGE_PORT}):
            if not box.window:
                lines.append(f'(allow network-bind (local tcp "localhost:{int(p)}"))')
            lines.append(f'(allow network-outbound (remote tcp "localhost:{int(p)}"))')
        lines += [f'(allow network-outbound (remote tcp "localhost:{int(p)}"))'
                  for p in sorted(set(box.window) - {*box.reserved, *box.local_ports, PAGE_PORT})]
        lines += [f"(allow network-bind (local unix-socket {_subpaths([p])}))" for p in box.writable]
        lines += _chrome_socket()
        # a suite's own servers on socket files, such as the role tools: only in its own folders, a rule each, since
        # a socket rule with several paths holds only the first
        lines += [f"(allow network-outbound (remote unix-socket {_subpaths([p])}))" for p in box.writable]
    if box.runs:
        lines += ["(deny process-exec)", "(allow process-exec " + " ".join(f"(literal {_lit(p)})" for p in box.runs) + ")"]
    lines += _browsers_denied(box)
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


# --- the box of a role's own program --------------------------------------------------------------------------

# The Jaynes page's port: a role's program never reaches it, even when it declares it.
PROGRAM_NEVER_PORT = 8750


class Refused(Exception):
    """What a program's box will not open, said before the program starts: it never runs with less than it asked."""


@dataclass(frozen=True)
class ProgramBox:
    """The box a role's program runs in, one call at a time, closed by default: it runs its interpreter and the
    commands it declares, reads its own folder, Jaynes's library, its interpreter's and what it declares, writes only
    its call's temporary folder, has no network but one socket file (the broker that runs the project's commands for
    it, since macOS opens no box inside a box) and the local ports it declares. What the engine works out at pinning
    (``program_needs``) goes in ``needs``."""
    role_dir: Path
    library: Path
    tmp: Path
    needs: "Needs"
    broker: Path | None = None  # the one socket file it may reach
    reads: tuple[Path, ...] = ()  # declared: read only
    writes: tuple[Path, ...] = ()  # declared: read and write
    local_ports: tuple[int, ...] = ()  # declared: TCP connect only, never the page's
    project: Path | None = None  # the run's copy, when the step this call serves reaches it
    project_write: bool = False  # and written, when its reach says write
    home: Path = field(default_factory=Path.home)


@dataclass(frozen=True)
class Needs:
    """What a program needs to start at all, worked out from its interpreter and commands, never written by hand."""
    run: tuple[Path, ...]  # executables, by the path declared and by the real one
    run_inside: tuple[Path, ...] = ()  # folders it may execute anything in: a framework Python runs itself there
    read: tuple[Path, ...] = ()  # folders it reads: the interpreter's, its environment's, the libraries it loads


def _shim(path: Path) -> bool:
    """A program of /usr/bin that only finds the real one through xcrun, which reads the whole disk."""
    try:
        out = subprocess.run(["otool", "-L", str(path)], capture_output=True, text=True, timeout=20).stdout
    except (OSError, subprocess.SubprocessError):
        return False
    return "libxcselect" in out


def _libraries(roots: Iterable[Path]) -> dict[Path, str]:
    """The folders of the libraries outside ``roots`` that the binaries inside them load, followed recursively, but
    the system's (which the system's own rules already let read); and each library by the path its binary names, when
    links lead from it to the real one (Homebrew's /opt/homebrew/opt/sqlite is a link into its Cellar): the loader
    follows them, so the box opens their metadata too. Each with what asked for it, to say so when it is refused."""
    roots = [Path(os.path.realpath(r)) for r in roots]
    todo = [f for r in roots for f in r.rglob("*") if f.is_file() and (f.suffix in (".so", ".dylib") or os.access(f, os.X_OK))]
    seen: set[Path] = set()
    out: dict[Path, str] = {}
    while todo:
        f = todo.pop()
        if f in seen:
            continue
        seen.add(f)
        try:
            listed = subprocess.run(["otool", "-L", str(f)], capture_output=True, text=True, timeout=20).stdout.splitlines()[1:]
        except (OSError, subprocess.SubprocessError):
            continue
        for line in listed:
            lib = line.strip().split(" (")[0]
            if not lib.startswith("/") or lib.startswith(("/usr/lib/", "/System/")):
                continue
            real = Path(os.path.realpath(lib))
            if any(r == real or r in real.parents for r in roots) or not real.exists():
                continue
            asked = f"the library {lib}, which {f} loads,"
            out.setdefault(real.parent, asked)
            if Path(lib) != real:
                out.setdefault(Path(lib), asked)
            todo.append(real)
    return dict(sorted(out.items()))


def program_needs(interpreter: Path, commands: Iterable[Path] = (), home: Path | None = None) -> Needs:
    """What a program run by ``interpreter`` and calling ``commands`` needs, from the files themselves: each by the
    path declared and the real one; the interpreter's folder and its environment's (a framework Python runs itself
    inside its folder); the libraries they load outside them (Homebrew's openssl, sqlite). Raises ``Refused`` for a
    shim of /usr/bin, which works only by reading the whole disk: the real program is to be declared; and for what it
    would open that a box never opens (``_never``), as a declaration of it is refused, naming what asked for it."""
    home = Path(home) if home is not None else Path.home()
    commands = list(commands)
    run, inside = [], []
    read: dict[Path, str] = {}
    for exe in [interpreter, *commands]:
        real = Path(os.path.realpath(exe))
        if not real.is_file() or not os.access(real, os.X_OK):
            raise Refused(f"{exe} is not a program on this machine")
        if _shim(real):
            raise Refused(f"{exe} is a shim that finds its program through xcrun, which reads the whole disk; declare the "
                          "real program instead (xcrun --find NAME says where it is)")
        held = next((b for b in browsers(home) if real.is_relative_to(os.path.realpath(b))), None)
        if held is not None:
            raise Refused(f"{exe} is in {held}, one of the person's browsers, which no box starts; Jaynes's own "
                          "photograph (jaynes_box.py photograph) takes pictures of pages")
        run += [Path(exe), real]
    prefix = subprocess.run([str(interpreter), "-c", "import sys; print(sys.prefix); print(sys.base_prefix)"],
                            capture_output=True, text=True, timeout=60)
    if prefix.returncode != 0:
        raise Refused(f"{interpreter} did not say where it lives: {prefix.stderr.strip()[-200:]}")
    for line in prefix.stdout.splitlines():
        if not line:
            continue
        real = Path(os.path.realpath(line))
        read.setdefault(real, f"the interpreter {interpreter}, whose folder it is,")
        if ".framework" in str(real) and real not in inside:
            inside.append(real)
    for c in commands:
        read.setdefault(Path(os.path.realpath(c)).parent, f"the command {c}, whose folder it is,")

    def refuse_never():
        for p, asked in read.items():
            if _never(p, home):
                raise Refused(f"{asked} would open {p}, which is {NEVER}; put it elsewhere, or declare one that lives elsewhere")

    # before the libraries are looked for: that walks every folder found so far
    refuse_never()
    for p, asked in _libraries(read).items():
        read.setdefault(p, asked)
    refuse_never()
    return Needs(tuple(dict.fromkeys(run)), tuple(inside), tuple(read))


def _chain(path: Path | str) -> list[str]:
    """Every path a lookup of ``path`` passes through, each link followed: the only places whose metadata a box
    closed by default lets read, so that nothing else, not even whether ~/.ssh/known_hosts exists, shows."""
    out: list[str] = []
    todo, seen = [os.path.abspath(path)], set()
    while todo:
        p = todo.pop()
        parts = p.split("/")
        cur = "/"
        for i, part in enumerate(parts[1:], 1):
            if not part:
                continue
            cur = os.path.join(cur, part)
            if cur in seen:
                continue
            seen.add(cur)
            out.append(cur)
            if os.path.islink(cur):
                target = os.readlink(cur)
                target = target if target.startswith("/") else os.path.join(os.path.dirname(cur), target)
                rest = "/".join(parts[i + 1:])
                todo.append(os.path.normpath(os.path.join(target, rest) if rest else target))
                break
    return out


def _literals(paths: Iterable[str]) -> str:
    return " ".join('(literal "' + p.replace("\\", "\\\\").replace('"', '\\"') + '")' for p in paths)


def _never(path: Path | str, home: Path) -> bool:
    """Whether a program's box never opens ``path``: the person's home or what holds it, a place secrets live, inside
    it or what holds it, the engine's folder, inside it or what holds it. One rule for what is declared and for what is
    worked out from the program's interpreter and commands."""
    real = Path(os.path.realpath(path))
    engine = Path(os.path.realpath(engine_home()))
    home = Path(os.path.realpath(home))
    holds = lambda inner: real == inner or real in inner.parents  # noqa: E731
    within = lambda outer: real == outer or outer in real.parents  # noqa: E731
    return holds(home) or holds(engine) or within(engine) or any(holds(x) or within(x) for x in (home / s for s in SECRETS))


NEVER = "the person's home, a folder that holds it, a place secrets live or the engine's folder: a role's program never reaches it"


def _declared(paths: Iterable[Path], home: Path) -> list[Path]:
    """The declared folders a program's box opens. Raises ``Refused`` for one it never opens (``_never``)."""
    out = []
    for p in paths:
        if _never(p, home):
            raise Refused(f"{p} is {NEVER}")
        out.append(Path(os.path.realpath(p)))
    return out


def program_profile(box: ProgramBox) -> str:
    """The sandbox-exec profile for a role's program: closed by default, with the system's own rules for what any
    process needs, and only what the program and its call need opened. The last rule that matches wins."""
    reads, writes = _declared(box.reads, box.home), _declared(box.writes, box.home)
    for p in [*box.needs.read, *box.needs.run_inside]:
        # the needs were worked out at pinning and kept: checked again, as what is declared is
        if _never(p, box.home):
            raise Refused(f"the program's needs name {p}, which is {NEVER}")
    project = [box.project] if box.project is not None else []
    writable = [box.tmp, *writes, *(project if box.project_write else [])]
    readable = [box.role_dir, box.library, *box.needs.read, *reads, *project]
    # it may stop its own children (a command run with a timeout), never a process outside its box
    lines = ["(version 1)", "(deny default)", '(import "system.sb")', "(allow process-fork)", "(allow signal (target same-sandbox))",
             f"(allow process-exec {_literals(dict.fromkeys(str(p) for p in box.needs.run))})"]
    if box.needs.run_inside:
        lines.append(f"(allow process-exec {_subpaths(box.needs.run_inside)})")
    lines += [f"(allow file-read* {_subpaths(readable)})",
              f"(allow file-read* file-write* {_subpaths(writable)})",
              f"(allow file-read* file-write* {' '.join(DEVICES)})"]
    walked = dict.fromkeys(q for p in [*readable, *writable, *box.needs.run, *([box.broker] if box.broker else [])]
                           for q in _chain(p))
    lines.append(f"(allow file-read-metadata {_literals(walked)})")
    if box.broker is not None:
        lines.append(f"(allow network-outbound (remote unix-socket (literal {_lit(box.broker)})))")
    lines += [f'(allow network-outbound (remote tcp "localhost:{int(p)}"))'
              for p in sorted(set(box.local_ports) - {PROGRAM_NEVER_PORT})]
    # what is secret stays out even where a declared folder or the role's own reaches it
    lines.append(f"(deny file-read* {_subpaths([box.home / s for s in SECRETS] + [engine_home() / f for f in ENGINE_SECRETS])})")
    lines += ["(deny lsopen)", "(deny appleevent-send)"]
    return "\n".join(lines) + "\n"


# --- the proof of a program's box on this machine --------------------------------------------------------------

# Where the engine keeps the proof that the program's box holds on this Mac, and on which macOS it was made.
PROOF = "program-box.json"

# A sample program: what a role's program does, then what it must not. Each line prints "work:" or "escape:" with
# what came of it; the proof holds when every work is ok and every escape is refused.
SAMPLE = r'''
import json, os, socket, sqlite3, ssl, subprocess, sys, tempfile, threading
a = json.loads(sys.argv[1])
got = {}
def try_(name, f):
    try: got[name] = f() or "ok"
    except Exception as e: got[name] = type(e).__name__
try_("work: sqlite, ssl, a thread", lambda: (sqlite3.connect(":memory:").execute("select 1").fetchone(), ssl.create_default_context(),
     threading.Thread(target=lambda: None).start()) and "ok")
try_("work: a temporary file", lambda: tempfile.NamedTemporaryFile(dir=os.environ["TMPDIR"]).close())
try_("work: its own folder", lambda: open(os.path.join(a["role_dir"], "role.toml")).read() and "ok")
try_("work: a child of itself", lambda: "ok" if subprocess.run([sys.executable, "-c", "print(1)"], capture_output=True).returncode == 0 else "failed")
def stop_a_child():
    try:
        subprocess.run([sys.executable, "-c", "import time; time.sleep(30)"], timeout=1)
        return "it was not stopped"
    except subprocess.TimeoutExpired:
        return "ok"
try_("work: stop a child of itself (a timeout)", stop_a_child)
s = socket.socket(socket.AF_UNIX)
try_("work: the broker", lambda: s.connect(a["broker"]))
home = a["real_home"]
try_("escape: list the person's home", lambda: os.listdir(home) and "listed")
try_("escape: know whether ~/.ssh exists", lambda: "knows" if os.path.exists(os.path.join(home, ".ssh")) else "PermissionError")
try_("escape: read the engine's folder", lambda: os.listdir(a["engine"]) and "listed")
# secrets planted inside a folder it may read: the rule that denies them again, last, is what keeps them out
try_("escape: an ssh key inside a folder it reads", lambda: open(os.path.join(a["role_dir"], ".ssh", "id")).read() and "read")
try_("escape: cloud credentials inside a folder it reads", lambda: open(os.path.join(a["role_dir"], ".aws", "credentials")).read() and "read")
try_("escape: write outside its call", lambda: open(os.path.join(a["role_dir"], "x"), "w").write("x") and "wrote")
try_("escape: run a program it did not declare", lambda: subprocess.run(["/bin/ls", "/"], capture_output=True) and "ran")
t = socket.socket(); t.settimeout(3)
try_("escape: a local port", lambda: t.connect(("127.0.0.1", a["port"])) or "reached")
u = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
try_("escape: UDP", lambda: u.sendto(b"", ("127.0.0.1", 9)) and "sent")
b = socket.socket()
try_("escape: bind", lambda: b.bind(("127.0.0.1", 0)) or "bound")
try_("escape: a name on the network", lambda: socket.getaddrinfo("example.com", 80) and "resolved")
try_("escape: signal a process outside its box", lambda: os.kill(a["outside"], 0) or "signalled")
print(json.dumps(got))
'''


def macos() -> str:
    """This macOS, by version and build: Apple's own rules (system.sb) change with it."""
    out = []
    for flag in ("-productVersion", "-buildVersion"):
        try:
            out.append(subprocess.run(["sw_vers", flag], capture_output=True, text=True, timeout=10).stdout.strip())
        except (OSError, subprocess.SubprocessError):
            out.append("")
    return " ".join(out)


# The rules a live probe cannot try without opening an app or asking the person to let one be controlled: the proof
# reads them in the profile instead, and fails when one is missing.
PROFILE_MUST = ("(deny default)", "(deny lsopen)", "(deny appleevent-send)")


def _profile_lacks(prof: str, box: ProgramBox) -> list[str]:
    lacks = [f"the profile lacks {rule}, which no probe can try without opening an app" for rule in PROFILE_MUST
             if rule not in prof.splitlines()]
    secrets = [_lit(box.home / s) for s in SECRETS] + [_lit(engine_home() / f) for f in ENGINE_SECRETS]
    denied = [line for line in prof.splitlines() if line.startswith("(deny file-read*")]
    if not denied or any(x not in denied[-1] for x in secrets):
        lacks.append("the profile does not deny, last, every place secrets live")
    return lacks


def program_selftest(interpreter: Path, make=None) -> list[str]:
    """Run the sample program in a program's box as ``make`` (program_profile) writes it, on this machine: what
    failed, each in a sentence; nothing when the box holds. What only an app opened could try is read in the profile
    itself. The program is the engine's own, in a folder of its own; nothing of the person's is read or written, no app
    is opened, the pasteboard and the keychain are not touched."""
    make = make or program_profile
    import socket as _socket
    with tempfile.TemporaryDirectory(prefix="jaynes-program-proof-") as d:
        base = Path(os.path.realpath(d))
        role_dir, tmp, fake_engine = base / "role", base / "tmp", base / "engine"
        for p in (role_dir, tmp, fake_engine):
            p.mkdir()
        (role_dir / "role.toml").write_text("role")
        (role_dir / "sample.py").write_text(SAMPLE)
        # the role's folder stands for a home here, with secrets where secrets live
        for secret in (".ssh/id", ".aws/credentials"):
            (role_dir / secret).parent.mkdir()
            (role_dir / secret).write_text("secret")
        (fake_engine / "page.key").write_text("secret")
        broker = tmp / "broker.sock"
        listener = _socket.socket(_socket.AF_UNIX)
        listener.bind(str(broker))
        listener.listen()
        undeclared = _socket.socket()
        undeclared.bind(("127.0.0.1", 0))
        undeclared.listen()
        try:
            needs = program_needs(interpreter)
            box = ProgramBox(role_dir, role_dir, tmp, needs, broker=broker, home=role_dir)
            prof = make(box)
            args = json.dumps({"role_dir": str(role_dir), "broker": str(broker), "real_home": str(Path.home()),
                               "engine": str(fake_engine), "port": undeclared.getsockname()[1], "outside": os.getpid()})
            env = {"PATH": "/usr/bin:/bin", "TMPDIR": str(tmp), "HOME": str(tmp), "LANG": "en_US.UTF-8", "PYTHONDONTWRITEBYTECODE": "1"}
            ran = subprocess.run([str(SANDBOX_EXEC), "-p", prof, str(interpreter), str(role_dir / "sample.py"), args],
                                 capture_output=True, text=True, timeout=120, env=env, cwd=role_dir)
        except Refused as e:
            return [f"the box could not be made: {e}"]
        except (OSError, subprocess.SubprocessError) as e:
            return [f"the sample program did not run: {e}"]
        finally:
            listener.close()
            undeclared.close()
        try:
            got = json.loads(ran.stdout.strip().splitlines()[-1])
        except (ValueError, IndexError):
            return [f"the sample program said nothing it could be judged by: {ran.stderr.strip()[-300:]}"]
        wrong = [f"{k}: {v}" for k, v in got.items()
                 if (k.startswith("work:") and v != "ok") or (k.startswith("escape:") and v not in ("PermissionError", "gaierror"))]
        wrong += _profile_lacks(prof, box)
        return wrong or ([] if len(got) == 18 else [f"the sample program judged {len(got)} things, not 18"])


def program_proof(interpreter: Path, again: bool = False) -> dict:
    """The proof that a program's box holds on this macOS, kept in the engine's home and made again when macOS, the
    box (this file, which writes the profile: a jaynes update may change it) or the interpreter changed, or ``again``:
    {"macos", "box", "interpreter", "at", "holds", "wrong"}."""
    file = engine_home() / PROOF
    now = {"macos": macos(), "box": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
           "interpreter": os.path.realpath(interpreter)}
    if not again:
        try:
            kept = json.loads(file.read_text())
            if all(kept.get(k) == v for k, v in now.items()):
                return kept
        except (OSError, ValueError):
            pass
    wrong = program_selftest(interpreter)
    proof = {**now, "at": int(__import__("time").time()), "holds": not wrong, "wrong": wrong}
    file.parent.mkdir(parents=True, exist_ok=True)
    tmp = file.with_name(f".{PROOF}.{os.getpid()}")
    tmp.write_text(json.dumps(proof, indent=1))
    tmp.replace(file)
    return proof


def program_box_holds(interpreter: Path) -> None:
    """Raises ``Refused`` unless the program's box was proved on this macOS: the box stays shut, and says why."""
    proof = program_proof(interpreter)
    if not proof.get("holds"):
        raise Refused(f"the box of a role's program did not hold on this macOS ({proof.get('macos')}), so no role's "
                      "program runs: " + "; ".join(proof.get("wrong") or ["no proof"]))


def command_in_box(script: str, box: Box) -> list[str]:
    """The argv that runs the shell ``script`` inside ``box``. Inside another program's sandbox it refuses."""
    if inside():
        raise Unavailable(INSIDE)
    if available():
        return [str(SANDBOX_EXEC), "-p", profile(box), "/bin/sh", "-c", script]
    raise Unavailable("Jaynes has no box for project commands on this system yet, so it runs none of them: run Jaynes "
                      "on macOS, where the box is, or wait for this system's box")


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


# The engine lays out the state a check judges beside the run's copy, as .COPY-judged-PID-N: its files, with the
# packages a step installed in the copy linked from the copy.
LAID_OUT = "-judged-"


def own_dependencies(project: Path, linked: list[str]) -> tuple[Path, ...]:
    """Where the dependency folders ``linked`` from ``project`` really are, for a box to read them, when ``project``
    is a state the engine laid out beside a run's copy and they are inside that copy. A link that leads anywhere
    else adds nothing the box may read: the copy is the agent's to write, and it may point one at ~/.ssh or at the
    run's own folders, where hidden steps keep what the agent never sees."""
    name = project.name
    if not name.startswith(".") or LAID_OUT not in name:
        return ()
    copy = (project.parent / name[1:].rsplit(LAID_OUT, 1)[0]).resolve()
    out = []
    for rel in linked:
        real = (project / rel).resolve()
        if (real == copy or copy in real.parents) and real not in out:
            out.append(real)
    return tuple(out)


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
    if not isinstance(script, str) or not script.strip() or NPM_DEFAULT in script or noop([script]):
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


MAKEFILES = ("Makefile", "GNUmakefile", "makefile")


def _make(root: Path) -> Runner | None:
    for name in MAKEFILES:
        path = root / name
        if path.is_file():
            target = make_target(path.read_text(errors="replace"), "test")
            if target is None or not target[0] and noop(target[1]):
                return None
            return Runner(kind="make", suite="make test", each="", found=f"{name} test target")
    return None


# A rule: its targets, then : or :: and what it needs; never an assignment (test := unit, test ::= unit, test != x).
RULE = re.compile(r"^(?P<targets>[^\s#:=][^:=]*?)\s*::?(?!:*=)(?P<rest>.*)$")
INCLUDE = re.compile(r"^\s*-?s?include\s", re.M)


def make_target(text: str, name: str) -> tuple[list[str], list[str]] | None:
    """What a Makefile's rules for the target ``name`` need and run: its prerequisites and its recipe's lines, or
    None when no rule names it. A target named only in a comment, or a variable of that name, is no rule. A Makefile
    that includes another and gives the target no recipe of its own may run anything: it needs ``include``."""
    lines = re.sub(r"\\\n", " ", text).split("\n")
    found, needs, recipe, inside = False, [], [], False
    for line in lines:
        if line.startswith("\t"):
            if inside:
                recipe.append(line[1:])
            continue
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        inside = False
        m = RULE.match(line)
        if not m or name not in m.group("targets").split():
            continue
        rest = m.group("rest").split("#", 1)[0]
        prerequisites, _, now = rest.partition(";")
        if "=" in prerequisites:  # a variable for the target, test: X = 1, is no rule of it
            continue
        found, inside = True, True
        needs += [p for p in prerequisites.split() if p != "|"]
        if now.strip():
            recipe.append(now)
    if not found:
        return None
    if not recipe and INCLUDE.search(text):
        needs.append("include")
    return needs, recipe


# What runs nothing that could fail: a command that only prints or only succeeds.
NOTHING = re.compile(r"(true|/bin/true|/usr/bin/true|:|exit(\s+0)?|(echo|printf)(\s.*)?)")


def noop(commands: list[str]) -> bool:
    """Whether ``commands``, a recipe's lines or a script's, run nothing that could fail: each is empty, true, :,
    exit 0, or only prints (echo, printf), joined by ; or &&. Anything the shell or make would expand first, $(...),
    ${...} or backquotes, may run something, and so does any other command."""
    for line in commands:
        line = line.strip().lstrip("@-+").strip()
        if "$(" in line or "${" in line or "`" in line or "|" in line.replace("||", ""):
            return False
        for piece in re.split(r";|&&|\|\|", line):
            if piece.strip() and not NOTHING.fullmatch(piece.strip()):
                return False
    return True


def runs_nothing(root: Path, runner: Runner) -> str | None:
    """What makes the suite ``runner`` found run nothing that could fail, as the project at ``root`` now stands, or
    None: a test target or test script that only succeeds or prints, or that is gone. Told only of a make test target
    and a package.json test script, the suites whose text says what they run."""
    if runner.kind == "make":
        name = next((n for n in MAKEFILES if (root / n).is_file()), None)
        target = make_target((root / name).read_text(errors="replace"), "test") if name else None
        if target is None:
            return "the project has no make test target any more"
        needs, recipe = target
        if not needs and noop(recipe):
            return f"make test runs only {'; '.join(r.strip().lstrip('@-+') for r in recipe if r.strip()) or 'nothing'}"
    if runner.kind == "node":
        try:
            package = json.loads((root / "package.json").read_text())
            scripts = package.get("scripts") or {}
        except (OSError, ValueError, AttributeError):
            return None
        script = scripts.get("test") if isinstance(scripts, dict) else None
        if isinstance(script, str) and noop([script]) and not any(isinstance(scripts.get(k), str) and not noop([scripts[k]])
                                                                for k in ("pretest", "posttest")):
            return f"the test script runs only {script.strip() or 'nothing'}"
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
    test_servers: bool = False  # it ran with the project's own test servers let, by the person's choice for the project
    held: tuple[int, ...] = ()  # the ports of HELD_PORTS Jaynes held while it ran
    window: tuple[int, ...] = ()  # the ports macOS gives next that its own test servers could reach (servers_window)


def _kill(proc: "subprocess.Popen[bytes]") -> None:
    """Stop a command in the box with everything it started in its group. One that ended with nothing left has no
    group left, or (on macOS, while it is not reaped) a group of its own exit alone, which refuses the signal."""
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        pass


# Where the engine has a command in the box note its process group while it runs (a file named by the number), so a
# halt stops it: the command runs in a session of its own, which the engine's stop of the role's group never reaches.
GROUPS = "JAYNES_BOX_GROUPS"


def _note_group(pid: int) -> Path | None:
    """Note a command's group where the engine looks on a halt; None when the engine names no place."""
    named = os.environ.get(GROUPS)
    if not named:
        return None
    folder = Path(named)
    try:
        folder.mkdir(parents=True, exist_ok=True)
        note = folder / str(pid)
        tmp = folder / f".{pid}.tmp"
        tmp.write_text(str(pid))
        tmp.replace(note)
        return note
    except OSError:
        return None


def _exits(pid: int, timeout: float) -> bool:
    """Wait until ``pid`` exits, without reaping it: True when it did within ``timeout``. macOS's Python has no
    waitid, so there a kqueue says when the process ends."""
    if hasattr(select, "kqueue"):
        kq = select.kqueue()
        try:
            ev = select.kevent(pid, filter=select.KQ_FILTER_PROC, flags=select.KQ_EV_ADD | select.KQ_EV_ONESHOT,
                               fflags=select.KQ_NOTE_EXIT)
            try:
                got = kq.control([ev], 1, timeout)
            except ProcessLookupError:
                return True  # it had ended before it could be watched
            if got and got[0].flags & select.KQ_EV_ERROR:
                return got[0].data == errno.ESRCH
            return bool(got)
        finally:
            kq.close()
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if os.waitid(os.P_PID, pid, os.WEXITED | os.WNOWAIT | os.WNOHANG) is not None:
            return True
        time.sleep(0.02)
    return False


def _wait_in_box(argv: list[str], *, cwd: Path | None, env: dict[str, str], stdin: str | None, timeout: float,
                 stderr_apart: bool = False) -> tuple[int, bytes, bytes, bool]:
    """Run ``argv`` (a command in the box) as a group of its own and wait for it: its code, what it printed (stdout,
    and stderr when apart), and whether it ran out of time. While it runs its group is noted for a halt; when it
    ends, whatever it left running in its group is stopped, as when it runs out of time. It is waited for without
    being reaped, so its number cannot name another group when the group is stopped; what it left holding its
    output in a session of its own no longer keeps the call waiting."""
    proc = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.DEVNULL if stdin is None else subprocess.PIPE,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE if stderr_apart else subprocess.STDOUT,
                            start_new_session=True)
    note = _note_group(proc.pid)
    chunks: dict[str, list[bytes]] = {"out": [], "err": []}

    def pump(pipe, into):
        for block in iter(lambda: pipe.read(65536), b""):
            into.append(block)

    readers = [threading.Thread(target=pump, args=(proc.stdout, chunks["out"]), daemon=True)]
    if stderr_apart:
        readers.append(threading.Thread(target=pump, args=(proc.stderr, chunks["err"]), daemon=True))
    for r in readers:
        r.start()
    if stdin is not None:
        def feed():
            try:
                proc.stdin.write(stdin.encode())
                proc.stdin.close()
            except (BrokenPipeError, OSError):
                pass
        readers.append(threading.Thread(target=feed, daemon=True))
        readers[-1].start()
    try:
        # exited, not reaped: its pid, and so its group's number, stays its own until wait()
        timed_out = not _exits(proc.pid, timeout)
    finally:
        _kill(proc)  # the whole group: what it left running ends with it
        proc.wait()
        if note is not None:
            note.unlink(missing_ok=True)
    for r in readers:
        r.join(timeout=2)  # what holds the pipe in a session of its own keeps nobody waiting
    code = 124 if timed_out else proc.returncode
    return code, b"".join(chunks["out"]), b"".join(chunks["err"]), timed_out


# git's variables that point it at a repository, an index or objects of its own (`git rev-parse --local-env-vars`), as
# the engine's GIT_LOCAL_ENV: inherited from whoever started the run, a project's commands would read and write that
# repository in place of the copy
GIT_LOCAL = ("GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_CONFIG", "GIT_CONFIG_PARAMETERS", "GIT_CONFIG_COUNT",
             "GIT_OBJECT_DIRECTORY", "GIT_DIR", "GIT_WORK_TREE", "GIT_IMPLICIT_WORK_TREE", "GIT_GRAFT_FILE", "GIT_INDEX_FILE",
             "GIT_NO_REPLACE_OBJECTS", "GIT_REPLACE_REF_BASE", "GIT_PREFIX", "GIT_SHALLOW_FILE", "GIT_COMMON_DIR",
             "GIT_INTERNAL_SUPER_PREFIX")


def inherited() -> dict[str, str]:
    """This process's environment without git's variables of another repository (``GIT_LOCAL``)."""
    return {k: v for k, v in os.environ.items()
            if k not in GIT_LOCAL and not k.startswith(("GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_"))}


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


# How many of the ports macOS gives next a project's own test servers may reach: a server falls outside only if the
# machine takes more than these while one command runs (here about 500 a minute), and it is then refused, never let
# through. 4,000 rules cost the profile about 0.3 s and stay far below where sandbox-exec breaks (about 16,000).
SERVERS_WINDOW = 4000


def _held(port: int) -> bool:
    """Whether something on this machine listens on ``port`` (or holds it) on a loopback or every-address address."""
    for family, address in ((socket.AF_INET, "127.0.0.1"), (socket.AF_INET6, "::1"), (socket.AF_INET, "0.0.0.0"), (socket.AF_INET6, "::")):
        try:
            s = socket.socket(family, socket.SOCK_STREAM)
        except OSError:
            continue
        try:
            if family == socket.AF_INET6:
                s.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
            s.bind((address, port))
        except OSError as e:
            if e.errno == errno.EADDRINUSE:
                return True
        finally:
            s.close()
    return False


def _system_ports() -> tuple[int, int, bool] | None:
    """The ports macOS gives a server that asks for any (first, last), and whether it gives them at random."""
    try:
        said = subprocess.run(["/usr/sbin/sysctl", "-n", "net.inet.tcp.randomize_ports", "net.inet.ip.portrange.hifirst",
                               "net.inet.ip.portrange.hilast"], capture_output=True, text=True, timeout=10).stdout.split()
        at_random, first, last = (int(x) for x in said)
    except (OSError, ValueError, subprocess.SubprocessError):
        return None
    return first, last, at_random != 0


def servers_window() -> tuple[int, ...]:
    """The ports a project's own test servers get from macOS while one command runs: macOS gives a port the system
    picks in sequence (``net.inet.tcp.randomize_ports`` 0), the same sequence for IPv4 and IPv6 and for every
    process, so the next ``SERVERS_WINDOW`` after the one it gives now, without the ones something listens on now.
    None where macOS gives them at random: the box then stays closed."""
    system = _system_ports()
    if system is None:
        return ()
    first, last, at_random = system
    if at_random or not 0 < first <= last <= 65535:
        return ()
    s = socket.socket()
    try:
        s.bind(("127.0.0.1", 0))
        given = s.getsockname()[1]
    finally:
        s.close()
    return _window_now(given, first, last)


def _window_from(given: int, first: int, last: int, listening: frozenset[int] | None) -> tuple[int, ...]:
    """The ``SERVERS_WINDOW`` ports macOS gives after ``given`` in its range ``first``..``last``, without the page's
    port and the ones a server listens on (``listening``; None when that could not be read: then every port a bind
    finds taken). Only a listening port is left out: a client's end of a connection, or one closing, holds its port
    only until it closes, and macOS then gives it to the next server that asks, a test's among them (measured 22/11:
    of 20 ports a bind found taken in the window, 18 were such ends)."""
    span = last - first + 1
    after = (first + (given - first + i) % span for i in range(1, min(SERVERS_WINDOW, span) + 1))
    taken = listening if listening is not None else frozenset(p for p in range(first, last + 1) if _held(p))
    return tuple(p for p in after if p != PAGE_PORT and p not in taken)


def _listening() -> frozenset[int] | None:
    """The TCP ports a server listens on now on this machine, on any address: from netstat, every process's, the
    person's too, with no privilege; where netstat lists no TCP socket at all, from lsof, the person's own
    processes' (netstat run from some Pythons, Homebrew's 3.14 among them, lists none: measured 22/11); None when
    neither could say."""
    listening = _netstat_listening()
    return listening if listening is not None else _lsof_listening()


def _netstat_listening() -> frozenset[int] | None:
    """The TCP ports every process listens on, from netstat; None where it lists no TCP socket at all."""
    try:
        said = subprocess.run(["/usr/sbin/netstat", "-an", "-p", "tcp"], capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.SubprocessError):
        return None
    if said.returncode != 0 or "Active Internet connections" not in said.stdout:
        return None
    ports = set()
    for line in said.stdout.splitlines():
        fields = line.split()
        if len(fields) >= 6 and fields[0].startswith("tcp") and fields[-1] == "LISTEN":
            port = fields[3].rsplit(".", 1)[-1]
            if port.isdigit():
                ports.add(int(port))
    return frozenset(ports)


def _lsof_listening() -> frozenset[int] | None:
    """The TCP ports the person's own processes listen on, from lsof; None when it could not say."""
    try:
        said = subprocess.run(["/usr/sbin/lsof", "-nP", "-iTCP", "-sTCP:LISTEN", "-F", "n"], capture_output=True,
                              text=True, timeout=10)
    except (OSError, subprocess.SubprocessError):
        return None
    # lsof says 1 when it found nothing to list, which is an answer too
    if said.returncode not in (0, 1) or (said.returncode == 1 and said.stderr.strip()):
        return None
    return frozenset(int(port) for line in said.stdout.splitlines() if line.startswith("n")
                     for port in [line.rsplit(":", 1)[-1]] if port.isdigit())


def _answering(ports: tuple[int, ...]) -> frozenset[int]:
    """Of ``ports``, the ones a server answers on, on this machine's loopback (IPv4 or IPv6): a connection to each,
    closed at once, with nothing sent. Every connection leaves from one end, taken once for each address and reused:
    one end each would move the sequence macOS gives the tests' servers their ports from by one a port (measured
    22/11: 1,004 for 500 ports, which put a test's server past the window)."""
    found = set()
    for family, address in ((socket.AF_INET, "127.0.0.1"), (socket.AF_INET6, "::1")):
        end = 0
        for port in ports:
            if port in found:
                continue
            try:
                s = socket.socket(family, socket.SOCK_STREAM)
            except OSError:
                break
            try:
                s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
                if family == socket.AF_INET6:
                    s.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
                s.bind((address, end))
                end = s.getsockname()[1]
                s.settimeout(0.2)
                if s.connect_ex((address, port)) == 0:
                    found.add(port)
            except OSError:
                pass
            finally:
                s.close()
    return frozenset(found)


def _window_now(given: int, first: int, last: int) -> tuple[int, ...]:
    """The window after ``given``, without the ports a server listens on now. Where only lsof lists them, which sees
    the person's own processes alone, a server of root's or of another user's there is found by connecting to each
    port of the window that a bind finds taken (``_answering``)."""
    listening = _netstat_listening()
    if listening is not None:
        return _window_from(given, first, last, listening)
    seen = _lsof_listening()
    window = _window_from(given, first, last, seen)
    if seen is None:
        return window
    # only a port a bind finds taken can have a server on it: a connection goes to those alone (a bind takes no
    # port of the sequence; 0.17 s for the window's 4,000, measured 22/11)
    found = _answering(tuple(p for p in window if _held(p)))
    return tuple(p for p in window if p not in found)


# The ports of the usual databases, caches and queues, which keep data or passwords: while a command with the project's
# own test servers runs, Jaynes holds the free ones (`_holding`), so a test server cannot take one and answer in place
# of the person's service while it is not running. The ports a test brings its own app up on (3000, 5173, 8000,
# 8080...) stay out: a test that serves there is common. The card names them, from the same list (listing.rs
# HELD_PORTS), in this order, the best known first.
HELD_PORTS = {
    5432: "database",  # PostgreSQL
    3306: "database",  # MySQL, MariaDB
    6379: "cache",  # Redis, Valkey
    27017: "database",  # MongoDB
    11211: "cache",  # Memcached
    9200: "database",  # Elasticsearch, OpenSearch
    5672: "queue",  # RabbitMQ
    9092: "queue",  # Kafka
    1433: "database",  # SQL Server
    1521: "database",  # Oracle
    26257: "database",  # CockroachDB
    9042: "database",  # Cassandra, ScyllaDB
    5984: "database",  # CouchDB
    7687: "database",  # Neo4j
    4222: "queue",  # NATS
    2379: "database",  # etcd
    8200: "secrets",  # Vault
}

_LOOPBACK = ((socket.AF_INET, "127.0.0.1"), (socket.AF_INET6, "::1"))
_EVERYWHERE = _LOOPBACK + ((socket.AF_INET, "0.0.0.0"), (socket.AF_INET6, "::"))


class _holding:
    """While a command with its own test servers runs, Jaynes holds the page's port on 127.0.0.1 and ::1 where the page
    does not (the page down), and each of ``ports`` on 127.0.0.1, ::1, 0.0.0.0 and ::, and closes what comes: the box
    binds any port then, and a bind it is denied does not hold in sandbox-exec. Every socket takes SO_REUSEADDR, so a
    test server's bind with it, as Go's, is refused on every address too (measured 22/11, probe8); the every-address
    ones are bound and never listen, so the firewall asks nothing. A port that cannot be held on one of its addresses
    is not held on any, since what holds it there is someone's. What the page or another program holds stays theirs.
    ``held``: the ones of ``ports`` it holds."""

    def __init__(self, ports: tuple[int, ...] = ()) -> None:
        self.ports = ports
        self.held: tuple[int, ...] = ()
        self.socks: list[socket.socket] = []
        self.listening: list[socket.socket] = []
        self._done = threading.Event()
        self._watch: threading.Thread | None = None

    @staticmethod
    def _bind(family: int, address: str, port: int, listen: bool) -> socket.socket | None | bool:
        """A socket bound to ``address``:``port`` (listening when ``listen``), False when another holds it, None when
        this machine has no such address family."""
        try:
            s = socket.socket(family, socket.SOCK_STREAM)
        except OSError:
            return None
        try:
            s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            if family == socket.AF_INET6:
                s.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
            s.bind((address, port))
            if listen:
                s.listen(16)
                s.setblocking(False)
            return s
        except OSError:
            s.close()
            return False

    def __enter__(self) -> "_holding":
        for family, address in _LOOPBACK:
            s = self._bind(family, address, PAGE_PORT, True)
            if isinstance(s, socket.socket):
                self.socks.append(s)
                self.listening.append(s)
        held = []
        for port in self.ports:
            got: list[socket.socket] = []
            for family, address in _EVERYWHERE:
                s = self._bind(family, address, port, (family, address) in _LOOPBACK)
                if s is False:
                    break
                if s is not None:
                    got.append(s)
            else:
                self.socks += got
                self.listening += [g for g, (f, a) in zip(got, _EVERYWHERE) if (f, a) in _LOOPBACK]
                held.append(port)
                continue
            for g in got:
                g.close()
        self.held = tuple(held)
        if self.listening:
            self._watch = threading.Thread(target=self._close_what_comes, daemon=True)
            self._watch.start()
        return self

    def _close_what_comes(self) -> None:
        while not self._done.is_set():
            ready, _, _ = select.select(self.listening, [], [], 0.2)
            for s in ready:
                try:
                    s.accept()[0].close()
                except OSError:
                    pass

    def __exit__(self, *_) -> None:
        self._done.set()
        if self._watch is not None:
            self._watch.join(timeout=3)
        for s in self.socks:
            s.close()


# How a failure says a port was taken: Go's and Node's name the port on that line; Python's names it in the
# traceback's line of the bind, above.
_IN_USE = ("address already in use", "eaddrinuse", "address in use")


def held_hit(ran: "Ran") -> tuple[tuple[int, ...], list[str]]:
    """The ports Jaynes held that a failed command names where it says a port was in use (on that line or in the five
    before it), and those lines, each once. Nothing when it did not fail or Jaynes held none."""
    if ran.code == 0 or not ran.held:
        return (), []
    lines = ran.printed.splitlines()
    hit: set[int] = set()
    said: list[str] = []
    for i, line in enumerate(lines):
        if not any(w in line.lower() for w in _IN_USE):
            continue
        near = "\n".join(lines[max(0, i - 5):i + 1])
        named = {p for p in ran.held if re.search(rf"(?<!\d){p}(?!\d)", near)}
        if named:
            hit |= named
            if line.strip() not in said:
                said.append(line.strip())
    return tuple(p for p in ran.held if p in hit), said


def in_use_said(ran: "Ran") -> bool:
    """Whether a failed command says some port was in use."""
    return ran.code != 0 and any(w in ran.printed.lower() for w in _IN_USE)


def run(project: Path, command: str, *, source: Path | None = None, timeout: float = 600.0,
        env: dict[str, str] | None = None, hidden: tuple[Path, ...] = (), stdin: str | None = None,
        readable: tuple[Path, ...] = (), local_ports: Iterable[int] = (), any_port: bool = False,
        test_servers: bool = False, freed_ports: Iterable[int] = (), over: dict[str, str | None] | None = None,
        browser: bool = False) -> Ran:
    """Run ``command`` from a throwaway copy of ``project``, in the box, with the installed dependencies of
    ``source`` linked, and ``stdin`` on its standard input. ``readable`` names more folders it may read, only read,
    inside the hidden ones, such as the run's own copy of the role a check's script is in. Its servers bind the ports
    in ``JAYNES_PORTS`` and the step's ``local_ports``; with ``test_servers`` (macOS, the person's choice for the
    project, in the engine's call) any port too, a port the system picks among them, while Jaynes holds the free ones
    of ``HELD_PORTS`` but for ``freed_ports``, the ones the person let the project's tests use, which it binds and
    reaches as the step's. ``over`` puts files back in the copy before it runs: a path
    inside the project to the text it holds, or None to take it out. Raises ``Unavailable`` where there is no box."""
    with tempfile.TemporaryDirectory(prefix="jaynes-suite-") as tmp:
        work, scratch = Path(tmp) / "project", Path(tmp) / "tmp"
        shutil.copytree(project, work, symlinks=True, ignore=shutil.ignore_patterns(".git", ".jaynes", *DEPENDENCIES))
        scratch.mkdir()
        # what a step installed in the run's copy first, then what the person's own folder has
        installed = own_dependencies(project, link_dependencies(project, work))
        if source is not None:
            link_dependencies(source, work)
        for path, text in (over or {}).items():
            put = work / path
            if Path(path).is_absolute() or ".." in Path(path).parts or not put.resolve().is_relative_to(work.resolve()) \
                    or put.resolve() == work.resolve():
                raise ValueError(f"{path} is not a file inside the project")
            if text is None:
                put.unlink(missing_ok=True)
            else:
                put.parent.mkdir(parents=True, exist_ok=True)
                put.write_text(text)
        ports = reserve()
        declared = tuple(sorted({int(p) for p in (*local_ports, *freed_ports)} - {PAGE_PORT}))
        # the run's say, from its setup, in the engine's call: the person opened this project's local ports
        anyone = any_port is True
        own = test_servers is True and not anyone
        window = servers_window() if own and sys.platform == "darwin" else ()
        box = Box((work, scratch), hidden=hidden, local=True, readable=(project, *installed, *readable), reserved=ports,
                  local_ports=declared, any_port=anyone, window=window, browser=_browser() if browser else None)
        argv = command_in_box(command, box)
        given = {"JAYNES_PORTS": ",".join(map(str, ports))}
        holds = tuple(p for p in HELD_PORTS if p not in declared and p not in ports) if window else ()
        with _holding(holds) if window else contextlib.nullcontext() as holding:
            code, out, _, late = _wait_in_box(argv, cwd=work, env={**inherited(), **suite_environment(work, scratch), **(env or {}), **given},
                                              stdin=stdin, timeout=timeout)
        if late:
            out += f"\n(stopped after {timeout:.0f} s)".encode()
        text = out.decode(errors="replace")
        return Ran(code, text[-TAIL:], text, reserved=ports, local_ports=declared, any_port=anyone, test_servers=own,
                   held=holding.held if holding is not None else (), window=window)


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
        installed: tuple[Path, ...] = ()
        if project is None:
            work = Path(tmp) / "empty"
            work.mkdir()
        elif write:
            work = project
            readonly = (project / ".git",)
        else:
            work = Path(tmp) / "project"
            shutil.copytree(project, work, symlinks=True, ignore=shutil.ignore_patterns(".git", ".jaynes", *DEPENDENCIES))
            installed = own_dependencies(project, link_dependencies(project, work))
            if source is not None:
                link_dependencies(source, work)
        # what it reads is said, so the engine's runs are hidden from it but for its role's folder
        reads = tuple(p for p in (role, None if write or project is None else project) if p is not None) or (scratch,)
        reads += installed
        # a role's own script starts the Chrome Jaynes photographs with, as a photograph does
        argv = command_in_box(command, Box((work, scratch, out), ports=ports, readonly=readonly, readable=reads,
                                           browser=_browser()))
        env = {**inherited(), **suite_environment(work, scratch), "JAYNES_OUTPUT": str(out / "output.json")}
        code, said, _, late = _wait_in_box(argv, cwd=work, env=env, stdin=stdin, timeout=timeout)
        if late:
            said += f"\n(stopped after {timeout:.0f} s)".encode()
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


def past_window(ran: Ran) -> list[int]:
    """The ports a refused command named that macOS gave its own test servers past the window the box let them reach
    (``servers_window``): more ports were taken while it ran than the window holds, by it or by the machine's other
    programs, so the test server's own port came after them. Empty when the window was not open."""
    if not ran.window:
        return []
    system = _system_ports()
    if system is None:
        return []
    first, last, _ = system
    window = set(ran.window)
    return [int(p) for p in ports_refused(ran) if p != "0" and first <= int(p) <= last and int(p) not in window]


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


def _browser() -> Path | None:
    """The Chrome a role's own script may start in its box: the one Jaynes photographs with (``chrome``)."""
    program = chrome()
    return Path(program) if program else None


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
            # the one browser it starts is the one chosen here, with its bundle's helpers: never another
            argv = command_in_box(script, Box((Path(tmp),), ports=ports, browser=Path(program)))
            # Chrome keeps its default folders under the home macOS reports; the box lets it write none there, so
            # it is given one of its own, inside the temporary folder
            env = {**inherited(), "TMPDIR": tmp, "CFFIXED_USER_HOME": tmp}
            code, _, errors, late = _wait_in_box(argv, cwd=None, env=env, stdin=None, timeout=90, stderr_apart=True)
        except Unavailable as e:
            return str(e)
        if late:
            return f"Chrome did not photograph {target} within 90 seconds"
        if code != 0 or not shot.is_file():
            said = [line for line in errors.decode(errors="replace").splitlines() if "crashpad" not in line and line.strip()]
            return f"Chrome could not photograph {target} (exit {code}): " + " ".join(said[-3:])
        shutil.copyfile(shot, out)
    return None


# --- an agent's shell commands (the single box) ---------------------------------------------------------------

# The script every agent command runs as, inside the box: it starts where the agent's last command ended (``$1``),
# runs the command as the agent wrote it (``$2``), and keeps where it ended in the turn's file (``$3``), even when the
# command exits with an error or ``exit``s.
AGENT_SCRIPT = 'trap \'pwd -P >| "$3" 2>/dev/null\' EXIT; cd -- "$1" || exit 1; eval "$2"'


class AgentRefused(Exception):
    """Why Jaynes runs none of an agent's commands: its box could not be set up as the turn said."""


def _agent_start(work: Path, pwd: Path) -> Path:
    """Where the agent's next command starts: where its last one ended, kept in ``pwd``, while that is a folder inside
    ``work``; else ``work``'s top. The file is the agent's to write, so what it names is only a wish."""
    try:
        named = Path(os.path.realpath(pwd.read_text().strip()))
    except (OSError, ValueError):
        return work
    return named if named.is_dir() and named.is_relative_to(work) else work


def agent_box(spec: dict[str, Any]) -> tuple[Box, Path, Path, dict[str, str]]:
    """The box of one shell command of a Claude Code agent, from the turn's ``spec`` (agent.rs writes it): the step's
    folder (``work``) and what it writes and reads, the whole home hidden but for those, what else is hidden (the
    other runs, the engine's keys, the folder Claude Code's sessions share), the ports the step names, and the
    person's choice for the project's test servers or ports, as a check's commands get it. With the folder the
    command starts in, the file that keeps where it ends, and what it adds to its environment."""
    if not available():
        raise AgentRefused(INSIDE if inside() else "Jaynes has no box for the agent's commands on this system")
    try:
        work = Path(os.path.realpath(spec["work"]))
        writable = tuple(Path(p) for p in spec["writable"])
        pwd = Path(spec["pwd"])
        home = Path(spec.get("home") or Path.home())
        local_ports = tuple(sorted({int(p) for p in spec.get("local_ports") or ()} - {PAGE_PORT}))
    except (KeyError, TypeError, ValueError) as e:
        raise AgentRefused(f"the turn's box says too little ({e})") from None
    if not work.is_dir() or not writable or not all(p.is_absolute() for p in writable):
        raise AgentRefused(f"the step's folder {work} is not there")
    anyone = spec.get("any_port") is True
    own = spec.get("test_servers") is True and not anyone
    ports = reserve()
    b = Box(writable, hidden=tuple(Path(p) for p in spec.get("hidden") or ()), hide_home=True, local=True,
            readable=(work, *(Path(p) for p in spec.get("readable") or ())), reserved=ports, local_ports=local_ports,
            any_port=anyone, window=servers_window() if own else (), home=home)
    return b, _agent_start(work, pwd), pwd, {"JAYNES_PORTS": ",".join(map(str, ports))}


def agent_run(spec_path: Path, command: str) -> int:
    """Run one of an agent's shell commands in its box, with this process's standard streams, and say its exit code.
    What the engine keeps is found from the spec's ``engine``, never from the command's environment."""
    try:
        spec = json.loads(spec_path.read_text())
        if not isinstance(spec, dict):
            raise ValueError("not an object")
        if spec.get("engine"):
            os.environ["JAYNES_ENGINE_HOME"] = str(spec["engine"])
        b, start, pwd, adds = agent_box(spec)
        argv = [str(SANDBOX_EXEC), "-p", profile(b), "/bin/bash", "-c", AGENT_SCRIPT, "jaynes-agent", str(start), command,
                str(pwd)]
    except AgentRefused as e:
        print(f"Jaynes ran none of this command: {e}", file=sys.stderr)
        return 126
    except (OSError, ValueError) as e:
        print(f"Jaynes ran none of this command: it could not read the turn's box ({e})", file=sys.stderr)
        return 126
    env = {k: v for k, v in inherited().items() if k != "JAYNES_ENGINE_HOME"} | adds
    holds = tuple(p for p in HELD_PORTS if p not in b.local_ports and p not in b.reserved) if b.window else ()
    with _holding(holds) if b.window else contextlib.nullcontext():
        return subprocess.call(argv, cwd=start, env=env)


if __name__ == "__main__":
    # jaynes_box.py photograph TARGET OUT [WIDTHxHEIGHT]: for the engine and for a role's scripts
    # jaynes_box.py program-profile < BOX.json: the profile of a role's program's box, for the engine that starts it,
    # with what its interpreter and commands need worked out here; a refusal is its sentence and exit 3
    args = sys.argv[1:]
    if args in (["program-proof"], ["program-proof", "--again"]):
        # at install (jaynes update, install.sh) and in CI: prove the program's box on this macOS, and keep the proof
        proof = program_proof(Path(sys.executable), again=args[-1] == "--again")
        print(("holds on macOS " if proof["holds"] else "does not hold on macOS ") + proof["macos"]
              + "".join(f"\n  {w}" for w in proof["wrong"]))
        sys.exit(0 if proof["holds"] else 3)
    if args == ["program-needs"]:
        # worked out once per role and interpreter, at pinning, and kept: it walks the interpreter's whole folder
        given = json.load(sys.stdin)
        try:
            n = program_needs(Path(given["interpreter"]), [Path(c) for c in given.get("commands", [])],
                              Path(given["home"]) if given.get("home") else None)
            print(json.dumps({k: [str(p) for p in getattr(n, k)] for k in ("run", "run_inside", "read")}))
        except Refused as e:
            print(e)
            sys.exit(3)
        sys.exit(0)
    if args == ["program-profile"]:
        # one call's box, from the needs worked out at pinning; it never walks the interpreter again
        given = json.load(sys.stdin)
        try:
            # Apple's own rules change with macOS: no program runs in a box not proved on this one
            program_box_holds(Path(sys.executable))
            n = given.get("needs")
            if not isinstance(n, dict) or not n.get("run"):
                raise Refused("the program's needs were not worked out: run program-needs once, at pinning, and pass them")
            needs = Needs(*(tuple(Path(p) for p in n.get(k, [])) for k in ("run", "run_inside", "read")))
            paths = {k: Path(v) for k, v in given.items() if k in ("role_dir", "library", "tmp", "broker", "project", "home") and v}
            lists = {k: tuple(Path(v) for v in given.get(k, [])) for k in ("reads", "writes")}
            print(program_profile(ProgramBox(**paths, **lists, needs=needs, project_write=given.get("project_write") is True,
                                             local_ports=tuple(int(p) for p in given.get("local_ports", [])))), end="")
        except Refused as e:
            print(e)
            sys.exit(3)
        sys.exit(0)
    if len(args) >= 4 and args[0] == "agent-run" and args[2] == "--":
        # jaynes_box.py agent-run SPEC -- COMMAND: one shell command of a Claude Code agent, in its box
        sys.exit(agent_run(Path(args[1]), " ".join(args[3:])))
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
