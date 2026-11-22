"""The ports a project's own test servers may reach on macOS (jaynes_box.servers_window): the next ones macOS gives,
without the ones a server listens on now. A client's end of a connection is no server: macOS gives its port again
the moment it closes, so the window keeps it. Run from engine/:
    PYTHONPATH=python python3 -m unittest discover -s python/tests
"""

from __future__ import annotations

import socket
import sys
import threading
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

    def test_where_only_lsof_lists_a_server_one_it_cannot_see_is_found_by_a_connection_that_sends_nothing(self):
        # lsof lists only the person's own processes: a server of root's, or of another user's, in the window would
        # be open to the tests. Jaynes connects to each port of the window and closes at once, sending nothing
        heard: list[bytes] = []

        def listen():
            conn, _ = self.server.accept()
            conn.settimeout(3)
            try:
                heard.append(conn.recv(1024))
            finally:
                conn.close()

        listener = threading.Thread(target=listen)
        listener.start()
        unseen, seen = box._netstat_listening, box._lsof_listening
        box._netstat_listening = lambda: None
        box._lsof_listening = lambda: frozenset()
        try:
            window = box._window_now(self.port - 1, *box._system_ports()[:2])
        finally:
            box._netstat_listening, box._lsof_listening = unseen, seen
        listener.join(5)
        self.assertNotIn(self.port, window)
        self.assertIn(self.end, window)
        self.assertEqual(heard, [b""])

    def test_looking_for_servers_moves_on_none_of_the_ports_macos_gives_the_tests(self):
        # a connection takes its own end from the same sequence the tests' servers get theirs from: one each for 500
        # ports would put a test's server 1,000 ports further, past the window (seen in the e2e, 22/11)
        def given():
            s = socket.socket()
            try:
                s.bind(("127.0.0.1", 0))
                return s.getsockname()[1]
            finally:
                s.close()

        before = given()
        found = box._answering(tuple(range(40000, 40500)) + (self.port,))
        moved = given() - before
        self.assertIn(self.port, found)
        self.assertLess(moved, 100, moved)

    def test_where_netstat_lists_every_server_nothing_is_connected_to(self):
        tried = []
        reach = box._answering
        box._answering = lambda ports: tried.append(ports) or frozenset()
        try:
            box._window_now(self.port - 1, *box._system_ports()[:2])
        finally:
            box._answering = reach
        if box._netstat_listening() is not None:
            self.assertEqual(tried, [])


if __name__ == "__main__":
    unittest.main()
