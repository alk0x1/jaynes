"""The cases that come with every check of the menu: each check has inputs it must reject and inputs it must
accept, every reject is refused and every accept accepted. Run from engine/:
    PYTHONPATH=python python3 -m unittest discover -s python/tests
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import jaynes_box as box  # noqa: E402
import jaynes_menu as menu  # noqa: E402
from jaynes_role import Blocked, Fail, Pass  # noqa: E402

CASES = Path(__file__).resolve().parent.parent / "cases"
# the checks that run the project's own commands, which need the box
RUN_COMMANDS = {"suite-passes", "tests-pass", "tests-fail-for-the-bug", "hidden-cases-pass"}
# a check with a reader of its own for its cases is held by the test that comes with it
OWN_READER = set(getattr(menu, "CASE_READERS", {}))
# a step of the menu that comes with cases and is judged as a check is: a reject is refused, an accept goes on
STEPS_WITH_CASES = {p.name for p in CASES.iterdir() if p.is_dir()} & set(menu.STEPS) - OWN_READER
# the steps that run the project's commands, which need the box
STEPS_RUN_COMMANDS = {"find-suite"}


def cases(check: str, file: str) -> list[dict[str, Any]]:
    path = CASES / check / file
    return json.loads(path.read_text()) if path.is_file() else []


class EveryCheckOfTheMenu(unittest.TestCase):
    def test_comes_with_a_case_to_reject_and_a_case_to_accept(self):
        self.assertEqual(STEPS_WITH_CASES, {"find-suite", "refuse-when-false"})
        for check in sorted(set(menu.CHECKS) - {"script"} | STEPS_WITH_CASES):
            with self.subTest(check):
                self.assertGreater(len(cases(check, "reject.json")), 0, f"{check} has no case it must reject")
                self.assertGreater(len(cases(check, "accept.json")), 0, f"{check} has no case it must accept")

    def test_has_no_cases_for_a_check_that_is_gone(self):
        # a step of the menu may come with cases too, such as one that downloads what an agent chose
        folders = {p.name for p in CASES.iterdir() if p.is_dir()}
        self.assertEqual(folders - set(menu.CHECKS) - set(menu.STEPS), set())

    def test_says_what_each_case_is_for(self):
        for check in sorted(set(menu.CHECKS) - {"script"} | STEPS_WITH_CASES):
            ids = [c.get("id") for c in cases(check, "reject.json") + cases(check, "accept.json")]
            self.assertEqual(len(ids), len(set(ids)), f"{check}: every case has an id of its own")
            for case in cases(check, "reject.json"):
                self.assertEqual((case.get("expect"), bool(case.get("hole"))), ("reject", True), f"{check} {case.get('id')}")
            for case in cases(check, "accept.json"):
                self.assertEqual((case.get("expect"), bool(case.get("keeps"))), ("accept", True), f"{check} {case.get('id')}")

    def test_refuses_every_reject_and_accepts_every_accept(self):
        ran = 0
        for check in sorted(set(menu.CHECKS) - {"script"} - OWN_READER | STEPS_WITH_CASES):
            if check in RUN_COMMANDS | STEPS_RUN_COMMANDS and not box.available():
                continue
            for case in cases(check, "reject.json"):
                with self.subTest(f"{check} rejects {case['id']}"):
                    got = menu.judge_case(check, {}, case)
                    self.assertIsInstance(got, Fail, f"{check} must reject {case['id']} ({case['hole']}), and said {got}")
                    ran += 1
            for case in cases(check, "accept.json"):
                with self.subTest(f"{check} accepts {case['id']}"):
                    got = menu.judge_case(check, {}, case)
                    self.assertIsInstance(got, Pass, f"{check} must accept {case['id']} ({case['keeps']}), and said {getattr(got, 'reasons', got)}")
                    ran += 1
        self.assertGreater(ran, 0)


class ACase(unittest.TestCase):
    def test_is_the_check_s_input_and_its_project_is_a_folder_of_its_own(self):
        seen: dict[str, Any] = {}

        def check(call: dict[str, Any], params: dict[str, Any]) -> Any:
            folder = Path(call["project"])
            seen.update(call=call, params=params, files={p.relative_to(folder).as_posix(): p.read_text() for p in folder.rglob("*") if p.is_file()})
            return Pass()

        case = {"output": {"n": 1}, "given": [{"step": "a", "output": {}}], "project": {"src/a.txt": "one\n", "b.txt": "two"}, "with": {"max": 5}}
        with mock.patch.dict(menu.CHECKS, {"probe": check}):
            self.assertIsInstance(menu.judge_case("probe", {"field": "n"}, case), Pass)
        self.assertEqual((seen["call"]["output"], seen["call"]["given"]), ({"n": 1}, [{"step": "a", "output": {}}]))
        self.assertEqual(seen["files"], {"src/a.txt": "one\n", "b.txt": "two"})
        self.assertEqual(seen["params"], {"field": "n", "max": 5})
        self.assertFalse(Path(seen["call"]["project"]).exists(), "the case's folder is gone once it is judged")

    def test_with_no_project_sees_none(self):
        with mock.patch.dict(menu.CHECKS, {"probe": lambda call, params: Pass() if call["project"] is None else Fail("a project")}):
            self.assertIsInstance(menu.judge_case("probe", {}, {"output": {}}), Pass)

    def test_that_cannot_be_judged_is_not_a_rejection(self):
        def breaks(call: dict[str, Any], params: dict[str, Any]) -> Any:
            raise KeyError("field")

        with mock.patch.dict(menu.CHECKS, {"probe": breaks}):
            got = menu.judge_case("probe", {}, {"output": {}})
        self.assertIsInstance(got, Blocked)
        self.assertIn("could not read the case", got.reason)
        for outside in ("../up.txt", "/etc/passwd"):
            self.assertIsInstance(menu.judge_case("field-required", {"field": "a"}, {"output": {"a": 1}, "project": {outside: "x"}}), Blocked)
        self.assertIsInstance(menu.judge_case("no-such-check", {}, {"output": {}}), Blocked)

    def test_of_a_role_s_own_check_runs_its_script(self):
        with mock.patch.object(menu, "script", return_value=Fail("no")) as ran:
            got = menu.judge_case("script", {"path": "/roles/r/checks/c.sh"}, {"output": {"a": 1}, "given": []})
        self.assertIsInstance(got, Fail)
        self.assertEqual((ran.call_args.args[0]["output"], ran.call_args.args[1]), ({"a": 1}, {"path": "/roles/r/checks/c.sh"}))

    def test_of_a_step_is_judged_by_what_the_step_does(self):
        # a step's reject is a refusal, said as a failure with the step's words; a step that goes on passes
        got = menu.judge_case("refuse-when-false", {}, {"with": {"field": "a.ok", "says": "not shown"}, "given": [{"step": "a", "output": {"ok": False}}], "output": {}})
        self.assertIsInstance(got, Fail)
        self.assertEqual(got.reasons, ["not shown"])
        self.assertIsInstance(menu.judge_case("refuse-when-false", {}, {"with": {"field": "a.ok", "says": "x"}, "given": [{"step": "a", "output": {"ok": True}}], "output": {}}), Pass)
        # a step whose cases another reader runs is not judged here
        self.assertIsInstance(menu.judge_case("fetch-pages", {}, {"output": {}}), Blocked)

    def test_is_asked_for_by_the_engine_as_a_verdict(self):
        said = menu.answer({"kind": "case", "use": "field-required", "with": {"field": "summary"}, "case": {"output": {"summary": " "}}})
        self.assertEqual((said["reply"], said["verdict"]["verdict"]), ("verdict", "fail"))


def a_start(folder: str, files: dict[str, str]) -> dict[str, str]:
    """The start of a run as the engine keeps it, beside the copy: a repository and its base commit."""
    repo = Path(folder) / "start"
    repo.mkdir()
    for path, text in files.items():
        (repo / path).parent.mkdir(parents=True, exist_ok=True)
        (repo / path).write_text(text)
    for args in (["init", "-q"], ["add", "-A"], ["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]):
        subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True)
    commit = subprocess.run(["git", "-C", str(repo), "rev-parse", "HEAD"], check=True, capture_output=True, text=True).stdout.strip()
    return {"git": str(repo / ".git"), "commit": commit}


@unittest.skipUnless(box.available(), "needs the box")
class TheSuiteAsTheRunBegan(unittest.TestCase):
    """When the run changed how the suite is defined (a Makefile, the test scripts of package.json, pytest's
    configuration), the suite runs again as the project defined it when the run began, over the code of now, and
    both pass: a suite the run made unable to fail is caught whatever made it so."""

    PERSON = "test:\n\t-rm gone.txt\n\tgrep -q hi greeting.txt\n"

    def judge(self, start: Any, makefile: str, greeting: str = "hi\n", check: str = "suite-passes", run: str = "") -> Any:
        with tempfile.TemporaryDirectory() as project:
            (Path(project) / "Makefile").write_text(makefile)
            (Path(project) / "greeting.txt").write_text(greeting)
            (Path(project) / "tests").mkdir()
            (Path(project) / "tests/says_hi.sh").write_text("grep -q hi greeting.txt\n")
            if run:
                (Path(project) / "tests/run.sh").write_text(run)
            runner = {"kind": "make", "suite": "make test", "each": "", "found": "Makefile test target"}
            given = [{"step": "runner", "output": runner}, {"step": "reproduce", "output": {"tests": ["tests/says_hi.sh"]}}]
            call = {"project": project, "given": given, "output": {}, "start": start}
            if check == "tests-pass":
                return menu.tests_pass(call, {"suite_step": "runner", "tests_from": "reproduce"})
            return menu.suite_passes(call, {"suite_step": "runner"})

    def test_a_failure_the_person_s_makefile_ignored_is_the_person_s(self):
        with tempfile.TemporaryDirectory() as folder:
            self.assertIsInstance(self.judge(a_start(folder, {"Makefile": self.PERSON}), self.PERSON), Pass)

    def test_a_makefile_changed_so_its_suite_cannot_fail_fails_as_the_suite_of_the_start_does(self):
        for now in ("test:\n\t-grep -q hi greeting.txt\n", "test:\n\tgrep -q hi greeting.txt || true\n",
                    "test:\n\tif grep -q hi greeting.txt; then :; fi\n", "test:\n\tgrep -q hi greeting.txt; echo done\n"):
            for check in ("suite-passes", "tests-pass"):
                with self.subTest(now=now, check=check), tempfile.TemporaryDirectory() as folder:
                    got = self.judge(a_start(folder, {"Makefile": "test:\n\tgrep -q hi greeting.txt\n"}), now, "hello\n", check)
                    self.assertIsInstance(got, Fail)
                    self.assertIn("as the project defined it when the run began", got.reasons[0])
                    self.assertIn("Makefile", got.reasons[0])

    def test_a_makefile_changed_whose_suite_of_the_start_still_passes_passes(self):
        # a target added, a line added beside the person's ignore, the tested line changed: the start's suite holds
        for now in (self.PERSON + "fmt:\n\t@echo formatted\n", self.PERSON + "\techo done\n",
                    self.PERSON.replace("grep -q hi", "grep -q h")):
            with self.subTest(now=now), tempfile.TemporaryDirectory() as folder:
                self.assertIsInstance(self.judge(a_start(folder, {"Makefile": self.PERSON}), now), Pass)

    def test_a_script_the_test_target_runs_is_held_as_it_began(self):
        makefile = "test:\n\tsh tests/run.sh\n"
        with tempfile.TemporaryDirectory() as folder:
            start = a_start(folder, {"Makefile": makefile, "tests/run.sh": "grep -q hi greeting.txt\n"})
            got = self.judge(start, makefile, "hello\n", run="grep -q hi greeting.txt || true\n")
        self.assertIsInstance(got, Fail)
        self.assertIn("the run changed tests/run.sh", got.reasons[0])
        # changed without harm, it passes as it began too
        with tempfile.TemporaryDirectory() as folder:
            start = a_start(folder, {"Makefile": makefile, "tests/run.sh": "grep -q hi greeting.txt\n"})
            self.assertIsInstance(self.judge(start, makefile, run="echo checking\ngrep -q hi greeting.txt\n"), Pass)

    def test_with_the_start_not_known_a_failure_ignored_or_a_line_that_cannot_fail_is_the_step_s(self):
        self.assertIsInstance(self.judge(None, self.PERSON), Fail)
        self.assertIsInstance(self.judge({"git": "/no/such/repository", "commit": "0" * 40}, self.PERSON), Fail)
        got = self.judge(None, "test:\n\tgrep -q hi greeting.txt || true\n")
        self.assertIsInstance(got, Fail)
        self.assertIn("grep -q hi greeting.txt || true", got.reasons[0])
        self.assertIsInstance(self.judge(None, "test:\n\tgrep -q hi greeting.txt\n"), Pass)


class WhatDefinesTheSuite(unittest.TestCase):
    """What the suite's definition is for each runner, and what is put back to run it as the run began."""

    def back(self, kind: str, start: dict[str, str], now: dict[str, str]) -> dict[str, Any]:
        with tempfile.TemporaryDirectory() as project:
            for path, text in now.items():
                (Path(project) / path).parent.mkdir(parents=True, exist_ok=True)
                (Path(project) / path).write_text(text)
            return menu.as_it_began(kind, (list(start), start.get), Path(project))

    def test_of_make_is_its_makefiles_and_what_they_include(self):
        start = {"Makefile": "include rules.inc\ntest:\n\tgrep -q hi greeting.txt\n", "rules.inc": "X = 1\n"}
        self.assertEqual(self.back("make", start, dict(start)), {})
        now = {**start, "rules.inc": ".IGNORE: test\n", "sub/Makefile": "check:\n\t-false\n"}
        self.assertEqual(self.back("make", start, now), {"rules.inc": "X = 1\n", "sub/Makefile": None})
        # what the tests are is not how the suite is defined: a test the step changed runs as it is now
        self.assertEqual(self.back("make", {**start, "tests/a.sh": "a"}, {**start, "tests/a.sh": "b"}), {})

    def test_of_make_holds_the_scripts_its_test_target_runs_and_not_the_files_it_reads(self):
        start = {"Makefile": "test: unit\nunit:\n\tsh tests/run.sh && ./tools/lint\n\tgrep -q hi greeting.txt\n\tpython3 -B tests/check.py x\n",
                 "tests/run.sh": "grep -q hi greeting.txt\n", "tools/lint": "true\n", "tests/check.py": "pass\n", "greeting.txt": "hello\n"}
        now = {**start, "tests/run.sh": "grep -q hi greeting.txt || true\n", "tools/lint": "exit 0\n", "tests/check.py": "x = 1\n", "greeting.txt": "hi\n"}
        self.assertEqual(self.back("make", start, now), {"tests/run.sh": start["tests/run.sh"], "tools/lint": "true\n", "tests/check.py": "pass\n"})
        # a script added beside, which no runner reads on its own and the start's Makefile does not run, stays as it is
        moved = {**start, "Makefile": start["Makefile"].replace("tests/run.sh", "tests/other.sh"), "tests/other.sh": "true\n"}
        self.assertEqual(self.back("make", start, moved), {"Makefile": start["Makefile"]})

    def test_of_node_holds_the_scripts_its_test_scripts_run(self):
        start = {"package.json": json.dumps({"scripts": {"test": "node tests/greet.js"}}), "tests/greet.js": "check()\n"}
        now = {**start, "tests/greet.js": "check() || true\n"}
        self.assertEqual(self.back("node", start, now), {"tests/greet.js": "check()\n"})

    def test_of_node_is_the_test_scripts_of_package_json(self):
        start = {"package.json": json.dumps({"name": "a", "scripts": {"test": "node --test", "build": "tsc"}})}
        same = {"package.json": json.dumps({"name": "a", "version": "2", "scripts": {"test": "node --test", "build": "esbuild"}})}
        self.assertEqual(self.back("node", start, same), {})
        now = {"package.json": json.dumps({"name": "a", "scripts": {"test": "node --test || true", "pretest": "exit 0", "build": "esbuild"}})}
        put = json.loads(self.back("node", start, now)["package.json"])
        self.assertEqual(put["scripts"], {"test": "node --test", "build": "esbuild"})

    def test_of_pytest_is_its_configuration_and_every_conftest(self):
        start = {"pytest.ini": "[pytest]\n", "tests/test_a.py": "def test_a(): pass\n"}
        self.assertEqual(self.back("pytest", start, dict(start)), {})
        now = {"pytest.ini": "[pytest]\naddopts = -k nothing\n", "tests/conftest.py": "skip everything", "tests/test_a.py": "changed"}
        self.assertEqual(self.back("pytest", start, now), {"pytest.ini": "[pytest]\n", "tests/conftest.py": None})

    def test_holds_the_script_the_suite_s_own_command_runs(self):
        start = {"tests/run.sh": "grep -q hi greeting.txt\n"}
        with tempfile.TemporaryDirectory() as project:
            (Path(project) / "tests").mkdir()
            (Path(project) / "tests/run.sh").write_text("grep -q hi greeting.txt\nexit 0\n")
            self.assertEqual(menu.as_it_began("given", (list(start), start.get), Path(project), "sh tests/run.sh"), start)
            self.assertEqual(menu.as_it_began("given", (list(start), start.get), Path(project), "npm test"), {})

    def test_an_if_hides_a_failure_only_when_its_body_cannot_fail(self):
        self.assertTrue(menu._cannot_fail("if grep -q hi greeting.txt; then :; fi"))
        self.assertTrue(menu._cannot_fail("if grep -q hi greeting.txt; then echo checked; fi"))
        for line in ('if [ "$(pbpaste)" = "x" ]; then echo "read it"; exit 1; fi', "if grep -q bye g; then false; fi",
                     "if grep -q hi g; then :; else exit 1; fi", "if test -f x; then rm x; fi"):
            with self.subTest(line=line):
                self.assertFalse(menu._cannot_fail(line))
        # the probe a test of the box runs as a suite: each of its ifs fails when it finds what it looks for
        probe = ('\nif [ "$(/usr/bin/pbpaste -pboard find 2>/dev/null)" = "t" ]; then echo "read the pasteboard"; exit 1; fi\n'
                 'printf w | /usr/bin/pbcopy -pboard find 2>/dev/null\n'
                 'if /usr/bin/security list-keychains >/dev/null 2>&1; then echo "reached the keychain"; exit 1; fi\n'
                 'grep -q hi greeting.txt\n')
        self.assertIsNone(menu._script_cannot_fail(probe))

    def test_a_shell_script_that_cannot_fail_is_told_and_one_that_can_is_not(self):
        for text in ("grep -q hi g || true\n", "grep -q hi g\nexit 0\n", "grep -q hi g\necho ok\n"):
            with self.subTest(text=text):
                self.assertTrue(menu._script_cannot_fail(text))
        for text in ("grep -q hi g\n", "grep -q hi g || exit 1\necho ok\n", "set -e\ngrep -q hi g\necho ok\n",
                     "#!/bin/sh -e\ngrep -q hi g\nexit 0\n", "echo nothing to test\n", "if grep -q hi g; then :; else exit 1; fi\n"):
            with self.subTest(text=text):
                self.assertIsNone(menu._script_cannot_fail(text))

    def test_of_a_runner_whose_suite_is_its_own_command_is_nothing(self):
        for kind in ("go", "cargo", "given"):
            self.assertEqual(self.back(kind, {"go.mod": "a"}, {"go.mod": "b"}), {})


if __name__ == "__main__":
    unittest.main()
