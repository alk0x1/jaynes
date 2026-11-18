"""The menu: the steps and checks of Jaynes's own that a role written in TOML picks by name.

Each is written once, here, beside the box where it runs a project's commands, and tested once. The program that
serves a TOML role (``jaynes role-serve``) asks for one with a line of JSON on standard input::

    {"kind": "step" | "check", "use": NAME, "with": {params}, "call": {the protocol's request fields}}

and reads one line back: the body of the protocol's reply (``result``, ``refused``, ``verdict`` or ``error``).
What a name means and which parameters it takes is checked before a run, by the engine; here a name is done.

A check the menu lacks is a script of the role (``use: "script"``): it runs in the box on a copy of the project,
reads ``{"output", "given", "project"}`` on standard input, and exits 0 to pass, 1 to fail with its reasons on
standard output, 3 when it cannot judge.
"""

from __future__ import annotations

import json
import re
import sys
import traceback
from fnmatch import fnmatchcase
from pathlib import Path
from typing import Any, Callable

import jaynes_box as box
from jaynes_role import Blocked, Fail, Pass, Refused, _result, _verdict

Call = dict[str, Any]
Params = dict[str, Any]


def _given(call: Call) -> dict[str, Any]:
    return {g["step"]: g["output"] for g in call.get("given") or []}


def _source(call: Call) -> Path | None:
    return Path(call["source"]) if call.get("source") else None


def _box(call: Call, command: str, timeout: float = 600.0, stdin: str | None = None) -> Any:
    """Run ``command`` on a copy of the run's project in the box: a ``Ran``, or ``Blocked``."""
    if not call.get("project"):
        return Blocked("the check has no copy of the project to run commands on")
    try:
        ran = box.run(Path(call["project"]), command, source=_source(call), timeout=timeout, stdin=stdin)
    except box.Unavailable as e:
        return Blocked(f"there is no box to run the project's commands in: {e}")
    if box.refused(ran):
        return Blocked("the box refused what the command tried, so its failure says nothing about the project: "
                       + "; ".join(box.refused(ran)[:3]))
    return ran


# --- steps -----------------------------------------------------------------------------------------------------

NO_RUNNER = ("no test runner was found in the project: Jaynes looks for pytest, a test script in package.json, "
             "go.mod, Cargo.toml and a Makefile test target")


def find_suite(call: Call, params: Params) -> Any:
    """How the project's tests run: ``{kind, suite, each, found}``, with ``suite`` empty when it has none, or a
    refusal when the role requires one or the machine cannot run them."""
    project = call.get("project")
    if not project:
        return Refused("finding the suite needs the project")
    found = box.detect(Path(project), "")
    if found is None:
        if params.get("required"):
            return Refused(NO_RUNNER)
        return {"kind": "", "suite": "", "each": "", "found": ""}
    try:
        why = box.missing(Path(project), found, source=_source(call))
    except box.Unavailable as e:
        return Blocked(str(e))
    if why:
        return Refused(why)
    return found.to_dict()


def _plain(text: str, given: dict[str, Any]) -> str:
    """``{{step.field}}`` filled plainly from what the step was given."""
    def one(m: re.Match[str]) -> str:
        step, _, field = m.group(1).strip().partition(".")
        value = (given.get(step) or {}).get(field) if field else given.get(step)
        return value if isinstance(value, str) else "" if value is None else json.dumps(value)
    return re.sub(r"\{\{(.*?)\}\}", one, text)


def refuse_when_false(call: Call, params: Params) -> Any:
    """Refuses, with the role's sentence, when an earlier step's field is false: the person decides how to go on."""
    step, _, field = str(params["field"]).partition(".")
    given = _given(call)
    if (given.get(step) or {}).get(field) is False:
        return Refused(_plain(str(params["says"]), given))
    return {}


# --- checks ----------------------------------------------------------------------------------------------------

def _suite(call: Call, params: Params) -> dict[str, Any]:
    return _given(call).get(str(params.get("suite_step") or "")) or {}


def suite_passes(call: Call, params: Params) -> Any:
    command = str(_suite(call, params).get("suite") or "")
    if not command:
        return Pass()
    ran = _box(call, command)
    if isinstance(ran, Blocked):
        return ran
    if ran.code != 0:
        return Fail(f"the suite fails: {command} exited with {ran.code}. The end of what it printed:\n{ran.output}")
    return Pass()


def _allowed(path: str, globs: list[str]) -> bool:
    """Whether ``path`` matches a glob of ``globs`` and none of its ``!`` globs."""
    def match(g: str) -> bool:
        return fnmatchcase(path, g) or (g.startswith("**/") and fnmatchcase(path, g[3:]))
    return any(match(g) for g in globs if not g.startswith("!")) and not any(match(g[1:]) for g in globs if g.startswith("!"))


def files_within(call: Call, params: Params) -> Any:
    field = str(params.get("field") or "files")
    files = [str(f) for f in (call.get("output") or {}).get(field) or []] if isinstance(call.get("output"), dict) else []
    outside = [f for f in files if not _allowed(f, list(params.get("write") or []))]
    if outside:
        return Fail(*(f"{f} is not a file this step may change" for f in outside))
    return Pass()


def _for_files(runner: dict[str, Any], tests: list[str]) -> str:
    return box.for_files(box.Runner.from_dict(runner), tests)


# What a quoted failure must say beyond: a line made only of these words says nothing of why.
STATUS = {"fail", "failed", "failure", "failures", "error", "errors", "not", "ok", "exit", "status", "code", "test",
          "tests", "setup", "build"}


def _quoted(failure: str, ran: Any, why: str) -> Any:
    quote = failure.strip()
    if all(w in STATUS for w in re.findall(r"[a-z]+", quote.lower())):
        return Fail(f"{quote} names no reason; quote the line that says why the tests fail {why}, such as the "
                    f"assertion message. The end of what it printed (exit {ran.code}):\n{ran.output}")
    if quote not in ran.printed:
        return Fail(f"{quote} is not in what the command printed; quote a line it prints {why}. The end of what it "
                    f"printed (exit {ran.code}):\n{ran.output}")
    return None


def tests_fail_for_the_bug(call: Call, params: Params) -> Any:
    """The tests the output names in ``tests`` are files the step may write, exist, and fail when run, printing the
    line the output quotes in ``failure``. An output that says ``reproduced`` is false passes: no test shows the bug,
    and a later step stops the run for the person."""
    out = call.get("output") if isinstance(call.get("output"), dict) else {}
    if out.get("reproduced") is False:
        return Pass()
    reasons = [f"{name} is empty; a reproduced bug needs it" for name in ("tests", "failure") if not out.get(name)]
    if reasons:
        return Fail(*reasons)
    tests = [str(t) for t in out.get("tests") or []]
    project = Path(call.get("project") or ".")
    write = list(params.get("write") or [])
    reasons = [f"{t} is not a file this step may write; tests go in paths such as tests/ or *_test.*" for t in tests
               if not _allowed(t, write)]
    reasons += [f"{t} does not exist in the project" for t in tests if not (project / t).is_file()]
    if reasons:
        return Fail(*reasons)
    runner = _suite(call, params)
    if not runner.get("suite"):
        return Blocked("the check was not given how the project's tests run")
    command = _for_files(runner, tests)
    ran = _box(call, command)
    if isinstance(ran, Blocked):
        return ran
    if ran.code == 0:
        return Fail(f"the tests pass before any fix; they must fail because of the bug. {command} printed:\n{ran.output}")
    return _quoted(str(out.get("failure") or ""), ran, "because of the bug") or Pass()


def tests_pass(call: Call, params: Params) -> Any:
    """The tests an earlier step named in its ``tests`` pass, and then the whole suite."""
    runner = _suite(call, params)
    if not runner.get("suite"):
        return Blocked("the check was not given how the project's tests run")
    tests = [str(t) for t in (_given(call).get(str(params["tests_from"])) or {}).get("tests") or []]
    for what, command in (("the tests still fail", _for_files(runner, tests)), ("the suite fails", runner["suite"])):
        ran = _box(call, command)
        if isinstance(ran, Blocked):
            return ran
        if ran.code != 0:
            return Fail(f"{what}: {command} exited with {ran.code}. The end of what it printed:\n{ran.output}")
    return Pass()


URL = "JAYNES_URL"
CASE_SECONDS = 120.0


def hidden_cases_pass(call: Call, params: Params) -> Any:
    """The cases a hidden step wrote (``cases``: ``says`` and ``run``) hold on the changed project. A failure tells
    the maker only the case's sentence; the person sees each case's result."""
    cases = (_given(call).get(str(params.get("cases_step") or "")) or {}).get("cases") or []
    failed: list[str] = []
    detail: list[str] = []
    for case in cases:
        says, command = str(case.get("says", "")), str(case.get("run", ""))
        if re.search(rf"\${{?{URL}\b", command):
            detail.append(f"Not run, the run serves no app: {says}")
            continue
        ran = _box(call, command, timeout=CASE_SECONDS)
        if isinstance(ran, Blocked):
            return ran
        detail.append(f"{'Passes' if ran.code == 0 else 'Fails'}: {says}")
        if ran.code != 0:
            failed.append(f"A check you cannot see found that this does not hold: {says}")
    if failed:
        return Fail(*failed, detail="\n".join(detail))
    return Pass()


TYPES = {"string": str, "boolean": bool, "array": list, "object": dict}


def _against(value: Any, schema: dict[str, Any], at: str, out: list[str]) -> None:
    kind = schema.get("type")
    if kind in ("number", "integer"):
        ok = isinstance(value, (int, float)) and not isinstance(value, bool) and (kind == "number" or float(value).is_integer())
    else:
        ok = kind not in TYPES or isinstance(value, TYPES[kind])
    if not ok:
        out.append(f"{at} is not {'an' if str(kind)[0] in 'aeiou' else 'a'} {kind}")
        return
    if isinstance(value, str) and "maxLength" in schema and len(value) > schema["maxLength"]:
        out.append(f"{at} has {len(value)} characters, at most {schema['maxLength']}")
    if isinstance(value, list):
        if "maxItems" in schema and len(value) > schema["maxItems"]:
            out.append(f"{at} has {len(value)} items, at most {schema['maxItems']}")
        for n, item in enumerate(value, 1):
            _against(item, schema.get("items") or {}, f"{at}, item {n},", out)
    if isinstance(value, dict):
        props = schema.get("properties") or {}
        for name in schema.get("required") or []:
            v = value.get(name)
            if v is None or (isinstance(v, str) and not v.strip()):
                out.append(f"{at} has no {name}" if at != "the output" else f"the output has no {name}")
        for name, v in value.items():
            if name in props and v is not None:
                _against(v, props[name], f"{name}" if at == "the output" else f"{at} {name}", out)


def output_follows_schema(call: Call, params: Params) -> Any:
    reasons: list[str] = []
    _against(call.get("output"), params.get("schema") or {}, "the output", reasons)
    return Fail(*reasons) if reasons else Pass()


def field_empty(call: Call, params: Params) -> Any:
    """A field the agent fills only to put a matter before the person, such as a test it may not change that stands
    in its way: while it is filled the check fails, with the role's sentence and what the agent wrote."""
    value = _field(call, params)
    if value not in (None, "", [], {}) and not (isinstance(value, str) and not value.strip()):
        return Fail(f"{params['says']}: {value if isinstance(value, str) else json.dumps(value)}")
    return Pass()


def _field(call: Call, params: Params) -> Any:
    return (call.get("output") or {}).get(str(params["field"])) if isinstance(call.get("output"), dict) else None


def field_required(call: Call, params: Params) -> Any:
    value = _field(call, params)
    if value is None or (isinstance(value, (str, list, dict)) and not (value.strip() if isinstance(value, str) else value)):
        return Fail(f"the output has no {params['field']}")
    return Pass()


def field_max_length(call: Call, params: Params) -> Any:
    value = _field(call, params)
    length = len(value) if isinstance(value, str) else len(json.dumps(value)) if value is not None else 0
    most = int(params["max"])
    if length > most:
        return Fail(f"{params['field']} has {length} characters; keep it to {most}")
    return Pass()


def script(call: Call, params: Params) -> Any:
    """A check of the role's own: a script run in the box on a copy of the project."""
    path = str(params["path"])
    given = json.dumps({"output": call.get("output"), "given": call.get("given") or [], "project": "."})
    ran = _box(call, _quote(path) if _executable(path) else f"sh {_quote(path)}", stdin=given)
    if isinstance(ran, Blocked):
        return ran
    said = [line for line in ran.printed.splitlines() if line.strip()]
    if ran.code == 0:
        return Pass()
    if ran.code == 1:
        return Fail(*(said or [f"{Path(path).name} failed and said nothing"]))
    if ran.code == 3:
        return Blocked(" ".join(said) or f"{Path(path).name} could not judge")
    return Blocked(f"{Path(path).name} exited with {ran.code}: " + " ".join(said[-5:]))


def _quote(s: str) -> str:
    import shlex
    return shlex.quote(s)


def _executable(path: str) -> bool:
    import os
    return os.access(path, os.X_OK)


# --- pages -----------------------------------------------------------------------------------------------------
#
# What an agent says a page says is the agent's word. Here Jaynes downloads the page itself, keeps its text in the
# step's output, which is in the record, and says by comparing text whether a quote is in it.
#
# The address comes from an agent, so it is held to rules before anything is sent: https only, the usual port, no
# name or password, and every address the name resolves to on the public internet, checked again at each
# redirect. The connection goes to the address that was checked. A page is read within a size and a time.

PAGE_BYTES = 2_000_000      # a page larger than this is not read
PAGE_SECONDS = 20.0         # nor one that takes longer, redirects included
PAGE_REDIRECTS = 5
PAGES_MAX = 12              # pages one step downloads
QUOTE_CHARS = 20            # a quote shorter than this, as compared, proves nothing
SCRIPTED_BELOW = 200        # an HTML page with less text than this was left for a script to fill
PAGE_AGENT = "jaynes"

_QUOTES = "\"“”„‟«»‘’‚‛`´"
_DASHES = "‐‑‒–—―−"
_UNSEEN = "­​‌‍⁠﻿"
_HTML_TYPES = ("text/html", "application/xhtml+xml")
_TEXT_TYPES = ("text/plain", "text/markdown")
_SKIPPED = ("script", "style", "noscript", "template", "svg")
_BLOCKS = ("address", "article", "aside", "blockquote", "br", "dd", "details", "div", "dl", "dt", "figcaption", "figure",
           "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "header", "hr", "li", "main", "nav", "ol", "p", "pre",
           "section", "summary", "table", "tbody", "td", "tfoot", "th", "thead", "title", "tr", "ul")


class Unread(Exception):
    """Why a page was not read. What is quoted from a page that was not read is not proved."""


# A word that goes on past the end of a line at its hyphen, as plain text wraps "best-effort": a letter or digit,
# the hyphen, one line break with nothing but spaces around it, and a letter or digit.
_WRAPPED = re.compile(r"(?<=\w)-[ \t]*\r?\n[ \t]*(?=\w)")


def same_words(text: str) -> str:
    """The text as two texts are compared: Unicode compatibility forms folded (NFKC), characters that do not show
    dropped, every kind of quotation mark made ', every kind of dash made -, a word wrapped at its hyphen joined
    again with the hyphen kept, and each run of spaces and line breaks made one space. Letters, digits, their case
    and every other mark stay as written.

    The joining is the one place a break counts: only a single line break right after a hyphen inside a word
    ("best-" then "effort" on the next line is best-effort). A hyphen followed by a space on the same line, or by a
    blank line, as between two blocks of a page, stays apart ("best- effort"); and the hyphen is never dropped, so
    a word split by hyphenation ("remo-" then "val") reads remo-val and not removal."""
    import unicodedata
    text = unicodedata.normalize("NFKC", text)
    text = "".join("'" if c in _QUOTES else "-" if c in _DASHES else "" if c in _UNSEEN else c for c in text)
    text = _WRAPPED.sub("-", text)
    return re.sub(r"\s+", " ", text).strip()


def held(text: str, quote: str) -> bool:
    """Whether ``text`` has the words of ``quote``, by ``same_words``."""
    words = same_words(quote)
    return bool(words) and words in same_words(text)


def _reader() -> Any:
    from html.parser import HTMLParser

    class Reader(HTMLParser):
        """The text of an HTML page as a reader sees it: no scripts or styles, and blocks apart by a blank line, so a
        hyphen at the end of one block never joins the next."""

        def __init__(self) -> None:
            super().__init__(convert_charrefs=True)
            self.parts: list[str] = []
            self.skipping = 0

        def handle_starttag(self, tag: str, attrs: Any) -> None:
            if tag in _SKIPPED:
                self.skipping += 1
            elif tag in _BLOCKS:
                self.parts.append("\n\n")

        def handle_endtag(self, tag: str) -> None:
            if tag in _SKIPPED:
                self.skipping = max(0, self.skipping - 1)
            elif tag in _BLOCKS:
                self.parts.append("\n\n")

        def handle_data(self, data: str) -> None:
            if not self.skipping:
                self.parts.append(data)

    return Reader()


def page_text(body: bytes, content_type: str) -> str:
    """The text of a downloaded page, or ``Unread``: a format Jaynes does not read, bytes that are not text, or an
    HTML page that leaves its text for a script to fill."""
    kind = content_type.split(";")[0].strip().lower()
    if kind not in _HTML_TYPES and kind not in _TEXT_TYPES:
        raise Unread(f"its format ({kind or 'not said'}) is not one Jaynes reads")
    if b"\x00" in body[:4096]:
        raise Unread("it is not text")
    named = re.search(r"charset=[\"']?([\w.:-]+)", content_type, re.I)
    charset = named.group(1) if named else ""
    if not charset and kind in _HTML_TYPES:
        meta = re.search(rb"<meta[^>]+charset=[\"']?([\w.:-]+)", body[:4096], re.I)
        charset = meta.group(1).decode("ascii", "replace") if meta else ""
    try:
        text = body.decode(charset or "utf-8", errors="replace")
    except LookupError:
        text = body.decode("utf-8", errors="replace")
    if kind in _TEXT_TYPES:
        return text
    reader = _reader()
    reader.feed(text)
    reader.close()
    seen = "".join(reader.parts)
    if len(same_words(seen)) < SCRIPTED_BELOW:
        raise Unread("it has almost no text of its own: a script puts the page together in the browser")
    return seen


def _public(ip: str) -> bool:
    """Whether an address is on the public internet: not this machine, a private or link-local network, a shared
    or reserved range, multicast, or an IPv6 form that carries an IPv4 address inside."""
    import ipaddress
    try:
        a = ipaddress.ip_address(ip.split("%")[0])
    except ValueError:
        return False
    if a.version == 6:
        if a.ipv4_mapped is not None:
            a = a.ipv4_mapped
        elif a.sixtofour is not None or a.teredo is not None or a in ipaddress.ip_network("64:ff9b::/96"):
            return False
    return a.is_global and not a.is_multicast


def _address(url: str) -> tuple[str, str]:
    """The host and the request target of an address Jaynes may download, or ``Unread``."""
    from urllib.parse import quote, urlsplit
    try:
        parts = urlsplit(url)
        port = parts.port
    except ValueError:
        raise Unread("only https addresses are downloaded, and this one does not read") from None
    if parts.scheme.lower() != "https" or not parts.hostname:
        raise Unread("only https addresses are downloaded")
    if parts.username is not None or parts.password is not None:
        raise Unread("its address carries a name or password")
    if port not in (None, 443):
        raise Unread(f"its address names the port {port}, and only the usual one is used")
    target = quote(parts.path or "/", safe="/%:@!$&'()*+,;=~-._") + ("?" + quote(parts.query, safe="/%:@!$&'()*+,;=~-._?") if parts.query else "")
    return parts.hostname, target


def _resolve(host: str) -> list[str]:
    """Every address a name resolves to."""
    import socket
    found = socket.getaddrinfo(host, 443, type=socket.SOCK_STREAM)
    return sorted({str(sockaddr[0]) for _family, _type, _proto, _name, sockaddr in found})


def _send(host: str, ip: str, target: str, timeout: float) -> tuple[int, dict[str, str], Callable[[int], bytes]]:
    """One GET over TLS to ``ip``, the address that was checked, with the certificate held to ``host``."""
    import http.client
    import socket
    import ssl

    class Pinned(http.client.HTTPSConnection):
        def connect(self) -> None:
            sock = socket.create_connection((ip, 443), self.timeout)
            self.sock = ssl.create_default_context().wrap_socket(sock, server_hostname=host)

    conn = Pinned(host, 443, timeout=timeout)
    conn.request("GET", target, headers={"User-Agent": PAGE_AGENT, "Accept": "text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.1",
                                         "Accept-Encoding": "identity", "Connection": "close"})
    answer = conn.getresponse()
    return answer.status, {k.lower(): v for k, v in answer.getheaders()}, answer.read


def download(url: str, *, resolve: Callable[[str], list[str]] = _resolve, send: Callable[..., Any] = _send,
             clock: Callable[[], float] | None = None) -> dict[str, Any]:
    """Download one page under the rules above: ``{final_url, type, body}``, or ``Unread`` with the reason."""
    import http.client
    import time
    from urllib.parse import urljoin
    clock = clock or time.monotonic
    slow = f"it took longer than {PAGE_SECONDS:.0f} s"
    deadline = clock() + PAGE_SECONDS
    for _hop in range(PAGE_REDIRECTS + 1):
        host, target = _address(url)
        try:
            addresses = resolve(host)
        except (OSError, UnicodeError):
            raise Unread("its address was not found") from None
        if not addresses or not all(_public(a) for a in addresses):
            raise Unread("its address is not on the public internet")
        left = deadline - clock()
        if left <= 0:
            raise Unread(slow)
        try:
            status, headers, read = send(host, addresses[0], target, left)
            if status in (301, 302, 303, 307, 308) and headers.get("location"):
                url = urljoin(url, headers["location"])
                continue
            if status != 200:
                raise Unread(f"the server answered {status}")
            body = b""
            while len(body) <= PAGE_BYTES:
                if clock() > deadline:
                    raise Unread(slow)
                piece = read(min(65536, PAGE_BYTES + 1 - len(body)))
                if not piece:
                    break
                body += piece
        except (OSError, http.client.HTTPException) as e:
            raise Unread(f"the connection failed ({type(e).__name__})") from None
        if len(body) > PAGE_BYTES:
            raise Unread(f"it is larger than {PAGE_BYTES // 1_000_000} MB")
        return {"final_url": url, "type": headers.get("content-type", ""), "body": body}
    raise Unread(f"it redirected more than {PAGE_REDIRECTS} times")


def _quotes_of(item: dict[str, Any]) -> list[str]:
    """The quotes an item carries: ``quote``, one, and ``quotes``, a list."""
    one = [item["quote"]] if isinstance(item.get("quote"), str) else []
    more = [q for q in item["quotes"] if isinstance(q, str)] if isinstance(item.get("quotes"), list) else []
    return one + more


def _pages_asked(listed: Any) -> dict[str, list[str] | None] | None:
    """The pages a list names, each with the quotes listed with it (None when the list is of addresses alone); None
    when it is not a list of pages."""
    if not isinstance(listed, list) or not listed:
        return None
    asked: dict[str, list[str] | None] = {}
    for item in listed:
        if isinstance(item, str) and item.strip():
            asked.setdefault(item, None)
        elif isinstance(item, dict) and isinstance(item.get("url"), str) and item["url"].strip():
            asked[item["url"]] = (asked.get(item["url"]) or []) + _quotes_of(item)
        else:
            return None
    return asked


def _fingerprint(data: bytes) -> str:
    import hashlib
    return hashlib.sha256(data).hexdigest()


def fetch_pages(call: Call, params: Params, *, download: Callable[[str], dict[str, Any]] = download) -> Any:
    """Download the pages an earlier step lists. The output says, for each, whether it was read and why not, with
    the fingerprint of what came, and in ``unread`` a line for each page not read, with why, for a sentence to the
    person; keeps the text of each page read, in ``texts``; and, for the quotes listed with
    a page, whether each is in its text. A page that could not be read is never an error of the step: it is a
    page not read, and nothing quoted from it is found."""
    from concurrent.futures import ThreadPoolExecutor
    step, _, field = str(params["pages"]).partition(".")
    asked = _pages_asked((_given(call).get(step) or {}).get(field))
    if asked is None:
        return Refused(f"{params['pages']} lists no pages: it is a list of https addresses, or of items that each have a url")
    if len(asked) > PAGES_MAX:
        return Refused(f"{params['pages']} lists {len(asked)} pages, and a step downloads at most {PAGES_MAX}")

    def one(url: str) -> tuple[dict[str, Any], str]:
        page: dict[str, Any] = {"url": url, "read": False, "why": "", "final_url": "", "type": "", "bytes": 0, "sha256": "",
                                "text_sha256": "", "chars": 0}
        text = ""
        try:
            got = download(url)
            body = got["body"]
            page.update(final_url=got["final_url"], type=got["type"].split(";")[0].strip().lower(), bytes=len(body), sha256=_fingerprint(body))
            text = page_text(body, got["type"])
            page.update(read=True, text_sha256=_fingerprint(text.encode()), chars=len(text))
        except Unread as e:
            page["why"] = str(e)
        except Exception as e:  # whatever went wrong, the page was not read, and that is said
            page["why"] = f"Jaynes could not read it ({type(e).__name__})"
        quotes = asked[url]
        if quotes is not None:
            page["quotes"] = [{"quote": q, "found": page["read"] and held(text, q)} for q in quotes]
        return page, text

    with ThreadPoolExecutor(max_workers=4) as pool:
        done = list(pool.map(one, asked))
    return {"pages": [page for page, _ in done], "texts": {page["url"]: text for page, text in done if page["read"]},
            "any_read": any(page["read"] for page, _ in done),
            "unread": [f"{page['url']}: {page['why']}" for page, _ in done if not page["read"]]}


def quotes_found(call: Call, params: Params) -> Any:
    """Every quote the output lists, each with the url of its page, is in the text Jaynes downloaded from that page.
    A quote from a page that was not read, or not downloaded at all, is not proved."""
    field, step = str(params["field"]), str(params["pages"])
    fetched = _given(call).get(step)
    pages = fetched.get("pages") if isinstance(fetched, dict) else None
    texts = fetched.get("texts") if isinstance(fetched, dict) else None
    if not isinstance(pages, list) or not isinstance(texts, dict) or not all(isinstance(p, dict) for p in pages):
        return Blocked(f"the check was not given the pages {step} downloaded")
    by_url = {str(p.get("url")): p for p in pages}
    for url, text in texts.items():
        if not isinstance(text, str) or _fingerprint(text.encode()) != by_url.get(url, {}).get("text_sha256"):
            return Blocked(f"the text kept from {url} is not the text that was downloaded")
    output = call.get("output")
    listed = output.get(field) if isinstance(output, dict) else None
    if not isinstance(listed, list) or not all(isinstance(i, dict) for i in listed):
        return Fail(f"{field} is a list of quotes, each with the url of its page and the quote")
    reasons: list[str] = []
    count = 0
    for item in listed:
        url = str(item.get("url") or "")
        for quote in _quotes_of(item):
            count += 1
            short = quote if len(quote) <= 80 else quote[:77] + "..."
            page = by_url.get(url)
            if page is None:
                reasons.append(f"\"{short}\": its page, {url or 'none named'}, was not downloaded by Jaynes in this run")
            elif not page.get("read"):
                reasons.append(f"\"{short}\": Jaynes could not read {url} ({page.get('why')}), so nothing quoted from it is proved")
            elif len(same_words(quote)) < QUOTE_CHARS:
                reasons.append(f"\"{short}\" is too short to prove anything: quote at least {QUOTE_CHARS} characters")
            elif not held(texts.get(url, ""), quote):
                reasons.append(f"\"{short}\": these words are not in the text Jaynes downloaded from {url}")
    if count == 0:
        return Fail(f"there is no quote to prove: {field} lists none")
    return Fail(*reasons[:20]) if reasons else Pass()


STEPS: dict[str, Callable[[Call, Params], Any]] = {"find-suite": find_suite, "refuse-when-false": refuse_when_false,
                                                       "fetch-pages": fetch_pages}
CHECKS: dict[str, Callable[[Call, Params], Any]] = {
    "suite-passes": suite_passes,
    "tests-fail-for-the-bug": tests_fail_for_the_bug,
    "tests-pass": tests_pass,
    "hidden-cases-pass": hidden_cases_pass,
    "output-follows-schema": output_follows_schema,
    "field-required": field_required,
    "field-empty": field_empty,
    "field-max-length": field_max_length,
    "files-within": files_within,
    "quotes-found": quotes_found,
    "script": script,
}


def answer(ask: dict[str, Any]) -> dict[str, Any]:
    use, params, call = str(ask.get("use")), ask.get("with") or {}, ask.get("call") or {}
    if ask.get("kind") == "step":
        if use not in STEPS:
            return {"reply": "error", "message": f"the menu has no step {use}"}
        return _result(STEPS[use](call, params))
    if use not in CHECKS:
        return {"reply": "error", "message": f"the menu has no check {use}"}
    return {"reply": "verdict", "verdict": _verdict(CHECKS[use](call, params))}


if __name__ == "__main__":
    try:
        body = answer(json.loads(sys.stdin.readline()))
    except Exception:  # the engine counts a menu that cannot answer as its own fault, never the agent's
        body = {"reply": "error", "message": traceback.format_exc(limit=5)[-2000:]}
    sys.stdout.write(json.dumps(body) + "\n")
    sys.stdout.flush()
