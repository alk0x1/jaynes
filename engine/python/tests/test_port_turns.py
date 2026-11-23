"""The ports' turns between runs (T10, 23/11): while a command with its own test servers runs, Jaynes holds the
ports of the usual databases (`_holding`), and the box of another run whose project the person let use one of them
waits for its turn instead of failing, saying which run holds it. A run that dies holding its turn lets it go: the
turn is the system's lock on a file, never a number written down."""

from __future__ import annotations

import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import jaynes_box as box  # noqa: E402

LIB = Path(__file__).resolve().parents[1]


class TurnsOfAPort(unittest.TestCase):
    def setUp(self) -> None:
        self.root = Path(tempfile.mkdtemp(prefix="jaynes-turns-"))
        self.turns = self.root / "turns"
        patch = mock.patch.object(box, "PORT_TURNS", self.turns)
        patch.start()
        self.addCleanup(patch.stop)
        self.addCleanup(shutil.rmtree, self.root, True)

    def test_a_run_that_uses_a_freed_port_waits_while_another_run_holds_it_and_says_which(self) -> None:
        waits = self.root / "b" / "port-waits"
        entered = threading.Event()
        said: list[str] = []
        live: list[bool] = []
        with box._port_turns(shared=(5432, 6379), who="horta · Add a ping"):
            def b() -> None:
                with mock.patch("sys.stderr") as err:
                    with box._port_turns(exclusive=(5432,), who="estoque · Fix login", waits=waits) as t:
                        said.extend(c.args[0] for c in err.write.call_args_list if c.args and c.args[0].strip())
                        self.assertGreater(t.waited, 0.2)
                        entered.set()
            other = threading.Thread(target=b)
            other.start()
            time.sleep(0.5)
            self.assertFalse(entered.is_set(), "it waits its turn")
            # where the person looks: the run's wait, live while it waits
            files = list(waits.glob("*.json"))
            self.assertEqual(len(files), 1, files)
            self.assertEqual(json.loads(files[0].read_text())["said"],
                             "Waiting for port 5432: another run holds it while its commands run (horta · Add a ping)")
            live.append(box._live(files[0]))
        other.join(5)
        self.assertTrue(entered.is_set(), "it goes on once the other run's commands end")
        self.assertEqual(live, [True])
        self.assertIn("Waiting for port 5432: another run holds it while its commands run (horta · Add a ping)", said)
        self.assertEqual(list(waits.glob("*.json")), [], "the wait is gone once it ended")

    def test_runs_that_only_hold_never_wait_for_each_other(self) -> None:
        with box._port_turns(shared=(5432,), who="a"), box._port_turns(shared=(5432,), who="b") as t:
            self.assertEqual(t.waited, 0)

    def test_a_run_that_dies_holding_its_turn_lets_it_go(self) -> None:
        holder = subprocess.Popen(
            [sys.executable, "-I", "-c",
             "import sys, time; sys.path.insert(0, sys.argv[1]); import jaynes_box as box; from pathlib import Path; "
             "box.PORT_TURNS = Path(sys.argv[2]); t = box._port_turns(shared=(5432,), who='horta · Add a ping'); "
             "t.__enter__(); print('in', flush=True); time.sleep(60)", str(LIB), str(self.turns)],
            stdout=subprocess.PIPE, text=True)
        self.addCleanup(holder.stdout.close)
        self.addCleanup(lambda: holder.poll() is None and holder.kill())
        self.assertEqual(holder.stdout.readline().strip(), "in")
        self.assertEqual(box._holders(5432), ["horta · Add a ping"])
        os.kill(holder.pid, signal.SIGKILL)  # only the process this test started
        holder.wait(5)
        started = time.monotonic()
        with box._port_turns(exclusive=(5432,), who="estoque · Fix login") as t:
            self.assertLess(time.monotonic() - started, 1)
            self.assertEqual(t.waited, 0)
        # what it left saying who held it names no one
        self.assertEqual(box._holders(5432), [])

    def test_two_runs_that_each_let_a_port_the_other_holds_both_go_on(self) -> None:
        done: list[str] = []

        def run(name: str, shared: tuple[int, ...], exclusive: tuple[int, ...]) -> None:
            with box._port_turns(shared=shared, exclusive=exclusive, who=name):
                time.sleep(0.2)
                done.append(name)

        runs = [threading.Thread(target=run, args=("a", (5432,), (6379,))),
                threading.Thread(target=run, args=("b", (6379,), (5432,)))]
        for r in runs:
            r.start()
        for r in runs:
            r.join(5)
        self.assertEqual(sorted(done), ["a", "b"])

    def test_the_turns_folder_is_this_user_s_alone(self) -> None:
        self.turns.symlink_to(self.root)
        with self.assertRaises(OSError):
            box._turns_dir()



@unittest.skipUnless(sys.platform == "darwin", "the window, and the ports another run's box holds, are macOS's box")
class TwoChecksAtOnce(unittest.TestCase):
    """The race of T10 in real boxes: one run's check holds the databases' ports while its tests run, and another
    run's check, whose project the person let use one of them, waits and passes instead of failing."""

    def test_the_check_let_use_a_port_another_run_s_check_holds_waits_and_passes(self) -> None:
        root = Path(tempfile.mkdtemp(prefix="jaynes-turns-real-"))
        self.addCleanup(shutil.rmtree, root, True)
        patch = mock.patch.object(box, "PORT_TURNS", root / "turns")
        patch.start()
        self.addCleanup(patch.stop)
        port = next(p for p in box.HELD_PORTS if not box._held(p))
        a, b = root / "a", root / "b"
        for d in (a, b):
            d.mkdir()
            (d / "README").write_text("x\n")
        holding = threading.Event()
        ran_a: list[box.Ran] = []

        def check_a() -> None:
            ran_a.append(box.run(a, "sleep 2", test_servers=True, timeout=30))

        first = threading.Thread(target=check_a)
        first.start()
        until = time.monotonic() + 10
        while time.monotonic() < until and not holding.is_set():
            if box._held(port):
                holding.set()
            time.sleep(0.05)
        self.assertTrue(holding.is_set(), "the first check holds the port while its tests run")
        started = time.monotonic()
        ran = box.run(b, f"python3 -c \"import socket; s = socket.socket(); s.bind(('127.0.0.1', {port})); print('bound')\"",
                      test_servers=True, freed_ports=(port,), timeout=1)
        first.join(30)
        self.assertEqual((ran.code, ran.output.strip().splitlines()[-1:]), (0, ["bound"]), ran.output)
        # it waited for the first check's tests, and its own second of time limit never counted the wait
        self.assertGreater(time.monotonic() - started, 1, ran.output)
        self.assertEqual(ran_a[0].code, 0, ran_a[0].output)


if __name__ == "__main__":
    unittest.main()
