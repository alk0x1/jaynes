# Jaynes specification

Version 0.2, 2026-10-21. Supersedes 0.1, which described only the action gate.
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

Status: the Safety layer works today for Claude Code (section 9, appendix A).
Everything else in this document is the design to build next.

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
| Executor | Who produces the output: an agent (`claude` or `codex`) or Jaynes itself (`run_by="jaynes"`) for evidence an agent could fake |
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
| Role definitions | Python modules declaring steps, schemas, checks and stops | to build |
| Conductor | Runs a task through a role, one step at a time; owns workspaces, write boundaries, retries and resumption | to build |
| Runtimes | Run one bounded agent turn in a workspace: Claude Code (`claude -p`) and Codex (app server) | to build; prototype exists in the first user's factory |
| Trusted executors | Steps Jaynes runs itself, such as a read-only browser walkthrough or the test suite | to build |
| Event log | Append-only record of everything; all state is a projection | works (`jaynes.store`), to extend |
| Attention engine | Decides, per stop, mandatory, sampled, or refuted | works (`jaynes.bounds`, `jaynes.gate`) |
| Review page | Local web page where stops wait for answers | to build |
| Action gate | Claude Code hook that classifies and gates each tool call | works |

## 4. Role definition

Python is the definition language. Schemas are Pydantic models; checks are
plain functions. Sketch of the v0 API:

```python
from jaynes import role, stop, Checked
from pydantic import BaseModel

class Moments(BaseModel):
    moments: list[Moment]            # each cites snapshot evidence

design = role("screen-design", agent="codex", method="method.md")

snapshot = design.step("snapshot", output=Snapshot, run_by="jaynes")
moments  = design.step("moments", output=Moments, needs=[snapshot],
                       check=cites_snapshot, stop=stop(budget=0.20))
objects  = design.step("object-map", output=ObjectMap, needs=[moments, snapshot],
                       check=[cites_snapshot, counts_match_data], stop=stop(budget=0.20))
paths    = design.step("paths", output=Paths, needs=[objects])
walk     = design.step("walkthrough", output=Captures, needs=[paths], run_by="jaynes")
breaks   = design.step("breaks", output=Breaks, needs=[walk], check=breaks_cite_captures)
reqs     = design.step("requirements", output=Requirements, needs=[breaks],
                       check=one_requirement_per_break, stop=stop(budget=0.10))
build    = design.step("implement", output=Patch, needs=[reqs],
                       check=[tests_fail_then_pass, contrast_aa, tokens_only, no_regression])
accept   = design.step("accept", needs=[build], stop=stop(budget=0.05, sampled=False))
```

Rules:

- `needs` must form a directed acyclic graph; v0 runs it in topological order,
  one step at a time.
- A check returns `Pass` or `Fail(reasons)`. Reasons are shown to the agent on
  retry and to the person at a stop.
- A step with `run_by="jaynes"` names a trusted executor; the agent never
  writes its output.
- `stop(budget, sampled=True, audit_rate=0.1)`: `sampled=False` makes it a
  floor.
- The role's prompt for each agent step is built from the method text, the
  step's instructions, its schema, and the needed outputs. All of it is part
  of the step's fingerprint.

## 5. Step execution protocol

For each step, the conductor:

1. **Prepares a workspace**: a fresh directory containing read-only copies of
   the needed outputs and any snapshot data, plus the empty output path. It
   records a manifest (path and hash of every file).
2. **Runs one agent turn** through the runtime, with a deadline and a token
   meter read from the runtime, never from the agent's own report.
   - Claude Code: `claude -p` with settings that allow writes only inside the
     workspace, OS sandbox on, no network tools, no subagents, permission mode
     `dontAsk`.
   - Codex: app server thread with `workspaceWrite` sandbox rooted at the
     workspace and network off.
3. **Enforces the write boundary**: after the turn, any created, changed or
   deleted file other than the output path invalidates the turn. The runtime
   sandbox prevents most violations; the manifest catches the rest.
4. **Validates**: parse the output, validate the schema, run the check against
   the output and the needed witnesses.
5. **Retries** up to `max_turns` (default 3) with the failures appended to the
   prompt. After that the run is `BLOCKED` and appears in the review page.
6. **Records the witness**: `Checked[T]` with the output hash and the check
   code hash.
7. **Opens the stop**, if the step has one (section 6).

Trusted executors follow steps 1, 4, 6 and 7; they have no write boundary to
enforce because Jaynes wrote them.

Durability: every transition is an event. `jaynes run <id>` after a crash, a
quota pause or a reboot resumes from the last recorded witness. A turn that
was in flight is discarded and rerun; its partial workspace is kept for
inspection.

Quota: before each turn, the runtime's quota reading is checked. A turn that
would start inside an exhausted window pauses the run until the window resets.

## 6. Stops and attention

### 6.1 At a stop

The conductor computes the stop's status from its window:

| Status | Condition | Behaviour |
|---|---|---|
| floor | `sampled=False` | waits every run |
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

A later change by the person to an output that went through unreviewed, for
example editing a delivered screen, is recorded as an outcome event. It is
shown on the role page; it does not enter the window in v0.

### 6.3 Window

A stop's labels count only under its current fingerprint: hash of the role
name, the step's prompt inputs (method text, instructions, schema), its check
code, the stop's budget and δ, and the agent model. Changing any of them opens
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

A local web server started with `jaynes review`, bound to 127.0.0.1.

| View | Content |
|---|---|
| Queue | Stops waiting, blocked runs, oldest first |
| Stop | The question generated from the step output, the evidence it cites, check results, the before and after where relevant; answer buttons; a note field |
| Run | Step timeline with witnesses, refusals, turns, tokens and time |
| Role | Per stop: status, labels, evidence against the threshold, audit history; per week: steps fulfilled, rounds until acceptance, tokens per task, the person's minutes |
| Log | The raw event stream, filterable |

The CLI mirrors the essentials: `jaynes queue`, `jaynes answer <run> <step>
accept|change|return|context [--note]`.

The visual language follows `site/index.html`.

## 8. Events

One SQLite table, append-only, one row per event with sequence, time, type,
run or action id, and a JSON payload.

Conductor events:

| Event | Payload |
|---|---|
| `RunStarted` | role, task, role fingerprint |
| `StepStarted` | step, attempt, executor, workspace manifest hash |
| `TurnFinished` | step, attempt, files changed, tokens in and out from the runtime meter, seconds |
| `TurnRejected` | step, attempt, reason: `write_boundary`, `schema`, `check`, `deadline` |
| `Checked` | step, output hash, check hash |
| `StopOpened` | step, status, coin, n, k, evidence |
| `StopSkipped` | step, status `certified`, coin |
| `StopAnswered` | step, answer, label, note, seconds waited, seconds spent |
| `RunPaused` | reason `quota`, until |
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
| M2 | Conductor, role API, both runtimes, write boundary, durability, quota pause | A role with a fake agent passes property tests (no step without its witnesses, no turn accepted after an out-of-bounds write, resume after `kill -9` reproduces the same events); one real step each with Claude Code and Codex |
| M3 | Review page and CLI answers | A run waits at a stop, is answered from the page, and continues; labels land in the window |
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
