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
from pathlib import Path

import pytest

from jaynes import cli
from jaynes.review import make_server
from jaynes.store import Store


@pytest.fixture
def served(tmp_path: Path) -> Iterator[tuple[str, Path, str]]:
    """A run of tests.stopped waiting at its stop, and the review server on a free port."""
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.stopped:ROLE", "--task", "write the draft",
              "--runtime", "quick=tests.stopped:Agent"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    server = make_server(tmp_path, port=0, idle_after=0)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_address[1]}", tmp_path, run
    server.shutdown()


def get(url: str) -> str:
    with urllib.request.urlopen(url, timeout=10) as r:
        return r.read().decode()


def post(url: str, fields: dict[str, str], headers: dict[str, str] | None = None) -> tuple[int, str]:
    req = urllib.request.Request(url, data=urllib.parse.urlencode(fields).encode(), headers=headers or {})
    opener = urllib.request.build_opener(NoRedirect())
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
    assert re.search(r'class="step here".*?draft', page, re.S)  # the stage timeline marks where the run waits

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
