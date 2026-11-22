"""Lock 1 of the estoque (22/11): a suite's own test server, on a port macOS gave after the box's window, reached
through a library call whose source binds (``socket.create_connection``), so the refusal names no port where it is
printed. The port is in the frame's arguments, as pytest prints them; the card says the window, never "local port 0,
a port of its own choosing". Run from engine/:
    PYTHONPATH=python python3 -m unittest discover -s python/tests
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import jaynes_box as box  # noqa: E402
import jaynes_menu as menu  # noqa: E402
from jaynes_role import Blocked  # noqa: E402

# what pytest printed for the estoque's test_ping_responde (made 22/11 with its own test file and pytest 3.14, the
# connect refused as the box refuses it; the interpreter's folder shortened): the fixture's server bound port 0, and
# create_connection reached it at 61593
PYTEST = '______________________________ test_ping_responde ______________________________\n\nbanco_falso_porta_livre = (\'127.0.0.1\', 61593)\n\n    def test_ping_responde(banco_falso_porta_livre):\n        host, port = banco_falso_porta_livre\n>       assert db.ping(host, port) is True\n               ^^^^^^^^^^^^^^^^^^^\n\ntests/test_db.py:40: \n_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ \npkg/db.py:37: in ping\n    with socket.create_connection((host, port), timeout=timeout) as s:\n         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n/usr/lib/python3.14/socket.py:874: in create_connection\n    raise exceptions[0]\n_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ \n\naddress = (\'127.0.0.1\', 61593), timeout = 2.0, source_address = None\nall_errors = False\n\n    def create_connection(address, timeout=_GLOBAL_DEFAULT_TIMEOUT,\n                          source_address=None, *, all_errors=False):\n        """Connect to *address* and return the socket object.\n    \n        Convenience function.  Connect to *address* (a 2-tuple ``(host,\n        port)``) and return the socket object.  Passing the optional\n        *timeout* parameter will set the timeout on the socket instance\n        before attempting to connect.  If no *timeout* is supplied, the\n        global default timeout setting returned by :func:`getdefaulttimeout`\n        is used.  If *source_address* is set it must be a tuple of (host, port)\n        for the socket to bind as a source address before making the connection.\n        A host of \'\' or port 0 tells the OS to use the default. When a connection\n        cannot be created, raises the last error if *all_errors* is False,\n        and an ExceptionGroup of all errors if *all_errors* is True.\n        """\n    \n        host, port = address\n        exceptions = []\n        for res in getaddrinfo(host, port, 0, SOCK_STREAM):\n            af, socktype, proto, canonname, sa = res\n            sock = None\n            try:\n                sock = socket(af, socktype, proto)\n                if timeout is not _GLOBAL_DEFAULT_TIMEOUT:\n                    sock.settimeout(timeout)\n                if source_address:\n                    sock.bind(source_address)\n>               sock.connect(sa)\nE               PermissionError: [Errno 1] Operation not permitted\n\n/usr/lib/python3.14/socket.py:859: PermissionError\n=========================== short test summary info ============================\nFAILED tests/test_db.py::test_ping_responde - PermissionError: [Errno 1] Oper...\n1 failed, 11 deselected in 0.02s\n'

# the 4,000 ports macOS gave next as the command began, the server's port after them
WINDOW = tuple(range(57593, 61593))
SYSTEM = (49152, 65535, False)


def ran(printed: str, **kw) -> box.Ran:
    return box.Ran(1, printed, printed, reserved=(56903, 56904), test_servers=True, window=WINDOW, **kw)


class WhatTheRefusalNamed(unittest.TestCase):
    def test_the_port_of_a_refused_connect_is_read_from_its_frame_s_arguments(self):
        self.assertEqual(box.ports_refused(ran(PYTEST)), ["61593"])

    def test_a_bind_in_the_library_s_source_above_the_refused_connect_is_no_bind_of_port_0(self):
        # the frame's arguments not printed (pytest --showlocals off and no arguments): no port, never "0"
        bare = "\n".join(line for line in PYTEST.splitlines() if not line.startswith(("address = ", "all_errors = ")))
        self.assertEqual(box.ports_refused(ran(bare)), [])

    def test_a_refused_bind_of_port_0_is_still_port_0(self):
        printed = ("    def test_serves():\n        srv = socket.socket()\n>       srv.bind((HOST, 0))\n"
                   "E       PermissionError: [Errno 1] Operation not permitted\n")
        self.assertEqual(box.ports_refused(ran(printed)), ["0"])

    def test_the_port_past_the_window_is_found(self):
        with mock.patch.object(box, "_system_ports", return_value=SYSTEM):
            self.assertEqual(box.past_window(ran(PYTEST)), [61593])


class TheCheckSaysTheWindow(unittest.TestCase):
    def test_the_check_says_macos_gave_the_port_after_the_window_never_a_port_of_its_own_choosing(self):
        call = {"project": "/Users/x/Projects/estoque", "source": "/Users/x/Projects/estoque", "test_servers": True}
        with mock.patch.object(box, "run", return_value=ran(PYTEST)), \
                mock.patch.object(box, "_system_ports", return_value=SYSTEM):
            said = menu._box(call, "uv run pytest")
        self.assertIsInstance(said, Blocked)
        self.assertIn(" It tried port 61593, which macOS gave after the 4,000 the box let the tests reach (57593 to 61592).",
                      said.reason)
        for wrong in ("own choosing", "did not help", "JAYNES_PORTS"):
            self.assertNotIn(wrong, said.reason)


if __name__ == "__main__":
    unittest.main()
