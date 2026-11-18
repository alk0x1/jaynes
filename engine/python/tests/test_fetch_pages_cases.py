"""The cases that come with fetch-pages, each run as a run reads a page and without the network: every page it must
refuse is not read and nothing is sent where it may not go, and every page it must read is read. Run from engine/:
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

import jaynes_menu as menu  # noqa: E402
from jaynes_role import Fail, Pass  # noqa: E402

CASES = Path(__file__).resolve().parent.parent / "cases" / "fetch-pages"


def load(name: str) -> list[dict[str, Any]]:
    return json.loads((CASES / name).read_text())


class Cases(unittest.TestCase):
    def test_every_case_says_what_it_is_for(self):
        for case in load("reject.json"):
            self.assertEqual((case["expect"], bool(case.get("hole"))), ("reject", True), case["id"])
        for case in load("accept.json"):
            self.assertEqual((case["expect"], bool(case.get("keeps"))), ("accept", True), case["id"])
        ids = [c["id"] for c in load("reject.json") + load("accept.json")]
        self.assertEqual(len(ids), len(set(ids)), "every case has an id of its own")
        self.assertGreater(len(load("reject.json")), 0)
        self.assertGreater(len(load("accept.json")), 0)

    def test_every_page_it_must_refuse_is_not_read_and_nothing_goes_where_it_may_not(self):
        for case in load("reject.json"):
            with self.subTest(case["id"]):
                got = menu.CASE_READERS["fetch-pages"](case)
                self.assertIsInstance(got, Fail, f"{case['id']} ({case['hole']}) was let through")

    def test_every_page_it_must_read_is_read(self):
        for case in load("accept.json"):
            with self.subTest(case["id"]):
                got = menu.CASE_READERS["fetch-pages"](case)
                self.assertIsInstance(got, Pass, f"{case['id']} ({case['keeps']}): {getattr(got, 'reasons', got)}")


class TheReader(unittest.TestCase):
    """The reader proves nothing unless it would see a page let through: each way of letting one through is seen."""

    PUBLIC = {"id": "p", "url": "https://docs.example.org/a", "resolve": {"docs.example.org": ["93.184.215.14"]},
              "answers": {"https://docs.example.org/a": {"status": 200, "headers": {"content-type": "text/html"}, "body": "<p>FILL</p>"}}}

    def test_sees_a_page_read(self):
        self.assertIsInstance(menu.CASE_READERS["fetch-pages"](self.PUBLIC), Pass)

    def test_sees_a_request_sent_to_a_private_address_though_nothing_was_read(self):
        private = {**self.PUBLIC, "resolve": {"docs.example.org": ["10.0.0.1"]}, "answers": {}}
        with mock.patch.object(menu, "_public", return_value=True):  # a fetch-pages that forgot the rule
            self.assertIsInstance(menu.CASE_READERS["fetch-pages"](private), Pass)

    def test_sees_a_page_refused_and_says_why(self):
        got = menu.CASE_READERS["fetch-pages"]({**self.PUBLIC, "url": "http://docs.example.org/a"})
        self.assertIsInstance(got, Fail)
        self.assertIn("only https addresses are downloaded", got.reasons[0])

    def test_makes_a_body_as_large_as_it_says(self):
        big = {**self.PUBLIC, "answers": {"https://docs.example.org/a": {"status": 200, "headers": {"content-type": "text/html"},
                                                                          "body": "<p>FILL</p>", "body_bytes": menu.PAGE_BYTES + 1}}}
        got = menu.CASE_READERS["fetch-pages"](big)
        self.assertIsInstance(got, Fail)
        self.assertIn("larger than", got.reasons[0])

    def test_needs_no_network(self):
        with mock.patch("socket.create_connection", side_effect=AssertionError("a case reached the network")):
            for case in load("reject.json") + load("accept.json"):
                menu.CASE_READERS["fetch-pages"](case)


if __name__ == "__main__":
    unittest.main()
