"""small-change, as a role of the redesigned engine: the agent makes a small change and Jaynes runs the project's
suite, with no stop. When the rules in code find the change is not small (more lines or files than a small one, or
a file that runs later outside any box), the run's risk goes up and the person sees the change.

The suite runs in the box of ``jaynes_box``, which comes with the role library, on a copy of the run's project.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

import jaynes_box as box
from jaynes_role import Blocked, Fail, Pass, Refused, Role, Stop

role = Role("small-change", version="2.0.0")


def find_suite(call: dict[str, Any]) -> Any:
    """The command that runs the project's whole suite, found from its files, or none."""
    project = call.get("project")
    if not project:
        return Refused("small-change needs a project")
    found = box.detect(Path(project), "")
    if found is None:
        return {"command": ""}
    why = box.missing(Path(project), found, source=Path(call["source"]) if call.get("source") else None)
    if why:
        return Refused(why)
    return {"command": found.suite}


role.step("runner", executor="role", reach={"project": "read"}, attempts=1, run=find_suite,
          about="Finds the command that runs the project's suite.",
          schema={"type": "object", "required": ["command"], "properties": {"command": {"type": "string"}}})

role.step(
    "change", executor="agent", needs=["runner"],
    reach={"project": "write", "write": ["**"], "commands": True},
    stop=Stop(verdicts=["keep", "redo"], from_risk="high", release=True,
              about="The change turned out bigger than a small one. Keep it, or have it done again?"),
    about="Makes the change.",
    instructions=(
        "The task is a small change the person agreed on. Make it in this folder, which is a copy of the project, "
        "keeping it as small as the task needs. .jaynes/inputs/runner.json holds the command that runs the "
        "project's whole suite, empty when it has none: run it yourself before you answer. Jaynes runs it again "
        "without internet and requires it to pass. Do not commit; Jaynes does."),
    schema={"type": "object", "required": ["summary", "files"],
            "properties": {"summary": {"type": "string", "description": "what changed, in a sentence or two"},
                           "files": {"type": "array", "items": {"type": "string"}}}})


@role.check("change", "has-summary", kind="open", about="the output says what changed, in at most 400 characters")
def has_summary(call: dict[str, Any]) -> Any:
    summary = str((call.get("output") or {}).get("summary") or "")
    if not summary.strip():
        return Fail("the output has no summary of what changed")
    if len(summary) > 400:
        return Fail(f"the summary has {len(summary)} characters; keep it to 400")
    return Pass()


@role.check("change", "suite", kind="open", about="the project's whole suite passes, run by Jaynes without internet")
def suite(call: dict[str, Any]) -> Any:
    given = {g["step"]: g["output"] for g in call.get("given") or []}
    command = str((given.get("runner") or {}).get("command") or "")
    if not command:
        return Pass()
    if not call.get("project"):
        return Blocked("the check has no copy of the project to run the suite on")
    try:
        ran = box.run(Path(call["project"]), command, source=Path(call["source"]) if call.get("source") else None)
    except box.Unavailable as e:
        return Blocked(f"there is no box to run the suite in: {e}")
    if box.refused(ran):
        return Blocked("the box refused what the suite tried, so its failure says nothing about the project: "
                       + "; ".join(box.refused(ran)[:3]))
    if ran.code != 0:
        return Fail(f"the suite fails: {command} exited with {ran.code}. The end of what it printed:\n{ran.output}")
    return Pass()


if __name__ == "__main__":
    role.serve()
