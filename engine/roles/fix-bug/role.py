"""fix-bug, as a role of the redesigned engine: a test that fails because of the bug, then a fix that makes it pass
with the whole suite, and proof the maker never sees.

Ported from the Python engine's package (``packages/software/fix_bug.py``). What changed with the protocol:

- The hidden cases are read by a hidden check of the fix (``proof``), not by a step Jaynes runs after it: a case
  that fails sends the fix back with its sentence alone, spending the attempt, as any hidden check does.
- When no test shows the bug, the role's own step ``found`` refuses, and the run stops with what the agent tried:
  send ``reproduce`` back with a hint, or abandon the run. No attempt is spent looking again.
- A test the fix may not change and that stands in its way is said in ``wrong_test``: the open check fails with it,
  so the person sees it, instead of two agents settling it first.
- The fix is merged by Jaynes into the branch the run started from, never pushed; Undo on the run's page reverts it.

The suite and the cases run in the box of ``jaynes_box``, which comes with the role library, on a copy of the
run's project.
"""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any

import jaynes_box as box
from jaynes_role import Blocked, Fail, Pass, Refused, Role, Stop

role = Role("fix-bug", version="2.0.0", delivery="merge")

TESTS = ["**/tests/**", "**/test/**", "**/__tests__/**", "**/spec/**", "**/test_*.py", "**/*_test.py",
         "**/*.test.*", "**/*.spec.*", "**/*_test.go", "**/*_spec.rb", "**/*Test.java", "**/*Tests.cs"]
LONGEST = {"case": 50, "now": 70, "after": 70}
SUMMARY = 400
CASE_SECONDS = 120.0
URL = "JAYNES_URL"
STATUS = {"fail", "failed", "failure", "failures", "error", "errors", "not", "ok", "exit", "status", "code", "test",
          "tests", "setup", "build"}

NO_RUNNER = ("no test runner was found in the project: Jaynes looks for pytest, a test script in package.json, "
             "go.mod, Cargo.toml and a Makefile test target")

WRITE_CASES = (
    "You write the checks a change will be judged by, before anyone makes it. You read only the task and the "
    "project as it is now; whoever makes the change never sees what you write, only the sentence of a check that "
    "fails. Write each case as a sentence a person reads, saying what must hold once the task is done, and a shell "
    "command that exits 0 when it holds and not otherwise. A command runs from the project's root, in a box with no "
    "internet, on the project with the change made. Check what the task states and can be seen from outside: what "
    "a command prints or how it exits, what a file holds. Never check how it is built: no internal names the task "
    "does not give, no exact wording it does not give; match key words without case (grep -qi). A case about "
    "something new must fail on the project as it is now. At most eight cases, each one thing. Put in untestable, "
    "in a sentence each, what the task asks that no command can check from outside. Change no file.")


def _given(call: dict[str, Any]) -> dict[str, Any]:
    return {g["step"]: g["output"] for g in call.get("given") or []}


def _box(call: dict[str, Any], command: str, timeout: float = 600.0) -> Any:
    """Run ``command`` on a copy of the run's project in the box: a ``Ran``, or ``Blocked``."""
    if not call.get("project"):
        return Blocked("the check has no copy of the project")
    try:
        ran = box.run(Path(call["project"]), command, source=Path(call["source"]) if call.get("source") else None,
                      timeout=timeout)
    except box.Unavailable as e:
        return Blocked(f"there is no box to run the project's commands in: {e}")
    if box.refused(ran):
        return Blocked("the box refused what the command tried, so its failure says nothing about the project: "
                       + "; ".join(box.refused(ran)[:3]))
    return ran


def _for_files(runner: dict[str, Any], tests: list[str]) -> str:
    return box.for_files(box.Runner.from_dict(runner), tests)


def _allowed(path: str, globs: list[str]) -> bool:
    from fnmatch import fnmatchcase
    return any(fnmatchcase(path, g) or (g.startswith("**/") and fnmatchcase(path, g[3:])) for g in globs)


# --- the steps ------------------------------------------------------------------------------------------------

def find_runner(call: dict[str, Any]) -> Any:
    found = box.detect(Path(call["project"]), "") if call.get("project") else None
    if found is None:
        return Refused(NO_RUNNER)
    why = box.missing(Path(call["project"]), found, source=Path(call["source"]) if call.get("source") else None)
    if why:
        return Refused(why)
    return found.to_dict()


role.step("runner", executor="role", reach={"project": "read"}, attempts=1, run=find_runner,
          about="Finds how the project's tests run.")

role.step("cases", executor="agent", hidden=True, reach={"project": "read", "commands": True}, attempts=2,
          about="Writes, from the request alone, the checks the fix will be judged by.", instructions=WRITE_CASES,
          schema={"type": "object", "required": ["cases"], "properties": {
              "cases": {"type": "array", "items": {"type": "object", "required": ["says", "run"], "properties": {
                  "says": {"type": "string"}, "run": {"type": "string"}}}},
              "untestable": {"type": "array", "items": {"type": "string"}}}})


@role.check("cases", "written", about="each case has a sentence and a command; at most eight")
def written(call: dict[str, Any]) -> Any:
    cases = (call.get("output") or {}).get("cases") or []
    bad = [f"case {n} has no sentence" for n, c in enumerate(cases, 1) if not str(c.get("says", "")).strip()]
    bad += [f"case {n} has no command" for n, c in enumerate(cases, 1) if not str(c.get("run", "")).strip()]
    if len(cases) > 8:
        bad.append(f"{len(cases)} cases; write at most eight")
    return Fail(*bad) if bad else Pass()


role.step(
    "reproduce", executor="agent", needs=["runner"], reach={"project": "write", "write": TESTS, "commands": True},
    stop=Stop(verdicts=["keep", "redo"], budget=0.2, about="Do these tests show the bug?"),
    about="Writes the smallest test that fails because of the bug.",
    instructions=(
        "Find the bug the task describes and write the smallest test that fails because of it, next to the "
        "project's other tests and in their style. When the task lists what must hold, write a test for each item "
        "now, even one that passes today: the fix may not touch the tests, so what you leave out here goes "
        "unproven. If the fix will change what an existing test expects, change that test now too. Change test "
        "files only; do not fix the bug yet. .jaynes/inputs/runner.json says how Jaynes runs the tests: each "
        "followed by test files runs only them, suite runs them all. Run your tests that way before you answer; "
        "Jaynes runs them again in a box without internet and requires them to fail, printing the line you quote. "
        "If you find no bug a test can show, set reproduced to false, say what you tried and why in bug, and "
        "change nothing."),
    schema={"type": "object", "required": ["bug"], "properties": {
        "bug": {"type": "string", "description": "what goes wrong, in one or two sentences"},
        "reproduced": {"type": "boolean", "default": True},
        "tests": {"type": "array", "items": {"type": "string"}, "description": "test files added or changed"},
        "failure": {"type": "string", "description": "a line the tests print because of the bug, copied exactly"},
        "examples": {"type": "array", "items": {"type": "object", "required": ["case", "now", "after"], "properties": {
            "case": {"type": "string", "description": "a situation the person knows, at most 50 characters"},
            "now": {"type": "string", "description": "what they see today, at most 70 characters"},
            "after": {"type": "string", "description": "what they will see once fixed, at most 70 characters"}}}}}})


def _quoted(failure: str, ran: Any, why: str) -> Any:
    quote = failure.strip()
    if all(w in STATUS for w in re.findall(r"[a-z]+", quote.lower())):
        return Fail(f"{quote} names no reason; quote the line that says why the tests fail {why}, such as the "
                    f"assertion message. The end of what it printed (exit {ran.code}):\n{ran.output}")
    if quote not in ran.printed:
        return Fail(f"{quote} is not in what the command printed; quote a line it prints {why}. The end of what it "
                    f"printed (exit {ran.code}):\n{ran.output}")
    return None


@role.check("reproduce", "fails-for-the-bug", about="the tests are test files and fail because of the bug, "
            "printing the failure quoted; each example is short enough to read at a glance")
def fails_for_the_bug(call: dict[str, Any]) -> Any:
    out = call.get("output") or {}
    if not str(out.get("bug") or "").strip():
        return Fail("say what goes wrong, or, when no test shows it, what you tried and why")
    if out.get("reproduced") is False:
        return Pass()
    reasons = [f"{name} is empty; a reproduced bug needs it" for name in ("tests", "failure", "examples")
               if not out.get(name)]
    reasons += [f"the {f} of example {n} is {len(t)} characters, at most {most}"
                for n, e in enumerate(out.get("examples") or [], 1) for f, most in LONGEST.items()
                if len(t := str(e.get(f, "")).strip()) > most]
    if reasons:
        return Fail(*reasons)
    tests = [str(t) for t in out.get("tests") or []]
    project = Path(call.get("project") or ".")
    reasons = [f"{t} is not a test file; tests go in paths such as tests/ or *_test.*" for t in tests if not _allowed(t, TESTS)]
    reasons += [f"{t} does not exist in the project" for t in tests if not (project / t).is_file()]
    if reasons:
        return Fail(*reasons)
    runner = _given(call).get("runner")
    if not runner:
        return Blocked("the check was not given how the project's tests run")
    command = _for_files(runner, tests)
    ran = _box(call, command)
    if isinstance(ran, Blocked):
        return ran
    if ran.code == 0:
        return Fail(f"the tests pass before any fix; they must fail because of the bug. {command} printed:\n{ran.output}")
    return _quoted(str(out.get("failure") or ""), ran, "because of the bug") or Pass()


@role.items("reproduce")
def show_bug(output: Any) -> list[dict[str, Any]]:
    out = output or {}
    examples = out.get("examples") or []
    if not examples:
        return [{"id": "bug", "label": str(out.get("bug") or "what goes wrong"), "content": out}]
    return [{"id": f"example-{n}", "label": f"{e.get('case', '')}: now {e.get('now', '')}; after the fix {e.get('after', '')}",
             "content": e} for n, e in enumerate(examples, 1)]


def found(call: dict[str, Any]) -> Any:
    out = _given(call).get("reproduce") or {}
    if out.get("reproduced") is False:
        return Refused(f"no test shows the bug: {out.get('bug', '')} Send reproduce back with a hint to look again, "
                       "or abandon the run.")
    return {"reproduced": True}


role.step("found", executor="role", needs=["reproduce"], attempts=1, run=found,
          about="Stops the run when no test shows the bug, so the person decides.")

role.step(
    "fix", executor="agent", needs=["runner", "reproduce", "found"],
    reach={"project": "write", "write": ["**", *(f"!{t}" for t in TESTS)], "commands": True},
    stop=Stop(verdicts=["keep", "redo"], about="Keep this fix?"),
    about="Fixes the bug so its tests and the whole suite pass, without touching the tests.",
    instructions=(
        "Fix the bug so the tests in .jaynes/inputs/reproduce.json pass, and the whole suite with them. Change the "
        "project's code, never its tests: they are how the person knows the bug is gone. Keep the change as small "
        "as the fix needs. Run the tests and the suite yourself as .jaynes/inputs/runner.json says; Jaynes runs them "
        "again in a box without internet. If a test expects what the task changes, say so in wrong_test; never "
        "bend the code to satisfy it."),
    schema={"type": "object", "required": ["summary", "files"], "properties": {
        "summary": {"type": "string", "description": "what was wrong and what the person sees now, in two or three "
                    "plain sentences, no names from the code, at most 400 characters"},
        "files": {"type": "array", "items": {"type": "string"}},
        "wrong_test": {"type": "string", "description": "empty, unless a test you may not change expects what the "
                       "task changes: name it and what it should expect"}}})


@role.check("fix", "fixed", about="the bug's tests pass, and then the whole suite; the summary has at most 400 "
            "characters; no test stands in the way")
def fixed(call: dict[str, Any]) -> Any:
    out = call.get("output") or {}
    if str(out.get("wrong_test") or "").strip():
        return Fail(f"you say a test stands in the way: {out['wrong_test']}. The tests are not yours to change; the "
                    "person decides, so this stays failed until they send reproduce back")
    if len(str(out.get("summary") or "")) > SUMMARY:
        return Fail(f"the summary is {len(out['summary'])} characters, at most {SUMMARY}")
    given = _given(call)
    runner = given.get("runner")
    if not runner:
        return Blocked("the check was not given how the project's tests run")
    tests = [str(t) for t in (given.get("reproduce") or {}).get("tests") or []]
    for what, command in (("the tests still fail", _for_files(runner, tests)), ("the suite fails", runner["suite"])):
        ran = _box(call, command)
        if isinstance(ran, Blocked):
            return ran
        if ran.code != 0:
            return Fail(f"{what}: {command} exited with {ran.code}. The end of what it printed:\n{ran.output}")
    return Pass()


@role.check("fix", "proof", kind="hidden", reads=["cases"],
            about="the cases written from the request alone hold on the fixed project")
def proof(call: dict[str, Any]) -> Any:
    cases = (_given(call).get("cases") or {}).get("cases") or []
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


@role.items("fix")
def show_fix(output: Any) -> list[dict[str, Any]]:
    out = output or {}
    return [{"id": "fix", "label": str(out.get("summary") or "the fix"), "content": out}]


if __name__ == "__main__":
    role.serve()
