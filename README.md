# Jaynes

Make your AI agents follow the method, check what they deliver, and call you
only where it matters. For people who run a team of agents in Claude Code.

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
2. **Jaynes conducts the agent one step at a time, always on the same model.**
   Claude Opus at high effort unless you set `JAYNES_CLAUDE_MODEL` or
   `JAYNES_CLAUDE_EFFORT`; a turn another model answers does not count. Claude Code runs a single
   step and can only write that step's output. The next step starts
   when the check passes. Evidence the agent could fake, like the references
   captured from the web, is produced by Jaynes.
3. **Where the method needs you, it shows you, and goes on.** The run
   continues with the agent's choice and delivers its branch; the question and
   its evidence wait for you in the review page, all of them at once, or come
   to you in Claude Code's question dialog. A return corrects on top: only
   the steps that used what changed run again. What cannot be undone, and a
   change to files that run outside the box, waits for your answer. A role
   whose delivery is a merge proves its work with checks written from your
   request alone, which its maker never sees, and merges by itself once they
   all pass; Undo reverts it.
4. **What counts is what happened after delivery.** An accept counts as
   right only once its delivery went 7 days with nobody correcting it; a
   change or a return counts as an error at once, and so does a correction
   found later, on the first stop after the step that erred. A stop that
   keeps proving right becomes sampled: it asks one time in ten. Two errors
   among its last ten outcomes brake it: it asks on every run and the run
   waits for you there, until the deliveries after the brake prove it right
   again.
5. **Underneath, every tool action goes through a gate.** Some actions, like
   pushing or deleting history, never run without you.

The whole flow, from a role to the evidence behind each stop, is in
[docs/how-it-works.png](docs/how-it-works.png) (source:
[how-it-works.drawio](docs/how-it-works.drawio)).

A sketch of the `screen-design` role (a working role is under
[Write a role](https://jaynes.dev/docs/write-a-role.html)):

```python
from jaynes import role, stop
from checks import cites_real_data, tests_pass, contrast_aa, tokens_only

design = role("screen-design")

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
| Package | A set of roles, installed at a version and used by the projects that list it, as an npm library; the engine has no role of its own |
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
at most 5%. With a 5% budget that takes 88 clean deliveries in a row; with
20%, it takes 20. The test is a mixture likelihood ratio, valid at any stopping time
by Ville's inequality. Details in [SPEC.md](SPEC.md).

## Status and roadmap

| Part | State |
|---|---|
| Action gate for Claude Code: categories, floors, sampled audits, labels from the transcript, anytime-valid certification | Works |
| Guard in every Claude Code session with the plugin: asks before git and GitHub actions that cannot be undone (push, reset --hard, clean, branch -D, tag -d, pull requests and repositories created, merged, closed, deleted or archived) outside your free owners' repositories, and before changes to its own files; switch and owners on Setup | Works in tests |
| History report and guarantee simulation | Works |
| Step conductor for Claude Code (Codex also runs): roles in Python or TOML, write boundary, checks, retries, resume after a crash, quota pause | Works |
| Review page and CLI answers: queue, stops, runs, roles, log; return to an earlier step; dismiss a run; running and paused runs in the queue, a paused run continuing at its reset | Works |
| Answers bound to who gave them and to the evidence shown; floor paths; irreversible steps with idempotency keys | Works |
| Counts from the outcome: an accept counts once its delivery held 7 days, corrections after delivery (`jaynes correct`, kind changeable on the page), the brake after two errors in ten | Works in tests; no real correction yet |
| Proof the maker cannot see, in build-feature and fix-bug: cases written from the request alone before the work, run by Jaynes against the changed project served in a box; a failing case sends the maker back with its sentence only | Works in tests; no real run yet |
| Merging alone, in build-feature and fix-bug: once every hidden check passes, Jaynes merges the branch into the branch you started from, only over a clean checkout, never pushing; a return merges the correction on top; Undo on the run's page reverts it | Works in tests; no real run yet |
| Halt all on the page (and `jaynes halt`): no run takes a step or merges until you resume | Works in tests |
| The page's home in four parts: what holds a run waits for you, the samples the runs went past, what was delivered since your last visit with its hidden checks, and what runs now; the browser opens by itself only for what holds a run | Works in tests and on a scratch page |
| A local API with every action of the page (`/api/` on the page's address, JSON), and the MCP server (`jaynes mcp`) the plugin starts in Claude Code: an agent answers what holds a run and sends a blocked run back; only you answer samples, drop a run, undo and resume | Works in tests; a Claude Code agent used it on a scratch home |
| Real projects: steps on copies, previews before and after, a branch at the end | Works |
| Box for previews on macOS; web access per step, with every address recorded | Works |
| Package design, which comes with Jaynes: visual-identity, redesign (from references) | Works, first real runs in progress |
| Package software, which comes with Jaynes: fix-bug, a failing test first, then a fix, both run in the box | Works; first real run delivered its branch |
| Package software: build-feature (the agreed plan's tests first, shown as examples with sketches of the screens, then code that keeps the project's identity) and small-change (no stop unless the change is big) | build-feature delivered four slices of a real Go project; small-change works in tests |
| Package design: screen-design, your design method from how the screen is used (moments, objects, walkthrough, requirements), then one slice measured, proved and reviewed; Jaynes's own Chrome on data frozen at the start | Works in tests on an invented product; no real run yet |
| Inputs a role declares, frozen at the start; choosing among an output's items at a stop; Jaynes sending a step back with what it found | Works |
| A request as a plan of runs (`jaynes plan`): each run starts once the runs it builds on delivered, from where their delivery landed, without waiting for samples; the page shows the plan | Works in tests and on a scratch page |
| Roles are packages, as npm libraries: `jaynes add` installs one at a fixed version, from git or a folder of yours, for one project or every project; `jaynes roles` lists what a project can start; the engine has no role of its own | Works in tests and on a scratch home with a private package |
| Packages on the page (Roles: what is installed, who uses what, add and remove) and for agents (API and MCP: `packages`, `add_package`, `remove_package`) | Works in tests and on a scratch page |
| A role whose step delivers a plan, which Jaynes checks and starts once its run delivered | Works in tests; a private direction role planned a real request with Claude Code |

Missing, roughly in order:

- **Published package.** Not on PyPI yet; install from GitHub (below).
- **A first real case**, measured against the first user's baseline.
- **Corrections after delivery.** A return corrects the branch until you
  answer the last stop; after that, a correction is a new run, which the
  agent records with `jaynes correct` so it counts. The agent finds the
  delivery from the list `jaynes delivered` prints, not yet from the code's
  history; a new preference is not yet kept as taste memory; every project
  waits 7 days, not yet a time of its own. A step cannot yet start over from
  scratch, giving its reason, instead of correcting on top.
- **Changing a plan.** A role can plan a request, and its plan's sample
  counts; once it started, you can drop a run that has not started, but not
  add or change one, and a plan cannot be started from the page.
- **small-change merging alone.** It has no hidden checks yet, so the
  person still merges it.
- **Halting a turn under way.** Halt all stops every run before its next
  step; a turn already running finishes first.
- **A turn past its deadline keeps its work.** The next attempt starts from
  nothing, even when the tests were almost written.
- **Old runs leave the queue.** A run waiting for two days from a test that
  was dropped sits next to today's with the same weight.
- **The page as an app.** Stops are answered on the page, and the terminal
  only says where; next, the page installed as an app (a PWA) with macOS
  notifications.
- **More roles in the software package** that need no design taste to check,
  such as upgrading dependencies.
- **Two or three directions to choose from** in redesign. Stops can let you
  choose among an output's items now; the design roles still propose one
  direction.
- **The slices after the first** in screen-design. A run builds the first
  slice of the requirements you approved; how the next run starts from them
  is open.
- **A box on Linux.** There, previews and test runs refuse to run unless
  `JAYNES_UNBOXED=1`.
- **Roles without Python**, for people who don't write code.
- **Judges calibrated on your labels**, so fewer answers are needed.
- **Keeping a stop's labels across agent model versions.** Today a new model
  starts the stop's count over; keeping them needs a detector that withdraws
  the certification if errors rise.
- **Answers only you can give.** An agent running as your user can start its
  own review page and read the link it prints, then answer as you. Closing
  this needs a confirmation the agent cannot reach, such as Touch ID.

## Get started

Type this in Claude Code, answer `y` to add the marketplace, and press Enter
to install it for your user:

```text
/plugin install jaynes --marketplace alk0x1/jaynes
```

Then work as usual: approve a plan ("go ahead", "do it") and your agent hands
it to Jaynes, installing it the first time (it is built with Rust's `cargo`).
Where a run needs you, you answer on the page, at http://127.0.0.1:8750, which
Jaynes opens in your browser.

The docs are at **[jaynes.dev/docs](https://jaynes.dev/docs/)**:

- [Get started](https://jaynes.dev/docs/): install, the page, the guard.
- [Runs and stops](https://jaynes.dev/docs/runs.html): what a run does with your project, and
  what each answer does.
- [Roles that come with Jaynes](https://jaynes.dev/docs/roles.html): software (build-feature,
  fix-bug, small-change) and design (visual-identity, redesign,
  screen-design).
- [Packages](https://jaynes.dev/docs/packages.html): roles of your own or of your team, for one
  project or for all.
- [Write a role](https://jaynes.dev/docs/write-a-role.html): your method, in Python or TOML.
- [Commands](https://jaynes.dev/docs/commands.html): every `jaynes` command.

### From a clone

The engine is in `engine/` (Rust): its core, the guarantees it proves
([docs/guarantees.md](docs/guarantees.md)) and the page.

```sh
cd engine && cargo test               # the guarantees on random runs, the Lean model, the engine end to end
cargo install --locked --path crates/shell --root ~/.local
```

The Python engine it replaces is still in `src/`:

```sh
uv sync
uv run pytest
uv run python docs/manual/build.py   # the docs, after changing docs/manual or a command's help
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
