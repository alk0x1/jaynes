# Decisions, not text, at the stops

2026-11-11. Decided in chat; no review step.

## The problem

A stop shows the person a block of text and asks them to accept or return the
whole thing. On a real identity run the person disagreed with one line (the
theme) and Return redid every part of the work; two rounds later the
specialists were deadlocked over that same line. The person also accepts most
stops without reading them, because reading is the cost.

## What changes

Three parts, all on the Jaynes page. Claude Code only says "Jaynes has 3
questions for you" and gives the link; there is no second place to answer.

### 1. The interview, once, at the start of a run

- A role's run inputs (`Input`) can carry 3 to 5 options. Every question also
  keeps a free text field: the person may type instead of choosing.
- Before the first working step, a short step reads the project and the request
  that started the run, pre-marks a guess on each question, and may add
  questions of its own. Each added question gets a fixed id, like a decision.
- The person answers every question on one screen, once, and the run goes on
  without them. The answers reach every step's prompt the way notes already do.
- A step that finds, mid-run, something only the person can answer asks it
  then, in the same form: options, a guess, free text.
- Questions are worded for someone who does not know the role's vocabulary
  ("how many different looks do you want to compare", never "directions"), and
  a permissive answer is not a demand ("either way" / "I want it to" / "I want
  it not to").

Why it is worth the wait: in the studies we found, 1 to 3 questions per area
kept 86 to 94% of the gain at a third to two thirds less of the person's
attention ("Ask Less, Ask Better"); in code, about 46 points of accuracy came
within 5 interactions (TiCoder); an agent that reads the project first asks
half as many questions for the same information. What tires people is
questions the project could have answered, not long ones.

### 2. Decisions with ids at a stop

- A step declares where its output's decisions come from (`decisions`). The
  role fixes the known ones, each with a fixed id (theme, accent colour, heading
  face, text face). The agent may add more, with no cap; Jaynes gives each new
  one an id the first time it appears, and the agent must reuse it after.
- The stop card is the list of decisions: each line has accept and reject, and a
  reject asks for one line of note. The picture stays one click away.
- What the person accepts is stored on the run and locked. The next round's
  prompt lists the decisions with their ids and which are locked. A check by
  code sends back, without reaching the person, an output that changes or drops
  a locked decision.
- Send redoes only what was rejected. A decision new in this round is marked
  NEW.
- A locked decision has a "change" button: the lock binds the agent, never the
  person. The role may say which decisions depend on which (the accent depends on
  the theme); changing one unlocks those that depend on it.

### 3. "Why?" explains, never changes

A Claude run by Jaynes reads that step only (output, notes, files) and answers on
the page. It changes nothing in the run. To change something, the person rejects
the line with a note.

## What stays

Taste stops (`waits=True`) are never sampled: they wait for the person every
time. Sampling stays for mechanical checks only, such as whether the agent
applied what was chosen.

## How we know it worked

Rounds until accepted and minutes per stop, which Jaynes already measures,
compared on the identity role before and after.
