# Write a role

A role is your method for one kind of work, written as steps in order. Each step has a required output and, usually, a check: code that must pass before the next step starts. A step can stop for you, and a step can be run by Jaynes instead of the agent, for evidence the agent could invent.

## A role in Python

Save this as `count_words.py`:

```python
from pydantic import BaseModel

from jaynes.role import Fail, Pass, StepContext, Witnesses, role


class Topic(BaseModel):
    topic: str
    words: int


class Sentence(BaseModel):
    sentence: str


def read_topic(ctx: StepContext) -> Topic:
    return Topic(topic=ctx.task, words=len(ctx.task.split()) + 5)


def matches(out: Sentence, needs: Witnesses) -> Pass | Fail:
    want = needs[TOPIC].value.words
    got = len(out.sentence.split())
    return Pass() if got == want else Fail(f"the sentence has {got} words; it must have exactly {want}")


ROLE = role("count-words", agent="claude", about="Writes one sentence with an exact number of words.")
TOPIC = ROLE.step("topic", output=Topic, run_by=read_topic)
SENTENCE = ROLE.step("sentence", output=Sentence, needs=[TOPIC], check=matches,
                     instructions="Write one sentence about the topic in inputs/topic.json with exactly "
                                  "the number of words given there.")
```

Jaynes runs the first step itself. Claude Code, with your login, writes the sentence, and the check sends it back until the count is right. `about` says what the role is for; your agent reads it to pick a role for a request. Use `agent="codex"` for Codex.

Run it from the folder that holds the file:

```sh
jaynes run --role count_words:ROLE --task "lighthouses in fog"
```

Or start `jaynes` from that folder and type `count_words:ROLE` under New run. To run it by its name in a project, put it in a [package](packages.md#make-a-package).

## A role in TOML

The steps can live in a TOML file, with the models and checks staying in Python. Save the classes and functions above, without the last three lines, as `count_checks.py`, and change the check to read its need by name, `needs["topic"]`. Then save this as `count_words.toml`:

```toml
name = "count-words"
agent = "claude"
about = "Writes one sentence with an exact number of words."

[[step]]
name = "topic"
output = "count_checks:Topic"
run_by = "count_checks:read_topic"

[[step]]
name = "sentence"
output = "count_checks:Sentence"
needs = ["topic"]
check = ["count_checks:matches"]
stop = { budget = 0.2 }
question = "Is this sentence right: {out.sentence}?"
instructions = "Write one sentence about the topic in inputs/topic.json with exactly the number of words given there."
```

```sh
jaynes run --role count_words.toml --task "lighthouses in fog"
```

A key Jaynes does not know, a reference that does not import, or a question that names a field the output lacks is refused, with the step it is in.

## Stops

`stop=stop(budget=0.2)` makes a step stop for you. The budget is the share of answers you would change that the stop may have once it asks less: with 0.2 it asks one time in ten after 20 clean deliveries in a row. `stop(budget=0.05, sampled=False)` always asks. Most stops show the output and the run goes on; `stop(0.2, waits=True)` holds the run until you answer, for a choice that is yours to make, such as a look among three. It always asks, and only you answer it. [Runs and stops](runs.md#a-stop-asks-less-as-it-proves-right) has how a stop earns that.

## Show the output as decisions

`decides` turns the stop into a list of decisions instead of a block of output. You reject the line that is wrong, with a sentence, and keep the rest: only what you rejected is redone, and what you kept is locked, so the agent cannot change it in the next round. Give each decision an id that stays the same across rounds.

```python
def decisions(o: Look) -> list[Decision]:
    return [Decision("theme", "Theme", o.theme),
            Decision("accent", "Accent colour", o.accent, after=("theme",)),
            Decision("text", "Text face", o.text)]

LOOK = ROLE.step("look", output=Look, stop=stop(0.2, waits=True), decides=decisions)
```

Change on a locked line opens it again. `after` says what a decision follows from: rejecting the theme unlocks the accent too, so the agent may change it to fit.

## Ask a few questions before the work

`ROLE.interview(questions)` makes the run's first step an interview. An agent reads the request and the project, marks the answer it would bet on for each question and says why, and may add a question only you can answer. You answer them all on one screen, by picking an option or writing your own, and the run goes on without you; every later step is told your answers.

```python
ROLE = role("visual-identity")
ASK = ROLE.interview([
    Question("theme", "Which theme for the work screen?", ("Dark", "Light", "Both")),
    Question("work", "Should it look like a work tool?", ("Either way", "I want it to", "I want it not to")),
])
```

Word each question for someone who does not know the role's vocabulary, and give two to five options, or none for an open question.

## Tell the person what each step does

`about` on a step is one sentence for the person: what the step does and, at a stop, what they decide there and what it changes. A stop shows it under its question, and the run's steps in one line that opens into what each does; the role's page lists the steps with who does each. Keep it short: who does the step is shown beside it.

```python
PICK = ROLE.step("pick", output=Looks, needs=[LOOKS], stop=stop(0.2, waits=True),
                 about="Only the colours and fonts are kept; the screen is just a sample.")
```

Without it, the person sees only the step's name. It is not shown to the agent and changes no fingerprint, so you can write it or change it at any time.

`by` names who of your team does an agent's step, such as `by="design director"`; the page shows it in place of "agent". A role can be a conversation between several specialists, each step by one of them: one makes, the next reviews and sends the work back with what it found (`back=Back(to=MAKE, when=...)`), and you answer at the stops. Say who each one is in its instructions; `by` is only what the page shows.

## Steps that act on the world

A step Jaynes runs that acts on the world declares it, with `effect`. A step with `effect="irreversible"`, such as booking, paying or sending, may only follow steps with floor stops, so you approve its whole payload. It runs once, with `ctx.idempotency_key` for the service it calls, and a run that stopped while it was running is blocked instead of running it again.

## Tools the agent can call

The agent of a step works in a closed box: no internet and no tools but its own files. `web=True` lets it read pages as text. When it needs more, the role gives it tools: plain functions that Jaynes runs outside the box while the agent works, recording every call.

```python
from pathlib import Path
from jaynes.browser import photograph
from jaynes.role import ToolContext, role

def capture(ctx: ToolContext, url: str) -> list[object]:
    """A picture of the web page at url. Only pages on the public internet."""
    out = ctx.folder / "ref-1.jpg"
    return [photograph(url, out), out]

ROLE = role("look-alike")
ROLE.step("direction", output=Direction, tools=[capture],
          instructions="Look at every link the task names with capture first.")
```

A tool's first argument is the `ToolContext`: the run, the step, the task, what the person said in the run so far, and `folder`, where it leaves files. Its other arguments are a `str`, `int`, `float` or `bool`, and its docstring is what the agent is told it does. It returns text, a `Path` to a picture in `folder`, which the agent sees, or a list of both. The pictures stay among the step's files, so the stop shows you what the agent saw. `jaynes.browser.photograph` takes a picture of a page on the public internet, and refuses this computer and its network.

A picture a stop shows can link the page it was drawn from: `Picture(step, "direction-1.png", page="direction-1.html")`. The page opens in a new tab, where it runs no script and reaches nothing.

## A role that plans

A role whose step outputs `jaynes.plan.Plan` plans a request: its runs, each with a role, a task, the runs it builds on (`after`) and, when a team planned it, the `specialist` who does it; and why. The step sees the roles the project can use. When its run delivers, Jaynes checks the plan and starts it by itself.

```python
from jaynes.plan import Plan
from jaynes.role import role, stop

ROLE = role("direction", about="Plans a request: which roles, in what order, and why.")
PLAN = ROLE.step("plan", output=Plan, project="read", stop=stop(0.2),
                 instructions="Plan the request with the roles you were given.")
```

## Check a role before it runs

Before any agent works, Jaynes checks the role and refuses it with a list of what to fix:

- a check, question or executor that reads a step its step does not need, or a step of another role;
- a check whose first argument is annotated with another step's output type;
- a step, other than the last, whose output no later step needs and no stop shows, unless it changes the project or acts.

`jaynes check count_words:ROLE` runs the same check alone.

## What a role may import

A role may import these parts of Jaynes, the role API, version 1: `jaynes.role`, `jaynes.plan`, `jaynes.proof`, `jaynes.project`, `jaynes.runners`, `jaynes.suite`, `jaynes.sandbox`, `jaynes.screens`, `jaynes.browser` and `jaynes.inputs`. A package says which version it is written for, and Jaynes refuses one written for another.

The roles that come with Jaynes are the longest examples: they are in [packages/](https://github.com/alk0x1/jaynes/tree/main/packages) on GitHub.
