"""The ports a project's own test servers may reach on macOS (jaynes_box.servers_window): the next ones macOS gives,
without the ones a server listens on now. A client's end of a connection is no server: macOS gives its port again
the moment it closes, so the window keeps it. Run from engine/:
    PYTHONPATH=python python3 -m unittest discover -s python/tests
"""

from __future__ import annotations

import socket
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import jaynes_box as box  # noqa: E402


@unittest.skipUnless(sys.platform == "darwin", "the window is macOS's box")
class Window(unittest.TestCase):
    def setUp(self):
        self.server = socket.socket()
        self.server.bind(("127.0.0.1", 0))
        self.server.listen()
        self.port = self.server.getsockname()[1]
        self.client = socket.create_connection(("127.0.0.1", self.port), timeout=3)
        self.end = self.client.getsockname()[1]
        self.accepted = self.server.accept()[0]

    def tearDown(self):
        for s in (self.accepted, self.client, self.server):
            s.close()

    def test_a_listening_server_is_left_out_and_a_client_s_end_of_a_connection_is_not(self):
        listening = box._listening()
        self.assertIn(self.port, listening)
        self.assertNotIn(self.end, listening)

    def test_the_window_leaves_out_only_listening_ports(self):
        # the window is the next ports macOS gives: a client's end there must stay in it
        first, last, _ = box._system_ports()
        listening = box._listening()
        self.assertIn(self.end, box._window_from(self.end - 1, first, last, listening))
        self.assertNotIn(self.port, box._window_from(self.port - 1, first, last, listening))


if __name__ == "__main__":
    unittest.main()
