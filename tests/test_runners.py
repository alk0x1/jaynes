"""Which command runs a project's tests, found from its files, so no agent names it."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from jaynes import runners
from jaynes.runners import Runner, detect, for_files


def project(tmp_path: Path, files: dict[str, str]) -> Path:
    for name, text in files.items():
        (tmp_path / name).parent.mkdir(parents=True, exist_ok=True)
        (tmp_path / name).write_text(text)
    return tmp_path


@pytest.mark.parametrize(("files", "suite", "chosen"), [
    ({"pyproject.toml": "[tool.pytest.ini_options]\n", "tests/test_a.py": ""},
     "python3 -m pytest -q -p no:cacheprovider", "python3 -m pytest -q -p no:cacheprovider tests/test_a.py"),
    ({"calc.py": "", "tests/test_a.py": ""},  # no configuration, but Python tests
     "python3 -m pytest -q -p no:cacheprovider", "python3 -m pytest -q -p no:cacheprovider tests/test_a.py"),
    ({"package.json": json.dumps({"scripts": {"test": "vitest run"}}), "src/a.test.ts": ""},
     "npm test", "npm test -- src/a.test.ts"),
    ({"package.json": json.dumps({"scripts": {"test": "jest"}}), "pnpm-lock.yaml": "", "src/a.test.ts": ""},
     "pnpm test", "pnpm test -- src/a.test.ts"),
    ({"go.mod": "module x\n", "pkg/calc/calc_test.go": ""}, "go test ./...", "go test ./pkg/calc"),
    ({"Cargo.toml": "[package]\n", "tests/adds.rs": ""}, "cargo test --offline", "cargo test --offline --test adds"),
    ({"Makefile": "build:\n\tcc x.c\n\ntest: build\n\t./run-tests\n", "t/a.sh": ""}, "make test", "make test"),
])
def test_each_runner_is_found_from_the_projects_files(tmp_path: Path, files: dict[str, str], suite: str,
                                                      chosen: str) -> None:
    found = detect(project(tmp_path, files))
    assert found is not None and found.suite == suite
    assert for_files(found, [next(f for f in files if "test" in f.lower() or f.startswith("t/"))]) == chosen


def test_a_project_s_virtualenv_runs_its_python_tests(tmp_path: Path) -> None:
    found = detect(project(tmp_path, {".venv/bin/python": "", "tests/test_a.py": ""}))
    assert found is not None and found.suite == ".venv/bin/python -m pytest -q -p no:cacheprovider"


def test_the_default_test_script_of_npm_init_is_no_runner(tmp_path: Path) -> None:
    script = 'echo "Error: no test specified" && exit 1'
    assert detect(project(tmp_path, {"package.json": json.dumps({"scripts": {"test": script}})})) is None


def test_nothing_found_is_none_and_a_suite_given_at_start_wins(tmp_path: Path) -> None:
    assert detect(project(tmp_path, {"README.md": "hi"})) is None
    given = detect(tmp_path, given="./check.sh")
    assert given is not None and given == Runner(kind="given", suite="./check.sh", each="", found="given at start")
    assert for_files(given, ["tests/x.sh"]) == "./check.sh"
    py = detect(project(tmp_path, {"tests/test_a.py": ""}), given="python3 -m pytest -q tests")
    assert py is not None and py.suite == "python3 -m pytest -q tests" and py.kind == "pytest"


def test_test_files_are_quoted_for_the_shell(tmp_path: Path) -> None:
    found = detect(project(tmp_path, {"tests/test_a.py": ""}))
    assert found is not None and for_files(found, ["tests/a b.py"]).endswith("'tests/a b.py'")


def test_dependency_folders_are_not_searched_for_tests(tmp_path: Path) -> None:
    assert detect(project(tmp_path, {"node_modules/x/test_a.py": "", ".venv/lib/test_b.py": ""})) is None
    assert runners.PYTHON == "python3"
