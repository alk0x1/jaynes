"""The review page, through HTTP: a run waits at a stop, is answered from the page, and continues (M3)."""

from __future__ import annotations

import fcntl
import json
import re
import subprocess
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Iterator
from http.cookiejar import CookieJar
from pathlib import Path

import pytest
from pydantic import BaseModel

from jaynes import cli, project
from jaynes.conductor import Conductor
from jaynes.review import ReviewServer, Stage, diff_html, make_server, stage_track
from jaynes.role import role
from jaynes.store import Store
from tests.fakes import Scripted, write
from tests.pictured import PNG


@pytest.fixture
def served(tmp_path: Path) -> Iterator[tuple[str, Path, str]]:
    """A run of tests.stopped waiting at its stop, and the review server on a free port."""
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.stopped:ROLE", "--task", "write the draft",
              "--runtime", "quick=tests.stopped:Agent"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    yield base, tmp_path, run
    server.shutdown()


JAR = CookieJar()  # the person's browser, shared by the tests' requests


def serving(home: Path) -> tuple[str, ReviewServer]:
    """A review server on a free port, opened once with its link, as jaynes opens it, so this browser may answer."""
    server = make_server(home, port=0, idle_after=0)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_address[1]}"
    get(f"{base}/?key={server.key}")
    return base, server


def get(url: str) -> str:
    with urllib.request.build_opener(urllib.request.HTTPCookieProcessor(JAR)).open(url, timeout=10) as r:
        return r.read().decode()


def post(url: str, fields: dict[str, str], headers: dict[str, str] | None = None,
         jar: CookieJar | None = JAR) -> tuple[int, str]:
    req = urllib.request.Request(url, data=urllib.parse.urlencode(fields).encode(), headers=headers or {})
    opener = urllib.request.build_opener(NoRedirect(), *([urllib.request.HTTPCookieProcessor(jar)] if jar else []))
    try:
        with opener.open(req, timeout=10) as r:
            return r.status, r.read().decode()
    except urllib.error.HTTPError as err:
        return err.code, err.read().decode()


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args: object, **kwargs: object) -> None:
        return None


def form(page: str) -> dict[str, str]:
    return {name: value for name, value in re.findall(r'<input type="hidden" name="(\w+)" value="([^"]*)"', page)}


def test_a_stop_answered_from_the_page_continues_the_run(served: tuple[str, Path, str]) -> None:
    base, home, run = served
    page = get(base + "/next")
    assert "Ship &#x27;draft one&#x27;?" in page  # the question, escaped
    assert "draft" in page and "write the draft" in page
    # the open queue item unfolds into the run's stages, marking where it waits and how earlier ones were released
    assert re.search(r'<li class="waiting">.*?draft.*?waiting', page, re.S)
    assert 'class="timeline"' not in page

    status, _ = post(base + "/answer", {**form(page), "answer": "accept", "note": ""})

    assert status == 303
    store = Store(home / "jaynes.sqlite3")
    deadline = time.monotonic() + 30
    while not list(store.events("RunFinished", run_id=run)):
        assert time.monotonic() < deadline, (home / "runs" / run / "continue.log").read_text()
        time.sleep(0.05)
    [answered] = [e.payload for e in store.events("StopAnswered", run_id=run)]
    assert answered["label"] == "correct" and answered["seconds_spent"] is not None
    assert "Nothing is waiting" in get(base + "/next")
    run_page = get(f"{base}/run/{run}")
    assert "accepted" in run_page and "finished" in run_page
    assert 'class="stages"' in run_page and 'class="timeline"' not in run_page
    role_page = get(base + "/role/stopped")
    assert re.search(r"draft.*?0 outcomes, 0 errors, 1 not yet held", role_page, re.S)  # until it holds
    assert "StopAnswered" in get(f"{base}/log?run={run}")


def test_change_from_the_page_is_checked_and_replaces_the_output(served: tuple[str, Path, str]) -> None:
    base, home, run = served
    fields = form(get(base + "/next"))
    status, body = post(base + "/answer", {**fields, "answer": "change", "output": "not json"})
    assert status == 400 and "not JSON" in body
    status, _ = post(base + "/answer", {**fields, "answer": "change", "output": json.dumps({"text": "draft two"})})
    assert status == 303
    store = Store(home / "jaynes.sqlite3")
    assert [e.payload["source"] for e in store.events("Checked", run_id=run)][-1] == "person"


def test_return_without_a_note_is_refused(served: tuple[str, Path, str]) -> None:
    base, home, run = served
    status, body = post(base + "/answer", {**form(get(base + "/next")), "answer": "return", "note": "  "})
    assert status == 400 and "needs a note" in body
    assert list(Store(home / "jaynes.sqlite3").events("StopAnswered", run_id=run)) == []


def test_answers_from_another_site_are_refused(served: tuple[str, Path, str]) -> None:
    base, home, run = served
    fields = form(get(base + "/next"))
    assert post(base + "/answer", {**fields, "token": "guess", "answer": "accept"})[0] == 403
    assert post(base + "/answer", {**fields, "answer": "accept"}, {"Origin": "https://evil.example"})[0] == 403
    assert post(base + "/answer", {**fields, "answer": "accept"}, {"Host": "evil.example"})[0] == 403
    assert list(Store(home / "jaynes.sqlite3").events("StopAnswered", run_id=run)) == []


def test_time_spent_counts_from_the_first_view_across_reloads(served: tuple[str, Path, str]) -> None:
    base, home, run = served
    get(base + "/next")
    time.sleep(1.2)
    status, _ = post(base + "/answer", {**form(get(base + "/next")), "answer": "accept"})
    assert status == 303
    [answered] = [e.payload for e in Store(home / "jaynes.sqlite3").events("StopAnswered", run_id=run)]
    assert answered["seconds_spent"] >= 1.2


def test_an_answer_while_another_process_runs_the_run_is_recorded(served: tuple[str, Path, str]) -> None:
    base, home, run = served
    fields = form(get(base + "/next"))
    with (home / "runs" / f"{run}.lock").open("a") as fh:  # the run goes on while its stops wait
        fcntl.flock(fh, fcntl.LOCK_EX)
        status, _ = post(base + "/answer", {**fields, "answer": "accept"})
    assert status == 303
    assert len(list(Store(home / "jaynes.sqlite3").events("StopAnswered", run_id=run))) == 1


def test_an_answer_while_another_answer_is_being_recorded_is_refused(served: tuple[str, Path, str]) -> None:
    base, home, run = served
    fields = form(get(base + "/next"))
    with (home / "runs" / f"{run}.answers.lock").open("a") as fh:
        fcntl.flock(fh, fcntl.LOCK_EX)
        status, body = post(base + "/answer", {**fields, "answer": "accept"})
    assert status == 409 and "busy" in body


def test_a_malformed_request_gets_an_answer(served: tuple[str, Path, str]) -> None:
    base, _, _ = served
    req = urllib.request.Request(base + "/answer", data=b"x", headers={"Content-Length": "nope"})
    with pytest.raises(urllib.error.HTTPError) as err:
        urllib.request.urlopen(req, timeout=10)
    assert err.value.code == 400


def test_an_idle_run_can_be_continued_from_the_page(served: tuple[str, Path, str]) -> None:
    base, home, run = served
    store = Store(home / "jaynes.sqlite3")
    store.append("StopAnswered", {"step": "draft", "answer": "accept", "label": "correct", "note": None,
                                  "fingerprint": "x", "seconds_waited": 0, "seconds_spent": 0}, run_id=run)
    home_ = get(base + "/")  # answered, but nothing continued it: the run is idle, and nothing goes on until you act
    waiting = home_.partition("<h2>To look at</h2>")[0]
    assert f'href="/stop/{run}/draft"' in waiting and "not running" in waiting and "Other runs" not in home_
    assert f'href="/stop/{run}/draft">Continue or dismiss' in get(f"{base}/run/{run}")  # one click from its page
    page = get(f"{base}/stop/{run}/draft")
    assert "not running" in page
    status, _ = post(base + "/continue", {**form(page)})
    assert status == 303
    deadline = time.monotonic() + 30
    while not list(store.events("RunFinished", run_id=run)):
        assert time.monotonic() < deadline
        time.sleep(0.05)


def test_an_idle_run_can_be_dismissed_from_the_page_and_the_queue_moves_on(served: tuple[str, Path, str]) -> None:
    base, home, run = served
    store = Store(home / "jaynes.sqlite3")
    store.append("StopAnswered", {"step": "draft", "answer": "accept", "label": "correct", "note": None,
                                  "fingerprint": "x", "seconds_waited": 0, "seconds_spent": 0}, run_id=run)
    page = get(f"{base}/stop/{run}/draft")
    assert 'action="/dismiss"' in page
    status, _ = post(base + "/dismiss", {**form(page)}, jar=None)
    assert status == 403 and not list(store.events("RunFinished", run_id=run))  # only the person's browser
    status, _ = post(base + "/dismiss", {**form(page)})
    assert status == 303 and "Nothing is waiting" in get(base + "/next")
    shown = get(f"{base}/run/{run}")
    assert '<div class="n">dismissed</div>' in shown and "dismissed; nothing was delivered" in shown


def test_after_an_answer_the_page_follows_the_run_it_continues(served: tuple[str, Path, str]) -> None:
    base, _, run = served
    fields = urllib.parse.urlencode({**form(get(base + "/next")), "answer": "return", "note": "shorter"}).encode()
    try:
        build_opener = urllib.request.build_opener(NoRedirect(), urllib.request.HTTPCookieProcessor(JAR))
        build_opener.open(urllib.request.Request(base + "/answer", data=fields), timeout=10)
        raise AssertionError("expected a redirect")
    except urllib.error.HTTPError as err:
        assert err.code == 303 and err.headers["Location"] == f"/run/{run}"


def test_a_stop_offers_to_return_an_earlier_step_and_the_answer_goes_there(tmp_path: Path) -> None:
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.two_stops:ROLE", "--task", "t",
              "--runtime", "quick=tests.two_stops:Agent"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    try:
        post(base + "/answer", {**form(get(base + "/next")), "answer": "accept"})
        store = Store(tmp_path / "jaynes.sqlite3")
        deadline = time.monotonic() + 30
        while '<option value="outline">' not in (page := get(base + "/next")):
            assert time.monotonic() < deadline, (tmp_path / "runs" / run / "continue.log").read_text()
            time.sleep(0.05)
        assert '<option value="outline">outline</option>' in page
        status, _ = post(base + "/answer", {**form(page), "answer": "return", "note": "shorter", "to": "outline"})
        assert status == 303
        answered = [e.payload for e in store.events("StopAnswered", run_id=run)][-1]
        assert answered["to"] == "outline" and answered["reset"] == ["outline", "draft"]
        assert "return to outline (error for outline): shorter" in get(base + f"/run/{run}")
    finally:
        server.shutdown()


def test_a_stop_shows_the_pictures_its_inputs_left_and_what_it_read_on_the_web(tmp_path: Path) -> None:
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.pictured:ROLE", "--task", "t",
              "--runtime", "quick=tests.pictured:Agent"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    try:
        page = get(base + f"/stop/{run}/direction")
        assert f'<img src="/file/{run}/look/ref-1.png"' in page and "ref-2.svg" not in page
        assert "read https://linear.app, search: dark fonts" in page
        with urllib.request.urlopen(base + f"/file/{run}/look/ref-1.png", timeout=10) as r:
            assert r.headers["Content-Type"] == "image/png" and r.read() == PNG
        for path in (f"/file/{run}/look/ref-2.svg", f"/file/{run}/look/other.png", f"/file/{run}/direction/x.png"):
            with pytest.raises(urllib.error.HTTPError) as err:
                urllib.request.urlopen(base + path, timeout=10)
            assert err.value.code == 404
    finally:
        server.shutdown()



def test_a_picture_drawn_from_a_page_links_the_page_served_where_it_runs_nothing(tmp_path: Path) -> None:
    from tests.paged import PAGE
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.paged:ROLE", "--task", "t",
              "--runtime", "quick=tests.paged:Agent"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    try:
        page = get(base + f"/stop/{run}/pick")
        link = f"/page/{run}/draw/direction-1.html"
        assert f'<a class="btn" href="{link}" target="_blank" rel="noopener">Open the page ↗</a>' in page
        assert "missing.html" not in page  # a page the step did not leave is not linked
        with urllib.request.urlopen(base + link, timeout=10) as r:
            policy = r.headers["Content-Security-Policy"]
            assert r.read() == PAGE.encode() and r.headers["Content-Type"].startswith("text/html")
        assert policy.startswith("sandbox;") and "default-src 'none'" in policy and "script-src" not in policy
        for path in (f"/page/{run}/draw/direction-1.png", f"/page/{run}/draw/missing.html", f"/page/{run}/pick/x.html"):
            with pytest.raises(urllib.error.HTTPError) as err:
                urllib.request.urlopen(base + path, timeout=10)
            assert err.value.code == 404
    finally:
        server.shutdown()

def test_a_stop_shows_the_commands_the_agent_ran_and_how_each_ended(tmp_path: Path) -> None:
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.stopped:ROLE", "--task", "t",
              "--runtime", "quick=tests.stopped:Tested"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    try:
        page = get(base + f"/stop/{run}/draft")
        assert page.count("<code>uv run pytest -q</code>") == 2
        assert "Exit code 1\n1 failed" in page and "3 passed in 0.02s" in page
        assert '<td class="m" style="color:var(--red)">ran</td>' in page
        assert '<td class="m" style="color:var(--green)">ran</td>' in page
    finally:
        server.shutdown()


def test_a_browser_without_the_link_can_look_but_not_answer(served: tuple[str, Path, str]) -> None:
    base, home, run = served
    with urllib.request.urlopen(base + "/", timeout=10) as r:  # like curl: no cookie
        page = r.read().decode()
    assert "Ship &#x27;draft one&#x27;?" in page and "open the link" in page.lower()
    status, body = post(base + "/answer", {**form(page), "answer": "accept"}, jar=None)
    assert status == 403 and "link" in body
    status, _ = post(base + "/answer", {**form(page), "answer": "accept"}, jar=CookieJar())
    assert status == 403
    assert list(Store(home / "jaynes.sqlite3").events("StopAnswered", run_id=run)) == []


def test_the_link_sets_the_key_and_leaves_the_address_bar(served: tuple[str, Path, str]) -> None:
    base, _, _ = served
    jar = CookieJar()
    opener = urllib.request.build_opener(NoRedirect(), urllib.request.HTTPCookieProcessor(jar))
    try:
        opener.open(base + "/?key=wrong", timeout=10)
    except urllib.error.HTTPError as err:
        assert err.code == 403
    assert not list(jar)
    [key] = [c.value for c in JAR if c.name.startswith("jaynes_key_") and base.endswith(c.name[11:])]
    try:
        opener.open(f"{base}/stop/x/y?key={key}", timeout=10)
    except urllib.error.HTTPError as err:
        assert err.code == 303 and err.headers["Location"] == "/stop/x/y"
    [cookie] = list(jar)
    assert cookie.value == key and cookie.has_nonstandard_attr("HttpOnly")


def test_an_answer_from_the_page_is_the_person_and_carries_what_was_shown(served: tuple[str, Path, str]) -> None:
    base, home, run = served
    fields = form(get(base + "/next"))
    assert fields["evidence"]
    status, body = post(base + "/answer", {**fields, "evidence": "0" * 16, "answer": "accept"})
    assert status == 400 and "reload" in body
    assert post(base + "/answer", {**fields, "answer": "accept"})[0] == 303
    [answered] = [e.payload for e in Store(home / "jaynes.sqlite3").events("StopAnswered", run_id=run)]
    assert answered["by"] == "person"


PATCH = """diff --git a/calc.py b/calc.py
index 1..2 100644
--- a/calc.py
+++ b/calc.py
@@ -1,4 +1,4 @@
 def add(a, b):
     \"\"\"Add two numbers,
-    badly.\"\"\"
-    return a - b
+    well.\"\"\"
+    return a + b
"""


def lines(shown: str) -> list[tuple[str, str]]:
    return re.findall(r'<span class="(add|del|ctx|hunk|file|meta)">(.*?)</span>(?=<span class="(?:add|del|ctx|hunk|file|meta)">|$)',
                      shown)


def test_a_diff_is_colored_by_change_and_by_the_files_language() -> None:
    shown = lines(diff_html(PATCH))
    assert shown[0] == ("file", "diff --git a/calc.py b/calc.py")
    [(_, well), (_, ret)] = [x for x in shown if x[0] == "add"]
    assert ret.startswith("<i>+</i>") and '<span class="k">return</span>' in ret
    assert '<span class="sd">    well."""</span>' in well  # still inside the docstring it continues
    [(_, badly), _] = [x for x in shown if x[0] == "del"]
    assert '<span class="sd">    badly."""</span>' in badly  # the removed side is read on its own


def test_a_diff_of_a_file_no_language_knows_is_shown_plain() -> None:
    shown = diff_html(PATCH.replace("calc.py", "notes.unknownext"))
    assert '<span class="add"><i>+</i>    return a + b</span>' in shown


def test_a_stop_that_ends_the_run_says_so_and_the_run_shows_it_ended(tmp_path: Path) -> None:
    from jaynes.conductor import Conductor
    from tests.stopped import ENDING, Nothing
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.stopped:ENDING", "--task", "find it",
              "--runtime", "quick=tests.stopped:Nothing"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    try:
        assert "Accepting ends the run here" in get(f"{base}/stop/{run}/look")
        c = Conductor(tmp_path, runtimes={"quick": Nothing})
        c.answer(ENDING, run, "look", "accept", by="person")
        c.run(ENDING, "find it", run_id=run)
        page = get(f"{base}/run/{run}")
        assert '<div class="n">ended</div>' in page and "ended at look; nothing was delivered" in page
    finally:
        server.shutdown()


class Note(BaseModel):
    text: str


def test_a_run_blocked_at_its_branch_links_to_a_page_that_names_the_reason(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / "a.txt").write_text("a\n")
    for a in (["init", "-q"], ["add", "-A"], ["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "s"]):
        subprocess.run(["git", *a], cwd=repo, check=True)

    def edit(ws: Path) -> None:
        (ws / "project" / "a.txt").write_text("b\n")
        write({"text": "b"})(ws)

    def refuse(*args: object) -> str:
        raise project.ProjectError("the repository is locked")

    monkeypatch.setattr(project, "branch", refuse)
    r = role("paint", agent="fake")
    r.step("paint", output=Note, project="write", may_change=["**"])
    home = tmp_path / "home"
    result = Conductor(home, runtimes={"fake": lambda: Scripted({("paint", 1): edit})}).run(r, "t", "r1", project=repo)
    assert result.status == "blocked" and result.step == "paint"
    base, server = serving(home)
    try:
        queue = get(base + "/")
        [link] = re.findall(r'<a class="item[^"]*" href="([^"]+)"', queue)
        # the run's row names its project: the folder's name, its whole path on hover
        assert f'<div class="p" title="{repo}">repo</div>' in queue
        page = get(base + link)
        assert "blocked after its last step" in page and "the repository is locked" in page
        assert "Every attempt of the round was rejected" not in page
        assert "r1" in get(base + "/stop/r1/")  # a link from an older log, with no step, opens the run
    finally:
        server.shutdown()


def test_the_page_reloads_for_runs_and_not_for_the_action_gate(served: tuple[str, Path, str]) -> None:
    base, home, run = served
    store = Store(home / "jaynes.sqlite3")
    before = get(base + "/seq")
    store.append("GateDecided", {"verdict": "allow"}, action_id="a1", session="s1")  # any agent on the machine
    assert get(base + "/seq") == before
    store.append("RunPaused", {"reason": "quota", "agent": "quick", "until": 0}, run_id=run)
    assert get(base + "/seq") != before


def test_a_stop_with_no_output_returns_to_the_step_before_it_by_default(tmp_path: Path) -> None:
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.two_stops:PURE", "--task", "t",
              "--runtime", "quick=tests.two_stops:Agent"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    try:
        page = get(base + f"/stop/{run}/accept")
        assert '<option value="write" selected>write</option>' in page
        assert "to accept</option>" not in page
        status, _ = post(base + "/answer", {**form(page), "answer": "return", "note": "shorter", "to": "write"})
        assert status == 303
        store = Store(tmp_path / "jaynes.sqlite3")
        deadline = time.monotonic() + 30
        while [e.payload["round"] for e in store.events("StepStarted", run_id=run)] != [1, 2]:
            assert time.monotonic() < deadline, (tmp_path / "runs" / run / "continue.log").read_text()
            time.sleep(0.05)
    finally:
        server.shutdown()


def test_accept_at_a_stop_with_no_output_goes_through_although_a_step_is_selected(tmp_path: Path) -> None:
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.two_stops:PURE", "--task", "t",
              "--runtime", "quick=tests.two_stops:Agent"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    try:
        page = get(base + f"/stop/{run}/accept")
        status, body = post(base + "/answer", {**form(page), "answer": "accept", "to": "write"})  # as the form sends
        assert status == 303, body
    finally:
        server.shutdown()


def test_a_floor_stop_marks_its_answers_so_one_key_does_not_accept_it(tmp_path: Path) -> None:
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.two_stops:FLOOR", "--task", "t",
              "--runtime", "quick=tests.two_stops:Agent"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    try:
        assert '<form id="answer" class="bar" method="post" action="/answer" data-floor="1">' in get(
            base + f"/stop/{run}/accept")
    finally:
        server.shutdown()


def test_a_sampled_stop_answers_on_one_key(served: tuple[str, Path, str]) -> None:
    base, _, _ = served
    assert '<form id="answer" class="bar" method="post" action="/answer">' in get(base + "/next")


def test_a_paused_run_shows_in_the_queue_and_continues_by_itself_when_its_quota_resets(tmp_path: Path) -> None:
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.stopped:ROLE", "--task", "later",
              "--runtime", "quick=tests.stopped:Exhausted"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    server = make_server(tmp_path, port=0, idle_after=0, wake_every=0.2)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_address[1]}"
    get(f"{base}/?key={server.key}")
    try:
        page = get(base + "/next")
        assert '<span class="chip amber">paused</span>' in page and f'href="/run/{run}"' in page
        assert '<div class="qhead" data-waiting="0"><span>Waiting for you</span>' in page
        assert 'id="answer"' not in page and "Nothing is waiting" in page
        store = Store(tmp_path / "jaynes.sqlite3")
        deadline = time.monotonic() + 30
        while not list(store.events("StopOpened", run_id=run)):  # nobody pressed Continue
            assert time.monotonic() < deadline, (tmp_path / "runs" / run / "continue.log").read_text()
            time.sleep(0.05)
    finally:
        server.shutdown()


def test_a_stop_while_no_tab_is_open_opens_the_page_and_an_open_tab_is_left_to_notify(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    opened: list[str] = []
    monkeypatch.setattr("webbrowser.open", opened.append)
    server = make_server(tmp_path, port=0, idle_after=0, call=True)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_address[1]}"
    try:
        get(f"{base}/?key={server.key}")
        get(base + "/seq")  # a tab is open
        cli.main(["run", "--home", str(tmp_path), "--role", "tests.held:ROLE", "--task", "one",
                  "--runtime", "quick=tests.held:Agent"])
        time.sleep(5)
        assert opened == []
        post(base + "/bye", {})  # the tab closed
        cli.main(["run", "--home", str(tmp_path), "--role", "tests.held:ROLE", "--task", "two",
                  "--runtime", "quick=tests.held:Agent"])
        deadline = time.monotonic() + 10
        while not opened:
            assert time.monotonic() < deadline
            time.sleep(0.1)
        assert opened == [server.at("/next")]  # at what to answer
    finally:
        server.shutdown()


def test_a_tab_sees_the_count_in_its_title_and_which_start_of_the_server_it_belongs_to(
        served: tuple[str, Path, str]) -> None:
    base, home, _ = served
    page = get(base + "/next")
    boot = re.search(r'<body data-boot="([^"]+)">', page)
    assert boot and get(base + "/seq").split(" ")[1] == boot.group(1)
    assert '<div class="qhead" data-waiting="0">' in page and 'id="notify"' in page  # a sample counts for nothing
    cli.main(["run", "--home", str(home), "--role", "tests.held:ROLE", "--task", "t", "--runtime", "quick=tests.held:Agent"])
    assert '<div class="qhead" data-waiting="1">' in get(base + "/")  # what holds a run counts


def test_the_page_offers_a_newer_jaynes_and_a_restart_after_an_update(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from jaynes import update
    from jaynes.update import Installed
    base, server = serving(tmp_path)
    old = Installed("https://github.com/alk0x1/jaynes", "1" * 40)
    monkeypatch.setattr(update, "RUNNING", old)
    monkeypatch.setattr(update, "installed", lambda: old)
    assert "A newer Jaynes" not in get(base + "/next")
    assert server.shared
    server.shared.latest = "2" * 40
    assert "A newer Jaynes is out." in get(base + "/next") and 'action="/update"' in get(base + "/next")
    monkeypatch.setattr(update, "installed", lambda: Installed(old.url, "2" * 40))
    page = get(base + "/next")
    assert "Jaynes was updated to 2222222." in page and 'action="/restart"' in page
    server.shutdown()


def test_runs_open_the_page_unless_it_already_runs(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from jaynes import review
    started: list[list[str]] = []

    def popen(args: list[str], **kwargs: object) -> None:
        started.append(args)

    def running(home: Path, port: int) -> int:
        return 1

    monkeypatch.delenv("JAYNES_NO_PAGE")
    monkeypatch.setattr(review.subprocess, "Popen", popen)
    review.ensure_page(tmp_path)
    assert started and started[0][-6:] == ["review", "--home", str(tmp_path), "--port", "8740", "--open"]
    started.clear()
    monkeypatch.setattr(review, "_running", running)
    review.ensure_page(tmp_path)
    assert started == []


def test_a_person_chooses_among_the_items_from_the_page(tmp_path: Path) -> None:
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.picked:ROLE", "--task", "pick moments",
              "--runtime", "fake=tests.picked:Agent"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    try:
        page = get(base + "/next")
        assert 'name="chosen"' in page and 'class="card pick" data-id="M1"' in page and "moment 1" in page
        status, body = post(base + "/answer", {**form(page), "answer": "choose", "chosen": "M9"})
        assert status == 400 and "&#x27;M9&#x27; is not one of" in body
        assert post(base + "/answer", {**form(page), "answer": "choose", "chosen": "M4, M2"})[0] == 303
    finally:
        server.shutdown()
    [answered] = [e.payload for e in Store(tmp_path / "jaynes.sqlite3").events("StopAnswered", run_id=run)]
    assert answered["chosen"] == ["M4", "M2"] and answered["by"] == "person"


def test_why_asks_a_claude_that_reads_the_step_and_changes_nothing(tmp_path: Path,
                                                                    monkeypatch: pytest.MonkeyPatch) -> None:
    from jaynes import why
    seen: list[tuple[Path, str]] = []

    def explains(here: Path, prompt: str) -> str:
        seen.append((here, prompt))
        output = json.loads((here / "output.json").read_text())
        return f"It chose {output['text']} because the step asked for a sober face."

    monkeypatch.setattr(why, "explainer", explains)
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.decided:ROLE", "--task", "a look",
              "--runtime", "fake=tests.decided:Agent"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    try:
        page = get(base + "/next")
        assert 'action="/why"' in page and f'data-whys="/why/{run}/look"' in page
        token = re.search(r'name="token" value="([^"]+)"', page)
        assert token
        status, _ = post(base + "/why", {"token": token.group(1), "run": run, "step": "look",
                                         "question": "Why Inter?"})
        assert status == 303
        for _ in range(50):
            if "It chose Inter" in get(f"{base}/why/{run}/look"):
                break
            time.sleep(0.1)
        assert "<b>Why Inter?</b><p>It chose Inter because the step asked for a sober face.</p>" in get(
            f"{base}/why/{run}/look")
    finally:
        server.shutdown()
    [(here, prompt)] = seen
    assert prompt.endswith("Question: Why Inter?") and (here / "task.md").read_text() == "a look"
    assert "# Step: look" in (here / "step.md").read_text()
    assert list(Store(tmp_path / "jaynes.sqlite3").events("StopAnswered", run_id=run)) == []  # nothing changed
    rules = why.settings(here)["permissions"]
    assert rules["allow"] == ["Read(./**)", "Glob", "Grep"] and {"Edit", "Write", "Bash", "WebFetch"} <= set(rules["deny"])


def test_a_person_answers_the_interview_on_one_screen_and_their_words_win(tmp_path: Path) -> None:
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.asked:ROLE", "--task", "a look",
              "--runtime", "fake=tests.asked:Agent"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    try:
        page = get(base + "/next")
        front = page.partition('<details class="fold"><summary>Details')[0]
        assert "<h1>3 questions before the work starts</h1>" in front
        assert '<input type="radio" name="pick:theme" value="Dark" form="answer" checked>Dark' in front
        assert '<div class="why">Guess: you said dark mode</div>' in front and 'name="own:looks"' in front
        assert 'data-a="accept">Start<kbd>' in page
        assert post(base + "/answer", {**form(page), "answer": "accept", "pick:theme": "Light", "own:theme": "",
                                       "pick:looks": "3", "own:looks": "one bold, one sober"})[0] == 303
    finally:
        server.shutdown()
    [answered] = [e.payload for e in Store(tmp_path / "jaynes.sqlite3").events("StopAnswered", run_id=run)]
    assert answered["answer"] == "change"
    [checked] = [e.payload for e in Store(tmp_path / "jaynes.sqlite3").events("Checked", run_id=run)
                 if e.payload["step"] == "interview" and e.payload.get("source") == "person"]
    assert checked


def test_a_person_rejects_one_decision_from_the_page_and_the_rest_is_locked(tmp_path: Path) -> None:
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.decided:ROLE", "--task", "a look",
              "--runtime", "fake=tests.decided:Agent"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    try:
        page = get(base + "/next")
        front = page.partition('<details class="fold"><summary>Details')[0]
        assert '<div class="dec" data-id="text">' in front and "<small>Text face</small><span>Inter</span>" in front
        assert 'name="reject:text" form="answer" disabled' in front and "Output" not in front
        status, body = post(base + "/answer", {**form(page), "answer": "accept", "reject:text": " "})
        assert status == 400 and "say what is wrong with Text face" in body
        assert post(base + "/answer", {**form(page), "answer": "accept", "reject:text": "more character"})[0] == 303
    finally:
        server.shutdown()
    [answered] = [e.payload for e in Store(tmp_path / "jaynes.sqlite3").events("StopAnswered", run_id=run)]
    assert answered["answer"] == "return" and "Text face (Inter): more character" in answered["note"]
    assert answered["locked"] == {"theme": "dark", "accent": "#e8a33d", "heading": "Fraunces"}


def test_a_person_drops_two_looks_from_the_page_and_the_one_left_is_chosen(tmp_path: Path) -> None:
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.offered:ROLE", "--task", "three looks",
              "--runtime", "fake=tests.offered:Agent"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    try:
        page = get(base + "/next")
        front = page.partition('<details class="fold"><summary>Details')[0]
        assert '<div class="dec" data-id="ledger" data-choose="Ledger">' in front and 'data-v="drop"' in front
        assert 'data-v="base"' in front and "Click to choose" not in front
        assert post(base + "/answer", {**form(page), "answer": "accept", "drop:booklet": "hated it",
                                       "drop:board": ""})[0] == 303
    finally:
        server.shutdown()
    [answered] = [e.payload for e in Store(tmp_path / "jaynes.sqlite3").events("StopAnswered", run_id=run)]
    assert answered["answer"] == "choose" and answered["chosen"] == ["ledger"]
    assert [d["id"] for d in answered["dropped"]] == ["booklet", "board"]


def test_a_stop_shows_what_the_person_judges_in_front_and_its_output_behind_details(tmp_path: Path) -> None:
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.shown:ROLE", "--task", "say the status",
              "--runtime", "quick=tests.shown:Agent"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    try:
        page = get(base + "/next")
    finally:
        server.shutdown()
    front, _, details = page.partition('<details class="fold"><summary>Details')
    assert "<h1>Make this change?</h1>" in front
    link = f"/file/{run}/look/screen.png"  # enlarged in the page with a click, never in another tab
    assert f'<a class="shot wide" href="{link}" data-zoom="{link}" style="max-width:1440px">' in front
    assert f'<img src="{link}" alt="before"' in front and 'target="_blank"' not in front.split("<main>")[1]
    assert "Step 2 of 2, waiting for you" in front  # where the run is; its steps a click away
    assert '<details class="steps"><summary>Steps</summary><ul class="stages">' in front
    assert "<b>The list hides the status</b><p>Say it: the status goes beside the name.</p>" in front
    assert front.index("<b>The list hides the status</b>") < front.index(f'<img src="{link}"')  # named, then seen
    assert '<details class="fold"><summary>More <span>1</span></summary>' in front  # the cost, folded
    assert "Output" not in front and ">Turns" not in front and 'class="label">Output' in details
    bar = page[page.index('<form id="answer"'):]
    menu, _, sight = bar.partition("</details>")
    assert 'data-a="context"' in menu and 'data-a="change"' in menu  # in the secondary menu
    assert 'data-a="return"' in sight and 'data-a="accept"' in sight  # in sight, with the note


def test_a_blocked_run_waits_for_the_person_with_its_reason_and_opens_from_next(served: tuple[str, Path, str]) -> None:
    base, home, run = served
    store = Store(home / "jaynes.sqlite3")
    store.append("StopAnswered", {"step": "draft", "answer": "accept", "label": "correct", "note": None,
                                  "fingerprint": "x", "seconds_waited": 0, "seconds_spent": 0}, run_id=run)
    store.append("RunBlocked", {"step": "final", "reasons": ["every attempt failed"]}, run_id=run)
    waiting = get(base + "/").partition("<h2>To look at</h2>")[0]
    assert '<div class="qhead" data-waiting="1">' in waiting and "every attempt failed" in waiting
    assert f'class="hrow blocked holds" href="/stop/{run}/final"' in waiting
    assert "every attempt failed" in get(base + "/next") and 'action="/send-back"' in get(base + "/next")


def test_a_run_that_asks_the_person_waits_for_them_and_says_who_needs_them(served: tuple[str, Path, str]) -> None:
    base, home, run = served
    store = Store(home / "jaynes.sqlite3")
    store.append("StopAnswered", {"step": "draft", "answer": "accept", "label": "correct", "note": None,
                                  "fingerprint": "x", "seconds_waited": 0, "seconds_spent": 0}, run_id=run)
    store.append("RunBlocked", {"step": "final", "asks": True, "reasons": ["a test must change: `add` is old"]}, run_id=run)
    page = get(base + "/next")
    assert '<div class="qhead" data-waiting="1">' in page
    assert "The agent needs you at final" in page and "<code>add</code>" in page
    assert "Every attempt of the round was rejected" not in page and 'action="/send-back"' in page
    assert '<details class="fold"><summary>What the agent did</summary>' in page  # the question opens alone


def test_a_blocked_run_waits_for_the_person_a_stop_after_every_step_ran_is_a_sample_and_the_same_task_twice_is_told_apart(
        served: tuple[str, Path, str]) -> None:
    base, home, run = served
    cli.main(["run", "--home", str(home), "--role", "tests.stopped:ROLE", "--task", "write the draft",
              "--runtime", "quick=tests.stopped:Agent"])
    blocked = next(r for r in Store(home / "jaynes.sqlite3").runs() if r != run)
    store = Store(home / "jaynes.sqlite3")
    store.append("StopAnswered", {"step": "draft", "answer": "accept", "label": "correct", "note": None,
                                  "fingerprint": "x", "seconds_waited": 0, "seconds_spent": 0}, run_id=blocked)
    store.append("RunBlocked", {"step": "final", "reasons": ["every attempt failed"]}, run_id=blocked)
    page = get(base + "/")
    assert '<div class="qhead" data-waiting="1">' in page and '<details class="others">' not in page
    assert f'<a class="item holds blocked" href="/stop/{blocked}/final"' in page
    assert f'<a class="item stop" href="/stop/{run}/draft"' in page  # every step ran: it only lets the run finish
    assert len(re.findall(r"<span>started \d\d:\d\d</span>", page)) == 2
    assert '<div class="p"' not in page  # runs with no project folder get no line for one
    call = get(f"{base}/run/{run}")
    assert ('<div class="call stop"><span>Waiting for you: Ship &#x27;draft one&#x27;?</span>'
            f'<a class="btn go" href="/stop/{run}/draft">Open the stop</a>') in call


def test_a_new_start_keeps_the_key_so_the_browser_that_answered_still_answers_after_the_mac_restarts(
        tmp_path: Path) -> None:
    first = make_server(tmp_path, port=0)
    first.server_close()
    again = make_server(tmp_path, port=0)  # a start from nothing, as after a reboot: no restart hands the key over
    again.server_close()
    assert again.key == first.key
    assert (tmp_path / "review.key").stat().st_mode & 0o777 == 0o600  # only the person's user reads it
    restarted = make_server(tmp_path, port=0, key="held-by-the-open-tab")  # a restart keeps the tab's key
    restarted.server_close()
    cold = make_server(tmp_path, port=0)
    cold.server_close()
    assert cold.key == "held-by-the-open-tab"


def test_a_restart_keeps_the_key_so_the_open_tab_still_answers_and_no_tab_opens(
        served: tuple[str, Path, str], monkeypatch: pytest.MonkeyPatch) -> None:
    from jaynes import review
    _, home, run = served
    first = make_server(home, port=0)
    threading.Thread(target=first.serve_forever, daemon=True).start()
    port = first.server_address[1]
    get(f"http://127.0.0.1:{port}/?key={first.key}")  # the tab, opened with the link
    calls: list[tuple[list[str], dict[str, str]]] = []
    def execve(path: str, args: list[str], env: dict[str, str]) -> None:
        calls.append((args, env))

    monkeypatch.setattr(review.os, "execve", execve)
    first.restart()
    [(args, env)] = calls
    assert env[review.KEPT] == first.key and args[-1] == "--open"
    first.shutdown()
    first.server_close()
    again = make_server(home, port=port, key=first.key)  # what the restart starts, on the same port
    threading.Thread(target=again.serve_forever, daemon=True).start()
    try:
        page = get(f"http://127.0.0.1:{port}/stop/{run}/draft")  # the same tab, reloaded: no new link needed
        assert "can look but not answer" not in page
        assert post(f"http://127.0.0.1:{port}/answer", {**form(page), "answer": "accept"})[0] == 303
    finally:
        again.shutdown()
    made: list[str | None] = []
    opened: list[str] = []

    class Stub:
        server_address, link = ("127.0.0.1", port + 1), "unused"

        def serve_forever(self) -> None:
            pass

        def server_close(self) -> None:
            pass

    monkeypatch.setenv(review.KEPT, first.key)
    def make(*args: object, key: str | None = None, **rest: object) -> Stub:
        made.append(key)
        return Stub()

    monkeypatch.setattr(review, "make_server", make)
    monkeypatch.setattr("webbrowser.open", opened.append)
    review.serve(home, port + 1, browser=True)
    assert made == [first.key] and opened == [] and review.KEPT not in review.os.environ  # runs never see it


LONG_TASK = ("Implement the team baseline and the ticket return ranking as Go packages in the module tenure, working on "
             "a frozen snapshot. First read CONTEXT.md and the sections Passes and Slice 1 of PLAN.md. " * 6).strip()


def test_a_run_is_titled_by_a_line_and_its_whole_task_is_one_click_away(tmp_path: Path) -> None:
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.stopped:ROLE", "--task", LONG_TASK,
              "--runtime", "quick=tests.stopped:Agent"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    try:
        title = "Implement the team baseline and the ticket return ranking as Go packages in the module tenure…"
        for page in (get(f"{base}/"), get(f"{base}/stop/{run}/draft")):
            assert title in page and LONG_TASK not in page
        whole = get(f"{base}/run/{run}")
        assert f"<b>{title}</b>" in whole and f'<summary>The whole task</summary><div class="task">{LONG_TASK}' in whole
    finally:
        server.shutdown()


def test_a_blocked_run_goes_back_from_its_page_to_the_step_the_person_names_with_their_note(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from jaynes import api
    from tests import stopped
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.stopped:BLOCKS", "--task", "rank people",
              "--runtime", "quick=tests.stopped:Drafts"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    c = Conductor(tmp_path, runtimes={"quick": stopped.Drafts})
    c.answer(stopped.BLOCKS, run, "draft", "accept", by="person")
    assert c.run(stopped.BLOCKS, "rank people", run).status == "blocked"
    continued: list[str] = []
    def background(home: Path, run_id: str) -> None:
        continued.append(run_id)

    monkeypatch.setattr(api, "continue_in_background", background)  # the page's buttons go through the API
    base, server = serving(tmp_path)
    try:
        page = get(f"{base}/stop/{run}/build")
        assert 'action="/send-back"' in page and '<option value="draft" selected>draft</option>' in page
        token = re.search(r'name="token" value="([^"]+)"', page)
        assert token is not None
        status, _ = post(f"{base}/send-back", {"token": token.group(1), "run": run, "to": "draft",
                                               "note": "bob closed 1 ticket: he ranks 3 of 4"})
        assert status == 303 and continued == [run]
        [back] = [e.payload for e in Store(tmp_path / "jaynes.sqlite3").events("RunSentBack", run_id=run)]
        assert (back["to"], back["note"], back["by"]) == ("draft", "bob closed 1 ticket: he ranks 3 of 4", "person")
        assert c.run(stopped.BLOCKS, "rank people", run).step == "draft"  # the draft runs again with the note
        assert "3 of 4" in str(c.output(run, stopped.DRAFTED))
    finally:
        server.shutdown()


def test_a_long_question_keeps_its_first_sentence_as_the_heading_and_the_rest_below(tmp_path: Path) -> None:
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.stopped:WORDY", "--task", "t",
              "--runtime", "quick=tests.stopped:Agent"])
    base, server = serving(tmp_path)
    try:
        page = get(base + "/next")
    finally:
        server.shutdown()
    assert "<h1>Ship it?</h1>" in page and "<h1>Ship it? Added" not in page
    # a rest that long is a summary the agent wrote into the question: folded, so the page opens on what matters
    assert ('<details class="fold"><summary>The whole question</summary><p class="ask">Added one draft, called '
            'draft one, after reading everything there was to read') in page


def test_a_stop_opened_before_its_step_said_how_to_show_it_shows_as_the_step_does_now(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from jaynes.role import Shown
    from tests import stopped
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.stopped:ROLE", "--task", "write the draft",
              "--runtime", "quick=tests.stopped:Agent"])
    monkeypatch.setattr(stopped.DRAFT, "shows", lambda out, _: [Shown("The draft", f"{out.text}, in full.")])
    base, server = serving(tmp_path)
    try:
        page = get(base + "/next")
    finally:
        server.shutdown()
    front, _, details = page.partition('<details class="fold"><summary>Details')
    assert "<b>The draft</b><p>draft one, in full.</p>" in front
    assert 'class="label">Output' in details and 'class="label">Output' not in front


def test_backticks_in_what_agents_write_are_shown_as_code(tmp_path: Path) -> None:
    from jaynes.review import marked
    assert marked("run `tenure serve` on <port>") == "run <code>tenure serve</code> on &lt;port&gt;"
    assert marked("a ` alone") == "a ` alone"
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.stopped:ROLE", "--task", "add `mul` to calc",
              "--runtime", "quick=tests.stopped:Agent"])
    base, server = serving(tmp_path)
    try:
        page = get(base + "/next")
    finally:
        server.shutdown()
    assert "add <code>mul</code> to calc" in page and "`mul`" not in page


def test_after_an_accept_the_page_opens_the_runs_next_stop(tmp_path: Path) -> None:
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.two_stops:ROLE", "--task", "t",
              "--runtime", "quick=tests.two_stops:Agent"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    try:
        first = get(base + "/next")
        assert "Waiting for you" in first and f'href="/stop/{run}/draft"' in first  # both stops at once
        assert "The run went on with this without waiting for you" in first
        fields = urllib.parse.urlencode({**form(first), "answer": "accept"}).encode()
        opener = urllib.request.build_opener(NoRedirect(), urllib.request.HTTPCookieProcessor(JAR))
        try:
            opener.open(urllib.request.Request(base + "/answer", data=fields), timeout=10)
            raise AssertionError("expected a redirect")
        except urllib.error.HTTPError as err:
            assert err.code == 303 and err.headers["Location"] == f"/stop/{run}/draft"
    finally:
        server.shutdown()


def finish(base: str, home: Path, run: str) -> None:
    status, _ = post(base + "/answer", {**form(get(base + "/next")), "answer": "accept", "note": ""})
    assert status == 303
    store = Store(home / "jaynes.sqlite3")
    deadline = time.monotonic() + 30
    while not list(store.events("RunFinished", run_id=run)):
        assert time.monotonic() < deadline, (home / "runs" / run / "continue.log").read_text()
        time.sleep(0.05)


def test_a_correction_shows_on_the_run_and_the_person_files_it_as_another_kind(served: tuple[str, Path, str]) -> None:
    from tests.stopped import ROLE
    base, home, run = served
    finish(base, home, run)
    Conductor(home).correct(ROLE, run, "draft", "error", "the draft named the wrong client", by="agent")

    page = get(f"{base}/run/{run}")
    assert "Corrected after delivery" in page and "the draft named the wrong client" in page
    assert 'chip red">error</span>' in page and "draft, counted at draft" in page

    status, _ = post(base + "/correction", {**form(page), "kind": "preference"})

    assert status == 303
    page = get(f"{base}/run/{run}")
    assert 'chip amber">new preference</span>' in page and "draft, not counted" in page
    [(_, now)] = Conductor(home).corrections(run)
    assert (now["kind"], now["by"], now["was"]) == ("preference", "person", "error")


def test_a_braked_stop_says_so_and_how_many_clean_deliveries_release_it(tmp_path: Path) -> None:
    from tests.stopped import ROLE
    c = Conductor(tmp_path)
    for _ in range(2):
        cli.main(["run", "--home", str(tmp_path), "--role", "tests.stopped:ROLE", "--task", "t",
                  "--runtime", "quick=tests.stopped:Agent"])
        run = Store(tmp_path / "jaynes.sqlite3").runs()[-1]
        c.answer(ROLE, run, "draft", "return", note="wrong", by="person")
        c.dismiss(run)
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.stopped:ROLE", "--task", "t",
              "--runtime", "quick=tests.stopped:Agent"])
    run = Store(tmp_path / "jaynes.sqlite3").runs()[-1]
    base, server = serving(tmp_path)
    try:
        page = get(f"{base}/stop/{run}/draft")
        assert 'chip red">braked</span>' in page
        assert "clean deliveries until it stops holding the run" in page
        assert "nothing after it runs until you answer" in page
        assert "<span class='chip red'>braked</span>" in get(base + "/role/stopped")
    finally:
        server.shutdown()


def test_a_merged_run_says_so_at_its_stop_and_undo_on_its_page_reverts_it(tmp_path: Path) -> None:
    main = tmp_path / "repo"
    main.mkdir()
    (main / "calc.py").write_text("def add(a, b):\n    return a - b\n")
    for args in (["init", "-q", "-b", "main"], ["add", "-A"], ["commit", "-qm", "start"]):
        subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@t", *args], cwd=main, check=True)
    cli.main(["run", "--home", str(tmp_path / "home"), "--role", "tests.merging:ROLE", "--task", "add should add",
              "--project", str(main), "--runtime", "quick=tests.merging:Agent"])
    [run] = Store(tmp_path / "home" / "jaynes.sqlite3").runs()
    assert "a + b" in (main / "calc.py").read_text()  # merged before any answer
    base, server = serving(tmp_path / "home")
    try:
        assert "Already merged into main by itself" in get(f"{base}/stop/{run}/accept")
        page = get(f"{base}/run/{run}")
        assert "into main by itself, at" in page and "Undo the merge" in page

        status, _ = post(base + "/undo", {**form(page), "note": "not now"})

        assert status == 303
        assert "a - b" in (main / "calc.py").read_text()
        page = get(f"{base}/run/{run}")
        assert "reverted on main: not now" in page and "Undo the merge" not in page
    finally:
        server.shutdown()


def test_a_long_run_shows_its_steps_in_one_line_with_the_steps_done_counted() -> None:
    r = role("long", agent="quick")
    names = [f"s{n}" for n in range(1, 13)]
    for name in names:
        r.step(name, output=Note, about=f"does {name}")
    items = [Stage(n, "checked" if i < 6 else "waiting" if i == 6 else "todo", "") for i, n in enumerate(names)]
    line = stage_track(items, r, "s7")
    assert re.findall(r"<li class=\"(\w+)\"[^>]*><i></i>([^<]+)</li>", line) == [
        ("done", "6 done"), ("here", "s7"), ("todo", "s8"), ("todo", "s9"), ("todo", "s10"), ("todo", "s11"),
        ("todo", "1 more")]
    assert 'title="does s7"' in line and 'title="s1, s2, s3, s4, s5, s6"' in line
    items[2] = Stage("s3", "todo", "")  # a step added to the role after it ran past it
    assert re.findall(r"<li class=\"(\w+)\"[^>]*><i></i>([^<]+)</li>", stage_track(items, r, "s7"))[:3] == [
        ("done", "5 done"), ("todo", "s3"), ("here", "s7")]
