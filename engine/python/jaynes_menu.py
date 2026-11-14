"""The menu: the steps and checks of Jaynes's own that a role written in TOML picks by name.

Each is written once, here, beside the box where it runs a project's commands, and tested once. The program that
serves a TOML role (``jaynes role-serve``) asks for one with a line of JSON on standard input::

    {"kind": "step" | "check", "use": NAME, "with": {params}, "call": {the protocol's request fields}}

and reads one line back: the body of the protocol's reply (``result``, ``refused``, ``verdict`` or ``error``).
What a name means and which parameters it takes is checked before a run, by the engine; here a name is done.

A check the menu lacks is a script of the role (``use: "script"``): it runs in the box on a copy of the project,
reads ``{"output", "given", "project"}`` on standard input, and exits 0 to pass, 1 to fail with its reasons on
standard output, 3 when it cannot judge.
"""

from __future__ import annotations

import json
import re
import sys
import traceback
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


def _box(call: Call, command: str, timeout: float = 600.0, stdin: str | None = None) -> Any:
    """Run ``command`` on a copy of the run's project in the box: a ``Ran``, or ``Blocked``."""
    if not call.get("project"):
        return Blocked("the check has no copy of the project to run commands on")
    try:
        ran = box.run(Path(call["project"]), command, source=_source(call), timeout=timeout, stdin=stdin)
    except box.Unavailable as e:
        return Blocked(f"there is no box to run the project's commands in: {e}")
    if box.refused(ran):
        return Blocked("the box refused what the command tried, so its failure says nothing about the project: "
                       + "; ".join(box.refused(ran)[:3]))
    return ran


# --- steps -----------------------------------------------------------------------------------------------------

NO_RUNNER = ("no test runner was found in the project: Jaynes looks for pytest, a test script in package.json, "
             "go.mod, Cargo.toml and a Makefile test target")


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
    why = box.missing(Path(project), found, source=_source(call))
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


# --- checks ----------------------------------------------------------------------------------------------------

def _suite(call: Call, params: Params) -> dict[str, Any]:
    return _given(call).get(str(params.get("suite_step") or "")) or {}


def suite_passes(call: Call, params: Params) -> Any:
    command = str(_suite(call, params).get("suite") or "")
    if not command:
        return Pass()
    ran = _box(call, command)
    if isinstance(ran, Blocked):
        return ran
    if ran.code != 0:
        return Fail(f"the suite fails: {command} exited with {ran.code}. The end of what it printed:\n{ran.output}")
    return Pass()


def _allowed(path: str, globs: list[str]) -> bool:
    """Whether ``path`` matches a glob of ``globs`` and none of its ``!`` globs."""
    def match(g: str) -> bool:
        return fnmatchcase(path, g) or (g.startswith("**/") and fnmatchcase(path, g[3:]))
    return any(match(g) for g in globs if not g.startswith("!")) and not any(match(g[1:]) for g in globs if g.startswith("!"))


def files_within(call: Call, params: Params) -> Any:
    field = str(params.get("field") or "files")
    files = [str(f) for f in (call.get("output") or {}).get(field) or []] if isinstance(call.get("output"), dict) else []
    outside = [f for f in files if not _allowed(f, list(params.get("write") or []))]
    if outside:
        return Fail(*(f"{f} is not a file this step may change" for f in outside))
    return Pass()


def _for_files(runner: dict[str, Any], tests: list[str]) -> str:
    return box.for_files(box.Runner.from_dict(runner), tests)


# What a quoted failure must say beyond: a line made only of these words says nothing of why.
STATUS = {"fail", "failed", "failure", "failures", "error", "errors", "not", "ok", "exit", "status", "code", "test",
          "tests", "setup", "build"}


def _quoted(failure: str, ran: Any, why: str) -> Any:
    quote = failure.strip()
    if all(w in STATUS for w in re.findall(r"[a-z]+", quote.lower())):
        return Fail(f"{quote} names no reason; quote the line that says why the tests fail {why}, such as the "
                    f"assertion message. The end of what it printed (exit {ran.code}):\n{ran.output}")
    if quote not in ran.printed:
        return Fail(f"{quote} is not in what the command printed; quote a line it prints {why}. The end of what it "
                    f"printed (exit {ran.code}):\n{ran.output}")
    return None


def tests_fail_for_the_bug(call: Call, params: Params) -> Any:
    """The tests the output names in ``tests`` are files the step may write, exist, and fail when run, printing the
    line the output quotes in ``failure``. An output that says ``reproduced`` is false passes: no test shows the bug,
    and a later step stops the run for the person."""
    out = call.get("output") if isinstance(call.get("output"), dict) else {}
    if out.get("reproduced") is False:
        return Pass()
    reasons = [f"{name} is empty; a reproduced bug needs it" for name in ("tests", "failure") if not out.get(name)]
    if reasons:
        return Fail(*reasons)
    tests = [str(t) for t in out.get("tests") or []]
    project = Path(call.get("project") or ".")
    write = list(params.get("write") or [])
    reasons = [f"{t} is not a file this step may write; tests go in paths such as tests/ or *_test.*" for t in tests
               if not _allowed(t, write)]
    reasons += [f"{t} does not exist in the project" for t in tests if not (project / t).is_file()]
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
    return _quoted(str(out.get("failure") or ""), ran, "because of the bug") or Pass()


def tests_pass(call: Call, params: Params) -> Any:
    """The tests an earlier step named in its ``tests`` pass, and then the whole suite."""
    runner = _suite(call, params)
    if not runner.get("suite"):
        return Blocked("the check was not given how the project's tests run")
    tests = [str(t) for t in (_given(call).get(str(params["tests_from"])) or {}).get("tests") or []]
    for what, command in (("the tests still fail", _for_files(runner, tests)), ("the suite fails", runner["suite"])):
        ran = _box(call, command)
        if isinstance(ran, Blocked):
            return ran
        if ran.code != 0:
            return Fail(f"{what}: {command} exited with {ran.code}. The end of what it printed:\n{ran.output}")
    return Pass()


URL = "JAYNES_URL"
CASE_SECONDS = 120.0


def hidden_cases_pass(call: Call, params: Params) -> Any:
    """The cases a hidden step wrote (``cases``: ``says`` and ``run``) hold on the changed project. A failure tells
    the maker only the case's sentence; the person sees each case's result."""
    cases = (_given(call).get(str(params.get("cases_step") or "")) or {}).get("cases") or []
    failed: list[str] = []
    detail: list[str] = []
    for case in cases:
        says, command = str(case.get("says", "")), str(case.get("run", ""))
        if re.search(rf"\${{?{URL}\b", command):
            detail.append(f"Not run, the run serves no app: {says}")
            continue
        ran = _box(call, command, timeout=CASE_SECONDS)
        if isinstance(ran, Blocked):
            return ran
        detail.append(f"{'Passes' if ran.code == 0 else 'Fails'}: {says}")
        if ran.code != 0:
            failed.append(f"A check you cannot see found that this does not hold: {says}")
    if failed:
        return Fail(*failed, detail="\n".join(detail))
    return Pass()


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
        for name in schema.get("required") or []:
            v = value.get(name)
            if v is None or (isinstance(v, str) and not v.strip()):
                out.append(f"{at} has no {name}" if at != "the output" else f"the output has no {name}")
        for name, v in value.items():
            if name in props and v is not None:
                _against(v, props[name], f"{name}" if at == "the output" else f"{at} {name}", out)


def output_follows_schema(call: Call, params: Params) -> Any:
    reasons: list[str] = []
    _against(call.get("output"), params.get("schema") or {}, "the output", reasons)
    return Fail(*reasons) if reasons else Pass()


def field_empty(call: Call, params: Params) -> Any:
    """A field the agent fills only to put a matter before the person, such as a test it may not change that stands
    in its way: while it is filled the check fails, with the role's sentence and what the agent wrote."""
    value = _field(call, params)
    if value not in (None, "", [], {}) and not (isinstance(value, str) and not value.strip()):
        return Fail(f"{params['says']}: {value if isinstance(value, str) else json.dumps(value)}")
    return Pass()


def _field(call: Call, params: Params) -> Any:
    return (call.get("output") or {}).get(str(params["field"])) if isinstance(call.get("output"), dict) else None


def field_required(call: Call, params: Params) -> Any:
    value = _field(call, params)
    if value is None or (isinstance(value, (str, list, dict)) and not (value.strip() if isinstance(value, str) else value)):
        return Fail(f"the output has no {params['field']}")
    return Pass()


def field_max_length(call: Call, params: Params) -> Any:
    value = _field(call, params)
    length = len(value) if isinstance(value, str) else len(json.dumps(value)) if value is not None else 0
    most = int(params["max"])
    if length > most:
        return Fail(f"{params['field']} has {length} characters; keep it to {most}")
    return Pass()


def script(call: Call, params: Params) -> Any:
    """A check of the role's own: a script run in the box on a copy of the project."""
    path = str(params["path"])
    given = json.dumps({"output": call.get("output"), "given": call.get("given") or [], "project": "."})
    ran = _box(call, _quote(path) if _executable(path) else f"sh {_quote(path)}", stdin=given)
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


STEPS: dict[str, Callable[[Call, Params], Any]] = {"find-suite": find_suite, "refuse-when-false": refuse_when_false}
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
    "script": script,
}


def answer(ask: dict[str, Any]) -> dict[str, Any]:
    use, params, call = str(ask.get("use")), ask.get("with") or {}, ask.get("call") or {}
    if ask.get("kind") == "step":
        if use not in STEPS:
            return {"reply": "error", "message": f"the menu has no step {use}"}
        return _result(STEPS[use](call, params))
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
