"""The cases that come with quotes-found, each run through fetch-pages and quotes-found as a run would, without the
network: every accept is accepted and every reject refused. Run from engine/:
    PYTHONPATH=python python3 -m unittest discover -s python/tests
"""

from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import jaynes_menu as menu  # noqa: E402
from jaynes_role import Fail, Pass  # noqa: E402

CASES = Path(__file__).resolve().parent.parent / "cases" / "quotes-found"


def verdict(case: dict[str, Any]) -> Any:
    """What quotes-found says of a case, by the menu's own reader of its cases."""
    return menu.CASE_READERS["quotes-found"](case)


def load(name: str) -> list[dict[str, Any]]:
    path = CASES / name
    return json.loads(path.read_text()) if path.is_file() else []


class Cases(unittest.TestCase):
    def test_every_case_says_what_it_is_for(self):
        for case in load("accept.json"):
            self.assertEqual(case["expect"], "accept", case["id"])
            self.assertTrue(case.get("keeps"), case["id"])
        for case in load("reject.json"):
            self.assertEqual(case["expect"], "reject", case["id"])
            self.assertTrue(case.get("hole"), case["id"])
        ids = [c["id"] for c in load("accept.json") + load("reject.json")]
        self.assertEqual(len(ids), len(set(ids)), "every case has an id of its own")

    def test_the_check_comes_with_cases_to_accept(self):
        self.assertGreater(len(load("accept.json")), 0)

    def test_every_accept_is_accepted(self):
        for case in load("accept.json"):
            with self.subTest(case["id"]):
                got = verdict(case)
                self.assertIsInstance(got, Pass, f"{case['id']} ({case['keeps']}): {getattr(got, 'reasons', got)}")

    def test_every_reject_is_refused(self):
        rejects = load("reject.json")
        if not rejects:
            self.skipTest("reject.json is written by someone other than the check's author, and has not come yet")
        for case in rejects:
            with self.subTest(case["id"]):
                self.assertIsInstance(verdict(case), Fail, f"{case['id']} passed, and the hole it closes is open: {case['hole']}")


if __name__ == "__main__":
    unittest.main()
