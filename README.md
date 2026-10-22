# Jaynes

Make your AI agents follow the method, check what they deliver, and call you
only where it matters. For people who run a team of agents in Claude Code and
Codex.

> Status: early. The action gate and the step conductor work today. The
> review page is being built. See [Status](#status).

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

A role, in the planned API (subject to change):

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

```sh
jaynes run screen-design --task "Redesign the People screen"
jaynes review          # opens the review page
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

## The guarantee

A stop becomes sampled only when its evidence clears a threshold that holds no
matter how often it is checked: if the true rate at which you change that
stop's output is at or above its budget, the chance it ever becomes sampled is
at most 5%. With a 5% budget that takes 88 accepts in a row; with 20%, it
takes 20. The test is a mixture likelihood ratio, valid at any stopping time
by Ville's inequality. Details in [SPEC.md](SPEC.md).

## Status

| Part | State |
|---|---|
| Action gate for Claude Code: categories, floors, sampled audits, labels from the transcript, anytime-valid certification | Works today |
| History report and guarantee simulation | Works today |
| Step conductor for Claude Code and Codex: role API, write boundary, checks, retries, resume after a crash, quota pause, stops through the Python API | Works today |
| Review page | In progress |
| Judges calibrated on your labels, fewer labels needed | Next |

## Try what works today

Read-only, on your own Claude Code history:

```sh
uv sync
uv run jaynes history --config examples/agent-team.jaynes.toml --days 30
uv run jaynes simulate
```

A two-step role through the conductor (uses your Claude Code login, one short turn):

```sh
PYTHONPATH=. uv run jaynes run --role examples.count_words:ROLE --task "lighthouses in fog"
```

To install the action gate as a Claude Code hook, copy a policy to
`~/.jaynes/jaynes.toml` and add `jaynes hook` to `PreToolUse`, `PostToolUse`,
`PostToolUseFailure`, `Stop` and `SessionStart` in `~/.claude/settings.json`.
Keep any deny hook you already have. On an internal error the hook prints
nothing and Claude Code decides as if Jaynes were not installed.

## Foundations

Typestate and session types for methods that cannot be skipped; object
capabilities for per-step write access; algebraic effect handlers for one
mechanism behind gating, auditing and replay; anytime-valid inference with
e-values for the guarantee; prediction-powered and active statistical
inference for needing fewer of your answers. References in the
[design document](docs/specs/2026-10-21-landing-and-readme-design.md#5-foundations).

## License

Apache-2.0
