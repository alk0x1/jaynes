"""A check is given the state its step left laid out beside the run's copy, with the packages a step installed in the
copy linked from there: its box reads those, and nothing a link in the copy leads to elsewhere. Run from engine/:
    PYTHONPATH=python python3 -m unittest discover -s python/tests
"""

from __future__ import annotations

import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import jaynes_box as box  # noqa: E402


def laid_out(base: Path) -> tuple[Path, Path, Path]:
    """A run's copy with a package installed in it and two links the agent made, one to a folder the box hides and
    one to the run's own steps; and the state laid out beside it as the engine lays it out. The laid-out state, the
    hidden folder and the copy."""
    home = base / "home"
    run = home / "engine" / "runs" / "r1"
    copy = run / "project"
    (copy / "node_modules").mkdir(parents=True)
    (copy / "node_modules" / "pkg.txt").write_text("installed\n")
    secret = home / ".ssh"
    secret.mkdir(parents=True)
    (secret / "key.txt").write_text("secret\n")
    (run / "steps" / "cases").mkdir(parents=True)
    (run / "steps" / "cases" / "hidden.txt").write_text("hidden case\n")
    (copy / "vendor").symlink_to(secret, target_is_directory=True)
    (copy / "bower_components").symlink_to(run / "steps" / "cases", target_is_directory=True)
    state = run / ".project-judged-1-0"
    state.mkdir()
    (state / "app.txt").write_text("app\n")
    # as the engine links them: where the copy's entry really is
    for name in ("node_modules", "vendor", "bower_components"):
        (state / name).symlink_to((copy / name).resolve(), target_is_directory=True)
    return state, secret, copy


class WhatACheckOfALaidOutStateReads(unittest.TestCase):
    def test_only_the_packages_inside_the_run_s_copy_are_added_to_what_the_box_reads(self):
        with tempfile.TemporaryDirectory() as tmp:
            state, _, copy = laid_out(Path(tmp))
            got = box.own_dependencies(state, ["node_modules", "vendor", "bower_components"])
            self.assertEqual(got, ((copy / "node_modules").resolve(),))

    def test_the_run_s_copy_itself_adds_nothing(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, _, copy = laid_out(Path(tmp))
            self.assertEqual(box.own_dependencies(copy, ["node_modules", "vendor", "bower_components"]), ())

    @unittest.skipUnless(box.available(), "needs the box for project commands")
    def test_the_box_reads_the_installed_packages_and_no_link_that_leads_out_of_the_copy(self):
        with tempfile.TemporaryDirectory() as tmp:
            state, secret, _ = laid_out(Path(tmp))
            command = ("cat node_modules/pkg.txt; "
                       "cat vendor/key.txt 2>/dev/null || echo vendor-denied; "
                       "cat bower_components/hidden.txt 2>/dev/null || echo steps-denied")
            home = Path(tmp) / "home"
            # the secret is hidden as ~/.ssh is; the run's own folders are hidden as the engine's home hides them
            with mock.patch.dict(os.environ, {"JAYNES_ENGINE_HOME": str(home / "engine")}):
                ran = box.run(state, command, hidden=(secret,), timeout=60)
            self.assertIn("installed", ran.printed)
            self.assertIn("vendor-denied", ran.printed)
            self.assertIn("steps-denied", ran.printed)
            self.assertNotIn("secret", ran.printed.replace("vendor-denied", ""))
            self.assertNotIn("hidden case", ran.printed)


if __name__ == "__main__":
    unittest.main()
