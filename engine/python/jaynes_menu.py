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
import sys
import traceback
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


STEPS: dict[str, Callable[[Call, Params], Any]] = {"find-suite": find_suite}
CHECKS: dict[str, Callable[[Call, Params], Any]] = {
    "suite-passes": suite_passes,
    "field-required": field_required,
    "field-max-length": field_max_length,
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
