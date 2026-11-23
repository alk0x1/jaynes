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
        self.outer: dict[str, str] = {}  # what the person's environment also holds, for the agent's shell and the engine

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
        (objects/info/alternates relative, to ../../../project.git/objects). The branch is named: the git of the
        Command Line Tools names it main in its own gitconfig, the git of GitHub's macOS runner master."""
        bare = self.run_dir / "project.git"
        subprocess.run(["git", "init", "-q", "-b", "main", "--bare", str(bare)], check=True)
        g = ["git", "-c", "user.name=t", "-c", "user.email=t@t.invalid", "-C", str(self.person)]
        subprocess.run(["git", "init", "-q", "-b", "main", str(self.person)], check=True)
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
               "npm_config_cache": str(scratch / "npm-cache"), "GOTOOLCHAIN": "local", "GOTELEMETRY": "off",
               **self.outer}
        done = subprocess.run([sys.executable, str(BOX_PY), "agent-run", str(at), "--", command],
                              capture_output=True, text=True, timeout=120, cwd=work, env=env)
        return done.returncode, done.stdout + done.stderr

    def check_run(self, work: Path, command: str) -> tuple[int, str]:
        """The same command as a check runs it: from the engine, whose environment is the run's start environment."""
        was = {k: os.environ.get(k) for k in ("PATH", *self.env, *self.outer)}
        os.environ.update({"PATH": os.pathsep.join(self.path), **self.env, **self.outer})
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

    def project_browser(self, cache: Path, install: str, program: str) -> Path:
        """A browser as Playwright or Puppeteer downloads it into its cache under the home: a stand-in, never a real
        browser, whose program reads a data file of its own install as Chrome reads icudtl.dat."""
        exe = cache / install / program
        exe.parent.mkdir(parents=True)
        (exe.parent / "icudtl.dat").write_text("rendered by the project's own browser\n")
        exe.write_text('#!/bin/sh\ncat "$(dirname "$0")/icudtl.dat"\n')
        exe.chmod(0o755)
        return exe

    def test_the_project_s_own_browser_runs_in_the_agent_s_box_and_the_check_s(self):
        """A project whose tests drive a browser that Playwright or Puppeteer downloaded runs them in the agent's box
        as in the check's (the tech-lead, 23/11): the real-user's agents found the cache under the hidden home
        refused, and the headless shell said "icudtl.dat not found in bundle"."""
        self.outer["HOME"] = str(self.home)
        playwright = self.project_browser(self.home / "Library" / "Caches" / "ms-playwright",
                                          "chromium_headless_shell-1255", "chrome-headless-shell-mac-arm64/chrome-headless-shell")
        puppeteer = self.project_browser(self.home / ".cache" / "puppeteer",
                                         "chrome-headless-shell", "mac_arm-140.0.1/chrome-headless-shell-mac-arm64/chrome-headless-shell")
        work = self.copy()
        for exe in (playwright, puppeteer):
            command = f"'{exe}' --headless --dump-dom about:blank 2>&1"
            for who, (code, said) in (("the agent", self.agent_run(work, command)), ("the check", self.check_run(work, command))):
                self.assertEqual((code, "rendered by the project's own browser" in said), (0, True), f"{who}, {exe}: {said}")

    # where each browser and browser tool keeps a profile of the person's, with what macOS keeps of it beside: a
    # cookie, saved login or history file of each, as the browser names it
    PROFILES = {
        "Chrome": ["Library/Application Support/Google/Chrome/Default/Cookies",
                   "Library/Application Support/Google/Chrome/Default/Login Data",
                   "Library/Application Support/Google/Chrome/Default/History",
                   "Library/Caches/Google/Chrome/Default/Cache/Cache_Data/f_000001"],
        "Safari": ["Library/Safari/History.db", "Library/Cookies/Cookies.binarycookies",
                   "Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies",
                   "Library/HTTPStorages/com.apple.Safari/httpstorages.sqlite",
                   "Library/WebKit/com.apple.Safari/WebsiteData/LocalStorage/x", "Library/Caches/com.apple.Safari/Cache.db"],
        "Firefox": ["Library/Application Support/Firefox/Profiles/a1.default-release/cookies.sqlite",
                    "Library/Application Support/Firefox/Profiles/a1.default-release/logins.json",
                    "Library/Application Support/Firefox/Profiles/a1.default-release/places.sqlite",
                    "Library/Caches/Firefox/Profiles/a1.default-release/cache2/entries/A1"],
        "Arc": ["Library/Application Support/Arc/User Data/Default/Cookies",
                "Library/Caches/Arc/User Data/Default/Cache/x", "Library/Caches/company.thebrowser.Browser/Cache.db"],
        "Edge": ["Library/Application Support/Microsoft Edge/Default/Cookies",
                 "Library/Caches/Microsoft Edge/Default/Cache/x"],
        "Brave": ["Library/Application Support/BraveSoftware/Brave-Browser/Default/Cookies",
                  "Library/Caches/BraveSoftware/Brave-Browser/Default/Cache/x"],
        "Playwright": ["Library/Caches/ms-playwright/mcp-chrome-f5334aa/Default/Cookies",
                       "Library/Caches/ms-playwright/mcp-chrome-f5334aa/Default/Login Data",
                       "Library/Caches/ms-playwright/b/browser@04b564bc", ".cache/ms-playwright/mcp-chrome-1/Default/Cookies",
                       "pw-browsers/mcp-chrome-2/Default/History"],
        "Puppeteer": [".cache/puppeteer/profile/Default/Cookies", "puppeteer-cache/profile/Default/Login Data"],
    }

    def test_no_browser_profile_of_the_person_s_is_read_by_either_box_and_its_installed_program_is(self):
        """Neither the agent's box nor the check's reads a profile of a browser or browser tool of the person's
        (the tech-lead, 23/11), Playwright's MCP profile in its cache included, which the check's box read; the
        browsers installed beside them in the tools' caches are read and run, wherever the person named the cache."""
        self.outer.update({"HOME": str(self.home), "PLAYWRIGHT_BROWSERS_PATH": str(self.home / "pw-browsers"),
                           "PUPPETEER_CACHE_DIR": str(self.home / "puppeteer-cache")})
        for files in self.PROFILES.values():
            for f in files:
                (self.home / f).parent.mkdir(parents=True, exist_ok=True)
                (self.home / f).write_text("the person's session\n")
        installed = [self.project_browser(self.home / cache, install, program) for cache, install, program in (
            ("Library/Caches/ms-playwright", "chromium-1255", "chrome-mac/Chromium.app/Contents/MacOS/Chromium"),
            (".cache/ms-playwright", "firefox-1490", "firefox/Nightly.app/Contents/MacOS/firefox"),
            ("pw-browsers", "webkit-2182", "pw_run.sh"),
            (".cache/puppeteer", "chrome", "mac_arm-140.0.1/chrome-mac-arm64/chrome"),
            ("puppeteer-cache", "chrome-headless-shell", "mac_arm-140.0.1/chrome-headless-shell"))]
        work = self.copy()
        tried = "; ".join(f"[ -r '{self.home / f}' ] && echo 'READ {b}: {f}'" for b, files in self.PROFILES.items()
                          for f in files)
        listed = "; ".join(f"ls '{(self.home / f).parent}' >/dev/null 2>&1 && echo 'LISTED {b}: {f}'"
                           for b, files in self.PROFILES.items() for f in files)
        ran = "; ".join(f"'{exe}' --headless" for exe in installed)
        command = f"{tried}; {listed}; {ran}; true"
        for who, (_, said) in (("the agent", self.agent_run(work, command)), ("the check", self.check_run(work, command))):
            self.assertNotIn("READ ", said, f"{who}: {said}")
            self.assertNotIn("LISTED ", said, f"{who}: {said}")
            self.assertEqual(said.count("rendered by the project's own browser"), len(installed), f"{who}: {said}")

    def test_the_agent_s_git_reads_the_person_s_ignore_and_attributes_with_no_warning(self):
        """Every git status of the real-user's agents said "warning: unable to access '~/.config/git/ignore':
        Operation not permitted" (23/11): the engine names the person's ignore and attributes among the step's reads
        and in the agent's git configuration (person_git_lists in drive.rs), and /dev/null where there are none."""
        git_dir = self.home / ".config" / "git"
        git_dir.mkdir(parents=True)
        (git_dir / "ignore").write_text("*.log\n")
        (git_dir / "attributes").write_text("*.bin binary\n")
        (self.home / "my ignore").write_text("*.tmp\n")
        work = self.copy()
        for name in ("a.log", "b.tmp", "c.txt"):
            (work / name).write_text("x\n")
        config = work / ".jaynes" / "gitconfig"
        self.outer.update({"HOME": str(self.home), "GIT_CONFIG_GLOBAL": str(config), "GIT_CONFIG_NOSYSTEM": "1"})
        for excludes, says in ((self.home / "my ignore", "?? a.log"), (git_dir / "ignore", "?? b.tmp"), (None, "?? a.log")):
            config.write_text("[user]\n\tname = Jaynes agent\n\temail = agent@jaynes.invalid\n[core]\n"
                              f'\texcludesFile = "{excludes or "/dev/null"}"\n'
                              f'\tattributesFile = "{git_dir / "attributes" if excludes else "/dev/null"}"\n')
            self.reads = [r for r in self.reads if self.home / ".config" not in r.parents and r.name != "my ignore"]
            self.reads += [excludes, git_dir / "attributes"] if excludes else []
            code, said = self.agent_run(work, "git status --porcelain 2>&1; git check-attr diff x.bin")
            self.assertEqual(code, 0, said)
            self.assertNotIn("unable to access", said)
            self.assertIn(says, said)
            self.assertIn("?? c.txt", said)
            self.assertIn("x.bin: diff: unset" if excludes else "x.bin: diff: unspecified", said)

    def test_the_git_that_comes_with_the_mac_works_in_the_agent_s_box_and_the_check_s(self):
        """/usr/bin/git, the git of a Mac without Homebrew, is a shim that finds the real one through xcrun, and
        xcrun keeps a cache in the user's temporary folder (getconf DARWIN_USER_TEMP_DIR), whatever TMPDIR says:
        on GitHub's macOS runner, where it had none yet, the agent's box refused it "couldn't create cache file
        '/var/folders/.../T/xcrun_db-...' (errno=Operation not permitted)" (CI 45110746893). The tech-lead's rule
        (23/11): that git works in the agent's box and the check's, alike."""
        git = Path("/usr/bin/git")
        found = subprocess.run(["/usr/bin/xcrun", "--find", "git"], capture_output=True, text=True) \
            if git.exists() and Path("/usr/bin/xcrun").exists() else None
        if found is None or found.returncode != 0:
            self.skipTest("this Mac has no developer tools for /usr/bin/git")
        # a Mac whose xcrun has no cache yet, as the runner: xcrun's own variable names a new cache outside the
        # box, so this Mac's real cache is never read nor written
        outside = self.tmp / "user-temp"
        outside.mkdir()
        self.outer["xcrun_db"] = str(outside / "xcrun_db")
        (self.person / "a.txt").write_text("a\n")
        work = self.copy()
        command = ("/usr/bin/git init -q . && /usr/bin/git -c user.name=t -c user.email=t@t.invalid "
                   "commit -q --allow-empty -m it 2>&1 && /usr/bin/git log --oneline -1 2>&1")
        for who, (code, said) in (("the agent", self.agent_run(work, command)), ("the check", self.check_run(work, command))):
            self.assertEqual((code, "couldn't create cache file" in said, " it" in said), (0, False, True), f"{who}: {said}")
        self.assertFalse((outside / "xcrun_db").exists(), "xcrun wrote outside the box")


if __name__ == "__main__":
    unittest.main()
