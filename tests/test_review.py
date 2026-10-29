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
from jaynes.review import ReviewServer, diff_html, make_server
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
    page = get(base + "/")
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
    assert "Nothing is waiting" in get(base + "/")
    run_page = get(f"{base}/run/{run}")
    assert "accepted" in run_page and "finished" in run_page
    assert 'class="stages"' in run_page and 'class="timeline"' not in run_page
    role_page = get(base + "/role/stopped")
    assert re.search(r"draft.*?1 label", role_page, re.S)
    assert "StopAnswered" in get(f"{base}/log?run={run}")


def test_change_from_the_page_is_checked_and_replaces_the_output(served: tuple[str, Path, str]) -> None:
    base, home, run = served
    fields = form(get(base + "/"))
    status, body = post(base + "/answer", {**fields, "answer": "change", "output": "not json"})
    assert status == 400 and "not JSON" in body
    status, _ = post(base + "/answer", {**fields, "answer": "change", "output": json.dumps({"text": "draft two"})})
    assert status == 303
    store = Store(home / "jaynes.sqlite3")
    assert [e.payload["source"] for e in store.events("Checked", run_id=run)][-1] == "person"


def test_return_without_a_note_is_refused(served: tuple[str, Path, str]) -> None:
    base, home, run = served
    status, body = post(base + "/answer", {**form(get(base + "/")), "answer": "return", "note": "  "})
    assert status == 400 and "needs a note" in body
    assert list(Store(home / "jaynes.sqlite3").events("StopAnswered", run_id=run)) == []


def test_answers_from_another_site_are_refused(served: tuple[str, Path, str]) -> None:
    base, home, run = served
    fields = form(get(base + "/"))
    assert post(base + "/answer", {**fields, "token": "guess", "answer": "accept"})[0] == 403
    assert post(base + "/answer", {**fields, "answer": "accept"}, {"Origin": "https://evil.example"})[0] == 403
    assert post(base + "/answer", {**fields, "answer": "accept"}, {"Host": "evil.example"})[0] == 403
    assert list(Store(home / "jaynes.sqlite3").events("StopAnswered", run_id=run)) == []


def test_time_spent_counts_from_the_first_view_across_reloads(served: tuple[str, Path, str]) -> None:
    base, home, run = served
    get(base + "/")
    time.sleep(1.2)
    status, _ = post(base + "/answer", {**form(get(base + "/")), "answer": "accept"})
    assert status == 303
    [answered] = [e.payload for e in Store(home / "jaynes.sqlite3").events("StopAnswered", run_id=run)]
    assert answered["seconds_spent"] >= 1.2


def test_an_answer_while_another_process_holds_the_run_is_refused(served: tuple[str, Path, str]) -> None:
    base, home, run = served
    fields = form(get(base + "/"))
    with (home / "runs" / f"{run}.lock").open("a") as fh:
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
    page = get(base + "/")  # answered, but nothing continued it: the run is idle
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
    page = get(base + "/")
    assert 'action="/dismiss"' in page
    status, _ = post(base + "/dismiss", {**form(page)}, jar=None)
    assert status == 403 and not list(store.events("RunFinished", run_id=run))  # only the person's browser
    status, _ = post(base + "/dismiss", {**form(page)})
    assert status == 303 and "Nothing is waiting" in get(base + "/")
    shown = get(f"{base}/run/{run}")
    assert '<div class="n">dismissed</div>' in shown and "dismissed; nothing was delivered" in shown


def test_after_an_answer_the_page_follows_the_run_it_continues(served: tuple[str, Path, str]) -> None:
    base, _, run = served
    fields = urllib.parse.urlencode({**form(get(base + "/")), "answer": "return", "note": "shorter"}).encode()
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
        post(base + "/answer", {**form(get(base + "/")), "answer": "accept"})
        store = Store(tmp_path / "jaynes.sqlite3")
        deadline = time.monotonic() + 30
        while '<option value="outline">' not in (page := get(base + "/")):
            assert time.monotonic() < deadline, (tmp_path / "runs" / run / "continue.log").read_text()
            time.sleep(0.05)
        assert '<option value="outline">back to outline</option>' in page
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
    fields = form(get(base + "/"))
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
        [link] = re.findall(r'<a class="item[^"]*" href="([^"]+)"', get(base + "/"))
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
