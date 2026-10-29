"""Which command runs a project's tests, found by Jaynes from the project's files rather than named by an agent.

An agent that names the suite command can name less than the suite. ``detect`` reads the project instead: a
Python project runs pytest, a Node project its ``test`` script, Go ``go test``, Rust ``cargo test``, and
anything else a Makefile ``test`` target. The person can give the suite at the start of a run, and that wins.
``for_files`` runs only some test files where the runner can pick them, and the whole suite where it cannot.
"""

from __future__ import annotations

import json
import os
import re
import shlex
from pathlib import Path
from typing import Literal, cast

from pydantic import BaseModel, Field

from jaynes.preview import DEPENDENCIES

PYTHON = "python3"  # without a virtualenv in the project
PYTEST = "-m pytest -q -p no:cacheprovider"
NPM_DEFAULT = "no test specified"  # the test script npm init writes, which only fails


class Runner(BaseModel):
    kind: Literal["pytest", "node", "go", "cargo", "make", "given"]
    suite: str = Field(description="runs the whole test suite, from the project folder")
    each: str = Field(description="followed by test files, runs only them; empty when the runner cannot pick "
                                  "files, and then the suite runs")
    found: str = Field(description="what it was found from")


def detect(root: Path, given: str = "") -> Runner | None:
    """The runner of the project at ``root``, or None. ``given``, the person's suite command, replaces the
    suite; the runner found still picks test files when it can."""
    found = _python(root) or _node(root) or _go(root) or _cargo(root) or _make(root)
    if not given:
        return found
    if found is None or found.kind == "make":
        return Runner(kind="given", suite=given, each="", found="given at start")
    return found.model_copy(update={"suite": given, "found": f"suite given at start; files by {found.found}"})


def for_files(runner: Runner, tests: list[str]) -> str:
    """The command that runs ``tests``, paths relative to the project folder."""
    files = [shlex.quote(t) for t in tests]
    if runner.kind == "go":  # go test takes packages, the folders of the files
        dirs = sorted({Path(t).parent.as_posix() for t in tests})
        return f"{runner.each} {' '.join(shlex.quote('.' if d == '.' else './' + d) for d in dirs)}"
    if runner.kind == "cargo":
        named = [f"--test {shlex.quote(Path(t).stem)}" for t in tests if Path(t).parts[:1] == ("tests",)]
        return f"{runner.each} {' '.join(named)}" if named else runner.suite
    if not runner.each or not files:
        return runner.suite
    return f"{runner.each} {' '.join(files)}"


def _python(root: Path) -> Runner | None:
    config = [n for n in ("pytest.ini", "conftest.py") if (root / n).is_file()]
    for name, mark in (("pyproject.toml", "pytest"), ("setup.cfg", "[tool:pytest]"), ("tox.ini", "[pytest]")):
        if (root / name).is_file() and mark in (root / name).read_text(errors="replace"):
            config.append(name)
    if not config and not _has(root, re.compile(r"(^test_.*|.*_test)\.py$")):
        return None
    python = ".venv/bin/python" if (root / ".venv" / "bin" / "python").exists() else PYTHON
    command = f"{python} {PYTEST}"
    return Runner(kind="pytest", suite=command, each=command, found=config[0] if config else "Python test files")


def _node(root: Path) -> Runner | None:
    try:
        package: object = json.loads((root / "package.json").read_text())
    except (OSError, ValueError):
        return None
    scripts = cast(dict[str, object], package).get("scripts") if isinstance(package, dict) else None
    script = cast(dict[str, object], scripts).get("test") if isinstance(scripts, dict) else None
    if not isinstance(script, str) or not script.strip() or NPM_DEFAULT in script:
        return None
    manager = next((m for lock, m in (("pnpm-lock.yaml", "pnpm"), ("yarn.lock", "yarn"), ("bun.lockb", "bun"),
                                      ("bun.lock", "bun")) if (root / lock).is_file()), "npm")
    suite = "bun run test" if manager == "bun" else f"{manager} test"
    return Runner(kind="node", suite=suite, each=suite if manager == "yarn" else f"{suite} --",
                  found="package.json scripts.test")


def _go(root: Path) -> Runner | None:
    if not (root / "go.mod").is_file():
        return None
    return Runner(kind="go", suite="go test ./...", each="go test", found="go.mod")


def _cargo(root: Path) -> Runner | None:
    if not (root / "Cargo.toml").is_file():
        return None
    return Runner(kind="cargo", suite="cargo test --offline", each="cargo test --offline", found="Cargo.toml")


def _make(root: Path) -> Runner | None:
    for name in ("Makefile", "GNUmakefile", "makefile"):
        path = root / name
        if path.is_file() and re.search(r"^test\s*:", path.read_text(errors="replace"), re.M):
            return Runner(kind="make", suite="make test", each="", found=f"{name} test target")
    return None


def _has(root: Path, name: re.Pattern[str]) -> bool:
    """Whether a file outside dependency folders and .git matches ``name``."""
    for _, dirs, files in os.walk(root):
        dirs[:] = [x for x in dirs if x not in DEPENDENCIES and x != ".git"]
        if any(name.match(f) for f in files):
            return True
    return False
