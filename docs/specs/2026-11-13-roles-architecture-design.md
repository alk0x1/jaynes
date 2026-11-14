# Roles: what a role is, where it lives, how it is made

**Status:** decided with the owner in chat on 2026-11-13. Not built. The last section is the handoff for the
session that builds it.

A role is the method for one kind of work: its steps, what each step may reach, the checks on each step, where
the person is asked, and what the role promises. The engine runs it. A specialist (Helm) is something else: it
uses several roles and brings knowledge and taste. Never model a specialist as one role.

## Why now

The Rust engine is done and nearly unused: there are two roles, small-change and fix-bug, both Python programs
written by hand. Whatever is bigger than a small change has no role, so most real work goes around Jaynes. Roles
are where the return is now. Before writing more of them, the owner wanted their architecture settled: what a
role is made of, where it lives, how it is made, and how it is shared.

## Decisions

### 1. A role is a folder of data, not a program

```
hotfix/
  role.toml          what is enforced: steps, order, reach, checks, stops, promises
  fix.md             optional: a step's instructions, when they are too long to sit in role.toml
  checks/odd.sh      optional: a check the menu does not have
```

The owner chose TOML over Markdown ("markdown parece não ter rigor"), JSON (hard to write by hand) and a new
format (a parser to write and a format nobody knows, for rigor the engine already gives). Rigor does not come
from the file's syntax. It comes from reading the file strictly into the engine's own types
(`jaynes_core::manifest::Manifest`), so that anything wrong is refused before a run:

- an unknown key (`chek`), refused, with its line;
- a value outside its closed list (`project = "tudo"`);
- a step that needs a step that does not exist, or a cycle;
- a check name that is not on the menu and has no script;
- a placeholder in instructions that the step may not see (see 3);
- a promise that does not hold (see 5).

Code roles stay possible: a folder with `role.py` (the library `engine/python/jaynes_role.py`) or an executable
`role` still works as today, for people who need the full power or another language. They are the exception.

What moves where, taking small-change as it is today (`engine/roles/small-change/role.py`, 84 lines):

| Today, in Python | Becomes |
|---|---|
| `role.step(...)`, reach, stop | `role.toml` |
| `instructions=("...")` | the `instructions` text, in `role.toml` or a `.md` |
| `find_suite()` | menu item `find-suite`, a role step |
| `suite()` | menu item `suite-passes` |
| `has_summary()` | menu item `field-max-length` |

The logic that was in each role (find the suite, run it in the box, read the result) is written once by Jaynes,
tested once, and picked by name. Today small-change and fix-bug each carry their own copy of "run the suite".

Why not leave roles in Python: a Python function can do anything, so nothing can be proved about it (a prompt
function could put a hidden step's output in the agent's text); installing one runs a stranger's code on every
call; and only programmers write it.

### 2. A sketch of `role.toml`

```toml
role = "fix-bug"
version = "3.0.0"
delivery = "merge"            # "branch" (default) or "merge"

[[step]]
name = "runner"
by = "jaynes"                 # a menu step of Jaynes's own: no agent
use = "find-suite"

[[step]]
name = "reproduce"
by = "agent"
needs = ["runner"]
reach = { project = "write", write = ["tests/**"], commands = true }
instructions = """
Write tests that fail because of the bug described in the request:

{{request}}

The command that runs the suite: {{runner.command}}
"""
checks = ["tests-fail-for-the-bug"]
stop = { ask = "Do these tests show the bug?", verdicts = ["keep", "redo"], budget = 0.2 }

[[step]]
name = "cases"
by = "agent"
hidden = true
instructions = "cases.md"

[[step]]
name = "fix"
by = "agent"
needs = ["reproduce"]
reach = { project = "write", write = ["**"], commands = true }
instructions = "fix.md"
checks = ["suite-passes", { use = "hidden-cases-pass", reads = ["cases"] }]
stop = { ask = "Keep this fix?", verdicts = ["keep", "redo"] }

[[promise]]
says = "Nothing is merged unless you answered at fix"
never = "merge"
unless = "person answered at fix"
```

Field names map onto `StepDecl`, `StopDecl`, `CheckDecl` and `Reach` in `engine/crates/core/src/manifest.rs`. The
builder may rename a key for plain reading (`by` for `executor`, `ask` for `about`), but every field of the
manifest must have exactly one spelling, and the mapping is written in one place.

### 3. Instructions are prose; Jaynes wraps the data

Instructions are what the agent reads: free text, in the style of Claude Code skills (Markdown), which models read
well and authors already know. Nothing is enforced from them; everything enforced is in TOML. They go inline
(`instructions = """..."""`, the default) or in a file (`instructions = "fix.md"`) when long.

Placeholders: `{{request}}`, `{{<step>}}` (that step's whole output) and `{{<step>.<field>}}`. A placeholder may
name only what the step may see: the request and the outputs of the steps it needs, never a hidden step's output
for an agent step. `jaynes check` refuses any other.

When Jaynes fills a placeholder it wraps the data in tags, so the model tells instruction from data and data cannot
pass for instruction (Anthropic's guidance on prompts recommends XML tags for this):

```
<request>
greet.sh should say hi instead of hello
</request>
<output step="runner">{"command": "make test"}</output>
```

### 4. Checks come from a menu; a script for the rare one

Jaynes ships a menu of checks with plain names. Each is written once, in the box (`engine/python/jaynes_box.py`)
where it runs project commands. A first menu, from what the two roles do today:

| Name | Kind | Holds the output to |
|---|---|---|
| `find-suite` | a Jaynes step | finds the command that runs the project's suite |
| `suite-passes` | open | the whole suite passes, run without internet |
| `tests-fail-for-the-bug` | open | the named tests fail, and for the bug's reason (fix-bug's `fails-for-the-bug`) |
| `tests-pass` | open | the named tests pass, then the whole suite (fix-bug's `fixed`) |
| `hidden-cases-pass` | hidden | the cases of a hidden step pass (fix-bug's `proof`) |
| `field-required`, `field-max-length` | open | a field of the output exists, or is at most N characters |
| `files-within` | open | the change touches only files matching the step's write globs |

A check the menu lacks is a script: `{ script = "checks/odd.sh", kind = "open", about = "..." }`. It runs in the
box, reads one JSON object on standard input (`output`, `given`, `project`), and exits 0 to pass, 1 to fail with
its reasons on standard output, 3 when it cannot judge. A script judges an output. It cannot change the order of
steps or what a step sees, so the promises stay provable.

Which checks fix-bug needs beyond the menu decides how big the menu grows. Port it before adding names.

### 5. Promises, proved before a run

A role may declare promises about its own method. Jaynes proves them when the role is checked, installed and
listed in the index, by exploring every way a run of the role can go:

- It runs the real core (`jaynes_core::decide`, pure: the same state and input always give the same decision).
- From each state it feeds every outside event the run could meet: an agent attempt that delivers (a small or a
  large change, since a large one raises the risk), fails, says it cannot be done, or is cut off; each check
  passing, failing or unable to judge; the person keeping, sending back or dropping; an agent releasing a stop
  marked `release`; a crash at any point (`Input::Recovered`); a run starting at low or high risk; a sampled stop
  asking or skipping.
- A state already visited is not explored again. Bounds keep the search finite: the role's attempts and fixes,
  and at most 3 send-backs per step.
- At each step it checks every promise. A promise that breaks stops the search with the path that broke it.

Only outcomes are explored, never the content of an agent's work: promises are about the process (who sees what,
who decides, what runs before what), never about quality.

Three forms, no more until a real role needs another:

| Form | Example |
|---|---|
| `never = EVENT`, `unless = CONDITION` | nothing merges unless the person answered at fix |
| `only_after = CONDITION`, for an `EVENT` | fix starts only after reproduce's checks passed |
| `agent_of = STEP`, `never_sees = STEP` | the agent of fix never sees cases |

Events: `merge`, `deliver`, `step <name> starts`. Conditions: `person answered at <step>`, `check <name> of
<step> passed`, `step <step> done`.

What the engine already guarantees for every role (`docs/guarantees.md`, G1 to G17) is proved for free. For
example, an agent never sees a hidden step's output (G3, `decide.rs` `given`). The value is in properties specific
to the method. A real one: small-change, at low risk, delivers without asking anyone, so the promise "nothing is
delivered unless the person answered at change" fails for it, with the path low risk → change delivers → checks
pass → the stop asks only from high risk → deliver.

Output of `jaynes check` for a failing promise:

```
✘ "Nothing is merged unless you answered at fix": fails
   low risk → fix delivers → checks pass → fix's stop asks only from high risk → merge, with no answer of yours
```

and for a passing one: `✔ "...": proved (18,402 states)`.

Where it lives: a pure module beside the core (it only calls `decide`), so it runs in the CLI, in tests and in
the index's CI. The random simulator in `engine/crates/core/tests/guarantees.rs` (`World`) already feeds the core
these kinds of events and is the model to start from.

### 6. Where roles live and how they are shared

- Each role lives in its author's git repository. A role's identity is its content: the fingerprint of its folder
  (`RoleProgram::fingerprint`), kept with the commit in `.jaynes-origin` since a766c9a.
- An index in a git repository lists roles: name, source, commit, fingerprint, and the promises proved. The
  index's CI proves each entry again, so "proved" cannot be claimed falsely; `jaynes add` proves again locally.
- `jaynes add NAME` resolves through the index; `jaynes add URL#path` keeps working, for private roles (Helm's)
  and for roles not listed.
- `jaynes update` already holds back a role whose new version reaches more (a766c9a). It should also hold back
  one whose promises were dropped or changed, and say which.
- The official roles (small-change, fix-bug, then build-feature) move out of the engine's repository into a
  roles repository of their own, which also holds the index, so a role has its own version and an engine update
  never touches it.
- Later, and only with consent, the index may show each stop's measured accuracy (G17 already counts it per run):
  "this stop is right 94% of the time over 210 runs". No package registry shows how often a package is right.

A hosted registry (accounts, publishing, search) waits until there are outside authors; the index grows into it.

### 7. How a role is made

- `jaynes new NAME` writes a starting folder with comments.
- `jaynes check DIR` reports every error with its line, and every promise proved or broken with its path.
- The main way: the person describes the method to an agent in Claude Code; the agent writes the role, runs
  `jaynes check`, and fixes what it reports until it passes. The agent writes, the check decides. The Jaynes
  plugin's skill (`skills/jaynes/SKILL.md`) gains this flow.

## What this does not change

The engine and its protocol (`engine/crates/shell/src/protocol.rs`, version 1) stay as they are. Jaynes ships one
program that reads any `role.toml` and speaks the protocol for it (describe, prompt, execute, check, items), so
the engine still talks to every role only through the protocol, as the owner decided in the redesign brief ("aceito
a fronteira"). The guarantees page does not change.

## Handoff: building it

For the session that builds this. Read this document, then `docs/guarantees.md`, then the code named above.

### Rules of the work

- Build directly, test first. Never use Jaynes runs to code Jaynes.
- `docs/guarantees.md` is the owner's: never edit it.
- After each change: `cargo test` and `cargo clippy --all-targets -- -D warnings` in `engine/`; GitHub CI
  (`.github/workflows/engine.yml`) must pass after the push.
- After pushing: `jaynes update`, restart the page (`pkill -f "jaynes page --port 8750"`, then
  `JAYNES_NO_BROWSER=1 nohup jaynes page --port 8750 > ~/.jaynes/engine/page.log 2>&1 &`), and look at the page
  in Chrome when it changed.
- Commit messages in plain English, no AI attribution, no em dashes.
- Talk to the owner in Portuguese, short, plain words first.

### Order

1. **The reader.** `role.toml` read strictly into `Manifest`; instructions inline or from a file; placeholders
   filled and wrapped in tags; the program that speaks the protocol for a TOML role; `jaynes check DIR` with
   errors and line numbers. Done when small-change, ported to TOML, passes the same end-to-end tests
   (`engine/crates/shell/tests/e2e.rs`) as its Python version.
2. **The menu and scripts.** The checks of section 4 and script checks in the box. Done when fix-bug, ported to
   TOML, passes its end-to-end tests, including the hidden cases and the merge with Undo.
3. **Promises.** The search of section 5 and its output in `jaynes check` and `jaynes add`. Tests: the failing
   small-change promise above, with that path; a role whose merge waits for the person, proved; a role whose stop
   asks only from high risk, refused at `jaynes add`.
4. **Making roles.** `jaynes new`, and the plugin skill's flow for writing a role with `jaynes check`.
5. **The index.** A roles repository with the official roles and the index, its CI proving each entry, and
   `jaynes add NAME`. Ask the owner first whether that repository is public or private.
6. **build-feature, measured.** The shortest method that holds: understand, make with the tests, deliver. Add a
   step only when the scoreboard shows it prevents an error. Measured by replay: real feature commits from the
   owner's repositories, undone in throwaway clones, redone by the role, and compared with the original commit
   (tests pass, size of the change, time, tokens). The owner sees only the scoreboard. Ask the owner which
   repositories to use before the first run; tenure holds employer data, so ask about it explicitly.

### Open, for the owner when the work reaches them

- Whether the roles repository is public.
- Which repositories the build-feature replay may use.
- Whether the index shows measured accuracy, and how people consent to sharing it.
