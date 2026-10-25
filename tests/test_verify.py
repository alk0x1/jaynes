"""The verifier: what a role gets wrong is found before any agent runs."""

from __future__ import annotations

import functools
import importlib
import subprocess
import sys
from pathlib import Path
from types import CellType, FunctionType
from typing import Any

import pytest
from pydantic import BaseModel

from jaynes import cli
from jaynes.role import Fail, Pass, Witnesses, role, stop
from jaynes.roles import BUILT_IN
from jaynes.store import Store
from jaynes.verify import verify


class Note(BaseModel):
    text: str


class Plan(BaseModel):
    steps: list[str]


class LongNote(Note):
    pages: int = 1


OTHER = role("other", agent="fake")
ELSEWHERE = OTHER.step("draft", output=Note)

BAD = role("bad", agent="fake")
DRAFT = BAD.step("draft", output=Note)
PLAN = BAD.step("plan", output=Plan)


def mentions_draft(out: Plan, needs: Witnesses) -> Pass | Fail:
    return Pass() if needs[DRAFT].value.text in " ".join(out.steps) else Fail("the plan forgets the draft")


def via_helper(out: Plan, needs: Witnesses) -> Pass | Fail:
    return mentions_draft(out, needs)


def reads_the_other_role(out: Plan, needs: Witnesses) -> Pass | Fail:
    return Pass() if needs[ELSEWHERE].value.text else Fail("empty")


def for_notes(out: Note, needs: Witnesses) -> Pass | Fail:
    return Pass() if out.text else Fail("empty")


# reads the draft without needing it, through a helper and from another role; is checked as a note; and the
# plan's output goes nowhere: no later step needs it and no stop shows it
BAD.step("publish", output=Plan, check=[mentions_draft, via_helper, reads_the_other_role, for_notes],
         question=lambda out, _: f"Publish {len(out.steps)} steps?", stop=stop(0.2))


def test_a_role_without_problems_has_none() -> None:
    r = role("good", agent="fake")
    draft = r.step("draft", output=LongNote)
    r.step("plan", output=Plan, needs=[draft], check=[mentions_draft_of(draft), for_plans], stop=stop(0.2),
           question=lambda out, needs: f"Plan for {needs[draft].value.text}?")
    assert verify(r) == []


def mentions_draft_of(draft: object):  # a check built in a closure names the step through a cell
    def check(out: Plan, needs: Witnesses) -> Pass | Fail:
        return Pass() if needs[draft].value.text else Fail("empty")  # type: ignore[index]
    return check


def for_plans(out: Plan, needs: Witnesses) -> Pass | Fail:
    return Pass() if out.steps else Fail("empty")


def test_every_problem_is_named_with_its_step_and_how_to_fix_it() -> None:
    problems = verify(BAD)

    assert ("publish: check mentions_draft reads draft, which publish does not need; add draft to its needs"
            in problems)
    assert "publish: check via_helper reads draft, which publish does not need; add draft to its needs" in problems
    assert ("publish: check reads_the_other_role reads draft of role other; use a step of bad, in publish's needs"
            in problems)
    assert "publish: check for_notes takes a Note, but publish's output is a Plan" in problems
    assert ("plan: its output reaches no later step and no stop; need it from a later step, stop at it or remove it"
            in problems)
    assert len(problems) == 6  # and draft reaches nothing either: publish reads it without needing it


def test_a_question_that_reads_a_step_it_does_not_need_is_a_problem() -> None:
    r = role("asks", agent="fake")
    draft = r.step("draft", output=Note)
    r.step("plan", output=Plan, needs=[draft], stop=stop(0.2))
    r.step("ship", output=Plan, stop=stop(0.2), question=lambda out, needs: f"Ship {needs[draft].value.text}?")

    assert verify(r) == ["ship: question reads draft, which ship does not need; add draft to its needs"]


def test_steps_that_change_the_project_or_act_are_used_without_a_stop() -> None:
    r = role("quiet", agent="fake")
    r.step("tidy", output=Note, project="write", may_change=["**"])
    r.step("report", output=Note, stop=stop(0.2))  # tidy is not last, and nothing needs it
    assert verify(r) == []


@pytest.mark.parametrize("ref", [*sorted(BUILT_IN.values()), "jaynes.demo:ROLE"])
def test_the_built_in_roles_and_the_demo_have_no_problems(ref: str) -> None:
    assert verify(cli.load_ref(ref)) == []


def test_jaynes_check_lists_the_problems_and_fails(capsys: pytest.CaptureFixture[str]) -> None:
    with pytest.raises(SystemExit) as exited:
        cli.main(["check", "tests.test_verify:BAD"])
    assert exited.value.code == 1
    out = capsys.readouterr().out
    assert "bad has 6 problems" in out and "publish: check for_notes takes a Note" in out

    assert cli.main(["check", "visual-identity"]) == 0
    assert "visual-identity has no problems" in capsys.readouterr().out


def test_a_run_of_a_role_with_problems_never_starts(tmp_path: Path) -> None:
    done = subprocess.run([sys.executable, "-m", "jaynes.cli", "run", "--home", str(tmp_path), "--role",
                           "tests.test_verify:BAD", "--task", "go"], capture_output=True, text=True,
                          cwd=Path(__file__).resolve().parents[1])
    assert done.returncode == 1 and "bad has 6 problems" in done.stderr
    assert list(Store(tmp_path / "jaynes.sqlite3").events("RunStarted")) == []


class Holder:
    draft = DRAFT


TABLE = {"draft": DRAFT}


def _reads_draft(out: Plan, needs: Witnesses, step: object = None) -> Pass | Fail:
    return Pass() if needs[step].value.text else Fail("empty")  # type: ignore[index]


class Callable_:
    def __call__(self, out: Plan, needs: Witnesses) -> Pass | Fail:
        return Pass() if needs[DRAFT].value.text else Fail("empty")

    def method(self, out: Plan, needs: Witnesses) -> Pass | Fail:
        return Pass() if needs[DRAFT].value.text else Fail("empty")


def by_default(out: Plan, needs: Witnesses, step: object = DRAFT) -> Pass | Fail:
    return Pass() if needs[step].value.text else Fail("empty")  # type: ignore[index]


def from_table(out: Plan, needs: Witnesses) -> Pass | Fail:
    return Pass() if needs[TABLE["draft"]].value.text else Fail("empty")


def from_class(out: Plan, needs: Witnesses) -> Pass | Fail:
    return Pass() if needs[Holder.draft].value.text else Fail("empty")


@pytest.mark.parametrize("check", [functools.partial(_reads_draft, step=DRAFT), Callable_(), Callable_().method,
                                   by_default, from_table, from_class],
                         ids=["partial", "callable", "method", "default", "dict", "class"])
def test_a_step_reached_through_a_partial_an_object_a_default_or_a_container_is_read(check: Any) -> None:
    r = role("hidden", agent="fake")
    r.step("plan", output=Plan, check=check, stop=stop(0.2))
    assert [p.split(": ", 1)[1].split(" reads ")[1] for p in verify(r)] == ["draft of role bad; use a step of hidden, "
                                                                           "in plan's needs"]


def test_naming_a_step_only_for_its_name_is_not_a_read() -> None:
    r = role("labels", agent="fake")
    draft = r.step("draft", output=Note, stop=stop(0.2))
    r.step("plan", output=Plan, stop=stop(0.2), question=lambda out, _: f"Plan after {draft.name}?")
    assert verify(r) == []


def test_a_trusted_executor_may_only_read_what_its_step_needs() -> None:
    r = role("exec", agent="fake")
    draft = r.step("draft", output=Note, stop=stop(0.2))
    r.step("plan", output=Plan, run_by=lambda ctx: Plan(steps=[ctx.needs[draft].value.text]), stop=stop(0.2))
    assert verify(r) == ["plan: executor reads draft, which plan does not need; add draft to its needs"]


def test_a_check_for_one_of_several_output_types_fits() -> None:
    def either(out: Note | Plan, needs: Witnesses) -> Pass | Fail:
        return Pass()

    r = role("union", agent="fake")
    r.step("plan", output=Plan, check=either, stop=stop(0.2))
    assert verify(r) == []


def test_code_the_verifier_cannot_read_is_skipped_never_a_crash() -> None:
    def make(v: int) -> Any:
        def check(out: Plan, needs: Witnesses) -> Pass | Fail:
            return Pass() if v else Fail("x")
        return check

    template = make(1)
    empty = FunctionType(template.__code__, template.__globals__, "empty", None, (CellType(),))  # never assigned
    nameless = FunctionType(compile("lambda out, needs: needs", "<x>", "eval").co_consts[0], {})
    r = role("odd", agent="fake")
    r.step("plan", output=Plan, check=[empty, nameless], stop=stop(0.2))
    assert verify(r) == []


def test_a_project_module_named_like_the_standard_library_is_still_read(tmp_path: Path,
                                                                         monkeypatch: pytest.MonkeyPatch) -> None:
    pkg = tmp_path / "calendar"
    pkg.mkdir()
    (pkg / "__init__.py").write_text(
        "from tests.test_verify import DRAFT\n"
        "def reads(out, needs):\n    return needs[DRAFT]\n")
    monkeypatch.setattr(sys, "path", [str(tmp_path), *sys.path])
    monkeypatch.delitem(sys.modules, "calendar", raising=False)
    shadow = importlib.import_module("calendar")
    r = role("cal", agent="fake")
    r.step("plan", output=Plan, check=shadow.reads, stop=stop(0.2))
    assert verify(r) == ["plan: check reads reads draft of role bad; use a step of cal, in plan's needs"]
