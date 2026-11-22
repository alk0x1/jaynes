"""The single box (the engineer lead's plan, 22/11): every shell command of a Claude Code agent runs in Jaynes's own
box, as the commands of a check do (``jaynes_box.py agent-run SPEC -- COMMAND``, which Jaynes's leave server puts in
place of the command the agent asked for). Stand-ins only: no model, no browser, no real session. Run from engine/:
    PYTHONPATH=python python3 -m unittest discover -s python/tests
"""

from __future__ import annotations

import json
import os
import socket
import subprocess
import sys
import tempfile
import threading
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import jaynes_box as box  # noqa: E402

BOX_PY = Path(box.__file__).resolve()


@unittest.skipUnless(box.available(), "Jaynes's box is macOS's sandbox-exec")
class TheAgentSCommandsRunInJaynesSBox(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="jaynes-agent-box-")).resolve()
        self.home = self.tmp / "home"
        self.engine = self.home / ".jaynes" / "engine"
        self.work = self.engine / "runs" / "r1" / "steps" / "change" / "copy"
        (self.work / ".jaynes" / "tmp").mkdir(parents=True)
        (self.work / "sub").mkdir()
        self.other_run = self.engine / "runs" / "r2"
        self.other_run.mkdir(parents=True)
        (self.other_run / "secret.txt").write_text("OTHER RUN")
        (self.home / "Documents").mkdir(parents=True)
        (self.home / "Documents" / "letter.txt").write_text("PERSON'S FILE")
        self.outside = self.tmp / "outside"
        self.outside.mkdir()

    def spec(self, **more) -> Path:
        given = {"work": str(self.work), "writable": [str(self.work)], "readable": [], "hidden": [str(self.other_run)],
                 "home": str(self.home), "engine": str(self.engine), "local_ports": [], "test_servers": False,
                 "any_port": False, "pwd": str(self.work / ".jaynes" / "tmp" / "pwd"), **more}
        at = self.tmp / "spec.json"
        at.write_text(json.dumps(given))
        return at

    def agent_run(self, command: str, spec: Path | None = None) -> tuple[int, str]:
        env = {k: v for k, v in os.environ.items() if k != "JAYNES_ENGINE_HOME"}
        done = subprocess.run([sys.executable, str(BOX_PY), "agent-run", str(spec or self.spec()), "--", command],
                              capture_output=True, text=True, timeout=60, cwd=self.work, env=env)
        return done.returncode, done.stdout + done.stderr

    def test_a_command_runs_in_the_step_s_folder_and_writes_only_there(self):
        code, said = self.agent_run(f"pwd -P; echo made > here.txt; echo out > {self.outside}/there.txt")
        self.assertNotEqual(code, 0, said)
        self.assertIn(str(self.work), said)
        self.assertEqual((self.work / "here.txt").read_text(), "made\n")
        self.assertFalse((self.outside / "there.txt").exists())
        self.assertIn("not permitted", said.lower())

    def test_a_cd_holds_for_the_next_command_while_it_stays_in_the_step_s_folder(self):
        spec = self.spec()
        self.assertEqual(self.agent_run("cd sub", spec)[0], 0)
        self.assertEqual(self.agent_run("pwd -P", spec), (0, f"{self.work / 'sub'}\n"))
        # a command that ends with an error still keeps where it went
        self.assertNotEqual(self.agent_run("cd .. && false", spec)[0], 0)
        self.assertEqual(self.agent_run("pwd -P", spec), (0, f"{self.work}\n"))
        # out of the step's folder, the next one starts at its top again
        self.assertEqual(self.agent_run(f"cd {self.outside}", spec)[0], 0)
        self.assertEqual(self.agent_run("pwd -P", spec), (0, f"{self.work}\n"))
        # the file it keeps that in is the agent's to write: a folder outside it named there is not followed
        Path(json.loads(spec.read_text())["pwd"]).write_text(str(self.outside))
        self.assertEqual(self.agent_run("pwd -P", spec), (0, f"{self.work}\n"))

    def test_it_reads_nothing_of_the_person_s_home_another_run_or_the_engine(self):
        (self.engine / "page.key").write_text("PAGE KEY")
        for path in (self.home / "Documents" / "letter.txt", self.other_run / "secret.txt", self.engine / "page.key"):
            code, said = self.agent_run(f"cat {path}")
            self.assertNotEqual(code, 0, f"{path}: {said}")
            self.assertNotIn("FILE", said)
            self.assertNotIn("RUN", said)
            self.assertNotIn("KEY", said)

    def test_it_reaches_no_local_server_but_the_ports_the_step_names(self):
        servers = [socket.create_server(("127.0.0.1", 0)) for _ in range(2)]
        try:
            named, other = (s.getsockname()[1] for s in servers)
            line = ("python3 -c 'import socket,sys; s=socket.socket(); s.settimeout(3); "
                    "print(s.connect_ex((\"127.0.0.1\", int(sys.argv[1]))))' {}")
            spec = self.spec(local_ports=[named])
            self.assertEqual(self.agent_run(line.format(named), spec), (0, "0\n"))
            code, said = self.agent_run(line.format(other), spec)
            self.assertNotEqual(said.strip(), "0", said)
            code, said = self.agent_run(line.format(box.PAGE_PORT), self.spec(local_ports=[box.PAGE_PORT]))
            self.assertNotEqual(said.strip(), "0", said)
        finally:
            for s in servers:
                s.close()

    def test_without_its_spec_or_its_work_it_runs_nothing(self):
        code, said = self.agent_run("echo ran", self.tmp / "missing.json")
        self.assertNotEqual(code, 0)
        self.assertNotIn("ran\n", said)
        self.assertIn("Jaynes", said)
        bad = self.tmp / "bad.json"
        bad.write_text(json.dumps({"work": str(self.tmp / "gone")}))
        code, said = self.agent_run("echo ran", bad)
        self.assertNotEqual(code, 0)
        self.assertNotIn("ran\n", said)

    def test_its_box_comes_whole_in_the_command_and_no_file_can_widen_it(self):
        # the leave server reads the turn's box once, as it starts, and gives it in each command: a file the agent's
        # own tools may write is never read again
        spec = self.spec()
        given = spec.read_text()
        spec.write_text(json.dumps({"work": str(self.tmp), "writable": [str(self.tmp)], "readable": [], "hidden": [],
                                    "home": str(self.home), "engine": str(self.engine), "pwd": str(self.tmp / "pwd")}))
        env = {k: v for k, v in os.environ.items() if k != "JAYNES_ENGINE_HOME"}
        done = subprocess.run([sys.executable, str(BOX_PY), "agent-run", "--spec", given, "--", f"pwd -P; echo out > {self.outside}/there.txt"],
                              capture_output=True, text=True, timeout=60, cwd=self.work, env=env)
        self.assertTrue(done.stdout.startswith(f"{self.work}\n"), done.stdout + done.stderr)
        self.assertFalse((self.outside / "there.txt").exists())
        done = subprocess.run([sys.executable, str(BOX_PY), "agent-run", "--spec", "not a box", "--", "echo ran"],
                              capture_output=True, text=True, timeout=60, cwd=self.work, env=env)
        self.assertNotEqual(done.returncode, 0)
        self.assertNotIn("ran\n", done.stdout)
        self.assertIn("Jaynes ran none of this command", done.stderr)

    def test_a_step_that_installs_packages_reaches_its_registries_through_jaynes_s_proxy_and_nothing_else(self):
        # a stand-in registry on this machine's loopback: the proxy lets the box reach it by the name the list gives
        registry = socket.create_server(("127.0.0.1", 0))
        port = registry.getsockname()[1]
        heard: list[bytes] = []

        def serve():
            while True:
                try:
                    conn, _ = registry.accept()
                except OSError:
                    return
                with conn:
                    conn.sendall(b"REGISTRY")
                    conn.settimeout(2)
                    try:
                        heard.append(conn.recv(64))
                    except OSError:
                        pass

        threading.Thread(target=serve, daemon=True).start()
        ask = ("python3 -c 'import os,socket,sys; u=os.environ[\"HTTPS_PROXY\"].split(\"//\")[1].rstrip(\"/\"); "
               "h,p=u.rsplit(\":\",1); s=socket.create_connection((h,int(p)),timeout=5); "
               "s.sendall((\"CONNECT %s HTTP/1.1\\r\\nHost: %s\\r\\n\\r\\n\" % (sys.argv[1], sys.argv[1])).encode()); "
               "head=s.recv(4096).decode(); print(head.split()[1]); "
               "head.startswith(\"HTTP/1.1 200\") and (print(s.recv(64).decode()), s.sendall(b\"hello\"))' {}")
        try:
            spec = self.spec(packages={"hosts": ["localhost"], "port": port})
            code, said = self.agent_run(ask.format(f"localhost:{port}"), spec)
            self.assertEqual((code, said), (0, "200\nREGISTRY\n"))
            # a host the list does not name, or another port, is refused by the proxy
            for target in (f"example.com:{port}", f"localhost:{port + 1}", "localhost:22"):
                code, said = self.agent_run(ask.format(target), spec)
                self.assertEqual((code, said), (0, "403\n"), target)
            # the box itself reaches the registry by no other way
            direct = f"python3 -c 'import socket; print(socket.socket().connect_ex((\"127.0.0.1\", {port})))'"
            code, said = self.agent_run(direct, spec)
            self.assertNotEqual(said.strip(), "0", said)
            # a step that installs nothing has no proxy at all
            code, said = self.agent_run("echo ${HTTPS_PROXY:-none}", self.spec())
            self.assertEqual((code, said), (0, "none\n"))
        finally:
            registry.close()
        self.assertIn(b"hello", heard)

    def test_the_command_gets_its_arguments_whole(self):
        code, said = self.agent_run("printf '%s|' \"a b\" 'c'\"'\"'d' $HOME_UNSET; echo")
        self.assertEqual((code, said), (0, "a b|c'd|\n"))


if __name__ == "__main__":
    unittest.main()
