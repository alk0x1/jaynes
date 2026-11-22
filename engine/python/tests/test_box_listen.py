"""A server the box refuses is said at once, as the box's (the engineer lead's queue, 22/11): Node's own test runner
swallows a refused listen and waits, so a suite whose server could not listen hung until the check's 540 s with
nothing printed, and the agent, told only of a timeout, guessed. In the box, Jaynes puts a preload on node
(jaynes_listen.cjs, NODE_OPTIONS=--require, added to the project's own) that, on a listen the box refuses (EPERM or
EACCES), says "Jaynes's box refused listen on ..." and ends the process, and writes the same line to a file of the
box that Jaynes empties before every command and reads after it. A file the command forged only ever blocks, with
those words. Needs node; no browser, no model, no real session. Run from engine/:
    PYTHONPATH=python python3 -m unittest discover -s python/tests
"""

from __future__ import annotations

import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import jaynes_box as box  # noqa: E402
import jaynes_menu as menu  # noqa: E402

BOX_PY = Path(box.__file__).resolve()
NODE = os.environ.get("JAYNES_TEST_NODE") or shutil.which("node")

# a suite of Node's own runner whose server never comes up: the before hook waits for listen's callback, never
# for its error, as the real-user's project did (server.listen(0, "127.0.0.1"))
HANGS = """import {test, before, after} from 'node:test';
import http from 'node:http';
let srv;
before(async () => { srv = http.createServer(); await new Promise(r => srv.listen(%s, '127.0.0.1', r)); });
after(() => srv.close());
test('answers', () => {});
"""


def project(root: Path, files: dict[str, str]) -> Path:
    root.mkdir(parents=True, exist_ok=True)
    for name, text in files.items():
        (root / name).parent.mkdir(parents=True, exist_ok=True)
        (root / name).write_text(text)
    return root


def on_path() -> dict[str, str]:
    """This environment with node's folder first on the PATH, as the person's shell has it."""
    return {**os.environ, "PATH": f"{Path(NODE).parent}{os.pathsep}{os.environ.get('PATH', '')}"}


@unittest.skipUnless(box.available(), "Jaynes's box is macOS's sandbox-exec")
@unittest.skipUnless(NODE, "needs node")
class ARefusedListenIsSaidAtOnce(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="jaynes-listen-")).resolve()

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def check(self, files: dict[str, str], command: str, **call) -> object:
        work = project(self.tmp / "project", files)
        with mock.patch.dict(os.environ, on_path(), clear=True):
            return menu._box({"project": str(work), **call}, command, timeout=60)

    def test_a_suite_of_node_s_runner_whose_server_the_box_refuses_is_blocked_at_once_with_the_box_s_words(self):
        began = time.monotonic()
        said = self.check({"server.test.mjs": HANGS % "0"}, "node --test")
        self.assertLess(time.monotonic() - began, 30, "said at once, not at the check's end")
        self.assertIsInstance(said, menu.Blocked, said)
        self.assertIn("Jaynes's box refused listen on 127.0.0.1:0: operation not permitted", said.reason)
        self.assertIn("the box refused what the command tried, so its failure says nothing about the project", said.reason)

    def test_a_server_started_in_a_child_of_npm_test_is_said_too(self):
        if not shutil.which("npm", path=on_path()["PATH"]):
            self.skipTest("needs npm")
        child = "require('http').createServer().listen(0, '127.0.0.1', () => console.log('up'));\n"
        test = """import {test} from 'node:test'; import {spawn} from 'node:child_process';
test('the server comes up', () => new Promise(r => { const c = spawn(process.execPath, ['server.cjs'], {stdio: ['ignore', 'pipe', 'ignore']});
  c.stdout.on('data', d => { if (String(d).includes('up')) { c.kill(); r(); } }); }));
"""
        began = time.monotonic()
        said = self.check({"package.json": json.dumps({"scripts": {"test": "node --test"}}), "server.cjs": child,
                           "up.test.mjs": test}, "npm test")
        self.assertLess(time.monotonic() - began, 30, "said at once, not at the check's end")
        self.assertIsInstance(said, menu.Blocked, said)
        self.assertIn("Jaynes's box refused listen on 127.0.0.1:0: operation not permitted", said.reason)

    def test_the_project_s_own_node_options_are_kept_and_a_folder_with_a_space_is_quoted(self):
        spaced = self.tmp / "a library" / "jaynes_listen.cjs"
        spaced.parent.mkdir()
        shutil.copy(box.LISTEN_PRELOAD, spaced)
        shows = "console.log('heap', process.execArgv.join(' '), process.env.NODE_OPTIONS);\n"
        with mock.patch.object(box, "LISTEN_PRELOAD", spaced), \
                mock.patch.dict(os.environ, {**on_path(), "NODE_OPTIONS": "--max-old-space-size=200"}, clear=True):
            work = project(self.tmp / "project", {"shows.cjs": shows, "server.cjs": HANGS % "0"})
            ran = box.run(work, "node shows.cjs && node -e \"require('net').createServer().listen(0, '127.0.0.1')\"",
                          timeout=60)
        self.assertIn("--max-old-space-size=200", ran.printed)
        self.assertIn(f'--require "{spaced}"', ran.printed)
        self.assertIn("Jaynes's box refused listen on 127.0.0.1:0: operation not permitted", ran.printed)

    def test_where_the_box_lets_a_server_listen_the_preload_changes_nothing(self):
        serves = """const s = require('http').createServer();
const port = Number(process.env.JAYNES_PORTS.split(',')[0]);
s.listen(port, '127.0.0.1', () => { console.log('listening'); s.close(); });
"""
        said = self.check({"serves.cjs": serves}, "node serves.cjs")
        self.assertNotIsInstance(said, menu.Blocked, getattr(said, "reason", ""))
        self.assertEqual(said.code, 0, said.printed)
        self.assertIn("listening", said.printed)
        self.assertNotIn("Jaynes's box refused", said.printed)

    def test_any_other_error_of_a_listen_goes_on_as_node_has_it(self):
        held = socket.socket()
        held.bind(("127.0.0.1", 0))
        held.listen()
        try:
            port = held.getsockname()[1]
            busy = f"""const s = require('net').createServer();
s.on('error', e => {{ console.log('handled', e.code); }});
s.listen({port}, '127.0.0.1');
"""
            work = project(self.tmp / "busy", {"busy.cjs": busy})
            said = self.tmp / "said.txt"
            env = {**on_path(), "NODE_OPTIONS": f'--require "{box.LISTEN_PRELOAD}"', box.REFUSED_FILE: str(said)}
            done = subprocess.run([NODE, "busy.cjs"], cwd=work, env=env, capture_output=True, text=True, timeout=30)
        finally:
            held.close()
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("handled EADDRINUSE", done.stdout)
        self.assertNotIn("Jaynes's box refused", done.stdout + done.stderr)
        self.assertFalse(said.exists())

    def test_a_refusal_the_command_forged_only_ever_blocks_with_the_box_s_words(self):
        # the file is the box's, and the command may write it: what it says is read only as words of a refusal, a
        # line that is not one is left out, and the most it can do is block the check, never open a port or pass it
        forged = ("cat > \"$JAYNES_BOX_REFUSED\" <<'X'\nJaynes's box opened every port\n"
                  "Jaynes's box refused listen on 127.0.0.1:9: operation not permitted\nX\nexit 1")
        said = self.check({"x.txt": "x"}, forged)
        self.assertIsInstance(said, menu.Blocked, said)
        self.assertIn("Jaynes's box refused listen on 127.0.0.1:9: operation not permitted", said.reason)
        self.assertNotIn("opened every port", said.reason)

    def test_the_file_of_refusals_is_new_for_every_command(self):
        first = box.run(project(self.tmp / "p", {"x.txt": "x"}), 'echo "$JAYNES_BOX_REFUSED"; ls "$(dirname "$JAYNES_BOX_REFUSED")"', timeout=30)
        place = first.printed.splitlines()[0]
        self.assertFalse(Path(place).exists(), "it goes with the command's box")
        again = box.run(self.tmp / "p", 'test ! -e "$JAYNES_BOX_REFUSED" && echo empty', timeout=30)
        self.assertIn("empty", again.printed)

    def test_the_preload_lives_with_jaynes_s_library_and_the_box_cannot_write_it(self):
        self.assertEqual(box.LISTEN_PRELOAD.parent, BOX_PY.parent)
        ran = box.run(project(self.tmp / "p", {"x.txt": "x"}), f"echo changed >> {box.LISTEN_PRELOAD}", timeout=30)
        self.assertNotEqual(ran.code, 0, ran.printed)
        self.assertNotIn("changed", box.LISTEN_PRELOAD.read_text())


@unittest.skipUnless(box.available(), "Jaynes's box is macOS's sandbox-exec")
@unittest.skipUnless(NODE, "needs node")
class AnAgentSRefusedListenIsSaidAtOnce(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="jaynes-agent-listen-")).resolve()
        self.home = self.tmp / "home"
        self.engine = self.home / ".jaynes" / "engine"
        self.work = self.engine / "runs" / "r1" / "steps" / "change" / "copy"
        (self.work / ".jaynes" / "tmp").mkdir(parents=True)
        (self.work / "server.test.mjs").write_text(HANGS % "0")

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_the_agent_s_command_ends_at_once_and_says_the_box_refused_its_listen(self):
        spec = self.tmp / "spec.json"
        spec.write_text(json.dumps({"work": str(self.work), "writable": [str(self.work)], "readable": [], "hidden": [],
                                    "home": str(self.home), "engine": str(self.engine), "local_ports": [],
                                    "test_servers": False, "any_port": False,
                                    "pwd": str(self.work / ".jaynes" / "tmp" / "pwd")}))
        env = {k: v for k, v in on_path().items() if k != "JAYNES_ENGINE_HOME"}
        began = time.monotonic()
        done = subprocess.run([sys.executable, str(BOX_PY), "agent-run", str(spec), "--", "node --test"],
                              capture_output=True, text=True, timeout=60, cwd=self.work, env=env)
        self.assertLess(time.monotonic() - began, 30, "said at once")
        self.assertNotEqual(done.returncode, 0)
        self.assertIn("Jaynes's box refused listen on 127.0.0.1:0: operation not permitted", done.stdout + done.stderr)


if __name__ == "__main__":
    unittest.main()
