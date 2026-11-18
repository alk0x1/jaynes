# Jaynes

Make your AI agents follow the method, check what they deliver, and call you
only where it matters. For people who run a team of agents in Claude Code.

> **Alpha.** Jaynes runs end to end on macOS with Claude Code, and on nothing
> else yet. Its protocol with roles is at version 1 and may still change. See
> [Status](#status) and [What is still true](#what-is-still-true).

## The problem

Your agents say "done". They skipped half the process, the automated checks
passed, and the result is still wrong, so you fix it yourself. You either
review everything and become the bottleneck, or let them run and have no idea
how often they get it wrong.

An agent writes a fix and a test, the test passes, and it reports done.
But the test never failed before the fix, so it proves nothing, and the bug
is still there the next time someone hits it. Nothing in the run told you.

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

6. **A run says what it did.** When it finishes, `jaynes summary RUN` tells it
   to someone who never installed Jaynes, in Markdown to paste in a merge
   request: the role and its version, each step with the checks that passed,
   the promises proved of the role, how it was delivered, and what it cost in
   agents' work and in people's. How long a run waited at its stops and how
   long a person gave it are never mixed: a person's time is measured only
   from their presence on a stop's screen, and a stop answered with none
   recorded says "not measured", never zero. The page and the app show the
   same summary under a finished run, with a button that copies it.

## What it guarantees

The core is a pure function from a run's state and one event to what it
records and does next. What it promises is one page,
[docs/guarantees.md](docs/guarantees.md), seventeen sentences such as "something
you kept stays as it is unless you change it". Each is tested on thousands of
random runs, and twelve are proved in Lean for a model of the core that a test
compares with the code on every state of those runs
([engine/model](engine/model)).

## Get started

On a Mac with Apple silicon, in a terminal:

```sh
curl -fsSL https://jaynes.dev/install.sh | sh
```

It installs the Jaynes app in `/Applications` (`~/Applications` when you cannot write there), the `jaynes` command in
`~/.local/bin`, [herdr-core](https://github.com/herdrdev/herdr-core) (which
keeps the app's terminals alive) when you have none, and the plugin in Claude
Code. `jaynes update` installs the next release the same way. The app is signed
ad hoc rather than by an Apple developer account, so macOS asks you once after
each update before it reads its key in your keychain; the source is all here, to
read or to build yourself.

Only the plugin, from Claude Code (the `jaynes` command is then built with Rust's
`cargo` the first time):

```text
/plugin install jaynes --marketplace alk0x1/jaynes
```

Then work as usual: approve a plan ("go ahead", "do it") and your agent hands
it to Jaynes. Where a run needs you, you answer in the app, or on the page at
http://localhost:8750, which Jaynes opens in your browser.

The plugin also brings a guard: before a git or GitHub action that cannot be
undone, such as a push, it asks you. Pushes and pull requests to your own
repositories pass; to let them pass for an organization you work in, add it to
Free owners in Setup, and every repository of that organization passes at once.
A forced push, deleting a branch or a tag, and deleting or archiving a
repository ask every time, on your own repositories too.

## The app

A window where you work with your agents. At the left, your agents and shells
in their spaces, then the runs: what waits for you first, then each project's.
In the middle, Claude Code's terminal, an agent you open from the column, a
run's tab with its story, or the Inbox of what needs you across them all. A
run's question shows over Claude Code's terminal and on its tab, where you
answer it; a run's tab also has the page's own controls (Resume, Continue,
Raise the risk, Raise the ceiling, Abandon), each only when it can do
something. The terminals live in herdr-core, not in the app, so closing the
window leaves every conversation running; the app opens again as you left it.
Command-K finds any agent, run or command by typing.

Its settings (Command-comma) are a file of yours, kept as you change them:
dark, light (Linen) or as macOS is; the terminal's colours, font, size, Option
key, copy on select and scroll speed; notifications; the command a new agent
starts; the editor a file opens in.

## MCP, and your instruction files

The plugin brings three things to Claude Code: the `jaynes` skill, which hands an approved plan to Jaynes; the
guard; and an MCP server, `jaynes mcp`, which Claude Code starts over stdio. The server offers seven tools:

| Tool | What it does |
|---|---|
| `home` | What waits for you, what is blocked and what runs, across every run |
| `roles` | The roles installed, by name |
| `start` | Starts a run of a role on a project, with the agreed plan as its request |
| `run` | Where a run is: its steps, what it waits for, what it spent |
| `answer` | Lets a result go on as it is, at a stop the role marks as a release, and nothing else |
| `halt` | Halts a run at once; only you resume it |
| `correct` | Records that a delivered run was wrong, so the rework counts |

What comes through the server is an agent's: the engine refuses there what only you may do.

Your own Claude Code session keeps your project's instruction files (`CLAUDE.md` and its settings) as they are:
Jaynes does not write them. Inside a run, the agent does not read them: Jaynes starts it with them turned off and
writes each step's settings itself, so what a run must know goes in the request or in the role. Where your code
and data go: [docs/where-your-data-goes.md](docs/where-your-data-goes.md).

## Roles

A role names what a run works on, its target. Today the one kind is a
repository: the run works in a copy of it at the commit it had, its reach is
given in the repository's paths, its risk is judged by the lines and files it
changes, and it delivers a branch, or a merge that Undo reverts.

Roles are listed in an index, [jaynes-roles](https://github.com/alk0x1/jaynes-roles), and installed by name with
`jaynes add`, which checks the role is what the index lists and proves its promises again. Two to start:

| Role | What it does |
|---|---|
| `small-change` | A change of a few lines; the project's suite must pass. Past 30 changed lines, or a file that runs outside any box, it becomes a high-risk run and asks you |
| `fix-bug` | A test that fails because of the bug, then a fix, judged by cases written from your request alone that the fixer never sees; merged into your branch, and undone with one click |

```sh
jaynes add small-change
jaynes add fix-bug
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
should. Such a program is code of whoever wrote it, and it runs with your
rights, outside any box: only a role in `role.toml` keeps every command it
runs on your project in Jaynes's box. Install a program role only from someone
you trust. The protocol's schema is
[engine/protocol/role-protocol-1.schema.json](engine/protocol/role-protocol-1.schema.json).

## Commands

```text
jaynes start --role NAME --request TEXT [--project DIR] [--risk high]
jaynes show RUN | runs | summary RUN | log RUN | cost | verify | page [--port N]
jaynes answer RUN QUESTION [ITEM=keep|change:TEXT|redo:TEXT|drop|skip] [--context TEXT] --send
jaynes halt RUN | resume RUN | continue RUN | abandon RUN | raise RUN high|--seconds N|--tokens N
jaynes undo RUN [--note TEXT] | correct RUN --note TEXT [--step STEP]
jaynes add SOURCE | remove NAME | roles | update | uninstall | guide
jaynes new NAME | check DIR | conformance DIR | schema
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
| The MCP server for agents in Claude Code | Works |
| A run's summary, with people's time from their presence at stops | Works |
| The app: agents, runs, stops and terminals in one window | Works on macOS; alpha |

Missing, roughly in order: plans of several runs, the Codex adapter, roles for
design work on this protocol, Linux, targets other than a repository.

What Jaynes costs is measured on your own runs: `jaynes cost` adds up the
finished ones by risk and outcome, with the minutes and tokens they spent, how
often a result was sent back, how often a delivery was corrected later, and
the attempts the agent spent.

## Maintenance

Jaynes has one maintainer today, @alk0x1.

## What is still true

- **Alpha.** Formats and the protocol may still change between releases.
- **One system.** The installer is for macOS on Apple silicon. A run's agent
  works inside the macOS sandbox that Claude Code's guards need, so runs need
  macOS; the program alone builds from source elsewhere.
- **One agent program.** Claude Code is the only agent Jaynes has an adapter
  for: it writes each step's settings for it and reads its stream. Any other
  program runs only as a plain command (`--agent command:CMD`), with the
  prompt on its input and none of the step's guards set for it.
- **One target.** A run works on a repository; nothing else yet.

## How to leave

Nothing of yours is locked in a format only Jaynes reads.

- **The record** is one SQLite file, `~/.jaynes/engine/record.sqlite3`. Every
  change of every run is a row of its `events` table (the run, the event's
  type, its time, the event itself as JSON, and the hash that chains it to the
  one before), readable with `sqlite3` alone.
- **A role** is text: a folder with a `role.toml` and the prose its agent
  reads. The starting file `jaynes new` writes says what each key does in its
  comments, and what a role becomes for the engine follows a published schema,
  [engine/protocol/role-protocol-1.schema.json](engine/protocol/role-protocol-1.schema.json)
  (also `jaynes schema`).
- **What a run delivered** is a branch or a merge in your own repository, with
  its summary in Markdown.
- **To remove Jaynes:** `jaynes uninstall` shows everything it would take away, then removes it on your yes: the
  Claude Code plugin first, then the app, herdr-core only when Jaynes installed it and nothing changed it, the
  record only when you ask (`--delete-data`; `--keep-data` keeps it), and the program last. It removes nothing
  while the app is open or a run's agent works, and it never touches your repositories: it lists what runs left
  there (their `jaynes/*` branches) with the command that removes it.

## From a clone

```sh
cd engine
cargo test                                   # the guarantees, the Lean model, the engine end to end
cargo install --locked --path crates/shell --root ~/.local
```

The Lean model needs Lean 4 (elan); without it, its test is skipped.

## License

Apache-2.0.
