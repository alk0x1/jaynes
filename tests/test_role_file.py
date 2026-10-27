"""Roles written in TOML: the steps in a file, the output models and checks in Python."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

import pytest
from pydantic import BaseModel

from jaynes import cli
from jaynes.role import Witnesses, role, stop
from jaynes.role_file import RoleFileError, load
from jaynes.store import Store
from jaynes.verify import verify

ROOT = Path(__file__).resolve().parents[1]

CHECKS = '''
from pydantic import BaseModel
from jaynes.role import Fail, Pass


class Note(BaseModel):
    text: str


class Plan(BaseModel):
    steps: list[str]


def not_empty(out, needs):
    return Pass() if out.text else Fail("the note is empty")


def follows_the_note(out, needs):
    note = needs["draft"].value.text
    return Pass() if any(note in s for s in out.steps) else Fail(f"no step mentions {note!r}")


def reads_the_plan(out, needs):
    return Pass() if needs["plan"].value.steps else Fail("no plan")


def is_empty(out):
    return not out.text
'''

ROLE = '''
name = "notes"
agent = "quick"
method = "Write short notes."

[[step]]
name = "draft"
output = "notes_checks:Note"
check = ["notes_checks:not_empty"]
stop = { budget = 0.2 }
question = "Ship {out.text!r}?"
instructions = "Write the note."

[[step]]
name = "plan"
output = "notes_checks:Plan"
needs = ["draft"]
check = ["notes_checks:follows_the_note"]
max_turns = 2

[[step]]
name = "accept"
needs = ["plan"]
stop = { budget = 0.05, sampled = false }
'''


def write(folder: Path, role_text: str = ROLE, checks: str = CHECKS) -> Path:
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "notes_checks.py").write_text(checks)
    (folder / "notes.toml").write_text(role_text)
    sys.modules.pop("notes_checks", None)  # each test's folder has its own
    return folder / "notes.toml"


def test_a_role_file_gives_the_same_steps_as_python(tmp_path: Path) -> None:
    r = load(write(tmp_path))
    draft, plan, accept = r.steps

    assert (r.name, r.agent, r.method_text()) == ("notes", "quick", "Write short notes.")
    assert (draft.output.__name__ if draft.output else None, draft.stop, draft.instructions) == (
        "Note", stop(0.2), "Write the note.")
    assert [n.name for n in plan.needs] == ["draft"] and plan.max_turns == 2
    assert accept.output is None and accept.stop == stop(0.05, sampled=False)
    assert draft.output is not None and draft.question is not None
    assert draft.question(draft.output(text="hi"), Witnesses({})) == "Ship 'hi'?"
    assert verify(r) == []


def test_editing_a_question_template_opens_a_new_window_of_labels(tmp_path: Path) -> None:
    before = load(write(tmp_path / "a")).steps[0].stop_fingerprint("m")
    after = load(write(tmp_path / "b", ROLE.replace("Ship {out.text!r}?", "Send {out.text!r}?"))).steps[0]
    assert after.stop_fingerprint("m") != before


@pytest.mark.parametrize("change,message", [
    (("max_turns = 2", "max_turn = 2"), "step plan: unknown key max_turn"),
    (('needs = ["draft"]', 'needs = ["drafts"]'), "step plan needs drafts, which is not an earlier step"),
    (("notes_checks:Plan", "notes_checks:Plans"), "step plan: notes_checks:Plans does not exist"),
    (("notes_checks:Plan", "notes_checks:follows_the_note"), "step plan: output notes_checks:follows_the_note is "
                                                              "not a pydantic model"),
    (("{out.text!r}", "{out.title}"), "step draft: the question names out.title, but Note has no field title"),
    (("{out.text!r}", "{needs}"), "step draft: the question may name only out and its fields"),
    (("budget = 0.2", "budge = 0.2"), "step draft: stop takes budget"),
    (('name = "accept"\n', ''), "every step needs a name"),
])
def test_mistakes_in_a_role_file_are_named_with_the_step(tmp_path: Path, change: tuple[str, str],
                                                         message: str) -> None:
    path = write(tmp_path, ROLE.replace(*change))
    with pytest.raises(RoleFileError) as err:
        load(path)
    assert str(err.value).startswith(f"{path}: ") and message in str(err.value)


def test_a_check_that_reads_a_step_by_name_is_verified(tmp_path: Path) -> None:
    path = write(tmp_path, ROLE.replace('"notes_checks:follows_the_note"',
                                        '"notes_checks:follows_the_note", "notes_checks:reads_the_plan"'))
    assert verify(load(path)) == ["plan: check reads_the_plan reads plan, which plan does not need; add plan to "
                                  "its needs"]

    nowhere = write(tmp_path / "b", ROLE, CHECKS.replace('needs["draft"]', 'needs["drfat"]'))
    assert verify(load(nowhere)) == ["plan: check follows_the_note reads drfat, which is not a step of notes"]


def test_a_string_that_is_not_a_needs_lookup_is_not_a_read() -> None:
    class Note(BaseModel):
        text: str

    def counts(out: Note, needs: Witnesses) -> object:
        return {"draft": 1}["draft"]

    r = role("strings", agent="fake")
    r.step("draft", output=Note, stop=stop(0.2))
    r.step("plan", output=Note, check=counts, stop=stop(0.2))  # type: ignore[arg-type]
    assert verify(r) == []


def jaynes(home: Path, *args: str) -> str:
    env = dict(os.environ, PYTHONPATH=str(ROOT))
    done = subprocess.run([sys.executable, "-m", "jaynes.cli", *args[:1], "--home", str(home), *args[1:]],
                          env=env, capture_output=True, text=True, timeout=60, cwd=ROOT)
    assert done.returncode == 0, done.stderr
    return done.stdout


def test_a_role_file_runs_stops_and_finishes_from_the_cli(tmp_path: Path,
                                                          capsys: pytest.CaptureFixture[str]) -> None:
    path = write(tmp_path / "roles")
    relative = os.path.relpath(path, ROOT)  # as a person types it, from the folder they are in
    home = tmp_path / "home"
    assert cli.main(["check", str(path)]) == 0 and "notes has no problems" in capsys.readouterr().out

    started = json.loads(jaynes(home, "run", "--role", relative, "--task", "t", "--runtime",
                                "quick=tests.role_file_agent:Agent"))
    assert (started["status"], started["step"]) == ("waiting", "draft")
    run = started["run"]
    assert "Ship 'buy milk'?" in jaynes(home, "queue")

    jaynes(home, "answer", run, "draft", "accept")  # continues in the background, from another process
    store = Store(home / "jaynes.sqlite3")
    deadline = time.monotonic() + 30
    while not [e for e in store.events("StopOpened", run_id=run) if e.payload["step"] == "accept"]:
        assert time.monotonic() < deadline, (home / "runs" / run / "continue.log").read_text()
        time.sleep(0.05)
    [first] = store.events("RunStarted", run_id=run)
    assert first.payload["ref"] == str(path.resolve())


def test_a_role_file_that_does_not_load_is_refused_with_its_reason(tmp_path: Path) -> None:
    path = write(tmp_path, ROLE.replace("max_turns = 2", "max_turn = 2"))
    with pytest.raises(SystemExit, match="unknown key max_turn"):
        cli.main(["check", str(path)])
    with pytest.raises(SystemExit, match="no role file"):
        cli.main(["check", str(tmp_path / "missing.toml")])


def test_a_role_file_step_can_end_the_run_by_its_output(tmp_path: Path) -> None:
    draft = load(write(tmp_path, ROLE.replace('instructions = "Write the note."',
                                              'instructions = "Write the note."\nends = "notes_checks:is_empty"'))).steps[0]
    assert draft.ends is not None and draft.output is not None
    assert draft.ends(draft.output(text="")) and not draft.ends(draft.output(text="hi"))
