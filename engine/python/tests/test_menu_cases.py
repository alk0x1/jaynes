"""The cases that come with every check of the menu: each check has inputs it must reject and inputs it must
accept, every reject is refused and every accept accepted. Run from engine/:
    PYTHONPATH=python python3 -m unittest discover -s python/tests
"""

from __future__ import annotations

import json
import sys
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


def cases(check: str, file: str) -> list[dict[str, Any]]:
    path = CASES / check / file
    return json.loads(path.read_text()) if path.is_file() else []


class EveryCheckOfTheMenu(unittest.TestCase):
    def test_comes_with_a_case_to_reject_and_a_case_to_accept(self):
        for check in sorted(set(menu.CHECKS) - {"script"}):
            with self.subTest(check):
                self.assertGreater(len(cases(check, "reject.json")), 0, f"{check} has no case it must reject")
                self.assertGreater(len(cases(check, "accept.json")), 0, f"{check} has no case it must accept")

    def test_has_no_cases_for_a_check_that_is_gone(self):
        folders = {p.name for p in CASES.iterdir() if p.is_dir()}
        self.assertEqual(folders - set(menu.CHECKS), set())

    def test_says_what_each_case_is_for(self):
        for check in sorted(set(menu.CHECKS) - {"script"}):
            ids = [c.get("id") for c in cases(check, "reject.json") + cases(check, "accept.json")]
            self.assertEqual(len(ids), len(set(ids)), f"{check}: every case has an id of its own")
            for case in cases(check, "reject.json"):
                self.assertEqual((case.get("expect"), bool(case.get("hole"))), ("reject", True), f"{check} {case.get('id')}")
            for case in cases(check, "accept.json"):
                self.assertEqual((case.get("expect"), bool(case.get("keeps"))), ("accept", True), f"{check} {case.get('id')}")

    def test_refuses_every_reject_and_accepts_every_accept(self):
        ran = 0
        for check in sorted(set(menu.CHECKS) - {"script"} - OWN_READER):
            if check in RUN_COMMANDS and not box.available():
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

    def test_is_asked_for_by_the_engine_as_a_verdict(self):
        said = menu.answer({"kind": "case", "use": "field-required", "with": {"field": "summary"}, "case": {"output": {"summary": " "}}})
        self.assertEqual((said["reply"], said["verdict"]["verdict"]), ("verdict", "fail"))


if __name__ == "__main__":
    unittest.main()
