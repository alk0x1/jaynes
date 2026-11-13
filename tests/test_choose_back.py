"""A person choosing among an output's items, Jaynes sending a step back, and what trusted steps may see."""

from __future__ import annotations

from pathlib import Path

import pytest
from pydantic import BaseModel

from jaynes.conductor import AnswerRejected, Conductor
from jaynes.role import Back, Blocked, Choice, Fail, Pass, StepContext, Witnesses, role, stop
from tests.fakes import Scripted, write


class Item(BaseModel):
    id: str
    text: str


class Items(BaseModel):
    items: list[Item]


def two_to_five(out: Items, _: Witnesses) -> Pass | Fail:
    return Pass() if 2 <= len(out.items) <= 5 else Fail(f"keep 2 to 5 items, not {len(out.items)}")


CHOICE = Choice(items=lambda out: [(i.id, i.text) for i in out.items],
                keep=lambda out, ids: Items(items=[next(i for i in out.items if i.id == x) for x in ids]))

PICK = role("pick", agent="fake")
CANDIDATES = PICK.step("moments", output=Items, check=two_to_five, stop=stop(0.2, sampled=False), choose=CHOICE)
PICK.step("use", output=Items, needs=[CANDIDATES])

FOUR = {"items": [{"id": f"M{i}", "text": f"moment {i}"} for i in range(1, 5)]}


def test_choosing_keeps_the_chosen_items_in_the_persons_order_as_an_accept(tmp_path: Path) -> None:
    agent = Scripted(default=lambda s: FOUR)
    c = Conductor(tmp_path, {"fake": lambda: agent})
    run = c.run(PICK, "t")
    rid = c.store.runs()[0]
    assert run.status == "waiting"
    c.answer(PICK, rid, "moments", "choose", chosen=["M3", "M1"], by="person")
    assert c.output(rid, CANDIDATES) == Items(items=[Item(id="M3", text="moment 3"), Item(id="M1", text="moment 1")])
    [answered] = [e.payload for e in c.store.events("StopAnswered")]
    assert (answered["label"], answered["chosen"]) == ("correct", ["M3", "M1"])
    assert c.run(PICK, "t", rid).status == "finished"
    # use ran on all four before the person chose, and again on the chosen
    given = next((tmp_path / "runs").glob("*/use/turn-2/workspace/inputs/moments.json")).read_text()
    assert given.index("M3") < given.index("M1") and "M2" not in given


def test_a_choice_is_checked_like_the_output_and_names_only_its_items(tmp_path: Path) -> None:
    c = Conductor(tmp_path, {"fake": lambda: Scripted(default=lambda s: FOUR)})
    c.run(PICK, "t")
    rid = c.store.runs()[0]
    with pytest.raises(AnswerRejected, match="keep 2 to 5 items, not 1"):
        c.answer(PICK, rid, "moments", "choose", chosen=["M2"], by="person")
    with pytest.raises(AnswerRejected, match="'M9' is not one of M1, M2, M3, M4"):
        c.answer(PICK, rid, "moments", "choose", chosen=["M2", "M9"], by="person")
    with pytest.raises(AnswerRejected, match="chosen twice"):
        c.answer(PICK, rid, "moments", "choose", chosen=["M2", "M2"], by="person")
    with pytest.raises(AnswerRejected, match="only a person answers it"):
        c.answer(PICK, rid, "moments", "choose", chosen=["M2", "M1"], by="agent")
    assert list(c.store.events("StopAnswered")) == []


class Built(BaseModel):
    summary: str


class Review(BaseModel):
    defects: list[str]


LOOP = role("loop", agent="fake")
BUILD = LOOP.step("build", output=Built)
REVIEW = LOOP.step("review", output=Review, needs=[BUILD],
                   back=Back(to=BUILD, when=lambda out, _: list(out.defects), rounds=2))
LOOP.step("accept", needs=[REVIEW], stop=stop(0.05, sampled=False))


def test_jaynes_sends_a_step_back_with_what_was_found_and_stops_after_its_rounds(tmp_path: Path) -> None:
    agent = Scripted(plan={("review", 1): write({"defects": ["the title is cut"]}),
                           ("review", 2): write({"defects": ["the button is grey"]}),
                           ("review", 3): write({"defects": ["still grey"]}),
                           ("build", 2): write({"summary": "fixed the title"}),
                           ("build", 3): write({"summary": "fixed the button"})},
                     default=lambda s: {"summary": "made it"})
    c = Conductor(tmp_path, {"fake": lambda: agent})
    assert c.run(LOOP, "t").status == "waiting"  # at accept, after two rounds back
    assert agent.counts == {"build": 3, "review": 3}
    builds = [p for p in agent.prompts if "# Step: build" in p]
    assert "the title is cut" in builds[1] and "Jaynes sent the previous output back: step review" in builds[1]
    assert "the button is grey" in builds[2] and "which Jaynes sent back from review" in builds[2]
    returned = [e.payload for e in c.store.events("StepReturned")]
    assert [(r["to"], r["reasons"], r["round"]) for r in returned] == [
        ("build", ["the title is cut"], 1), ("build", ["the button is grey"], 2)]
    rid = c.store.runs()[0]
    assert c.output(rid, REVIEW) == Review(defects=["still grey"])  # what remains goes on to the person
    assert list(c.store.events("StopAnswered")) == []  # no label: nobody answered


def test_a_check_that_cannot_judge_blocks_the_run_without_spending_an_attempt(tmp_path: Path) -> None:
    def broken_tool(out: Built, _: Witnesses) -> Pass | Fail:
        raise Blocked("the measuring tool exited with 2: no browser")

    r = role("blocked", agent="fake")
    r.step("build", output=Built, check=broken_tool)
    c = Conductor(tmp_path, {"fake": lambda: Scripted(default=lambda s: {"summary": "x"})})
    out = c.run(r, "t")
    assert (out.status, out.reasons) == ("blocked", ("the measuring tool exited with 2: no browser",))
    assert list(c.store.events("TurnRejected")) == []


class Shot(BaseModel):
    files: int


def test_a_trusted_step_may_leave_more_files_when_it_says_so_and_private_ones_reach_checks_only(
        tmp_path: Path) -> None:
    def shoot(ctx: StepContext) -> Shot:
        (ctx.workspace / "files" / "private").mkdir(parents=True)
        for i in range(30):
            (ctx.workspace / "files" / f"s{i:02}.png").write_bytes(b"png")
        (ctx.workspace / "files" / "private" / "raw.json").write_text('{"secret": 1}')
        return Shot(files=31)

    seen: list[str] = []

    def reads_private(out: Built, needs: Witnesses) -> Pass | Fail:
        seen.append(needs.files["shots"]["private/raw.json"].read_text())
        return Pass()

    for limit, status in ((20, "blocked"), (40, "finished")):
        r = role(f"shots{limit}", agent="fake")
        shots = r.step("shots", output=Shot, run_by=shoot, max_files=limit, max_turns=1)
        r.step("look", output=Built, needs=[shots], check=reads_private)
        agent = Scripted(default=lambda s: {"summary": "x"})
        c = Conductor(tmp_path / str(limit), {"fake": lambda: agent})
        assert c.run(r, "t").status == status
    assert seen == ['{"secret": 1}']
    ws = next((tmp_path / "40" / "runs").glob("*/look/turn-1/workspace"))
    assert (ws / "inputs" / "shots" / "s00.png").is_file() and not (ws / "inputs" / "shots" / "private").exists()
    assert "private/raw.json" not in agent.prompts[0] and "inputs/shots/s29.png" in agent.prompts[0]


class Heard(BaseModel):
    notes: list[str]
    cost: list[str]


def test_a_trusted_step_hears_what_people_said_and_jaynes_sent_back_and_what_each_step_cost(tmp_path: Path) -> None:
    heard: list[StepContext] = []

    def listen(ctx: StepContext) -> Heard:
        heard.append(ctx)
        return Heard(notes=[n.note for n in ctx.history.notes], cost=[c.step for c in ctx.history.cost])

    r = role("listen", agent="fake")
    build = r.step("build", output=Built, stop=stop(0.2, sampled=False))
    review = r.step("review", output=Review, needs=[build],
                    back=Back(to=build, when=lambda out, _: list(out.defects), rounds=1))
    r.step("notes", output=Heard, needs=[review, build], run_by=listen)
    agent = Scripted(plan={("review", 1): write({"defects": ["cut title"]}),
                           **{("build", n): write({"summary": f"x{n}"}) for n in (1, 2, 3)}},
                     default=lambda s: {"defects": []})
    c = Conductor(tmp_path, {"fake": lambda: agent})
    c.run(r, "t")  # the stop does not hold it: review sends build back, and notes runs on the second build
    rid = c.store.runs()[0]
    c.answer(r, rid, "build", "return", note="use the brand colour", by="person")
    c.run(r, "t", rid)
    c.answer(r, rid, "build", "accept", note="fine now", by="person")
    assert c.run(r, "t", rid).status == "finished"
    first, ctx = heard  # once on the second build, again on the third
    assert [n.answer for n in first.history.notes] == ["back"]
    notes = [(n.step, n.answer, n.by, n.note, n.reasons) for n in ctx.history.notes]
    assert notes == [("build", "back", "jaynes", "sent back from review", ("cut title",)),
                     ("build", "return", "person", "use the brand colour", ())]
    cost = {x.step: x for x in ctx.history.cost}
    assert (cost["build"].turns, cost["build"].tokens_new, cost["build"].tokens_cached) == (3, 330, 0)


def test_a_check_sees_the_files_its_own_turn_left_so_it_can_hold_what_the_output_names_to_them(
        tmp_path: Path) -> None:
    def shoot(ctx: StepContext) -> Shot:
        (ctx.workspace / "files").mkdir()
        (ctx.workspace / "files" / "ref-1.jpg").write_bytes(b"jpg")
        return Shot(files=2)  # names two pictures, left one

    def all_there(out: Shot, needs: Witnesses) -> Pass | Fail:
        own = needs.files.get("shots", {})
        return Pass() if len(own) == out.files else Fail(f"names {out.files} pictures; the turn left {sorted(own)}")

    r = role("own", agent="fake")
    r.step("shots", output=Shot, run_by=shoot, check=all_there, max_turns=1)
    c = Conductor(tmp_path, {"fake": lambda: Scripted(default=lambda s: {"summary": "x"})})
    assert c.run(r, "t").status == "blocked"
    [rejected] = [e.payload for e in c.store.events("TurnRejected")]
    assert rejected["reasons"] == ["names 2 pictures; the turn left ['ref-1.jpg']"]
