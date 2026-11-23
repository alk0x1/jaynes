"""The agent's box reaches at least what the check's box reaches (the tech-lead's rule, 22/11): the real-user's three
runs on aa08fedc stopped at "impossible" on their first attempt, since in the agent's session ".venv/bin/python" said
"Operation not permitted", "node" was not on the PATH, and git said "unable to normalize alternate object path",
while the check ran the same suites. Here a project of each kind (Python with a uv virtualenv, node with npm, Go) has
its toolchain reached the way a person's is: through links inside a home the agent's box hides, and on a PATH that
only the run's start environment has. The agent's command and the check's command run the same suite, and both pass.
Stand-ins only: no model, no network, no browser, the person's own toolchains only read. Run from engine/:
    PYTHONPATH=python python3 -m unittest discover -s python/tests
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import jaynes_box as box  # noqa: E402

BOX_PY = Path(box.__file__).resolve()


def _which(name: str) -> Path | None:
    """A real program of this Mac, followed to its file, or None."""
    given = os.environ.get("JAYNES_TEST_NODE") if name == "node" else None
    found = given or shutil.which(name)
    if not found and shutil.which("mise"):
        out = subprocess.run(["mise", "which", name], capture_output=True, text=True)
        found = out.stdout.strip() if out.returncode == 0 else None
    return Path(os.path.realpath(found)) if found else None


@unittest.skipUnless(box.available(), "Jaynes's box is macOS's sandbox-exec")
class TheAgentReachesWhatTheCheckReaches(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="jaynes-same-reach-")).resolve()
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        self.home = self.tmp / "home"
        self.engine = self.home / ".jaynes" / "engine"
        self.run_dir = self.engine / "runs" / "r1"
        self.person = self.home / "Projects" / "app"
        self.person.mkdir(parents=True)
        self.tools = self.home / ".local" / "share"
        self.path = ["/usr/bin", "/bin"]  # the start environment's PATH, toolchains first (added below)
        self.reads: list[Path] = []  # what the engine names for the agent's box to read (copy.reads, toolchains)
        self.env: dict[str, str] = {}

    def toolchain(self, kind: str, version: str, root: Path) -> Path:
        """``root`` installed as a version manager does under the hidden home: a folder of its version, reached by a
        link of its short name (``kind/24`` to ``kind/24.1.0``), whose bin goes on the start PATH."""
        installs = self.tools / "mise" / "installs" / kind
        installs.mkdir(parents=True, exist_ok=True)
        (installs / f"{version}.1.0").symlink_to(root, target_is_directory=True)
        (installs / version).symlink_to(installs / f"{version}.1.0", target_is_directory=True)
        self.path.insert(0, str(installs / version / "bin"))
        # the engine's toolchains(): the installation followed to where it is
        self.reads.append(Path(os.path.realpath(installs / version)))
        return installs / version

    def copy(self) -> Path:
        """The run's copy of the person's project, a git clone sharing the run's objects as the engine lays it out
        (objects/info/alternates relative, to ../../../project.git/objects)."""
        bare = self.run_dir / "project.git"
        subprocess.run(["git", "init", "-q", "--bare", str(bare)], check=True)
        g = ["git", "-c", "user.name=t", "-c", "user.email=t@t.invalid", "-C", str(self.person)]
        subprocess.run(["git", "init", "-q", str(self.person)], check=True)
        (self.person / ".gitignore").write_text(".venv/\nnode_modules/\n")
        subprocess.run([*g, "add", "-A"], check=True)
        subprocess.run([*g, "commit", "-qm", "start"], check=True)
        subprocess.run([*g, "push", "-q", str(bare), "HEAD:refs/heads/main"], check=True)
        work = self.run_dir / "project"
        subprocess.run(["git", "clone", "-q", "--shared", str(bare), str(work)], check=True)
        (work / ".git" / "objects" / "info" / "alternates").write_text("../../../project.git/objects\n")
        self.reads.append(bare / "objects")
        for dep in (".venv", "node_modules"):
            if (self.person / dep).exists():
                (work / dep).symlink_to(self.person / dep, target_is_directory=True)
                self.reads.append(self.person / dep)
        (work / ".jaynes" / "tmp").mkdir(parents=True)
        return work

    def agent_run(self, work: Path, command: str) -> tuple[int, str]:
        """The command as Claude Code's Bash gives it to Jaynes: its shell's own short PATH, the caches the engine
        sets for the agent, and the box's spec as agent.rs writes it, with the run's start environment."""
        scratch = work / ".jaynes" / "tmp"
        spec = {"work": str(work), "writable": [str(work)], "readable": [str(p) for p in self.reads],
                "hidden": [], "home": str(self.home), "engine": str(self.engine), "local_ports": [],
                "test_servers": False, "any_port": False, "pwd": str(scratch / "pwd"),
                "env": {"PATH": os.pathsep.join(self.path), **self.env}}
        at = self.tmp / "spec.json"
        at.write_text(json.dumps(spec))
        env = {"HOME": str(self.home), "PATH": "/usr/bin:/bin", "TMPDIR": str(scratch),
               "GOCACHE": str(scratch / "go-cache"), "GOPATH": str(scratch / "go-path"),
               "npm_config_cache": str(scratch / "npm-cache"), "GOTOOLCHAIN": "local", "GOTELEMETRY": "off"}
        done = subprocess.run([sys.executable, str(BOX_PY), "agent-run", str(at), "--", command],
                              capture_output=True, text=True, timeout=120, cwd=work, env=env)
        return done.returncode, done.stdout + done.stderr

    def check_run(self, work: Path, command: str) -> tuple[int, str]:
        """The same command as a check runs it: from the engine, whose environment is the run's start environment."""
        was = {k: os.environ.get(k) for k in ("PATH", *self.env)}
        os.environ.update({"PATH": os.pathsep.join(self.path), **self.env})
        try:
            ran = box.run(work, command, timeout=120)
        finally:
            for k, v in was.items():
                if v is None:
                    os.environ.pop(k, None)
                else:
                    os.environ[k] = v
        return ran.code, ran.printed

    def both_pass(self, work: Path, command: str, says: str) -> None:
        agent, check = self.agent_run(work, command), self.check_run(work, command)
        self.assertEqual((check[0], says in check[1]), (0, True), f"the check: {check[1]}")
        self.assertEqual((agent[0], says in agent[1]), (0, True), f"the agent: {agent[1]}")

    def test_a_python_project_whose_virtualenv_links_to_uv_s_python_under_the_home(self):
        python = Path(os.path.realpath(sys.executable))
        # uv's way: cpython-3.X-... a link to cpython-3.X.Y-..., whose bin has the interpreter
        uv = self.tools / "uv" / "python"
        (uv / "cpython-3.x.9-macos" / "bin").mkdir(parents=True)
        (uv / "cpython-3.x.9-macos" / "bin" / "python3").symlink_to(python)
        (uv / "cpython-3.x-macos").symlink_to(uv / "cpython-3.x.9-macos", target_is_directory=True)
        (self.person / ".venv" / "bin").mkdir(parents=True)
        (self.person / ".venv" / "bin" / "python").symlink_to(uv / "cpython-3.x-macos" / "bin" / "python3")
        (self.person / "test_it.py").write_text("import unittest\n\nclass T(unittest.TestCase):\n"
                                                "    def test_it(self):\n        self.assertTrue(True)\n")
        work = self.copy()
        # copy.reads(): the virtualenv's interpreter, its link and where it leads, as the engine names them
        self.reads += [uv / "cpython-3.x-macos", uv / "cpython-3.x.9-macos", python.parent.parent]
        self.both_pass(work, ".venv/bin/python -m unittest -q test_it 2>&1", "OK")

    def test_a_node_project_run_by_npm_test_with_node_only_on_the_start_path(self):
        node = _which("node")
        if node is None or not (node.parent / "npm").exists():
            self.skipTest("no node with npm on this Mac")
        self.toolchain("node", "24", node.parent.parent)
        (self.person / "package.json").write_text(json.dumps({"name": "app", "version": "1.0.0",
                                                              "scripts": {"test": "node --test"}}))
        (self.person / "test").mkdir()
        (self.person / "test" / "it.test.js").write_text(
            "const test = require('node:test');\nconst assert = require('node:assert');\n"
            "test('it', () => assert.ok(true));\n")
        work = self.copy()
        self.both_pass(work, "npm test 2>&1", "pass 1")

    def test_a_go_project_with_go_only_on_the_start_path(self):
        go = _which("go")
        if go is None:
            self.skipTest("no go on this Mac")
        root = self.toolchain("go", "1", go.parent.parent)
        self.env["GOROOT"] = str(root)
        (self.person / "go.mod").write_text("module example.com/app\n\ngo 1.21\n")
        (self.person / "it_test.go").write_text('package app\n\nimport "testing"\n\nfunc TestIt(t *testing.T) {}\n')
        work = self.copy()
        self.both_pass(work, "go test ./... 2>&1", "ok  \texample.com/app")

    def test_git_reads_the_run_s_objects_through_the_copy_s_alternates(self):
        (self.person / "a.txt").write_text("a\n")
        work = self.copy()
        code, said = self.agent_run(work, "git show --stat HEAD 2>&1")
        self.assertEqual(code, 0, said)
        self.assertIn("a.txt", said)
        self.assertNotIn("unable to normalize", said)


if __name__ == "__main__":
    unittest.main()
