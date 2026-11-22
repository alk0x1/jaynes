"""No command in Jaynes's box starts the person's browser (the tech-lead's rule, 22/11): a check's commands, a
project's suite, an agent program in the box. Only Jaynes's own headless Chrome (JAYNES_CHROME), and the one Chrome a
photograph's box was made for, start. Stand-ins only: no app bundle is made and no browser of this Mac is ever started,
so a box that failed would run a stand-in, never open a window. Run from engine/:
    PYTHONPATH=python python3 -m unittest discover -s python/tests
"""

from __future__ import annotations

import os
import shlex
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

    def test_a_headless_chrome_outside_every_browser_starts_in_every_box_and_a_browser_named_inside_one_does_not(self):
        headless = self.tmp / "headless" / "chrome-headless-shell"
        headless.parent.mkdir()
        headless.write_text("#!/bin/sh\necho 'headless ran'\n")
        headless.chmod(0o755)
        # the tests' own headless shell, outside any browser's bundle
        code, said = self.run_in(box.Box((self.work,), local=True), headless, {"JAYNES_CHROME": str(headless)})
        self.assertEqual(code, 0, said)
        # the person's browser named in JAYNES_CHROME: a project's suite and a check's commands still start none of it
        named = {"JAYNES_CHROME": str(self.mine / "Contents/MacOS/browser")}
        for b in (box.Box((self.work,), local=True), box.Box((self.work,), local=True, any_port=True)):
            for program in (self.mine / "Contents/MacOS/browser", self.mine / "Contents/Frameworks/Helper/helper"):
                code, said = self.run_in(b, program, named)
                self.assertNotEqual(code, 0, f"{program}: {said}")
                self.assertNotIn("ran", said)

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
