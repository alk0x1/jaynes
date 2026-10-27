"""Fix a bug: first a test that fails because of it, then a fix that makes it pass with the rest of the suite.

The agent writes tests only, and names the command that runs them; Jaynes runs it in a box and requires that
it fails, printing the failure the agent quotes. The person sees the tests and decides whether they show the
bug. Then the agent changes the project, never its tests, until Jaynes sees those tests and the whole suite
pass. The person gives the final review, and the change comes back as a branch.

The project's suite must pass at its last commit, with its dependencies installed in the person's folder:
the box reaches no internet, only local ports, so nothing is installed during the run. A suite whose own
tests open a macOS box cannot run inside Jaynes's: macOS allows no box inside another.
"""

from __future__ import annotations

from pydantic import BaseModel, Field

from jaynes import project, sandbox, suite
from jaynes.role import Fail, Pass, Witnesses, role, stop

TESTS = ("**/tests/**", "**/test/**", "**/__tests__/**", "**/spec/**", "**/test_*.py", "**/*_test.py",
         "**/*.test.*", "**/*.spec.*", "**/*_test.go", "**/*_spec.rb", "**/*Test.java", "**/*Tests.cs")


class Reproduction(BaseModel):
    bug: str = Field(description="what goes wrong, in one or two sentences")
    tests: list[str] = Field(min_length=1, description="the test files you added or changed, relative to the "
                                                       "project folder")
    command: str = Field(description="a shell command, run from the project folder, that runs only these tests")
    suite: str = Field(description="a shell command, run from the project folder, that runs the whole test suite")
    failure: str = Field(description="a line the command prints because of the bug, copied exactly, such as the "
                                     "assertion message")


class Fix(BaseModel):
    summary: str = Field(description="what was wrong and what changed, for the person reviewing it")
    files: list[str]


def _ran(needs: Witnesses, command: str) -> suite.Ran | Fail:
    try:
        return suite.run(needs, command)
    except sandbox.Unavailable as err:
        return Fail(str(err))


def fails_for_the_bug(out: Reproduction, needs: Witnesses) -> Pass | Fail:
    """The tests are test files, and their command fails, printing the failure quoted."""
    assert needs.project is not None
    reasons = [f"{t} is not a test file; tests go in paths such as tests/ or *_test.*" for t in out.tests
               if project.allowed([t], TESTS)]
    reasons += [f"{t} does not exist in the project" for t in out.tests if not (needs.project / t).is_file()]
    if reasons:
        return Fail(*reasons)
    ran = _ran(needs, out.command)
    if isinstance(ran, Fail):
        return ran
    if ran.code == 0:
        return Fail(f"the tests pass before any fix; they must fail because of the bug. {out.command} printed:\n"
                    + ran.output)
    if out.failure.strip() not in ran.output:
        return Fail(f"{out.failure.strip()} is not in what the command printed; quote a line it prints because of "
                    f"the bug. The end of what it printed (exit {ran.code}):\n{ran.output}")
    return Pass()


def fixed(out: Fix, needs: Witnesses) -> Pass | Fail:
    """The reproduction's tests pass, and then the whole suite does."""
    found: Reproduction = needs[REPRODUCE].value
    for what, command in (("the tests still fail", found.command), ("the suite fails", found.suite)):
        ran = _ran(needs, command)
        if isinstance(ran, Fail):
            return ran
        if ran.code != 0:
            return Fail(f"{what}: {command} exited with {ran.code}. The end of what it printed:\n{ran.output}")
    return Pass()


ROLE = role("fix-bug")
REPRODUCE = ROLE.step(
    "reproduce", output=Reproduction, project="write", may_change=TESTS, check=fails_for_the_bug, stop=stop(0.2),
    question=lambda out, _: f"Do these tests show the bug? {out.bug}",
    instructions="Find the bug the task describes and write the smallest test that fails because of it, next to "
                 "the project's other tests and in their style. Change test files only; do not fix the bug yet. "
                 "Give the command that runs only your tests and the command that runs the whole suite, as the "
                 "project runs them (its README, Makefile, package.json or pyproject.toml say how). Jaynes runs "
                 "your command from the project folder in a box without internet, with the dependencies already "
                 "installed, and requires it to fail, printing the line you quote. If the person returned an "
                 "earlier attempt, follow their note.")
FIX = ROLE.step(
    "fix", output=Fix, needs=[REPRODUCE], project="write", may_change=("**", *(f"!{t}" for t in TESTS)),
    check=fixed, deadline=3600.0,
    instructions="Fix the bug so the tests in inputs/reproduce.json pass, and the whole suite with them. Change "
                 "the project's code, never its tests: they are how the person knows the bug is gone. Keep the "
                 "change as small as the fix needs. Jaynes runs both commands in a box without internet.")
ROLE.step("accept", needs=[FIX], stop=stop(0.05, sampled=False),
          question=lambda _, needs: f"Merge this fix? {needs[FIX].value.summary}")
