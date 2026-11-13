# Engine redesign: design and first slice

Branch `engine-redesign`. Built from the brief (`2026-11-12-engine-redesign-brief.md`) and the research report it
names. The owner's decisions in the brief stand; where this design picked a default, it says so and why.

## The shape

```
person ── jaynes-engine (CLI today, the page next)
              │
        shell (Rust): record in SQLite, role process, agent program, project copy
              │  inputs ↓        ↑ events + commands
        core (Rust): decide(state, input) → events, commands     ← no disk, network, process or clock
              │
        protocol 1 (JSON lines, one process per call)
              │
        role (any language; Python through engine/python/jaynes_role.py)
```

- **Core** (`engine/crates/core`): one pure function, `decide(run, input) -> (events, commands)`. A run's state is only the fold of its events (`Run::fold`). Every guarantee of `docs/guarantees.md` is a property of this function and is tested on thousands of random runs (`tests/guarantees.rs`).
- **Shell** (`engine/crates/shell`, binary `jaynes-engine`): records the events of each input in one SQLite transaction (`synchronous=full`, each entry chained to the one before), carries out the commands, and turns what happened into the next input. One process drives a run at a time (a file lock); inputs from people while it works wait in an inbox it takes between commands.
- **Protocol 1** (`engine/crates/shell/src/protocol.rs`, schema in `engine/protocol/role-protocol-1.schema.json`):
  the engine starts the role program per call, writes one request line, reads one reply line. Calls: `hello`,
  `describe`, `prompt`, `execute`, `check`, `items`. The role never decides what runs next and never writes what
  an answer will do. A role is correct when it passes `jaynes-engine conformance DIR`, not when it uses a library.
- **Roles**: `engine/roles/small-change` and `engine/roles/fix-bug`, in Python, using the Python engine's box for the
  project's commands. fix-bug's docstring says what changed in the port.

## Core and packages

In the core, because moving any of it out would let a role break a guarantee: the record, the order of steps,
the gate of the checks, the one question and its effect sentences, the ceilings, the risk rules, what a step
reads, holds before what cannot be undone. In packages: the methods (roles), the agent adapters, the page.

## Decisions taken in this session

**Risk: two levels, `low` and `high`.** A step or a stop declares `from_risk`; the short path is the steps from
`low`. The rules in code (`risk.rs`): a step that cannot be undone on the short path makes the run high from the
start; a change past 30 changed lines (the Python small-change's number) or 5 files, or touching a file that runs
later outside any box (the Python engine's floor list, plus `Cargo.toml` and `build.rs`), raises it. Only rules
and the person raise it; nothing lowers it. When it rises, the steps that come into the path run, earlier ones
first, and a step that would now read their change runs again.

**Ceilings: the owner accepted these values on 2026-11-12.** Low: 20 minutes and 2 million tokens; high: 2 hours
and 10 million. Tokens are those sent and received; tokens read back from the cache are recorded apart. Said in
`RunStarted`; raised only by the person; an attempt's deadline is what is left.

**Checks are `open` or `hidden`, not shape or behaviour.** What decides how a check is treated is whether the
agent may know it. An open check (a field, a length, and also the project's own tests, which the agent runs
itself) is listed in the prompt, and a failure goes back to the same agent session to be fixed in place (up to
`fixes`, default 2), spending no attempt. A hidden check is never named to the agent; a failure tells it only the
check's sentence and spends the attempt.

**A check may read more than its step.** `reads` names earlier steps a check reads besides what its step uses,
such as fix-bug's proof reading the hidden cases. Only a hidden check may read a hidden step; what a check reads
counts for whether its step is fresh. Added for fix-bug, the first role that needed it. A hidden step may now
read the project as it was, since no step before it has changed it.

**Sampled stops (G17), ported from the Python engine.** A stop declares `budget` and `audit_rate`. When a run
starts, the shell asks the core for each stop's count (`attention::count`, a pure function of the whole record and
of the time, which the shell passes in) and the run records it; a count for another stop's key is ignored. A
stop's key is its role, its step as declared and the role's fingerprint, so any change to the role starts a new
count. A certified stop skips asking when the run's coin (a fingerprint of the run, the step and the round) falls
outside its sample, recorded as `StopSkipped`. A braked stop holds the run and no agent may release it. The
statistic, the 7 days, the brake at two errors in ten and the labels needed (88 at 5%, 20 at 20%) are the Python
engine's. Each event's time is now part of what the record's chain fingerprints, since the count reads it.

**The page (owner's choice, 2026-11-12: direction C).** `jaynes-engine page` serves, on 127.0.0.1, the questions
waiting across runs and the releases an agent made, one question at a time: rows with the offered verdicts, a
note when a verdict asks for one, and one line of context. The context changes no verdict: the person's words go
to the agents that work after the answer (`Answer.context`, in every later prompt). What the answer will do comes
from the core as the person marks the rows. Whoever opened the link with the page's key answers as the person;
anyone else as an agent. An answer is recorded at once, and a separate process carries the run on. Each run has its
own view (steps, cost, what blocks it) with Halt, Resume, Continue, Raise the risk, Raise the ceiling, Abandon and
Correct; the page follows the record and never reloads under the person's hands.

**After a crash.** macOS tells no child that its parent died, so the agent program and each role call note their
process group while they run; the process that takes the run next stops what was left before anything else, and
refuses to drive over a record whose chain does not hold or a run another engine version started.

**Each step works on its own copy.** The run's copy is put, before each step, at the base with the changes of the
earlier steps it uses, and, when it runs again, its own previous change on top. Each step's change is kept as one
commit; the delivery applies them in order. Found on the page's first use: with one copy for the whole run, an
earlier step sent back saw a later step's change.

**Steps are gates, not agents.** `session = "continue"` lets an agent step go on in the session of the agent step
before it. A fix in place resumes the same session. A hidden step never shares a session.

**Faults.** Before a step's first attempt the shell probes what it reaches; a failed probe blocks the run as
Jaynes's fault. A check that cannot judge, a role that does not answer, an agent program that does not start, a
delivery that fails: all Jaynes's, no attempt spent. Found while building, and fixed: the reason of a Jaynes fault
used to reach the agent's next prompt and could name a hidden check; now only the agent's own failures are told to
it. Found in the first real Claude Code turn: when the agent's own guards refuse a write the step may make, the
shell now reads that from the transcript and counts it as Jaynes's fault; a step folder under a `.claude` folder,
which Claude Code refuses to write, now fails at the probe.

**What a step reads is what Jaynes gave it.** The core records, per step, the outputs it was given: its needs and,
for a step on the project, every earlier step that changes the project. Never what the agent says it read. A step
is fresh when it read exactly what it would read now. A correction makes the corrected step run again on top of its
output; a step that used it runs again only if that output came out different (as build systems do). The
guarantees page says plainly that this is not a run started over.

**One question.** Items (listed by the role from the output, each with the fingerprint of its content) and a
verdict per item from a closed list: keep, change, redo, drop, skip. An item left out is kept. The answer carries
the fingerprint of the question as shown; a question replaced since is refused. The core writes what the answer
will do (`effects`) before it is sent, records it with the answer, and the test checks it did that. A kept item is
locked; a dropped one may not come back; both are enforced as open checks on later outputs. A question of the
person's own step holds the run; any other question holds only when its stop says `holds`, and a step that cannot
be undone waits for every answer it rests on.

**Who answers.** The person, and, in one case, an agent. The owner loosened G9 on 2026-11-12: a stop the role
marks `release` (it only asks whether a result may go on, not a matter of taste or direction) may be answered by
an agent, only with keep. That answer is recorded as the agent's, locks nothing, and counts as unconfirmed: a step
that cannot be undone still waits for the person's answer. The person may change it while the run is open, or
correct the delivery later. small-change's stop is a release. The core alone answers with a rule, only a leave to
read inside the project (`read-inside-project`), and records the rule's name; an answer that claims to be a rule
is refused. On the command line, "the person" is whoever answers from a terminal: a weak signal (a program can
fake a terminal), kept until the page with its key speaks this protocol.

**The project copy.** A git clone of the person's repository at its current commit, on `jaynes/<run>`, with its
installed dependencies linked. An output that passes becomes a commit; a failed attempt leaves nothing. The
delivery fetches the branch into the person's repository; their working tree is never touched. Uncommitted work
in the person's folder is not in the copy (the Python engine copied it).

**Versions.** The role folder and the library are copied into the run's folder at the start and the run uses that
copy, so editing the role never changes a running run. `RunStarted` records the role's fingerprint, the engine's
version, the protocol and the record's format.

## Not in this slice

- Plans of runs; merging alone and Undo; build-feature and the design roles; pictures and other content of an item on the page; the Codex adapter; leaves asked in the middle of a turn (
- Replaying the Python engine's 53 recorded runs as an oracle; proving the Rust code itself (step 6 of the ladder).

## The model with proofs (owner's choice, 2026-11-13)

`engine/model`: the core's rules for its next move, for taking an answer and for the gate of the checks, in Lean 4,
with proofs of G1, G2, G3, G4, G5, G9, G10, G12, G13 and G14. A test feeds the model every state, answer and gate
of 800 random runs of the core (84,157 cases) and requires the same decision. Planted defects: three in the model's
moves (the wait before what cannot be undone, the hold, freshness ignoring what was read), all caught by the
comparison, the third only once the states inside one decision were compared too; three in answers and the gate,
caught by the comparison and by the proofs themselves. What stays open: the model takes as given which steps a
step reads; the comparison covers the states the random runs reach.

## Leaves in the middle of a turn (2026-11-13)

Claude Code runs in its default permission mode with `--permission-prompt-tool mcp__jaynes__permit`: what a step's
settings did not allow comes to `jaynes-engine permit`, an MCP server started for the turn. It records a
`PermissionAsked` in the core, in one transaction with reading the state. The core's rule grants reading inside the
project; anything else is a question on the page, which the agent waits for. The answer is recorded at once even
while the run's process is blocked on the turn, and the server reads it from the record. A leave refused is not
counted as work sent back. Checked with a real Claude Code turn: it asked to write outside its folder, the person
refused with a reason, the agent went on, and the run was delivered in 20 s.

## How it was checked

- `cargo test` in `engine/`: the core's 15 tests, including 3,000 random runs per test run checking every
  guarantee after every input, and a test that the random runs reach every state the guarantees are about.
- Nine defects planted by hand in the core (no wait before what cannot be undone, a fault spending an attempt,
  locks ignored, hidden checks given, late reports taken, freshness always true, ceiling skipped, an agent's
  answer taken, the shown fingerprint ignored): the tests caught all nine.
- End to end, with the real binary, the real role over the protocol and a real git project: a small change
  delivered as a branch with no stop; a failing suite fixed in the same session with no attempt spent; a change
  to a Makefile raising the risk and asking; an agent program that does not start spending no attempt.
- A real Claude Code turn on a scratch project: 59 s, one attempt, 8,355 tokens, delivered.

## Acceptance: the change of 678acbc

The change the Python engine's build-feature failed to deliver three times (1 h 35 min, nothing delivered) and that took about 20 minutes made directly. Same request, on a throwaway clone at `ccff401`, through this engine with Claude Code (Opus 5.5, high effort), role small-change, the risk raised to high at the start:

- 9 min 51 s from start to the delivered branch; one attempt, none spent, no fix needed.
- 114,383 tokens sent and received, 2,599,358 read back from the cache.
- The suite (the whole Python test suite, in the box) passed as the engine's check.
- The delivery: `opinion.py`, its tests, the page and the demo, 390 lines added and 22 removed, the same four
  files as the direct commit. Its new tests pass. Two preview tests of `test_review_start.py` fail when run
  outside the box on this machine, and fail the same way on `ccff401` without the change.
- The run then waits, delivered, for the person's answer at the stop, since the change is not small; it goes on
  without holding anything.

Not compared: the quality of the delivery against the direct commit, line by line. One run is one run.
