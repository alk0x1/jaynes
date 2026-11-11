# Runs and stops

A run is one task carried out by one role. Jaynes gives Claude Code one step of the role at a time, checks what the step made by code, and starts the next step only when the check passes. The agent can write that step's output and nothing else.

## A run works on copies

Each step works on a copy of your project's last commit, so uncommitted changes are not in the run, and your checkout is untouched while it works. When the run delivers, the accepted changes are a branch, `jaynes/<run>`.

Jaynes always runs the agent on the same model: Claude Opus at high effort, unless you set `JAYNES_CLAUDE_MODEL` or `JAYNES_CLAUDE_EFFORT`. A turn another model answers does not count. A role can run Codex instead, with `agent="codex"`.

## Stops

A stop shows a question and the evidence behind it: the step's output, the checks it passed, pictures of the screen when there is a preview, and what the agent spent.

Most stops do not hold the run. It goes on with the agent's choice and delivers its branch, and your answer, given later, corrects on top: only the steps that used what changed run again. These wait for your answer instead:

- a step that does something that cannot be undone, such as sending or paying, waits for every stop before it;
- a change to a file that runs outside the box (see [Files that run later](#files-that-run-later));
- an output that would end the run early, such as fix-bug finding no bug a test can show;
- a stop the role marks as one that waits for your choice, such as a look among three;
- a stop that was wrong too often lately (see [A stop asks less as it proves right](#a-stop-asks-less-as-it-proves-right)).

A stop the role marks as a floor asks on every run and only you answer it, but the run goes on meanwhile, unless it is one of the above.

## Your answers

| Answer | What happens | How it counts |
|---|---|---|
| Accept | The run goes on with the output as it is | Right, once the delivery holds |
| Change | Your edited output takes the agent's place, and is checked again | An error |
| Return, with a note | The step runs again with your note, on top of its output; or an earlier step you pick does, with every step after it | An error |
| Context, with a note | The step runs again with what the agent could not have known | Not counted |
| Choose | The items you picked, in your order, take the place of the output | Right, once the delivery holds |

A return at a step Jaynes runs itself, such as one that only lays out the agent's work for you to choose, goes to the agent's step before it, since only an agent reads your note. When that step reviews another one, such as a director's critique of a designer's work, the return goes to the work it reviews, and the review runs again after it. What you say at one step reaches every later step of the run too: ask for dark mode once, and the steps after it know.

Where a step shows its output as decisions, you reject only the line that is wrong, with a sentence; Accept becomes Send, only that line is redone, and the rest stays locked against the agent. Change on a locked line opens it again.

Under any stop, Why? asks a Claude that reads the step, what it was told, what it delivered, what was said in the run, and answers in a few sentences. It cannot edit, run commands or read the web, and it changes nothing in the run.

In Claude Code's dialog you Accept or Return with a note. The page has all five. At a floor, the A key accepts only when you press it twice within two seconds, so a stray key does not let it through.

Only you answer as a person. The page takes answers only from the browser it opened with its link. An agent's answer, through `jaynes answer` or the API, lets a stop that holds its run go on, but it never counts as right, so an agent cannot make a stop ask less by accepting its own work.

## A stop asks less as it proves right

An accept counts as right once its delivery went 7 days with nobody correcting it. A change or a return counts as an error at once, and so does a correction found later.

A stop that keeps proving right becomes sampled: it asks one time in ten, at random. With a budget of 20%, the error rate the stop may have, that takes 20 clean deliveries in a row; with 10%, 43; with 5%, 88. If the true rate at which you would change the stop's output is at or above its budget, the chance it ever becomes sampled is at most 5%.

Two errors among a stop's last ten outcomes brake it: it asks on every run, and the run waits for you there, until the deliveries after the brake prove it right again.

## Merging by itself

build-feature and fix-bug prove their work with checks written from your request alone, before the work, which the agent that makes the change never sees. Jaynes runs them against the changed project. When every one passes, Jaynes merges the run's branch into the branch you started from, only over a clean checkout, and never pushes. A return merges its correction on top. Undo, on the run's page, reverts the merge.

**Dismiss**, at the bottom of any run's page, ends a run that has not finished: what it has not delivered is dropped and its open questions close. If the run is working, it ends when the step under way ends, and nothing after it runs. What it already merged stays; Undo reverts that. In a plan, the runs after it do not start.

**Halt all**, in the page's sidebar, stops every run before its next step, and every merge, until you press Resume. A turn already under way finishes first.

## Corrections after delivery

When you ask your agent to fix something a run delivered, the agent first records it with `jaynes correct`: the run, the step whose output was wrong, and the kind of wrong.

- **error**: wrong by what was already known: the task, the code, or a preference already written down.
- **preference**: a new preference nobody had written.
- **info**: the world changed, or a new fact appeared.

Only an error counts against the stop that let it through. You can change the kind on the run's page.

## Plans

A request that needs more than one role, such as a new look and then a screen in it, becomes a plan of runs. Each run starts once the runs it builds on delivered, from where their delivery landed, and the page shows the whole plan. A role can plan a request too: Jaynes checks the plan it delivers, and starts it by itself. On the request's page, **Drop** takes a run out of the plan before it starts, when no other run builds on it; the request then delivers without it. See [Write a role](write-a-role.md#a-role-that-plans).

## Previews

With a preview command, such as `npm run dev -- --port {port}`, each stop shows the project before and after the change. The agent tells Jaynes the command when your project has a dev server; on New run it is a field.

The preview runs the agent's changes, so on macOS it runs in a box: it writes only to its copy, cannot read where secrets live, and reaches no network but the local ports you list, such as a backend it needs.

## The box

Your test suite also runs in the box. It reaches no internet, only local ports, so tests that start servers or use a local database work. The suite must pass at your last commit with its dependencies already installed (`node_modules`, `.venv`).

The box needs macOS. Elsewhere, previews and test runs refuse to start, unless you accept running them unboxed with `JAYNES_UNBOXED=1`.

## Files that run later

Some files run later, outside any box: agent and editor settings (`.claude`, `.vscode`, `CLAUDE.md`), git attributes and hooks, `.envrc`, `package.json`, `pyproject.toml` and CI workflows. A step that changes the project lists what it may change (`may_change` in the role). Even then, a change to one of these files is refused in a step without a stop, and turns a step's stop into a floor that shows you the file's diff.
