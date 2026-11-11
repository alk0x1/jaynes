"""An interview before the work: an agent guesses each answer from the project and the request, the person answers
every question once on one screen, and every later step is told the answers."""

from __future__ import annotations

from pathlib import Path

import pytest
from pydantic import BaseModel

from jaynes.conductor import AnswerRejected, Conductor
from jaynes.role import Interview, Question, role
from tests.fakes import Scripted, write

QUESTIONS = [Question("theme", "Which theme for the work screen?", ("Dark", "Light", "Both")),
             Question("work", "Should it look like a work tool?", ("Either way", "I want it to", "I want it not to"))]


class Look(BaseModel):
    summary: str


ASKS = role("asks", agent="fake")
INTERVIEW = ASKS.interview(QUESTIONS, project=False)
LOOK = ASKS.step("look", output=Look, needs=[INTERVIEW])

GUESSED = {"questions": [
    {"id": "theme", "text": "Which theme for the work screen?", "options": ["Dark", "Light", "Both"],
     "guess": "Dark", "why": "you said dark mode"},
    {"id": "work", "text": "Should it look like a work tool?", "options": ["Either way", "I want it to",
                                                                          "I want it not to"], "guess": "Either way"},
    {"id": "looks", "text": "How many different looks do you want to compare?", "options": ["2", "3", "4"],
     "guess": "3"}]}


def agent() -> Scripted:
    return Scripted(default=lambda step: GUESSED if step == "interview" else {"summary": "a look"})


def test_the_run_waits_for_the_answers_and_every_later_step_is_told_them(tmp_path: Path) -> None:
    a = agent()
    c = Conductor(tmp_path, {"fake": lambda: a})
    assert c.run(ASKS, "a look").status == "waiting"
    rid = c.store.runs()[0]
    assert "- theme: Which theme for the work screen? Options: Dark | Light | Both." in a.prompts[0]
    c.answer(ASKS, rid, "interview", "accept", answers={"work": "I want it not to", "looks": "one bold, one sober"},
             by="person")
    [answered] = [e.payload for e in c.store.events("StopAnswered")]
    assert answered["answer"] == "change" and answered["label"] == "error"  # a guess was wrong
    assert c.run(ASKS, "a look", rid).status == "finished"
    told = a.prompts[-1]
    assert "What the person answered before the work started" in told
    assert "- Which theme for the work screen? Dark" in told  # the guess left standing
    assert "- Should it look like a work tool? I want it not to" in told
    assert "- How many different looks do you want to compare? one bold, one sober" in told


def test_leaving_every_guess_is_an_accept(tmp_path: Path) -> None:
    c = Conductor(tmp_path, {"fake": lambda: agent()})
    c.run(ASKS, "a look")
    rid = c.store.runs()[0]
    c.answer(ASKS, rid, "interview", "accept", answers={"theme": "Dark", "work": " "}, by="person")
    [answered] = [e.payload for e in c.store.events("StopAnswered")]
    assert answered["answer"] == "accept" and answered["label"] == "correct"


def test_an_answer_names_only_questions_asked(tmp_path: Path) -> None:
    c = Conductor(tmp_path, {"fake": lambda: agent()})
    c.run(ASKS, "a look")
    rid = c.store.runs()[0]
    with pytest.raises(AnswerRejected, match="'mood' is not a question"):
        c.answer(ASKS, rid, "interview", "accept", answers={"mood": "calm"}, by="person")


def test_the_guesses_are_checked_before_the_person_sees_them(tmp_path: Path) -> None:
    missing = {"questions": [GUESSED["questions"][0], {**GUESSED["questions"][2], "options": ["2"]}]}
    a = Scripted(plan={("interview", 1): write(missing)}, default=lambda step: GUESSED)
    c = Conductor(tmp_path, {"fake": lambda: a})
    c.run(ASKS, "a look")
    [rejected] = [e.payload for e in c.store.events("TurnRejected")]
    assert any("answer the question 'work'" in r for r in rejected["reasons"])
    assert any("'looks' a text and 3 to 5 options" in r for r in rejected["reasons"])
    rid = c.store.runs()[0]
    assert isinstance(c.output(rid, INTERVIEW), Interview)


def test_the_interview_comes_first_and_its_questions_are_well_formed() -> None:
    r = role("late", agent="fake")
    r.step("first", output=Look)
    with pytest.raises(ValueError, match="the interview is its first step"):
        r.interview(QUESTIONS)
    with pytest.raises(ValueError, match="two to five options"):
        role("one", agent="fake").interview([Question("x", "Pick?", ("only",))])
