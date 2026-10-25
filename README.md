# Jaynes

Make your AI agents follow the method, check what they deliver, and call you
only where it matters. For people who run a team of agents in Claude Code and
Codex.

> **Alpha.** Jaynes runs end to end on macOS with Claude Code, but its role
> API and stored formats will still change, and it is not on PyPI yet. See
> [Status and roadmap](#status-and-roadmap).

## The problem

Your agents say "done". They skipped half the process, the automated checks
passed, and the result is still wrong, so you fix it yourself. You either
review everything and become the bottleneck, or let them run and have no idea
how often they get it wrong.

In a one-person company run by agents, over seven weeks: 11,894 tool calls,
25 of them seen by the person. Of 8 design screens, none followed the first
four steps of the design method, and screens that passed every automated check
were still sent back.

## How it works

1. **Describe a role as a method.** Ordered steps, each with a required output
   and a check.
2. **Jaynes conducts the agent one step at a time.** Claude Code or Codex runs
   a single step and can only write that step's output. The next step starts
   when the check passes. Evidence the agent could fake, like a walkthrough of
   a screen, is produced by Jaynes.
3. **Where the method needs you, the run stops.** The question and its
   evidence wait for you in the review page.
4. **Every answer is a label.** A stop you keep accepting without changes
   becomes sampled: it asks one time in ten. Change one and it is mandatory
   again.
5. **Underneath, every tool action goes through a gate.** Some actions, like
   pushing or deleting history, never run without you.

The whole flow, from a role to the evidence behind each stop, is in
[docs/how-it-works.png](docs/how-it-works.png) (source:
[how-it-works.drawio](docs/how-it-works.drawio)).

A sketch of the `screen-design` role (a working role is under
[Write a role](#write-a-role)):

```python
from jaynes import role, stop
from checks import cites_real_data, tests_pass, contrast_aa, tokens_only

design = role("screen-design", agent="codex")

moments = design.step("moments", output=Moments, stop=stop(budget=0.20))
objects = design.step("object-map", output=ObjectMap, needs=moments, check=cites_real_data)
walk = design.step("walkthrough", output=Walkthrough, needs=objects, run_by="jaynes")
reqs = design.step("requirements", output=Requirements, needs=walk, stop=stop(budget=0.10))
build = design.step("implement", output=Patch, needs=reqs, check=[tests_pass, contrast_aa, tokens_only])
done = design.step("accept", needs=build, stop=stop(budget=0.05, sampled=False))
```

## Concepts

| Term | Meaning |
|---|---|
| Role | A kind of work: screen design, copy review, bug fix |
| Step | One unit of a role's method, with a required output |
| Check | Code that must pass before the next step starts |
| Stop | A step where the method needs a person |
| Label | Your answer at a stop: accepted, changed, or returned |
| Budget | The error rate a stop may have once it stops being mandatory |
| Floor | An action or stop that never runs without a person |
| Effect | What a trusted step does to the world: `pure`, `read`, `write-copy`, `web` or `irreversible` |

## The guarantee

A stop becomes sampled only when its evidence clears a threshold that holds no
matter how often it is checked: if the true rate at which you change that
stop's output is at or above its budget, the chance it ever becomes sampled is
at most 5%. With a 5% budget that takes 88 accepts in a row; with 20%, it
takes 20. The test is a mixture likelihood ratio, valid at any stopping time
by Ville's inequality. Details in [SPEC.md](SPEC.md).

## Status and roadmap

| Part | State |
|---|---|
| Action gate for Claude Code: categories, floors, sampled audits, labels from the transcript, anytime-valid certification | Works |
| History report and guarantee simulation | Works |
| Step conductor for Claude Code and Codex: role API, write boundary, checks, retries, resume after a crash, quota pause | Works |
| Review page and CLI answers: queue, stops, runs, roles, log; return to an earlier step | Works |
| Answers bound to who gave them and to the evidence shown; floor paths; irreversible steps with idempotency keys | Works |
| Real projects: steps on copies, previews before and after, a branch at the end | Works |
| Box for previews on macOS; web access per step, with every address recorded | Works |
| Built-in roles: visual-identity, redesign (from references) | Works, first real runs in progress |

Missing, roughly in order:

- **Published package.** Not on PyPI yet; install from a clone with
  `uv tool install .`.
- **A first real case**, measured against the first user's baseline.
- **More built-in roles** that need no design taste to check: fix a bug (a
  failing test first), build a feature (a spec stop, then tests), upgrade
  dependencies. They need checks that run the project's tests in the box.
- **Choosing among two or three directions** at a stop, instead of one.
- **Dismissing a blocked run** from the page.
- **A box on Linux.** There, previews refuse to run unless
  `JAYNES_UNBOXED=1`.
- **Codex web search**, wired but not yet verified in a live run.
- **Roles without Python**, for people who don't write code.
- **Judges calibrated on your labels**, so fewer answers are needed.
- **SPEC.md** brought up to date with projects, references and web steps.

## Get started

Python 3.12 or later.

```sh
uv tool install jaynes-ai      # or: pip install jaynes-ai
jaynes                         # opens the page on http://127.0.0.1:8740
```

Everything else happens in the page:

- **New run** starts a run: pick a role, write the task, and point it at a git
  repository. The demo needs no repository, API key or quota: a scripted agent
  works through a screen-design role, a check sends one step back, and the run
  stops for your answer. **visual-identity** comes with Jaynes: the agent reads
  the project's current colours and fonts, proposes a direction (Jaynes checks
  its contrast), and applies it to the style files. **redesign** goes further:
  it also changes motion and the markup of the screens (HTML, JSX, TSX, Vue,
  Svelte), keeping what each screen does. It starts from references: put one
  or more links or image paths of interfaces you like in the task. Jaynes
  captures each link with headless Chrome and gives the pictures to the agent,
  and the direction must say what it takes from each one. Agents run without
  the web unless a step allows it (`web=True`); the direction step does, so
  the agent can read the references' sites and look up fonts, and the stop
  lists every address it read. Shell commands stay offline. With a preview command, such as
  `npm run dev -- --port {port}`, each stop shows the project before and after.
  The preview runs the agent's changes, so on macOS it runs in a box: it writes
  only to its copy, cannot read where secrets live, and reaches no network but
  the local ports you list (a backend it needs). Elsewhere previews refuse to
  start unless `JAYNES_UNBOXED=1` accepts running them unboxed.
  At a stop, return and context can go back to an earlier step: it runs again
  with your note, and so does every step after it. Steps work on copies of the
  last commit; when the run finishes, the accepted changes are a branch,
  `jaynes/<run>`, and your checkout is untouched.
- **History** reads your Claude Code transcripts and shows how many tool calls a
  person approved. It changes nothing.
- **Setup** installs the action gate in Claude Code: `jaynes hook` on
  `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `Stop` and `SessionStart`,
  with the policy in `~/.jaynes/jaynes.toml`. Your other hooks stay, and
  `settings.json` is copied to `settings.json.jaynes-backup` first. On an
  internal error the hook prints nothing and Claude Code decides as if Jaynes
  were not installed.

The same from a terminal or an agent: `jaynes demo`, `jaynes history`,
`jaynes simulate`, `jaynes policy` (prints the built-in policy), `jaynes queue`
and `jaynes answer`. `jaynes --help` lists them all.

Only you answer as a person. The page answers only in a browser opened with
the link `jaynes` prints when it starts (it opens it for you), which it keeps
as a cookie; anything else, like an agent with `curl`, can read the page but
not answer. An answer from `jaynes answer` releases the stop but is recorded
as an agent's: it never counts as a label, so an agent cannot make a stop
sampled by accepting its own work, and a floor stop refuses it. Every answer
carries the hash of what was shown, so a page left open cannot accept a newer
round it never showed.

Some files run later, outside any box: agent and editor settings (`.claude`,
`.vscode`, `CLAUDE.md`), git attributes and hooks, `.envrc`, `package.json`,
`pyproject.toml`, CI workflows. A step that changes the project must list what
it may change (`may_change`, `["**"]` for anything), and even then a change to
one of those files is rejected in a step without a stop, and turns a step's
stop into a floor that shows the file's diff.

### Write a role

A role is a Python module. Save this as `count_words.py`:

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


ROLE = role("count-words", agent="claude")
TOPIC = ROLE.step("topic", output=Topic, run_by=read_topic)
SENTENCE = ROLE.step("sentence", output=Sentence, needs=[TOPIC], check=matches,
                     instructions="Write one sentence about the topic in inputs/topic.json with exactly "
                                  "the number of words given there.")
```

A trusted step that acts on the world declares it: `effect="irreversible"`
(book, pay, send) needs only steps with floor stops, so you approve its whole
payload. It runs once, with `ctx.idempotency_key` for the service
it calls, and a run that stopped while it was running blocks instead of running
it again.

Jaynes runs the first step itself; Claude Code (with your login) writes the
sentence, and the check sends it back until the count is right. Use
`agent="codex"` for Codex. Start `jaynes` from the folder that holds the file,
then pick the role under New run (type `count_words:ROLE` the first time), or:

```sh
jaynes run --role count_words:ROLE --task "lighthouses in fog"
```

### From a clone

```sh
uv sync
uv run pytest
```

## Foundations

Typestate and session types for methods that cannot be skipped; object
capabilities for per-step write access; algebraic effect handlers for one
mechanism behind gating, auditing and replay; anytime-valid inference with
e-values for the guarantee; prediction-powered and active statistical
inference for needing fewer of your answers. References in the
[design document](docs/specs/2026-10-21-landing-and-readme-design.md#5-foundations).

## License

Apache-2.0
