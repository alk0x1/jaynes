"""A run's project: a copy of the last commit per step, its changes as patches, and a branch at the end."""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

from jaynes import project


def git(cwd: Path, *args: str) -> str:
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    r = tmp_path / "repo"
    (r / "src").mkdir(parents=True)
    (r / "src" / "app.css").write_text(":root { --bg: #ffffff; }\n")
    (r / "README.md").write_text("hello\n")
    (r / ".gitignore").write_text("node_modules/\n")
    (r / "node_modules" / "dep").mkdir(parents=True)
    (r / "node_modules" / "dep" / "index.js").write_text("x\n")
    git(r, "init", "-q")
    git(r, "add", "-A")
    git(r, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "start")
    (r / "README.md").write_text("uncommitted edit\n")
    return r


def test_resolve_finds_the_root_and_the_last_commit(repo: Path) -> None:
    root, base = project.resolve(repo / "src")
    assert root == repo.resolve() and base == git(repo, "rev-parse", "HEAD")


def test_resolve_refuses_a_folder_outside_git_and_a_repository_without_commits(tmp_path: Path) -> None:
    with pytest.raises(project.ProjectError, match="not in a git repository"):
        project.resolve(tmp_path)
    git(tmp_path, "init", "-q")
    with pytest.raises(project.ProjectError, match="no commit"):
        project.resolve(tmp_path)


def test_a_copy_is_the_last_commit_with_earlier_patches_and_its_changes_become_a_patch(
        repo: Path, tmp_path: Path) -> None:
    root, base = project.resolve(repo)
    first = tmp_path / "w1" / "project"
    project.prepare(root, base, [], first, cache=tmp_path / "cache")
    assert (first / "README.md").read_text() == "hello\n"  # the commit, not the working tree
    assert not (first / "node_modules").exists() and not (first / ".git").exists()

    tracker = project.Tracker(tmp_path / "w1" / "project.git", first)
    (first / "src" / "app.css").write_text(":root { --bg: #101418; }\n")
    (first / "logo.svg").write_text("<svg/>\n")
    patch, changed = tracker.changes()
    assert changed == ["logo.svg", "src/app.css"]

    second = tmp_path / "w2" / "project"
    project.prepare(root, base, [patch], second, cache=tmp_path / "cache")
    assert (second / "src" / "app.css").read_text() == ":root { --bg: #101418; }\n"
    assert project.Tracker(tmp_path / "w2" / "project.git", second).changes() == (b"", [])


def test_the_branch_has_one_commit_per_patch_and_the_working_tree_is_untouched(repo: Path, tmp_path: Path) -> None:
    root, base = project.resolve(repo)
    copy = tmp_path / "w" / "project"
    project.prepare(root, base, [], copy, cache=tmp_path / "cache")
    tracker = project.Tracker(tmp_path / "w" / "project.git", copy)
    (copy / "src" / "app.css").write_text(":root { --bg: #101418; }\n")
    first, _ = tracker.changes()
    copy2 = tmp_path / "w2" / "project"
    project.prepare(root, base, [first], copy2, cache=tmp_path / "cache")
    tracker2 = project.Tracker(tmp_path / "w2" / "project.git", copy2)
    (copy2 / "logo.svg").write_text("<svg/>\n")
    second, _ = tracker2.changes()

    commit = project.branch(root, base, "jaynes/r1", [("apply palette", first), ("add logo", second)])

    assert git(repo, "rev-parse", "jaynes/r1") == commit
    assert git(repo, "log", "--format=%s", "jaynes/r1").splitlines() == ["add logo", "apply palette", "start"]
    assert git(repo, "show", "jaynes/r1:src/app.css") == ":root { --bg: #101418; }"
    assert git(repo, "rev-parse", "--abbrev-ref", "HEAD") in ("main", "master")
    assert (repo / "README.md").read_text() == "uncommitted edit\n"
    assert (repo / "src" / "app.css").read_text() == ":root { --bg: #ffffff; }\n"
    assert git(repo, "status", "--short") == "M README.md"


def test_paths_match_globs_with_double_stars() -> None:
    assert project.allowed(["src/app.css", "logo.svg"], ["**/*.css", "*.svg"]) == []
    assert project.allowed(["src/app.tsx", "a/b/c.css"], ["**/*.css"]) == ["src/app.tsx"]
    assert project.allowed(["anything.py"], []) == []  # no globs: any path
