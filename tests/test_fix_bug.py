"""The built-in fix-bug role: a failing test first, then a fix that makes it and the suite pass, in a box."""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest

from jaynes import sandbox, suite
from jaynes.conductor import Conductor
from jaynes.role import Checked, Fail, Pass, Witnesses
from jaynes.roles import fix_bug as fb
from jaynes.verify import verify
from tests.fakes import Action, Scripted, write

pytestmark = pytest.mark.skipif(not sandbox.available(), reason="project commands run only in a box")

PYTHON = f"{sys.executable} -m pytest -q -p no:cacheprovider"
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
    base: dict[str, Any] = {"bug": "add subtracts", "tests": ["tests/test_add.py"], "command": f"{PYTHON} tests/test_add.py",
            "suite": PYTHON, "failure": "AssertionError: add subtracts"}
    return fb.Reproduction(**{**base, **overrides})


def test_a_reproduction_must_fail_for_the_reason_it_quotes(repo: Path, tmp_path: Path) -> None:
    needs = Witnesses({}, copy(repo, tmp_path, {"tests/test_add.py": TEST}), source=repo)
    assert isinstance(fb.fails_for_the_bug(reproduction(), needs), Pass)

    quoted = fb.fails_for_the_bug(reproduction(failure="ZeroDivisionError"), needs)
    assert isinstance(quoted, Fail) and "ZeroDivisionError is not in what the command printed" in quoted.reasons[0]
    assert "add subtracts" in quoted.reasons[0]  # the end of the output, so the agent sees the real failure

    passing = fb.fails_for_the_bug(reproduction(command=f"{PYTHON} tests/test_zero.py", tests=["tests/test_zero.py"]),
                                   needs)
    assert isinstance(passing, Fail) and "the tests pass before any fix" in passing.reasons[0]

    elsewhere = fb.fails_for_the_bug(reproduction(tests=["calc.py"]), needs)
    assert isinstance(elsewhere, Fail) and "calc.py is not a test file" in elsewhere.reasons[0]


def test_the_quoted_failure_counts_wherever_the_command_printed_it(repo: Path, tmp_path: Path) -> None:
    needs = Witnesses({}, copy(repo, tmp_path, {}), source=repo)
    long = "echo 'AssertionError: add subtracts'; head -c 20000 /dev/zero | tr '\\0' x; echo; false"
    assert isinstance(fb.fails_for_the_bug(reproduction(tests=["tests/test_zero.py"], command=long), needs), Pass)


def test_a_fix_must_pass_the_new_tests_and_the_whole_suite(repo: Path, tmp_path: Path) -> None:
    found = Checked("reproduce", reproduction(), "h", "c")
    broken = Witnesses({"reproduce": found}, copy(repo, tmp_path, {"tests/test_add.py": TEST}), source=repo)
    failed = fb.fixed(fb.Fix(summary="s", files=["calc.py"]), broken)
    assert isinstance(failed, Fail) and "the tests still fail" in failed.reasons[0]

    shutil.rmtree(tmp_path / "copy")
    good = copy(repo, tmp_path, {"tests/test_add.py": TEST, "calc.py": "def add(a, b):\n    return a + b\n"})
    assert isinstance(fb.fixed(fb.Fix(summary="s", files=["calc.py"]), Witnesses({"reproduce": found}, good,
                                                                                   source=repo)), Pass)


def test_project_commands_never_write_into_the_steps_copy(repo: Path, tmp_path: Path) -> None:
    c = copy(repo, tmp_path, {"tests/test_add.py": TEST})
    before = sorted(p.relative_to(c).as_posix() for p in c.rglob("*"))
    fb.fails_for_the_bug(reproduction(command=f"{sys.executable} -m pytest -q tests/test_add.py"), Witnesses({}, c,
                                                                                                         source=repo))
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
    assert isinstance(fb.fails_for_the_bug(reproduction(command=""), Witnesses({}, repo, source=repo)), Fail)
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
