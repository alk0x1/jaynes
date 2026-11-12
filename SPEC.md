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
| Mandatory stop | A stop shown to the person on every run |
| Sampled stop | A stop certified within its budget; it is shown only on a random share of runs (`audit_rate`) |
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
| Trusted executors | Steps Jaynes runs itself, such as capturing references from the web or running the test suite | works; one shipped, `jaynes.roles.references.gather` (`effect="web"`), see section 4 |
| Event log | Append-only record of everything; all state is a projection | works (`jaynes.store`), conductor events keyed by `run_id` |
| Attention engine | Decides, per stop, mandatory, sampled, or refuted | works (`jaynes.bounds`, `jaynes.gate`) |
| Review page | Local web page where stops wait for answers | works (`jaynes.review`) |
| Action gate | Claude Code hook that classifies and gates each tool call | works |

## 4. Role definition

Python is the definition language, or a TOML file that lists the steps and
names its models and functions in Python (below). Schemas are Pydantic models;
checks are plain functions. Sketch of the v0 API:

```python
from jaynes import role, stop, Checked
from pydantic import BaseModel

class Moments(BaseModel):
    moments: list[Moment]            # each cites snapshot evidence

design = role("screen-design", method=Path("method.md"))

snapshot = design.step("snapshot", output=Snapshot, run_by=take_snapshot)
moments  = design.step("moments", output=Moments, needs=[snapshot],
                       check=cites_snapshot, stop=stop(budget=0.20))
objects  = design.step("object-map", output=ObjectMap, needs=[moments, snapshot],
                       check=[cites_snapshot, counts_match_data])
paths    = design.step("paths", output=Paths, needs=[objects])
walk     = design.step("walkthrough", output=Captures, needs=[paths], run_by=walk_paths)
breaks   = design.step("breaks", output=Breaks, needs=[walk], check=breaks_cite_captures)
reqs     = design.step("requirements", output=Requirements, needs=[breaks],
                       check=one_requirement_per_break, stop=stop(budget=0.10))
build    = design.step("implement", output=Patch, needs=[reqs],
                       check=[tests_fail_then_pass, contrast_aa, tokens_only, no_regression])
accept   = design.step("accept", needs=[build], stop=stop(budget=0.05, sampled=False))
```

The built-in `screen-design` role (`src/jaynes/roles/screen_design.py`) is the
full version of this sketch, with the person's method and heuristics as
inputs.

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
  `fn(StepContext(run_id, task, needs, workspace, idempotency_key, started))`
  and the agent never writes its output; `started` is what the run started
  with, such as the `suite` given to `jaynes run --suite`. The function's
  source is part of the step's fingerprint.
- A step with no `output` is a pure stop over its needs, like `accept` above.
- Per step: `agent` overrides the role's agent, `max_turns` (default 3),
  `deadline` in seconds per turn (default 900), `max_messages` per Claude
  turn (default 200; Codex counts none).
- `question=fn(output, needs) -> str` writes what the person is asked at the
  stop. It is recorded in `StopOpened` and is part of the stop's fingerprint.
  Without it the page asks whether the output is right, or, for a step with
  no output, whether the person accepts the result of the steps it needs.
- `stop(budget, sampled=True, audit_rate=0.1, waits=False)`: `sampled=False`
  makes it a floor. `waits=True` makes it a floor that holds the run until the
  person answers (`holds` in `StopOpened`), for a choice that is theirs, such
  as a look among three; it is left out of the stop's repr, so fingerprints do
  not change.
- `ends=fn(output) -> bool` lets an output end the run early, such as
  fix-bug's "no test shows the bug". It needs a stop, and when it holds
  that stop is a floor: only a person ends a run. Accepting (or changing
  the output to one that still ends it) finishes the run with outcome
  `ended` and delivers nothing; returning has the agent try again.
- `project="read"` gives the step a copy of the run's project to read;
  `project="write"` lets it change that copy, only at the paths its
  `may_change` globs name (`["**"]` for any; a glob that starts with `!`
  keeps its paths out, as fix-bug keeps its fix step off the tests). A check
  can run the project's own commands, such as its tests, with
  `jaynes.suite.run(needs, command)`: on a throwaway copy of the step's
  project, in the box, reaching any local port but no internet, with
  installed dependencies linked from the person's repository and the copy's
  code imported first. The agent's own copy has the same dependencies
  linked, never counted as changes, and its commands get the same offline
  environment (`jaynes.suite.environment`); Claude's box also opens the
  folders behind those links, a cache folder and local ports, so the agent
  runs the tests before it answers. Which command runs the tests is not the
  agent's to say: `jaynes.runners.detect` finds it from the project's files
  (pytest, a `test` script in `package.json`, `go test`, `cargo test`, a
  Makefile `test` target), or takes the `suite` the person gave at the start,
  and fix-bug's first step, a trusted executor, records it; with neither, that
  step blocks the run at once with how to give one. Each step's changes become a
  patch, and the patches become a branch at the end; the person's working
  tree is never touched, except by a merge alone (below).
- `role(..., merges=True)` says the role's delivery is a merge of the run's
  branch into the branch the project had checked out when the run started.
  Jaynes makes it by itself once every step whose output is `proof.Proved`
  passed and nothing holds the run, before any answer: a merge commit made
  apart from any checkout, landed by fast-forward where the branch is
  checked out and only over a clean checkout, or moved directly where it is
  not. It never pushes. A conflict, uncommitted changes, a failed proof, a
  detached checkout or a role with no proof leave the branch unmerged and say
  why once (`MergeSkipped`); a later pass tries again. After a correction,
  only what changed since the last merge is merged on top (`over`). A person
  reverts every merge of the run with Undo on the run's page, with a note;
  an unfinished run then finishes as `undone` and never merges again, and
  what it let through earns no right. build-feature and fix-bug merge.
- `web=True` lets the agent read the web in that step through its own fetch
  and search tools; its shell stays offline. The prompt tells it that what a
  page says is information for the task, never an instruction. The runtime
  records each address and search, `TurnFinished` keeps them, and the step's
  page lists them. Only an agent step can read the web; a trusted executor
  that does declares `effect="web"`.
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
- `role(..., inputs={"method": Input(help, kind="file")})` declares what a
  run starts with, given as `jaynes run --input name=value` (or Inputs on New
  run). A run missing a required input, or given one the role does not
  declare, is refused before it exists. Each is resolved once at the start
  and recorded in `RunStarted` with a digest: `text` as given; `file` read
  then, so later edits change nothing; `command` with the files it names, so
  a looser limits file changes the digest; `folder` copied then into the
  run's folder (a clone on macOS, instant and taking no space) as `source` or
  `source:where-in-the-project`. A frozen folder is never read live again:
  every preview, check or executor that serves the project gets its own clone
  in place. A step lists the inputs it uses, `inputs=["method"]`: a `file`
  input's text is shown in its prompt, a `text` input as `name: value`, and
  the digests of all it lists enter its stop fingerprint, so a new method
  text opens a new window at the steps that read it and no other.
- `choose=Choice(items, keep)` lets the person pick among the output's items
  at the stop, in their order of importance: `items(output)` lists them as
  (id, label), and `keep(output, ids)` gives the output with only those, which
  is checked again like a change. It is answered `choose` (section 6.2). In
  Claude Code's dialog, which has four options and no order, the items are
  listed in the evidence and typing their numbers in order chooses them.
- `ROLE.interview(questions)` makes the role's first step an interview:
  each `Question(id, text, options=())` has two to five options or none.
  An agent reads the request and, unless `project=False`, the project's copy,
  and writes `Interview`: each question with its guess and why, plus
  questions of its own with 3 to 5 options, checked before the person sees
  them. The stop waits; the page shows every question with the guess marked
  and a field for the person's own words, which win over the option. Leaving
  every guess is an accept; any other answer is a change to what they
  answered. Every later step, hidden ones aside, is told the questions, in the
  role's words, and the answers.
- `decides=fn` shows the output at the stop as decisions: `fn(output)` lists
  `Decision(id, label, value, after=())`. The person rejects a decision with a
  line, and the rest are kept: the answer becomes a return of the rejected ones
  alone, whose note names each, and the others are locked, by id, with their
  value. An accept with none rejected locks them all. Jaynes rejects a later
  output of the step that changes or drops a locked decision, as a failed
  check, so it never reaches the person; the prompt lists what is locked and
  asks the agent to keep each decision's id across rounds. The person unlocks a
  decision by rejecting it (Change on the page); `after` names the decisions
  one follows from, and rejecting one of those unlocks it too. Jaynes sending
  the step back keeps the locks; a person sending it back from another stop,
  or a blocked run, clears them, since they could not reject a decision there.
  Besides rejecting, the person may drop a decision for good or start from it,
  each with an optional line: either makes the answer a return, whose note
  lists what was dropped, what to start from and change a lot, and what to
  keep. What is dropped is recorded for the whole run: every later prompt,
  hidden steps aside, lists it, checks read it as `needs.dropped`, and Jaynes
  rejects a later output of the step that brings back a dropped value. At a
  step that also lets the person choose, dropping alone chooses what is left.
  A return of decisions at a step Jaynes runs goes to the agent before it, as
  any return there does, and such a step is held to no lock: the agent was
  told what to keep. `Decision(..., pictures=(Picture(...),))` shows pictures
  in its line.
- `back=Back(to, when, rounds=2)` lets Jaynes send an earlier step back:
  when the checked output makes `when(output, needs)` return reasons, `to`
  and every step after it run again with the reasons in `to`'s prompt, as a
  person's return would, but no label is recorded, since nobody answered.
  After `rounds` returns in a run, the output goes on as it is and later
  steps, and the person, see what remains. A broken `when` sends nothing
  back.
- A check raises `Disputes(to, said, about)` when the agent says an earlier
  step's output stands in its way, such as a test it may not change
  (build-feature's and fix-bug's `wrong_test`). The agents settle it first:
  `to` and every step after it run again with what the agent said in `to`'s
  prompt, up to `DISPUTES` (2) times in a run. When they still disagree, the
  run blocks and the page says in plain words that two agents disagree, with
  two buttons and nothing to write: Fix the test sends `to` back with what
  the agent said, and Keep the test sends the step back to work with it.
- `hidden=True` marks proof the maker cannot see: the step reads only the
  task, so it needs no earlier step and changes no project, and it comes
  before every step that changes the project. No agent step may need it;
  Jaynes's own steps and the person's stops may. `jaynes.proof` gives the
  pair a role adds: a hidden agent step that writes `Cases` (`proof.WRITE`,
  `proof.written`), each a sentence and a shell command that exits 0 when
  the sentence holds, and a trusted step after the work (`proof.prove`) that
  serves the changed project with the run's preview in a box, runs each
  command in a box over a copy of the project with the app at
  `$JAYNES_URL`, and outputs `Proved`. With `back=Back(maker, proof.failed)`
  a failing case sends the maker back with its sentence alone, never the
  command. A case that needs the app in a run without a preview is not run
  and fails the proof without sending anyone back. `Proved.passed` holds
  when at least one case ran and every case passed; nothing testable from
  outside is no proof. build-feature and fix-bug have it; their merge stop
  shows each case's sentence and whether it passed.
- A check or trusted executor that cannot judge at all, such as a measuring
  tool that failed, raises `Blocked(reason)`: the run blocks with the reason
  at once and no attempt is spent, since the agent is not at fault.
- A trusted executor leaves files for later steps in `files/`, at most
  `max_files` (default 20) of 10 MB each. Files under `files/private/` reach
  checks only, as `needs.files[step][path]`, and are never copied to an
  agent: raw data a check recomputes from, which would cost an agent tokens.
  `needs.started` is what the run started with, inputs included.
  `ctx.history` gives an executor what people said in the run (their notes
  and changes), what Jaynes sent back and why, and each step's turns, new
  tokens, cached tokens and seconds.
- Before a new run, from the CLI or the page, the verifier reads the role's
  checks, questions and trusted executors from their bytecode, without
  running them, and refuses the role if one names a step its step does not
  need, or a step of another role, for more than the step's name: directly or
  through a closure, a default value, a module or class attribute, a dict or
  list, a partial or callable object, or a plain function it names in turn;
  if a check or question is annotated for an output type its step does not
  produce; or if a step's output, other than the last step's, reaches no
  later step and no stop and the step neither changes the project nor acts.
  Steps reached through a method of another object, or a name computed at run
  time, are not seen. Resuming a run warns instead of refusing. `jaynes check
  <role>` runs it alone.
- A role file (`.toml`) holds the same role as data: top-level `name`,
  `agent` and `method` (or `method_file`, next to the file), then one
  `[[step]]` table per step with the keys of `step`. `output`, `check` and
  `run_by` are `module:attribute` references, imported from the file's folder
  and from the current folder; `needs` lists earlier step names; `stop` is a
  table, `{ budget = 0.2, sampled = false }`; `question` is a template over the
  step's output, `"Apply the {out.name} direction?"`, whose fields must exist
  in the output model and whose text is part of the stop's fingerprint. Checks
  in a role file read their needs by name, `needs["draft"]`, and the verifier
  treats a constant name looked up in `needs` as a read of that step, refusing
  names that are not steps of the role. An unknown key, a missing reference or
  a step needed before it is defined is refused with the file and the step.
  A run stores the file's absolute path, so it continues from any folder.
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
     `dontAsk`, and at most `max_messages` messages (`--max-turns`, default
     200, so the deadline is what usually ends a long turn). A turn that runs
     out is rejected with that reason in plain words.
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
known to be exhausted. While the review page runs, it continues each paused run
once its reset has passed (it checks every 15 s, and once per reset, so a run
that pauses again waits for its new one); without the page, `jaynes run <id>`
continues it.

Known limits of v0: `kill -9` of the conductor leaves the agent process (its
own session) running until its deadline, writing only into the abandoned
workspace; a paused run continues by itself only while the review page runs;
outputs are JSON only, without
attached files; the action gate is not installed inside conductor turns yet
(hooks are off in the turn settings); resuming a run does not check that the
role's fingerprint is unchanged; `OutcomeObserved` (section 6.2) is not
recorded yet.

### 5.1 Plans of runs

A request that needs several roles is a plan: `jaynes plan FILE --project DIR`
with `{"request": ..., "runs": [{"name", "role", "task", "after", ...}]}`. A
run's `after` lists runs before it in the plan, so a plan never loops; a run
with a project may follow at most one run whose delivery is a branch. Each run
gets its id when the plan is recorded, and starts, as `jaynes run` with the
request and its name, once every run it follows delivered: merged; on its
branch when its role does not merge or a merge was skipped; or every step
checked when it has no project. Open samples do not count. It works from the
person's branch when the runs it follows merged, or from the one branch it
follows otherwise (`base` in `RunStarted`), and its task adds the request and
what each earlier run delivered. A run dismissed, ended or undone stops the runs
after it. The run that delivers starts the next ones (`jaynes run` does it as it
returns), under a lock per request; the review page does it every 15 s for
every request still working, and starts again a run marked started whose
`RunStarted` has not arrived after 60 s. `jaynes wait <request>` returns when
the request delivered or stopped, or when something new holds one of its runs.
The page shows `/request/<id>` and, under Running now, the requests whose next
runs wait. On the request page the person drops a run that has not started and
that no other run builds on (`RequestRunDropped`, the API's `drop-run`, the
person's alone): it never starts, and the request delivers without it.

A role plans when a step's output is `jaynes.plan.Plan` (`runs`, each with
`name`, `role` by name, `task`, `after`, `preview`, `preview_path`, `suite`,
`inputs`, and `specialist`, who does it when a team planned it, which the
request page shows; and `why`). The step's prompt lists the roles from `jaynes roles`
that do not plan, each with its `about`, inputs and whether it changes or
merges the project; the conductor checks the plan as `jaynes plan` would, and
its reasons send the step back. The run's `RunStarted` carries `plans: true`.
Once the run delivered (a role that only reads the project delivers when every
step is checked), `jaynes run` records `RequestPlanned` with `by` the run's id,
the run's task as the request and its project and runtimes, and advances it;
the page's sweep does the same for a planning run whose process died. The
request page links the run, and the planning step's sample says that a return
does not replace the plan already started.

### 5.2 Packages of roles

The engine has no role of its own. A package is a folder with
`jaynes-package.toml` (`name`, `version`, `about`, `api`); every role file and
every Python file with a `Role` at its top level there is one of its roles,
except files whose name starts with `_`, and a file that does not import is
left out. `jaynes add SOURCE [--project DIR] [--ref REF] [--folder SUB]`
installs one: a name of a package that comes with Jaynes (`software`,
`design`, in `packages/` and shipped as `jaynes/_packages`), a folder, linked,
or a git repository, cloned at its newest commit or `REF`. An installed version
is copied to `packages/<name>/<version>-<commit or content hash>/` in the home
and kept; one that comes with Jaynes is installed again when Jaynes's copy
changed, and `jaynes update` installs a git package's newest commit unless
`REF` is a commit. The home's `packages.json` holds what is installed, the
default list and each project's list, by the project's git root. A project
without its own list uses the default one; `jaynes add --project` starts its
list from the default one and puts the package first. The page's Roles shows
the same: each installed package with its version, where it came from and its
roles, each list in order with Remove, and a form to add a folder or a git
repository for the project the person names, or, ticked, for every project;
the project is never filled in for them. The API's `add-package` and `remove-package`, and the MCP
tools `packages`, `add_package` and `remove_package`, do it for an agent the
person asked.

A role is started by its name, a role file's path, a Python file's
`path.py:attribute`, or `module:attribute`. A process that imported a package's
file reads it again, with the package's helpers, once it changed on disk, so the
page shows a linked folder's edits without a restart; steps are looked up by name,
so a run that goes on after an edit finds its own. A name is looked up in the
project's packages in order, the first package with it winning. A package's
Python files are imported as one Python package per installed folder, so they
import each other relatively (`from . import fix_bug`). `RunStarted` records
the reference, a file by its absolute path, which names the version. A run
recorded as `jaynes.roles.<module>:ROLE`, from before packages, loads that
module from the package that comes with Jaynes now.

The role API, version 1, is what a role may import from Jaynes:
`jaynes.role`, `jaynes.plan` (`Plan`), `jaynes.proof`, `jaynes.project`,
`jaynes.runners`, `jaynes.suite`, `jaynes.sandbox`, `jaynes.screens`,
`jaynes.browser` and `jaynes.inputs`. Jaynes refuses a package whose `api` is
not its own.

## 6. Stops and attention

### 6.1 At a stop

The conductor computes the stop's status from its window:

| Status | Condition | Behaviour |
|---|---|---|
| floor | `sampled=False`, or the round's output changed a floor path | shown every run |
| certified | certify statistic ≥ 1/δ | shown with probability `audit_rate`, otherwise skipped |
| refuted | refute statistic ≥ 1/δ | shown every run; flagged on the role page |
| undetermined | neither | shown every run |

The coin is a deterministic hash of the run id and step name, so a replay
reproduces every decision.

A stop that is shown does not hold the run (decided 2026-11-07, the person on
the loop). The run goes on with the agent's output, makes its branch, and waits
only for the answers: it finishes once every stop is answered. Three things do
wait for the person:

- a stop over a change to a floor path, or over an output that would end the
  run (`holds` in `StopOpened`);
- an irreversible step, which runs only once every stop before it is answered;
- a step that needs a stop with no output, since all that stop passes on is the
  person's answer, and every step that needs that step, or works on the project
  after it.

Each `Checked` records what its step received: each need's output and files,
and for a step on the project, every earlier step's change. A step whose
received no longer matches runs again (`InputsChanged`), correcting on top: it
gets its previous output, and its previous change to the project already
applied in its copy when it still applies. So after a return, only the steps
that used what changed run again, and their checks with them; a rerun that
gives the same output changes nothing after it. When the changes differ from
those on the branch, the branch is made again. A run can now block while an
earlier stop waits; `RunBlocked` records what the blocked step received, and
when an answer changes it, the step tries again (`InputsChanged` with
`unblocks`). A block no answer changes, such as an irreversible step that was
running when Jaynes stopped, stays until the person sends the run back. Answers take a lock of their
own, so a person answers while a process runs the run; an answer recorded after
its stop closed changes nothing.

### 6.2 Answers and labels

| Answer | Effect on the run | Label |
|---|---|---|
| accept | continues with the output as is | correct |
| change | the person's edited output replaces the agent's and is rechecked | error |
| return | the step reruns with the person's note in the prompt, on top of its previous output; or, with an earlier step named, that step does, and the error counts on its stop; at a step Jaynes runs, with no step named, the agent's step before it does, since Jaynes reads no note, or the step that one reviews when it sends a step back | error |
| context | the step reruns with the note; the agent was not at fault | not counted |
| choose | the chosen items of the output, in the person's order, replace it and are rechecked | correct |
| accept with decisions rejected | a return of the rejected decisions alone; the rest are locked | error |

The label is not yet the count. An error, a change or a return, counts at
once. What a stop lets through, accepted, chosen or skipped as certified,
counts as right only once its run delivered and nothing corrected it for 7
days (`SETTLE`); until then it is pending. Only the last release of a stop in
a run counts, since only it was delivered: a release that ran again because
what its step received changed counts nothing. A dismissed run's releases
count nothing; the errors found in it still count. The count is so made of
every delivery, asked or not, and its outcome.

Every answer says who gave it, and only a person's answer is a label:

- **person**: an answer from the review page, which needs the page key
  (section 7). It is counted in the window.
- **agent**: anything else, such as `jaynes answer` run by an agent. It
  releases the stop, but what it let through never counts as right; a
  correction afterwards still counts against it. A floor stop and a braked
  one refuse it. A small change the role let through (`asks`) is the same:
  nobody looked.

From Python, `Conductor.answer` has no default for `by`, so nothing passes for
a person by omission. When a person sends a run back to an earlier step, the
label that step's stop was released with is replaced by the person's; if an
agent had released it, there was no label, so the person's is added.

A stop with no output, such as `accept`, has nothing of its own to rerun, so
`return` and `context` there must name the earlier step that runs again
(`to`); without it the answer is refused with the step to name. The page
selects that step by default: the stop's last need with an output, else the
nearest earlier step with one.

Each opened stop records `evidence`, the hash of the output and question it
shows. An answer that carries it is refused if the stop has shown something
else since, so a stale page cannot accept what came after it.

A correction after delivery is recorded on the run that delivered
(`Conductor.correct`, `jaynes correct <run> --step <step> --kind <kind>
--note <what was wrong>`), with the step whose output was wrong and one of
three kinds:

- `error`: wrong by what was already known, including a preference already
  written where the agent could read it. It counts as an error on the first
  stop at or after that step that let the delivery through, even after the 7
  days.
- `preference`: a new preference nobody knew. It does not count.
- `info`: the world changed or a new fact appeared. It does not count.

The agent making the correction proposes the kind; the person sees it on the
run's page and can file it as another kind there (`reclassify`), with no
stop. Only a delivered run takes a correction; while a run is open, a return
from a stop corrects it. `jaynes delivered [--project dir]` lists the latest
deliveries with their steps, the files their branch changed and their
corrections, so the agent finds which one a fix corrects.

### 6.3 Window

A stop's outcomes count only under its current fingerprint: hash of the role
name, the step's prompt inputs (method text, instructions, schema), its check
code, the code of its question function when it has one, the stop's budget and
δ, and the agent model. Changing any of them opens
a new window. This is the declared-change half of the stationarity assumption.

### 6.4 The guarantee

For each stop, with `n` outcomes known and `k` errors in the window:

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

Why this statistic and not predictable betting (simulated 2026-10-27, budget 5%,
δ 5%, 300 streams per rate, median labels to certify): no test can certify faster
on average than `log(1/δ)` divided by the information per label, so the floor is
58 at a true 0%, 121 at 1%, 247 at 2% and 613 at 3%. The mixture needs 88, 134,
244 and 573: at the floor from 1% up. Plug-in betting strategies did 76 to 129 on
clean streams and no better from 1% up, with worse 90th percentiles (281 against
244 at 1%). The gain is at most a dozen labels on clean streams, so the mixture
stays.

Assumption: future runs of a stop resemble its window. Covered by new windows
on declared changes; undeclared drift is the job of the change detector
planned in section 11.

Errors are known at once and rights only after 7 days, so at any moment the
known outcomes hold proportionally more errors than the deliveries do; the
statistic only waits longer for that. A correction found after the 7 days
turns a right into an error, which can take a stop's certification back.

### 6.5 The brake

Two errors (`BRAKE_ERRORS`) among a stop's last ten outcomes (`BRAKE_LAST`),
in the order the stop let them through, brake it. A braked stop asks on every
run, even for a small change and when it is certified, and holds the run:
nothing after it runs until the person answers, and only a person may. It is
released when the outcomes since the brake certify on their own, by the test
of section 6.4; the errors before the release no longer count towards the
next brake. Braking needs little proof because braking without need only
costs waiting. While any stop holds the run, a later step whose inputs changed
since it ran, such as one built on an output just returned, has its stop
closed and runs again once the person answered, so no stop offers work on top
of something already sent back. The stop's `StopOpened` records `braked` with the outcomes and
errors since the brake.

## 7. Review page

A local web server started with `jaynes review` (port 8740), bound to
127.0.0.1, built with the standard library. The layout keeps the queue on the
left and the open item on the right; every page shows the run's steps in
order with how each was released (`by jaynes`, `checked`, `accepted`,
`changed`, `sampled`, `waiting`, `blocked`).

| View | Content |
|---|---|
| Queue (`/`) | Waiting for you: the open stops, oldest first; the first opens. Below them, folded under Other runs: blocked runs, idle runs (unfinished, not waiting, no process holds them, no event for 10 s), runs a process holds (`running`) and runs paused for quota (`paused`, with the reset time). Two runs of the same task show when each started. The open item says where its run is in a line ("Step 15 of 17, waiting for you"); its steps, turns and tokens are a click away. An idle run shows the end of its `continue.log` and a Continue button. Every unfinished run's page has Dismiss, as idle and blocked runs' pages do: it finishes the run with nothing delivered, at once, or, while a process holds it, when the step under way ends (`DismissAsked`; the run's loop finishes it before its next step, and `jaynes run` prints `dismissed`). What it merged stays; Undo reverts that. A run's page says at the top what it waits for, one click from it, with its steps and events folded below |
| Stop (`/stop/<run>/<step>`) | The question; when the step says how its stop shows the output (`shows`), its items in front: a title, a sentence and pictures cut to the part that matters (what the last action changed, or where the screen has content, never through a line) at no more than their own pixels, a computer's across the column and a phone's two to a row, each enlarged whole in the page with a click and closed with Escape, the items the person may choose picked by clicking them in order, and costs and measures folded under More. Everything below sits under a folded Details then: the output rendered from its JSON in schema order (lists of records as tables), after a return the returned output as Before, earlier rejections of the round, the person's note from the last round, the names of the checks its witness passed, turns and their cost with each shell command the agent ran, green or red by its exit and its output a click away, the stop's outcomes, errors, deliveries not yet held and evidence, and when braked how many clean deliveries release it. Code that runs later, outside any box, stays in front. Accept and Return, with the note field, are in sight; Change, Context and the step a return goes to are under More; keys A, C, R, X (at a floor, A accepts only when pressed again within two seconds; a click accepts at once); Change opens an editor over the output JSON. For a step with no output, the outputs of the steps it needs, no Change, and Return and Context going to the earlier step selected by default (section 6.2). At a step that shows decisions, they are in front as lines, each kept unless rejected with a line of its own, locked ones with Change; Accept reads Send once one is rejected. At an interview, every question with the guess marked, why, and a field for the person's words; Accept reads Start. Under any stop with an output, Why?: the person's question goes to a Claude Jaynes runs in a folder of copies of the request, the step's instructions, its output and previous output, the run's notes about it, its decisions and its files, allowed only to read them (no edit, command, web or agent), at most 12 questions per step; the answer appears in place when it comes and changes nothing in the run |
| Blocked | The last rejection reasons and the turns of the round. A run blocked after its last step, because its branch was not created, says so with the reason |
| Run (`/run/<run>`) | Step timeline, state, turns, tokens, time, every event; witnesses by output hash; its branch, each merge Jaynes made or why it did not, and Undo with a note while it is merged; the corrections of its delivery, each with its kind, where it counts and a select to file it as another kind |
| Role (`/role/<name>`) | Per stop: status (`braked` among them), outcomes, errors and releases not yet held, evidence against 1/δ with the budget and δ logged at the stop, audits and skips, and each audit with its answer; per week: runs, distinct steps checked, rounds until accepted, tokens per finished run, the person's minutes; the runs |
| Log (`/log`) | The latest 500 events, payloads cut at 600 characters, filterable by type and run |
| New run (`/new`) | Starts a run: a built-in role, one run before (proposed first, the last one run) or a role in the folder, the task, the project folder and an optional preview command |
| History (`/history`) | The report of `jaynes history`: how many past tool calls a person approved, by category |
| Setup (`/setup`) | First the guard (section 9.1): on or off with a switch, and the free owners, added and removed one at a time, the GitHub user found by gh marked. Then the optional action gate, installed in or removed from Claude Code's settings, keeping a backup |

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
person; any other client can read the page but not answer from it. When the
page restarts after an update it keeps its key, handed to the new start in
its environment and removed there before any run starts: the open tab sees
the restart, reloads into it and still answers, and no new tab opens.

Known limit: the agent runs as the same operating-system user as the person,
so a process that starts `jaynes review` itself and reads the printed link
gets the key. Separating the two needs isolation from the operating system,
such as a separate user or a confirmation the agent cannot reach; v0 does not
do it.

The CLI mirrors the essentials: `jaynes queue`, and `jaynes answer <run>
<step> accept|change|return|context [--note] [--output JSON|@file]`, which
also continues the run. `jaynes delivered` and `jaynes correct` find a
delivery and record its correction (section 6.2); changing a correction's
kind is the person's, on the page.

The local API, under `/api/` on the same server, has every action of the
page as JSON: `POST /api/<action>` with the fields as a JSON object, for
answer, continue, send-back, dismiss, correct, correction, undo, halt, resume,
start, add-package and remove-package, and `GET /api/home`,
`/api/stop/<run>/<step>`, `/api/run/<run>` and `/api/packages` for what the
page shows. The page's buttons go through the same functions
(`jaynes.api`). A request with the page key's cookie is the person's; any
other is an agent's. An agent may answer a stop that holds its run, send a
blocked run back, continue a run, start one, record a correction, halt, and
add or remove a package of roles;
only the person answers a sample (an agent's answer never counts as proof), a
floor or a braked stop, dismisses a run, files a correction as another kind,
undoes and resumes, and the API answers anyone else 403. It takes only
`Content-Type: application/json` and a local Host and Origin, so another
site's form cannot post to it. `jaynes mcp` serves the same reads and the
agent's actions over stdio, as an MCP server, needing no page running; the
plugin declares it, so Claude Code starts it wherever the plugin is on.

Halt all, in the page's sidebar, halts everything: each run returns `halted`
before its next step, a merge waits, and quota resets continue nothing. A turn
already under way finishes, and what it made waits too. Answers are still
recorded. Anyone may halt, an agent with `jaynes halt`, since it only makes
things wait; only a person resumes, with Resume in the same place, which
continues every unfinished run no process holds.

## 8. Events

One SQLite table, append-only, one row per event with sequence, time, type,
run or action id, and a JSON payload.

Conductor events:

| Event | Payload |
|---|---|
| `RunStarted` | role, task, role fingerprint, role reference and runtime overrides (for `jaynes run <id>`), inputs as resolved with their digests; for a plan's run, the request and its name in the plan, and the project's base when it works from an earlier run's branch |
| `StepStarted` | step, round, attempt, executor, workspace manifest hash; for an irreversible step, effect, idempotency key and payload |
| `TurnFinished` | step, attempt, files changed, tokens in and out from the runtime meter and the part of tokens in read back from the cache, model, seconds; on a web step, what it read; the shell commands the turn ran as the runtime saw them (the last 50, each with whether it exited 0 and the last 2000 characters of its output) |
| `TurnRejected` | step, attempt, reason: `write_boundary`, `schema`, `check`, `deadline`, `runtime`, `executor`; reasons |
| `TurnInterrupted` | step, attempt, reason `crash` or `quota`; not an attempt |
| `Checked` | step, round, attempt, output hash, check hash, source (agent, `jaynes`, `person`), model, what the step received |
| `InputsChanged` | step, round, the earlier steps whose output or change it received differently; the step runs again on top of its output |
| `StopOpened` | step, round, status (`floor`, `braked`, `certified`, `refuted`, `undetermined`), coin, n, k, pending, log evidence, stop fingerprint, budget, δ, question, evidence hash, floor paths, the outcomes and errors since the brake, whether it holds the run |
| `StopSkipped` | step, status `certified` with coin, budget and δ, or `small`; stop fingerprint |
| `StopAnswered` | step, answer, label, note, stop fingerprint, the event that opened the stop, seconds waited, seconds spent, by (`person` or `agent`), the earlier label it revises, the ids chosen |
| `StepReturned` | step whose output sent it back, the earlier step, the steps reset, reasons, round |
| `RunPaused` | reason `quota`, agent, until |
| `RunBlocked` | step, reasons, what the step received when an answer can change it |
| `RunFinished` | outcome: `delivered`, `ended` with the step whose accepted output ended it, `dismissed` by a person from the page, or `undone` when a person reverted its merge first |
| `Merged` | the branch merged into, the merge commit, the run's branch commit it merged, and `over`, the one merged before, for a correction |
| `MergeSkipped` | the run's branch commit and why it did not merge by itself |
| `MergeUndone` | the branch, the merges reverted and the commits that revert them, the person's note, by; `incomplete` when not every revert landed |
| `RequestPlanned` | the request's id instead of a run: the request in the person's words, the project, each planned run with its id, role reference, task, the runs it follows and its flags, and `by`, the run that planned it, when a role did |
| `RequestRunStarted` | the request's id: the name and id of a planned run Jaynes started |
| `Halted` | no run: by and a note; every run stops before its next step and merges nothing |
| `Resumed` | no run: by, always a person; the halt is lifted |
| `DeliveryCorrected` | id, the step that erred, the stop it counts on and its fingerprint, kind (`error`, `preference`, `info`), note, by, the kind it was before when a person refiled it |

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

### 9.1 The guard

The floor that always applies, with the gate or without it. The plugin's
`hooks/hooks.json` runs `hooks/guard.py` before every Bash, Edit, Write,
MultiEdit and NotebookEdit call, in every Claude Code session where the
plugin is on. The script uses only python3's standard library and never
imports jaynes, so it works before the `jaynes` command is installed.

- **What it holds.** git push, git reset --hard, git clean, git branch -D
  (and --delete --force), git tag -d, gh pr create, merge and close, gh repo
  create, delete and archive. A dry run (`-n`, `--dry-run`) of push or clean
  passes. A command counts only at a command boundary (the start, after `;`,
  `&&`, `||`, `|`, `&`, a newline or a parenthesis, `$(` or a backtick, after
  `if`, `then`, `do`, variable assignments and wrappers such as `env` or
  `sudo`, and inside `sh -c` or `eval`), with git's own options such as
  `-C <dir>` and `-c k=v` before the subcommand. Words inside quotes, comments
  and heredoc bodies are text, not commands.
- **How it holds.** It prints PreToolUse JSON with `permissionDecision`
  `ask` and a `permissionDecisionReason` saying in plain words what cannot be
  undone and why it asks. It never exits 2. In `claude -p`, where nobody can
  answer, Claude Code turns the ask into a deny. Everything else gets no
  output and exit 0, so Claude Code's own permission flow applies. Bad input
  and the guard's own errors also give no output.
- **Free owners.** A held action whose repository is on GitHub with an owner
  in the free owners list passes silently (owners compare case-insensitively).
  The repository is the hook's `cwd`, changed by `cd X` earlier in the line
  and by `git -C X`. For git push it is the push URL of the remote named in
  the command (origin when none is named), or the URL itself. For gh, `-R` or
  `--repo` owner/name names it, as does the repository argument of gh repo
  create, delete and archive; otherwise it is the `cwd`'s origin. A
  repository with no origin, or one not on GitHub, always asks.
- **Settings.** `$JAYNES_HOME/guard.json` (`~/.jaynes/guard.json`):
  `{"on": true, "owners": ["nora-builds"], "github_user": "nora-builds"}`.
  `on` is true when missing or not a boolean. When `owners` is not a list of
  strings (the file is missing or broken), the first held action finds the
  person's GitHub user with `gh api user --jq .login` (5 s timeout) and saves
  it as the only owner and as `github_user`, so later commands never wait on
  the network. If gh is missing, not logged in or slow, the list is empty and
  every held action asks. A configured list, even an empty one, replaces the
  GitHub user, and gh is not called.
- **Its own files.** An Edit, Write, MultiEdit or NotebookEdit of a file in
  the plugin's `hooks/` folder or of the settings file asks. So does a Bash
  command that visibly writes them: a redirection into them, `rm`, `mv`
  (source or target), `cp`, `ln`, `install` or `rsync` onto them, `tee`,
  `truncate`, `touch`, `chmod`, `sed -i` or `perl -i` on them, `dd of=`.
  Removing a folder that holds them asks too. Reading them passes.
- **Setup.** The Setup page reads and writes the same file through the
  `/setup` form with the page token (`guard-on`, `guard-off`,
  `guard-owner-add`, `guard-owner-remove`). The guard off is silent for
  everything.

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

Jaynes is validated on this user when its `screen-design` role shows: all
steps fulfilled with requirements approved, fewer rounds until acceptance,
fewer rejections after checks pass, tokens per screen measured and lower, and
an error rate stated per stop. Tokens per screen count new tokens (input
outside the cache, cache writes and output); what is read back from the cache
is shown apart, with the minutes. `visual-identity` and `redesign` stay for
changing a visual direction from references; they have no moments, objects,
walkthrough or requirements, so they cannot show the first criterion.

## 11. Milestones

| Milestone | Scope | Exit evidence |
|---|---|---|
| M1, done | Action gate, bounds, history report, simulation | Appendix A |
| M2, done | Conductor, role API, both runtimes, write boundary, durability, quota pause | Appendix B |
| M3, done | Review page and CLI answers | Appendix C |
| M4 | Built-in roles (`visual-identity`, `redesign`, `fix-bug`) on the first user's product | Two screens end to end with real stops; measures from section 10 recorded |
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

- Resolved: the method stays in the person's file, given as an input; the
  role keeps only the structure and the checks (`screen-design`).
- How a run of `screen-design` starts from requirements already approved, for
  the slices after the first.
- Whether the slice's test can be a browser test kept in the product: on
  macOS, Chrome opens in Jaynes's box only with its own sandbox off and the
  user's shared temporary folder writable, so today the test is a path Jaynes
  performs with its own Chrome.
- Whether `change` answers should feed the agent's examples for the step
  automatically.
- Turn deadline and `max_turns` defaults per step type.
- Whether resuming a run whose role fingerprint changed should continue, block,
  or restart the affected steps.

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
