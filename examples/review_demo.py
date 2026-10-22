"""A screen-design shaped role with a scripted agent, to see the review page without spending quota.

    PYTHONPATH=. uv run jaynes run --role examples.review_demo:ROLE --task "Redesign the People screen" \\
        --runtime scripted=examples.review_demo:Agent
    PYTHONPATH=. uv run jaynes review
"""

import json
from pathlib import Path

from pydantic import BaseModel

from jaynes.role import Fail, Pass, StepContext, Witnesses, role, stop
from jaynes.runtime import TurnResult


class Snapshot(BaseModel):
    screen: str
    people: int


class Break(BaseModel):
    id: str
    problem: str
    capture: str


class Breaks(BaseModel):
    breaks: list[Break]


class Requirement(BaseModel):
    id: str
    requirement: str
    fixes: str
    evidence: str


class Requirements(BaseModel):
    requirements: list[Requirement]


class Patch(BaseModel):
    files: list[str]
    summary: str


def take_snapshot(ctx: StepContext) -> Snapshot:
    return Snapshot(screen=ctx.task, people=128)


def breaks_cite_captures(out: Breaks, needs: Witnesses) -> Pass | Fail:
    bad = [b.id for b in out.breaks if not b.capture.startswith("capture")]
    return Fail(*(f"{b} cites no capture" for b in bad)) if bad else Pass()


def one_requirement_per_break(out: Requirements, needs: Witnesses) -> Pass | Fail:
    known = {b.id for b in needs[BREAKS].value.breaks}
    unknown = [f"{r.id} cites break {r.fixes}, which does not exist" for r in out.requirements if r.fixes not in known]
    missing = known - {r.fixes for r in out.requirements}
    return Fail(*unknown, *(f"break {b} has no requirement" for b in sorted(missing))) if unknown or missing else Pass()


def ask(out: Requirements, needs: Witnesses) -> str:
    return (f"Each of these {len(out.requirements)} requirements fixes one break from the walkthrough. "
            "Are they the right fixes?")


ROLE = role("screen-design-demo", agent="scripted")
SNAPSHOT = ROLE.step("snapshot", output=Snapshot, run_by=take_snapshot)
BREAKS = ROLE.step("breaks", output=Breaks, needs=[SNAPSHOT], check=breaks_cite_captures)
REQUIREMENTS = ROLE.step("requirements", output=Requirements, needs=[BREAKS], check=one_requirement_per_break,
                         stop=stop(0.10), question=ask)
IMPLEMENT = ROLE.step("implement", output=Patch, needs=[REQUIREMENTS])
ACCEPT = ROLE.step("accept", needs=[IMPLEMENT], stop=stop(0.05, sampled=False))

PROBLEMS = [
    ("B1", "Search loses the typed query after opening a person and going back.", "capture 03",
     "Search keeps the typed query after opening a person and going back."),
    ("B2", "The empty list gives no reason and no way forward.", "capture 05",
     "The empty list says why it is empty and offers Add person."),
    ("B3", "The role filter hides how many people each role has.", "capture 06",
     "The role filter shows the count per role before it is applied."),
    ("B4", "Bulk selection is lost on the next page.", "capture 09",
     "Bulk selection survives moving to the next page."),
    ("B5", "Secondary text fails AA contrast.", "capture 02",
     "Secondary text meets AA contrast, 4.5 to 1."),
]


class Agent:
    """Writes plausible outputs; its first requirements attempt cites a break that does not exist."""

    name = "scripted"

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        info = json.loads((workspace / "step.json").read_text())
        step, attempt = info["step"], info["attempt"]
        if step == "breaks":
            out = {"breaks": [{"id": i, "problem": p, "capture": c} for i, p, c, _ in PROBLEMS]}
        elif step == "requirements":
            fixes = ["B1", "B2", "B3", "B7" if attempt == 1 else "B4", "B5"]
            out = {"requirements": [{"id": f"R{n + 1}", "requirement": r, "fixes": fixes[n], "evidence": c}
                                    for n, (_, _, c, r) in enumerate(PROBLEMS)]}
        else:
            out = {"files": ["src/people/list.tsx", "src/people/filter.tsx"],
                   "summary": "Keeps search state, adds the empty state and per-role counts."}
        (workspace / "output.json").write_text(json.dumps(out))
        return TurnResult(30_500, 1_550, "scripted-1")
