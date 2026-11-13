# Engine redesign: brief

**Status: the interview ended on 2026-11-12. Ready to be worked on.** This is the brief for a separate session,
on a separate branch. It records what the owner decided, in his words, and the working plan around those
decisions. It is not a design: the design is the work of that session.

Read with it: the research report `docs/research/reports/Redesenho do motor da Jaynes 2.md` (Portuguese; one-page
summary first) and the notes in `docs/research/research_notes/`. Those are evidence. Where the report favours one
thing and this brief records that the owner decided another, the owner's decision stands (item 10 is one such
case).

Started 2026-11-12. The owner's words are quoted as he wrote them (Portuguese), each followed by what it means.

## Why a redesign

Roles today are Python code loaded inside the engine's process. Each time a role needed something, the engine
gained a field: decisions at stops, then dropping one for good, then taking one as the base, then pictures on a
decision's line. The last three were added in one week for a single role.

> "eu continuo bem preocupado com o split de responsabilidade da jaynes e roles, parece que estamos abrindo a
> jaynes para caber nas roles"

The owner is worried about how responsibility is split between the engine and the roles: the engine is being
opened up to fit the roles. He accepts growth only if the core stays lean and grows by need, "como nodejs com
npms" (like Node with npm packages).

A second symptom, from the same week: at a stop with four generic buttons per line, the owner had to ask an
assistant which button to press.

> "voce percebe como eu to tendo que te perguntar como usar a jayns e que botoes marcar? percebe como nao e o
> ideal isso?"

A third symptom, from the day this brief was started, and the one the owner weighs most. A small change to
Jaynes's own page (a text box above a stop's decisions) was sent through the `build-feature` role three times and
delivered nothing; the same change made directly in a session, test-first, took about 20 minutes (commit
`678acbc`). The event log of the first two runs:

| Run | Before any code (cases, specify) | Build | Result |
|---|---|---|---|
| 1, 48 min | 8.7 min, then 5.5 min rejected by a check and 6.7 min again | three attempts of 9.2, 7.0 and 8.6 min, all rejected | blocked |
| 2, 25 min | 4.8 min, then 4.9 min rejected by a check and 4.8 min again | 10.3 min rejected, second attempt abandoned | abandoned |

Every build attempt failed for a fault of Jaynes itself (the box refused a path the suite needs), and each
failure still cost a whole fresh turn of an agent. Each step was a fresh agent. Jaynes recorded seconds per turn
and no tokens.

> "mas que demora absurda e essa?" ... "nao tamo torrando token atoa?"

> "olha a diferenca, a velocidade e mq vc fez e a inutilidade da jaynes fazendo, isso tem q ser anotado, nao pode
> acontecer senao a jaynes fica inutil"

What an absurd delay; are we burning tokens for nothing? Look at the difference between doing it directly and
Jaynes doing it: this must not happen, or Jaynes is useless.

## Requirement: time spent beyond the direct cost must buy something

The first reading of the incident was "a delivery through Jaynes must never cost much more than the same change
made directly, above every other goal". The owner corrected it the same day:

> "nao sei se essa seria uma boa prioridade, talvez a demora se justifique pra uma mudanca critica ou se a demora
> pra entregar poupassee tempo de retrabalho futuro"

Slowness may be justified for a critical change, or when the time spent delivering saves rework later. So the
rule is not "as fast as direct". It is: the extra time follows the risk of the change, and it has to buy
something that can be shown afterwards (less rework, fewer defects reaching the person). Time that buys nothing is
never justified, and that is what the two runs above were: nothing was delivered. The owner confirmed this
wording ("sim"). Where it ranks among the goals of the redesign is still open. What the two runs ask of the design:

- A small change takes a short path. Steps exist where they are a gate (a check, a permission, a decision of the
  person, a ceiling), not to slice work one agent can hold in one piece.
- A step is a gate in the log, not necessarily a fresh agent. One agent session may span several steps.
- A fault of Jaynes or of the machine is told apart from a fault of the agent, is found before the agent starts
  (the box is tested from inside first), and never spends an attempt.
- A check on shape (a length, a field, a file that must exist) is known to the agent before it delivers and is
  fixed in place in seconds. Only checks on behaviour stay out of the agent's sight.
- Every run has a ceiling of time and tokens, said before it starts, and both are recorded per turn.
- The path follows the risk of the change: short for a small, reversible one, full for a critical one. Rules
  in code judge the risk (decided, item 8 below).
- Jaynes records what each path cost and what rework followed (returns, corrections, later fixes), so whether the
  slow path pays is measured and not assumed.
- The redesign is accepted on real changes with a known direct cost, this one first.

## How the work is to be done

- A formal redesign, "com rigidez de engenharia" (with engineering rigour).
- The effort it takes is not a constraint. Correctness and a design that lasts are.
- On another branch and in another session. The current line of work (stops, the visual identity role) goes on
  in parallel on `main`.
- Never through Jaynes runs: changes to Jaynes's own code are made directly, test-first. The owner, the same
  day: "nao usamos a jaynes pra codar a jaynes, ok?"

## Decided so far

1. **Architecture is decided before the split of features.** Asked what belongs in the core and what in
   packages, the owner answered that the architecture and the stack come first:

   > "acho que antes disso, vamos pensar na arquitetura? eu proponho sairmos de python e irmos pra uma stack
   > que faca sentido, aqui talvez vamos nos afundar em teoria, primeira linguagem que me vem a mente e rust
   > pelo sistema de tipos, mas vamos pensar."

   Leave Python for a stack that makes sense. Rust is the first candidate, for its type system. Not yet final.

2. **The boundary.** A role runs outside the engine's process and talks to the engine only through a fixed,
   versioned protocol. The engine never imports role code. Anything new a role needs becomes an explicit
   question: does this enter the protocol? Roles may stay in Python, or any language, on the other side.

   > "aceito a fronteira."

3. **Innovation is wanted.** Formal proofs and WebAssembly for roles were raised as elegant but expensive. The
   owner's answer:

   > "se for ajudar em algo nao tem problema, a intencao da jaynes e inovar mesmo."

   If it helps, cost is no objection: the intent of the project is to innovate.

4. **Research before the next decisions.** The owner asked for deep research on the theory, on the edge of AI
   agent systems and on today's problems, to ground the choices. The notes and the report are kept outside the
   repository's history (`docs/research/`, never committed). Done on 2026-11-12 in two rounds. The single complete report
   is `docs/research/reports/Redesenho do motor da Jaynes 2.md` (Portuguese; a one-page summary first, then a
   table of what the second round corrected, then the limits, then ten decisions, then twenty questions for
   the owner in dependency order). It replaces the first report beside it. The notes of both rounds are in
   `docs/research/research_notes/`, eight files per round. Its limits: the first round ran out of web searches
   part-way and the second did not; many numbers come from vendors; a few measurements were made locally on
   one machine and are only indicative; the writer checked the report against the notes, not the notes against
   the sources. Its findings are evidence, not decisions: nothing in it is decided until the owner says so
   below.

5. **The direction.** Put to the owner as four lines and approved ("sim"), after the research:

   - A small core in Rust. The core is a pure function: it takes the state and one event and returns the new
     state and what to do, and touches no disk, network or clock. A thin shell around it writes to SQLite,
     supervises the agents' processes, serves the page and sets what each step may reach (files, network,
     credentials; how far that is enforced was decided later, in item 10).
   - Roles outside the core, speaking the protocol. A role is a file of data (steps, permissions, what each stop
     asks) and a program that runs one step at a time. The engine owns the order of the steps. Roles may stay in
     Python.
   - One kind of question at a stop. The engine knows the items shown and the shape of the answer, with a
     verdict per item from a closed list, and the engine, not the role, writes what each answer will do.
   - A short or a full path according to the risk of the change (see the requirement above).

   Also proposed with it and not objected to, so treat as the working plan and not as a closed decision: an event
   log in which each event carries the fingerprint of the one before, so tampering shows; a build order of
   protocol, question and a one-page list of the engine's guarantees first, then the current engine speaking the
   protocol while the Rust core is written against the guarantees and against recorded real runs, then the switch
   when the Rust core reproduces those runs, with open runs finishing on the old engine; and, left out for now,
   WebAssembly (today's checks use a library with native code), a role's own screen, a public registry and
   Windows.

6. **The owner reads the guarantees.** Asked whether he accepts to read and approve one page of plain sentences
   saying what the engine guarantees (for example, "a decision you locked changes only if you change it"), he
   answered "sim". This is what makes proving the core worth it: the research found that agents now write most
   proofs and that the weak point became properties nobody read, or properties too weak to mean anything. So
   verification may go as far as a small model of the core with proofs of those sentences, with the real code
   compared against the model by tests. The sentences are written for him, kept few, and kept out of reach of
   the agent that writes the code and the proofs.

7. **A correction redoes only what depended on it.** Asked what Jaynes redoes when he corrects an answer after
   the run has gone on, everything from that point or only what depended on his answer, the owner chose the
   second ("b"). The price, accepted with it: from the first version the engine records what each step read, so
   it can tell which later steps depended on the answer. The research found this in no comparable engine; the
   rules exist in neighbouring fields (build systems that redo only what a change invalidates, and systems that
   hold outputs while an input is provisional). Two rules come with it from that research and are part of the
   working plan: nothing leaves the machine while it depends on an answer the person has not confirmed, and so
   the run waits just before the first step that cannot be undone and depends on such an answer.

8. **Risk is judged by rules in code.** Asked who says whether a change is of low or high risk (the person
   when asking, rules in code, or an agent at the start), the owner chose rules in code ("b"): the level comes
   from what the change touches, such as which files and whether anything cannot be undone. The person may raise
   the level. An agent may never lower it. The reason given to him: the research found that a control judged by
   the same model that must follow it ends up skipped.

9. **Rules in code may answer routine questions.** Asked whether a routine question (an agent asking leave to
   read a folder of the project) may be answered without him, the owner chose yes, by a rule in code, for
   routine permissions only ("b"). A decision of taste or of direction is always his. The record says who
   answered each question. A model never answers in his place.

10. **The guards stay the agent's.** Asked whether, inside a fence owned by Jaynes, the agent should run without
    its own guards, the owner chose to keep things as they are today ("b"): the guards are the agent program's
    own sandbox and permission settings, and Jaynes only configures them for each step. He chose this against
    the recommendation put to him, which was a fence owned by Jaynes, conditional on a prototype. What follows
    for the design:

    - Jaynes does not promise that a step is confined. It promises that it configured the agent program's guards
      for the step, and it records which settings it applied. The research found those guards partial (in one
      program the sandbox is off unless turned on and covers shell commands only), so the page and the docs must
      not claim more.
    - The research found that two operating-system sandboxes do not work one inside the other, so a fence owned
      by Jaynes would mean turning the agent's own off. That trade was not taken.
    - A role still declares, per step, what the step may reach (files, network, commands). The declaration is
      what Jaynes turns into the agent program's settings today, and it leaves a fence owned by Jaynes possible
      later without changing the protocol.
    - Whatever does not depend on a fence still holds: nothing leaves the machine while it depends on an answer
      the person has not confirmed (item 7), rules in code judge risk (item 8), and a fault of Jaynes or of the
      machine never spends an attempt of the agent.

## Left to the redesign session

The interview covered what only the owner could decide. The rest is design work, with these working defaults from
the research; the session may change one when it finds a reason, and says so.

- Core and packages. In the core: the event log, the machine that moves a run from step to step, the gate of the
  checks, the one question, the ceilings, and what each step may reach. In packages: the methods (roles) and the
  adapter for each agent program. The page is part of the shell around the core. The owner did not pick among
  the three options first put to him; this split follows from the direction he approved (item 5).
- The agent programs. Claude Code first. One adapter per program behind one narrow contract, so a second
  program is an adapter and not a change to the core.
- Trust across a change of model. The count of "something is wrong" never resets; the count of "it is going
  well" is discounted by what a replay of past steps on the new model measures.
- Existing roles keep working through a small Python library on the other side of the protocol. A run stays on
  the version of the role and of the engine it started with.
- Platforms: macOS first, then Linux. Windows is out for now.
- Registry: nothing runs at install, a version is fixed by the fingerprint of its content, and an index in a git
  repository is enough until authors from outside appear.
- WebAssembly for checks is prepared for in the protocol (a check is a pure function of declared inputs) and is
  not built now.

## To ask the owner when the design needs it

- The ceiling of time and tokens for each level of risk, and the ratio to the direct cost he accepts for a small
  change.
- The one page of guarantees, when it exists, for him to read and approve (item 6).
- What promise of stability to make to people who write roles, and when to open a public registry.
- Whether a role may bring its own screen, if the standard question turns out not to be enough.
