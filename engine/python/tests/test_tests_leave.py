"""The person lets a run change how the project's tests are defined (T1, the real-user's estoque, three times): with
their leave in words, suite-passes judges the suite as the agent left it, never the suite of the start, and says
which files of its definition changed, for the merge and the claims. Without it, nothing changes: the suite passes as
the project defined it when the run began too. Run from engine/:
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
from jaynes_role import Blocked, Fail, Pass, _verdict_reply  # noqa: E402

HELD = tuple(box.HELD_PORTS)
MADE: list[Path] = []

# the estoque as the run began: its fake database serves on 5432, a port Jaynes holds while the checks run
START = {
    "pyproject.toml": "[project]\nname = \"estoque\"\n",
    "estoque/db.py": "import os\nHOST, PORT = os.environ.get(\"ESTOQUE_DB\", \"127.0.0.1:5432\").split(\":\")\n",
    "tests/conftest.py": "import socket\nfrom estoque import db\n\ndef banco_falso():\n    s = socket.socket()\n    s.bind((db.HOST, int(db.PORT)))\n",
    "tests/test_db.py": "def test_ping_responde():\n    pass\n",
}
# and as the agent left it, after "pode mudar o teste, usa uma porta livre": the fake database on a free port
NOW = {**START, "tests/conftest.py": "import os, socket\n\ndef banco_falso():\n    s = socket.socket()\n    s.bind((\"127.0.0.1\", 0))\n    os.environ[\"ESTOQUE_DB\"] = \"127.0.0.1:%d\" % s.getsockname()[1]\n"}
WORDS = "pode mudar o teste, usa uma porta livre pro banco falso e passa pelo ESTOQUE_DB"

HELD_SAID = """_____________________ ERROR at setup of test_ping_responde _____________________
>       s.bind((db.HOST, int(db.PORT)))
E       OSError: [Errno 48] Address already in use

tests/conftest.py:6: OSError
1 error in 0.02s
"""


def tearDownModule() -> None:
    for root in MADE:
        shutil.rmtree(root, ignore_errors=True)


def project(files: dict[str, str]) -> Path:
    root = Path(tempfile.mkdtemp(prefix="jaynes-leave-"))
    MADE.append(root)
    for path, text in files.items():
        (root / path).parent.mkdir(parents=True, exist_ok=True)
        (root / path).write_text(text)
    return root


def ran(printed: str, code: int) -> box.Ran:
    return box.Ran(code, printed[-box.TAIL:], printed, test_servers=True, held=HELD)


def judged(files: dict[str, str], runs: list[box.Ran], **call: object) -> tuple[object, mock.Mock]:
    root = project(files)
    whole = {"project": str(root), "source": str(root), "test_servers": True, "start": {"files": START},
             "given": [{"step": "runner", "output": {"kind": "pytest", "suite": "python3 -m pytest"}}], **call}
    with mock.patch.object(menu.box, "run", side_effect=runs) as run:
        return menu.suite_passes(whole, {"suite_step": "runner"}), run


class AskedTheEstoque(unittest.TestCase):
    def test_asked_with_the_person_s_leave_the_check_runs_the_conftest_the_agent_left(self):
        got, run = judged(NOW, [ran("11 passed", 0)], tests_leave=WORDS)
        self.assertIsInstance(got, Pass, got)
        self.assertEqual(run.call_count, 1, "the suite of the start, on 5432, is not run again")
        self.assertEqual(got.tests_changed, ["tests/conftest.py"])
        self.assertEqual(_verdict_reply(got), {"reply": "verdict", "verdict": {"verdict": "pass"}, "tests_changed": ["tests/conftest.py"]})

    def test_without_it_the_suite_of_the_start_still_judges_and_is_held_on_5432(self):
        got, run = judged(NOW, [ran("11 passed", 0), ran(HELD_SAID, 1)])
        self.assertIsInstance(got, Blocked, got)
        self.assertEqual(run.call_count, 2)

    def test_the_leave_does_not_hide_a_failure_of_the_suite_the_agent_left(self):
        got, _ = judged(NOW, [ran("E   assert 2 == 3\n1 failed", 1)], tests_leave=WORDS)
        self.assertIsInstance(got, Fail, got)

    def test_the_leave_is_for_changing_the_tests_never_for_a_suite_that_cannot_fail(self):
        made = {"Makefile": "test:\n\t-python3 -m pytest\n"}
        root = project(made)
        call = {"project": str(root), "source": str(root), "start": {"files": {"Makefile": "test:\n\tpython3 -m pytest\n"}},
                "given": [{"step": "runner", "output": {"kind": "make", "suite": "make test"}}], "tests_leave": WORDS}
        with mock.patch.object(menu.box, "run", side_effect=[box.Ran(0, "", "make: [test] Error 1 (ignored)\n")]):
            got = menu.suite_passes(call, {"suite_step": "runner"})
        self.assertIsInstance(got, Fail, got)
        self.assertIn("make went on after a failure", " ".join(got.reasons))

    def test_with_the_leave_and_nothing_of_the_definition_changed_nothing_is_said(self):
        got, run = judged(START, [ran("11 passed", 0)], tests_leave=WORDS)
        self.assertIsInstance(got, Pass, got)
        self.assertEqual(got.tests_changed, [])
        self.assertEqual(_verdict_reply(got), {"reply": "verdict", "verdict": {"verdict": "pass"}})


# The real-user's case 2 (uso-real/achados/2026-11-23-02.md, run r1795449920-193, seq 782 to 795): the fix-bug role's
# tests-pass, not suite-passes. The team's tests end in _test.py, the pyproject collected only test_*.py, the person
# wrote "pode mudar o pyproject pra rodar os *_test.py também" (TestsLeft, seq 782), and fix 2.0 still failed "as the
# project defined it when the run began" (seq 795): the leave reached the call and tests-pass never read it.
CONTAS_START = {
    "pyproject.toml": "[tool.pytest.ini_options]\npython_files = [\"test_*.py\"]\n",
    "contas/relatorio.py": "def dias_do_mes(ultimo):\n    return range(1, ultimo)\n",
    "tests/relatorio_test.py": "def test_marco_fecha_no_31():\n    pass\n",
}
CONTAS_NOW = {**CONTAS_START,
              "pyproject.toml": "[tool.pytest.ini_options]\npython_files = [\"test_*.py\", \"*_test.py\"]\n",
              "contas/relatorio.py": "def dias_do_mes(ultimo):\n    return range(1, ultimo + 1)\n",
              "tests/test_linha.py": "def test_arquivos_no_padrao_do_time_rodam_com_a_suite():\n    pass\n"}
CONTAS_WORDS = "pode mudar o pyproject pra rodar os *_test.py também"
CONTAS_FAILED = "E       AssertionError: a suíte só procura testes em: test_*.py\n1 failed, 5 passed\n"


class AskedTheContas(unittest.TestCase):
    def judged(self, **call: object) -> tuple[object, list[dict[str, object]]]:
        root = project(CONTAS_NOW)
        calls: list[dict[str, object]] = []

        def run(where: Path, command: str, **kw: object) -> box.Ran:
            calls.append({"where": Path(where), "over": kw.get("over")})
            if kw.get("over"):  # the pyproject as the run began: relatorio_test.py and the config's test never pass
                return box.Ran(1, CONTAS_FAILED, CONTAS_FAILED)
            if Path(where) != root:  # the tests alone, beside none of the code: they fail
                return box.Ran(1, "E   ModuleNotFoundError: No module named 'contas'\n", "")
            return box.Ran(0, "6 passed", "6 passed")
        whole = {"project": str(root), "source": str(root), "start": {"files": CONTAS_START},
                 "given": [{"step": "runner", "output": {"kind": "pytest", "suite": ".venv/bin/python -m pytest -q -p no:cacheprovider"}},
                           {"step": "reproduce", "output": {"tests": ["tests/test_linha.py", "tests/relatorio_test.py"]}}],
                 **call}
        with mock.patch.object(menu.box, "run", side_effect=run):
            return menu.tests_pass(whole, {"suite_step": "runner", "tests_from": "reproduce"}), calls

    def test_asked_with_the_person_s_leave_tests_pass_judges_the_pyproject_the_agent_left_and_names_it(self):
        got, calls = self.judged(tests_leave=CONTAS_WORDS)
        self.assertIsInstance(got, Pass, got)
        self.assertEqual(got.tests_changed, ["pyproject.toml"])
        self.assertFalse([c for c in calls if c["over"]], "the pyproject of the start is not put back")

    def test_without_it_tests_pass_still_judges_as_the_project_began(self):
        got, _ = self.judged()
        self.assertIsInstance(got, Fail, got)
        self.assertIn("as the project defined it when the run began", " ".join(got.reasons))

    def test_the_leave_never_lets_the_bug_s_tests_fail(self):
        root = project(CONTAS_NOW)
        whole = {"project": str(root), "source": str(root), "start": {"files": CONTAS_START}, "tests_leave": CONTAS_WORDS,
                 "given": [{"step": "runner", "output": {"kind": "pytest", "suite": "python3 -m pytest"}},
                           {"step": "reproduce", "output": {"tests": ["tests/relatorio_test.py"]}}]}
        with mock.patch.object(menu.box, "run", side_effect=[box.Ran(0, "tests/relatorio_test.py::test_x\n", ""), box.Ran(1, "E   assert 10 == 210\n1 failed", "")]):
            got = menu.tests_pass(whole, {"suite_step": "runner", "tests_from": "reproduce"})
        self.assertIsInstance(got, Fail, got)


# The real-user's contas, round of 16:08 on 76a06b8a (uso-real/achados/2026-11-23-03.md, runs r1795460919-451 and -455,
# the fix-bug of a8d3944): "tests pass" counted a test the suite never ran. Case 2: reproduce wrote its test in
# tests/relatorio_test.py, which the pyproject as the run began does not collect; the agent added "*_test.py" to
# python_files with nobody's leave, and tests-pass ran the file by its path (pytest runs a file it is given even when no
# pattern matches it) and the suite (which as the start defined it never holds that file): green. Case 1: the test is in
# tests/test_linha.py, which the suite as the start defined it does collect.
def _collected_by(pyproject: str, files: list[str]) -> list[str]:
    import fnmatch
    import re
    got = re.search(r"python_files\s*=\s*\[(.*?)\]", pyproject)
    patterns = re.findall(r"\"([^\"]+)\"", got.group(1)) if got else ["test_*.py"]
    return [f for f in files if any(fnmatch.fnmatch(Path(f).name, g) for g in patterns)]


class TheTestOfTheBugIsAmongTheOnesRun(unittest.TestCase):
    def judged(self, tests: list[str], now: dict[str, str], check: str = "tests_pass", **call: object) -> tuple[object, list[str]]:
        root = project(now)
        ran_commands: list[str] = []
        files = ["tests/relatorio_test.py", "tests/test_linha.py"]

        def run(where: Path, command: str, **kw: object) -> box.Ran:
            ran_commands.append(command)
            if "--collect-only" in command:  # -qq  # what pytest lists, by the pyproject it reads: the start's when put back
                seen = (kw.get("over") or {}).get("pyproject.toml") or (Path(where) / "pyproject.toml").read_text()
                listed = "".join(f"{f}::test_x\n" for f in _collected_by(seen, files))
                return box.Ran(0, listed, listed)
            if Path(where) != root:
                return box.Ran(1, "E   ModuleNotFoundError: No module named 'contas'\n", "")
            return box.Ran(0, "6 passed", "6 passed")
        whole = {"project": str(root), "source": str(root), "start": {"files": CONTAS_START},
                 "given": [{"step": "runner", "output": {"kind": "pytest", "suite": "python3 -m pytest"}},
                           {"step": "reproduce", "output": {"tests": tests}}], **call}
        params = {"suite_step": "runner", "tests_from": "reproduce"}
        with mock.patch.object(menu.box, "run", side_effect=run):
            return getattr(menu, check)(whole, params), ran_commands

    def test_case_2_a_test_the_suite_of_the_start_never_collects_is_refused_and_says_why(self):
        got, _ = self.judged(["tests/relatorio_test.py"], CONTAS_NOW)
        self.assertIsInstance(got, Fail, got)
        said = " ".join(got.reasons)
        self.assertIn("tests/relatorio_test.py", said)
        self.assertIn("does not collect", said)
        self.assertIn("pyproject.toml", said)

    def test_case_2_with_the_person_s_leave_the_suite_as_the_agent_left_it_collects_it_and_it_passes(self):
        got, _ = self.judged(["tests/relatorio_test.py"], CONTAS_NOW, tests_leave=CONTAS_WORDS)
        self.assertIsInstance(got, Pass, got)

    def test_with_the_leave_a_suite_left_that_no_longer_collects_the_test_is_refused(self):
        now = {**CONTAS_NOW, "pyproject.toml": "[tool.pytest.ini_options]\npython_files = [\"test_*.py\"]\n# changed\n"}
        got, _ = self.judged(["tests/relatorio_test.py"], now, tests_leave=CONTAS_WORDS)
        self.assertIsInstance(got, Fail, got)
        self.assertIn("does not collect", " ".join(got.reasons))

    def test_the_listing_pytest_7_2_1_prints_on_titan_in_both_forms_is_read(self):
        # titan, jaynes-linux-test: `--collect-only -qq` prints "path: N" (with addopts none or -q) and node ids (-v)
        for printed in ("tests/relatorio_test.py: 1\ntests/test_linha.py: 1\n", "tests/relatorio_test.py::test_a\n1 test collected in 0.00s\n"):
            root = project(CONTAS_NOW)
            with mock.patch.object(menu.box, "run", return_value=box.Ran(0, printed, printed)) as run:
                got = menu._collected({"project": str(root), "source": str(root)}, {"kind": "pytest", "suite": "python3 -m pytest"},
                                      ["tests/relatorio_test.py", "tests/other_test.py"], None, False)
            self.assertEqual(got, ["tests/other_test.py"], printed)
            self.assertIn("--collect-only -qq", run.call_args[0][1])

    def test_case_1_a_test_the_suite_of_the_start_collects_still_passes(self):
        got, _ = self.judged(["tests/test_linha.py"], CONTAS_NOW)
        self.assertIsInstance(got, Pass, got)

    def test_suite_passes_asked_for_the_tests_of_a_step_refuses_the_same(self):
        got, _ = self.judged(["tests/relatorio_test.py"], CONTAS_NOW, check="suite_passes")
        self.assertIsInstance(got, Fail, got)
        self.assertIn("does not collect", " ".join(got.reasons))


if __name__ == "__main__":
    unittest.main()
