"""A photograph is taken in the light theme, whatever the Mac's appearance, and Jaynes says so (the
roles-engineer's queue, item 3: the Design Director read a board photographed dark as a dark design). A headless
Chrome that is the full browser may follow the system's appearance; the photograph names the theme instead, and the
roles that show photographs tell their agents it is the light one. Stand-ins
only: the Chrome here is a script that writes the arguments it was given as its "screenshot", so no browser of this
Mac starts. Run from engine/:
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

# Blink's preferred colour scheme: 0 is dark, 1 is light (seen with Playwright's headless shell, 23/11)
LIGHT = "--blink-settings=preferredColorScheme=1"


@unittest.skipUnless(box.available(), "Jaynes's box is macOS's sandbox-exec")
class APhotographIsTakenInTheLightTheme(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="jaynes-theme-"))
        program = self.tmp / "Chrome" / "Contents/MacOS/browser"
        program.parent.mkdir(parents=True)
        # the stand-in writes every argument it got, one a line, where --screenshot asks for the picture
        program.write_text('#!/bin/sh\nfor a in "$@"; do case $a in --screenshot=*) out=${a#--screenshot=};; esac; done\n'
                           'printf "%s\\n" "$@" > "$out"\n')
        program.chmod(0o755)
        self.chrome = program
        self.page = self.tmp / "board.html"
        self.page.write_text("<html><body>board</body></html>")

    def test_the_photograph_asks_chrome_for_the_light_theme(self):
        out = self.tmp / "board.png"
        with mock.patch.dict(os.environ, {"JAYNES_CHROME": str(self.chrome)}):
            self.assertIsNone(box.photograph(str(self.page), out))
        self.assertIn(LIGHT, out.read_text().splitlines())


if __name__ == "__main__":
    unittest.main()
