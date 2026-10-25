"""A run's project: the git repository a run works on, without ever touching its working tree.

A run records the repository root and its last commit (the base). A step that reads or changes the project
gets a copy of the base with the patches of earlier steps applied. A private git directory beside the
workspace, out of the agent's reach, turns what the step changed into a patch. When the run finishes, the
patches become one commit each on a branch built with a temporary index, so the person's checkout, index
and uncommitted work stay as they were.
"""

from __future__ import annotations

import io
import os
import shutil
import subprocess
import tarfile
import tempfile
from collections.abc import Sequence
from fnmatch import fnmatchcase
from pathlib import Path

# Never let a person's git config (hooks, signing, templates) act on Jaynes's own git calls.
SAFE = ["-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false", "-c", "init.templateDir=",
        "-c", "user.name=Jaynes", "-c", "user.email=jaynes@localhost", "-c", "core.autocrlf=false"]


class ProjectError(Exception):
    pass


def _git(*args: str, cwd: Path | None = None, env: dict[str, str] | None = None, data: bytes | None = None) -> bytes:
    done = subprocess.run(["git", *SAFE, *args], cwd=cwd, input=data, capture_output=True,
                          env={**os.environ, **(env or {})})
    if done.returncode != 0:
        raise ProjectError(f"git {' '.join(args[:2])}: {done.stderr.decode(errors='replace').strip()[:500]}")
    return done.stdout


def resolve(path: Path) -> tuple[Path, str]:
    """The repository root that holds ``path`` and its last commit."""
    try:
        root = Path(_git("rev-parse", "--show-toplevel", cwd=path).decode().strip())
    except (ProjectError, FileNotFoundError, NotADirectoryError):
        raise ProjectError(f"{path} is not in a git repository. Run git init and commit, then start again.") from None
    try:
        base = _git("rev-parse", "--verify", "HEAD", cwd=root).decode().strip()
    except ProjectError:
        raise ProjectError(f"{root} has no commit yet. Commit once, then start again.") from None
    return root.resolve(), base


def prepare(root: Path, base: str, patches: Sequence[bytes], dest: Path, *, cache: Path) -> None:
    """Fill ``dest`` with the files of ``base``, then apply ``patches`` in order. ``cache`` keeps the base."""
    if not cache.exists():
        tmp = Path(tempfile.mkdtemp(prefix=".base-", dir=cache.parent if cache.parent.exists() else None))
        cache.parent.mkdir(parents=True, exist_ok=True)
        archive = _git("archive", "--format=tar", base, cwd=root)
        with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
            tar.extractall(tmp, filter="data")
        try:
            os.replace(tmp, cache)
        except OSError:  # another process filled it first
            shutil.rmtree(tmp, ignore_errors=True)
    shutil.copytree(cache, dest, symlinks=True)
    for patch in patches:
        if patch:  # outside any repository, so git applies paths from dest and nothing above it
            _git("apply", "--binary", "--whitespace=nowarn", "-", cwd=dest, data=patch,
                 env={"GIT_CEILING_DIRECTORIES": str(dest.parent)})


class Tracker:
    """Records what a step changed in its copy, from a git directory the agent cannot reach."""

    def __init__(self, git_dir: Path, work: Path) -> None:
        self.env = {"GIT_DIR": str(git_dir), "GIT_WORK_TREE": str(work)}
        self.work = work
        _git("init", "-q", cwd=work, env=self.env)
        _git("add", "-A", cwd=work, env=self.env)
        _git("commit", "-q", "--allow-empty", "--no-verify", "-m", "before", cwd=work, env=self.env)

    def changes(self) -> tuple[bytes, list[str]]:
        """The patch from the copy as handed over to the copy now, and the paths it touches."""
        _git("add", "-A", cwd=self.work, env=self.env)
        patch = _git("diff", "--cached", "--binary", "--no-renames", "HEAD", cwd=self.work, env=self.env)
        names = _git("diff", "--cached", "--name-only", "--no-renames", "HEAD", cwd=self.work, env=self.env)
        return patch, sorted(n for n in names.decode().splitlines() if n)


def branch(root: Path, base: str, name: str, patches: Sequence[tuple[str, bytes]]) -> str:
    """Commit each patch on ``name`` starting from ``base``, with a temporary index. Returns the last commit."""
    with tempfile.TemporaryDirectory() as tmp:
        env = {"GIT_INDEX_FILE": str(Path(tmp) / "index")}
        _git("read-tree", base, cwd=root, env=env)
        parent = base
        for message, patch in patches:
            if patch:
                _git("apply", "--cached", "--binary", "--whitespace=nowarn", "-", cwd=root, env=env, data=patch)
            tree = _git("write-tree", cwd=root, env=env).decode().strip()
            parent = _git("commit-tree", tree, "-p", parent, "-m", message, cwd=root, env=env).decode().strip()
        _git("update-ref", f"refs/heads/{name}", parent, cwd=root)
    return parent


# Paths that trusted tools run or obey later, outside any box: agents' and editors' settings and instructions, git
# attributes and hooks, direnv, interpreters and package scripts, CI. ``may_change`` alone never lets them through.
FLOOR = ("**/.claude/**", "**/.codex/**", "**/.cursor/**", "**/.gemini/**", "**/.vscode/**", "**/.idea/**",
         "**/.zed/**", "**/.mcp.json", "**/CLAUDE.md", "**/AGENTS.md", "**/.cursorrules",
         "**/.git*", "**/.husky/**", "**/.pre-commit-config.yaml", "**/.envrc",
         "**/.venv/**", "**/venv/**", "**/package.json", "**/.npmrc", "**/.yarnrc*", "**/pyproject.toml",
         "**/setup.py", "**/setup.cfg", ".gitlab-ci.yml", ".circleci/**", ".buildkite/**", "Jenkinsfile",
         "azure-pipelines.yml", ".travis.yml")


def _matches(path: str, glob: str) -> bool:
    return fnmatchcase(path, glob) or (glob.startswith("**/") and fnmatchcase(path, glob[3:]))


def allowed(paths: Sequence[str], globs: Sequence[str]) -> list[str]:
    """The paths no glob matches, so with no globs every path. ``**/`` also matches the top level."""
    return [p for p in paths if not any(_matches(p, g) for g in globs)]


def floor(paths: Sequence[str]) -> list[str]:
    """The paths on the floor, in any case, since macOS and Windows file systems ignore it."""
    return [p for p in paths if any(_matches(p.lower(), g.lower()) for g in FLOOR)]

