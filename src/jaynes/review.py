"""The review page (SPEC section 7): ``jaynes review`` serves the queue, stops, runs, roles and the log.

Every number on the page is a projection of the event log. The server binds
127.0.0.1 only and refuses answers that do not carry its per-process token or
that come with a foreign Host or Origin, so another site open in the browser
cannot answer a stop. Only a browser opened with the link the server prints,
which carries its key once and keeps it as a cookie, may answer: any other
client, such as an agent with curl, can read the page but not act as the
person, unless it starts a server itself and reads the link (SPEC section 7,
known limit). Each answer carries the hash of the evidence shown, so a stale
page cannot accept what came after it. After an answer the run continues in
the background.
"""

from __future__ import annotations

import errno
import html
import importlib
import json
import math
import os
import hashlib
import re
import secrets
import shutil
import signal
import sys
import threading
import time
import urllib.parse
from collections import defaultdict
from collections.abc import Iterable, Iterator
from dataclasses import dataclass, field
from datetime import datetime
from http.cookies import CookieError, SimpleCookie
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Literal, cast

from pygments import highlight
from pygments.formatters import HtmlFormatter
from pygments.lexer import Lexer
from pygments.lexers import get_lexer_for_filename  # pyright: ignore[reportUnknownVariableType]
from pygments.util import ClassNotFound

from jaynes import bounds, install, role_file, roles, sandbox, verify
from jaynes import project as repo
from jaynes.conductor import (LABELS, Answer, AnswerRejected, Conductor, RunBusy, RunState, StepState, Waiting,
                              continue_in_background, new_run_id, project, start_in_background)
from jaynes.history import Report, past_calls, report
from jaynes.preview import Previews
from jaynes.policy import find_policy, load_policy
from jaynes.role import Role, Step
from jaynes.store import Event

esc = html.escape
CHIP = {"certified": "green", "refuted": "red", "floor": "blue"}  # any other status is amber
DEMO = "jaynes.demo:ROLE"
BUILT_IN = {DEMO: {"scripted": "jaynes.demo:Agent"}, **{ref: {} for ref in roles.BUILT_IN.values()}}
StageState = Literal["jaynes", "checked", "accepted", "changed", "sampled", "waiting", "blocked", "running", "todo"]

PICTURES = {".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".webp": "image/webp",
            ".gif": "image/gif"}  # no SVG: it can carry scripts

def q(segment: str) -> str:
    """One URL path segment."""
    return urllib.parse.quote(segment, safe="")


def diff_html(text: str) -> str:
    """A unified diff, escaped: added lines green, removed lines red, each file's header set apart, and the code
    highlighted in the file's language where Pygments knows it."""
    out: list[str] = []
    files = repo.diffs(text)
    for path, d in files.items() or [("", text)]:
        try:
            lexer = get_lexer_for_filename(path, stripnl=False, ensurenl=False) if path else None
        except ClassNotFound:
            lexer = None
        lines = d.rstrip("\n").split("\n")
        i = 0
        while i < len(lines):
            line = lines[i]
            if not line.startswith("@@"):
                kind = "file" if line.startswith("diff --git ") else "meta" if files else "ctx"
                out.append(f'<span class="{kind}">{esc(line) or " "}</span>')
                i += 1
                continue
            out.append(f'<span class="hunk">{esc(line)}</span>')
            end = next((j for j in range(i + 1, len(lines)) if lines[j].startswith("@@")), len(lines))
            out += _hunk(lines[i + 1:end], lexer)
            i = end
    return "".join(out)


def _hunk(body: list[str], lexer: Lexer | None) -> list[str]:
    """A hunk's lines, each side read whole by the lexer so a line inside a string or comment is colored as one."""
    def side(marks: str) -> Iterator[str]:
        code = [x[1:] for x in body if x[:1] in marks]
        if lexer is None:
            return iter([esc(x) for x in code])
        formatter: HtmlFormatter[str] = HtmlFormatter(nowrap=True)
        shown = highlight("\n".join(code) + "\n", lexer, formatter).split("\n")
        return iter(shown[:len(code)])
    old, new = side(" -"), side(" +")
    out: list[str] = []
    for x in body:
        mark = x[:1]
        if mark == " ":
            next(old)
            out.append(f'<span class="ctx"><i> </i>{next(new)}</span>')
        elif mark in "+-":
            code = next(new) if mark == "+" else next(old)
            out.append(f'<span class="{"add" if mark == "+" else "del"}"><i>{mark}</i>{code}</span>')
        else:  # "\ No newline at end of file", or an empty line
            out.append(f'<span class="meta">{esc(x) or " "}</span>')
    return out


# --- reading the log -------------------------------------------------------------

@dataclass(frozen=True)
class Stage:
    name: str
    state: StageState
    detail: str
    seconds: float | None = None  # from the step's first event to its last, once it is released


def stages(names: Iterable[str], events: list[Event], st: RunState, now: float) -> list[Stage]:
    """How each step of a run was released, in role order."""
    last_answer: dict[str, str] = {}
    skipped: set[str] = set()
    span: dict[str, tuple[float, float]] = {}
    for ev in events:
        step = ev.payload.get("step")
        if step is not None:
            first = span.get(str(step), (ev.ts, ev.ts))[0]
            span[str(step)] = (first, ev.ts)
        if ev.type == "StopAnswered":
            last_answer[str(step)] = str(ev.payload["answer"])
        elif ev.type == "StopSkipped":
            skipped.add(str(step))
    out: list[Stage] = []
    for name in names:
        s = st.steps.get(name, StepState())
        took = span[name][1] - span[name][0] if name in span else None
        tries = f"{s.turns} attempt{'s' if s.turns != 1 else ''}"
        if st.blocked is not None and st.blocked["step"] == name:
            out.append(Stage(name, "blocked", f"blocked, {tries}"))
        elif s.open_stop is not None:
            again = f", round {s.round}" if s.round > 1 else ""
            out.append(Stage(name, "waiting", f"waiting {age(now - s.open_stop[0])}{again}"))
        elif s.in_flight is not None:
            out.append(Stage(name, "running", f"running, attempt {s.in_flight['attempt']}"))
        elif s.checked is not None:
            answered = last_answer.get(name) if s.resolved else None
            if s.resolved and name in skipped:
                out.append(Stage(name, "sampled", "not asked, sampled", took))
            elif answered == "accept":
                out.append(Stage(name, "accepted", "you accepted", took))
            elif answered == "change":
                out.append(Stage(name, "changed", "you changed it", took))
            elif s.checked["source"] == "jaynes":
                out.append(Stage(name, "jaynes", "by jaynes", took))
            else:
                out.append(Stage(name, "checked", f"checked, {tries}", took))
        else:
            out.append(Stage(name, "todo", ""))
    return out


@dataclass(frozen=True)
class Totals:
    turns: int
    tokens_in: int
    tokens_out: int
    seconds: float
    stops: int
    answered: int


def totals(events: list[Event]) -> Totals:
    return Totals(turns=sum(1 for ev in events if ev.type == "StepStarted"),
                  tokens_in=sum(int(ev.payload.get("tokens_in", 0)) for ev in events if ev.type == "TurnFinished"),
                  tokens_out=sum(int(ev.payload.get("tokens_out", 0)) for ev in events if ev.type == "TurnFinished"),
                  seconds=events[-1].ts - events[0].ts if events else 0.0,
                  stops=sum(1 for ev in events if ev.type == "StopOpened"),
                  answered=sum(1 for ev in events if ev.type == "StopAnswered"))


def age(seconds: float) -> str:
    for unit, size in (("d", 86400), ("h", 3600), ("m", 60)):
        if seconds >= size:
            return f"{seconds // size:.0f}{unit}"
    return f"{max(seconds, 0):.0f}s"


def tokens(n: int) -> str:
    return f"{n / 1000:.1f}k" if n >= 1000 else str(n)


# --- rendering -------------------------------------------------------------------

CSS = """
.notice { background: var(--amber-soft); color: var(--amber); padding: 10px 16px; font-size: 13px; }
:root {
  --bg: #07090d; --panel: #0e1218; --panel-2: #121823; --line: #222a36; --line-2: #2c3646;
  --ink: #eef2f7; --dim: #93a0b2; --faint: #5d6a7c;
  --blue: #6fb4ff; --blue-soft: rgba(111, 180, 255, .12);
  --amber: #ffb35c; --amber-soft: rgba(255, 179, 92, .13);
  --green: #69dba0; --green-soft: rgba(105, 219, 160, .13);
  --red: #ff6f6f; --red-soft: rgba(255, 111, 111, .12);
  --display: "Instrument Sans", "Helvetica Neue", Arial, sans-serif;
  --mono: "IBM Plex Mono", ui-monospace, SFMono-Regular, Menlo, monospace;
}
* { box-sizing: border-box; }
html, body { margin: 0; height: 100%; background: var(--bg); color: var(--ink); font-family: var(--display); font-size: 15px; line-height: 1.5; }
a { color: inherit; text-decoration: none; }
.app { display: grid; grid-template-columns: 300px 1fr; height: 100vh; }
aside { border-right: 1px solid var(--line); display: flex; flex-direction: column; min-height: 0; overflow: auto; }
.brand { display: flex; flex-wrap: wrap; align-items: center; gap: 12px 10px; padding: 16px 18px; border-bottom: 1px solid var(--line); font-weight: 600; }
.brand span { font-family: var(--mono); font-weight: 400; font-size: 11px; color: var(--dim); }
.brand nav { flex-basis: 100%; display: flex; gap: 12px; font-family: var(--mono); font-size: 11px; font-weight: 400; color: var(--dim); white-space: nowrap; }
.brand nav a:hover, .crumbs a:hover { color: var(--ink); }
.qhead { padding: 14px 18px 8px; font-family: var(--mono); font-size: 11px; color: var(--dim); text-transform: uppercase; letter-spacing: .08em; display: flex; justify-content: space-between; }
.item { padding: 12px 18px; border-left: 2px solid transparent; display: grid; gap: 6px; }
.item:hover { background: rgba(255, 255, 255, .025); }
.item.on { border-left-color: var(--amber); padding-bottom: 16px; }
.item.on:hover { background: none; }
.item.on.blocked { border-left-color: var(--red); }
.item.on.idle { border-left-color: var(--blue); }
.strip { display: flex; gap: 3px; margin-top: 2px; }
.strip i { width: 14px; height: 4px; border-radius: 2px; background: var(--green); display: block; }
.strip i.waiting { background: var(--amber); }
.strip i.blocked { background: var(--red); }
.strip i.running { background: var(--blue); }
.strip i.todo { background: var(--line-2); }
.strip i.sampled { background: none; box-shadow: inset 0 0 0 1px var(--green); }
.stages { list-style: none; margin: 8px 0 0; padding: 0; }
.stages li { display: grid; grid-template-columns: 14px 1fr; gap: 10px; padding: 5px 0; position: relative; }
.stages li::before { content: ""; position: absolute; left: 6px; top: 18px; bottom: -6px; width: 1px; background: var(--line-2); }
.stages li:last-child::before { display: none; }
.stages .dot { width: 9px; height: 9px; border-radius: 50%; margin: 6px 0 0 2px; background: var(--green); position: relative; z-index: 1; }
.stages .waiting .dot { background: var(--amber); box-shadow: 0 0 0 4px var(--amber-soft); }
.stages .blocked .dot { background: var(--red); box-shadow: 0 0 0 4px var(--red-soft); }
.stages .running .dot { background: var(--blue); box-shadow: 0 0 0 4px var(--blue-soft); }
.stages .todo .dot { background: var(--bg); box-shadow: inset 0 0 0 1px var(--line-2); }
.stages .sampled .dot { background: var(--bg); box-shadow: inset 0 0 0 1.5px var(--green); }
.stages .accepted .dot, .stages .changed .dot { box-shadow: 0 0 0 3px var(--green-soft); }
.stages .r { display: flex; justify-content: space-between; gap: 8px; }
.stages b { font-family: var(--mono); font-size: 12px; font-weight: 500; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.stages em { font-style: normal; font-family: var(--mono); font-size: 11px; color: var(--faint); }
.stages small { font-family: var(--mono); font-size: 11px; color: var(--dim); display: block; }
.stages .todo b { color: var(--faint); font-weight: 400; }
.stages .waiting b, .stages .waiting small { color: var(--amber); }
.stages .blocked b, .stages .blocked small { color: var(--red); }
.foot { margin-top: auto; padding: 14px 18px; border-top: 1px solid var(--line); font-family: var(--mono); font-size: 11px; color: var(--dim); display: grid; grid-template-columns: 1fr auto; gap: 4px 10px; }
.foot b { color: var(--ink); font-weight: 500; text-align: right; }
.item .t { font-size: 14px; }
.item .m { font-family: var(--mono); font-size: 11px; color: var(--dim); display: flex; gap: 8px; align-items: center; white-space: nowrap; }
.item .m .s { overflow: hidden; text-overflow: ellipsis; }
.empty-q { padding: 12px 18px; font-family: var(--mono); font-size: 12px; color: var(--faint); }
.chip { font-family: var(--mono); font-size: 10.5px; padding: 2px 7px; border-radius: 4px; white-space: nowrap; }
.amber { color: var(--amber); background: var(--amber-soft); }
.red { color: var(--red); background: var(--red-soft); }
.green { color: var(--green); background: var(--green-soft); }
.blue { color: var(--blue); background: var(--blue-soft); }
main { display: flex; flex-direction: column; min-height: 0; min-width: 0; }
.crumbs { padding: 16px 32px; border-bottom: 1px solid var(--line); font-family: var(--mono); font-size: 12px; color: var(--dim); display: flex; gap: 10px; align-items: center; }
.crumbs b { color: var(--ink); font-weight: 500; }
.crumbs .r { margin-left: auto; }
.body { flex: 1; overflow: auto; padding: 28px 32px 48px; }
.body > * { max-width: 960px; }
.body > * + * { margin-top: 26px; }
.body.wide > * { max-width: none; }
h1 { font-size: 26px; font-weight: 500; letter-spacing: -0.02em; line-height: 1.2; margin: 0; text-wrap: balance; }
h2 { font-size: 18px; font-weight: 500; margin: 0; }
.sub { color: var(--dim); margin-top: 8px; }
.label { font-family: var(--mono); font-size: 11px; color: var(--dim); text-transform: uppercase; letter-spacing: .08em; margin-bottom: 10px; display: flex; justify-content: space-between; gap: 12px; }
.label span { text-transform: none; letter-spacing: 0; color: var(--faint); }
.box { border: 1px solid var(--line); border-radius: 6px; overflow-x: auto; }
table { width: 100%; border-collapse: collapse; }
th { text-align: left; font-family: var(--mono); font-size: 10.5px; font-weight: 400; color: var(--faint); padding: 9px 16px; }
td { border-top: 1px solid var(--line); padding: 12px 16px; vertical-align: top; }
td.m, td .m, .m { font-family: var(--mono); font-size: 11.5px; color: var(--dim); }
td.id { font-family: var(--mono); font-size: 12px; color: var(--blue); white-space: nowrap; }
.kv td:first-child { font-family: var(--mono); font-size: 11.5px; color: var(--dim); width: 160px; }
ul.plain { margin: 0; padding-left: 18px; }
pre { margin: 0; font-family: var(--mono); font-size: 12px; white-space: pre-wrap; word-break: break-word; color: var(--dim); }
.diff > span { display: block; padding: 0 16px; }
.diff > span > i { font-style: normal; display: inline-block; width: 2ch; color: var(--faint); user-select: none; }
.diff > .add, .diff > .del { color: var(--ink); }
.diff > .add { background: var(--green-soft); }
.diff > .add > i { color: var(--green); }
.diff > .del { background: var(--red-soft); }
.diff > .del > i { color: var(--red); }
.diff > .hunk { color: var(--blue); }
.diff > .file { color: var(--ink); font-weight: 600; margin-top: 10px; }
.diff > .file:first-child { margin-top: 0; }
.diff > .meta { color: var(--faint); }
.diff .k, .diff .kd, .diff .kn, .diff .kr, .diff .kc, .diff .kp, .diff .ow, .diff .na { color: #c4a7ff; }
.diff .kt, .diff .nb, .diff .bp, .diff .nd { color: #ff9e64; }
.diff .s, .diff .s1, .diff .s2, .diff .sa, .diff .sb, .diff .sc, .diff .sd, .diff .se, .diff .sh, .diff .si,
.diff .sr, .diff .ss, .diff .sx, .diff .dl { color: #f2c48d; }
.diff .c, .diff .c1, .diff .cm, .diff .cp, .diff .cs, .diff .ch, .diff .cpf { color: #6b7789; font-style: italic; }
.diff .m, .diff .mi, .diff .mf, .diff .mh, .diff .mo, .diff .mb, .diff .il { color: #7cc4ff; }
.diff .nf, .diff .fm, .diff .nc, .diff .nn, .diff .ne { color: #6fe0d0; }
.diff .nt { color: #ff9fc0; }
.diff .gh, .diff .gu { color: var(--blue); font-weight: 600; }
.field + .field { margin-top: 18px; }
.checks { display: flex; gap: 8px; flex-wrap: wrap; }
.tiles { display: grid; grid-template-columns: repeat(3, 1fr); gap: 1px; background: var(--line); border: 1px solid var(--line); border-radius: 6px; overflow: hidden; }
.tiles div { background: var(--panel); padding: 14px 16px; }
.tiles .n { font-size: 22px; font-weight: 500; letter-spacing: -0.02em; font-variant-numeric: tabular-nums; }
.tiles .n small { font-size: 14px; color: var(--dim); font-weight: 400; margin-left: 4px; letter-spacing: 0; }
.tiles p { margin: 2px 0 0; font-family: var(--mono); font-size: 11px; color: var(--dim); }
.bar { border-top: 1px solid var(--line); background: rgba(7, 9, 13, .96); padding: 14px 32px; display: grid; gap: 10px; }
.bar .row { display: flex; gap: 10px; align-items: center; }
.bar input[type=text] { flex: 1; min-width: 0; background: var(--panel); border: 1px solid var(--line-2); border-radius: 4px; color: var(--ink); font: inherit; font-size: 14px; padding: 10px 12px; }
.bar select { background: var(--panel); border: 1px solid var(--line-2); border-radius: 4px; color: var(--ink); font-family: var(--mono); font-size: 12.5px; padding: 10px 8px; }
.bar textarea { width: 100%; min-height: 160px; background: var(--panel); border: 1px solid var(--line-2); border-radius: 4px; color: var(--ink); font-family: var(--mono); font-size: 12.5px; padding: 10px 12px; }
.btn { font-family: var(--mono); font-size: 12.5px; padding: 10px 14px; border-radius: 4px; border: 1px solid var(--line-2); color: var(--ink); background: var(--panel); cursor: pointer; white-space: nowrap; }
.btn kbd { font-family: var(--mono); font-size: 10px; color: var(--faint); margin-left: 6px; }
.btn.solid { background: var(--green); border-color: var(--green); color: #04150c; font-weight: 500; }
.btn.solid kbd { color: #0b3a22; }
.err { color: var(--red); font-family: var(--mono); font-size: 12px; white-space: pre-line; }
.hidden { display: none; }
.empty { margin: auto; text-align: center; color: var(--dim); display: grid; gap: 6px; }
.empty h1 { color: var(--ink); }
.filters { display: flex; gap: 8px; flex-wrap: wrap; font-family: var(--mono); font-size: 11px; }
.filters a { padding: 3px 8px; border: 1px solid var(--line-2); border-radius: 999px; color: var(--dim); }
.filters a.on { color: var(--ink); border-color: var(--blue); }
.form { display: grid; gap: 20px; max-width: 640px; }
.form label { display: grid; }
.form select, .form input[type=text], .form textarea { background: var(--panel); border: 1px solid var(--line-2); border-radius: 4px; color: var(--ink); font: inherit; font-size: 14px; padding: 10px 12px; }
.form textarea { min-height: 96px; resize: vertical; }
.sw { display: inline-block; width: 12px; height: 12px; border-radius: 3px; margin-right: 7px; vertical-align: -1px; box-shadow: inset 0 0 0 1px rgba(255, 255, 255, .18); }
.previews { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }
.previews figure { margin: 0; border: 1px solid var(--line); border-radius: 6px; overflow: hidden; display: grid; grid-template-rows: auto 1fr; }
.previews figcaption { display: flex; justify-content: space-between; padding: 8px 12px; border-bottom: 1px solid var(--line); font-family: var(--mono); font-size: 11px; color: var(--dim); }
.previews iframe { width: 100%; height: 520px; border: 0; background: #fff; }
.body > .wide { max-width: none; }
.pictures { display: grid; grid-template-columns: repeat(auto-fill, minmax(260px, 1fr)); gap: 12px; }
.pictures a { border: 1px solid var(--line); border-radius: 6px; overflow: hidden; display: grid; }
.pictures img { width: 100%; height: 220px; object-fit: cover; object-position: top; display: block; background: var(--panel); }
.pictures span { padding: 8px 12px; border-top: 1px solid var(--line); font-family: var(--mono); font-size: 11px; color: var(--dim); }
.btn.go { background: var(--blue); border-color: var(--blue); color: #04101f; font-weight: 500; justify-self: start; }
"""

ICON = ('<link rel="icon" href="data:image/svg+xml,%3Csvg xmlns=%22http://www.w3.org/2000/svg%22 viewBox=%220 0 32 32%22%3E%3Crect width=%2232%22 height=%2232%22 rx=%227%22 fill=%22%2307090d%22/%3E%3Cpath d=%22M11.5 14.6 20 9.6M11.5 17.4 20 22.4%22 stroke=%22%236fb4ff%22 stroke-width=%222%22/%3E%3Ccircle cx=%229%22 cy=%2216%22 r=%224.2%22 fill=%22%236fb4ff%22/%3E%3Ccircle cx=%2222.5%22 cy=%228.5%22 r=%223.2%22 fill=%22%23eef2f7%22/%3E%3Ccircle cx=%2222.5%22 cy=%2223.5%22 r=%223.2%22 fill=%22%23eef2f7%22/%3E%3C/svg%3E">')

FONTS = ('<link rel="preconnect" href="https://fonts.googleapis.com">'
         '<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>'
         '<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Instrument+Sans:wght@400;500;600'
         '&family=IBM+Plex+Mono:wght@400;500&display=swap">')

# Reload when the log grows, unless the person is typing; keys answer the open stop.
SCRIPT = """
<script>
const seq = %d;
const busy = () => { const f = document.activeElement; return f && ["INPUT", "TEXTAREA", "SELECT"].includes(f.tagName); };
setInterval(async () => {
  try {
    const r = await fetch("/seq"); const now = Number(await r.text());
    if (now !== seq && !busy() && (document.querySelector("#editor") || {}).className !== "") location.reload();
  } catch (e) {}
}, 2500);
const form = document.querySelector("#answer");
if (form) {
  const note = form.querySelector("[name=note]"), hint = form.querySelector("#hint"), editor = form.querySelector("#editor");
  const send = (a) => {
    if (!form.querySelector("[data-a=" + a + "]")) return;
    if ((a === "return" || a === "context") && !note.value.trim()) { hint.textContent = a + " needs a note for the agent"; note.focus(); return; }
    if (a === "change" && editor.className === "hidden") { editor.className = ""; editor.querySelector("textarea").focus(); return; }
    form.querySelector("[name=answer]").value = a; form.submit();
  };
  form.querySelectorAll("[data-a]").forEach(b => b.addEventListener("click", () => send(b.dataset.a)));
  document.addEventListener("keydown", (ev) => {
    if (busy() || ev.metaKey || ev.ctrlKey || ev.altKey) return;
    const a = {a: "accept", c: "change", r: "return", x: "context"}[ev.key.toLowerCase()];
    if (a) { ev.preventDefault(); send(a); }
  });
}
</script>
"""


def page(title: str, sidebar: str, main: str, seq: int) -> str:
    return (f'<!doctype html><html lang="en"><head><meta charset="utf-8">'
            f'<meta name="viewport" content="width=device-width, initial-scale=1"><title>{esc(title)}</title>{ICON}'
            f'{FONTS}<style>{CSS}</style></head><body><div class="app">{sidebar}<main>{main}</main></div>'
            f'{SCRIPT % seq}</body></html>')


def render_value(v: Any) -> str:
    """Generic, readable rendering of a step output: lists of records become tables."""
    if isinstance(v, list) and v and all(isinstance(x, dict) for x in cast(list[Any], v)):
        rows = cast(list[dict[str, Any]], v)
        keys = list(dict.fromkeys(k for r in rows for k in r))
        head = "".join(f"<th>{esc(k)}</th>" for k in keys)
        body = "".join("<tr>" + "".join(_cell(r.get(k), i == 0) for i, k in enumerate(keys)) + "</tr>" for r in rows)
        return f'<div class="box"><table><tr>{head}</tr>{body}</table></div>'
    if isinstance(v, list):
        items = cast(list[Any], v)
        return '<ul class="plain">' + "".join(f"<li>{render_value(x)}</li>" for x in items) + "</ul>"
    if isinstance(v, dict):
        fields = cast(dict[str, Any], v)
        if all(not isinstance(x, (dict, list)) for x in fields.values()):
            return '<div class="box"><table class="kv">' + "".join(
                f"<tr><td>{esc(k)}</td><td>{render_value(x)}</td></tr>" for k, x in fields.items()) + "</table></div>"
        return "".join(f'<div class="field"><div class="label">{esc(k)}</div>{render_value(x)}</div>' for k, x in fields.items())
    if v is None:
        return '<span class="m">none</span>'
    return swatch(v)


HEX = re.compile(r"#[0-9a-fA-F]{3}(?:[0-9a-fA-F]{3})?(?:[0-9a-fA-F]{2})?")


def swatch(v: Any) -> str:
    """A scalar as text, with a colour sample when it is a hex colour."""
    text = str(v)
    return (f'<span class="sw" style="background:{text}"></span>{esc(text)}' if isinstance(v, str) and HEX.fullmatch(text)
            else esc(text))


def _cell(v: Any, first: bool) -> str:
    if first and isinstance(v, (str, int)) and len(str(v)) <= 8:
        return f'<td class="id">{esc(str(v))}</td>'
    if isinstance(v, (dict, list)):
        return f"<td><pre>{esc(json.dumps(v, indent=1, ensure_ascii=False))}</pre></td>"
    if isinstance(v, (int, float)) or (isinstance(v, str) and len(v) <= 24):
        return f'<td class="m">{swatch(v)}</td>'
    return f"<td>{render_value(v)}</td>"


def stage_list(items: list[Stage]) -> str:
    """The open queue item's stages, top to bottom, each with how it was released and how long it took."""
    rows = "".join(
        f'<li class="{s.state}"><span class="dot"></span><div><div class="r"><b>{esc(s.name)}</b>'
        f'<em>{"" if s.seconds is None else "under 1s" if s.seconds < 1 else age(s.seconds)}</em></div>'
        + (f"<small>{esc(s.detail)}</small>" if s.detail else "") + "</div></li>"
        for s in items)
    return f'<ul class="stages">{rows}</ul>'


def stage_strip(items: list[Stage]) -> str:
    """One mark per stage, for queue items that are not open."""
    return '<div class="strip">' + "".join(
        f'<i class="{s.state}" title="{esc(s.name)}"></i>' for s in items) + "</div>"


# --- the application ---------------------------------------------------------------

@dataclass
class Shared:
    """What outlives a request: the answer token and key, imported roles, and when each open stop was first shown."""

    token: str
    key: str
    idle_after: float = 10.0
    roles: dict[str, Role] = field(default_factory=dict[str, Role])
    first_seen: dict[tuple[str, str, float], float] = field(default_factory=dict[tuple[str, str, float], float])
    history: tuple[float, Report | None, str] | None = None  # when it was read, the report, the policy used
    previews: Previews = field(default_factory=Previews)


class Review:
    """One request's view of the log. SQLite connections stay in their thread, so each request makes one."""

    def __init__(self, home: Path, shared: Shared) -> None:
        self.c = Conductor(home)
        self.shared = shared

    def seq(self) -> int:
        return self.c.store.max_seq()

    def queue(self) -> list[Waiting]:
        return self.c.queue(idle_after=self.shared.idle_after)

    def role_of(self, run_id: str) -> Role | None:
        """The role a run was started with, if its module can be imported here. Edits need a restart."""
        started = next(iter(self.c.store.events("RunStarted", run_id=run_id)), None)
        return self.import_role(str(started.payload.get("ref", "")) if started else "")

    def import_role(self, ref: str) -> Role | None:
        if ref not in self.shared.roles:
            module, _, attr = ref.partition(":")
            try:
                if ref.endswith(".toml"):
                    found = role_file.load(Path(ref))
                else:
                    found = getattr(importlib.import_module(module), attr) if module and attr else None
            except Exception:
                return None  # not cached, so a fixed import works without a restart
            if not isinstance(found, Role):
                return None
            self.shared.roles[ref] = found
        return self.shared.roles[ref]

    def names(self, role: Role | None, events: list[Event]) -> list[str]:
        """The run's steps in role order, or in the order they appear when the role cannot be imported."""
        if role is not None:
            return [s.name for s in role.steps]
        return list(dict.fromkeys(str(ev.payload["step"]) for ev in events if "step" in ev.payload))

    # sidebar ----------------------------------------------------------------

    def sidebar(self, selected: tuple[str, str] | None) -> str:
        items = self.queue()
        now = self.c.clock()
        rows: list[str] = []
        foot = ""
        for w in items:
            on = selected == (w.run_id, w.step)
            events = list(self.c.store.events(run_id=w.run_id))
            run_stages = stages(self.names(self.role_of(w.run_id), events), events, project(events), now)
            chip = {"stop": '<span class="chip amber">stop</span>', "blocked": '<span class="chip red">blocked</span>',
                    "idle": '<span class="chip blue">idle</span>'}[w.kind]
            rows.append(f'<a class="item{" on" if on else ""} {w.kind}" href="/stop/{q(w.run_id)}/{q(w.step)}">'
                        f'<div class="t">{esc(w.task)}</div>'
                        f'<div class="m">{chip}<span class="s">{esc(w.step)}</span>'
                        f'<span>{age(now - w.since)}</span></div>'
                        f'{stage_list(run_stages) if on else stage_strip(run_stages)}</a>')
            if on:
                t = totals(events)
                foot = (f'<div class="foot"><span>started</span><b>{age(now - events[0].ts)} ago</b>'
                        f'<span>turns</span><b>{t.turns}</b>'
                        f'<span>tokens</span><b>{tokens(t.tokens_in)} in, {tokens(t.tokens_out)} out</b>'
                        f'<span>you answered</span><b>{t.answered} of {t.stops} stops</b></div>')
        body = "".join(rows) or '<div class="empty-q">Nothing is waiting.</div>'
        return ('<aside><div class="brand"><a href="/">Jaynes</a> <span>review</span><nav>'
                '<a href="/">Queue</a><a href="/new">New run</a><a href="/roles">Roles</a><a href="/history">History</a><a href="/log">Log</a><a href="/setup">Setup</a></nav></div>'
                f'<div class="qhead"><span>Waiting</span><span>{len(items)}</span></div>{body}{foot}</aside>')

    # pages ------------------------------------------------------------------

    def queue_page(self) -> str:
        items = self.queue()
        if items:
            return self.stop_page(items[0].run_id, items[0].step)
        main = ('<div class="crumbs"><b>Queue</b></div><div class="body" style="display:flex">'
                '<div class="empty"><h1>Nothing is waiting</h1><p>Runs that stop or block will appear here.</p>'
                '<p><a class="btn go" href="/new">Start a run</a></p></div></div>')
        return page("Jaynes review", self.sidebar(None), main, self.seq())

    def stop_page(self, run_id: str, step: str, error: str = "", fields: dict[str, str] | None = None) -> str:
        w = next((x for x in self.queue() if (x.run_id, x.step) == (run_id, step)), None)
        if w is None:
            return self.run_page(run_id)
        events = list(self.c.store.events(run_id=run_id))
        st = project(events)
        role = self.role_of(run_id)
        target = next((s for s in role.steps if s.name == step), None) if role else None
        s = st.steps.get(step, StepState())
        crumbs = (f'<div class="crumbs"><span>{esc(w.role)}</span>/<a href="/run/{q(run_id)}">{esc(w.task)}</a>/'
                  f'<b>{esc(step)}</b><span class="r">{esc(self._where(s))}</span></div>')
        parts: list[str] = []
        if w.kind == "idle":
            log = self.c.home / "runs" / run_id / "continue.log"
            tail = log.read_text(errors="replace")[-1500:] if log.exists() else ""
            parts.append(f"<div><h1>This run is not running</h1>"
                         f'<div class="sub">No process holds it and it is not waiting for you; its last process '
                         f"stopped or failed. Continue starts <code>jaynes run</code> for it.</div></div>")
            if tail:
                parts.append(f'<div><div class="label">Last output</div><div class="box"><pre style="padding:12px 16px">'
                             f"{esc(tail)}</pre></div></div>")
            form = (f'<form class="bar" method="post" action="/continue"><input type="hidden" name="token" '
                    f'value="{esc(self.shared.token)}"><input type="hidden" name="run" value="{esc(run_id)}">'
                    f'<div class="row"><span style="flex:1"></span><button class="btn solid">Continue</button></div></form>')
            main = crumbs + f'<div class="body">{"".join(parts)}</div>' + form
            return page(f"Idle: {run_id}", self.sidebar((run_id, step)), main, self.seq())
        if w.kind == "blocked":
            parts.append(f"<div><h1>This run is blocked at {esc(step)}</h1>"
                         f'<div class="sub">Every attempt of the round was rejected. The last reasons:</div></div>')
            parts.append('<div class="box"><table>' + "".join(
                f'<tr><td class="m" style="color:var(--red)">rejected</td><td>{esc(r)}</td></tr>' for r in w.reasons)
                + "</table></div>")
            parts.append(self._turns(events, step, s.round))
            main = crumbs + f'<div class="body">{"".join(parts)}</div>'
            return page(f"Blocked: {step}", self.sidebar((run_id, step)), main, self.seq())

        value = self._value(s, target)
        pure = target is not None and target.output is None  # a stop over what earlier steps made
        needs = [n.name for n in target.needs] if target else []
        default = f"Do you accept the result of {', '.join(needs)}?" if pure and needs else f"Is the output of {step} right?"
        question = w.question or default
        rejected = [ev for ev in events if ev.type == "TurnRejected" and ev.payload["step"] == step
                    and self._round_of(events, ev) == s.round]
        sub = (f'<div class="sub">{len(rejected)} earlier attempt{"s" if len(rejected) != 1 else ""} '
               f'rejected by Jaynes before you saw it: {esc(rejected[-1].payload["reasons"][0])}</div>'
               if rejected and rejected[-1].payload.get("reasons") else "")
        if s.note:
            sub += f'<div class="sub">You returned round {s.round - 1} with: {esc(s.note)}</div>'
        parts.append(f"<div><h1>{esc(question)}</h1>{sub}</div>")
        parts.append(self._floor_part(s))
        parts.append(self._project_part(run_id, st, role))
        parts.append(self._pictures_part(run_id, st, target))
        if pure and target is not None:
            for need in target.needs:
                parts.append(self._output(f"From {need.name}", self._value(st.steps.get(need.name, StepState()), need)))
        else:
            parts.append(self._output("Output", value, f"round {s.round}"))
            if s.previous:
                parts.append(self._output("Before", self._value_of(s.previous, target), f"round {s.round - 1}, returned"))
        if target is not None and target.checks:
            # a Checked witness means every check passed; the names come from the role's code
            check_names = ", ".join(esc(getattr(c, "__name__", "check")) for c in target.checks)
            parts.append(f'<div><div class="label">Checks</div><div class="checks">'
                         f'<span class="chip green">passed: {check_names}</span></div></div>')
        if s.open_stop is not None:
            self.shared.first_seen.setdefault((run_id, step, s.open_stop[0]), time.time())
        if not pure:
            parts.append(self._turns(events, step, s.round))
        parts.append(self._tiles(s, target))
        main = (crumbs + f'<div class="body">{"".join(parts)}</div>'
                + self._answer_bar(run_id, step, value, error, fields, editable=not pure,
                                   earlier=self._earlier(role, step),
                                   evidence=str(s.open_stop[1].get("evidence") or "") if s.open_stop else ""))
        return page(f"Stop: {step}", self.sidebar((run_id, step)), main, self.seq())

    def _project_part(self, run_id: str, st: RunState, role: Role | None) -> str:
        """What the run changed in its project so far, and the preview before and after, when there is one."""
        info = (st.started or {}).get("project")
        changed = sorted({f for x in st.steps.values() if x.checked for f in x.checked.get("changed", [])})
        if not info or not changed:
            return ""
        out = ""
        if (st.started or {}).get("preview") and role is not None:
            frames = "".join(
                f'<figure><figcaption><span>{label}</span><a href="/preview/{q(run_id)}/{which}" target="_blank">'
                f'open</a></figcaption><iframe src="/preview/{q(run_id)}/{which}" loading="lazy"></iframe></figure>'
                for label, which in (("Before", "before"), ("After", "after")))
            out += f'<div class="wide"><div class="label">Preview <span>your preview command, on copies of the project</span></div><div class="previews">{frames}</div></div>'
        files = "".join(f'<tr><td class="m">{esc(f)}</td></tr>' for f in changed)
        out += (f'<div><div class="label">Changed in the project <span>{esc(str(info["root"]))}, from '
                f'{esc(str(info["base"])[:8])}</span></div><div class="box"><table>{files}</table></div></div>')
        for step, patch in self.c.patches(role, st) if role is not None else []:
            text = patch.decode(errors="replace")
            if len(text) > 20000:  # never cut silently: what is not shown here must still be read
                text = text[:20000] + f"\n\n[{len(text) - 20000} more characters not shown: read the branch before merging it]"
            out += (f'<div class="wide"><div class="label">Changes from {esc(step)}</div><div class="box">'
                    f'<pre class="diff" style="padding:12px 0">{diff_html(text)}</pre></div></div>')
        return out

    def _floor_part(self, s: StepState) -> str:
        """The diff of the paths this step changed that tools run later, which is why the stop is a floor."""
        later = set((s.checked or {}).get("floor") or [])
        patch = (s.checked or {}).get("patch")
        if not later or not patch:
            return ""
        text = (self.c.home / "objects" / f"{patch}.patch").read_bytes().decode(errors="replace")
        shown = "".join(d for path, d in repo.diffs(text).items() if path in later)
        if len(shown) > 20000:  # never cut silently: what is not shown here must still be read
            shown = shown[:20000] + f"\n\n[{len(shown) - 20000} more characters not shown: read the branch before merging it]"
        return (f'<div class="wide"><div class="label" style="color:var(--red)">These files run later, outside any '
                f'box <span>editors, agents, git, direnv, package managers or CI may run or obey them; read every '
                f'line</span></div><div class="box"><pre class="diff" style="padding:12px 0"><span>{esc(", ".join(sorted(later)))}'
                f'</span><span> </span>{diff_html(shown)}</pre></div></div>')

    def _pictures_part(self, run_id: str, st: RunState, target: Step[Any] | None) -> str:
        """The pictures the stop's step and the steps it needs left, such as references."""
        out = ""
        for x in [*target.needs, target] if target else []:
            files = self._files(st, x.name)
            pics = "".join(f'<a href="/file/{q(run_id)}/{q(x.name)}/{q(rel)}" target="_blank"><img src="/file/'
                           f'{q(run_id)}/{q(x.name)}/{q(rel)}" alt="{esc(rel)}" loading="lazy"><span>{esc(rel)}</span></a>'
                           for rel in files if Path(rel).suffix.lower() in PICTURES)
            if pics:
                out += (f'<div class="wide"><div class="label">Pictures from {esc(x.name)}</div>'
                        f'<div class="pictures">{pics}</div></div>')
        return out

    @staticmethod
    def _files(st: RunState, step: str) -> dict[str, str]:
        """The files a step's checked output carries: path to object."""
        checked = st.steps.get(step, StepState()).checked or {}
        return cast(dict[str, str], checked.get("files") or {})

    def picture(self, run_id: str, step: str, rel: str) -> tuple[bytes, str] | None:
        """A picture a step of the run left, with its type; other files are not served."""
        kind = PICTURES.get(Path(rel).suffix.lower())
        files = self._files(project(self.c.store.events(run_id=run_id)), step)
        if kind is None or rel not in files:
            return None
        return (self.c.home / "objects" / str(files[rel])).read_bytes(), kind

    def preview(self, run_id: str, which: str) -> tuple[int, str]:
        """Redirects to the run's preview once it answers; until then, a page that reloads itself."""
        st = project(self.c.store.events(run_id=run_id))
        info, spec = (st.started or {}).get("project"), (st.started or {}).get("preview")
        role = self.role_of(run_id)
        if not info or not spec or role is None or which not in ("before", "after"):
            return 404, "no preview for this run"
        patches = [] if which == "before" else [p for _, p in self.c.patches(role, st)]
        key = f"{run_id}-{which}-{hashlib.sha256(b''.join(patches)).hexdigest()[:12]}"
        folder = self.c.home / "runs" / run_id / "preview" / key

        def prepare() -> Path:
            if folder.exists():
                shutil.rmtree(folder)
            folder.parent.mkdir(parents=True, exist_ok=True)
            repo.prepare(Path(info["root"]), str(info["base"]), patches, folder,
                         cache=self.c.home / "runs" / run_id / "base")
            return folder

        try:
            p = self.shared.previews.ensure(key, prepare, Path(info["root"]), str(spec["command"]),
                                            str(spec.get("path", "/")), hidden=(self.c.home,),
                                            ports=tuple(int(x) for x in spec.get("ports", [])))
        except sandbox.Unavailable as err:
            state, text = "failed", str(err)
        else:
            state, text = Previews.state(p)
        if state == "ready":
            return 302, text
        head = "Starting the preview" if state == "starting" else "The preview failed"
        refresh = '<meta http-equiv="refresh" content="1">' if state == "starting" else ""
        return 200, (f'<!doctype html><html><head><meta charset="utf-8">{refresh}<style>body{{margin:0;padding:20px;'
                     f'font:13px/1.5 ui-monospace,Menlo,monospace;color:#5d6a7c;background:#fff}}pre{{white-space:pre-wrap}}'
                     f'</style></head><body><b>{head}</b><pre>{esc(text)}</pre></body></html>')

    def run_page(self, run_id: str) -> str:
        events = list(self.c.store.events(run_id=run_id))
        log = self.c.home / "runs" / run_id / "continue.log"
        if not events and log.is_file():  # started from the page; the page reloads on its first event
            out = log.read_text(errors="replace")[-4000:]
            main = (f'<div class="crumbs"><b>Starting</b><span class="r">{esc(run_id)}</span></div><div class="body">'
                    + (f'<div><div class="label">Output</div><div class="box" style="padding:14px 16px"><pre>{esc(out)}'
                       '</pre></div></div>' if out else '<div class="sub">Waiting for the run\'s first event.</div>')
                    + '</div>')
            return page(f"Run {run_id}", self.sidebar(None), main, self.seq())
        if not events:
            return self.not_found(f"No run {run_id}")
        st = project(events)
        role = self.role_of(run_id)
        names = self.names(role, events)
        started = st.started or {}
        state = ("finished" if st.finished else "blocked" if st.blocked else
                 "waiting" if any(x.open_stop for x in st.steps.values()) else
                 "paused" if st.paused_until and st.paused_until > self.c.clock() else
                 "running" if self.c._held(run_id) else "idle")  # pyright: ignore[reportPrivateUsage]
        t = totals(events)
        crumbs = (f'<div class="crumbs"><a href="/role/{q(str(started.get("role", "")))}">{esc(str(started.get("role", "")))}</a>/'
                  f'<b>{esc(str(started.get("task", "")))}</b><span class="r">{esc(run_id)}</span></div>')
        rows = "".join(self._event_row(ev) for ev in events)
        made = (f'<div class="box"><table class="kv"><tr><td>branch</td><td class="m">{esc(str(st.branch["branch"]))} '
                f'in {esc(str(st.branch["root"]))}, at {esc(str(st.branch["commit"])[:8])}</td></tr></table></div>'
                if st.branch else "")
        body = (stage_list(stages(names, events, st, self.c.clock())) + made
                + f'<div class="tiles"><div><div class="n">{esc(state)}</div><p>run</p></div>'
                f'<div><div class="n">{t.turns}<small>turns</small></div><p>{tokens(t.tokens_in)} tokens in, {tokens(t.tokens_out)} out</p></div>'
                f'<div><div class="n">{age(t.seconds)}</div><p>from start to last event</p></div></div>'
                f'<div><div class="label">Events <span><a href="/log?run={q(run_id)}">raw log</a></span></div>'
                f'<div class="box"><table><tr><th>time</th><th>event</th><th>step</th><th>detail</th></tr>{rows}</table></div></div>')
        main = crumbs + f'<div class="body">{body}</div>'
        return page(f"Run {run_id}", self.sidebar(None), main, self.seq())

    def known_roles(self) -> dict[str, dict[str, str]]:
        """Roles a run can be started with and their runtime overrides: those that come with Jaynes, then each one run."""
        known = {k: dict(v) for k, v in BUILT_IN.items()}
        for ev in self.c.store.events("RunStarted"):
            if "ref" in ev.payload:
                known[str(ev.payload["ref"])] = {str(k): str(v) for k, v in dict(ev.payload["runtimes"]).items()}
        return known

    @staticmethod
    def default_project() -> str:
        """The folder the page was opened from, when it is in a git repository."""
        try:
            return str(repo.resolve(Path.cwd())[0])
        except repo.ProjectError:
            return ""

    def new_page(self, error: str = "", fields: dict[str, str] | None = None) -> str:
        f = fields or {}
        chosen = f.get("role", DEMO)
        options = "".join(f'<option value="{esc(ref)}"{" selected" if ref == chosen else ""}>'
                          f'{esc(f"{r.name} ({ref})" if (r := self.import_role(ref)) else ref)}</option>'
                          for ref in self.known_roles())
        main = ('<div class="crumbs"><b>New run</b></div><div class="body">'
                f'<form class="form" method="post" action="/start"><input type="hidden" name="token" value="{self.shared.token}">'
                f'<label><span class="label">Role</span><select name="role">{options}</select></label>'
                f'<label><span class="label">Or a role in this folder</span>'
                f'<input type="text" name="ref" placeholder="my_role.toml or my_role:ROLE" value="{esc(f.get("ref", ""))}">'
                f'<span class="m" style="margin-top:6px">{esc(str(Path.cwd()))}</span></label>'
                f'<label><span class="label">Task</span><textarea name="task" placeholder="Redesign the People screen">'
                f'{esc(f.get("task", ""))}</textarea></label>'
                f'<label><span class="label">Project folder</span>'
                f'<input type="text" name="project" placeholder="/path/to/repository" '
                f'value="{esc(f.get("project", self.default_project()))}"><span class="m" style="margin-top:6px">'
                f'A git repository. Steps get copies of its last commit; accepted changes come back as a branch.'
                f'</span></label>'
                f'<label><span class="label">Preview command</span>'
                f'<input type="text" name="preview" placeholder="npm run dev -- --port {{port}}" '
                f'value="{esc(f.get("preview", ""))}"><span class="m" style="margin-top:6px">Optional. Serves the '
                f'project on {{port}}; stops show it before and after.</span></label>'
                f'<label><span class="label">Preview path</span>'
                f'<input type="text" name="preview_path" placeholder="/" value="{esc(f.get("preview_path", ""))}">'
                f'</label>'
                f'<label><span class="label">Local ports the preview may reach</span>'
                f'<input type="text" name="preview_ports" placeholder="8789" '
                f'value="{esc(f.get("preview_ports", ""))}"><span class="m" style="margin-top:6px">Optional. The '
                f'preview runs the agent\'s changes in a box with no network; list the ports of local servers it '
                f'needs, such as a backend.</span></label>'
                + (f'<div class="err">{esc(error)}</div>' if error else '')
                + '<button class="btn go" type="submit">Start the run</button></form></div>')
        return page("New run", self.sidebar(None), main, self.seq())

    def history_page(self) -> str:
        if self.shared.history is None or time.time() - self.shared.history[0] > 300:  # reading takes seconds
            policy = load_policy(find_policy(str(Path.cwd())) or install.BUILT_IN_POLICY)
            self.shared.history = (time.time(), report(policy, past_calls(days=30)), policy.source)
        _, r, source = self.shared.history
        crumbs = '<div class="crumbs"><b>History</b><span class="r">last 30 days</span></div>'
        if r is None:
            main = crumbs + ('<div class="body" style="display:flex"><div class="empty"><h1>No tool calls found</h1>'
                             '<p>Claude Code transcripts from the last 30 days appear here.</p></div></div>')
            return page("History", self.sidebar(None), main, self.seq())
        alone = 1 - r.answered / r.calls
        kinds = {"floor": "blue", "pure": "green", "learns": "amber", "unknown": "red"}
        rows = "".join(
            f'<tr><td class="m">{esc(x.category)}</td><td><span class="chip {kinds[x.kind]}">{x.kind}</span></td>'
            f'<td class="m">{x.calls}</td><td class="m">{x.per_week:.0f}</td>'
            f'<td class="m">{esc(", ".join(f"{k} {v}" for k, v in x.approvers))}</td>'
            f'<td class="m">{x.audits_per_week:.0f}</td>'
            f'<td class="m">{"" if x.weeks_to_certify is None else f"{x.weeks_to_certify:.1f}"}</td></tr>'
            for x in r.rows)
        unknown = "".join(f'<tr><td class="m">{v}</td><td class="m">{esc(k)}</td></tr>' for k, v in r.unknown[:10])
        body = (f'<div><h1>What your agents did</h1><div class="sub">Read from your Claude Code transcripts and '
                f'classified by {esc(source)}. Nothing is changed.</div></div>'
                f'<div class="tiles"><div><div class="n">{r.calls}<small>tool calls</small></div>'
                f'<p>{r.calls / r.weeks:.0f} per week</p></div>'
                f'<div><div class="n">{r.answered}<small>answered by you</small></div>'
                f'<p>{r.answered / r.weeks:.1f} per week</p></div>'
                f'<div><div class="n">{alone:.1%}</div><p>ran without a person</p></div></div>'
                f'<div><div class="label">By category <span>with the gate: {r.audits_per_week:.0f} audit prompts '
                f'a week; a category is certified after {r.labels_to_certify} answers with no error</span></div>'
                f'<div class="box"><table><tr><th>category</th><th>kind</th><th>calls</th><th>per week</th>'
                f'<th>approved today by</th><th>audits per week</th><th>weeks to certify</th></tr>{rows}</table></div></div>'
                + (f'<div><div class="label">No category matches <span>add these to the policy</span></div>'
                   f'<div class="box"><table>{unknown}</table></div></div>' if unknown else ''))
        main = crumbs + f'<div class="body">{body}</div>'
        return page("History", self.sidebar(None), main, self.seq())

    def setup_page(self) -> str:
        g = install.status(self.c.home)
        chip = ('<span class="chip green">installed</span>' if g.installed else
                f'<span class="chip amber">partly installed, {len(g.hooked)} of {len(install.EVENTS)} events</span>'
                if g.hooked else '<span class="chip amber">not installed</span>')
        buttons = ('' if g.installed else '<button class="btn go" name="action" value="install">Install</button>') + \
            ('<button class="btn" name="action" value="remove">Remove</button>' if g.hooked else '')
        main = ('<div class="crumbs"><b>Setup</b></div><div class="body">'
                '<div><h1>Action gate in Claude Code</h1><div class="sub">Every tool call your Claude Code agents make '
                'goes through Jaynes. Actions that cannot be undone, like pushing, never run without you. Everything '
                'else runs as it does today, except a random one in ten that asks you, so Jaynes can measure.</div></div>'
                f'<div class="box"><table class="kv"><tr><td>state</td><td>{chip}</td></tr>'
                f'<tr><td>settings</td><td class="m">{esc(str(g.settings))}</td></tr>'
                f'<tr><td>policy</td><td class="m">{esc(str(g.policy))}'
                f'{"" if g.policy.is_file() else " (written on install)"}</td></tr></table></div>'
                + ('<div class="err">Jaynes is running from a temporary environment, so the hook would stop working '
                   'when it is cleaned. Install it first with uv tool install jaynes-ai (or pip install jaynes-ai), '
                   'then open this page with jaynes.</div>' if install.temporary() and not g.installed else '')
                + f'<form method="post" action="/setup" class="checks"><input type="hidden" name="token" value="{self.shared.token}">'
                f'{buttons}</form>'
                f'<div class="sub">Install and Remove rewrite {esc(str(g.settings))} and keep the previous file as '
                f'{esc(g.settings.name)}.jaynes-backup. Your other hooks and settings stay as they are.</div></div>')
        return page("Setup", self.sidebar(None), main, self.seq())

    def setup(self, fields: dict[str, str]) -> tuple[int, str]:
        action = fields.get("action", "")
        if action not in ("install", "remove"):
            return 400, self.setup_page()
        (install.install if action == "install" else install.remove)(self.c.home)
        return 303, "/setup"

    def roles_page(self) -> str:
        names = sorted({str(ev.payload["role"]) for ev in self.c.store.events("RunStarted")})
        rows = "".join(f'<tr><td><a href="/role/{q(n)}">{esc(n)}</a></td></tr>' for n in names)
        body = f'<div class="box"><table><tr><th>role</th></tr>{rows}</table></div>' if names else \
            '<div class="sub">No runs yet.</div>'
        main = f'<div class="crumbs"><b>Roles</b></div><div class="body">{body}</div>'
        return page("Roles", self.sidebar(None), main, self.seq())

    def role_page(self, name: str) -> str:
        runs = [ev.run_id for ev in self.c.store.events("RunStarted") if ev.payload["role"] == name and ev.run_id]
        if not runs:
            return self.not_found(f"No runs of role {name}")
        role = self.role_of(runs[-1])
        by_run = {r: list(self.c.store.events(run_id=r)) for r in runs}
        all_events = [ev for evs in by_run.values() for ev in evs]

        stop_rows: list[str] = []
        audit_rows: list[str] = []
        stop_steps = [s for s in role.steps if s.stop] if role else []
        for step in stop_steps:
            decided = [ev for ev in all_events if ev.type in ("StopOpened", "StopSkipped") and ev.payload["step"] == step.name]
            if not decided:
                stop_rows.append(f"<tr><td>{esc(step.name)}</td><td class='m'>no runs reached it</td><td></td><td></td><td></td></tr>")
                continue
            last = decided[-1].payload
            w = self.c.window(str(last["fingerprint"]))
            assert step.stop is not None
            # budget and delta as logged with the window, falling back to the code for logs before 0.4
            budget, delta = float(last.get("budget", step.stop.budget)), float(last.get("delta", step.stop.delta))
            status = ("floor" if not step.stop.sampled else "certified" if bounds.certified(w.n, w.k, budget, delta)
                      else "refuted" if bounds.refuted(w.n, w.k, budget, delta) else "undetermined")
            evidence = math.exp(bounds.log_certify_evalue(w.n, w.k, budget)) if w.n else 1.0
            skips = sum(1 for ev in decided if ev.type == "StopSkipped")
            audit_runs = [ev.run_id for ev in decided if ev.type == "StopOpened" and ev.payload["status"] == "certified"]
            for run in audit_runs:
                answer = next((ev.payload["answer"] for ev in by_run.get(run or "", [])
                               if ev.type == "StopAnswered" and ev.payload["step"] == step.name), "waiting")
                audit_rows.append(f'<tr><td>{esc(step.name)}</td><td><a href="/run/{q(run or "")}">{esc(run or "")}</a></td>'
                                  f'<td class="m">{esc(str(answer))}</td></tr>')
            stop_rows.append(
                f"<tr><td>{esc(step.name)}</td><td><span class='chip {CHIP.get(status, 'amber')}'>{esc(status)}</span></td>"
                f"<td class='m'>{w.n} label{'s' if w.n != 1 else ''}, {w.k} error{'s' if w.k != 1 else ''}, budget {budget:.0%}</td>"
                f"<td class='m'>{evidence:.2f} of {1 / delta:.0f}</td>"
                f"<td class='m'>{len(audit_runs)} audits, {skips} skipped</td></tr>")
        stops = (f'<div><div class="label">Stops</div><div class="box"><table><tr><th>step</th><th>status</th>'
                 f'<th>labels</th><th>evidence</th><th>audits</th></tr>{"".join(stop_rows)}</table></div></div>')
        if audit_rows:
            stops += (f'<div><div class="label">Audits <span>a certified stop that waited by the coin</span></div>'
                      f'<div class="box"><table><tr><th>step</th><th>run</th><th>answer</th></tr>'
                      f'{"".join(audit_rows)}</table></div></div>')

        weeks: dict[str, dict[str, float]] = defaultdict(lambda: defaultdict(float))
        for evs in by_run.values():
            week = datetime.fromtimestamp(evs[0].ts).strftime("%G-W%V")
            wk = weeks[week]
            wk["runs"] += 1
            st = project(evs)
            if st.finished:
                wk["finished"] += 1
                wk["tokens"] += sum(int(ev.payload.get("tokens_in", 0)) + int(ev.payload.get("tokens_out", 0))
                                    for ev in evs if ev.type == "TurnFinished")
            wk["steps"] += len({ev.payload["step"] for ev in evs if ev.type == "Checked"})
            wk["rounds"] += sum(1 for ev in evs if ev.type == "StopAnswered" and ev.payload["answer"] in ("return", "context"))
            wk["stops"] += sum(1 for ev in evs if ev.type == "StopAnswered" and ev.payload["answer"] in ("accept", "change"))
            wk["seconds"] += sum(float(ev.payload.get("seconds_spent") or 0) for ev in evs if ev.type == "StopAnswered")
        week_rows = "".join(
            f"<tr><td class='m'>{esc(k)}</td><td class='m'>{v['runs']:.0f} ({v['finished']:.0f} finished)</td>"
            f"<td class='m'>{v['steps']:.0f}</td>"
            f"<td class='m'>{(1 + v['rounds'] / v['stops']) if v['stops'] else 0:.1f}</td>"
            f"<td class='m'>{tokens(int(v['tokens'] / v['finished'])) if v['finished'] else '-'}</td>"
            f"<td class='m'>{v['seconds'] / 60:.1f}</td></tr>" for k, v in sorted(weeks.items()))
        weekly = (f'<div><div class="label">Per week</div><div class="box"><table><tr><th>week</th><th>runs</th>'
                  f'<th>steps checked</th><th>rounds until accepted</th><th>tokens per finished run</th>'
                  f'<th>your minutes</th></tr>{week_rows}</table></div></div>')
        run_rows = "".join(
            f'<tr><td><a href="/run/{q(r)}">{esc(str(evs[0].payload.get("task", "")))}</a></td>'
            f'<td class="m">{esc(r)}</td></tr>' for r, evs in reversed(list(by_run.items())))
        runs_html = f'<div><div class="label">Runs</div><div class="box"><table>{run_rows}</table></div></div>'
        note = "" if role else '<div class="sub">The role module cannot be imported here, so stops are not listed.</div>'
        main = (f'<div class="crumbs"><a href="/roles">Roles</a>/<b>{esc(name)}</b></div>'
                f'<div class="body">{note}{stops}{weekly}{runs_html}</div>')
        return page(f"Role {name}", self.sidebar(None), main, self.seq())

    def log_page(self, kind: str | None, run_id: str | None) -> str:
        events = list(self.c.store.events(kind or None, run_id=run_id or None))[-500:]
        kinds = self.c.store.kinds()
        def link(k: str | None, text: str) -> str:
            q = urllib.parse.urlencode({x: y for x, y in (("type", k), ("run", run_id)) if y})
            on = " on" if k == kind else ""
            return f'<a class="{on.strip()}" href="/log{"?" + q if q else ""}">{esc(text)}</a>'
        filters = '<div class="filters">' + link(None, "all") + "".join(link(k, k) for k in kinds) + "</div>"
        rows = "".join(
            f'<tr><td class="m">{ev.seq}</td><td class="m">{datetime.fromtimestamp(ev.ts).strftime("%m-%d %H:%M:%S")}</td>'
            f'<td class="m">{esc(ev.type)}</td><td class="m">{esc(ev.run_id or ev.action_id or "")}</td>'
            f'<td><pre>{esc(json.dumps(ev.payload, ensure_ascii=False)[:600])}</pre></td></tr>' for ev in reversed(events))
        scope = f" for run {esc(run_id)}" if run_id else ""
        main = (f'<div class="crumbs"><b>Log</b><span>{scope}</span><span class="r">last {len(events)} events</span></div>'
                f'<div class="body wide">{filters}<div class="box"><table>'
                f'<tr><th>seq</th><th>time</th><th>type</th><th>run</th><th>payload</th></tr>{rows}</table></div></div>')
        return page("Log", self.sidebar(None), main, self.seq())

    # answering ----------------------------------------------------------------

    def answer(self, fields: dict[str, str]) -> tuple[int, str]:
        """Returns (status, location or page)."""
        run_id, step, raw = fields.get("run", ""), fields.get("step", ""), fields.get("answer", "")
        note = fields.get("note", "").strip() or None
        if raw not in LABELS:
            return 400, self.stop_page(run_id, step, f"unknown answer {raw!r}", fields)
        answer = cast(Answer, raw)
        if answer in ("return", "context") and not note:
            return 400, self.stop_page(run_id, step, f"{answer} needs a note for the agent", fields)
        role = self.role_of(run_id)
        if role is None:
            return 400, self.stop_page(run_id, step, "the role of this run cannot be imported here", fields)
        output = None
        if answer == "change":
            try:
                output = json.loads(fields.get("output", ""))
            except ValueError as err:
                return 400, self.stop_page(run_id, step, f"the edited output is not JSON: {err}", fields)
        opened = project(self.c.store.events(run_id=run_id)).steps.get(step, StepState()).open_stop
        seen = self.shared.first_seen.get((run_id, step, opened[0])) if opened else None
        spent = round(max(0.0, time.time() - seen), 3) if seen else None  # from the first view, across reloads
        try:
            self.c.answer(role, run_id, step, answer, note=note, output=output, seconds_spent=spent,
                          to=fields.get("to") or None, by="person", evidence=fields.get("evidence") or None)
        except AnswerRejected as err:
            return 400, self.stop_page(run_id, step, "not recorded: " + "; ".join(err.reasons), fields)
        except RunBusy:
            return 409, self.stop_page(run_id, step, "the run is busy in another process; try again in a moment", fields)
        continue_in_background(self.c.home, run_id)
        return 303, f"/run/{q(run_id)}"

    def start(self, fields: dict[str, str]) -> tuple[int, str]:
        ref = fields.get("ref", "").strip() or fields.get("role", "")
        task = fields.get("task", "").strip()
        if not task:
            return 400, self.new_page("a run needs a task", fields)
        if ref.endswith(".toml"):  # stored whole, so the run continues from any folder; its mistakes are shown
            ref = str(Path(ref).expanduser().resolve())
            try:
                role_file.load(Path(ref))
            except role_file.RoleFileError as err:
                return 400, self.new_page(str(err), fields)
        r = self.import_role(ref)
        if r is None:
            return 400, self.new_page(f"{ref} cannot be imported from {Path.cwd()}", fields)
        if problems := verify.verify(r):
            return 400, self.new_page(verify.report(r, problems), fields)
        folder = fields.get("project", "").strip()
        preview = fields.get("preview", "").strip()
        if folder:
            try:
                repo.resolve(Path(folder).expanduser())
            except repo.ProjectError as err:
                return 400, self.new_page(str(err), fields)
        if preview and "{port}" not in preview:
            return 400, self.new_page("the preview command needs {port} where the port goes", fields)
        ports = fields.get("preview_ports", "").replace(",", " ").split()
        if not all(p.isdigit() and 0 < int(p) < 65536 for p in ports):
            return 400, self.new_page("local ports are numbers, such as 8789", fields)
        run_id = new_run_id()
        start_in_background(self.c.home, run_id, ref, task, self.known_roles().get(ref, {}),
                            str(Path(folder).expanduser()) if folder else "", preview,
                            fields.get("preview_path", "").strip(), " ".join(ports))
        return 303, f"/run/{q(run_id)}"

    # pieces -------------------------------------------------------------------

    def resume(self, fields: dict[str, str]) -> tuple[int, str]:
        run_id = fields.get("run", "")
        if not list(self.c.store.events("RunStarted", run_id=run_id)):
            return 404, self.not_found(f"No run {run_id}")
        if self.c._held(run_id):  # pyright: ignore[reportPrivateUsage]
            return 409, self.run_page(run_id)
        continue_in_background(self.c.home, run_id)
        return 303, f"/run/{q(run_id)}"

    def _value(self, s: StepState, target: Step[Any] | None) -> Any:
        return None if s.checked is None else self._value_of(s.checked, target)

    def _value_of(self, checked: dict[str, Any], target: Step[Any] | None) -> Any:
        """A checked output, with fields in schema order when the role is known (stored bytes sort keys)."""
        if checked.get("output_hash") is None:
            return None
        raw = (self.c.home / "objects" / f"{checked['output_hash']}.json").read_text()
        if target is not None and target.output is not None:
            return target.output.model_validate_json(raw).model_dump(mode="json")
        return json.loads(raw)

    @staticmethod
    def _where(s: StepState) -> str:
        return f"round {s.round}" + (f", attempt {s.checked['attempt']}" if s.checked and s.checked.get("attempt") else "")

    @staticmethod
    def _round_of(events: list[Event], target: Event) -> int:
        """The round a step event belongs to: the round of the last StepStarted of that step before it."""
        r = 1
        for ev in events:
            if ev.seq > target.seq:
                break
            if ev.type == "StepStarted" and ev.payload["step"] == target.payload["step"]:
                r = int(ev.payload["round"])
        return r

    def _turns(self, events: list[Event], step: str, round_: int) -> str:
        rows: list[str] = []
        tin = tout = 0
        seconds = 0.0
        models: set[str] = set()
        for ev in events:
            p = ev.payload
            if p.get("step") != step or self._round_of(events, ev) != round_:
                continue
            if ev.type == "TurnFinished":
                if "web" in p:
                    read = ", ".join(str(x) for x in p["web"]) or "nothing"
                    rows.append(f'<tr><td class="m" style="color:var(--blue)">web</td><td>read {esc(read)}</td>'
                                f'<td class="m">attempt {esc(str(p["attempt"]))}</td></tr>')
                tin += int(p["tokens_in"])
                tout += int(p["tokens_out"])
                seconds += float(p.get("seconds") or 0)
                models.add(str(p["model"]))
            elif ev.type == "TurnRejected":
                reasons = "; ".join(str(x) for x in p.get("reasons", [])[:3])
                rows.append(f'<tr><td class="m" style="color:var(--red)">rejected</td><td>{esc(reasons)}</td>'
                            f'<td class="m">attempt {esc(str(p["attempt"]))}, {esc(str(p["reason"]))}</td></tr>')
            elif ev.type == "TurnInterrupted":
                rows.append(f'<tr><td class="m">interrupted</td><td>{esc(str(p["reason"]))}</td>'
                            f'<td class="m">attempt {esc(str(p["attempt"]))}</td></tr>')
            elif ev.type == "Checked":
                rows.append(f'<tr><td class="m" style="color:var(--green)">checked</td><td>by {esc(str(p["source"]))}</td>'
                            f'<td class="m">attempt {esc(str(p["attempt"]))}</td></tr>')
        cost = (f"{tokens(tin)} tokens in, {tokens(tout)} out, {age(seconds)}"
                + (f", {esc(', '.join(sorted(models)))}" if models else ""))
        return (f'<div><div class="label">Turns <span>{cost}</span></div>'
                f'<div class="box"><table>{"".join(rows)}</table></div></div>')

    def _tiles(self, s: StepState, target: Step[Any] | None) -> str:
        if s.open_stop is None:
            return ""
        p = s.open_stop[1]
        n, k = int(p["n"]), int(p["k"])
        status = str(p["status"])
        if target is None or target.stop is None:
            return ""
        sp = target.stop
        evidence = math.exp(float(p["log_e"])) if n else 1.0
        budget, delta = float(p.get("budget", sp.budget)), float(p.get("delta", sp.delta))
        need = bounds.labels_to_certify(k, budget, delta)
        if status == "floor":
            third = '<div><div class="n">always</div><p>a floor waits on every run</p></div>'
        elif status == "certified":
            third = f'<div><div class="n">{sp.audit_rate:.0%}</div><p>an audit: this stop is sampled</p></div>'
        else:
            more = "never" if need is None else str(max(need - n, 0))
            third = f'<div><div class="n">{more}</div><p>more clean answers needed</p></div>'
        return (f'<div><div class="label">This stop <span class="chip {CHIP.get(status, "amber")}">{esc(status)}</span>'
                f'</div><div class="tiles">'
                f'<div><div class="n">{n}<small>label{"s" if n != 1 else ""}</small></div>'
                f'<p>{k} error{"s" if k != 1 else ""}, budget {budget:.0%}</p></div>'
                f'<div><div class="n">{evidence:.2f}<small>of {1 / delta:.0f}</small></div><p>evidence to sample it</p></div>'
                f"{third}</div></div>")

    @staticmethod
    def _output(label: str, value: Any, right: str = "") -> str:
        name = ""
        if isinstance(value, dict) and len(cast(dict[str, Any], value)) == 1:
            name, value = next(iter(cast(dict[str, Any], value).items()))  # one field: its name joins the label
        return (f'<div><div class="label">{esc(label)}{": " + esc(name) if name else ""} <span>{esc(right)}</span></div>'
                f"{render_value(value)}</div>")

    @staticmethod
    def _earlier(role: Role | None, step: str) -> list[str]:
        """Steps before ``step`` with an output, which a return or context from its stop can go back to."""
        names = [x.name for x in role.steps] if role else []
        return [x.name for x in role.steps[:names.index(step)] if x.output is not None] if role and step in names else []

    def _answer_bar(self, run_id: str, step: str, value: Any, error: str, fields: dict[str, str] | None,
                    editable: bool = True, earlier: list[str] | None = None, evidence: str = "") -> str:
        fields = fields or {}
        chosen = fields.get("to", "")
        back = ("" if not earlier else
                '<select name="to" title="Return and context go to this step; every step after it runs again">'
                + f'<option value="">to {esc(step)}</option>'
                + "".join(f'<option value="{esc(n)}"{" selected" if n == chosen else ""}>back to {esc(n)}</option>'
                          for n in earlier) + "</select>")
        edited = fields.get("output") or json.dumps(value, indent=2, ensure_ascii=False)
        editor_class = "" if fields.get("answer") == "change" else "hidden"
        hidden = "".join(f'<input type="hidden" name="{k}" value="{esc(v)}">' for k, v in (
            ("token", self.shared.token), ("run", run_id), ("step", step), ("evidence", evidence), ("answer", "")))
        return (f'<form id="answer" class="bar" method="post" action="/answer">{hidden}'
                f'<div id="editor" class="{editor_class}"><div class="label">Edit the output, then press Change again '
                f'<span>it is checked before it replaces the agent\'s</span></div>'
                f'<textarea name="output" spellcheck="false">{esc(edited)}</textarea></div>'
                f'<div class="row"><input type="text" name="note" value="{esc(fields.get("note", ""))}" '
                f'placeholder="Note for the agent (needed for return and context)">{back}'
                f'<button type="button" class="btn" data-a="context">Context<kbd>X</kbd></button>'
                f'<button type="button" class="btn" data-a="return">Return<kbd>R</kbd></button>'
                + (f'<button type="button" class="btn" data-a="change">Change<kbd>C</kbd></button>' if editable else "") +
                f'<button type="button" class="btn solid" data-a="accept">Accept<kbd>A</kbd></button></div>'
                f'<div id="hint" class="err">{esc(error)}</div></form>')

    def _event_row(self, ev: Event) -> str:
        p = ev.payload
        detail = {
            "RunStarted": lambda: f"task {p.get('task', '')}",
            "StepStarted": lambda: f"round {p['round']}, attempt {p['attempt']}, by {p['executor']}",
            "TurnFinished": lambda: f"{tokens(int(p['tokens_in']))} in, {tokens(int(p['tokens_out']))} out, "
                                    f"{age(float(p.get('seconds') or 0))}, {p['model']}",
            "TurnRejected": lambda: f"{p['reason']}: " + "; ".join(str(x) for x in p.get("reasons", [])[:2]),
            "TurnInterrupted": lambda: str(p["reason"]),
            "Checked": lambda: f"by {p['source']}" + (f", output {str(p['output_hash'])[:12]}" if p.get("output_hash") else ""),
            "StopOpened": lambda: f"{p['status']}, {p['n']} labels, {p['k']} errors",
            "StopSkipped": lambda: f"certified, coin {float(p['coin']):.2f}",
            "StopAnswered": lambda: (f"{p['answer']} to {p['to']}" + (f" ({p['revises']['label']} for {p['to']})"
                                                                      if p.get("revises") else "")
                                     if p.get("to") else
                                     f"{'accepted' if p['answer'] == 'accept' else p['answer']} ({p['label']})")
                                    + (f": {p['note']}" if p.get("note") else ""),
            "RunPaused": lambda: f"quota until {datetime.fromtimestamp(float(p['until'])).strftime('%H:%M')}",
            "RunBlocked": lambda: "; ".join(str(x) for x in p.get("reasons", [])[:2]),
            "RunFinished": lambda: "finished",
        }.get(ev.type, lambda: "")()
        when = datetime.fromtimestamp(ev.ts).strftime("%H:%M:%S")
        return (f'<tr><td class="m">{when}</td><td class="m">{esc(ev.type)}</td><td class="m">{esc(str(p.get("step", "")))}</td>'
                f"<td>{esc(detail)}</td></tr>")

    def not_found(self, text: str) -> str:
        main = f'<div class="crumbs"><b>{esc(text)}</b></div><div class="body"></div>'
        return page(text, self.sidebar(None), main, self.seq())


# --- HTTP ------------------------------------------------------------------------------

def make_server(home: Path, port: int = 8740, idle_after: float = 10.0) -> ReviewServer:
    """A review server for ``home`` on 127.0.0.1; ``port=0`` picks a free one."""
    if str(Path.cwd()) not in sys.path:
        sys.path.insert(0, str(Path.cwd()))  # role modules are imported from the project, as `jaynes run` does
    shared = Shared(secrets.token_urlsafe(16), secrets.token_urlsafe(24), idle_after)

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, format: str, *args: Any) -> None:
            return None

        def _send(self, status: int, body: str | bytes, kind: str = "text/html; charset=utf-8",
                  location: str | None = None, frame: str = "DENY", cookie: str | None = None) -> None:
            if isinstance(body, str) and kind.startswith("text/html") and not self._person():
                body = body.replace("<main>", f'<main><div class="notice">{NOT_PERSON}</div>', 1)
            data = body.encode() if isinstance(body, str) else body
            self.send_response(status)
            if cookie:
                self.send_header("Set-Cookie", cookie)
            self.send_header("Content-Type", kind)
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Frame-Options", frame)
            if location:
                self.send_header("Location", location)
            self.end_headers()
            self.wfile.write(data)

        def _local(self) -> bool:
            port = cast(tuple[str, int], self.server.server_address)[1]
            hosts = {f"127.0.0.1:{port}", f"localhost:{port}"}
            origin = self.headers.get("Origin")
            return self.headers.get("Host") in hosts and (origin is None or origin.split("//", 1)[-1] in hosts)

        def _cookie_name(self) -> str:  # cookies ignore ports, so two servers keep apart by name
            return f"jaynes_key_{cast(tuple[str, int], self.server.server_address)[1]}"

        def _person(self) -> bool:
            jar = SimpleCookie()
            try:
                jar.load(self.headers.get("Cookie") or "")
            except CookieError:
                return False
            got = jar.get(self._cookie_name())
            return got is not None and secrets.compare_digest(got.value, shared.key)

        def do_GET(self) -> None:
            try:
                self._get()
            except BrokenPipeError:
                pass
            except Exception as err:  # answer, rather than drop the connection
                self._send(500, f"error: {type(err).__name__}: {err}", "text/plain")

        def do_POST(self) -> None:
            try:
                self._post()
            except BrokenPipeError:
                pass
            except Exception as err:
                self._send(500, f"error: {type(err).__name__}: {err}", "text/plain")

        def _get(self) -> None:
            if not self._local():
                return self._send(403, "forbidden", "text/plain")
            app = Review(home, shared)
            url = urllib.parse.urlsplit(self.path)
            parts = [urllib.parse.unquote(x) for x in url.path.strip("/").split("/") if x]
            query = dict(urllib.parse.parse_qsl(url.query))
            if "key" in query:  # the link the server printed: keep the key as a cookie and drop it from the address
                if not secrets.compare_digest(query["key"], shared.key):
                    return self._send(403, "this link is from an earlier start of jaynes; open the one it printed "
                                           "last", "text/plain")
                rest = urllib.parse.urlencode({k: v for k, v in query.items() if k != "key"})
                return self._send(303, "", location=url.path + (f"?{rest}" if rest else ""), cookie=(
                    f"{self._cookie_name()}={shared.key}; Path=/; HttpOnly; SameSite=Strict; Max-Age=31536000"))
            match parts:
                case []:
                    self._send(200, app.queue_page())
                case ["seq"]:
                    self._send(200, str(app.seq()), "text/plain")
                case ["stop", run_id, step]:
                    self._send(200, app.stop_page(run_id, step))
                case ["run", run_id]:
                    self._send(200, app.run_page(run_id))
                case ["new"]:
                    self._send(200, app.new_page())
                case ["preview", run_id, which]:
                    status, out = app.preview(run_id, which)
                    if status == 302:
                        self._send(302, "", location=out)
                    else:
                        self._send(status, out, frame="SAMEORIGIN")
                case ["file", run_id, step, *rel] if rel:
                    found = app.picture(run_id, step, "/".join(rel))
                    if found is None:
                        self._send(404, app.not_found("Not found"))
                    else:
                        self._send(200, found[0], found[1])
                case ["history"]:
                    self._send(200, app.history_page())
                case ["setup"]:
                    self._send(200, app.setup_page())
                case ["roles"]:
                    self._send(200, app.roles_page())
                case ["role", name]:
                    self._send(200, app.role_page(name))
                case ["log"]:
                    self._send(200, app.log_page(query.get("type"), query.get("run")))
                case _:
                    self._send(404, app.not_found("Not found"))

        def _post(self) -> None:
            if not self._local():
                return self._send(403, "forbidden", "text/plain")
            try:
                length = int(self.headers.get("Content-Length") or 0)
            except ValueError:
                length = -1
            if not 0 <= length <= 2_000_000:
                return self._send(400, "bad Content-Length", "text/plain")
            fields = dict(urllib.parse.parse_qsl(self.rfile.read(length).decode(errors="replace"), keep_blank_values=True))
            path = urllib.parse.urlsplit(self.path).path
            handlers = {"/answer": Review.answer, "/continue": Review.resume, "/start": Review.start, "/setup": Review.setup}
            if path not in handlers:
                return self._send(404, "not found", "text/plain")
            if not secrets.compare_digest(fields.get("token", ""), shared.token):
                return self._send(403, "this page is stale or not from this server; reload it", "text/plain")
            if not self._person():
                return self._send(403, NOT_PERSON, "text/plain")
            app = Review(home, shared)
            status, out = handlers[path](app, fields)
            if status == 303:
                self._send(303, "", location=out)
            else:
                self._send(status, out)

    return ReviewServer(("127.0.0.1", port), Handler, shared.previews, shared.key)


NOT_PERSON = ("This browser can look but not answer. To answer, open the link jaynes printed in its terminal "
              "when it started.")


class ReviewServer(ThreadingHTTPServer):
    """Stops the previews it started when it shuts down. ``link`` opens the page as the person."""

    def __init__(self, address: tuple[str, int], handler: type[BaseHTTPRequestHandler], previews: Previews,
                 key: str) -> None:
        self.previews = previews  # set first: a port in use closes the server inside __init__
        self.key = key
        super().__init__(address, handler)

    @property
    def link(self) -> str:
        return f"http://127.0.0.1:{self.server_address[1]}/?key={self.key}"

    def shutdown(self) -> None:
        super().shutdown()
        self.previews.stop_all()

    def server_close(self) -> None:
        super().server_close()
        self.previews.stop_all()


def _running(home: Path, port: int) -> int | None:
    """The process serving ``home``'s page on ``port``, if one still is."""
    try:
        info = json.loads((home / "review.json").read_text())
        pid = int(info["pid"])
        os.kill(pid, 0)
    except (OSError, ValueError, KeyError, TypeError):
        return None
    return pid if info.get("port") == port else None


def serve(home: Path, port: int, browser: bool = False) -> None:
    """Serve the page; ``browser`` opens it with its link, as the person. If ``home``'s page already runs on
    ``port``, point to it instead: its key stays in the browser that opened it first."""
    home = home.resolve()
    home.mkdir(parents=True, exist_ok=True)  # a new home: the page records itself there before any request
    try:
        server = make_server(home, port)
    except OSError as err:
        if err.errno != errno.EADDRINUSE:
            raise
        pid = _running(home, port)
        if pid is None:
            sys.exit(f"port {port} is in use by another program or another Jaynes home; stop it or pass --port")
        print(f"Jaynes review is already running on http://127.0.0.1:{port}/ (process {pid})", flush=True)
        if browser:
            import webbrowser
            webbrowser.open(f"http://127.0.0.1:{port}/")
        return
    mine, port = home / "review.json", server.server_address[1]  # port 0 picks a free one
    mine.write_text(json.dumps({"pid": os.getpid(), "port": port}))

    def leave(signum: int, frame: object) -> None:
        sys.exit(0)

    for sig in (signal.SIGTERM, signal.SIGHUP):  # a closed terminal or a kill still stops the previews
        signal.signal(sig, leave)
    print(f"Jaynes review on {server.link}  (Ctrl+C to stop)", flush=True)
    if browser:
        import webbrowser
        threading.Timer(0.5, webbrowser.open, [server.link]).start()
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
        if _running(home, port) == os.getpid():
            mine.unlink(missing_ok=True)
