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
from jaynes.role import Blocked, Checked, Fail, Pass, Witnesses
from software import fix_bug as fb
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
# the hidden case, written before the work: it runs in the box against the fixed project
CASES = {"cases": [{"says": "Adding 2 and 2 gives 4",
                    "run": f"{sys.executable} -c 'from calc import add; assert add(2, 2) == 4'"}], "untestable": []}


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
        (c / name).parent.mkdir(parents=True, exist_ok=True)
        (c / name).write_text(text)
    return c


def reproduction(**overrides: Any) -> fb.Reproduction:
    base: dict[str, Any] = {"bug": "add subtracts", "tests": ["tests/test_add.py"],
                            "failure": "AssertionError: add subtracts",
                            "examples": [{"case": "Adding 2 and 2", "now": "gives 0", "after": "gives 4"}]}
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


def test_a_reproduction_says_the_bug_in_short_plain_examples(repo: Path, tmp_path: Path) -> None:
    fixed_copy = copy(repo, tmp_path, {"tests/test_add.py": TEST})
    needs = Witnesses(ran_by(), fixed_copy, source=repo)
    none = fb.fails_for_the_bug(reproduction(examples=[]), needs)
    assert isinstance(none, Fail) and "examples" in none.reasons[0]
    long = fb.fails_for_the_bug(reproduction(examples=[{"case": "x" * 51, "now": "gives 0", "after": "gives 4"}]), needs)
    assert isinstance(long, Fail) and "the case of example 1" in long.reasons[0]
    assert isinstance(fb.fails_for_the_bug(reproduction(), needs), Pass)


def test_the_stop_opens_on_the_examples_and_the_failure_with_the_code_behind_details() -> None:
    needs = Witnesses(ran_by())
    shown = fb.show_bug(reproduction(), needs)
    assert [(x.title, x.text, x.more) for x in shown] == [
        ("Adding 2 and 2", "Now: gives 0. After the fix: gives 4.", False),
        ("What the test printed", "AssertionError: add subtracts", False),
        ("What goes wrong", "add subtracts.", True),
        ("The tests", "tests/test_add.py.", True)]
    assert fb.show_bug(reproduction(examples=[]), needs)[0].title == "What the test printed"  # a run from before
    assert fb.show_bug(fb.Reproduction(bug="nothing wrong", reproduced=False), needs) == []
    assert fb.ask_about_bug(reproduction(), needs) == "Do these tests show the bug?"


def test_the_merge_says_what_changed_and_that_the_tests_pass() -> None:
    found = Checked("reproduce", reproduction(), "h", "c")
    made = fb.Fix(summary="add adds now", files=["project/calc.py"])
    shown = fb.show_fixed(None, Witnesses(ran_by(reproduce=found, fix=Checked("fix", made, "h", "c"))))
    assert [(x.title, x.text, x.more) for x in shown] == [
        ("What changed", "add adds now.", False),
        ("The bug's tests pass", "Jaynes ran them and the whole suite itself, in a box without internet.", False),
        ("Adding 2 and 2", "Now: gives 0. After the fix: gives 4.", True),
        ("Files changed", "calc.py.", True)]
    old = fb.Fix(summary="word " * 200, files=[])
    [changed, *_] = fb.show_fixed(None, Witnesses(ran_by(reproduce=found, fix=Checked("fix", old, "h", "c"))))
    assert len(changed.text) <= fb.SUMMARY + 1 and changed.text.endswith("…")


def test_an_agent_that_a_test_stops_sends_it_back_and_the_person_chooses_only_if_they_still_disagree(
        repo: Path, tmp_path: Path) -> None:
    say = "tests/test_zero.py expects 0 for add(0, 0), which the task changes"
    agent = Scripted(name="claude", plan={
        ("cases", 1): write(CASES),
        **{("reproduce", n): edit({"tests/test_add.py": TEST}, reproduction().model_dump()) for n in (1, 2, 3)},
        **{("fix", n): write({"summary": "s", "files": [], "wrong_test": say}) for n in (1, 2, 3)},
    })
    c = Conductor(tmp_path / "home", runtimes={"claude": lambda: agent})
    c.run(fb.ROLE, "add(2, 2) gives 0", run_id="r1", project=repo)
    c.answer(fb.ROLE, "r1", "reproduce", "accept", by="person")
    seen = [c.run(fb.ROLE, "add(2, 2) gives 0", run_id="r1")]
    while seen[-1].status == "waiting" and len(seen) < 6:  # the reproduction's stop, opened again on each round
        c.answer(fb.ROLE, "r1", "reproduce", "accept", by="person")
        seen.append(c.run(fb.ROLE, "add(2, 2) gives 0", run_id="r1"))

    blocked = seen[-1]
    assert (blocked.status, blocked.step) == ("blocked", "fix")
    assert blocked.reasons[0].startswith("Two agents disagree about a test")
    assert all(say in p for p in agent.prompts if "# Step: reproduce\n" in p and p is not agent.prompts[1])
    assert len(list(c.store.events("StepReturned", run_id="r1"))) == 2
    [waiting] = [w for w in c.queue() if w.kind == "blocked"]
    assert waiting.waits  # a question for the person, not a failure to look into later
    assert [e.payload for e in c.store.events("TurnRejected", run_id="r1")] == []  # no attempt spent on it


def test_a_fix_with_a_long_summary_is_refused(repo: Path, tmp_path: Path) -> None:
    needs = Witnesses(ran_by(reproduce=Checked("reproduce", reproduction(), "h", "c")), repo, source=repo)
    refused = fb.fixed(fb.Fix(summary="x" * 401, files=[]), needs)
    assert isinstance(refused, Fail) and "at most 400" in refused.reasons[0]


def test_the_role_finds_the_bug_with_a_test_and_then_fixes_it_without_touching_tests(repo: Path,
                                                                                     tmp_path: Path) -> None:
    assert verify(fb.ROLE) == []
    found = reproduction().model_dump()
    agent = Scripted(name="claude", plan={
        ("cases", 1): write(CASES),
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


def test_a_failure_the_box_caused_is_not_the_bug(repo: Path, tmp_path: Path) -> None:
    outside = tmp_path / "outside"
    blocked = f"touch {outside}; echo FAIL; exit 1; :"
    needs = Witnesses(ran_by(Runner(kind="given", suite=blocked, each=blocked, found="the test")),
                      copy(repo, tmp_path, {"tests/test_add.py": TEST}), source=repo)
    with pytest.raises(Blocked) as refused:  # the run blocks at once: no attempt of the agent is spent on it
        fb.fails_for_the_bug(reproduction(failure="FAIL"), needs)
    assert "the box refused" in str(refused.value) and str(outside) in str(refused.value)


def test_a_bare_status_word_is_not_a_quote_of_the_failure(repo: Path, tmp_path: Path) -> None:
    needs = Witnesses(ran_by(), copy(repo, tmp_path, {"tests/test_add.py": TEST}), source=repo)
    for word in ("FAIL", "failed", "Error", "1 failed"):
        bare = fb.fails_for_the_bug(reproduction(failure=word), needs)
        assert isinstance(bare, Fail) and "names no reason" in bare.reasons[0], word


GO_TEST = 'package calc\n\nimport "testing"\n\nfunc TestDouble(t *testing.T) {\n\tif Double(2) != 4 {\n\t\tt.Fatal("no")\n\t}\n}\n'


@pytest.mark.skipif(shutil.which("go") is None, reason="needs Go")
def test_go_tests_run_in_the_box_with_their_build_cache_inside_it(tmp_path: Path,
                                                                  monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("HOME", str(tmp_path / "home"))  # a cold cache, so Go must write one
    project = tmp_path / "go"
    project.mkdir()
    (project / "go.mod").write_text("module calc\n\ngo 1.21\n")
    (project / "calc.go").write_text("package calc\n")
    (project / "calc_test.go").write_text(GO_TEST)
    ran = suite.run(Witnesses({}, project, source=project), "go test ./...")
    assert "undefined: Double" in ran.printed and "operation not permitted" not in ran.printed, ran.output
    (project / "calc.go").write_text("package calc\n\nfunc Double(n int) int { return 2 * n }\n")
    passed = suite.run(Witnesses({}, project, source=project), "go test ./...")
    assert passed.code == 0, passed.output


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
    agent = Scripted(name="claude", plan={("cases", 1): write(CASES), ("reproduce", 1): write(nothing)})
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
    assert given.step == "cases"  # the scripted agent has no plan: the runner step passed
    assert c.output("r2", fb.RUNNER) == Runner(kind="given", suite="sh -c 'exit 0'", each="", found="given at start")


def test_the_agent_cannot_narrow_the_suite_a_fix_that_breaks_another_test_is_rejected(repo: Path,
                                                                                       tmp_path: Path) -> None:
    found = {**reproduction().model_dump(), "suite": f"{PYTHON} tests/test_add.py"}  # ignored: not in the schema
    breaks = "def add(a, b):\n    return 4 if (a, b) == (2, 2) else 1\n"  # test_add passes, test_zero fails
    agent = Scripted(name="claude", plan={
        ("cases", 1): write(CASES),
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
