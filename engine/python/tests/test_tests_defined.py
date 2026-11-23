"""A step leaves how the project's tests are defined as the run began, unless the person let it change (the
real-user's contas, 23/11: fix-bug's fix added "*_test.py" to pyproject's python_files without asking, the risk rose
to high and the run cost 5.3 times the direct one). tests-defined-as-they-began refuses a step that changed the
test settings, by code, and passes one with the person's leave, saying which files changed. Run from engine/:
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

import jaynes_menu as menu  # noqa: E402
from jaynes_role import Blocked, Fail, Pass  # noqa: E402

MADE: list[Path] = []

# contas as the run began (ef71b87)
PYPROJECT = """[project]
name = "contas"
version = "0.1.0"
requires-python = ">=3.11"

[dependency-groups]
dev = ["pytest>=8"]

[tool.pytest.ini_options]
testpaths = ["tests"]
pythonpath = ["."]
python_files = ["test_*.py"]
"""
START = {
    "pyproject.toml": PYPROJECT,
    "contas/relatorio.py": "def ultimo_dia(mes):\n    return 30\n",
    "tests/test_relatorio.py": "def test_existe():\n    pass\n",
    "tests/relatorio_test.py": "def test_antigo():\n    pass\n",
}
# as the fix 1.0 of r1795460919-451 left it
WIDENED = PYPROJECT.replace('python_files = ["test_*.py"]', 'python_files = ["test_*.py", "*_test.py"]')
PYTEST = {"step": "runner", "output": {"kind": "pytest", "suite": "python3 -m pytest", "each": "python3 -m pytest"}}


def tearDownModule() -> None:
    for root in MADE:
        shutil.rmtree(root, ignore_errors=True)


def judged(now: dict[str, str], start: dict[str, str] | None = START, runner: dict | None = None, **call: object):
    root = Path(tempfile.mkdtemp(prefix="jaynes-defined-"))
    MADE.append(root)
    for path, text in now.items():
        (root / path).parent.mkdir(parents=True, exist_ok=True)
        (root / path).write_text(text)
    whole = {"project": str(root), "given": [runner or PYTEST], "start": {"files": start} if start is not None else None,
             **call}
    with mock.patch.object(menu.box, "run", side_effect=AssertionError("the check runs nothing")):
        return menu.tests_defined_as_they_began(whole, {"suite_step": "runner"})


class TheContas(unittest.TestCase):
    def test_a_step_that_widened_python_files_without_leave_is_refused_and_told_why(self):
        got = judged({**START, "pyproject.toml": WIDENED, "contas/relatorio.py": "def ultimo_dia(mes):\n    return 31\n"})
        self.assertIsInstance(got, Fail, got)
        said = " ".join(got.reasons)
        self.assertIn("pyproject.toml", said)
        self.assertIn("as it was when the run began", said)

    def test_with_the_person_s_leave_it_passes_and_says_the_file(self):
        got = judged({**START, "pyproject.toml": WIDENED}, tests_leave="pode mudar o python_files")
        self.assertIsInstance(got, Pass, got)
        self.assertEqual(got.tests_changed, ["pyproject.toml"])

    def test_a_test_added_where_the_suite_collects_it_passes(self):
        got = judged({**START, "tests/test_linha.py": "def test_31():\n    assert False\n"})
        self.assertIsInstance(got, Pass, got)
        self.assertFalse(getattr(got, "tests_changed", None))

    def test_a_test_written_into_a_file_the_suite_does_not_collect_is_not_this_check_s(self):
        # collected or not is tests-fail-for-the-bug's (collected = true); this one judges the settings only
        got = judged({**START, "tests/relatorio_test.py": "def test_31():\n    assert False\n"})
        self.assertIsInstance(got, Pass, got)


class ThePytestSettings(unittest.TestCase):
    def test_a_change_to_pyproject_outside_the_test_settings_passes(self):
        added = PYPROJECT.replace('requires-python = ">=3.11"\n', 'requires-python = ">=3.11"\ndependencies = ["python-dateutil"]\n')
        self.assertIsInstance(judged({**START, "pyproject.toml": added}), Pass)

    def test_the_settings_moved_to_a_dotted_key_under_tool_are_refused(self):
        moved = PYPROJECT.replace("[tool.pytest.ini_options]\n", "[tool]\npytest.ini_options.addopts = \"-p no:cacheprovider\"\n[tool.pytest.ini_options]\n")
        self.assertIsInstance(judged({**START, "pyproject.toml": moved}), Fail)

    def test_a_quoted_header_is_the_same_table(self):
        quoted = WIDENED.replace("[tool.pytest.ini_options]", '[ tool . "pytest" . ini_options ]')
        self.assertIsInstance(judged({**START, "pyproject.toml": quoted}), Fail)

    def test_a_conftest_added_or_a_pytest_ini_added_is_refused(self):
        for path, text in (("tests/conftest.py", "collect_ignore = ['test_relatorio.py']\n"), ("pytest.ini", "[pytest]\npython_files = *.py\n")):
            with self.subTest(path):
                self.assertIsInstance(judged({**START, path: text}), Fail)

    def test_setup_cfg_and_tox_ini_count_by_their_pytest_section(self):
        for name, section in (("setup.cfg", "tool:pytest"), ("tox.ini", "pytest")):
            with self.subTest(name):
                start = {**START, name: f"[metadata]\nname = contas\n\n[{section}]\npython_files = test_*.py\n"}
                other = {**start, name: start[name].replace("name = contas", "name = contas2")}
                self.assertIsInstance(judged(other, start), Pass)
                widened = {**start, name: start[name].replace("test_*.py", "test_*.py *_test.py")}
                self.assertIsInstance(judged(widened, start), Fail)

    def test_a_removed_pytest_ini_is_refused(self):
        start = {**START, "pytest.ini": "[pytest]\npython_files = test_*.py\n"}
        now = {p: t for p, t in start.items() if p != "pytest.ini"}
        self.assertIsInstance(judged(now, start), Fail)


class OtherRunners(unittest.TestCase):
    def test_make_s_test_target_and_node_s_test_script(self):
        make = {"step": "runner", "output": {"kind": "make", "suite": "make test", "each": ""}}
        start = {"Makefile": "test:\n\tsh tests/run.sh\n", "tests/run.sh": "grep -q hi g.txt\n", "g.txt": "hi\n"}
        self.assertIsInstance(judged({**start, "tests/run.sh": "true\n"}, start, make), Fail)
        self.assertIsInstance(judged({**start, "g.txt": "hello\n"}, start, make), Pass)
        node = {"step": "runner", "output": {"kind": "node", "suite": "npm test", "each": ""}}
        start = {"package.json": '{"name": "a", "scripts": {"test": "node --test"}, "dependencies": {}}'}
        self.assertIsInstance(judged({"package.json": '{"name": "a", "scripts": {"test": "node --test"}, "dependencies": {"x": "1"}}'}, start, node), Pass)
        self.assertIsInstance(judged({"package.json": '{"name": "a", "scripts": {"test": "true"}}'}, start, node), Fail)


class WhatItCannotSee(unittest.TestCase):
    def test_with_the_start_unknown_it_cannot_judge(self):
        self.assertIsInstance(judged(START, None), Blocked)

    def test_with_no_suite_found_there_is_nothing_to_hold(self):
        self.assertIsInstance(judged(START, START, {"step": "runner", "output": {}}), Pass)


if __name__ == "__main__":
    unittest.main()
