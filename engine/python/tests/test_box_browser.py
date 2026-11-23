"""No command in Jaynes's box starts the person's browser (the tech-lead's rule, 22/11): a check's commands, a
project's suite, an agent program in the box. Only Jaynes's own headless Chrome (JAYNES_CHROME), and the one Chrome a
photograph's box was made for, start. Stand-ins only: no app bundle is made and no browser of this Mac is ever started,
so a box that failed would run a stand-in, never open a window. Run from engine/:
    PYTHONPATH=python python3 -m unittest discover -s python/tests
"""

from __future__ import annotations

import os
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import jaynes_box as box  # noqa: E402


def stand_in(root: Path, name: str) -> Path:
    """A folder laid out as a browser's bundle, without the .app that would make macOS take it for an app: the
    program, and a helper inside it, as Chrome starts its helpers from inside its bundle."""
    for part, said in (("Contents/MacOS/browser", f"{name} ran"), ("Contents/Frameworks/Helper/helper", f"{name}'s helper ran")):
        program = root / name / part
        program.parent.mkdir(parents=True, exist_ok=True)
        program.write_text(f'#!/bin/sh\necho "{said}"\n')
        program.chmod(0o755)
    return root / name


@unittest.skipUnless(box.available(), "Jaynes's box is macOS's sandbox-exec")
class TheBoxStartsNoBrowserOfThePerson(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="jaynes-browser-"))
        self.work = self.tmp / "work"
        self.work.mkdir()
        self.mine, self.other = stand_in(self.tmp, "Browser"), stand_in(self.tmp, "Other Browser")
        listed = [self.mine, self.other]
        patcher = mock.patch.object(box, "browsers", lambda home: listed)
        patcher.start()
        self.addCleanup(patcher.stop)

    def run_in(self, b: box.Box, program: Path | str, env: dict[str, str] | None = None) -> tuple[int, str]:
        """``program`` started in ``b``, or a shell line given as it is."""
        line = program if isinstance(program, str) else shlex.quote(str(program))
        with mock.patch.dict(os.environ, env or {}, clear=False):
            argv = box.command_in_box(line, b)
        out = subprocess.run(argv, capture_output=True, text=True, timeout=30, cwd=self.work)
        return out.returncode, out.stdout + out.stderr

    def test_a_check_s_commands_and_a_suite_s_start_no_browser_and_still_run_other_programs(self):
        for b in (box.Box((self.work,)), box.Box((self.work,), local=True), box.Box((self.work,), local=True, any_port=True)):
            for program in (self.mine / "Contents/MacOS/browser", self.mine / "Contents/Frameworks/Helper/helper",
                            self.other / "Contents/MacOS/browser"):
                code, said = self.run_in(b, program)
                self.assertNotEqual(code, 0, f"{program} ran in {b}: {said}")
                self.assertNotIn("ran", said)
                self.assertIn("not permitted", said.lower())
            code, said = self.run_in(b, Path("/bin/echo"))
            self.assertEqual(code, 0, said)

    def test_the_box_made_for_a_photograph_starts_its_one_browser_and_its_helpers_and_no_other(self):
        b = box.Box((self.work,), browser=self.mine / "Contents/MacOS/browser")
        self.assertEqual(self.run_in(b, self.mine / "Contents/MacOS/browser"), (0, "Browser ran\n"))
        self.assertEqual(self.run_in(b, self.mine / "Contents/Frameworks/Helper/helper"), (0, "Browser's helper ran\n"))
        code, said = self.run_in(b, self.other / "Contents/MacOS/browser")
        self.assertNotEqual(code, 0, said)

    def test_jaynes_chrome_starts_only_in_a_photograph_s_box_and_a_role_s_script_s_and_a_browser_named_inside_one_in_none_else(self):
        # A4 (the tech-lead's, 23/11): the Chrome Jaynes photographs with never runs where an agent's or a project's
        # commands run, only in the box made for it
        headless = self.tmp / "headless" / "chrome-headless-shell"
        headless.parent.mkdir()
        headless.write_text("#!/bin/sh\necho 'headless ran'\n")
        headless.chmod(0o755)
        named = {"JAYNES_CHROME": str(headless)}
        for b in (box.Box((self.work,)), box.Box((self.work,), local=True), box.Box((self.work,), local=True, any_port=True),
                  box.Box((self.work,), local=True, hide_home=True, home=self.tmp / "home")):
            code, said = self.run_in(b, headless, named)
            self.assertNotEqual(code, 0, f"{headless} ran in {b}: {said}")
            self.assertNotIn("ran", said)
        self.assertEqual(self.run_in(box.Box((self.work,), browser=headless), headless, named), (0, "headless ran\n"))
        with mock.patch.dict(os.environ, named):
            ran = box.run(self.work, shlex.quote(str(headless)), browser=True)
            self.assertEqual((ran.code, ran.printed), (0, "headless ran\n"))
            self.assertNotEqual(box.run(self.work, shlex.quote(str(headless))).code, 0)
        # the agent's box, from the spec agent.rs writes: denied by the name the engine gives, though the environment
        # of Claude Code's shell does not carry JAYNES_CHROME
        env = {k: v for k, v in os.environ.items() if k != "JAYNES_CHROME"}
        with mock.patch.dict(os.environ, env, clear=True):
            b, _, _, _ = box.agent_box({"work": str(self.work), "writable": [str(self.work)], "pwd": str(self.tmp / "pwd"),
                                        "home": str(self.tmp / "home"), "jaynes_chrome": str(headless)})
            code, said = self.run_in(b, headless)
        self.assertNotEqual(code, 0, said)
        self.assertNotIn("ran", said)
        # the person's browser named in JAYNES_CHROME: a project's suite and a check's commands still start none of it
        named = {"JAYNES_CHROME": str(self.mine / "Contents/MacOS/browser")}
        for b in (box.Box((self.work,), local=True), box.Box((self.work,), local=True, any_port=True)):
            for program in (self.mine / "Contents/MacOS/browser", self.mine / "Contents/Frameworks/Helper/helper"):
                code, said = self.run_in(b, program, named)
                self.assertNotEqual(code, 0, f"{program}: {said}")
                self.assertNotIn("ran", said)

    def test_jaynes_chrome_installed_by_playwright_is_also_the_project_s_browser_and_starts_where_the_project_s_do(self):
        # the headless shell a browser tool installed, as the board's rule 9 names it: the same program is the
        # project's own browser, which both boxes start (they read no profile of the tool's cache)
        home = self.tmp / "home"
        shell = home / "Library/Caches/ms-playwright/chromium_headless_shell-1255/chrome-headless-shell-mac-arm64/chrome-headless-shell"
        shell.parent.mkdir(parents=True)
        shell.write_text("#!/bin/sh\necho 'headless ran'\n")
        shell.chmod(0o755)
        for b in (box.Box((self.work,), local=True, home=home), box.Box((self.work,), local=True, hide_home=True, home=home)):
            self.assertEqual(self.run_in(b, shell, {"JAYNES_CHROME": str(shell)}), (0, "headless ran\n"), b)

    def test_a_role_s_own_scripts_start_the_chrome_jaynes_photographs_with_and_a_suite_starts_none(self):
        # a Mac without JAYNES_CHROME, its Chrome where macOS keeps apps (a stand-in here): a role's script check
        # draws its pages with it, as Helm's pages.py does, and the project's suite in the same box starts none
        installed = self.mine / "Contents/MacOS/browser"
        script = self.tmp / "pages.sh"
        script.write_text(f"#!/bin/sh\n{shlex.quote(str(installed))} && {shlex.quote(str(self.mine / 'Contents/Frameworks/Helper/helper'))}\n")
        script.chmod(0o755)
        env = {k: v for k, v in os.environ.items() if k != "JAYNES_CHROME"}
        with mock.patch.object(box, "CHROMES", (str(installed),)), mock.patch.dict(os.environ, env, clear=True):
            self.assertEqual(box.chrome(), str(installed))
            ran = box.run(self.work, shlex.quote(str(script)), browser=True)
            self.assertEqual((ran.code, ran.printed), (0, "Browser ran\nBrowser's helper ran\n"))
            ran = box.run(self.work, shlex.quote(str(script)))
            self.assertNotEqual(ran.code, 0, ran.printed)
            self.assertNotIn("ran", ran.printed)
            done = box.run_step(shlex.quote(str(script)), project=None)
            self.assertEqual((done.ran.code, done.ran.printed), (0, "Browser ran\nBrowser's helper ran\n"))

    def test_jaynes_chrome_wins_over_the_installed_chrome_in_a_role_s_scripts_and_a_photograph(self):
        installed, named = self.mine / "Contents/MacOS/browser", self.other / "Contents/MacOS/browser"
        with mock.patch.object(box, "CHROMES", (str(installed),)), mock.patch.dict(os.environ, {"JAYNES_CHROME": str(named)}):
            self.assertEqual(box.chrome(), str(named))
            self.assertEqual(box.run(self.work, shlex.quote(str(named)), browser=True).code, 0)
            ran = box.run(self.work, shlex.quote(str(installed)), browser=True)
            self.assertNotEqual(ran.code, 0, ran.printed)
            self.assertNotEqual(box.run_step(shlex.quote(str(installed)), project=None).ran.code, 0)

    def test_a_box_that_starts_only_its_agent_program_starts_no_headless_chrome_either(self):
        agent = self.tmp / "agent"
        agent.write_text("#!/bin/sh\necho 'agent ran'\n")
        agent.chmod(0o755)
        headless = self.tmp / "chrome-headless-shell"
        headless.write_text("#!/bin/sh\necho 'headless ran'\n")
        headless.chmod(0o755)
        b = box.Box((self.work,), runs=(*box.programs_of(Path("/bin/sh")), *box.programs_of(agent)))
        code, said = self.run_in(b, headless, {"JAYNES_CHROME": str(headless)})
        self.assertNotEqual(code, 0, said)

    def test_the_box_reads_no_browser_profile_of_the_person(self):
        home = self.tmp / "home"
        profiles = ["Library/Application Support/Google/Chrome", "Library/Application Support/Chromium",
                    "Library/Application Support/Firefox", "Library/Safari", "Library/Containers/com.apple.Safari",
                    "Library/Application Support/BraveSoftware", "Library/Application Support/Microsoft Edge",
                    "Library/Application Support/Arc", "Library/Application Support/Google/Chrome Beta",
                    "Library/Application Support/Google/Chrome Canary", "Library/Application Support/com.operasoftware.Opera",
                    "Library/Application Support/Vivaldi", "Library/Application Support/zen",
                    "Library/Application Support/librewolf", "Library/Application Support/Waterfox",
                    "Library/Application Support/Floorp", "Library/Application Support/Orion",
                    "Library/Application Support/Thorium"]
        for p in profiles:
            (home / p).mkdir(parents=True)
            (home / p / "Cookies").write_text("STAND-IN")
        for p in profiles:
            code, said = self.run_in(box.Box((self.work,), local=True, home=home), f"/bin/cat {shlex.quote(str(home / p / 'Cookies'))}")
            self.assertNotEqual(code, 0, f"{p}: {said}")
            self.assertNotIn("STAND-IN", said)

    def test_no_app_is_opened_nor_sent_apple_events_as_the_profile_is_written(self):
        # never tried for real, which would open an app on the person's screen if the box failed
        for b in (box.Box((self.work,)), box.Box((self.work,), local=True, any_port=True)):
            lines = box.profile(b).splitlines()
            for rule in ("(deny lsopen)", "(deny appleevent-send)"):
                self.assertIn(rule, lines)


@unittest.skipUnless(box.available(), "Jaynes's box is macOS's sandbox-exec")
class ARoleSScriptCheckStillDrawsItsPages(unittest.TestCase):
    def test_a_script_check_of_a_role_starts_the_installed_chrome_on_a_mac_without_jaynes_chrome(self):
        import jaynes_menu as menu
        tmp = Path(tempfile.mkdtemp(prefix="jaynes-browser-check-"))
        bundle = stand_in(tmp, "Browser")
        project = tmp / "project"
        project.mkdir()
        check = tmp / "role" / "checks" / "pages.sh"
        check.parent.mkdir(parents=True)
        check.write_text(f"#!/bin/sh\n{shlex.quote(str(bundle / 'Contents/MacOS/browser'))} || exit 3\n")
        check.chmod(0o755)
        env = {k: v for k, v in os.environ.items() if k != "JAYNES_CHROME"}
        with mock.patch.object(box, "browsers", lambda home: [bundle]), \
                mock.patch.object(box, "CHROMES", (str(bundle / "Contents/MacOS/browser"),)), \
                mock.patch.dict(os.environ, env, clear=True):
            self.assertIsInstance(menu.script({"project": str(project)}, {"path": str(check)}), menu.Pass)

    def test_a_check_whose_command_starts_a_browser_says_the_box_kept_it_from_starting_and_nothing_of_ports(self):
        # the refusal was read as one of ports, and the card offered to open the project's ports, which starts no browser
        import jaynes_menu as menu
        tmp = Path(tempfile.mkdtemp(prefix="jaynes-browser-said-"))
        bundle = stand_in(tmp, "Browser")
        project = tmp / "project"
        project.mkdir()
        program = shlex.quote(str(bundle / "Contents/MacOS/browser"))
        env = {k: v for k, v in os.environ.items() if k != "JAYNES_CHROME"}
        with mock.patch.object(box, "browsers", lambda home: [bundle]), mock.patch.dict(os.environ, env, clear=True):
            said = menu._box({"project": str(project)}, f"{program} --headless https://example.com")
        self.assertIsInstance(said, menu.Blocked)
        self.assertIn("the box refused what the command tried, so its failure says nothing about the project", said.reason)
        self.assertIn("It tried to start a browser of this Mac, which no check's command starts:", said.reason)
        self.assertIn(str(bundle / "Contents/MacOS/browser"), said.reason)
        for ports in ("JAYNES_PORTS", "open-ports", "local port"):
            self.assertNotIn(ports, said.reason)


NODE = os.environ.get("JAYNES_TEST_NODE") or shutil.which("node")
# A program run by node that starts the browser it is given against a test server of its own, as a project's browser
# test does: Node says a refused start as "Error: spawn EPERM" and names the program only in the lines under it.
SPAWNS = """const CHROME = '%s';
const http = require('http'), { spawn } = require('child_process'), fs = require('fs'), os = require('os');
const srv = http.createServer((q, r) => r.end('<html><body>page-of-the-test</body></html>'));
srv.listen(0, '127.0.0.1', () => {
  const profile = fs.mkdtempSync(require('path').join(os.tmpdir(), 'profile-'));
  const p = spawn(process.argv[2] || CHROME, [...process.argv.slice(3), '--headless', '--no-first-run', `--user-data-dir=${profile}`,
                  '--dump-dom', `http://127.0.0.1:${srv.address().port}/`]);
  let out = '';
  p.stdout.on('data', d => out += d); p.stderr.on('data', d => process.stderr.write(d));
  p.on('error', e => { console.error(e); process.exit(2); });
  p.on('close', () => { srv.close(); console.log(out.includes('page-of-the-test') ? 'THE PAGE OPENED' : 'NO PAGE'); process.exit(out.includes('page-of-the-test') ? 0 : 1); });
});
"""


def installed_headless_shell() -> Path | None:
    """The headless shell Playwright installed for the person, a project's own browser: the newest, never an app."""
    found = sorted(c for cache in box.browser_caches(Path.home()) for c in cache.glob("chromium_headless_shell-*/chrome-headless-shell-*/chrome-headless-shell"))
    return found[-1] if found else None


@unittest.skipUnless(box.available() and NODE, "Jaynes's box is macOS's sandbox-exec, and the suite is node's")
class ABrowserASuiteStarts(unittest.TestCase):
    """The real user's of 23/11 (T8): a suite's browser test was refused with "spawn EPERM", and the card offered the
    ports, which start no browser."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="jaynes-browser-suite-")).resolve()
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        self.project = self.tmp / "project"
        (self.project / ".jaynes" / "tmp").mkdir(parents=True)
        (self.project / "browser.test.js").write_text(SPAWNS % "")

    def check(self, *program: Path | str) -> object:
        import jaynes_menu as menu
        line = " ".join(shlex.quote(str(a)) for a in (NODE, "browser.test.js", *program))
        return menu._box({"project": str(self.project), "test_servers": True}, line)

    def agent(self, program: Path, *flags: str) -> tuple[int, str]:
        """The same command in the agent's box, as agent.rs gives it, with the person's home hidden."""
        import json
        scratch = self.project / ".jaynes" / "tmp"
        line = " ".join(shlex.quote(str(a)) for a in (NODE, "browser.test.js", program, *flags))
        spec = {"work": str(self.project), "writable": [str(self.project)],
                "readable": [os.path.realpath(Path(NODE).parent.parent)], "hidden": [], "home": str(Path.home()),
                "engine": str(self.tmp / "engine"), "local_ports": [], "test_servers": True, "any_port": False,
                "pwd": str(scratch / "pwd"), "env": {"PATH": f"{Path(NODE).parent}:/usr/bin:/bin"}}
        (self.tmp / "spec.json").write_text(json.dumps(spec))
        done = subprocess.run([sys.executable, box.__file__, "agent-run", str(self.tmp / "spec.json"), "--", line],
                              capture_output=True, text=True, timeout=120, cwd=self.project,
                              env={"HOME": str(Path.home()), "PATH": "/usr/bin:/bin", "TMPDIR": str(scratch)})
        return done.returncode, done.stdout + done.stderr

    def test_the_browser_playwright_installed_opens_a_page_of_the_suite_s_own_server_in_the_check_s_box_and_the_agent_s(self):
        shell = installed_headless_shell()
        if not shell:
            self.skipTest("no headless shell of Playwright's installed on this Mac")
        # as Playwright starts it: a profile of its own, and without the browser's own sandbox, which does not start
        # inside another
        said = self.check(shell, "--no-sandbox")
        self.assertIsInstance(said, box.Ran, getattr(said, "reason", said))
        self.assertEqual((said.code, "THE PAGE OPENED" in said.printed), (0, True), said.printed[-1500:])
        code, printed = self.agent(shell, "--no-sandbox")
        self.assertEqual((code, "THE PAGE OPENED" in printed), (0, True), printed[-1500:])

    def test_a_browser_started_with_its_own_sandbox_says_to_start_it_without_and_nothing_of_ports(self):
        # what Chrome prints when its own sandbox cannot start inside the box (measured with Playwright's headless
        # shell and Chromium, 23/11), from a stand-in: no browser starts
        chrome = self.tmp / "chrome"
        chrome.write_text("#!/bin/sh\necho 'sandbox initialization failed: Operation not permitted' >&2\n"
                          "echo 'FATAL:gpu_data_manager_impl_private.cc(387)] GPU process isn'\\''t usable. Goodbye.' >&2\nexit 1\n")
        chrome.chmod(0o755)
        said = self.check(chrome)
        self.assertIsInstance(said, menu_blocked())
        self.assertIn("the box refused what the command tried, so its failure says nothing about the project", said.reason)
        self.assertIn("It started a browser with its own sandbox, which does not start inside the box:", said.reason)
        self.assertIn("--no-sandbox", said.reason)
        for ports in ("JAYNES_PORTS", "open-ports", "local port"):
            self.assertNotIn(ports, said.reason)

    def test_a_suite_that_starts_a_browser_of_the_person_s_by_node_says_so_and_nothing_of_ports(self):
        # as the real user's agent wrote it: the browser's program in the test's file, and Node's refusal names none
        bundle = stand_in(self.tmp, "Browser")
        program = bundle / "Contents/MacOS/browser"
        (self.project / "browser.test.js").write_text(SPAWNS % program)
        env = {k: v for k, v in os.environ.items() if k != "JAYNES_CHROME"}
        with mock.patch.object(box, "browsers", lambda home: [bundle]), mock.patch.dict(os.environ, env, clear=True):
            said = self.check()
        self.assertIsInstance(said, menu_blocked())
        self.assertIn("Error: spawn EPERM", said.reason)
        self.assertIn(f"It tried to start a browser of this Mac, which no check's command starts: {program}.", said.reason)
        self.assertIn("A browser the project installs runs in the box, as Playwright's and Puppeteer's do.", said.reason)
        for ports in ("JAYNES_PORTS", "open-ports", "local port"):
            self.assertNotIn(ports, said.reason)


def menu_blocked() -> type:
    import jaynes_menu as menu
    return menu.Blocked


class ARoleSProgramStartsNoBrowserOfThePerson(unittest.TestCase):
    def test_a_role_that_declares_a_browser_s_program_as_a_command_is_refused_before_it_runs(self):
        tmp = Path(tempfile.mkdtemp(prefix="jaynes-browser-role-"))
        bundle = stand_in(tmp, "Browser")
        with mock.patch.object(box, "browsers", lambda home: [bundle]):
            for command in (bundle / "Contents/MacOS/browser", bundle / "Contents/Frameworks/Helper/helper"):
                with self.assertRaises(box.Refused) as said:
                    box.program_needs(Path(sys.executable), [command], home=tmp / "home")
                self.assertIn("browser", str(said.exception))
            # any other program it declares is worked out as before
            self.assertTrue(box.program_needs(Path(sys.executable), [Path("/bin/echo")], home=tmp / "home").run)


class TheBrowsersListed(unittest.TestCase):
    def test_the_browsers_are_the_ones_macos_keeps_for_everyone_and_for_the_person(self):
        home = Path("/Users/someone")
        listed = [str(p) for p in box.browsers(home)]
        for p in ("/Applications/Google Chrome.app", "/Applications/Chromium.app", "/Applications/Firefox.app",
                  "/Applications/Safari.app", "/System/Cryptexes/App/System/Applications/Safari.app",
                  "/System/Volumes/Preboot/Cryptexes/App/System/Applications/Safari.app",
                  "/Users/someone/Applications/Google Chrome.app", "/Applications/Brave Browser.app",
                  "/Applications/Zen.app", "/Applications/Zen Browser.app", "/Applications/Orion.app",
                  "/Applications/LibreWolf.app", "/Applications/Waterfox.app", "/Applications/Floorp.app",
                  "/Applications/Thorium.app"):
            self.assertIn(p, listed)


if __name__ == "__main__":
    unittest.main()
