"""A stop that shows the output as decisions: the person accepts or rejects each, accepted ones are locked against
the agent, and only what was rejected is redone."""

from __future__ import annotations

from pathlib import Path

import pytest
from pydantic import BaseModel

from jaynes.conductor import AnswerRejected, Conductor
from jaynes.role import Decision, role, stop
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
