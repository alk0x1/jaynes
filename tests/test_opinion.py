"""What do you think? at a stop with decisions: the person writes in their words, a Claude that may only read marks
each line, and the page fills the form with the marks; nothing reaches the run until the person sends it."""

from __future__ import annotations

import html
import json
import re
import time
from pathlib import Path

import pytest

from jaynes import cli, opinion
from jaynes.role import Decision, StepContext, role, stop
from jaynes.store import Store
from tests.test_decisions import ITEMS, THREE, Items, item_decisions
from tests.test_review import form, get, post, serving

DECS = [Decision("theme", "Theme", "dark"), Decision("accent", "Accent colour", "#e8a33d"),
        Decision("heading", "Heading face", "Fraunces"), Decision("text", "Text face", "Inter")]


def test_marks_keep_only_known_lines_valid_marks_and_what_is_not_locked() -> None:
    raw = json.dumps({"marks": {"theme": {"mark": "keep"}, "accent": {"mark": "redo", "note": "too orange"},
                                "heading": {"mark": "drop", "note": "never a serif"}, "text": {"mark": "fly"},
                                "ghost": {"mark": "drop"}},
                      "understood": "Fica o tema, a cor é refeita, a serifa sai de vez."})
    marks, said = opinion.marks(raw, DECS, locked={"theme"})
    assert marks == {"accent": {"mark": "no", "note": "too orange"}, "heading": {"mark": "drop", "note": "never a serif"}}
    assert said == "Fica o tema, a cor é refeita, a serifa sai de vez."


def test_marks_read_the_json_inside_a_fence_and_refuse_what_is_not_json() -> None:
    raw = 'Here it is:\n```json\n{"marks": {"text": {"mark": "base", "note": "lighter"}}, "understood": "ok"}\n```'
    assert opinion.marks(raw, DECS, locked=set()) == ({"text": {"mark": "base", "note": "lighter"}}, "ok")
    with pytest.raises(ValueError):
        opinion.marks("I think the theme is fine.", DECS, locked=set())


def test_the_person_writes_what_they_think_and_the_page_gets_marks_but_nothing_is_sent(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    seen: list[tuple[Path, str]] = []

    def reads(here: Path, prompt: str) -> str:
        seen.append((here, prompt))
        return json.dumps({"marks": {"accent": {"mark": "redo", "note": "less orange"},
                                     "heading": {"mark": "drop", "note": ""}, "nope": {"mark": "keep"}},
                           "understood": "Accent redone, Fraunces dropped."})

    monkeypatch.setattr(opinion, "reader", reads)
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.decided:ROLE", "--task", "a look",
              "--runtime", "fake=tests.decided:Agent"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    try:
        page = get(base + "/next")
        assert 'action="/opinion"' in page and f'data-opinion="/opinion/{run}/look"' in page
        assert page.index('id="opinion"') < page.index('class="decs"')  # the box sits above the lines
        assert 'data-v="no"' in page and 'data-v="drop"' in page  # the buttons stay
        status, _ = post(base + "/opinion", {"token": form(page)["token"], "run": run, "step": "look",
                                             "text": "the orange is too loud, and never Fraunces again"})
        assert status == 303
        part = ""
        for _ in range(50):
            part = get(f"{base}/opinion/{run}/look")
            if "data-marks" in part:
                break
            time.sleep(0.1)
        found = re.search(r"data-marks=\"([^\"]+)\"", part)
        assert found and json.loads(html.unescape(found.group(1))) == {
            "accent": {"mark": "no", "note": "less orange"}, "heading": {"mark": "drop", "note": ""}}
        assert "Accent redone, Fraunces dropped." in part
        assert "the orange is too loud" in get(f"{base}/stop/{run}/look")  # the words stay in the box
    finally:
        server.shutdown()
    [(_, prompt)] = seen
    assert prompt.endswith("the orange is too loud, and never Fraunces again")
    assert "- accent: Accent colour = #e8a33d" in prompt
    assert list(Store(tmp_path / "jaynes.sqlite3").events("StopAnswered", run_id=run)) == []  # nothing was sent


def test_a_reader_that_fails_says_so_and_marks_nothing(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    def fails(here: Path, prompt: str) -> str:
        raise TimeoutError("no answer in 240 s")

    monkeypatch.setattr(opinion, "reader", fails)
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.decided:ROLE", "--task", "a look",
              "--runtime", "fake=tests.decided:Agent"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    try:
        page = get(base + "/next")
        post(base + "/opinion", {"token": form(page)["token"], "run": run, "step": "look", "text": "keep it all"})
        part = ""
        for _ in range(50):
            part = get(f"{base}/opinion/{run}/look")
            if "could not" in part:
                break
            time.sleep(0.1)
        assert "It could not mark the lines" in part and "mark them by hand" in part and "data-marks" not in part
    finally:
        server.shutdown()


def draws(ctx: StepContext) -> Items:
    """Three looks, each drawn as a picture among the step's files, as a designer step leaves them."""
    (ctx.workspace / "files").mkdir()
    for item in THREE["items"]:
        (ctx.workspace / "files" / f"{item['id']}.png").write_bytes(b"\x89PNG " + item["id"].encode())
    return Items.model_validate(THREE)


PICTURED = role("pictured", agent="fake")
DRAWN = PICTURED.step("looks", output=Items, run_by=draws, effect="pure", max_turns=1)
PICKED = PICTURED.step("shown", output=Items, needs=[DRAWN], run_by=lambda ctx: ctx.needs[DRAWN].value,
                       effect="pure", max_turns=1, stop=stop(0.2, waits=True), choose=ITEMS, decides=item_decisions)


def test_the_reader_sees_each_decisions_pictures_though_another_step_drew_them(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    seen: list[tuple[dict[str, bytes], str]] = []

    def reads(here: Path, prompt: str) -> str:
        files = {str(f.relative_to(here)): f.read_bytes() for f in (here / "files").rglob("*") if f.is_file()}
        seen.append((files, prompt))
        return json.dumps({"marks": {"board": {"mark": "drop", "note": "the left one"}}, "understood": "ok"})

    monkeypatch.setattr(opinion, "reader", reads)
    cli.main(["run", "--home", str(tmp_path), "--role", "tests.test_opinion:PICTURED", "--task", "three looks"])
    [run] = Store(tmp_path / "jaynes.sqlite3").runs()
    base, server = serving(tmp_path)
    try:
        page = get(base + "/next")
        post(base + "/opinion", {"token": form(page)["token"], "run": run, "step": "shown", "text": "not the left one"})
        for _ in range(50):
            if "data-marks" in get(f"{base}/opinion/{run}/shown"):
                break
            time.sleep(0.1)
    finally:
        server.shutdown()
    [(files, prompt)] = seen
    assert files == {"files/1/ledger.png": b"\x89PNG ledger", "files/2/booklet.png": b"\x89PNG booklet",
                     "files/3/board.png": b"\x89PNG board"}
    assert "- ledger: Ledger = a bank statement (pictures: files/1/ledger.png)" in prompt
    assert "- board: Board = a tactics board (pictures: files/3/board.png)" in prompt
