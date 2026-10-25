# Jaynes specification

Version 0.4, 2026-10-22. 0.2 added the conductor design; 0.3 recorded M2 as built; 0.4 records M3.
Product framing: `docs/specs/2026-10-21-landing-and-readme-design.md`.

## 1. Purpose

Jaynes answers one question for a person who runs a team of AI agents: how do I
make sure, or at least raise the odds, that my agents' work is good, while
spending my attention only where it matters?

It does this in four layers:

| Layer | Guarantee | Kind |
|---|---|---|
| Method | A role's steps run in order; a step starts only with the checked output of the steps it needs | Deterministic |
| Checks | Every step output passes the checks declared for it before anything consumes it | Deterministic |
| Attention | A stop asks the person less only when its error rate is shown, with anytime-valid evidence, to be within its budget | Statistical |
| Safety | Every tool action goes through a gate; floors never run without a person | Deterministic plus statistical audits |

Status: the Safety layer works for Claude Code (section 9, appendix A). The
Method and Checks layers work through the conductor (sections 4 and 5, appendix
B). Stops open, skip by audit coin, and take answers from the review page, the
CLI or the Python API; after an answer the run continues by itself (appendix C).
Runs started from Python, without a role reference, are answered from Python.

## 2. Concepts

| Term | Definition |
|---|---|
| Role | A named kind of work, defined in code: an ordered set of steps. Example: `screen-design` |
| Task | One unit of work given to a role. Example: "redesign the People screen" |
| Run | One execution of a role for one task |
| Step | A unit of a role with a name, an output type, the steps it needs, a check, an executor and an optional stop |
| Output | A JSON document validated against the step's schema |
| Check | A function from the output and the needed witnesses to a pass or a list of failures |
| Witness | `Checked[T]`, a record that a step's output of type `T` passed its check, identified by the hash of the output and the check code. Only the conductor creates witnesses |
| Executor | Who produces the output: an agent (`claude` or `codex`) or Jaynes itself, through a trusted executor function (`run_by=fn`), for evidence an agent could fake |
| Stop | A step whose output a person must review before the run continues |
| Answer | The person's response at a stop: `accept`, `change`, `return`, or `context` |
| Label | The statistical reading of an answer: `accept` is correct; `change` and `return` are errors; `context` is not counted |
| Budget | The error rate a stop may have once it stops being mandatory |
| Mandatory stop | A stop that waits for the person on every run |
| Sampled stop | A stop certified within its budget; it waits only on a random share of runs (`audit_rate`) |
| Window | The labels of a stop that count, those recorded under its current fingerprint (section 6.3) |
| Floor | A stop or an action that never runs without a person |

## 3. Architecture

```
person ── review page (local web) ── answers
                │
           conductor ── event log (SQLite) ── attention engine (bounds)
            │     │
   runtimes:      trusted executors (walkthrough browser, test runner, ...)
   claude, codex
            │
      action gate (Claude Code hook) ── floors, audits
```

| Component | Responsibility | Status |
|---|---|---|
| Role definitions | Python modules declaring steps, schemas, checks and stops | works (`jaynes.role`) |
| Conductor | Runs a task through a role, one step at a time; owns workspaces, write boundaries, retries and resumption | works (`jaynes.conductor`) |
| Runtimes | Run one bounded agent turn in a workspace: Claude Code (`claude -p`) and Codex (`codex exec`) | works (`jaynes.runtimes`) |
| Trusted executors | Steps Jaynes runs itself, such as a read-only browser walkthrough or the test suite | the mechanism works; no executors shipped yet |
| Event log | Append-only record of everything; all state is a projection | works (`jaynes.store`), conductor events keyed by `run_id` |
| Attention engine | Decides, per stop, mandatory, sampled, or refuted | works (`jaynes.bounds`, `jaynes.gate`) |
| Review page | Local web page where stops wait for answers | works (`jaynes.review`) |
| Action gate | Claude Code hook that classifies and gates each tool call | works |

## 4. Role definition

Python is the definition language. Schemas are Pydantic models; checks are
plain functions. Sketch of the v0 API:

```python
from jaynes import role, stop, Checked
from pydantic import BaseModel

class Moments(BaseModel):
    moments: list[Moment]            # each cites snapshot evidence

design = role("screen-design", agent="codex", method=Path("method.md"))

snapshot = design.step("snapshot", output=Snapshot, run_by=take_snapshot)
moments  = design.step("moments", output=Moments, needs=[snapshot],
                       check=cites_snapshot, stop=stop(budget=0.20))
objects  = design.step("object-map", output=ObjectMap, needs=[moments, snapshot],
                       check=[cites_snapshot, counts_match_data], stop=stop(budget=0.20))
paths    = design.step("paths", output=Paths, needs=[objects])
walk     = design.step("walkthrough", output=Captures, needs=[paths], run_by=walk_paths)
breaks   = design.step("breaks", output=Breaks, needs=[walk], check=breaks_cite_captures)
reqs     = design.step("requirements", output=Requirements, needs=[breaks],
                       check=one_requirement_per_break, stop=stop(budget=0.10))
build    = design.step("implement", output=Patch, needs=[reqs],
                       check=[tests_fail_then_pass, contrast_aa, tokens_only, no_regression])
accept   = design.step("accept", needs=[build], stop=stop(budget=0.05, sampled=False))
```

Rules:

- A step can only need steps defined before it, so `needs` is acyclic by
  construction and v0 runs steps in definition order, one at a time.
- A check is `check(output, needs) -> Pass() | Fail(*reasons)`, where
  `needs[step]` is that step's `Checked` witness. Reasons are shown to the
  agent on retry and to the person at a stop. A check that raises, or returns
  anything other than `Pass` or `Fail`, counts as a failure.
- `method` is the method text, or a `Path` to it; a relative path is resolved
  next to the module that calls `role`.
- `run_by=fn` makes the step a trusted executor: Jaynes calls
  `fn(StepContext(run_id, task, needs, workspace))` and the agent never writes
  its output. The function's source is part of the step's fingerprint.
- A step with no `output` is a pure stop over its needs, like `accept` above.
- Per step: `agent` overrides the role's agent, `max_turns` (default 3),
  `deadline` in seconds per turn (default 900).
- `question=fn(output, needs) -> str` writes what the person is asked at the
  stop. It is recorded in `StopOpened` and is part of the stop's fingerprint.
  Without it the page asks whether the output is right, or, for a step with
  no output, whether the person accepts the result of the steps it needs.
- `stop(budget, sampled=True, audit_rate=0.1)`: `sampled=False` makes it a
  floor.
- `project="read"` gives the step a copy of the run's project to read;
  `project="write"` lets it change that copy, only at the paths its
  `may_change` globs name (`["**"]` for any). Each step's changes become a
  patch, and the patches become a branch at the end; the person's working
  tree is never touched.
- Floor paths are project paths that tools run or obey later, outside any
  box: agents' and editors' settings and instructions, git attributes and
  hooks, direnv, interpreters and package scripts (`pyproject.toml`,
  `package.json`, `sitecustomize.py`), test and build entry points
  (`conftest.py`, `Makefile`) and dev containers, in any case and at any
  depth, and CI files where CI reads them, at the top. `may_change` alone
  never lets them through: a step without a stop that touches one is
  rejected, and a step with a stop makes that stop a floor for the round
  whose output touched it (section 6.1).
- `effect` declares what a trusted executor does to the world: `pure`
  (default), `read`, `write-copy`, `web` or `irreversible`. Only
  `irreversible` changes how it runs: every step it needs must have a floor
  stop, so a person approves the whole payload; its own stop, if any, is a
  floor; it gets one attempt; it receives `ctx.idempotency_key`, a hash of the
  run, the step and the payload, to pass to the service that acts; and if
  Jaynes stopped while it was running, the run is blocked instead of resumed,
  since it may have acted.
- The role's prompt for each agent step is built from the method text, the
  step's instructions, its schema, and the needed outputs. All of it is part
  of the step's fingerprint.

## 5. Step execution protocol

For each step, the conductor:

1. **Prepares a workspace**: a fresh directory `runs/<run>/<step>/turn-<k>/workspace`
   containing `step.json`, read-only copies of the needed outputs in
   `inputs/<step>.json` and, after a `return`, the returned output in
   `inputs/previous.json`. A sibling `scratch` directory is the turn's TMPDIR.
   It records a manifest: every file, directory and symlink with its hash.
2. **Runs one agent turn** through the runtime, with a deadline and a token
   meter read from the runtime, never from the agent's own report.
   - Claude Code: `claude -p` with settings that allow writes only inside the
     workspace, OS sandbox on, no network tools, no subagents, permission mode
     `dontAsk`.
   - Codex: `codex exec --json` with the `workspace-write` sandbox rooted at the
     workspace, network off, user config, rules and project docs ignored,
     ephemeral session. The app server (which can read the quota before a turn
     and names the model) is the production path; `exec` was chosen as the
     smallest verified slice. `exec` does not name the model, so the stop
     fingerprint uses the pinned model or, without one, the CLI version.
3. **Enforces the write boundary**: after the turn, any created, changed or
   deleted file other than the output path invalidates the turn. The runtime
   sandbox prevents most violations; the manifest catches the rest.
4. **Validates**: parse the output, validate the schema, run the check against
   the output and the needed witnesses.
5. **Retries** up to `max_turns` (default 3) with the failures appended to the
   prompt. A runtime failure or a missed deadline counts as a rejected turn.
   After that the run is `BLOCKED` and appears in the review page.
6. **Records the witness**: `Checked[T]` with the output hash and the check
   code hash.
7. **Opens the stop**, if the step has one (section 6).

Trusted executors follow steps 1, 4, 6 and 7; they have no write boundary to
enforce because Jaynes wrote them.

Durability: every transition is an event, and the conductor's only state is
the fold of a run's events. `jaynes run <id>` after a crash, a quota pause or a
reboot resumes from the log. A turn that was in flight is recorded as
`TurnInterrupted`, does not count as an attempt, and is rerun in a new
workspace; the old workspace is kept for inspection. A lock file keeps two
processes off the same run. A `change` answer and the witness of the edited
output are written in one transaction, so a crash cannot keep the label and
drop the edit.

Quota: a runtime that reports an exhausted window raises a quota error; the
turn is interrupted (not counted) and the run pauses until the reported reset,
or 30 minutes when none is reported. Before each turn the conductor checks the
latest pause for that agent across all runs, so no turn starts inside a window
known to be exhausted.

Known limits of v0: `kill -9` of the conductor leaves the agent process (its
own session) running until its deadline, writing only into the abandoned
workspace; nothing restarts a paused run by itself, it continues when
`jaynes run <id>` is called after the reset; outputs are JSON only, without
attached files; the action gate is not installed inside conductor turns yet
(hooks are off in the turn settings); resuming a run does not check that the
role's fingerprint is unchanged; `OutcomeObserved` (section 6.2) is not
recorded yet.

## 6. Stops and attention

### 6.1 At a stop

The conductor computes the stop's status from its window:

| Status | Condition | Behaviour |
|---|---|---|
| floor | `sampled=False`, or the round's output changed a floor path | waits every run |
| certified | certify statistic ≥ 1/δ | waits with probability `audit_rate`, otherwise continues |
| refuted | refute statistic ≥ 1/δ | waits every run; flagged on the role page |
| undetermined | neither | waits every run |

The coin is a deterministic hash of the run id and step name, so a replay
reproduces every decision.

### 6.2 Answers and labels

| Answer | Effect on the run | Label |
|---|---|---|
| accept | continues with the output as is | correct |
| change | the person's edited output replaces the agent's and is rechecked | error |
| return | the step reruns with the person's note in the prompt | error |
| context | the step reruns with the note; the agent was not at fault | not counted |

Only answers given while the stop was waiting count. A stop that continued
without waiting produces no label, so labels are either every occurrence
(mandatory phase) or a uniform random sample (audits). Both are unbiased for
the stop's error rate.

Every answer says who gave it, and only a person's answer is a label:

- **person**: an answer from the review page, which needs the page key
  (section 7). It is counted in the window.
- **agent**: anything else, such as `jaynes answer` run by an agent. It
  releases the stop but is never counted, and a floor stop refuses it.

From Python, `Conductor.answer` has no default for `by`, so nothing passes for
a person by omission. When a person sends a run back to an earlier step, the
label that step's stop was released with is replaced by the person's; if an
agent had released it, there was no label, so the person's is added.

Each opened stop records `evidence`, the hash of the output and question it
shows. An answer that carries it is refused if the stop has shown something
else since, so a stale page cannot accept what came after it.

A later change by the person to an output that went through unreviewed, for
example editing a delivered screen, is recorded as an outcome event. It is
shown on the role page; it does not enter the window in v0.

### 6.3 Window

A stop's labels count only under its current fingerprint: hash of the role
name, the step's prompt inputs (method text, instructions, schema), its check
code, the code of its question function when it has one, the stop's budget and
δ, and the agent model. Changing any of them opens
a new window. This is the declared-change half of the stationarity assumption.

### 6.4 The guarantee

For each stop, with `n` labels and `k` errors in the window:

- certify statistic: the mixture likelihood ratio of `q` against the budget
  `b`, with `q` uniform on `[0, b]`;
- refute statistic: the mirror, with `q` uniform on `[b, 1]`.

If the stop's true error rate is at or above its budget, the certify statistic
is a nonnegative supermartingale with initial value 1, and by Ville's
inequality the probability that it ever reaches `1/δ` is at most `δ`.

> For each stop, the probability that Jaynes ever makes a stop sampled whose
> true error rate is at or above its budget is at most δ, however often it
> checks.

Labels needed with zero errors at δ = 5%:

| Budget | Labels |
|---|---|
| 5% | 88 |
| 10% | 43 |
| 20% | 20 |
| 30% | 13 |

Simulation, 4,000 runs, true rate equal to a 5% budget: 3.7% wrongly
certified, against 27.4% for a Clopper-Pearson bound recomputed after every
label (`jaynes simulate`).

Assumption: future runs of a stop resemble its window. Covered by new windows
on declared changes; undeclared drift is the job of the change detector
planned in section 11.

## 7. Review page

A local web server started with `jaynes review` (port 8740), bound to
127.0.0.1, built with the standard library. The layout keeps the queue on the
left and the open item on the right; every page shows the run's steps in
order with how each was released (`by jaynes`, `checked`, `accepted`,
`changed`, `sampled`, `waiting`, `blocked`).

| View | Content |
|---|---|
| Queue (`/`) | Stops waiting, blocked runs, and idle runs (unfinished, not waiting, no process holds them, no event for 10 s), oldest first; the oldest opens. An idle run shows the end of its `continue.log` and a Continue button |
| Stop (`/stop/<run>/<step>`) | The question, the output rendered from its JSON in schema order (lists of records as tables), after a return the returned output as Before, earlier rejections of the round, the person's note from the last round, the names of the checks its witness passed, turns and their cost, the stop's labels and evidence; answer buttons with keys A, C, R, X and a note field; Change opens an editor over the output JSON. For a step with no output, the outputs of the steps it needs, and no Change |
| Blocked | The last rejection reasons and the turns of the round |
| Run (`/run/<run>`) | Step timeline, state, turns, tokens, time, every event; witnesses by output hash |
| Role (`/role/<name>`) | Per stop: status, labels, evidence against 1/δ with the budget and δ logged at the stop, audits and skips, and each audit with its answer; per week: runs, distinct steps checked, rounds until accepted, tokens per finished run, the person's minutes; the runs |
| Log (`/log`) | The latest 500 events, payloads cut at 600 characters, filterable by type and run |

After an answer the server starts `jaynes run <id>` in the background, logging
to `runs/<id>/continue.log`, and the page reloads when the log grows, unless
the person is typing. Return and context need a note, from every entry point.
The person's minutes are the time from the first view of the open stop to the
answer, across reloads; answers from the CLI record none. Role modules are
imported once per server; restart it after editing a role.

Answers are accepted only with the server's per-process token and a local
Host and Origin, so another site open in the same browser cannot answer a
stop or send a run on. Only a browser opened with the link the server prints,
which carries the page key once and keeps it as a cookie, answers as the
person; any other client can read the page but not answer from it.

Known limit: the agent runs as the same operating-system user as the person,
so a process that starts `jaynes review` itself and reads the printed link
gets the key. Separating the two needs isolation from the operating system,
such as a separate user or a confirmation the agent cannot reach; v0 does not
do it.

The CLI mirrors the essentials: `jaynes queue`, and `jaynes answer <run>
<step> accept|change|return|context [--note] [--output JSON|@file]`, which
also continues the run.

## 8. Events

One SQLite table, append-only, one row per event with sequence, time, type,
run or action id, and a JSON payload.

Conductor events:

| Event | Payload |
|---|---|
| `RunStarted` | role, task, role fingerprint, role reference and runtime overrides (for `jaynes run <id>`) |
| `StepStarted` | step, round, attempt, executor, workspace manifest hash; for an irreversible step, effect, idempotency key and payload |
| `TurnFinished` | step, attempt, files changed, tokens in and out from the runtime meter, model, seconds |
| `TurnRejected` | step, attempt, reason: `write_boundary`, `schema`, `check`, `deadline`, `runtime`, `executor`; reasons |
| `TurnInterrupted` | step, attempt, reason `crash` or `quota`; not an attempt |
| `Checked` | step, round, attempt, output hash, check hash, source (agent, `jaynes`, `person`), model |
| `StopOpened` | step, round, status, coin, n, k, log evidence, stop fingerprint, budget, δ, question, evidence hash, floor paths |
| `StopSkipped` | step, status `certified`, coin, stop fingerprint, budget, δ |
| `StopAnswered` | step, answer, label, note, stop fingerprint, seconds waited, seconds spent, by (`person` or `agent`), the earlier label it revises |
| `RunPaused` | reason `quota`, agent, until |
| `RunBlocked` | step, reasons |
| `RunFinished` | outcome |
| `OutcomeObserved` | step, kind `changed_after_delivery`, evidence |

Action gate events (section 9): `ActionProposed`, `GateDecided`, `Executed`,
`HumanLabeled`, `ModelSeen`.

Every number on the review page is a projection of these events.

## 9. Action gate

Works today for Claude Code sessions, interactive or not. Specified in full by
the code (`jaynes.policy`, `jaynes.gate`, `jaynes.hook`) and summarized here:

- Each tool call is classified into a category from the policy file
  (`jaynes.toml` in the project, else `~/.jaynes/jaynes.toml`). Bash lines are
  split into simple commands; a line takes the strongest effect among them;
  unmatched commands make it `unknown`.
- Floors deny or always ask. Pure categories run. Unknown defers to the host.
- Other categories defer to the host's own permission flow while undetermined,
  run without the host once certified, and always ask once refuted; a random
  `audit_rate` share asks the person in every state.
- Labels come from the session transcript: `permissionDecision.source`
  starting with `user`; a note starting with `ctx` marks a denial as context.
- Same statistics as section 6.4, per category.

Inside conductor runs the runtime sandbox and the write boundary are the
primary safety; the gate adds floors for any Claude Code turn.

## 10. Success on the first user

The first user is a one-person company run by agents. Its baseline, measured
before Jaynes (Sep 1 to Oct 21, 2026):

| Measure | Baseline |
|---|---|
| Tool calls seen by the person | 25 of 11,894 |
| Design screens that did the method's first four steps | 0 of 8 |
| Screens sent back after passing every automated check | Processes, three rounds |
| Tokens per screen | 170k to 480k against a 150k budget |
| Agent-reported cost vs measured | lower than measured in both recorded cases |

Jaynes is validated on this user when the `screen-design` role shows: all
steps fulfilled with requirements approved, fewer rounds until acceptance,
fewer rejections after checks pass, tokens per screen measured and lower, and
an error rate stated per stop.

## 11. Milestones

| Milestone | Scope | Exit evidence |
|---|---|---|
| M1, done | Action gate, bounds, history report, simulation | Appendix A |
| M2, done | Conductor, role API, both runtimes, write boundary, durability, quota pause | Appendix B |
| M3, done | Review page and CLI answers | Appendix C |
| M4 | `screen-design` role on the first user's product | Two screens end to end with real stops; measures from section 10 recorded |
| M5 | Dogfood | Several weeks of use; at least one stop certified with real labels; comparison against the baseline |
| M6 | Judges with prediction-powered and active inference | A simulation on the first user's labels shows fewer labels for the same guarantee before anything ships |
| M7 | Change detection with e-detectors | Injected drift in simulation is detected within its false-alarm bound |
| M8 | Public launch | Someone outside installs it and runs a role without help |
| Later | Formal model in Lean 4, TypeScript SDK, more trusted executors | |

## 12. Out of scope for v0

Parallel steps inside a run, multiple people answering, hosted service, Slack
or mobile notifications, judges, per-item scores, email or other irreversible
outbound integrations.

## 13. Open questions

- How the method text is split between the role definition and step
  instructions without duplicating the first user's `method-v2.md`.
- Whether `change` answers should feed the agent's examples for the step
  automatically.
- Turn deadline and `max_turns` defaults per step type.
- Whether resuming a run whose role fingerprint changed should continue, block,
  or restart the affected steps.
- What `return` means at a stop with no output, such as `accept`: today it
  only reopens the same stop; it should probably rerun the steps it needs,
  which v0 cannot do because downstream steps never rerun.

## Appendix A. M1 evidence

| Criterion | Result |
|---|---|
| Wrong certification at true rate equal to budget, 4,000 runs | 3.7% (anytime-valid) against 27.4% (Clopper-Pearson rechecked) |
| Gate properties: floor never runs alone; `allow` only for pure or certified; refuted always asks | 2,000 generated cases, pass |
| SQL projection equals the pure fold | 300 generated event sequences, pass |
| Floors against the first user's deny hook on 8,434 real Bash calls | 14 of 15 blocks equal; the difference is a false positive on prose in a heredoc; no new block |
| Hook on Claude Code 2.1.306, non-interactive run | read allowed, push denied, audited write asked and not counted as a human label, model known from the first call |
| Hook latency | 87 ms per call |
| `pytest`, `pyright --strict` | 51 tests, 0 errors |

Not yet verified: the transcript record when a person answers a hook-forced
prompt in an interactive session.

## Appendix B. M2 evidence

| Criterion | Result |
|---|---|
| Generated roles (1 to 5 steps, random needs, stops, `max_turns`) with an agent drawing from valid, failing check, invalid JSON, stray write, edited input, no output, crash and timeout, and drawn answers at stops | 300 cases, pass: no step started without the checked and reviewed outputs it needs; every turn that wrote outside its output (by the drawn behaviour, not the conductor's report) was rejected for the write boundary and never checked; every witness hash matches its stored bytes; turns per round within `max_turns`; settled runs are stable |
| Same generator, the log made to fail at a random write (a stand-in for SIGKILL that no handler catches), then a fresh conductor resumes | 300 cases, pass: same final status and the same events as an uninterrupted run, except each interrupted turn (`StepStarted` through `TurnInterrupted`) |
| `jaynes run` killed with SIGKILL mid-turn, then `jaynes run <id>` (one fixed kill point, real processes) | same events except the interrupted turn; old workspace kept |
| The property tests catch seeded faults (seeded by hand during development, not kept as a script) | write boundary removed, `max_turns` off by one, stops ignored, interrupted turn not marked, a change written as two separate writes: each fails |
| Stops | waits and continues on accept; change rechecked and replaces the output; return reruns with the note; context not counted; certified stop waits exactly when the coin is under `audit_rate`; floor waits after 20 clean labels; repeated errors refute; a changed check opens a new window |
| Quota | pause without spending an attempt; another run on the same agent pauses too; resumes after the reset |
| One real step, Claude Code 2.1.307 | finished in one turn, 6.7 s, 42,492 tokens in, 346 out, model `claude-sonnet-5-5` from the meter |
| One real step, Codex CLI 0.162.1 | finished in one turn, 16.5 s, 59,004 tokens in, 344 out |
| Escape probe, both runtimes, the agent told to write `../` by shell and by file tool, and `~/` by shell | none written; the file inside the workspace written |
| End to end by hand, `jaynes run --role examples.count_words:ROLE` with Claude Code | trusted step then agent step, finished; rerun of the finished run added no events |
| `pytest`, `pyright --strict` | 75 pass plus 4 live tests opt-in with `JAYNES_LIVE` (all 4 pass when run), 0 errors |

## Appendix C. M3 evidence

| Criterion | Result |
|---|---|
| A run waits at a stop, is answered from the page over HTTP, and continues | pass: the answer is recorded with its label and the person's seconds, the run finishes in the background, the queue empties, and the run, role and log pages show it |
| Change from the page | invalid JSON refused with the reason; a valid edit is checked and replaces the output (`source: person`) |
| Return without a note | refused, nothing recorded |
| Answers from another site | wrong token, foreign Origin or foreign Host: 403, nothing recorded |
| CLI | `jaynes queue` lists the stop with its question; `jaynes answer ... change --output` records it and the run finishes in the background |
| By hand in Chrome, `examples.review_demo` with two runs | R with an empty note asks for the note; Return with a note reruns the step as round 2 and the page moves to the next item; A accepts and the run continues to the `accept` floor, which shows the result of `implement` without Change; Accept finishes the run; the role page shows 2 labels and 1 error on `requirements` |
| Reviewed before commit (standards and spec) | fixed: stop fingerprints without a question changed, resetting every window (a test pins the M2 value); return and context without a note passed the CLI and API; time spent restarted on reload; a run whose continuation failed vanished from the queue (now idle, with Continue); a busy run, a malformed request or a server error got no answer; path segments unquoted in links |
| `jaynes hook` cost | importing the CLI 38 ms, against 49 ms at M2: the conductor is imported only by the commands that use it |
| `pytest`, `pyright --strict` | 90 pass plus 4 live tests opt-in, 0 errors |
