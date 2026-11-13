"""What do you think? at a stop with decisions: the person writes their opinion in their own words, and a Claude that
Jaynes runs reads it with the decisions and marks each line: keep, redo, drop for good, or start from it, with the
note each asks for.

It may only read copies in a folder of its own, as Why? does, and changes nothing in the run. Its marks only fill the
form on the page: the person checks them, changes any by hand, and sends the stop as usual.
"""

from __future__ import annotations

import json
import re
import shutil
import threading
import time
from collections.abc import Callable, Collection, Mapping
from pathlib import Path
from typing import Any, cast

from jaynes import why
from jaynes.role import Decision

MOST = 12  # opinions kept per step
BUTTONS = {"keep": "ok", "redo": "no", "drop": "drop", "base": "base"}  # a mark, as the page's button names it

PROMPT = """The person judges the decisions below at a stop of a run, and wrote what they think in their own words.
Mark each decision as they meant it:
- keep: they want it as it is;
- redo: they want it redone; the note says what is wrong, in their words;
- drop: they never want it again, in this run; the note says why, if they said;
- base: they want to start from it and change a lot; the note says what to change;
- none: they did not speak of it, or you cannot tell.
Pictures of the decisions, when there are any, are in files/, named on each decision's line. Read nothing else and change nothing.

Answer with one JSON object and nothing else:
{"marks": {"<id>": {"mark": "keep|redo|drop|base|none", "note": "<in their words, or empty>"}},
 "understood": "<one short sentence saying what you understood, in the language the person wrote in>"}

The decisions, as id: label = value:
%s

What the person wrote:
"""


def folder(home: Path, run_id: str, step: str) -> Path:
    return home / "runs" / run_id / step / "opinion"


def latest(home: Path, run_id: str, step: str) -> dict[str, Any] | None:
    """The last opinion written at this step, with its marks once they are read, or why they could not be."""
    root = folder(home, run_id, step)
    found = sorted((p for p in root.glob("*/asked.json") if p.parent.name.isdigit()),
                   key=lambda p: int(p.parent.name)) if root.is_dir() else []
    for f in reversed(found):
        try:
            return cast(dict[str, Any], json.loads(f.read_text()))
        except (OSError, ValueError):
            continue
    return None


def marks(raw: str, decisions: list[Decision], locked: Collection[str]) -> tuple[dict[str, dict[str, str]], str]:
    """The marks the Claude gave, as the page's buttons name them, and what it understood. Lines that are not on the
    stop, marks that are not one of the four, keep (the page's default) and what the person locked are left out."""
    text = raw.strip()
    fenced = re.search(r"```(?:json)?\s*(\{.*\})\s*```", text, re.S)
    start, end = text.find("{"), text.rfind("}")
    body = fenced.group(1) if fenced else text[start:end + 1] if 0 <= start < end else ""
    try:
        got = json.loads(body)
    except ValueError as err:
        raise ValueError(f"the answer was not the JSON asked for: {text[:200]}") from err
    if not isinstance(got, dict):
        raise ValueError("the answer was not a JSON object")
    answer = cast(dict[str, Any], got)
    ids = {d.id for d in decisions}
    out: dict[str, dict[str, str]] = {}
    given = answer.get("marks")
    pairs: list[tuple[str, Any]] = list(cast(dict[str, Any], given).items()) if isinstance(given, dict) else []
    for ident, m in pairs:
        if ident not in ids or ident in locked or not isinstance(m, dict):
            continue
        mark = cast(dict[str, Any], m)
        button = BUTTONS.get(str(mark.get("mark", "")))
        if button is None or button == "ok":
            continue
        out[ident] = {"mark": button, "note": str(mark.get("note") or "")[:500]}
    return out, str(answer.get("understood") or "")[:300]


def ask(home: Path, run_id: str, step: str, round_: int, text: str, decisions: list[Decision],
        locked: Collection[str], pictures: Mapping[str, Mapping[str, Path]], wait: bool = False) -> None:
    """Start reading what the person wrote, in the background, against the stop's ``decisions``. ``pictures`` are
    each decision's, by its id, as path to file: the reader finds them in files/<its number>/, whatever step drew
    them, so it can tell "the left one" from the rest."""
    root = folder(home, run_id, step)
    root.mkdir(parents=True, exist_ok=True)
    n = 1 + max((int(p.name) for p in root.iterdir() if p.name.isdigit()), default=0)
    if n > MOST:
        raise ValueError(f"{MOST} opinions were written at this step already")
    here = root / str(n)
    (here / "files").mkdir(parents=True)
    shown: dict[str, list[str]] = {}
    room = 40
    for n, d in enumerate(decisions, 1):
        for rel, src in list(pictures.get(d.id, {}).items())[:max(room, 0)]:
            dest = here / "files" / str(n) / rel
            if dest.resolve().is_relative_to((here / "files").resolve()) and src.is_file():
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(src, dest)
                shown.setdefault(d.id, []).append(f"files/{n}/{rel}")
                room -= 1
    lines = "\n".join(f"- {d.id}: {d.label} = {d.value}"
                      + (f" (pictures: {', '.join(shown[d.id])})" if d.id in shown else "")
                      + (" (locked: the person kept it)" if d.id in locked else "") for d in decisions)
    record = here / "asked.json"
    base = {"text": text, "round": round_, "asked": time.time()}
    record.write_text(json.dumps({**base, "marks": None}))

    def read() -> None:
        try:
            got, said = marks(reader(here, PROMPT % lines + text), decisions, locked)
            done: dict[str, Any] = {**base, "marks": got, "understood": said}
        except Exception as err:  # the person sees why and marks by hand, or writes again
            done = {**base, "marks": None, "error": f"{type(err).__name__}: {err}"[:500]}
        record.write_text(json.dumps(done))

    if wait:
        read()
    else:
        threading.Thread(target=read, daemon=True).start()


reader: Callable[[Path, str], str] = why.claude_explains  # tests put a fake here
