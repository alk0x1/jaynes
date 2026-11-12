"""A stop that shows the output as decisions: the person accepts or rejects each, accepted ones are locked against
the agent, and only what was rejected is redone."""

from __future__ import annotations

from pathlib import Path

import pytest
from pydantic import BaseModel

from jaynes.conductor import AnswerRejected, Conductor, project
from jaynes.role import Choice, Decision, Pass, Picture, Witnesses, role, stop
from tests.fakes import Scripted, write


class Look(BaseModel):
    theme: str
    accent: str
    heading: str
    text: str


def decisions(o: Look) -> list[Decision]:
    return [Decision("theme", "Theme", o.theme), Decision("accent", "Accent colour", o.accent, after=("theme",)),
            Decision("heading", "Heading face", o.heading), Decision("text", "Text face", o.text)]


LOOKS = role("looks", agent="fake")
LOOK = LOOKS.step("look", output=Look, stop=stop(0.2, waits=True), decides=decisions)

FIRST = {"theme": "dark", "accent": "#e8a33d", "heading": "Fraunces", "text": "Inter"}


def started(tmp_path: Path, agent: Scripted) -> tuple[Conductor, str]:
    c = Conductor(tmp_path, {"fake": lambda: agent})
    assert c.run(LOOKS, "a look").status == "waiting"
    return c, c.store.runs()[0]


def test_rejecting_one_decision_returns_only_it_and_locks_the_rest(tmp_path: Path) -> None:
    agent = Scripted(plan={("look", 2): write({**FIRST, "text": "Source Serif"})}, default=lambda s: FIRST)
    c, rid = started(tmp_path, agent)
    c.answer(LOOKS, rid, "look", "accept", rejected={"text": "too plain, I want character"}, by="person")
    [answered] = [e.payload for e in c.store.events("StopAnswered")]
    assert answered["answer"] == "return" and answered["label"] == "error"
    assert answered["locked"] == {"theme": "dark", "accent": "#e8a33d", "heading": "Fraunces"}
    assert "Text face (Inter): too plain, I want character" in answered["note"]
    c.run(LOOKS, "a look", rid)
    prompt = agent.prompts[-1]
    assert "locked" in prompt and "- theme (Theme): dark" in prompt and "- heading (Heading face): Fraunces" in prompt
    assert "- text (" not in prompt.split("locked", 1)[1]


def test_an_output_that_changes_a_locked_decision_never_reaches_the_person(tmp_path: Path) -> None:
    agent = Scripted(plan={("look", 2): write({**FIRST, "theme": "light", "text": "Source Serif"}),
                           ("look", 3): write({**FIRST, "text": "Source Serif"})}, default=lambda s: FIRST)
    c, rid = started(tmp_path, agent)
    c.answer(LOOKS, rid, "look", "accept", rejected={"text": "too plain"}, by="person")
    assert c.run(LOOKS, "a look", rid).status == "waiting"
    [rejected] = [e.payload for e in c.store.events("TurnRejected")]
    assert any("Theme" in r and "dark" in r and "light" in r for r in rejected["reasons"])
    assert c.output(rid, LOOK) == Look(**{**FIRST, "text": "Source Serif"})


def test_dropping_a_locked_decision_is_rejected_too(tmp_path: Path) -> None:
    class Loose(BaseModel):
        items: dict[str, str]

    loose = role("loose", agent="fake")
    step = loose.step("pick", output=Loose, stop=stop(0.2, waits=True),
                      decides=lambda o: [Decision(k, k.title(), v) for k, v in o.items.items()])
    agent = Scripted(plan={("pick", 2): write({"items": {"b": "2"}}), ("pick", 3): write({"items": {"a": "1", "b": "3"}})},
                     default=lambda s: {"items": {"a": "1", "b": "2"}})
    c = Conductor(tmp_path, {"fake": lambda: agent})
    c.run(loose, "t")
    rid = c.store.runs()[0]
    c.answer(loose, rid, "pick", "accept", rejected={"b": "another"}, by="person")
    c.run(loose, "t", rid)
    [rejected] = [e.payload for e in c.store.events("TurnRejected")]
    assert any("'a'" in r and "dropped" in r for r in rejected["reasons"])
    assert c.output(rid, step) == Loose(items={"a": "1", "b": "3"})


def test_the_person_may_change_their_mind_and_what_follows_from_it_unlocks(tmp_path: Path) -> None:
    agent = Scripted(plan={("look", 2): write({**FIRST, "text": "Source Serif"}),
                           ("look", 3): write({**FIRST, "theme": "light", "accent": "#0a6", "text": "Source Serif"})},
                     default=lambda s: FIRST)
    c, rid = started(tmp_path, agent)
    c.answer(LOOKS, rid, "look", "accept", rejected={"text": "too plain"}, by="person")
    c.run(LOOKS, "a look", rid)
    c.answer(LOOKS, rid, "look", "accept", rejected={"theme": "light after all"}, by="person")
    second = [e.payload for e in c.store.events("StopAnswered")][-1]
    assert second["locked"] == {"heading": "Fraunces", "text": "Source Serif"}  # the accent follows the theme
    assert c.run(LOOKS, "a look", rid).status == "waiting"
    assert not list(c.store.events("TurnRejected"))
    assert c.output(rid, LOOK) == Look(theme="light", accent="#0a6", heading="Fraunces", text="Source Serif")


def test_accepting_with_nothing_rejected_locks_every_decision(tmp_path: Path) -> None:
    c, rid = started(tmp_path, Scripted(default=lambda s: FIRST))
    c.answer(LOOKS, rid, "look", "accept", by="person")
    [answered] = [e.payload for e in c.store.events("StopAnswered")]
    assert answered["answer"] == "accept" and answered["locked"] == {
        "theme": "dark", "accent": "#e8a33d", "heading": "Fraunces", "text": "Inter"}


def test_a_rejection_needs_a_note_and_a_known_decision(tmp_path: Path) -> None:
    c, rid = started(tmp_path, Scripted(default=lambda s: FIRST))
    with pytest.raises(AnswerRejected, match="say what is wrong with Text face"):
        c.answer(LOOKS, rid, "look", "accept", rejected={"text": " "}, by="person")
    with pytest.raises(AnswerRejected, match="'mood' is not a decision of this output"):
        c.answer(LOOKS, rid, "look", "accept", rejected={"mood": "no"}, by="person")
    assert list(c.store.events("StopAnswered")) == []


def test_decisions_need_an_output_and_a_stop() -> None:
    r = role("bad", agent="fake")
    with pytest.raises(ValueError, match="shows its output as decisions, but has no stop"):
        r.step("s", output=Look, decides=decisions)


class Item(BaseModel):
    id: str
    text: str


class Items(BaseModel):
    items: list[Item]


def item_decisions(o: Items) -> list[Decision]:
    return [Decision(i.id, i.id.title(), i.text, pictures=(Picture("looks", f"{i.id}.png"),)) for i in o.items]


ITEMS = Choice(items=lambda o: [(i.id, i.text) for i in o.items],
               keep=lambda o, ids: Items(items=[i for x in ids for i in o.items if i.id == x]))
THREE = {"items": [{"id": "ledger", "text": "a bank statement"}, {"id": "booklet", "text": "a work booklet"},
                   {"id": "board", "text": "a tactics board"}]}

OFFER = role("offer", agent="fake")
LOOKS_ = OFFER.step("looks", output=Items)
SHOWN = OFFER.step("shown", output=Items, needs=[LOOKS_], run_by=lambda ctx: ctx.needs[LOOKS_].value,
                   effect="pure", max_turns=1, stop=stop(0.2, waits=True), choose=ITEMS, decides=item_decisions)


def offered(tmp_path: Path, agent: Scripted) -> tuple[Conductor, str]:
    c = Conductor(tmp_path, {"fake": lambda: agent})
    assert c.run(OFFER, "three looks").status == "waiting"
    return c, c.store.runs()[0]


def test_dropping_alone_where_the_person_chooses_chooses_what_is_left(tmp_path: Path) -> None:
    c, rid = offered(tmp_path, Scripted(default=lambda s: THREE))
    c.answer(OFFER, rid, "shown", "accept", dropped={"booklet": "hated it", "board": ""}, by="person")
    [answered] = [e.payload for e in c.store.events("StopAnswered")]
    assert answered["answer"] == "choose" and answered["chosen"] == ["ledger"]
    assert [d["id"] for d in answered["dropped"]] == ["booklet", "board"]
    assert c.output(rid, SHOWN) == Items(items=[Item(id="ledger", text="a bank statement")])


def test_a_base_and_drops_go_back_to_the_agent_before_a_step_jaynes_runs(tmp_path: Path) -> None:
    again = {"items": [{"id": "ledger-2", "text": "a statement, louder"}, {"id": "ledger-3", "text": "a passbook"}]}
    agent = Scripted(plan={("looks", 2): write(again)}, default=lambda s: THREE)
    c, rid = offered(tmp_path, agent)
    c.answer(OFFER, rid, "shown", "accept", dropped={"booklet": "hated it", "board": ""},
             based={"ledger": "the best, but still bad"}, by="person")
    [answered] = [e.payload for e in c.store.events("StopAnswered")]
    assert answered["answer"] == "return" and answered["to"] == "looks"
    note = answered["note"]
    assert note.index("dropped these for good") < note.index("- Booklet (a work booklet): hated it")
    assert "- Board (a tactics board)" in note and "Start from these" in note
    assert "- Ledger (a bank statement): the best, but still bad" in note
    assert c.run(OFFER, "three looks", rid).status == "waiting"
    prompt = agent.prompts[-1]
    assert "dropped for good in this run" in prompt and "- at shown: Booklet (a work booklet): hated it" in prompt
    shown = project(c.store.events(run_id=rid)).step("shown").checked  # the new looks wait at the stop
    assert [d.value for d in c.decisions_of(SHOWN, shown)] == ["a statement, louder", "a passbook"]
    assert c.decisions_of(SHOWN, shown)[0].pictures == (Picture("looks", "ledger-2.png"),)


def test_a_dropped_decision_brought_back_is_rejected(tmp_path: Path) -> None:
    agent = Scripted(plan={("look", 2): write(FIRST), ("look", 3): write({**FIRST, "text": "Source Serif"})},
                     default=lambda s: FIRST)
    c, rid = started(tmp_path, agent)
    c.answer(LOOKS, rid, "look", "accept", dropped={"text": "never Inter"}, by="person")
    assert c.run(LOOKS, "a look", rid).status == "waiting"
    [rejected] = [e.payload for e in c.store.events("TurnRejected")]
    assert any("dropped Text face (Inter) for good" in r for r in rejected["reasons"])
    assert c.output(rid, LOOK) == Look(**{**FIRST, "text": "Source Serif"})


def test_a_decision_marked_twice_is_refused(tmp_path: Path) -> None:
    c, rid = started(tmp_path, Scripted(default=lambda s: FIRST))
    with pytest.raises(AnswerRejected, match="Text face is marked twice"):
        c.answer(LOOKS, rid, "look", "accept", rejected={"text": "x"}, dropped={"text": ""}, by="person")


def test_checks_see_what_the_person_dropped(tmp_path: Path) -> None:
    seen: list[list[str]] = []

    def notes(o: Look, needs: Witnesses) -> Pass:
        seen.append([d.id for d in needs.dropped])
        return Pass()

    r = role("seen", agent="fake")
    r.step("look", output=Look, stop=stop(0.2, waits=True), decides=decisions, check=notes)
    agent = Scripted(plan={("look", 2): write({k: v for k, v in FIRST.items()} | {"text": "Lora"})},
                     default=lambda s: FIRST)
    c = Conductor(tmp_path, {"fake": lambda: agent})
    c.run(r, "a look")
    rid = c.store.runs()[0]
    c.answer(r, rid, "look", "accept", dropped={"text": ""}, by="person")
    c.run(r, "a look", rid)
    assert seen[0] == [] and seen[-1] == ["text"]
