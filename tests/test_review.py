"""The review page, through HTTP: a run waits at a stop, is answered from the page, and continues (M3)."""

from __future__ import annotations

import fcntl
import json
import re
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Iterator
from http.cookiejar import CookieJar
from pathlib import Path

import pytest

from jaynes import cli
from jaynes.review import ReviewServer, make_server
from jaynes.store import Store
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
