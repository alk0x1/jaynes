"""A project's tests that serve on a port Jaynes holds while the checks run (the usual databases and caches): Jaynes
finds it from what the tests printed, or from the project's own files when what they printed does not name the port,
and the run stops at its first step, before any agent works, saying so. Run from engine/:
    PYTHONPATH=python python3 -m unittest discover -s python/tests
"""

from __future__ import annotations

import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import jaynes_box as box  # noqa: E402
import jaynes_menu as menu  # noqa: E402
from jaynes_role import Blocked  # noqa: E402

HELD = tuple(box.HELD_PORTS)

# pytest's report of the real-user's run of 22/11 (uso-real, estoque): the port is six lines above the error
NAMED_ABOVE = """_____________________ ERROR at setup of test_ping_responde _____________________

    @pytest.fixture(scope="session")
    def banco_falso():
>       srv = _sobe_banco_falso(db.HOST, db.PORT)

tests/conftest.py:32:
_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _

host = '127.0.0.1', port = 5432

    def _sobe_banco_falso(host, port):
        srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
>       srv.bind((host, port))
E       OSError: [Errno 48] Address already in use

tests/conftest.py:12: OSError
=========================== short test summary info ============================
ERROR tests/test_db.py::test_ping_responde - OSError: [Errno 48] Address already in use
10 passed, 1 error in 0.02s
"""

# the same project before the agent touched it: a fixture with no arguments, so pytest names no port at all
NAMED_NOWHERE = """_____________________ ERROR at setup of test_ping_responde _____________________

    @pytest.fixture(scope="session")
    def banco_falso():
        srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
>       srv.bind((db.HOST, db.PORT))
E       OSError: [Errno 48] Address already in use

tests/conftest.py:14: OSError
=========================== short test summary info ============================
ERROR tests/test_db.py::test_ping_responde - OSError: [Errno 48] Address already in use
10 passed, 1 error in 0.02s
"""


def ran(printed: str, code: int = 1, held: tuple[int, ...] = HELD) -> box.Ran:
    return box.Ran(code, printed[-box.TAIL:], printed, test_servers=True, held=held)


MADE: list[Path] = []


def tearDownModule() -> None:
    for root in MADE:
        shutil.rmtree(root, ignore_errors=True)


def project(files: dict[str, str]) -> Path:
    root = Path(tempfile.mkdtemp(prefix="jaynes-held-"))
    MADE.append(root)
    for path, text in files.items():
        (root / path).parent.mkdir(parents=True, exist_ok=True)
        (root / path).write_text(text)
    return root


ESTOQUE = {
    "pyproject.toml": "[project]\nname = \"estoque\"\n",
    "estoque/db.py": "HOST = \"127.0.0.1\"\nPORT = 5432\n",
    "tests/conftest.py": "from estoque import db\n",
    "tests/test_db.py": "def test_ping_responde(banco_falso):\n    pass\n",
}


class WhatTheTestsPrinted(unittest.TestCase):
    def test_a_port_named_in_the_traceback_above_the_error_is_found(self):
        hit, lines = box.held_hit(ran(NAMED_ABOVE))
        self.assertEqual(hit, (5432,))
        self.assertEqual(lines, ["E       OSError: [Errno 48] Address already in use"])

    def test_a_port_named_in_another_test_s_report_is_not_this_error_s(self):
        other = "port = 5432\n\n_____ ERROR at setup of test_two _____\n" + "\n" * 3 + "E   OSError: [Errno 48] Address already in use\n"
        self.assertEqual(box.held_hit(ran(other))[0], ())


class WhatTheProjectNames(unittest.TestCase):
    def test_a_held_port_written_in_the_project_s_code_is_found(self):
        self.assertEqual(menu.held_named(project(ESTOQUE)), (5432,))

    def test_a_number_that_only_holds_the_port_s_digits_is_not_it(self):
        root = project({"a.py": "N = 15432\nX = 54321\nV = '1.5432'\nH = 'sha256:ab5432cd'\n"})
        self.assertEqual(menu.held_named(root), ())

    def test_locks_dependencies_and_git_are_not_read(self):
        root = project({"uv.lock": "port = 5432\n", "node_modules/pg/index.js": "5432\n", ".git/config": "5432\n",
                        ".venv/lib/x.py": "5432\n", "package-lock.json": "6379\n", "go.sum": "6379\n"})
        self.assertEqual(menu.held_named(root), ())

    def test_ports_come_in_the_box_s_order_each_once(self):
        root = project({"compose.yml": "redis: 6379\npg: 5432\n", "b.env": "PG=5432\n", "README.md": "on 11211.\n"})
        self.assertEqual(menu.held_named(root), (5432, 6379, 11211))


class TheRunStopsBeforeAnyAgent(unittest.TestCase):
    def suite_of(self, root: Path, printed: str, code: int = 1, **call: object) -> tuple[object, mock.Mock]:
        with mock.patch.object(menu.box, "run", return_value=ran(printed, code)) as run, \
                mock.patch.object(menu.box, "missing", return_value=None):
            return menu.find_suite({"project": str(root), "source": str(root), **call}, {}), run

    def test_tests_that_serve_on_a_held_port_block_the_first_step_saying_jaynes_held_it(self):
        root = project(ESTOQUE)
        found, run = self.suite_of(root, NAMED_NOWHERE, test_servers=True)
        self.assertIsInstance(found, Blocked)
        self.assertTrue(found.reason.startswith(menu.HELD_KEPT), found.reason)
        self.assertIn(" It tried held port 5432.", found.reason)
        self.assertIn(f"`jaynes project test-servers {root}`", found.reason)
        self.assertIn("E       OSError: [Errno 48] Address already in use", found.reason)
        self.assertEqual(run.call_count, 1)
        self.assertTrue(run.call_args.kwargs["test_servers"])

    def test_tests_that_pass_go_on_with_the_suite_found(self):
        found, run = self.suite_of(project(ESTOQUE), "10 passed", code=0, test_servers=True)
        self.assertIsInstance(found, dict)
        self.assertTrue(found["suite"])
        self.assertEqual(run.call_count, 1)

    def test_a_failure_of_the_project_s_own_is_for_the_checks_to_judge(self):
        found, _ = self.suite_of(project(ESTOQUE), "E   AssertionError: 2 != 3\n1 failed", test_servers=True)
        self.assertIsInstance(found, dict)

    def test_nothing_runs_first_without_test_servers_with_the_port_let_or_with_no_held_port_named(self):
        for root, call in [(project(ESTOQUE), {}),
                           (project(ESTOQUE), {"test_servers": True, "freed_ports": [5432]}),
                           (project(ESTOQUE), {"test_servers": True, "any_port": True}),
                           (project({**ESTOQUE, "estoque/db.py": "PORT = 8000\n"}), {"test_servers": True})]:
            found, run = self.suite_of(root, NAMED_NOWHERE, **call)
            self.assertIsInstance(found, dict, call)
            self.assertEqual(run.call_count, 0, call)


class AChecksTestsThatNameNoPort(unittest.TestCase):
    def test_a_check_finds_the_held_port_in_the_project_s_files(self):
        root = project(ESTOQUE)
        with mock.patch.object(menu.box, "run", return_value=ran(NAMED_NOWHERE)):
            got = menu._box({"project": str(root), "source": str(root), "test_servers": True}, "python3 -m pytest")
        self.assertIsInstance(got, Blocked)
        self.assertIn(" It tried held port 5432.", got.reason)

    def test_an_error_on_a_port_jaynes_did_not_hold_is_the_project_s(self):
        root = project(ESTOQUE)
        printed = NAMED_NOWHERE.replace("Address already in use", "Address already in use: 8000")
        for held in [HELD, (6379,)]:
            with mock.patch.object(menu.box, "run", return_value=ran(printed, held=held)):
                got = menu._box({"project": str(root), "source": str(root), "test_servers": True}, "python3 -m pytest")
            self.assertNotIsInstance(got, Blocked, held)



class TheSuiteAsTheRunBegan(unittest.TestCase):
    def test_the_suite_of_the_start_that_serves_on_a_held_port_is_jaynes_s_never_the_change_s(self):
        # the real-user's estoque, attempt 1.1: the agent moved the fake database to a free port in conftest.py, the
        # suite passed, and the suite as the project defined it when the run began, on 5432, failed for Jaynes's hold
        root = project(ESTOQUE)
        runs = [ran("11 passed", code=0), ran(NAMED_NOWHERE)]
        with mock.patch.object(menu.box, "run", side_effect=runs):
            got = menu._passes({"project": str(root), "source": str(root), "test_servers": True}, {"kind": "pytest"},
                               "python3 -m pytest", "the suite fails", {"tests/conftest.py": "the old one"})
        self.assertIsInstance(got, Blocked)
        self.assertTrue(got.reason.startswith(menu.HELD_KEPT), got.reason)
        self.assertIn(" It tried held port 5432.", got.reason)


if __name__ == "__main__":
    unittest.main()
