"""A photograph's box and a role's own script's bind a port only on the ports they were given (A8, the tech-lead's of
23/11): their profile denied the network going out but not the binding, so a script or a page's script listened on any
port, reachable by the network, while the page said they "take a port they are allowed". Stand-ins only: a Python
that binds and nothing else, no browser, no model. Run from engine/:
    PYTHONPATH=python python3 -m unittest discover -s python/tests
"""

from __future__ import annotations

import shlex
import shutil
import socket
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import jaynes_box as box  # noqa: E402

BIND = ("import socket,sys\n"
        "s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)\n"
        "try:\n"
        "    s.bind((sys.argv[1], int(sys.argv[2]))); print('bound')\n"
        "except OSError as e:\n"
        "    print('refused', e.errno)\n")


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


@unittest.skipUnless(box.available(), "Jaynes's box is macOS's sandbox-exec")
class ABoxMadeForAPhotographOrAScriptBindsOnlyItsPorts(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="jaynes-bind-")).resolve()
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        (self.tmp / "bind.py").write_text(BIND)

    def bind(self, address: str, port: int, **given) -> str:
        argv = box.command_in_box(f"/usr/bin/python3 {shlex.quote(str(self.tmp / 'bind.py'))} {address} {port}",
                                  box.Box((self.tmp,), **given))
        done = subprocess.run(argv, capture_output=True, text=True, timeout=60)
        return done.stdout.strip() or done.stderr.strip()

    def test_a_port_that_was_not_given_is_refused_on_every_address(self):
        given = free_port()
        for address in ("127.0.0.1", "0.0.0.0", "::"):
            self.assertTrue(self.bind(address, free_port(), ports=(given,), bind_given_only=True).startswith("refused"),
                            address)
        self.assertTrue(self.bind("127.0.0.1", 0, ports=(given,), bind_given_only=True).startswith("refused"))

    def test_a_port_that_was_given_binds_on_the_loopback(self):
        given = free_port()
        self.assertEqual(self.bind("127.0.0.1", given, ports=(given,), bind_given_only=True), "bound")

    def test_the_page_s_port_is_never_bound_even_when_given(self):
        # the page's port is a free one chosen now, so that the bind fails only for the box's rule, on any machine
        page = free_port()
        with mock.patch.object(box, "PAGE_PORT", page):
            self.assertTrue(self.bind("127.0.0.1", page, ports=(page,), bind_given_only=True).startswith("refused"))

    def test_the_boxes_of_a_photograph_and_of_a_role_s_script_ask_for_it(self):
        source = Path(box.__file__).read_text()
        self.assertEqual(source.count("bind_given_only=True"), 3, "photograph, its height, a role's script")


if __name__ == "__main__":
    unittest.main()
