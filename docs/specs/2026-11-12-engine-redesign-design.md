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

- **Core** (`engine/crates/core`): one pure function, `decide(run, input) -> (events, commands)`. A run's state
  is only the fold of its events (`Run::fold`). Every guarantee of `docs/guarantees.md` is a property of this
  function and is tested on thousands of random runs (`tests/guarantees.rs`).
- **Shell** (`engine/crates/shell`, binary `jaynes-engine`): records the events of each input in one SQLite
  transaction (`synchronous=full`, each entry chained to the one before), carries out the commands, and turns what
  happened into the next input. One process drives a run at a time (a file lock); inputs from people while it
  works wait in an inbox it takes between commands.
- **Protocol 1** (`engine/crates/shell/src/protocol.rs`, schema in `engine/protocol/role-protocol-1.schema.json`):
  the engine starts the role program per call, writes one request line, reads one reply line. Calls: `hello`,
  `describe`, `prompt`, `execute`, `check`, `items`. The role never decides what runs next and never writes what
  an answer will do. A role is correct when it passes `jaynes-engine conformance DIR`, not when it uses a library.
- **Roles**: `engine/roles/small-change` is the first, in Python, using the Python engine's box for the suite.

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

**Ceilings: working values until the owner sets his own.** Low: 20 minutes and 2 million tokens; high: 2 hours
and 10 million. Tokens are those sent and received; tokens read back from the cache are recorded apart. Said in
`RunStarted`; raised only by the person; an attempt's deadline is what is left.

**Checks are `open` or `hidden`, not shape or behaviour.** What decides how a check is treated is whether the
agent may know it. An open check (a field, a length, and also the project's own tests, which the agent runs
itself) is listed in the prompt, and a failure goes back to the same agent session to be fixed in place (up to
`fixes`, default 2), spending no attempt. A hidden check is never named to the agent; a failure tells it only the
check's sentence and spends the attempt.

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

**Who answers.** Only the person. The core alone answers with a rule, only a leave to read inside the project
(`read-inside-project`), and records the rule's name. An answer that claims to be a rule, or comes from an agent,
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

- The attention engine (sampled stops, the anytime-valid certification, the brake): every declared stop asks.
  It is a projection over the record and ports next.
- The page; plans of runs; merging alone and Undo; the hidden proof helpers of build-feature and fix-bug; roles
  other than small-change; the Codex adapter; leaves asked in the middle of a turn (the core handles them; Claude
  Code runs in `dontAsk`, so none arise yet).
- Replaying the Python engine's 53 recorded runs as an oracle; the small model of the core with proofs.

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
