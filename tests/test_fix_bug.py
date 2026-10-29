"""The built-in fix-bug role: a failing test first, then a fix that makes it and the suite pass, in a box."""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest

from jaynes import runners, sandbox, suite
from jaynes.conductor import Conductor
from jaynes.role import Checked, Fail, Pass, Witnesses
from jaynes.roles import fix_bug as fb
from jaynes.runners import Runner
from jaynes.verify import verify
from tests.fakes import Action, Scripted, write

pytestmark = pytest.mark.skipif(not sandbox.available(), reason="project commands run only in a box")

PYTHON = f"{sys.executable} -m pytest -q -p no:cacheprovider"
PYTEST = Runner(kind="pytest", suite=PYTHON, each=PYTHON, found="the test")


@pytest.fixture(autouse=True)
def python(monkeypatch: pytest.MonkeyPatch) -> None:
    """The fixture projects have no virtualenv: their tests run with this interpreter, which has pytest."""
    monkeypatch.setattr(runners, "PYTHON", sys.executable)


def ran_by(runner: Runner = PYTEST, **more: Checked[Any]) -> dict[str, Checked[Any]]:
    return {"runner": Checked("runner", runner, "h", "c"), **more}
TEST = "from calc import add\n\n\ndef test_add_adds():\n    assert add(2, 2) == 4, 'add subtracts'\n"


def git(cwd: Path, *args: str) -> str:
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    r = tmp_path / "repo"
    (r / "tests").mkdir(parents=True)
    (r / "calc.py").write_text("def add(a, b):\n    return a - b\n")
    (r / "tests" / "test_zero.py").write_text("from calc import add\n\n\ndef test_zero():\n    assert add(0, 0) == 0\n")
    git(r, "init", "-q")
    git(r, "add", "-A")
    git(r, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "start")
    return r


def copy(repo: Path, tmp_path: Path, files: dict[str, str]) -> Path:
    c = tmp_path / "copy"
    shutil.copytree(repo, c, ignore=shutil.ignore_patterns(".git"))
    for name, text in files.items():
        (c / name).write_text(text)
    return c


def reproduction(**overrides: Any) -> fb.Reproduction:
    base: dict[str, Any] = {"bug": "add subtracts", "tests": ["tests/test_add.py"],
                            "failure": "AssertionError: add subtracts"}
    return fb.Reproduction(**{**base, **overrides})


def test_a_reproduction_must_fail_for_the_reason_it_quotes(repo: Path, tmp_path: Path) -> None:
    needs = Witnesses(ran_by(), copy(repo, tmp_path, {"tests/test_add.py": TEST}), source=repo)
    assert isinstance(fb.fails_for_the_bug(reproduction(), needs), Pass)

    quoted = fb.fails_for_the_bug(reproduction(failure="ZeroDivisionError"), needs)
    assert isinstance(quoted, Fail) and "ZeroDivisionError is not in what the command printed" in quoted.reasons[0]
    assert "add subtracts" in quoted.reasons[0]  # the end of the output, so the agent sees the real failure

    passing = fb.fails_for_the_bug(reproduction(tests=["tests/test_zero.py"]), needs)
    assert isinstance(passing, Fail) and "the tests pass before any fix" in passing.reasons[0]

    elsewhere = fb.fails_for_the_bug(reproduction(tests=["calc.py"]), needs)
    assert isinstance(elsewhere, Fail) and "calc.py is not a test file" in elsewhere.reasons[0]


def test_the_quoted_failure_counts_wherever_the_command_printed_it(repo: Path, tmp_path: Path) -> None:
    long = "echo 'AssertionError: add subtracts'; head -c 20000 /dev/zero | tr '\\0' x; echo; false"
    needs = Witnesses(ran_by(Runner(kind="given", suite=long, each="", found="t")), copy(repo, tmp_path, {}),
                      source=repo)
    assert isinstance(fb.fails_for_the_bug(reproduction(tests=["tests/test_zero.py"]), needs), Pass)


def test_a_fix_must_pass_the_new_tests_and_the_whole_suite(repo: Path, tmp_path: Path) -> None:
    found = Checked("reproduce", reproduction(), "h", "c")
    broken = Witnesses(ran_by(reproduce=found), copy(repo, tmp_path, {"tests/test_add.py": TEST}), source=repo)
    failed = fb.fixed(fb.Fix(summary="s", files=["calc.py"]), broken)
    assert isinstance(failed, Fail) and "the tests still fail" in failed.reasons[0]

    shutil.rmtree(tmp_path / "copy")
    good = copy(repo, tmp_path, {"tests/test_add.py": TEST, "calc.py": "def add(a, b):\n    return a + b\n"})
    assert isinstance(fb.fixed(fb.Fix(summary="s", files=["calc.py"]), Witnesses(ran_by(reproduce=found), good,
                                                                                   source=repo)), Pass)


def test_project_commands_never_write_into_the_steps_copy(repo: Path, tmp_path: Path) -> None:
    c = copy(repo, tmp_path, {"tests/test_add.py": TEST})
    before = sorted(p.relative_to(c).as_posix() for p in c.rglob("*"))
    fb.fails_for_the_bug(reproduction(), Witnesses(ran_by(), c, source=repo))
    assert sorted(p.relative_to(c).as_posix() for p in c.rglob("*")) == before  # no caches, no __pycache__


def edit(files: dict[str, str], value: Any) -> Action:
    def act(ws: Path) -> None:
        for path, text in files.items():
            (ws / "project" / path).write_text(text)
        write(value)(ws)
    return act


def test_the_role_finds_the_bug_with_a_test_and_then_fixes_it_without_touching_tests(repo: Path,
                                                                                     tmp_path: Path) -> None:
    assert verify(fb.ROLE) == []
    found = reproduction().model_dump()
    agent = Scripted(name="claude", plan={
        ("reproduce", 1): edit({"tests/test_add.py": TEST}, found),
        ("fix", 1): edit({"tests/test_add.py": "def test_add_adds():\n    pass\n"}, {"summary": "s", "files": []}),
        ("fix", 2): edit({"calc.py": "def add(a, b):\n    return a + b\n"}, {"summary": "adds", "files": ["calc.py"]}),
    })
    c = Conductor(tmp_path / "home", runtimes={"claude": lambda: agent})
    first = c.run(fb.ROLE, "add(2, 2) gives 0", run_id="r1", project=repo)
    assert (first.status, first.step) == ("waiting", "reproduce")

    c.answer(fb.ROLE, "r1", "reproduce", "accept", by="person")
    last = c.run(fb.ROLE, "add(2, 2) gives 0", run_id="r1")
    assert (last.status, last.step) == ("waiting", "accept")
    [rejected] = [e.payload for e in c.store.events("TurnRejected", run_id="r1")]
    assert rejected["step"] == "fix" and "tests/test_add.py" in rejected["reasons"][0]

    c.answer(fb.ROLE, "r1", "accept", "accept", by="person")
    assert c.run(fb.ROLE, "add(2, 2) gives 0", run_id="r1").status == "finished"
    branch = git(repo, "branch", "--list", "jaynes/*").strip(" *")
    assert git(repo, "show", f"{branch}:calc.py").endswith("a + b")
    assert git(repo, "show", f"{branch}:tests/test_add.py") == TEST.strip()


def test_project_commands_commit_with_git_even_when_the_person_signs_commits_with_a_hidden_key(
        repo: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    config = tmp_path / "gitconfig"
    config.write_text("[commit]\n\tgpgsign = true\n[gpg]\n\tformat = ssh\n[user]\n\tsigningkey = /nowhere/key.pub\n")
    monkeypatch.setenv("GIT_CONFIG_GLOBAL", str(config))
    ran = suite.run(Witnesses({}, copy(repo, tmp_path, {}), source=repo),
                    'cd "$TMPDIR" && git init -q g && cd g && git -c user.name=t -c user.email=t@t commit -q '
                    '--allow-empty -m x && echo committed')
    assert (ran.code, ran.output.strip()) == (0, "committed")


def test_when_no_test_shows_the_bug_a_person_decides_and_accepting_ends_the_run_with_nothing_changed(
        repo: Path, tmp_path: Path) -> None:
    assert isinstance(fb.fails_for_the_bug(reproduction(tests=[]), Witnesses(ran_by(), repo, source=repo)), Fail)
    nothing = {"bug": "add adds; the report used the wrong numbers", "reproduced": False}
    agent = Scripted(name="claude", plan={("reproduce", 1): write(nothing)})
    c = Conductor(tmp_path / "home", runtimes={"claude": lambda: agent})
    assert c.run(fb.ROLE, "add(2, 2) gives 0", run_id="r1", project=repo).step == "reproduce"
    [opened] = [e.payload for e in c.store.events("StopOpened", run_id="r1")]
    assert opened["status"] == "floor" and opened["question"].startswith("No test shows the bug: add adds")

    c.answer(fb.ROLE, "r1", "reproduce", "accept", by="person")
    assert c.run(fb.ROLE, "add(2, 2) gives 0", run_id="r1").status == "finished"
    assert [e.payload for e in c.store.events("RunFinished", run_id="r1")] == [{"outcome": "ended",
                                                                                "step": "reproduce"}]
    assert git(repo, "branch", "--list", "jaynes/*") == ""


def test_with_no_runner_the_run_blocks_at_once_saying_how_to_give_one_and_a_given_suite_runs(tmp_path: Path) -> None:
    bare = tmp_path / "bare"
    bare.mkdir()
    (bare / "add.sh").write_text("echo $(($1 - $2))\n")
    git(bare, "init", "-q")
    git(bare, "add", "-A")
    git(bare, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "start")
    c = Conductor(tmp_path / "home", runtimes={"claude": lambda: Scripted(name="claude")})
    blocked = c.run(fb.ROLE, "add subtracts", run_id="r1", project=bare)
    assert (blocked.status, blocked.step) == ("blocked", "runner") and "--suite" in blocked.reasons[0]
    assert len(list(c.store.events("StepStarted", run_id="r1"))) == 1  # the same files would give the same answer

    given = c.run(fb.ROLE, "add subtracts", run_id="r2", project=bare, started={"suite": "sh -c 'exit 0'"})
    assert given.step == "reproduce"  # the scripted agent has no plan: the runner step passed
    assert c.output("r2", fb.RUNNER) == Runner(kind="given", suite="sh -c 'exit 0'", each="", found="given at start")


def test_the_agent_cannot_narrow_the_suite_a_fix_that_breaks_another_test_is_rejected(repo: Path,
                                                                                       tmp_path: Path) -> None:
    found = {**reproduction().model_dump(), "suite": f"{PYTHON} tests/test_add.py"}  # ignored: not in the schema
    breaks = "def add(a, b):\n    return 4 if (a, b) == (2, 2) else 1\n"  # test_add passes, test_zero fails
    agent = Scripted(name="claude", plan={
        ("reproduce", 1): edit({"tests/test_add.py": TEST}, found),
        ("fix", 1): edit({"calc.py": breaks}, {"summary": "s", "files": ["calc.py"]}),
        ("fix", 2): edit({"calc.py": "def add(a, b):\n    return a + b\n"}, {"summary": "adds", "files": ["calc.py"]}),
    })
    c = Conductor(tmp_path / "home", runtimes={"claude": lambda: agent})
    c.run(fb.ROLE, "add(2, 2) gives 0", run_id="r1", project=repo)
    c.answer(fb.ROLE, "r1", "reproduce", "accept", by="person")
    assert c.run(fb.ROLE, "add(2, 2) gives 0", run_id="r1").step == "accept"
    [rejected] = [e.payload for e in c.store.events("TurnRejected", run_id="r1")]
    assert rejected["step"] == "fix" and rejected["reasons"][0].startswith(f"the suite fails: {PYTHON} exited")
