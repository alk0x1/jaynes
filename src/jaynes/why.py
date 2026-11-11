"""Why? at a stop: a Claude that Jaynes runs reads one step of a run and answers the person's question.

It reads copies of what the step was told, what it delivered and what was said in the run, in a folder of its own,
and may change nothing: no edit, no command, no web. Its answer is shown on the stop and changes nothing in the run.
To change something, the person rejects it, as at any stop.
"""

from __future__ import annotations

import json
import shutil
import threading
import time
from collections.abc import Callable, Mapping
from pathlib import Path
from typing import Any

from jaynes import runtimes

DEADLINE = 240.0  # seconds a question may take
MOST = 12  # questions kept per step

PROMPT = """You explain one step of a run to the person who owns the run. In this folder:
- task.md: what the person asked for;
- step.md: what the step was told to do;
- output.json: what the step delivered, as the person sees it now;
- previous.json: what it delivered in the round before, when there is one;
- notes.md: what the person and Jaynes said in this run;
- decisions.md: the decisions the person judges at this stop, which are locked and what unlocks with what, when the
  step shows them;
- files/: files the step left, such as pictures.
Read what you need and change nothing. Answer the person's question in at most five short sentences, in plain words
and in the language of the question, and say what in these files your answer rests on. If the files do not say, say
that they do not.

Question: """


def folder(home: Path, run_id: str, step: str) -> Path:
    return home / "runs" / run_id / step / "why"


def asked(home: Path, run_id: str, step: str) -> list[dict[str, Any]]:
    """The questions asked at this step, oldest first, each with its answer once there is one."""
    found: list[dict[str, Any]] = []
    for f in sorted(folder(home, run_id, step).glob("*/asked.json"), key=lambda p: int(p.parent.name)):
        try:
            found.append(json.loads(f.read_text()))
        except (OSError, ValueError):
            continue
    return found


def ask(home: Path, run_id: str, step: str, round_: int, question: str, given: Mapping[str, str],
        files: Mapping[str, Path], wait: bool = False) -> None:
    """Start answering ``question`` in the background, from ``given`` (file name to text) and the step's ``files``."""
    root = folder(home, run_id, step)
    root.mkdir(parents=True, exist_ok=True)
    n = 1 + max((int(p.name) for p in root.iterdir() if p.name.isdigit()), default=0)
    if n > MOST:
        raise ValueError(f"{MOST} questions were asked at this step already")
    here = root / str(n)
    (here / "files").mkdir(parents=True)
    for name, text in given.items():
        (here / name).write_text(text)
    for rel, src in list(files.items())[:40]:
        dest = here / "files" / rel
        if dest.resolve().is_relative_to((here / "files").resolve()) and src.is_file():
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(src, dest)
    record = here / "asked.json"
    record.write_text(json.dumps({"question": question, "round": round_, "asked": time.time(), "answer": None}))

    def answer() -> None:
        try:
            said, error = explainer(here, PROMPT + question), None
        except Exception as err:  # the question failed; the person sees why and may ask again
            said, error = None, f"{type(err).__name__}: {err}"[:500]
        record.write_text(json.dumps({"question": question, "round": round_, "asked": time.time(),
                                      "answer": said, "error": error}))

    if wait:
        answer()
    else:
        threading.Thread(target=answer, daemon=True).start()


def settings(here: Path) -> dict[str, Any]:
    """Read this folder and nothing else: no edit, no command, no web, no agents."""
    return {"permissions": {"defaultMode": "dontAsk", "allow": ["Read(./**)", "Glob", "Grep"],
                            "deny": ["Edit", "Write", "Bash", "NotebookEdit", "Agent", "Task", *runtimes.WEB_TOOLS],
                            "blockReadsOutsideWorkingDirectories": True, "disableBypassPermissionsMode": "disable"},
            "disableAllHooks": True, "autoMemoryEnabled": False, "disableClaudeAiConnectors": True,
            "syncClaudeAiPlugins": False, "syncClaudeAiSkills": False, "disableBundledSkills": True}


def claude_explains(here: Path, prompt: str) -> str:
    args = [runtimes.binary("claude"), "-p", "--output-format", "stream-json", "--verbose",
            "--setting-sources", "", "--settings", json.dumps(settings(here)), "--strict-mcp-config",
            "--mcp-config", json.dumps({"mcpServers": {}}), "--no-session-persistence", "--permission-mode",
            "dontAsk", "--max-turns", "12", "--model", runtimes.MODEL]
    scratch = here.parent / f"{here.name}.scratch"
    scratch.mkdir(exist_ok=True)
    code, out, err = runtimes.run(args, here, scratch, prompt, DEADLINE,
                                  {"CLAUDE_CODE_DISABLE_CLAUDE_MDS": "1"})
    result, _ = runtimes.claude_result(out)
    if result.get("is_error") or code != 0:
        raise runtimes.claude_failure(result, code, err)
    return str(result.get("result") or "").strip()


explainer: Callable[[Path, str], str] = claude_explains  # tests put a fake here
