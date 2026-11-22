"""The menu: the steps and checks of Jaynes's own that a role written in TOML picks by name.

Each is written once, here, beside the box where it runs a project's commands, and tested once. The program that
serves a TOML role (``jaynes role-serve``) asks for one with a line of JSON on standard input::

    {"kind": "step" | "check", "use": NAME, "with": {params}, "call": {the protocol's request fields}}

and reads one line back: the body of the protocol's reply (``result``, ``refused``, ``verdict`` or ``error``).
What a name means and which parameters it takes is checked before a run, by the engine; here a name is done.

Every check comes with cases: inputs it must reject and inputs it must accept, in ``cases/NAME/reject.json`` and
``accept.json`` (see ``cases/README.md``). ``{"kind": "case", "use": NAME, "with": {params}, "case": {a case}}``
asks what the check says of one case, run as in a run.

A check the menu lacks is a script of the role (``use: "script"``): it runs in the box on a copy of the project,
reads ``{"output", "given", "project"}`` on standard input, and exits 0 to pass, 1 to fail with its reasons on
standard output, 3 when it cannot judge.

A step the menu lacks is a script of the role too (``use: "script"``, kind ``step``): it runs in the box with the
reach its step declares, reads ``{"request", "given", "project", "round", "attempt", "key"}`` on standard input,
writes its output as JSON to the file named in ``JAYNES_OUTPUT``, and exits 0 when done, 1 when the step cannot be
done (with why, on standard output), 3 when it could not run. What it prints is read to tell when the box refused
what it tried, so a script says what happened in its own words and does not print what the project holds.
"""

from __future__ import annotations

import json
import os
import posixpath
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import traceback
import unicodedata
from fnmatch import fnmatchcase
from pathlib import Path
from typing import Any, Callable

import jaynes_box as box
from jaynes_role import Blocked, Fail, Pass, Refused, _result, _verdict

Call = dict[str, Any]
Params = dict[str, Any]


def _given(call: Call) -> dict[str, Any]:
    return {g["step"]: g["output"] for g in call.get("given") or []}


def _source(call: Call) -> Path | None:
    return Path(call["source"]) if call.get("source") else None


def _freed(call: Call) -> tuple[int, ...]:
    """The ports Jaynes holds that the person let this project's tests use, in the engine's call."""
    return tuple(int(p) for p in call.get("freed_ports") or () if isinstance(p, int) and not isinstance(p, bool))


def _box(call: Call, command: str, timeout: float = 600.0, stdin: str | None = None, readable: tuple[Path, ...] = (),
         alone: bool = False, over: dict[str, str | None] | None = None) -> Any:
    """Run ``command`` on a copy of the run's project in the box: a ``Ran``, or ``Blocked``. ``readable`` names
    more folders the command may read. ``alone``: a command that needs no project runs in an empty folder when its
    step reaches none. ``over``: files put back in the copy first (see ``box.run``)."""
    if not call.get("project") and not alone:
        return Blocked("the check has no copy of the project to run commands on")
    try:
        if call.get("project"):
            ran = box.run(Path(call["project"]), command, source=_source(call), timeout=timeout, stdin=stdin, readable=readable,
                         local_ports=(call.get("reach") or {}).get("local_ports") or (), any_port=call.get("any_port") is True,
                         test_servers=call.get("test_servers") is True, freed_ports=_freed(call), over=over)
        else:
            with tempfile.TemporaryDirectory(prefix="jaynes-empty-") as empty:
                ran = box.run(Path(empty), command, timeout=timeout, stdin=stdin, readable=readable,
                         local_ports=(call.get("reach") or {}).get("local_ports") or (), any_port=call.get("any_port") is True,
                         test_servers=call.get("test_servers") is True, freed_ports=_freed(call))
    except box.Unavailable as e:
        return Blocked(f"there is no box to run the project's commands in: {e}")
    if box.refused(ran):
        ports = box.ports_refused(ran)
        tried = (f" It tried local port {', '.join('0, a port of its own choosing' if p == '0' else p for p in ports)}.")\
            if ports else ""
        given = ", ".join(map(str, ran.reserved))
        # the project's test servers serve a port the system picks: one refused with them on says so, and the card
        # then offers what they did not give
        servers = " The project's test servers did not help here." if ran.test_servers and "0" in ports else ""
        ways = (" The person's open ports did not help here." if ran.any_port else servers +
                f" A check's commands bind and reach only the ports in JAYNES_PORTS ({given}) and the ones the check"
                " names in its local_ports, never its step's. Two ways out, the first the better: (1) the suite takes its"
                " test servers' ports from JAYNES_PORTS, and the box stays closed; (2) the person opens this project"
                f" with `jaynes project open-ports {_source(call) or call['project']}`, and its checks then reach every"
                " service listening on this machine, the Jaynes page among them.")
        return Blocked("the box refused what the command tried, so its failure says nothing about the project: "
                       + "; ".join(box.refused(ran)[:3]) + tried + ways)
    hit, lines = box.held_hit(ran)
    if hit:
        # a test server on a port Jaynes held: Jaynes's doing, never the project's, and the card offers the port
        return Blocked("Jaynes held a port the command tried to serve on, so its failure says nothing about the project: "
                       + "; ".join(lines[:3]) + f" It tried held port {', '.join(map(str, hit))}."
                       " The person lets this project's tests use them with"
                       f" `jaynes project test-servers {_source(call) or call['project']}`, naming the ports.")
    if (ran.any_port or ran.test_servers) and ran.code != 0:
        # a failure of a check that ran open says so, beside what it says about the project
        how = "every local port open" if ran.any_port else "the project's own test servers let"
        if box.in_use_said(ran) and ran.held:
            how += f", and Jaynes holding ports {', '.join(map(str, ran.held))} of databases and caches"
        ran = box.Ran(ran.code, ran.output + f"\n(this check ran with {how}, by the person's choice for this project)",
                      ran.printed, ran.reserved, ran.local_ports, ran.any_port, ran.test_servers, ran.held)
    return ran


# --- steps -----------------------------------------------------------------------------------------------------

NO_RUNNER = ("no test runner was found in the project: Jaynes looks for pytest, a test script in package.json, "
             "go.mod, Cargo.toml and a Makefile test target, and a test script or target that only succeeds or "
             "prints (true, echo) runs no test")


def find_suite(call: Call, params: Params) -> Any:
    """How the project's tests run: ``{kind, suite, each, found}``, with ``suite`` empty when it has none, or a
    refusal when the role requires one or the machine cannot run them."""
    project = call.get("project")
    if not project:
        return Refused("finding the suite needs the project")
    found = box.detect(Path(project), "")
    if found is None:
        if params.get("required"):
            return Refused(NO_RUNNER)
        return {"kind": "", "suite": "", "each": "", "found": ""}
    try:
        why = box.missing(Path(project), found, source=_source(call))
    except box.Unavailable as e:
        return Blocked(str(e))
    if why:
        return Refused(why)
    return found.to_dict()


def _plain(text: str, given: dict[str, Any]) -> str:
    """``{{step.field}}`` filled plainly from what the step was given."""
    def one(m: re.Match[str]) -> str:
        step, _, field = m.group(1).strip().partition(".")
        value = (given.get(step) or {}).get(field) if field else given.get(step)
        return value if isinstance(value, str) else "" if value is None else json.dumps(value)
    return re.sub(r"\{\{(.*?)\}\}", one, text)


def refuse_when_false(call: Call, params: Params) -> Any:
    """Refuses, with the role's sentence, when an earlier step's field is false: the person decides how to go on."""
    step, _, field = str(params["field"]).partition(".")
    given = _given(call)
    if (given.get(step) or {}).get(field) is False:
        return Refused(_plain(str(params["says"]), given))
    return {}


def script_step(call: Call, params: Params) -> Any:
    """A step of the role's own: a script of its folder, run in the box with the reach its step declares.

    Exit 0 with an output is the step done. Exit 1 is the script's word that the step cannot be done and that it
    did nothing: the role's fault, with what it printed. Exit 3, no box, or a box that refused what it tried: the
    machine's. Anything else is a script that broke, the role's fault too. A step that cannot be undone (``acts``)
    may have acted in every case but exit 1 and exit 3: then its outcome is not known, which is told to the engine as
    such, so that it asks the person before anything acts again, and never as the role's refusal, after which a
    continue would run the act again."""
    path = str(params["path"])
    name = str(params.get("name") or Path(path).name)
    access = str(params.get("project") or "none")
    project = Path(call["project"]) if call.get("project") and access != "none" else None
    acted = (f". {call.get('step') or name} cannot be undone, so it may have acted: look before you continue"
             if params.get("acts") else "")
    # what may have acted is not the role's refusal: its outcome is not known, and the engine holds it so
    def failed(reason: str) -> Any:
        return Blocked(reason) if params.get("acts") else Refused(reason)
    given = json.dumps({"request": call.get("request"), "given": call.get("given") or [],
                        "project": "." if project is not None else None, "round": call.get("round"),
                        "attempt": call.get("attempt"), "key": call.get("key")})
    try:
        done = box.run_step(_quote(path) if _executable(path) else f"sh {_quote(path)}", project=project,
                            write=access == "write", source=_source(call),
                            role=Path(str(params["role"])) if params.get("role") else None,
                            ports=tuple(int(p) for p in params.get("local_ports") or []), stdin=given)
    except box.Unavailable as e:
        return Blocked(f"there is no box to run the role's script in: {e}")
    ran = done.ran
    said = [line.strip() for line in ran.printed.splitlines() if line.strip()]
    if box.refused(ran):
        return Blocked(f"the box refused what {name} tried, which its step's reach does not allow: "
                       + "; ".join(box.refused(ran)[:3]) + acted)
    if ran.code == 1:
        return Refused(" ".join(said[-5:]) or f"{name} said the step cannot be done, and not why")
    if ran.code == 3:
        return Blocked(" ".join(said[-5:]) or f"{name} could not run")
    if ran.code != 0:
        return failed(f"{name} exited with {ran.code}" + (": " + " ".join(said[-5:]) if said else ""))
    if done.output is None:
        return failed(f"{name} " + (f"gave an output Jaynes does not read: {done.unread}" if done.unread else
                                    "wrote no output: it writes JSON to the file named in JAYNES_OUTPUT"))
    try:
        output = json.loads(done.output)
    except ValueError as e:
        return failed(f"what {name} wrote to JAYNES_OUTPUT is not JSON: {e}")
    if not isinstance(output, dict):
        return failed(f"what {name} wrote to JAYNES_OUTPUT is not a JSON object")
    return output


# --- checks ----------------------------------------------------------------------------------------------------

def _suite(call: Call, params: Params) -> dict[str, Any]:
    return _given(call).get(str(params.get("suite_step") or "")) or {}


# make's line for a failure it went on after, as a - before a recipe's line, -i or .IGNORE tell it to
IGNORED = re.compile(r"^g?make(?:\[\d+\])?: \[.*\] Error \d+ \(ignored\)$", re.M)
INCLUDED = re.compile(r"^\s*-?s?include\s+(.*)$")
# pytest's configuration: what it reads before any test, and every conftest.py, which can change any test's outcome
PYTEST_CONFIG = ("pytest.ini", "pyproject.toml", "setup.cfg", "tox.ini")
NODE_SCRIPTS = ("test", "pretest", "posttest")

Start = tuple[list[str], Callable[[str], "str | None"]]


def _emptied(call: Call, runner: dict[str, Any]) -> Any:
    """A failure when the suite an earlier step found now runs nothing that could fail: emptied, it passes whatever
    the project does. A project whose test target never ran anything has no suite to begin with (find-suite)."""
    if not call.get("project"):
        return None
    why = box.runs_nothing(Path(call["project"]), box.Runner.from_dict(runner))
    return Fail(f"the suite runs nothing now: {why}, so it passes whatever the project does; the suite is to run "
                "the tests it ran when the run began") if why else None


def _start_reader(call: Call) -> Start | None:
    """The project as the run began, as its paths and a reader of any of them, or None when that is not known. A
    run's check is told where the engine keeps the start, beside the copy and out of every agent's reach (``start``:
    its repository and the commit the run began from); a case gives its files (``start``: a map of paths to text)."""
    start = call.get("start")
    if not isinstance(start, dict):
        return None
    if isinstance(start.get("files"), dict):
        given = {p: t for p, t in start["files"].items() if isinstance(t, str)}
        return list(given), given.get
    git, commit = start.get("git"), start.get("commit")
    if not isinstance(git, str) or not isinstance(commit, str) or not git or not commit:
        return None
    def show(*args: str) -> bytes | None:
        try:
            done = subprocess.run(["git", f"--git-dir={git}", *args], capture_output=True, timeout=60)
        except (OSError, subprocess.SubprocessError):
            return None
        return done.stdout if done.returncode == 0 else None
    def read(path: str) -> str | None:
        text = show("show", f"{commit}:{path}")
        return None if text is None else text.decode(errors="replace")
    listed = show("ls-tree", "-r", "-z", "--name-only", commit)
    return None if listed is None else ([p for p in listed.decode(errors="replace").split("\0") if p], read)


def _kept(path: str) -> bool:
    """Whether ``path`` is the project's own, outside dependency folders and .git."""
    return not any(p in box.DEPENDENCIES or p == ".git" for p in Path(path).parts)


def _now(project: Path) -> Start:
    paths = []
    for top, dirs, files in os.walk(project):
        dirs[:] = [d for d in dirs if d not in box.DEPENDENCIES and d != ".git"]
        paths += [(Path(top) / f).relative_to(project).as_posix() for f in files]
    def read(path: str) -> str | None:
        return (project / path).read_text(errors="replace") if (project / path).is_file() else None
    return paths, read


def _with_included(files: dict[str, str], read: Callable[[str], str | None]) -> dict[str, str]:
    """``files`` and the files they include by a path written out, read by ``read``, from the folder of the file that
    includes them."""
    out, todo = dict(files), list(files)
    while todo:
        path = todo.pop()
        for line in re.sub(r"\\\n", " ", out[path]).split("\n"):
            m = INCLUDED.match(line)
            for name in (m.group(1).split("#", 1)[0].split() if m else []):
                named = posixpath.normpath(posixpath.join(posixpath.dirname(path), name))
                if "$" not in name and named not in out and not named.startswith("../"):
                    text = read(named)
                    if text is not None:
                        out[named] = text
                        todo.append(named)
    return out


# What runs a script named after it: the script is its first word that is not an option
INTERPRETERS = {"sh", "bash", "zsh", "dash", "ksh", "python", "python3", "node", "perl", "ruby", "php", "bun", "deno"}


def _scripts_run(lines: list[str], at: str) -> list[str]:
    """The scripts of the project that ``lines``, a recipe's or a test script's, run by a path written out: the first
    word of a command when it is a path, or what an interpreter runs (sh tests/run.sh, python3 tests/check.py). A file
    a command only reads is not one. Paths from the folder ``at``; one made of variables, or outside, is left out."""
    found: list[str] = []
    for line in lines:
        for command in re.split(r"&&|\|\||;|\|", line.strip().lstrip("@-+")):
            try:
                words = shlex.split(command)
            except ValueError:
                continue
            while words and re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", words[0]):
                words = words[1:]
            if not words:
                continue
            if Path(words[0]).name in INTERPRETERS:
                rest = [w for w in words[1:] if not w.startswith("-")]
                if not rest or any(w in ("-c", "-e") for w in words[1:]):
                    continue
                word = rest[0]
            elif "/" in words[0]:
                word = words[0]
            else:
                continue
            named = posixpath.normpath(posixpath.join(at, word))
            if "$" not in word and not word.startswith(("/", "~")) and not named.startswith("../") and named not in found:
                found.append(named)
    return found


def _make_runs(files: dict[str, str], target: str = "test") -> list[str]:
    """The scripts the make target runs, and the targets it needs, transitively, in these Makefiles."""
    found: list[str] = []
    todo, seen = [target], set()
    while todo and len(seen) < 64:
        name = todo.pop()
        if name in seen:
            continue
        seen.add(name)
        for path, text in files.items():
            rule = box.make_target(text, name)
            if rule:
                needs, recipe = rule
                todo += needs
                found += [s for s in _scripts_run(recipe, posixpath.dirname(path)) if s not in found]
    return found


def _definition(kind: str, tree: Start, suite: str = "") -> dict[str, str] | None:
    """How the suite of a runner of ``kind`` is defined in ``tree``: the files that say what it runs, by path, or
    None when one of them cannot be read. Make's: every Makefile, GNUmakefile, makefile and .mk of the project, with
    what they include by a path written out. pytest's: its configuration and every conftest.py. node's: package.json,
    of which only its test scripts are compared. Go's and cargo's are the runner's own command. With each, the
    scripts the project's files above run by a path, and those ``suite``, the command, runs (sh tests/run.sh)."""
    paths, read = tree
    runs = _scripts_run([suite], "") if suite else []
    if kind == "make":
        named = [p for p in paths if _kept(p) and (Path(p).name in box.MAKEFILES or p.endswith(".mk"))]
    elif kind == "pytest":
        named = [p for p in paths if p in PYTEST_CONFIG or (Path(p).name == "conftest.py" and _kept(p))]
    elif kind == "node":
        named = [p for p in paths if p == "package.json"]
    else:
        named = []
    files: dict[str, str] = {}
    for path in named:
        text = read(path)
        if text is None:
            return None
        files[path] = text
    if kind == "make":
        files = _with_included(files, read)
        runs += _make_runs(files)
    elif kind == "node":
        runs += [s for k, v in _scripts(files.get("package.json"))[1].items() if k in NODE_SCRIPTS for s in _scripts_run([str(v)], "")]
    for path in runs:
        text = read(path)
        if text is not None:
            files.setdefault(path, text)
    return files


def _read_on_its_own(kind: str, path: str) -> bool:
    """Whether the runner reads ``path`` whatever names it: a Makefile make finds, pytest's configuration or a
    conftest.py. One the run added is taken out to run the suite as it began; a script beside is left."""
    name = Path(path).name
    return (kind == "make" and (name in box.MAKEFILES or path.endswith(".mk"))) or \
        (kind == "pytest" and (path in PYTEST_CONFIG or name == "conftest.py"))


def _scripts(text: str | None) -> tuple[dict[str, Any] | None, dict[str, Any]]:
    try:
        package = json.loads(text or "")
    except ValueError:
        return None, {}
    scripts = package.get("scripts") if isinstance(package, dict) else None
    return (package if isinstance(package, dict) else None), (scripts if isinstance(scripts, dict) else {})


def as_it_began(kind: str, start: Start, project: Path, suite: str = "") -> dict[str, str | None] | None:
    """What to put back in a copy of ``project`` so that its suite is defined as it was when the run began, over the
    code of now: each file of the definition that changed, with its text then, or None for one the run added. Empty
    when the definition is the same; None when the start cannot be read. A test is not the definition: a test the
    step changed runs as it is now."""
    tree = _now(project)
    then, now = _definition(kind, start, suite), _definition(kind, tree, suite)
    if then is None or now is None:
        return None
    current = tree[1]
    back: dict[str, str | None] = {p: t for p, t in sorted(then.items()) if p != "package.json" and current(p) != t}
    back.update({p: None for p in sorted(now) if p not in then and _read_on_its_own(kind, p)})
    if kind == "node":
        (_, before), (package, after) = _scripts(then.get("package.json")), _scripts(now.get("package.json"))
        if any(before.get(k) != after.get(k) for k in NODE_SCRIPTS):
            if package is None:
                back["package.json"] = then.get("package.json")
            else:
                scripts = {k: v for k, v in after.items() if k not in NODE_SCRIPTS}
                scripts.update({k: before[k] for k in NODE_SCRIPTS if k in before})
                back["package.json"] = json.dumps({**package, "scripts": scripts}, indent=2) + "\n"
    return back


def _if_hides(line: str) -> bool:
    """Whether a one-line if hides its condition's failure: with no else, its body does nothing that could fail, so
    the line succeeds whatever the condition does. A body that exits with a failure, or runs a command that could
    fail, is a test: `if [ ... ]; then echo bad; exit 1; fi` fails when it should. An if written over several lines is
    not read here."""
    m = re.match(r"if\s+.+?;\s*then\s+(?P<body>.*?)\s*;?\s*fi$", line)
    if not m or re.search(r"\b(else|elif)\b", line):
        return False
    pieces = [p.strip() for p in re.split(r";|&&|\|\|", m.group("body")) if p.strip()]
    return all(box.NOTHING.fullmatch(p) for p in pieces)


def _cannot_fail(line: str) -> bool:
    """Whether a recipe's or a test script's ``line`` runs something that could fail and is made unable to: its last
    alternative after || only succeeds or prints, its last command after ; does, or it is an if with no else."""
    line = line.strip().lstrip("@-+").strip()
    if box.noop([line]):
        return False
    if re.match(r"if\s", line):
        return _if_hides(line)
    for joined in (r"\|\|", r";"):
        parts = [p.strip() for p in re.split(joined, line) if p.strip()]
        if len(parts) > 1 and box.NOTHING.fullmatch(parts[-1]) and not all(box.NOTHING.fullmatch(p) for p in parts):
            return True
    return False


def _unexplained(call: Call, runner: dict[str, Any], ran: Any) -> str | None:
    """With the start not known, what in a suite that passed may be the step's way of making it pass: a failure make
    was told to ignore, or a line of the test target or test script that cannot fail."""
    ignored = IGNORED.search(ran.printed)
    if ignored:
        return f"make went on after a failure ({ignored.group(0)})"
    project = Path(call["project"]) if call.get("project") else None
    if project is None:
        return None
    tree = _now(project)
    lines: list[str] = []
    if runner.get("kind") == "make":
        name = next((n for n in box.MAKEFILES if (project / n).is_file()), None)
        target = box.make_target((project / name).read_text(errors="replace"), "test") if name else None
        lines = target[1] if target else []
    if runner.get("kind") == "node":
        lines = [str(v) for k, v in _scripts(tree[1]("package.json"))[1].items() if k in NODE_SCRIPTS]
    cannot = [line.strip() for line in lines if _cannot_fail(line)]
    if cannot:
        return f"its test {'target' if runner.get('kind') == 'make' else 'script'} has a line that cannot fail ({cannot[0]})"
    definition = _definition(str(runner.get("kind") or ""), tree, str(runner.get("suite") or "")) or {}
    for path, text in definition.items():
        if _read_on_its_own(str(runner.get("kind") or ""), path) or path == "package.json" or not _shell_script(path, text):
            continue
        why = _script_cannot_fail(text)
        if why:
            return f"a script it runs, {path}, {why}"
    return None


def _shell_script(path: str, text: str) -> bool:
    first = text.split("\n", 1)[0]
    return path.endswith((".sh", ".bash")) or bool(re.match(r"#!.*\b(sh|bash|zsh|dash|ksh)\b", first)) or "." not in Path(path).name


def _script_cannot_fail(text: str) -> str | None:
    """Why a shell script cannot fail, or None: a line of it that cannot, or, with no set -e and no exit with a
    failure in it, a last command that only succeeds or prints, whose status is the script's."""
    lines = [line.strip() for line in text.split("\n") if line.strip() and not line.strip().startswith("#")]
    cannot = [line for line in lines if _cannot_fail(line)]
    if cannot:
        return f"has a line that cannot fail ({cannot[0]})"
    errexit = re.search(r"^\s*set\s+(-[a-zA-Z]*e|-o\s+errexit)", text, re.M) or re.match(r"#!\S*\s+-[a-zA-Z]*e", text)
    fails = any(re.search(r"\bexit\s+([1-9]|\$)|\bfalse\b|\breturn\s+[1-9]", line) for line in lines)
    if lines and not errexit and not fails and box.NOTHING.fullmatch(lines[-1]) and not box.noop(lines):
        return f"ends in {lines[-1]}, so it passes whatever ran before"
    return None


def _passes(call: Call, runner: dict[str, Any], command: str, what: str, began: dict[str, str | None] | None) -> Any:
    """None when ``command`` passes, run on the run's copy, and again as the project defined the suite when the run
    began (``began``: what to put back) when the run changed that; or why not. With the start not known
    (``began`` None), a failure make ignored, or a line that cannot fail, is the step's: nothing explains it."""
    ran = _box(call, command)
    if isinstance(ran, Blocked):
        return ran
    if ran.code != 0:
        return Fail(f"{what}: {command} exited with {ran.code}. The end of what it printed:\n{ran.output}")
    if began is None:
        why = _unexplained(call, runner, ran)
        if why:
            return Fail(f"{what}: {command} passed, but {why}, and Jaynes cannot see how the project stood when the "
                        f"run began, so it counts as the step's. A suite passes when nothing in it fails. The end of "
                        f"what it printed:\n{ran.output}")
        return None
    if not began:
        return None
    again = _box(call, command, over=began)
    if isinstance(again, Blocked):
        return again
    if again.code != 0:
        changed = ", ".join(began)
        return Fail(f"{what} as the project defined it when the run began: the run changed {changed}, and with "
                    f"{'it' if len(began) == 1 else 'them'} as at the start, {command} exited with {again.code} on the "
                    f"code of now. A suite passes as the project defined it too. The end of what it printed:\n"
                    f"{again.output}")
    return None


def _began(call: Call, runner: dict[str, Any]) -> dict[str, str | None] | None:
    start = _start_reader(call)
    if start is None or not call.get("project"):
        return None
    return as_it_began(str(runner.get("kind") or ""), start, Path(call["project"]), str(runner.get("suite") or ""))


def suite_passes(call: Call, params: Params) -> Any:
    runner = _suite(call, params)
    command = str(runner.get("suite") or "")
    if not command:
        return Pass()
    emptied = _emptied(call, runner)
    if emptied:
        return emptied
    return _passes(call, runner, command, "the suite fails", _began(call, runner)) or Pass()


def _allowed(path: str, globs: list[str]) -> bool:
    """Whether ``path``, a path inside the project as :func:`in_project` gives it, matches a glob of ``globs`` and
    none of its ``!`` globs. A folder kept out holds every spelling a disk may take for its name: its ``!`` globs
    match whatever the capitals, as a Mac's disk compares names, and a glob that lets in matches only as written."""
    def match(p: str, g: str) -> bool:
        return fnmatchcase(p, g) or (g.startswith("**/") and fnmatchcase(p, g[3:]))
    path = unicodedata.normalize("NFC", path)
    globs = [unicodedata.normalize("NFC", g) for g in globs]
    return any(match(path, g) for g in globs if not g.startswith("!")) and \
        not any(match(path.casefold(), g[1:].casefold()) for g in globs if g.startswith("!"))


def in_project(path: str) -> str | None:
    """``path`` as the project's file system reads it, or None when it names no file inside the project: an empty
    path, an absolute one, one in a home folder (``~``), or one that goes above the project once its ``..`` are
    followed. Its ``./`` and ``//`` are dropped, its ``..`` followed, and its accents composed, as a Mac's disk
    stores names."""
    if not path or path.startswith(("/", "~")):
        return None
    named = posixpath.normpath(unicodedata.normalize("NFC", path))
    return None if named == "." or named == ".." or named.startswith("../") else named


def _not_within(path: str, globs: list[str], may: str) -> str | None:
    """Why ``path`` is not a file of the project the step may ``may`` (change, write), as the person reads it, or
    None."""
    named = in_project(path)
    if named is None:
        return f"{path or 'an empty path'} is not a file inside the project, and this step may {may} only files in it"
    if not _allowed(named, globs):
        return f"{path} is not a file this step may {may}" if named == path else \
            f"{path} is {named}, which is not a file this step may {may}"
    return None


def files_within(call: Call, params: Params) -> Any:
    """The files the output names in ``field`` are files of the project the step may change: the list the person
    reads tells the truth. What the step really changed is measured apart, from the copy, by the engine."""
    field = str(params.get("field") or "files")
    files = [str(f) for f in (call.get("output") or {}).get(field) or []] if isinstance(call.get("output"), dict) else []
    outside = [why for why in (_not_within(f, list(params.get("write") or []), "change") for f in files) if why]
    if outside:
        return Fail(*outside)
    return Pass()


def _for_files(runner: dict[str, Any], tests: list[str]) -> str:
    return box.for_files(box.Runner.from_dict(runner), tests)


# What a quoted failure must say beyond: a line made only of these words says nothing of why.
STATUS = {"fail", "failed", "failure", "failures", "error", "errors", "not", "ok", "exit", "status", "code", "test",
          "tests", "setup", "build"}


# Lines a runner or the shell prints of any failure, whatever made it, each with whose line it is: quoted, they name
# no reason. Matched against the whole quote.
RUNNER_LINES = (
    (r"g?make(\[\d+\])?: \*\*\* .*", "make's own line for a target that failed"),
    (r"g?make(\[\d+\])?: \[.*\] Error \d+ \(ignored\)", "make's own line for a failure it was told to ignore"),
    (r"npm (ERR!|error)(\s.*)?", "npm's own line"),
    (r"error Command failed with exit code \d+\.?", "yarn's own line"),
    (r".*\bELIFECYCLE\b.*|.*\bERR_PNPM_\w+.*", "pnpm's own line"),
    (r"error: (test failed, to rerun pass .*|\d+ targets? failed.*)", "cargo's own line"),
    (r"test result: FAILED\..*", "the test runner's count of what failed"),
    (r"--- FAIL: \S+ \([\d.]+s\)|FAIL\s+\S+\s+[\d.]+s|FAIL: \S+ \([\w.]+\)|FAILED \S+", "the test runner's line naming what failed"),
    (r"=+ .*\b(failed|errors?)\b.* in [\d.]+s.*=+", "the test runner's count of what failed"),
    (r"((\S+: )*(line \d+: )?\S+: )?command not found|\S+: \d+: \S+: not found",
     "the shell's line for a program it could not find, so the tests failed before they could show anything"),
)


def _quoted(failure: str, ran: Any, why: str) -> Any:
    quote = failure.strip()
    if all(w in STATUS for w in re.findall(r"[a-z]+", quote.lower())):
        return Fail(f"{quote} names no reason; quote the line that says why the tests fail {why}, such as the "
                    f"assertion message. The end of what it printed (exit {ran.code}):\n{ran.output}")
    whose = next((says for line, says in RUNNER_LINES if re.fullmatch(line, quote)), None)
    if whose:
        return Fail(f"{quote} is {whose}, and names no reason; quote the line that says why the tests fail {why}, "
                    f"such as the assertion message. The end of what it printed (exit {ran.code}):\n{ran.output}")
    if quote not in ran.printed:
        return Fail(f"{quote} is not in what the command printed; quote a line it prints {why}. The end of what it "
                    f"printed (exit {ran.code}):\n{ran.output}")
    return None


_TRACE_FILE = re.compile(r'^[E>\s]*File "([^"]+)", line \d+', re.M)
_TRACE_END = re.compile(r"^[E>\s]*([A-Za-z_][\w.]*(?:Error|Exception))\b[^\n]*$", re.M)
_JS_OWN = re.compile(r"^\s*(SyntaxError|ReferenceError|TypeError)\b[^\n]*\n(?:[^\n]*\n){0,3}?\s*at [^\n]*?\(?([^\s()]+):\d+:\d+\)?$", re.M)


def _own_error(printed: str, tests: list[str]) -> str | None:
    """The line that says a named test broke on its own code, not on the bug: a Python traceback whose last frame is
    in the test and whose error is no assertion, a JavaScript SyntaxError, ReferenceError or TypeError thrown from
    the test, or a shell's error about a line of the test. None when the tests fail on what they assert."""
    mine = lambda path: any(path == t or path.endswith("/" + t) for t in tests)
    for end in _TRACE_END.finditer(printed):
        if end.group(1).split(".")[-1] in ("AssertionError", "AssertError"):
            continue
        frames = _TRACE_FILE.findall(printed[:end.start()])
        if frames and mine(frames[-1]):
            return end.group(0).strip()
    for own in _JS_OWN.finditer(printed):
        if mine(own.group(2)):
            return own.group(0).splitlines()[0].strip()
    for t in tests:
        found = re.search(rf"^\S*{re.escape(t)}: (?:line )?\d+: [^\n]*$", printed, re.M)
        if found:
            return found.group(0).strip()
    return None


def tests_fail_for_the_bug(call: Call, params: Params) -> Any:
    """The tests the output names in ``tests`` are files the step may write, exist, and fail when run, printing the
    line the output quotes in ``failure``, and not on their own code (an exception the test throws, a parse error of
    the test). What the output says the test got, in ``saw``, is in what it printed. An output that says
    ``reproduced`` is false passes: no test shows the bug, and a later step stops the run for the person."""
    out = call.get("output") if isinstance(call.get("output"), dict) else {}
    if out.get("reproduced") is False:
        return Pass()
    reasons = [f"{name} is empty; a reproduced bug needs it" for name in ("tests", "failure") if not out.get(name)]
    if reasons:
        return Fail(*reasons)
    named = [str(t) for t in out.get("tests") or []]
    project = Path(call.get("project") or ".")
    write = list(params.get("write") or [])
    reasons = [f"{why}; tests go in paths such as tests/ or *_test.*" for why in (_not_within(t, write, "write") for t in named) if why]
    if reasons:
        return Fail(*reasons)
    tests = [in_project(t) or t for t in named]
    reasons = [f"{t} does not exist in the project" for t in tests if not (project / t).is_file()]
    if reasons:
        return Fail(*reasons)
    runner = _suite(call, params)
    if not runner.get("suite"):
        return Blocked("the check was not given how the project's tests run")
    command = _for_files(runner, tests)
    ran = _box(call, command)
    if isinstance(ran, Blocked):
        return ran
    if ran.code == 0:
        return Fail(f"the tests pass before any fix; they must fail because of the bug. {command} printed:\n{ran.output}")
    own = _own_error(ran.printed, tests)
    if own:
        return Fail(f"the tests fail on their own code, not on the bug: {own}. A test shows the bug when what it "
                    f"asserts is false; fix the test. The end of what {command} printed (exit {ran.code}):\n{ran.output}")
    saw = " ".join(str(out.get("saw") or "").split())
    if params.get("saw") and not saw:
        return Fail("saw is empty; give what the test got where it asserts, copied from what it printed, for the "
                    "person to read beside what they see now")
    if saw and saw not in " ".join(ran.printed.split()):
        return Fail(f"saw, \"{saw}\", is not in what the tests printed; quote what the test got, as it printed it. "
                    f"The end of what {command} printed (exit {ran.code}):\n{ran.output}")
    return _quoted(str(out.get("failure") or ""), ran, "because of the bug") or Pass()


def _alone(call: Call, tests: list[str], command: str) -> bool:
    """Whether ``command``, which runs ``tests`` alone, passes in a folder that holds those tests and none of the
    project's other files. A test that passes there passes whatever the code does. One that cannot be run so, or
    that the box refused, did not pass."""
    project = Path(call["project"])
    with tempfile.TemporaryDirectory(prefix="jaynes-alone-") as folder:
        for t in tests:
            if (project / t).is_file():
                (Path(folder) / t).parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(project / t, Path(folder) / t)
        ran = _box({**call, "project": folder}, command)
    return not isinstance(ran, Blocked) and ran.code == 0


def tests_pass(call: Call, params: Params) -> Any:
    """The tests an earlier step named in its ``tests`` pass, and then the whole suite. No test named is no test
    passing. A test that passes beside none of the project's other files, run alone where the runner can pick it,
    passes whatever the code does, and shows no fix."""
    runner = _suite(call, params)
    if not runner.get("suite"):
        return Blocked("the check was not given how the project's tests run")
    step = str(params["tests_from"])
    tests = [in_project(str(t)) or str(t) for t in (_given(call).get(step) or {}).get("tests") or []]
    if not tests:
        return Fail(f"{step} named no test, so there is no test of the bug to pass; the whole suite passing does not "
                    "show the bug is fixed")
    emptied = _emptied(call, runner)
    if emptied:
        return emptied
    picked, began = _for_files(runner, tests), _began(call, runner)
    for what, command in [("the tests still fail", picked)] + ([("the suite fails", runner["suite"])] if picked != runner["suite"] else []):
        failed = _passes(call, runner, command, what, began)
        if failed:
            return failed
        if command == picked and picked != runner["suite"] and _alone(call, tests, picked):
            return Fail(f"{', '.join(tests)} {'passes' if len(tests) == 1 else 'pass'} with none of the project's "
                        f"other files beside {'it' if len(tests) == 1 else 'them'} ({picked}), so "
                        f"{'it passes' if len(tests) == 1 else 'they pass'} whatever the code does and cannot show "
                        "the bug is fixed; the tests that showed the bug are to run the code they test")
    return Pass()


URL = "JAYNES_URL"
CASE_SECONDS = 120.0


def hidden_cases_pass(call: Call, params: Params) -> Any:
    """The cases a hidden step wrote (``cases``: ``says`` and ``run``) hold on the changed project. A failure tells
    the maker only the case's sentence; the person sees each case's result, and each case that failed in full: what
    it says, what it ran, the code it exited with and the end of what it printed."""
    cases = (_given(call).get(str(params.get("cases_step") or "")) or {}).get("cases") or []
    failed: list[str] = []
    detail: list[str] = []
    shown: list[dict[str, Any]] = []
    for case in cases:
        says, command = str(case.get("says", "")), str(case.get("run", ""))
        if expands(command, URL):
            detail.append(f"Not run, the run serves no app: {says}")
            continue
        ran = _box(call, command, timeout=CASE_SECONDS)
        if isinstance(ran, Blocked):
            return ran
        detail.append(f"{'Passes' if ran.code == 0 else 'Fails'}: {says}")
        if ran.code != 0:
            failed.append(f"A check you cannot see found that this does not hold: {says}")
            shown.append({"says": says, "run": command, "code": ran.code, "printed": ran.output})
    if failed:
        return Fail(*failed, detail="\n".join(detail), cases=shown)
    if cases and not any(d.startswith(("Passes", "Fails")) for d in detail):
        # cases written and none run: nothing was checked, and nothing checked is not passing
        return Fail(f"A check you cannot see could run none of its cases: each needs the app at {URL}, and this run "
                    "serves no app", detail="\n".join(detail))
    return Pass()


def expands(command: str, name: str) -> bool:
    """Whether the shell expands the variable ``name`` in ``command``: written $name or ${name...}, outside single
    quotes, a backslash's reach and comments."""
    quote, start, i = "", True, 0
    while i < len(command):
        c = command[i]
        if quote == "'":
            quote = "" if c == "'" else quote
        elif c == "\\":
            i += 1
        elif c in "'\"" and (not quote or quote == c):
            quote = "" if quote else c
        elif c == "#" and start and not quote:
            i, c = command.find("\n", i), "\n"
            if i < 0:
                return False
        elif c == "$" and re.match(rf"\$(\{{[#!]?)?{name}(?![A-Za-z0-9_])", command[i:]):
            return True
        start = not quote and c in " \t\n;|&()<>"
        i += 1
    return False


# --- what is blank ---------------------------------------------------------------------------------------------

# The characters Unicode says a reader does not show (Default_Ignorable_Code_Point), and the blank braille cell,
# which looks like a space: with white space and the controls, what shows nothing where a person reads it.
UNSEEN = ((0x00AD, 0x00AD), (0x034F, 0x034F), (0x061C, 0x061C), (0x115F, 0x1160), (0x17B4, 0x17B5), (0x180B, 0x180F),
          (0x200B, 0x200F), (0x202A, 0x202E), (0x2060, 0x206F), (0x2800, 0x2800), (0x3164, 0x3164), (0xFE00, 0xFE0F),
          (0xFEFF, 0xFEFF), (0xFFA0, 0xFFA0), (0xFFF0, 0xFFF8), (0x1BCA0, 0x1BCA3), (0x1D173, 0x1D17A), (0xE0000, 0xE0FFF))


def _unseen(c: str) -> bool:
    n = ord(c)
    return c.isspace() or unicodedata.category(c) == "Cc" or any(a <= n <= b for a, b in UNSEEN)


def blank(value: Any) -> bool:
    """Whether ``value`` shows nothing to whoever reads it: nothing, text of characters that show nothing, or a list
    or an object whose every item is blank. False and zero are values. A field a role requires is filled only when
    it is not blank, for field-required and output-follows-schema alike."""
    if value is None:
        return True
    if isinstance(value, str):
        return all(_unseen(c) for c in value)
    if isinstance(value, list):
        return all(blank(v) for v in value)
    if isinstance(value, dict):
        return all(blank(v) for v in value.values())
    return False


def nothing_written(value: Any) -> bool:
    """Whether nothing at all was written: no value, text of spaces and line breaks alone, an empty list or object.
    A field that must stay empty fails on anything more, an invisible character or a list of one blank item
    included, as a required field fails on anything less than what shows: each errs toward failing."""
    return value in (None, "", [], {}) or (isinstance(value, str) and not value.strip())


TYPES = {"string": str, "boolean": bool, "array": list, "object": dict}


def _against(value: Any, schema: dict[str, Any], at: str, out: list[str]) -> None:
    kind = schema.get("type")
    if kind in ("number", "integer"):
        ok = isinstance(value, (int, float)) and not isinstance(value, bool) and (kind == "number" or float(value).is_integer())
    else:
        ok = kind not in TYPES or isinstance(value, TYPES[kind])
    if not ok:
        out.append(f"{at} is not {'an' if str(kind)[0] in 'aeiou' else 'a'} {kind}")
        return
    if isinstance(value, str) and "maxLength" in schema and len(value) > schema["maxLength"]:
        out.append(f"{at} has {len(value)} characters, at most {schema['maxLength']}")
    if isinstance(value, list):
        if "maxItems" in schema and len(value) > schema["maxItems"]:
            out.append(f"{at} has {len(value)} items, at most {schema['maxItems']}")
        for n, item in enumerate(value, 1):
            _against(item, schema.get("items") or {}, f"{at}, item {n},", out)
    if isinstance(value, dict):
        props = schema.get("properties") or {}
        required = schema.get("required") or []
        for name in required:
            if blank(value.get(name)):
                out.append(f"{at} has no {name}" if at != "the output" else f"the output has no {name}")
        for name, v in value.items():
            if name not in props:
                continue
            where = f"{name}" if at == "the output" else f"{at} {name}"
            if v is None:
                # a field the schema does not require may be left out, never set to nothing
                if name not in required:
                    kind = str((props[name] or {}).get("type") or "value")
                    out.append(f"{where} is null: leave it out, or give {'an' if kind[0] in 'aeiou' else 'a'} {kind}")
                continue
            _against(v, props[name], where, out)


def output_follows_schema(call: Call, params: Params) -> Any:
    reasons: list[str] = []
    _against(call.get("output"), params.get("schema") or {}, "the output", reasons)
    return Fail(*reasons) if reasons else Pass()


def field_empty(call: Call, params: Params) -> Any:
    """A field the agent fills only to put a matter before the person, such as a test it may not change that stands
    in its way: while it is filled the check fails, with the role's sentence and what the agent wrote."""
    value = _field(call, params)
    if not nothing_written(value):
        return Fail(f"{params['says']}: {value if isinstance(value, str) else json.dumps(value)}")
    return Pass()


def _field(call: Call, params: Params) -> Any:
    return (call.get("output") or {}).get(str(params["field"])) if isinstance(call.get("output"), dict) else None


def field_required(call: Call, params: Params) -> Any:
    if blank(_field(call, params)):
        return Fail(f"the output has no {params['field']}")
    return Pass()


def field_max_length(call: Call, params: Params) -> Any:
    value = _field(call, params)
    length = len(value) if isinstance(value, str) else len(json.dumps(value)) if value is not None else 0
    most = int(params["max"])
    if length > most:
        return Fail(f"{params['field']} has {length} characters; keep it to {most}")
    return Pass()


# The extensions of files a program is made of or reads: a token that ends in one is a file's name, not a person's word
_CODE_EXTENSIONS = ("py", "js", "mjs", "cjs", "ts", "tsx", "jsx", "rs", "go", "rb", "java", "kt", "cs", "c", "h", "cpp",
                    "hpp", "swift", "php", "sh", "sql", "json", "toml", "yaml", "yml", "xml", "html", "css", "scss", "md",
                    "lock", "cfg", "ini", "env", "txt", "csv", "vue", "svelte", "lua", "ex", "exs", "scala", "dart")
_CODE_SIGNS: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("braces", re.compile(r"[{}]")),
    ("JSON", re.compile(r"\[\s*[\"\[{]")),
    ("a file path", re.compile(r"[\w.-]+/[\w./-]*\.\w+")),
    ("a file name", re.compile(r"(?<![\w.])[a-z0-9_][\w-]*\.(?:" + "|".join(_CODE_EXTENSIONS) + r")\b")),
    # "item(s)" is a person's plural, not a call
    ("a name called with parentheses", re.compile(r"[A-Za-z_]\w*\((?!s\))")),
    ("a name with an underscore", re.compile(r"\b\w*[A-Za-z0-9]_\w*\b|\b_\w+")),
    ("a quote of code", re.compile(r"`")),
    # a page's element by its tag and class or id, as a selector names it: div.row, span#tick; never "p.m."
    ("a selector", re.compile(r"(?<![\w.])(?:div|span|a|p|li|ul|ol|button|input|section|article|tr|td|th|table|form|label|img|nav|header|footer|main|h[1-6])[.#][A-Za-z][\w-]+")),
)


def _strings(value: Any, at: str) -> list[tuple[str, str]]:
    """Every text inside value, however deep, with where it is."""
    if isinstance(value, str):
        return [(at, value)]
    if isinstance(value, list):
        return [s for n, v in enumerate(value) for s in _strings(v, f"{at}[{n}]")]
    if isinstance(value, dict):
        return [s for k, v in value.items() for s in _strings(v, f"{at}.{k}")]
    return []


def _reach(call: Call, field: str) -> Any:
    """The output's field, or with a dot a field of each item of a list: stacks.idea is every stack's idea."""
    value: Any = call.get("output") if isinstance(call.get("output"), dict) else None
    for key in field.split("."):
        if isinstance(value, list):
            value = [v.get(key) for v in value if isinstance(v, dict)]
        elif isinstance(value, dict):
            value = value.get(key)
        else:
            return None
    return value


def plain_words(call: Call, params: Params) -> Any:
    """What the person reads in a field is in plain words: no code in it, which they would have to read as code. Every
    text inside the field is looked at, however deep; a field that is not there passes."""
    field = str(params["field"])
    reasons = []
    for at, text in _strings(_reach(call, field), field):
        for what, sign in _CODE_SIGNS:
            found = sign.search(text)
            if found:
                reasons.append(f"{at} has {what} ({found.group(0).strip()!r}): the person reads it, so write what they see, in plain words, with no code")
                break
    return Fail(*reasons) if reasons else Pass()


_REFUSES = re.compile(r"\b(not|no|never|without|but|except|instead|rather|avoid|don't|dont|do not|nor|n\u00e3o|nao|nem|sem|nada|exceto|menos|evite|evitar|fora|nunca)\b", re.I)
_UNDECIDED = re.compile(r"\?|\b(or|either|whichever|maybe|perhaps|might|could|should we|unsure|not sure|ou|talvez|tanto faz|qualquer|n\u00e3o sei|nao sei|quem sabe|ser\u00e1)\b", re.I)

def _as_compared(text: str) -> str:
    """Text as it is compared to the request: lower case, one space, straight quotes."""
    text = text.replace("\u201c", '"').replace("\u201d", '"').replace("\u2018", "'").replace("\u2019", "'")
    return " ".join(text.casefold().split())


def named_in_request(call: Call, params: Params) -> Any:
    """An option that says the request already named it (its words field) quotes the request word for word, shares a
    word with the option's name, is not refused there ("anything but Rust", "Next n\u00e3o"), and is the only option: the
    request decided, so nothing else is invented. An option with no words passes. In doubt it fails, and the agent
    offers options the person chooses from."""
    field, words = str(params["field"]), str(params["words"])
    options = _field(call, params)
    if not isinstance(options, list):
        return Pass()
    request = _as_compared(str(call.get("request") or ""))
    reasons = []
    named = [o for o in options if isinstance(o, dict) and isinstance(o.get(words), str) and o.get(words).strip()]
    for o in named:
        said = _as_compared(o[words])
        name = str(o.get("name", ""))
        found = re.search(rf"(?<!\w){re.escape(said)}(?!\w)", request)
        if len(said) < 2 or not found:
            reasons.append(f"{name}: the request does not say \"{o[words].strip()}\" word for word, so it did not name this option")
            continue
        if not set(re.findall(r"\w+", said)) & set(re.findall(r"\w+", _as_compared(name))):
            reasons.append(f"{name}: \"{o[words].strip()}\" shares no word with the option's name, so it does not name it")
            continue
        at = found.start()
        before = re.split(r"[.;!?,\n]", request[:at])[-1]
        after = re.split(r"[.;!?,\n]", request[at + len(said):])[0]
        start = max(request.rfind(c, 0, at) for c in ".;!\n")
        ends = [i for i in (request.find(c, at + len(said)) for c in ".;!?\n") if i >= 0]
        sentence = request[start + 1:min(ends) + 1 if ends else len(request)]
        # a refusal inside the quote counts where the quote names this option: "Python, not Rust" names Python
        names = set(re.findall(r"\w+", _as_compared(name)))
        # "but" and "mas" end a refusal ("Not Rust but Python"), save where they are it ("anything but Rust")
        turn = r"(?<!anything )(?<!everything )(?<!all )(?<!coisa )\b(?:but|mas)\b"
        own = [c for part in re.split(r"[,;]", said) for c in re.split(turn, part) if set(re.findall(r"\w+", c)) & names]
        before = re.split(turn, before)[-1]
        if _REFUSES.search(" ".join(before.split()[-4:])) or any(_REFUSES.search(c) for c in own) or re.match(r"^\s*(not|no|never|n\u00e3o|nao|nunca|nem)\b", after, re.I):
            reasons.append(f"{name}: the request may refuse \"{o[words].strip()}\" rather than ask for it, so the person chooses")
        elif _UNDECIDED.search(sentence):
            reasons.append(f"{name}: the request asks or leaves it open where it says \"{o[words].strip()}\", so it did not decide; the person chooses")
    if named and len(options) > 1:
        reasons.append(f"{field} has {len(options)} options though the request named one: give it alone")
    return Fail(*reasons) if reasons else Pass()


def _own_role(path: str) -> tuple[Path, ...]:
    """The folder a check's script may read beyond the project: its run's own copy of the role, which the box hides
    with the rest of the runs. Only when the menu was started there (the engine starts it in the role's folder),
    exactly a run's role folder (runs/RUN/role, never a run's own folder, which holds its steps), and the script is
    inside it. A role anywhere else is not hidden and needs nothing made readable; and making a wide folder
    readable would open again what the box hides inside it."""
    import os
    here = Path.cwd().resolve()
    runs = Path(os.path.realpath(box.engine_home() / "runs"))
    is_run_role = here.name == "role" and here.parent.parent == runs
    return (here,) if is_run_role and here in Path(path).resolve().parents else ()


def script(call: Call, params: Params) -> Any:
    """A check of the role's own: a script run in the box on a copy of the project, able to read the role's own
    folder (its other files, the modules it imports) and to write none of it."""
    path = str(params["path"])
    given = json.dumps({"output": call.get("output"), "given": call.get("given") or [], "project": "."})
    ran = _box(call, _quote(path) if _executable(path) else f"sh {_quote(path)}", stdin=given, readable=_own_role(path), alone=True)
    if isinstance(ran, Blocked):
        return ran
    said = [line for line in ran.printed.splitlines() if line.strip()]
    if ran.code == 0:
        return Pass()
    if ran.code == 1:
        return Fail(*(said or [f"{Path(path).name} failed and said nothing"]))
    if ran.code == 3:
        return Blocked(" ".join(said) or f"{Path(path).name} could not judge")
    return Blocked(f"{Path(path).name} exited with {ran.code}: " + " ".join(said[-5:]))


def _quote(s: str) -> str:
    import shlex
    return shlex.quote(s)


def _executable(path: str) -> bool:
    import os
    return os.access(path, os.X_OK)


# --- pages -----------------------------------------------------------------------------------------------------
#
# What an agent says a page says is the agent's word. Here Jaynes downloads the page itself, keeps its text in the
# step's output, which is in the record, and says by comparing text whether a quote is in it.
#
# The address comes from an agent, so it is held to rules before anything is sent: https only, the usual port, no
# name or password, at most URL_CHARS characters, and every address the name resolves to on the public internet,
# checked again at each redirect. The connection goes to an address that was checked, IPv4 first, each in turn
# until one answers. One clock runs around the whole page, from the lookup to the last character of its text, in
# a process of its own that is stopped when the clock runs out. Every address asked is kept.
#
# A page that is not read is one of two things, and they go apart (the engine's rule that a check which cannot
# judge never spends the agent's attempt):
# - The source said no, or does not exist (``Unread``): the server answered and refused (a 4xx other than 429),
#   the page is too large, in a format or character set Jaynes does not read, or has no text of its own, its
#   certificate does not hold, it broke a rule above, or its name does not exist while other names are found.
#   Nothing quoted from it is proved, and that is the agent's to deal with: choose another source, or drop the
#   claim.
# - Jaynes could not ask, or the source could not answer now (``Unreachable``): names are not being looked up on
#   this machine, the lookup failed, no address answered (no route, refused, dropped, timed out), the page took
#   longer than its clock, or the server answered 5xx or 429 every time it was asked, with waits between. The step
#   stops as Jaynes's fault, saying which page and why, and no attempt is spent: continue the run when it can ask.
#
# A quote is in a page when its words, compared by ``same_words``, are inside one block of the page's text (a
# paragraph, a heading, a cell: never across two), start and end at the edge of a word or number (a hyphenated
# word is one word, and 12,500 or 1.5 one number), and are not words the page's own address carries, since a page
# may only be repeating what it was asked.
#
# What it proves, and only that: these words stand here, in this page as Jaynes downloaded it. Whether the source
# asserts them or quotes them to deny them is a person's to judge, shown the text around them. It does not prove,
# and is not meant to:
# - Text a style sheet hides (a class whose rule is in <style>, opacity 0, a font size of 0): there is no engine of
#   CSS here, only the hidden attribute, aria-hidden and a style written on the element itself.
# - An address that carries the quote in no words (base64, a paste site): only words in the address are seen.
# - Context: "safe to cache" taken from "not safe to cache", or a denial in the block before. The window around a
#   quote exists for this, for the person to read.
# - The text of <textarea>, <option>, an <iframe>'s page and a closed <dialog>, which are read as text of the page.

PAGE_BYTES = 2_000_000      # a page larger than this is not read
PAGE_SECONDS = 20.0         # the time one asking of a page may take to come, redirects included
PAGE_WALL = 60.0            # the clock around downloading a page: lookup, asking again, every address, the body
PARSE_SECONDS = 20.0        # the clock around taking a page's text apart; past it, the page is the source's no
PAGE_REDIRECTS = 5
PAGE_WAITS = (1.0, 3.0)     # the waits before asking again a server that answered 5xx or 429: three times in all
PAGE_WAIT_MOST = 10.0       # the longest wait a server's Retry-After is followed for
PAGES_MAX = 12              # pages one step downloads
URL_CHARS = 2048            # an address longer than this is not sent
QUOTE_CHARS = 20            # a quote shorter than this, as compared, proves nothing
ECHO_WORDS = 4              # this many words of a quote in a row, in the page's address, and the address carries it
WINDOW_CHARS = 160          # the text shown on each side of a quote found, for the person to judge what it means
SCRIPTED_BELOW = 200        # an HTML page with scripts and less text than this is put together by them
RESOLVER_CANARY = "example.com"  # a name that always exists: when it is not found either, the machine cannot look names up
PAGE_AGENT = "jaynes"

# Written as escapes, since most of them look alike or do not show: quotation marks and apostrophes (with the
# modifier letter apostrophe and the prime, which pages use for one), dashes, and the characters that take no room
# (soft hyphen, zero-width space and joiners, word joiner, byte order mark, and the marks that set the direction
# of text).
_QUOTES = "\"\u201c\u201d\u201e\u201f\u00ab\u00bb\u2018\u2019\u201a\u201b`\u00b4\u02bc\u2032\u2033"
_DASHES = "\u2010\u2011\u2012\u2013\u2014\u2015\u2212"
_UNSEEN = "\u00ad\u200b\u200c\u200d\u2060\ufeff\u200e\u200f\u061c\u202a\u202b\u202c\u202d\u202e\u2066\u2067\u2068\u2069"
_HTML_TYPES = ("text/html", "application/xhtml+xml")
_TEXT_TYPES = ("text/plain", "text/markdown")
_CHARSETS = ("utf-8", "utf8", "us-ascii", "ascii", "iso-8859-1", "latin-1", "latin1", "iso-8859-15", "windows-1252", "cp1252")
_SKIPPED = ("script", "style", "noscript", "template", "svg", "del", "s", "strike")
_VOID = ("area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr")
_BLOCKS = ("address", "article", "aside", "blockquote", "caption", "dd", "details", "div", "dl", "dt", "figcaption",
           "figure", "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "header", "hr", "li", "main", "nav", "ol",
           "p", "pre", "section", "summary", "table", "tbody", "td", "tfoot", "th", "thead", "title", "tr", "ul")
# a blank line, which parts two blocks of a page's text
_GAP = re.compile(r"\n[ \t\r\f\v]*\n")
# A word that goes on past the end of a line at its hyphen: a letter or digit, the hyphen, one line break with
# nothing but spaces around it, and a letter or digit.
_WRAPPED = re.compile(r"(?<=\w)-[ \t]*\r?\n[ \t]*(?=\w)")
# where, in a page read with its hyphens left hanging, a line break was: a quote may not start just after it, since
# "non-" then "compliant" may be one word
_BREAK = "\x00"
# where a page groups the digits of a number with a no-break or thin space (12 500): kept as a mark, read as a space
# when searching, so the quote "12 500" is found and "12" is not
_GROUP = "\x03"
_GROUPED = re.compile("(?<=\\d)[\u00a0\u202f\u2009](?=\\d{3}(?!\\d))")


class Unread(Exception):
    """Why a page was not read, when the source said no or does not exist. What is quoted from it is not proved."""


class Unreachable(Exception):
    """Why Jaynes could not ask for a page, or the source could not answer now: the machine's or the network's
    fault, never the agent's."""


# a footnote mark (a raised link or a number in brackets), kept between these so a quote may leave it out
_NOTE_OPEN, _NOTE_CLOSE = "\x01", "\x02"
_NOTE = re.compile("\x01[^\x01\x02]*\x02")
# raised digits or signs after a digit, and a fraction after a digit, stay apart: 10 raised 6 is not 106, 5 and a
# half is not 51
_RAISED_AFTER_DIGIT = re.compile("(?<=\\d)([\u2070\u00b9\u00b2\u00b3\u2074-\u207b\u00bc-\u00be\u2150-\u215e])")
_FOLD = str.maketrans({**{c: "'" for c in _QUOTES}, **{c: "-" for c in _DASHES}, **{c: None for c in _UNSEEN}})


def same_words(text: str, hanging: bool = False, notes: bool = True) -> str:
    """The text as two texts are compared: Unicode compatibility forms folded (NFKC), characters that do not show
    dropped, every kind of quotation mark and apostrophe made ', every kind of dash made -, a word wrapped at its
    hyphen joined again with the hyphen kept, and each run of spaces and line breaks made one space. Letters,
    digits, their case and every other mark stay as written.

    Raised and lowered text as a reader writes it: lowered text joins its base (CO2, H2O), raised text after a
    letter joins it (km2, 1st), and raised digits or a fraction after a digit stay apart (10 raised 6 is not 106,
    5 and a half is not 51). A footnote mark is kept, or, without ``notes``, left out.

    A break counts in one place only: a single line break right after a hyphen inside a word. The page cannot say
    which it was, so a page is read both ways: as one word ("best-" then "effort" is best-effort) and, with
    ``hanging``, as a hyphen left hanging before the next word ("pre-" then "and" is pre- and), with the break
    marked so that no quote starts just after it. A hyphen and a space on the same line stay apart, and the hyphen is
    never dropped: "remo-" then "val" is never removal."""
    import unicodedata
    if _NOTE_OPEN in text:
        text = text.replace(_NOTE_OPEN, "").replace(_NOTE_CLOSE, "") if notes else _NOTE.sub("", text)
    text = _GROUPED.sub(_GROUP, text)
    text = _RAISED_AFTER_DIGIT.sub(r" \1", text)
    text = unicodedata.normalize("NFKC", text).translate(_FOLD)
    text = _WRAPPED.sub("-" + _BREAK if hanging else "-", text)
    return re.sub(r"\s+", " ", text).strip()


def _joined(block: str, at: int, step: int) -> bool:
    """Whether the character at ``at`` of ``block`` joins the quote beside it to more of a word or number, looking
    away from the quote by ``step``: a letter or digit; a hyphen or an underscore with a letter or digit beyond it,
    since a hyphenated word or a name like no_cache is one word, and a minus sign before a number; a no-break or
    thin space that groups the digits of a number; a comma or point between digits, since 12,500
    and 1.5 are one number; a percent sign after a number; an apostrophe inside a word, since can't is not can;
    or a line break a hyphen hung on."""
    if not 0 <= at < len(block):
        return False
    c = block[at]
    beyond = block[at + step] if 0 <= at + step < len(block) else ""
    near = block[at - step] if 0 <= at - step < len(block) else ""
    if c.isalnum() or c in (_BREAK, _GROUP):
        return True
    if c == "-":
        return beyond.isalnum() or (step < 0 and near.isdigit())
    if c == "_":
        return beyond.isalnum()
    if c in ",.":
        return beyond.isdigit() and near.isdigit()
    if c in "%\u2030":
        return step > 0 and near.isdigit()
    if c == "'":
        return beyond.isalpha() and near.isalpha()
    return False


def _searchable(text: str) -> str:
    """A compared text with its marks read as spaces: a line break a hyphen hung on, a space that groups digits."""
    return text.replace(_BREAK, " ").replace(_GROUP, " ")


def _bounded_at(words: str, block: str, search: str | None = None) -> int:
    """Where ``words`` stands in ``block`` with nothing of a word or number joined to it on either side, or -1. In a
    block read with its hyphens left hanging, a line break counts as a space, except just before the quote;
    ``search`` is the block with those breaks made spaces, when it was made already."""
    search = search if search is not None else _searchable(block)
    start = 0
    while (at := search.find(words, start)) >= 0:
        end = at + len(words)
        open_left = not words[0].isalnum() or not _joined(block, at - 1, -1)
        open_right = not words[-1].isalnum() or not _joined(block, end, 1)
        if open_left and open_right:
            return at
        start = at + 1
    return -1


def _bounded_in(words: str, block: str, search: str | None = None) -> bool:
    """Whether ``words`` is in ``block`` with nothing of a word or number joined to it on either side."""
    return _bounded_at(words, block, search) >= 0


NEAR_CHARS = 240            # the text shown, when a quote is not found, of what Jaynes read nearest to it


class Prepared:
    """A page's text made ready to be asked about many quotes: each block compared once, with its footnote marks
    and without them, and read both ways where a hyphen ends a line."""

    def __init__(self, text: str) -> None:
        self.blocks: list[tuple[int, str, str]] = []
        self.plain: list[str] = []
        for n, block in enumerate(_GAP.split(text)):
            compared = same_words(block)
            self.plain.append(_searchable(compared))
            for notes in ((True, False) if _NOTE_OPEN in block else (True,)):
                marked = compared if notes else same_words(block, notes=False)
                self.blocks.append((n, marked, _searchable(marked)))
                if _WRAPPED.search(block):
                    hanging = same_words(block, hanging=True, notes=notes)
                    self.blocks.append((n, hanging, _searchable(hanging)))

    def holds(self, quote: str) -> bool:
        words = _searchable(same_words(quote))
        return bool(words) and any(_bounded_in(words, b, search) for _, b, search in self.blocks)

    def locate(self, quote: str) -> tuple[int, int, str] | None:
        """Where the quote stands: the block of the page (counting from 0), where in that block's compared text,
        and the window around it, with the quote marked between \u00bb and \u00ab. The window goes on into the
        blocks before and after, each break between blocks shown as \u2016 (a mark pages rarely use, where the
        pilcrow is common), since a heading or the paragraph before can say that a source mentions a claim only to
        deny it."""
        words = _searchable(same_words(quote))
        if not words:
            return None
        for n, block, search in self.blocks:
            at = _bounded_at(words, block, search)
            if at >= 0:
                end = at + len(words)
                shown = search
                earlier = " \u2016 ".join(b for b in self.plain[:n] if b)
                later = " \u2016 ".join(b for b in self.plain[n + 1:] if b)
                before = ((earlier + " \u2016 ") if earlier else "") + shown[:at]
                after = shown[end:] + ((" \u2016 " + later) if later else "")
                window = (("..." if len(before) > WINDOW_CHARS else "") + before[-WINDOW_CHARS:] + "\u00bb" + shown[at:end] + "\u00ab"
                          + after[:WINDOW_CHARS] + ("..." if len(after) > WINDOW_CHARS else ""))
                return n, at, window
        return None

    def places(self, quote: str) -> int:
        """In how many blocks of the page the quote stands: a sentence a page says in two sections is shown where
        it stands first, and the person is told there are more."""
        words = _searchable(same_words(quote))
        if not words:
            return 0
        return len({n for n, block, search in self.blocks if _bounded_in(words, block, search)})

    def nearest(self, quote: str) -> str:
        """What Jaynes read nearest to a quote it did not find: the block sharing most of its words, cut around
        the first of them, for the agent to copy what is there."""
        said = set(_words(quote))
        if not said:
            return ""
        best = max(self.plain, key=lambda b: len(said & set(_words(b))), default="")
        if not said & set(_words(best)):
            return ""
        first = min((best.lower().find(w) for w in said if best.lower().find(w) >= 0), default=0)
        start = max(0, first - NEAR_CHARS // 4)
        cut = best[start:start + NEAR_CHARS]
        return ("..." if start else "") + cut + ("..." if start + NEAR_CHARS < len(best) else "")


def held(text: str, quote: str) -> bool:
    """Whether ``text`` has the words of ``quote``, inside one of its blocks, as whole words."""
    return Prepared(text).holds(quote)


def _words(text: str) -> list[str]:
    return re.findall(r"[0-9a-z]+", same_words(text).lower())


def _undone(address: str) -> str:
    """An address with its escapes undone until undoing changes nothing, so one escaped twice reads as its words."""
    from urllib.parse import unquote_plus
    for _ in range(8):
        plain = unquote_plus(address)
        if plain == address:
            break
        address = plain
    return address


def _echoes(quote: str, addresses: list[str]) -> bool:
    """Whether an address the page was asked by carries the quote: the whole of it, or ECHO_WORDS of its words in a
    row, read from the address with its escapes undone, and with hyphens, underscores, plus signs and slashes
    between words. A true quote that shares that many words with its address is refused too, and the agent chooses
    another sentence: that error is the one that cannot make a page say what it does not."""
    said = _words(quote)
    if not said:
        return False
    runs = [said] if len(said) <= ECHO_WORDS else [said[i:i + ECHO_WORDS] for i in range(len(said) - ECHO_WORDS + 1)]
    for address in addresses:
        carried = " " + " ".join(_words(_undone(address))) + " "
        if any(" " + " ".join(run) + " " in carried for run in runs):
            return True
    return False


def _hidden(attrs: list[tuple[str, str | None]]) -> bool:
    """Whether an element's attributes keep it from the reader: hidden, aria-hidden, or a style that hides it."""
    named = {k.lower(): (v or "") for k, v in attrs}
    style = re.sub(r"\s+", "", named.get("style", "").lower())
    return "hidden" in named or named.get("aria-hidden", "").lower() == "true" or "display:none" in style or "visibility:hidden" in style


# elements whose content a page fills in later; one left open at the top would hide the rest of the page
_FILLED_LATER = ("template", "noscript", "svg")


def _reader() -> Any:
    from html.parser import HTMLParser

    class Reader(HTMLParser):
        """The text of an HTML page as a reader sees it: no scripts, styles, struck, deleted or hidden parts; blocks
        apart by a blank line, a line break as a line break, and spaces inside text as one space, outside <pre>;
        lowered text joined to its base, raised text after a letter joined to it, raised digits after a digit apart,
        and a footnote mark (a raised link, or a number in brackets) marked so a quote may leave it out. What it
        leaves out ends with its element, or with the element around it, so one left open does not take the rest
        of the page."""

        def __init__(self, skipped: tuple[str, ...]) -> None:
            super().__init__(convert_charrefs=True)
            self.skipped = skipped
            self.parts: list[str] = []
            self.open: list[str] = []
            self.hidden_at: int | None = None
            self.raised: list[tuple[int, bool]] = []  # where each open <sup> began in parts, and whether it holds a link

        def _before(self, at: int) -> str:
            for piece in reversed(self.parts[:at]):
                if piece:
                    return piece[-1]
            return ""

        def handle_starttag(self, tag: str, attrs: Any) -> None:
            if tag in _VOID:
                if self.hidden_at is None:
                    self.parts.append("\n" if tag == "br" else "\n\n" if tag in _BLOCKS else "")
                return
            self.open.append(tag)
            if self.hidden_at is None and (tag in self.skipped or _hidden(attrs)):
                self.hidden_at = len(self.open) - 1
            elif self.hidden_at is None:
                if tag == "sup":
                    self.raised.append((len(self.parts), False))
                elif tag == "a" and self.raised:
                    self.raised[-1] = (self.raised[-1][0], True)
                self.parts.append("\n\n" if tag in _BLOCKS else "")

        def handle_startendtag(self, tag: str, attrs: Any) -> None:
            self.handle_starttag(tag, attrs)
            if tag not in _VOID:
                self.handle_endtag(tag)

        def handle_endtag(self, tag: str) -> None:
            if tag in _VOID or tag not in self.open:
                return
            while self.open and self.open.pop() != tag:
                pass
            if self.hidden_at is not None and len(self.open) <= self.hidden_at:
                self.hidden_at = None
            elif self.hidden_at is None:
                if tag == "sup" and self.raised:
                    start, link = self.raised.pop()
                    content = "".join(self.parts[start:])
                    del self.parts[start:]
                    plain = content.strip()
                    if link or re.fullmatch(r"\[[^\[\]]{1,12}\]", plain):
                        self.parts.append(_NOTE_OPEN + content + _NOTE_CLOSE)
                    elif plain and self._before(start).isdigit() and all(c.isdigit() or c in "+-\u2212" for c in plain):
                        self.parts.append(" " + content)
                    else:
                        self.parts.append(content)
                self.parts.append("\n\n" if tag in _BLOCKS else "")

        def handle_data(self, data: str) -> None:
            if self.hidden_at is None:
                # spaces and line breaks of the markup are one space; a no-break or thin space is the page's own
                self.parts.append(data if "pre" in self.open else re.sub(r"[ \t\r\n\f\v]+", " ", data))

    return Reader


_KIND = re.compile(r"[a-z0-9][a-z0-9.+-]{0,40}/[a-z0-9][a-z0-9.+-]{0,60}")


def _kind(content_type: str) -> str:
    """A page's format, as type/subtype, or empty when the server's word for it is not one: what a server writes
    is kept only in that form, so no text it chooses reaches the record or a later step."""
    kind = content_type.split(";")[0].strip().lower()
    return kind if _KIND.fullmatch(kind) else ""


def page_text(body: bytes, content_type: str) -> str:
    """The text of a downloaded page, or ``Unread``: a format or character set Jaynes does not read, bytes that are
    not text, markup the parser cannot read, an HTML page with no text, or one that leaves its text for a script
    to fill. A page that says ISO-8859-1 or ASCII is read as windows-1252, as browsers do."""
    kind = _kind(content_type)
    if kind not in _HTML_TYPES and kind not in _TEXT_TYPES:
        raise Unread(f"its format ({kind or 'not said in a form Jaynes reads'}) is not one Jaynes reads")
    if b"\x00" in body[:4096]:
        raise Unread("it is not text")
    named = re.search(r"charset=[\"']?([\w.:-]+)", content_type, re.I)
    charset = named.group(1) if named else ""
    if not charset and kind in _HTML_TYPES:
        meta = re.search(rb"<meta[^>]+charset=[\"']?([\w.:-]+)", body[:4096], re.I)
        charset = meta.group(1).decode("ascii", "replace") if meta else ""
    if charset and charset.lower() not in _CHARSETS:
        raise Unread(f"its character set ({charset[:40]}) is not one Jaynes reads")
    if charset.lower() in ("iso-8859-1", "latin-1", "latin1", "us-ascii", "ascii"):
        charset = "cp1252"
    text = body.decode(charset or "utf-8", errors="replace")
    if kind in _TEXT_TYPES:
        return text
    Reader = _reader()
    try:
        reader = Reader(_SKIPPED)
        reader.feed(text)
        reader.close()
        if reader.hidden_at is not None and reader.open[reader.hidden_at] in _FILLED_LATER:
            # left open with nothing around it to end it: read the page again with that kind of element shown
            reader = Reader(tuple(t for t in _SKIPPED if t not in _FILLED_LATER))
            reader.feed(text)
            reader.close()
    except Exception as e:  # noqa: BLE001: markup the parser cannot read is the page's, not Jaynes's
        raise Unread(f"its markup could not be read ({type(e).__name__})") from None
    seen = "".join(reader.parts)
    words = same_words(seen)
    if len(words) < SCRIPTED_BELOW and re.search(rb"<script", body, re.I):
        raise Unread("it has almost no text of its own: a script puts the page together in the browser")
    if not words:
        raise Unread("it has no text")
    return seen


def _public(ip: str) -> bool:
    """Whether an address is on the public internet. IPv4: global and not multicast. IPv6: global unicast only
    (2000::/3), and none of the forms that carry an IPv4 address inside (6to4, Teredo, mapped, compatible,
    translated), which leaves out this machine, private, link-local and site-local networks. Never an address
    with a zone, and never the ranges listed in ``closed``, which this Python's own list does not close."""
    import ipaddress
    if "%" in ip:  # a zone names an interface of this machine
        return False
    try:
        a = ipaddress.ip_address(ip)
    except ValueError:
        return False
    # ranges a Python may still call global that no public page is on
    closed = [ipaddress.ip_network(n) for n in ("192.0.0.0/24", "192.88.99.0/24", "2002::/16", "2001::/32", "3fff::/20", "5f00::/16")]
    if any(a.version == n.version and a in n for n in closed):
        return False
    if a.version == 4:
        return a.is_global and not a.is_multicast
    return a in ipaddress.ip_network("2000::/3") and a.is_global


def _address(url: str) -> tuple[str, str]:
    """The host and the request target of an address Jaynes may download, or ``Unread``."""
    from urllib.parse import quote, urlsplit
    if len(url) > URL_CHARS:
        raise Unread(f"its address is longer than {URL_CHARS} characters")
    try:
        parts = urlsplit(url)
        port = parts.port
    except ValueError:
        raise Unread("only https addresses are downloaded, and this one does not read") from None
    if parts.scheme.lower() != "https" or not parts.hostname:
        raise Unread("only https addresses are downloaded")
    if parts.username is not None or parts.password is not None:
        raise Unread("its address carries a name or password")
    if port not in (None, 443):
        raise Unread(f"its address names the port {port}, and only the usual one is used")
    # A host written as numbers is read only in the one form every reader reads alike, four decimal numbers of at
    # most 255 with no zero in front: the system's resolver reads 0177 as decimal and a browser as octal, so 0177.0.0.1
    # is a public address to one and this machine to the other, and the page downloaded would not be the one meant.
    labels = parts.hostname.split(".")
    if all(re.fullmatch(r"0x[0-9a-f]*|[0-9]+", label) for label in labels):
        plain = len(labels) == 4 and all(re.fullmatch(r"0|[1-9][0-9]{0,2}", label) and int(label) <= 255 for label in labels)
        if not plain:
            raise Unread("its address writes a number in a form that readers do not agree on")
    target = quote(parts.path or "/", safe="/%:@!$&'()*+,;=~-._") + ("?" + quote(parts.query, safe="/%:@!$&'()*+,;=~-._?") if parts.query else "")
    return parts.hostname, target


def _resolve(host: str) -> list[str]:
    """Every address a name resolves to, in the resolver's order."""
    import socket
    found = socket.getaddrinfo(host, 443, type=socket.SOCK_STREAM)
    return list(dict.fromkeys(str(sockaddr[0]) for _family, _type, _proto, _name, sockaddr in found))


def _lookup(host: str, resolve: Callable[[str], list[str]]) -> list[str]:
    """The addresses of ``host``, IPv4 first; ``Unread`` when the name does not exist, ``Unreachable`` when it could
    not be looked up. A name not found counts as not existing only when a name that always exists is found."""
    import socket
    missing = {getattr(socket, n) for n in ("EAI_NONAME", "EAI_NODATA") if hasattr(socket, n)}
    try:
        addresses = resolve(host)
    except UnicodeError:
        raise Unread("its name does not read") from None
    except socket.gaierror as e:
        if e.errno not in missing:
            raise Unreachable(f"the name {host} could not be looked up ({e.strerror or e})") from None
        try:
            resolve(RESOLVER_CANARY)
        except (OSError, UnicodeError):
            raise Unreachable(f"names are not being looked up on this machine ({host}, and {RESOLVER_CANARY} too)") from None
        raise Unread("its name does not exist") from None
    except OSError as e:
        raise Unreachable(f"the name {host} could not be looked up ({e.strerror or e})") from None
    return sorted(addresses, key=lambda a: ":" in a)


def _send(host: str, ip: str, target: str, timeout: float, *, port: int = 443, context: Any = None) -> tuple[int, dict[str, str], Callable[[int], bytes]]:
    """One GET over TLS to ``ip``, the address that was checked, with the certificate held to ``host``."""
    import http.client
    import socket
    import ssl
    tls = context or ssl.create_default_context()

    class Pinned(http.client.HTTPSConnection):
        def connect(self) -> None:
            sock = socket.create_connection((ip, port), self.timeout)
            self.sock = tls.wrap_socket(sock, server_hostname=host)

    conn = Pinned(host, port, timeout=timeout)
    conn.request("GET", target, headers={"User-Agent": PAGE_AGENT, "Accept": "text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.1",
                                         "Accept-Encoding": "identity", "Connection": "close"})
    answer = conn.getresponse()
    return answer.status, {k.lower(): v for k, v in answer.getheaders()}, answer.read


def _ask(host: str, addresses: list[str], target: str, send: Callable[..., Any], left: float) -> Any:
    """The answer of the first address that answers, each tried in turn; ``Unreachable`` naming every one when none
    does, and ``Unread`` when the server's certificate does not hold."""
    import http.client
    import ssl
    failed: list[str] = []
    for ip in addresses:
        try:
            return send(host, ip, target, left)
        except ssl.SSLCertVerificationError:
            raise Unread("its certificate does not hold") from None
        except (OSError, http.client.HTTPException) as e:
            failed.append(f"{ip} ({getattr(e, 'strerror', None) or e or type(e).__name__})")
    raise Unreachable(f"no address of {host} answered: {', '.join(failed)}")


def _wait(n: int, headers: dict[str, str]) -> float:
    """How long to wait before asking again: the server's Retry-After in seconds, up to a limit, else the n-th wait."""
    import math
    try:
        wait = float(headers.get("retry-after", ""))
    except ValueError:
        return PAGE_WAITS[n]
    return min(max(wait, 0.0), PAGE_WAIT_MOST) if math.isfinite(wait) else PAGE_WAITS[n]


def download(url: str, *, resolve: Callable[[str], list[str]] = _resolve, send: Callable[..., Any] = _send,
             clock: Callable[[], float] | None = None, sleep: Callable[[float], None] | None = None) -> dict[str, Any]:
    """Download one page under the rules above: ``{final_url, type, body, asked}``, with every address asked in
    order; ``Unread`` when the source said no or does not exist, or ``Unreachable`` when Jaynes could not ask, each
    with the reason and with the addresses asked in ``asked``."""
    import http.client
    import time
    from urllib.parse import urljoin
    clock = clock or time.monotonic
    sleep = sleep or time.sleep
    asked: list[str] = []
    slow = f"it took longer than {PAGE_SECONDS:.0f} s to come"
    try:
        for _hop in range(PAGE_REDIRECTS + 1):
            host, target = _address(url)
            addresses = _lookup(host, resolve)
            if not addresses or not all(_public(a) for a in addresses):
                raise Unread("its address is not on the public internet")
            asked.append(url)
            for n in range(len(PAGE_WAITS) + 1):
                deadline = clock() + PAGE_SECONDS
                status, headers, read = _ask(host, addresses, target, send, PAGE_SECONDS)
                if status != 429 and not 500 <= status <= 599:
                    break
                if n == len(PAGE_WAITS):
                    raise Unreachable(f"the server answered {status} {n + 1} times, with waits between")
                sleep(_wait(n, headers))
            if status in (301, 302, 303, 307, 308) and headers.get("location"):
                url = urljoin(url, headers["location"])
                continue
            if status != 200:
                raise Unread(f"the server answered {status}")
            # what the headers say decides before any of the body comes: a slow large PDF is the source's no
            kind = _kind(headers.get("content-type", ""))
            if kind not in _HTML_TYPES and kind not in _TEXT_TYPES:
                raise Unread(f"its format ({kind or 'not said in a form Jaynes reads'}) is not one Jaynes reads")
            length = headers.get("content-length", "")
            if length.isdigit() and int(length) > PAGE_BYTES:
                raise Unread(f"it is larger than {PAGE_BYTES // 1_000_000} MB")
            body = b""
            # an http.client answer is read as it comes, so a server that sends a byte at a time meets the clock
            read = getattr(getattr(read, "__self__", None), "read1", read)
            try:
                while len(body) <= PAGE_BYTES:
                    if clock() > deadline:
                        raise Unreachable(slow)
                    piece = read(min(65536, PAGE_BYTES + 1 - len(body)))
                    if not piece:
                        break
                    body += piece
            except (OSError, http.client.HTTPException) as e:
                raise Unreachable(f"the connection dropped while the page came ({getattr(e, 'strerror', None) or e or type(e).__name__})") from None
            if len(body) > PAGE_BYTES:
                raise Unread(f"it is larger than {PAGE_BYTES // 1_000_000} MB")
            return {"final_url": url, "type": headers.get("content-type", ""), "body": body, "asked": asked}
        raise Unread(f"it redirected more than {PAGE_REDIRECTS} times")
    except (Unread, Unreachable) as e:
        e.asked = asked  # type: ignore[attr-defined]
        raise


def _child_main() -> None:
    """What runs in the process ``within`` starts: reads which function to call and with what, as a pickle on
    standard input, and writes what came of it, as a pickle on standard output."""
    import importlib
    import pickle
    module, name, args = pickle.loads(sys.stdin.buffer.read())
    try:
        fn = importlib.import_module(module)
        for part in name.split("."):
            fn = getattr(fn, part)
        done: tuple[str, Any, Any] = ("ok", fn(*args), None)
    except Unread as e:
        done = ("unread", str(e), getattr(e, "asked", None))
    except Unreachable as e:
        done = ("unreachable", str(e), getattr(e, "asked", None))
    except BaseException as e:  # noqa: BLE001: whatever stopped it is said, as Jaynes's own fault
        done = ("error", f"{type(e).__name__}: {e}", None)
    sys.stdout.buffer.write(pickle.dumps(done))
    sys.stdout.buffer.flush()


def within(seconds: float, fn: Callable[..., Any], *args: Any, late: type = None, says: str = "") -> Any:  # type: ignore[assignment]
    """``fn(*args)`` in a program of its own, killed when ``seconds`` run out, so nothing it does (a lookup, a
    server that sends a byte at a time, a page slow to take apart) outlasts its clock. Out of time, it raises
    ``late`` (``Unreachable`` unless said otherwise) with ``says``; a program that died is ``Unreachable``, Jaynes's
    fault. ``fn`` is a function of a module, called by name in the new program, which does not run the caller's
    own program again."""
    import os
    import pickle
    import subprocess
    late = late or Unreachable
    module = fn.__module__
    if module == "__main__":
        module = Path(sys.modules["__main__"].__file__ or "").stem
    here = Path(__file__).resolve().parent
    # the program imports Jaynes's own library and the function's module, and nothing of the folder it is called
    # from (-I: no current folder, no PYTHONPATH, no user site); it stays in the step's process group, so a halt
    # or the engine's own deadline, which stop that group, stop it too
    home = getattr(sys.modules.get(fn.__module__), "__file__", None)
    paths = [str(here)] + ([str(Path(home).resolve().parent)] if home and Path(home).resolve().parent != here else [])
    starter = f"import sys; sys.path[:0] = {paths!r}; import jaynes_menu; jaynes_menu._child_main()"
    proc = subprocess.Popen([sys.executable, "-I", "-c", starter], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, cwd=str(here), env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"})
    try:
        out, _err = proc.communicate(pickle.dumps((module, fn.__qualname__, args)), timeout=seconds)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.communicate()
        raise late(says or f"it took longer than {seconds:.0f} s in all") from None
    if proc.returncode != 0 or not out:
        raise Unreachable(f"Jaynes's reading of it stopped (exit code {proc.returncode})")
    kind, value, asked = pickle.loads(out)
    if kind == "ok":
        return value
    error: Exception = Unread(value) if kind == "unread" else Unreachable(value) if kind == "unreachable" else Unreachable(f"Jaynes could not read it ({value})")
    error.asked = asked  # type: ignore[attr-defined]
    raise error


def _key(url: str) -> str:
    """An address as pages are found by: without its section."""
    return url.split("#", 1)[0]


def _hostname(url: str) -> str:
    from urllib.parse import urlsplit
    return (urlsplit(url).hostname or "").lower()


def _host(url: str) -> str:
    """A page's host as two are compared, without a leading www."""
    host = _hostname(url)
    return host[4:] if host.startswith("www.") else host


def _quotes_of(item: dict[str, Any]) -> list[str]:
    """The quotes an item carries: ``quote``, one, and ``quotes``, a list."""
    one = [item["quote"]] if isinstance(item.get("quote"), str) else []
    more = [q for q in item["quotes"] if isinstance(q, str)] if isinstance(item.get("quotes"), list) else []
    return one + more


def _quoteless(item: dict[str, Any]) -> bool:
    """Whether an item brings no quote, or brings one that is not text."""
    bad_one = "quote" in item and not isinstance(item["quote"], str)
    bad_more = "quotes" in item and not (isinstance(item["quotes"], list) and all(isinstance(q, str) for q in item["quotes"]))
    return bad_one or bad_more or not _quotes_of(item)


def _pages_asked(listed: Any) -> dict[str, list[str] | None] | None:
    """The pages a list names, each with the quotes listed with it (None when the list is of addresses alone); None
    when it is not a list of pages. A page listed twice, with different sections after #, is one page."""
    if not isinstance(listed, list) or not listed:
        return None
    asked: dict[str, list[str] | None] = {}
    first: dict[str, str] = {}  # each page by its address without a section, as it was first listed
    for item in listed:
        if isinstance(item, str) and item.strip():
            url = first.setdefault(_key(item), item)
            asked.setdefault(url, None)
        elif isinstance(item, dict) and isinstance(item.get("url"), str) and item["url"].strip():
            url = first.setdefault(_key(item["url"]), item["url"])
            asked[url] = (asked.get(url) or []) + _quotes_of(item)
        else:
            return None
    return asked


def _fingerprint(data: bytes) -> str:
    import hashlib
    return hashlib.sha256(data).hexdigest()


def _found(quote: str, prepared: Prepared, asked: list[str]) -> tuple[bool, str, str]:
    """Whether a quote is proved by a page read, why not, and, when its words are not there, the text Jaynes read
    nearest to it, for the agent to copy what stands there."""
    if len(same_words(quote)) < QUOTE_CHARS:
        return False, f"too short to prove anything: quote at least {QUOTE_CHARS} characters", ""
    if _echoes(quote, asked):
        return False, "the page's own address carries these words, so the page may only be repeating what it was asked", ""
    if not prepared.holds(quote):
        return False, "these words are not in the text Jaynes downloaded", prepared.nearest(quote)
    return True, "", ""


def _near(text: str) -> str:
    return f"; the nearest text Jaynes read there: \"{text}\"" if text else ""


def read_page(url: str, get: Callable[[str], dict[str, Any]] | None = None) -> dict[str, Any]:
    """One page as fetch-pages records it: downloaded and taken apart, or why not; ``Unreachable`` when Jaynes could
    not ask. Without ``get``, this is the real download: in a program of its own within PAGE_WALL, and the text
    taken apart in another within PARSE_SECONDS, whose running out is the source's answer (a page too slow to read)."""
    real = get is None
    page: dict[str, Any] = {"url": url, "read": False, "why": "", "final_url": "", "came_from": "", "asked": [url], "type": "",
                            "bytes": 0, "sha256": "", "text_sha256": "", "chars": 0, "text": ""}
    try:
        got = within(PAGE_WALL, download, url) if real else get(url)  # type: ignore[misc]
        body = got["body"]
        page.update(final_url=str(got["final_url"])[:URL_CHARS], asked=got.get("asked") or [url], type=_kind(got["type"]),
                    bytes=len(body), sha256=_fingerprint(body))
        if _host(got["final_url"]) != _host(url):
            page["came_from"] = _hostname(got["final_url"])
        text = (within(PARSE_SECONDS, page_text, body, got["type"], late=Unread, says=f"taking its text apart took longer than {PARSE_SECONDS:.0f} s")
                if real else page_text(body, got["type"]))
        page.update(read=True, text=text, text_sha256=_fingerprint(text.encode()), chars=len(text))
    except Unread as e:
        page.update(why=str(e), asked=getattr(e, "asked", None) or page["asked"])
    return page


def _not_read(url: str, why: str) -> dict[str, Any]:
    return {"url": url, "read": False, "why": why, "final_url": "", "came_from": "", "asked": [url], "type": "",
            "bytes": 0, "sha256": "", "text_sha256": "", "chars": 0, "text": ""}


CANARY_URL = "https://example.com/"  # a page that always answers, asked when no page of a list did


def canary_answers() -> bool:
    """Whether a page that always answers does, now: when it does and a page of a list did not, the page is the
    source's problem, not the machine's."""
    try:
        within(PAGE_WALL / 3, download, CANARY_URL)
        return True
    except Unread:
        return True  # it answered, even if with a no
    except Unreachable:
        return False


def fetch_pages(call: Call, params: Params, *, download: Callable[[str], dict[str, Any]] | None = None) -> Any:
    """Download the pages an earlier step lists. The output says, for each, whether it was read and why not, every
    address asked for it, the host it came from when a redirect took it to another, and the fingerprint of what
    came; ``unread`` has a line for each page not read, with why, for a sentence to the person; ``texts`` keeps the
    text of each page read; and, for the quotes listed with a page, whether each is in its text, and why not. A
    page the source refused is a page not read, and nothing quoted from it is found. A page that did not answer is
    a page not read too when another page of the list, or a page that always answers, did; when nothing answers,
    the step stops with ``Unreachable``, naming every page: what is proved must not depend on whether the network
    held, and a run must not stop for ever over one source that never answers."""
    from concurrent.futures import ThreadPoolExecutor
    step, _, field = str(params["pages"]).partition(".")
    asked = _pages_asked((_given(call).get(step) or {}).get(field))
    if asked is None:
        return Refused(f"{params['pages']} lists no pages: it is a list of https addresses, or of items that each have a url")
    if len(asked) > PAGES_MAX:
        return Refused(f"{params['pages']} lists {len(asked)} pages, and a step downloads at most {PAGES_MAX}")
    unreachable: dict[str, str] = {}

    def one(url: str) -> dict[str, Any] | None:
        try:
            return read_page(url, download)
        except Unreachable as e:
            unreachable[url] = str(e)
            return None

    with ThreadPoolExecutor(max_workers=4) as pool:
        done = list(pool.map(one, asked))
    if unreachable:
        # A page that did not answer is the source's only when something else did: another page of the list, or a
        # page that always answers. When nothing answers, it is the machine or the network.
        if any(p is not None and p["read"] for p in done):
            others = "other pages did"
        elif canary_answers():
            others = "a page that always answers did"
        else:
            raise Unreachable(f"Jaynes could not download {'; '.join(f'{u}: {w}' for u, w in sorted(unreachable.items()))}. Nothing answered, "
                              "so that is the machine or the network, not the agent: no attempt was spent, and continuing the run "
                              "downloads every page again")
        done = [p if p is not None else _not_read(url, f"it did not answer, while {others}: {unreachable[url]}") for p, url in zip(done, asked)]
    for page, url in zip(done, asked):
        quotes = asked[url]
        if quotes is not None and page is not None:
            prepared = Prepared(page["text"]) if page["read"] else None
            page["quotes"] = []
            for q in quotes:
                found, why, near = _found(q, prepared, page["asked"]) if prepared else (False, f"the page was not read: {page['why']}", "")
                where = prepared.locate(q) if found and prepared else None
                block, at, window = where if where else (None, None, "")
                page["quotes"].append({"quote": q, "found": found, "why": why + _near(near), "block": block, "at": at, "window": window})
    pages = [p for p in done if p is not None]
    texts = {p["url"]: p.pop("text") for p in pages if p["read"]}
    for p in pages:
        p.pop("text", None)
    return {"pages": pages, "texts": texts, "any_read": any(p["read"] for p in pages),
            "unread": [f"{p['url']}: {p['why']}" for p in pages if not p["read"]]}


def quotes_found(call: Call, params: Params) -> Any:
    """Every quote the output lists, each with the url of its page (as listed, with a section or not, or the address
    the page came from), is in the text Jaynes downloaded from that page, by the rule above. An item with no quote,
    a quote from a page that was not read or not downloaded at all, or one the page's address carries, is not
    proved."""
    field, step = str(params["field"]), str(params["pages"])
    fetched = _given(call).get(step)
    pages = fetched.get("pages") if isinstance(fetched, dict) else None
    texts = fetched.get("texts") if isinstance(fetched, dict) else None
    if not isinstance(pages, list) or not isinstance(texts, dict) or not all(isinstance(p, dict) for p in pages):
        return Blocked(f"the check was not given the pages {step} downloaded")
    by_url: dict[str, dict[str, Any]] = {}
    # a page is found by the address it was listed by, and else by where it came from: when A redirects to B and both
    # are listed, B is B's own page
    for u, p in [(p.get("final_url"), p) for p in pages] + [(p.get("url"), p) for p in pages]:
        if isinstance(u, str) and u:
            by_url[_key(u)] = p
    for p in pages:
        if p.get("read"):
            text = texts.get(str(p.get("url")))
            if not isinstance(text, str) or _fingerprint(text.encode()) != p.get("text_sha256"):
                return Blocked(f"the text kept from {p.get('url')} is not the text that was downloaded")
    output = call.get("output")
    listed = output.get(field) if isinstance(output, dict) else None
    if not isinstance(listed, list) or not all(isinstance(i, dict) for i in listed):
        return Fail(f"{field} is a list of quotes, each with the url of its page and the quote")
    reasons: list[str] = []
    prepared: dict[str, Prepared] = {}
    count = 0
    for n, item in enumerate(listed, 1):
        if _quoteless(item):
            reasons.append(f"{field}, item {n} has no quote: give quote, the words as text, or quotes, a list of them")
        url = str(item.get("url") or "")
        for quote in _quotes_of(item):
            count += 1
            short = quote if len(quote) <= 80 else quote[:77] + "..."
            page = by_url.get(_key(url))
            moved = f" (the page came from {page['came_from']})" if page and page.get("came_from") else ""
            if page is None:
                reasons.append(f"\"{short}\": its page, {url or 'none named'}, was not downloaded by Jaynes in this run")
            elif not page.get("read"):
                reasons.append(f"\"{short}\": Jaynes could not read {url} ({page.get('why')}), so nothing quoted from it is proved{moved}")
            else:
                key = str(page.get("url"))
                if key not in prepared:
                    prepared[key] = Prepared(texts.get(key, ""))
                found, why, near = _found(quote, prepared[key], page.get("asked") or [key])
                if not found:
                    said = f"{why} from {url}{moved}" if why.startswith("these words") else f"{why}{moved}"
                    reasons.append(f"\"{short}\": {said}{_near(near)}")
    if count == 0 and not reasons:
        return Fail(f"there is no quote to prove: {field} lists none")
    return Fail(*reasons[:20]) if reasons else Pass()


def quote_windows(call: Call, params: Params) -> Any:
    """Each quote a later step lists (``claims``, items with a url and a quote), shown in the page Jaynes downloaded
    (``pages``, a fetch-pages step): whether it is there, why not, the fingerprint of the page, where it stands,
    and the window of the page's own text around it. The check of code says only that the words stand there; the
    window is for the person, who judges whether the source says them or only mentions them. Nothing here comes
    from an agent but the quotes and their urls. ``beside``, a field of an earlier step, is copied as it was, to be
    shown with the quotes; ``windows`` is all of it as one text, for a stop."""
    given = _given(call)
    step, _, field = str(params["claims"]).partition(".")
    listed = (given.get(step) or {}).get(field)
    fetched = given.get(str(params["pages"]))
    pages = fetched.get("pages") if isinstance(fetched, dict) else None
    texts = fetched.get("texts") if isinstance(fetched, dict) else None
    if not isinstance(pages, list) or not isinstance(texts, dict) or not all(isinstance(p, dict) for p in pages):
        return Refused(f"{params['pages']} made no pages to show the quotes in: it is a fetch-pages step")
    by_url: dict[str, dict[str, Any]] = {}
    for p in pages:
        for u in (p.get("final_url"), p.get("url")):
            if isinstance(u, str) and u:
                by_url[_key(u)] = p
    prepared: dict[str, Prepared] = {}
    shown: list[dict[str, Any]] = []
    lines: list[str] = []
    for item in listed if isinstance(listed, list) else []:
        if not isinstance(item, dict):
            continue
        url = str(item.get("url") or "")
        for quote in _quotes_of(item):
            n = len(shown) + 1
            page = by_url.get(_key(url))
            entry: dict[str, Any] = {"n": n, "url": url, "quote": quote, "found": False, "why": "", "page_sha256": "",
                                     "block": None, "at": None, "window": "", "places": 0}
            if page is None:
                entry["why"] = "its page was not downloaded by Jaynes in this run"
            elif not page.get("read"):
                entry.update(why=f"the page was not read: {page.get('why')}", page_sha256=page.get("sha256", ""))
            else:
                key = str(page.get("url"))
                text = texts.get(key, "")
                if not isinstance(text, str) or _fingerprint(text.encode()) != page.get("text_sha256"):
                    return Refused(f"the text kept from {key} is not the text that was downloaded")
                if key not in prepared:
                    prepared[key] = Prepared(text)
                found, why, near = _found(quote, prepared[key], page.get("asked") or [key])
                entry.update(found=found, why=why + _near(near), page_sha256=page.get("sha256", ""))
                where = prepared[key].locate(quote) if found else None
                if where:
                    entry.update(block=where[0], at=where[1], window=where[2], places=prepared[key].places(quote))
            shown.append(entry)
            seen = entry["window"] if entry["found"] else f"(not on the page: {entry['why']})"
            if entry["places"] > 1:
                seen = f"(in {entry['places']} places on the page; the first:) {seen}"
            lines.append(f"{n}. \"{quote}\"\n   {url}\n   {seen}")
    out: dict[str, Any] = {"quotes": shown, "judged": len(shown),
                           "windows": "\n\n".join(lines) if lines else "No quotation to show: the note quotes nothing."}
    if params.get("beside"):
        b_step, _, b_field = str(params["beside"]).partition(".")
        out["beside"] = (given.get(b_step) or {}).get(b_field)
    return out


# --- the cases a check comes with ------------------------------------------------------------------------------
#
# A check of the menu comes with the cases it must accept and refuse, in cases/NAME/accept.json and reject.json. A
# case reader turns one case into the check's verdict, the way a run would reach it.

CASE_FILL = " ".join(["This paragraph only makes the page long enough to count as a page with text of its own."] * 4)


def quotes_found_case(case: dict[str, Any]) -> Any:
    """quotes-found's verdict on a case: its page (``page``, with FILL as a long paragraph, encoded in
    ``page_charset`` or UTF-8, of content type ``type``) downloaded by fetch-pages from ``url`` without the network,
    and its ``quote`` cited at ``cite``, or at ``url``."""
    body = case["page"].replace("FILL", CASE_FILL).encode(case.get("page_charset") or "utf-8")

    def get(url: str) -> dict[str, Any]:
        if url != case["url"]:
            raise Unread("the server answered 404")
        return {"final_url": url, "type": case["type"], "body": body, "asked": [url]}

    call = {"given": [{"step": "read", "output": {"sources": [{"url": case["url"], "quotes": [case["quote"]]}]}}]}
    fetched = fetch_pages(call, {"pages": "read.sources"}, download=get)
    claim = {"url": case.get("cite") or case["url"], "quote": case["quote"]}
    return quotes_found({"output": {"claims": [claim]}, "given": [{"step": "fetch", "output": fetched}]}, {"field": "claims", "pages": "fetch"})


def _case_public(ip: str) -> bool:
    """Whether a case's reader takes an address as on the public internet: by its own rule and by the menu's, so what
    either calls off the internet is, and the reader stays as strict as the step at the least."""
    import ipaddress
    try:
        a = ipaddress.ip_address(ip.split("%")[0])
    except ValueError:
        return False
    own = "%" not in ip and a.is_global and not a.is_multicast and not (a.version == 6 and a.ipv4_mapped is not None)
    return own and _public(ip)


def fetch_pages_case(case: dict[str, Any]) -> Any:
    """What fetch-pages does with a case, as a run reads a page (download, then its text), with no network. A case
    gives the ``url`` an agent listed; ``resolve``, what each name resolves to (left out, and always for an address
    written as a number or for localhost, the machine's own resolver, which needs no network for them); and
    ``answers``, what the server answers at each address asked: ``status``, ``headers``, ``body`` (FILL in it is a
    long paragraph, and ``body_bytes`` makes it that many bytes long), and ``endless`` for an answer that never ends.
    ``Pass`` when the page is read, or when anything was sent to an address that is not on the public internet,
    though nothing was read: both let the agent's address reach what it may not. ``Fail``, with why, when it was not
    read and nothing went where it may not."""
    import io
    import ipaddress
    import socket
    sent: list[str] = []
    now = [0.0]

    def numeric(host: str) -> bool:
        """An address written as a number, or localhost: what the machine's resolver reads with no network."""
        h = host.lower().rstrip(".")
        if h == "localhost":
            return True
        try:
            ipaddress.ip_address(h.split("%")[0])
            return True
        except ValueError:
            pass
        try:  # what the machine reads as an IPv4 address in any of its forms, and nothing else
            socket.inet_aton(h)
            return True
        except OSError:
            return False

    def resolve(host: str) -> list[str]:
        if numeric(host):
            return _resolve(host)
        # a name is only what the case says it resolves to: none, and it does not exist
        if host not in (case.get("resolve") or {}):
            raise socket.gaierror(socket.EAI_NONAME, "nodename nor servname provided, or not known")
        return list(case["resolve"][host])

    def answer_at(host: str, target: str) -> dict[str, Any] | None:
        for url, said in (case.get("answers") or {}).items():
            try:
                h, t = _address(url)
            except Unread:
                continue
            if h == host and t == target:
                return said
        return None

    def send(host: str, ip: str, target: str, timeout: float, **_: Any) -> Any:
        sent.append(ip)
        said = answer_at(host, target)
        if said is None:
            raise ConnectionRefusedError(f"nothing answers at {host}{target}")
        body = str(said.get("body", "")).replace("FILL", CASE_FILL).encode()
        if said.get("body_bytes"):
            body = body + b" " * max(0, int(said["body_bytes"]) - len(body))
        if said.get("endless"):
            def more(_n: int) -> bytes:
                now[0] += 0.5
                return b"<p>more</p>"
            return said["status"], dict(said.get("headers") or {}), more
        return said["status"], dict(said.get("headers") or {}), io.BytesIO(body).read

    def get(url: str) -> dict[str, Any]:
        return download(url, resolve=resolve, send=send, clock=lambda: now[0], sleep=lambda _s: None)

    try:
        page = read_page(case["url"], get)
    except Unreachable as e:
        page = {"read": False, "why": f"Jaynes could not read it in time: {e}"}
    # what was sent where it may not is let through, read or not
    if page["read"] or any(not _case_public(ip) for ip in sent):
        return Pass()
    return Fail(str(page["why"]))


CASE_READERS: dict[str, Callable[[dict[str, Any]], Any]] = {"quotes-found": quotes_found_case, "fetch-pages": fetch_pages_case}


# --- what a role needs of the project, before a run ------------------------------------------------------------

def requires(params: Params, project: Path) -> Any:
    """Whether the person's ``project`` has what a role needs, reading it only: ``Pass``, ``Fail`` with why, or
    ``Blocked`` when Jaynes could not tell. ``use: "suite"``: tests find-suite finds and the machine can run.
    ``script``: a script of the role, run in the box on a throwaway copy of the project, with no network and at most
    ``seconds``; exit 0 when the project has it, 1 when it has not, with why."""
    if params.get("use") == "suite":
        found = box.detect(project, "")
        if found is None:
            return Fail(NO_RUNNER)
        try:
            why = box.missing(project, found)
        except box.Unavailable as e:
            return Blocked(f"there is no box to find whether the project's tests run: {e}")
        return Fail(why) if why else Pass()
    path = str(params.get("script") or "")
    name = Path(path).name
    try:
        done = box.run_step(_quote(path) if _executable(path) else f"sh {_quote(path)}", project=project,
                            timeout=float(params.get("seconds") or 60), stdin=json.dumps({"project": "."}),
                            role=Path(str(params["role"])) if params.get("role") else None)
    except box.Unavailable as e:
        return Blocked(f"there is no box to run {name} in: {e}")
    ran = done.ran
    said = [line.strip() for line in ran.printed.splitlines() if line.strip()]
    if box.refused(ran):
        return Blocked(f"the box refused what {name} tried: " + "; ".join(box.refused(ran)[:3]))
    if ran.code == 0:
        return Pass()
    if ran.code == 1:
        return Fail(" ".join(said[-5:]) or f"{name} says the project lacks it, and not why")
    if ran.code == 124:
        return Blocked(f"{name} took more than {params.get('seconds') or 60} seconds")
    return Blocked(f"{name} exited with {ran.code}" + (": " + " ".join(said[-5:]) if said else ""))


# --- the cases that come with a check ---------------------------------------------------------------------------

def judge_case(use: str, params: Params, case: dict[str, Any]) -> Any:
    """What the check ``use`` says of one of its cases, run as in a run: ``Pass``, ``Fail``, or ``Blocked`` when it
    could not judge. A case is the check's input: ``output``, the output it judges; ``given``, the outputs of the
    steps its step uses; ``project``, a map of paths to text, the folder it sees (none: it sees no project); and
    ``with``, the parameters a check of the menu takes. ``use`` is ``script`` for a role's own check, whose path is
    in ``params``."""
    files = case.get("project") or {}
    if not isinstance(files, dict):
        return Blocked("the case's project is a map of paths to text")
    with tempfile.TemporaryDirectory(prefix="jaynes-case-") as folder:
        for path, text in files.items():
            if os.path.isabs(path) or ".." in Path(path).parts or not isinstance(text, str):
                return Blocked(f"the case names {path}, which is not a text file inside its project")
            target = Path(folder) / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(text)
        start = case.get("start")
        call = {"output": case.get("output"), "given": case.get("given") or [], "request": case.get("request") or "",
                "project": folder if files else None, "source": None,
                "start": {"files": start} if isinstance(start, dict) else None}
        if use == "script":
            return script(call, params)
        if use in STEP_CASES:
            # a step's case: a reject is a refusal, told as a failure in the step's words; a step that goes on passes
            try:
                did = STEPS[use](call, {**params, **(case.get("with") or {})})
            except Exception as e:
                return Blocked(f"{use} could not read the case: {type(e).__name__}: {e}")
            return Fail(did.reason) if isinstance(did, Refused) else did if isinstance(did, Blocked) else Pass()
        if use not in CHECKS:
            return Blocked(f"the menu has no check {use}")
        try:
            return CHECKS[use](call, {**params, **(case.get("with") or {})})
        except Exception as e:  # a case the check cannot read is a case that did not run, never a rejection
            return Blocked(f"{use} could not read the case: {type(e).__name__}: {e}")


STEPS: dict[str, Callable[[Call, Params], Any]] = {"find-suite": find_suite, "refuse-when-false": refuse_when_false,
                                                       "fetch-pages": fetch_pages, "quote-windows": quote_windows,
                                                       "script": script_step}
# The steps whose cases judge_case runs, as it runs a check's: those whose input is a project and what they are given
STEP_CASES = ("find-suite", "refuse-when-false")
CHECKS: dict[str, Callable[[Call, Params], Any]] = {
    "suite-passes": suite_passes,
    "tests-fail-for-the-bug": tests_fail_for_the_bug,
    "tests-pass": tests_pass,
    "hidden-cases-pass": hidden_cases_pass,
    "output-follows-schema": output_follows_schema,
    "field-required": field_required,
    "field-empty": field_empty,
    "field-max-length": field_max_length,
    "files-within": files_within,
    "quotes-found": quotes_found,
    "plain-words": plain_words,
    "named-in-request": named_in_request,
    "script": script,
}


def answer(ask: dict[str, Any]) -> dict[str, Any]:
    use, params, call = str(ask.get("use")), ask.get("with") or {}, ask.get("call") or {}
    if ask.get("kind") == "requires":
        return {"reply": "verdict", "verdict": _verdict(requires(params, Path(str(ask.get("project") or "."))))}
    if ask.get("kind") == "case":
        return {"reply": "verdict", "verdict": _verdict(judge_case(use, params, ask.get("case") or {}))}
    if ask.get("kind") == "step":
        if use not in STEPS:
            return {"reply": "error", "message": f"the menu has no step {use}"}
        try:
            return _result(STEPS[use](call, params))
        except Unreachable as e:  # Jaynes could not do its own step: its fault, never the agent's
            return {"reply": "error", "message": str(e)}
    if use not in CHECKS:
        return {"reply": "error", "message": f"the menu has no check {use}"}
    return {"reply": "verdict", "verdict": _verdict(CHECKS[use](call, params))}


if __name__ == "__main__":
    try:
        body = answer(json.loads(sys.stdin.readline()))
    except Exception:  # the engine counts a menu that cannot answer as its own fault, never the agent's
        body = {"reply": "error", "message": traceback.format_exc(limit=5)[-2000:]}
    sys.stdout.write(json.dumps(body) + "\n")
    sys.stdout.flush()
