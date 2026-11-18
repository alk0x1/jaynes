"""A role's own step as a script: how its end is read (done, cannot be done, could not run, broke, may have acted),
what it is given, how its output is read, and what the box lets it write and reach. Run from engine/:
    PYTHONPATH=python python3 -m unittest discover -s python/tests
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import jaynes_box as box  # noqa: E402
import jaynes_menu as menu  # noqa: E402
from jaynes_role import Blocked, Pass, Refused  # noqa: E402

SCRIPT = "/roles/announce/steps/announce.sh"
WITH = {"path": SCRIPT, "name": "steps/announce.sh", "project": "none", "local_ports": [], "acts": False}
CALL = {"step": "announce", "request": "tell them", "round": 0, "attempt": 1, "key": None,
        "given": [{"step": "survey", "output": {"version": "1.4.0"}}], "project": None, "source": None}
ACTED = "announce cannot be undone, so it may have acted: look before you continue"


def ended(code: int, printed: str = "", output: str | None = None, unread: str = "") -> box.Stepped:
    return box.Stepped(box.Ran(code, printed[-box.TAIL:], printed), output, unread)


def step(end: box.Stepped, params: dict[str, Any] | None = None, call: dict[str, Any] | None = None) -> tuple[Any, mock.Mock]:
    """The step's result when its script ended as ``end``, and how the box was asked to run it."""
    with mock.patch.object(menu.box, "run_step", return_value=end) as ran:
        return menu.script_step({**CALL, **(call or {})}, {**WITH, **(params or {})}), ran


class HowItEnded(unittest.TestCase):
    def test_exit_0_with_an_object_is_the_step_s_output(self):
        got, _ = step(ended(0, "posting\n", '{"id": "a1"}'))
        self.assertEqual(got, {"id": "a1"})

    def test_exit_1_is_the_script_s_word_that_the_step_cannot_be_done_and_nothing_was(self):
        for acts in (False, True):
            got, _ = step(ended(1, "the registry does not answer, so nothing was announced\n"), {"acts": acts})
            self.assertIsInstance(got, Refused)
            self.assertEqual(got.reason, "the registry does not answer, so nothing was announced")
        got, _ = step(ended(1))
        self.assertEqual(got.reason, "steps/announce.sh said the step cannot be done, and not why")

    def test_exit_3_is_the_machine_s_and_the_script_did_not_run(self):
        for acts in (False, True):
            got, _ = step(ended(3, "python3 is not installed\n"), {"acts": acts})
            self.assertIsInstance(got, Blocked)
            self.assertEqual(got.reason, "python3 is not installed")

    def test_any_other_exit_is_a_script_that_broke_and_is_the_role_s(self):
        got, _ = step(ended(2, "line 3: no such command\n"))
        self.assertIsInstance(got, Refused)
        self.assertEqual(got.reason, "steps/announce.sh exited with 2: line 3: no such command")
        got, _ = step(ended(124, "(stopped after 600 s)\n"))
        self.assertIn("exited with 124", got.reason)

    def test_ending_well_with_no_output_or_one_that_does_not_read_is_the_role_s(self):
        for end, says in [
            (ended(0), "wrote no output: it writes JSON to the file named in JAYNES_OUTPUT"),
            (ended(0, unread="the output is larger than 1000000 bytes"), "gave an output Jaynes does not read: the output is larger than 1000000 bytes"),
            (ended(0, output="done!"), "is not JSON"),
            (ended(0, output='["a1"]'), "is not a JSON object"),
        ]:
            got, _ = step(end)
            self.assertIsInstance(got, Refused, says)
            self.assertIn(says, got.reason)
            self.assertNotIn("may have acted", got.reason)

    def test_a_step_that_cannot_be_undone_may_have_acted_in_every_failure_but_its_own_word(self):
        # its outcome is not known: told to the engine as such, never as the role's refusal, which a continue would
        # take as nothing done and run the act again
        for end in [ended(2, "sent\n"), ended(0), ended(0, output="done!"), ended(0, output="[]"), ended(124)]:
            got, _ = step(end, {"acts": True})
            self.assertIsInstance(got, Blocked, got)
            got, _ = step(end, {"acts": False})
            self.assertIsInstance(got, Refused, "a step that does not act is the role's to fix")

    def test_a_box_that_refused_what_the_script_tried_is_the_machine_s(self):
        printed = "urlopen error [Errno 1] Operation not permitted\nthe registry is out of reach\n"
        got, _ = step(ended(1, printed))
        self.assertIsInstance(got, Blocked)
        self.assertIn("the box refused what steps/announce.sh tried, which its step's reach does not allow", got.reason)
        self.assertNotIn("may have acted", got.reason)
        got, _ = step(ended(1, printed), {"acts": True})
        self.assertTrue(got.reason.endswith(ACTED), "what it did before the box refused may be done: " + got.reason)
        # a script that went on after a refusal and ended well is done
        got, _ = step(ended(0, printed, "{}"))
        self.assertEqual(got, {})

    def test_where_there_is_no_box_the_script_does_not_run(self):
        with mock.patch.object(menu.box, "run_step", side_effect=box.Unavailable("no box here")):
            got = menu.script_step(CALL, WITH)
        self.assertIsInstance(got, Blocked)
        self.assertIn("there is no box to run the role's script in: no box here", got.reason)


class WhatItIsGiven(unittest.TestCase):
    def test_it_reads_the_request_what_it_needs_and_its_key_and_nothing_else(self):
        _, ran = step(ended(0, output="{}"), call={"key": "k-1", "answers": [{"by": "person"}], "previous": {"id": "a0"}})
        read = json.loads(ran.call_args.kwargs["stdin"])
        self.assertEqual(read, {"request": "tell them", "given": CALL["given"], "project": None, "round": 0, "attempt": 1, "key": "k-1"})

    def test_it_runs_with_the_reach_its_step_declares(self):
        with tempfile.TemporaryDirectory() as copy:
            reaching = {"project": copy, "source": "/home/me/project"}
            # no reach: nothing of the project, though the run has a copy
            _, ran = step(ended(0, output="{}"), call=reaching)
            self.assertEqual((ran.call_args.kwargs["project"], ran.call_args.kwargs["write"], ran.call_args.kwargs["ports"]), (None, False, ()))
            self.assertIsNone(json.loads(ran.call_args.kwargs["stdin"])["project"])
            # it reads: a copy it cannot keep
            _, ran = step(ended(0, output="{}"), {"project": "read"}, reaching)
            self.assertEqual((ran.call_args.kwargs["project"], ran.call_args.kwargs["write"]), (Path(copy), False))
            self.assertEqual(json.loads(ran.call_args.kwargs["stdin"])["project"], ".")
            self.assertEqual(ran.call_args.kwargs["source"], Path("/home/me/project"))
            # it writes: the run's copy itself
            _, ran = step(ended(0, output="{}"), {"project": "write"}, reaching)
            self.assertEqual((ran.call_args.kwargs["project"], ran.call_args.kwargs["write"]), (Path(copy), True))
            # of the local ports, the ones it names
            _, ran = step(ended(0, output="{}"), {"local_ports": [5432, 8125]})
            self.assertEqual(ran.call_args.kwargs["ports"], (5432, 8125))

    def test_a_script_that_may_not_be_run_by_itself_is_run_by_the_shell(self):
        _, ran = step(ended(0, output="{}"))
        self.assertEqual(ran.call_args.args[0], f"sh {SCRIPT}")
        with tempfile.TemporaryDirectory() as folder:
            script = Path(folder) / "act it.py"
            script.write_text("#!/usr/bin/env python3\n")
            script.chmod(0o755)
            _, ran = step(ended(0, output="{}"), {"path": str(script)})
            self.assertEqual(ran.call_args.args[0], f"'{script}'")


class TheOutputIsReadAsData(unittest.TestCase):
    def setUp(self) -> None:
        self.folder = tempfile.TemporaryDirectory()
        self.at = Path(self.folder.name) / "output.json"
        self.addCleanup(self.folder.cleanup)

    def test_a_plain_file_is_read_and_nothing_there_is_no_output(self):
        self.assertEqual(box._output(self.at), (None, ""))
        self.at.write_text('{"id": "a1"}')
        self.assertEqual(box._output(self.at), ('{"id": "a1"}', ""))

    def test_a_link_is_never_followed(self):
        elsewhere = Path(self.folder.name) / "elsewhere"
        elsewhere.write_text('{"secret": true}')
        self.at.symlink_to(elsewhere)
        text, why = box._output(self.at)
        self.assertIsNone(text)
        self.assertIn("is a link", why)

    def test_a_second_name_for_another_file_is_not_the_script_s_own(self):
        elsewhere = Path(self.folder.name) / "elsewhere"
        elsewhere.write_text('{"secret": true}')
        os.link(elsewhere, self.at)
        text, why = box._output(self.at)
        self.assertIsNone(text)
        self.assertIn("not a plain file of its own", why)

    def test_what_is_not_a_file_is_not_read_and_does_not_hold_jaynes(self):
        self.at.mkdir()
        self.assertEqual(box._output(self.at)[0], None)
        self.at.rmdir()
        os.mkfifo(self.at)  # a pipe nobody writes to would hold a reader for ever
        text, why = box._output(self.at)
        self.assertIsNone(text)
        self.assertIn("not a plain file", why)

    def test_an_output_without_end_is_refused(self):
        self.at.write_bytes(b" " * box.OUTPUT_MOST)
        self.assertEqual(len(box._output(self.at)[0] or ""), box.OUTPUT_MOST)
        self.at.write_bytes(b" " * (box.OUTPUT_MOST + 1))
        text, why = box._output(self.at)
        self.assertIsNone(text)
        self.assertIn(f"larger than {box.OUTPUT_MOST} bytes", why)


class WhatTheBoxLets(unittest.TestCase):
    def test_a_folder_kept_from_writing_is_denied_after_its_parent_is_allowed(self):
        with tempfile.TemporaryDirectory() as copy:
            (Path(copy) / ".git").mkdir()
            rules = box.profile(box.Box((Path(copy),), readonly=(Path(copy) / ".git",))).splitlines()
            allow = next(n for n, line in enumerate(rules) if line.startswith("(allow file-write* (subpath"))
            deny = next(n for n, line in enumerate(rules) if line.startswith("(deny file-write* (subpath"))
            self.assertGreater(deny, allow, "of two rules that match, the later one holds")
            self.assertIn(os.path.realpath(Path(copy) / ".git"), rules[deny])
            self.assertFalse(any(line.startswith("(deny file-write* (subpath") for line in box.profile(box.Box((Path(copy),))).splitlines()))

    def test_no_local_port_is_open_unless_named(self):
        with tempfile.TemporaryDirectory() as copy:
            closed = box.profile(box.Box((Path(copy),)))
            self.assertIn("(deny network-outbound)", closed)
            self.assertNotIn("localhost", closed)
            named = box.profile(box.Box((Path(copy),), ports=(8125,)))
            self.assertIn('(allow network-outbound (remote tcp "localhost:8125"))', named)
            self.assertNotIn("localhost:*", named)

    def test_stopping_a_command_that_just_ended_is_no_error(self):
        done = subprocess.Popen(["/usr/bin/true"], start_new_session=True)
        done.wait()
        box._kill(done)


class ACheckOfAStepThatReachesNoProject(unittest.TestCase):
    def test_its_script_runs_in_an_empty_folder(self):
        seen: dict[str, Any] = {}

        def run(project: Path, command: str, **more: Any) -> box.Ran:
            seen.update(project=project, entries=list(project.iterdir()), command=command, stdin=more.get("stdin"))
            return box.Ran(0, "", "")

        with mock.patch.object(menu.box, "run", side_effect=run):
            got = menu.script({"output": {"id": "a1"}, "given": CALL["given"], "project": None}, {"path": "/roles/announce/checks/announced.sh"})
        self.assertIsInstance(got, Pass)
        self.assertEqual(seen["entries"], [])
        self.assertEqual(json.loads(seen["stdin"])["output"], {"id": "a1"})
        # a check that runs the project's own commands still needs the project
        self.assertIsInstance(menu._box({"project": None}, "true"), Blocked)


@unittest.skipUnless(box.available(), "Jaynes has no box to open here")
class InTheBox(unittest.TestCase):
    def test_a_step_that_writes_works_in_the_run_s_copy_and_not_where_git_keeps_it(self):
        with tempfile.TemporaryDirectory() as copy:
            (Path(copy) / ".git" / "hooks").mkdir(parents=True)
            command = "echo note > note.txt; echo x > .git/hooks/post-commit 2>/dev/null; printf '{\"done\": true}' > \"$JAYNES_OUTPUT\""
            done = box.run_step(command, project=Path(copy), write=True)
            self.assertEqual((done.ran.code, done.output), (0, '{"done": true}'), done.ran.printed)
            self.assertEqual((Path(copy) / "note.txt").read_text(), "note\n")
            self.assertFalse((Path(copy) / ".git" / "hooks" / "post-commit").exists())

    def test_a_step_that_only_reads_leaves_the_run_s_copy_as_it_was_and_one_with_no_reach_sees_none(self):
        with tempfile.TemporaryDirectory() as copy:
            (Path(copy) / "VERSION").write_text("1.4.0\n")
            done = box.run_step("cat VERSION; echo left > LEFT.txt", project=Path(copy))
            self.assertEqual((done.ran.code, done.ran.printed, done.output), (0, "1.4.0\n", None))
            self.assertEqual(sorted(p.name for p in Path(copy).iterdir()), ["VERSION"])
            blind = box.run_step("ls -A | wc -l", project=None)
            self.assertEqual(blind.ran.printed.strip(), "0")


if __name__ == "__main__":
    unittest.main()
