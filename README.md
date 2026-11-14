# Jaynes

Make your AI agents follow the method, check what they deliver, and call you
only where it matters. For people who run a team of agents in Claude Code.

> **Alpha.** Jaynes runs end to end on macOS with Claude Code. Its protocol
> with roles is at version 1 and may still change. See [Status](#status).

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

1. **A role is a method.** Its steps, what each may reach, its checks and where
   it asks you. A role is a program of its own, in any language, that speaks a
   fixed protocol with the engine; the engine never runs its code inside itself.
2. **Jaynes runs your agent one step at a time.** Claude Code does each step in
   its own copy of your project, with settings Jaynes writes for that step. A
   step's output reaches the next only after its checks pass. A check the role
   marks hidden is proof the agent never sees.
3. **Where the method needs you, it asks and goes on.** You answer on a local
   page, a verdict per item (keep, change, redo, drop, or "I don't know") and a
   line of context in your own words. When you correct an answer later, only
   the steps that used what changed run again. Nothing that cannot be undone
   runs before you answer.
4. **It costs what it buys.** A small change takes the short path; rules in
   code raise the risk when the change touches more. Every run has a ceiling of
   time and tokens, said before it starts, and Jaynes's own faults never spend
   an attempt of the agent.
5. **A stop asks less only on evidence.** One that keeps being right asks only
   a sample of runs, by a test that holds however often it is checked; two
   errors among its last ten answers make it ask every time again.

## What it guarantees

The core is a pure function from a run's state and one event to what it
records and does next. What it promises is one page,
[docs/guarantees.md](docs/guarantees.md), seventeen sentences such as "something
you kept stays as it is unless you change it". Each is tested on thousands of
random runs, and twelve are proved in Lean for a model of the core that a test
compares with the code on every state of those runs
([engine/model](engine/model)).

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

The plugin also brings a guard: before a git or GitHub action that cannot be
undone, such as a push, it asks you.

## Roles

Two come with Jaynes, installed with `jaynes add`:

| Role | What it does |
|---|---|
| `small-change` | A change of a few lines; the project's suite must pass. Past 30 changed lines, or a file that runs outside any box, it becomes a high-risk run and asks you |
| `fix-bug` | A test that fails because of the bug, then a fix, judged by cases written from your request alone that the fixer never sees; merged into your branch, and undone with one click |

```sh
jaynes add https://github.com/alk0x1/jaynes#engine/roles/small-change
jaynes add https://github.com/alk0x1/jaynes#engine/roles/fix-bug
jaynes roles
```

A role of your own is a folder with a `role.toml`: the steps in order, what each
may reach, the checks on each from a menu Jaynes ships, where you are asked,
and what the role promises. Ask your agent for a new role and it writes one
with `jaynes new NAME`, then runs `jaynes check NAME`, which reads the file
strictly, says every error with its line, and proves each promise over every
way a run can go, or shows the run that breaks it:

```text
✔ fix-bug 3.0.0 reads: runner → cases → reproduce → found → fix
✔ "The agent that fixes never sees the cases the fix is judged by": proved (529,962 states)
✔ "The fix starts only after tests failed because of the bug": proved (529,962 states)
✔ "Nothing is merged before the hidden cases passed on the fix": proved (529,962 states)
```

`jaynes add` installs only roles whose promises hold. A role can also be a
program (`role.py` with the Python library, or any executable named `role`);
`jaynes conformance <folder>` says whether it answers the protocol as it
should. The protocol's schema is
[engine/protocol/role-protocol-1.schema.json](engine/protocol/role-protocol-1.schema.json).

## Commands

```text
jaynes start --role NAME --project DIR --request TEXT [--risk high]
jaynes show RUN | runs | page | verify
jaynes answer RUN QUESTION [ITEM=keep|change:TEXT|redo:TEXT|drop|skip] [--context TEXT] --send
jaynes halt RUN | resume RUN | continue RUN | abandon RUN | undo RUN | correct RUN --note TEXT
jaynes add SOURCE | remove NAME | roles | update | guide
jaynes new NAME | check DIR
```

The record of every run lives in `~/.jaynes/engine`, each entry chained to the
one before it, so a change to any past entry shows (`jaynes verify`).

## Status

| Part | State |
|---|---|
| Core, its seventeen guarantees, the Lean model | Works; the guarantees are tested on random runs, twelve are proved |
| Claude Code as the agent: per-step settings, leaves asked mid-turn, fixes in place in the same session | Works; checked with real turns |
| The page: questions across runs, verdicts per item with pictures, context, each run's view and actions | Works |
| Recovery after a crash, a record that shows tampering, runs pinned to their role's and engine's version | Works in tests |
| Merging alone, and Undo | Works in tests |
| Sampled stops and the brake | Works in tests; no real certification yet |

Missing, roughly in order: plans of several runs, the Codex adapter, roles for
design work on this protocol, Linux.

The same change that took the earlier engine three runs and 1 h 35 min without
delivering was delivered by this one in under ten minutes, in one attempt
([the design record](docs/specs/2026-11-12-engine-redesign-design.md)).

## From a clone

```sh
cd engine
cargo test                                   # the guarantees, the Lean model, the engine end to end
cargo install --locked --path crates/shell --root ~/.local
```

The Lean model needs Lean 4 (elan); without it, its test is skipped.

## License

Apache-2.0.
