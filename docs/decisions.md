# Decisions

What was decided, what it costs, and what is still open. Newest first.

## 2026-11-11: A stop shows decisions, not text; an interview comes first

**Decided.** A run starts with one interview: questions with 3 to 5 options, a
pre-marked guess and a free text field, answered once on one screen. A stop
shows the output as decisions with fixed ids, each accepted or rejected with a
note; accepted ones are locked against the agent, checked by code, and only the
rejected ones are redone. "Why?" explains a step and never changes it. Design
in docs/specs/2026-11-11-decisions-at-stops-design.md.

- Why: on a real run the person disagreed with one line and Return redid
  everything, twice, until two specialists deadlocked on that line.
- Ids, not names: the agent may add decisions without a cap, and an id lets
  Jaynes follow one across rounds even when its wording changes.
- A lock binds the agent, never the person: "change" unlocks it, and the
  decisions that depend on it unlock with it.
- One place: the page. Mirroring stops in the terminal doubles what to keep.
- Open: how many questions an interview should hold in practice; we start at
  3 to 5 per screen and read rounds until accepted and minutes per stop.

## 2026-11-11: Specialists talk inside a role, step by step; a return goes to the work, past its review

**Decided.** A role can be a conversation between several specialists of a
team: each step is done by one of them, named with `by` (the page shows it in
place of "agent"), the next step receives what it made, and a step that reviews
sends it back with what it found, a bounded number of times. Every exchange is
recorded and checked like any other step; the person answers at the stops.

- Why: Alan asked how a product's look is born in a company with several
  professionals, and how that becomes standard. A strategist writes the brand
  contract, a designer explores, a director rejects the weak work before the
  owner sees it, the owner chooses, a design system engineer makes the tokens.
  The director has to send the designer's work back, which Jaynes does between
  steps of one run, not between runs of a plan.
- What is standard lives with the team, in code: the roles that hold the
  conversation, and the plan step that refuses a plan which leaves a specialist
  out. Jaynes gives the mechanism; the team's package gives who and when.
- A return with no step named, at a step Jaynes runs, used to go to the latest
  agent step it needs. When that step reviews another, the return now goes to
  the work it reviews, since the person's note is about the work, and the
  review runs again after it.
- What the person said at one step reaches every later step's prompt, as "what the person said earlier in
  this run". Found when a strategist added to a running identity wrote a light theme: Alan had asked for dark
  mode at the designer's step, and only that step was told.
- Open: who is on the team is a label on the step; Jaynes does not yet keep a
  record per specialist across runs.

## 2026-11-11: Each step tells the person what it does, and a stop shows where it is

**Decided.** A step has an `about`: one sentence for the person, saying what
the step does and, at a stop, what they decide there and what it changes. A
stop shows it under its question, then the run's steps in one line, with the
runs of the plan that wait for this one; pointing at a step says what it does,
and a click opens every step with who does it (agent, Jaynes, you). The choice
stays in front: the first version put all of that above it, and Alan found the
page a wall of text. The run's page and the role's page show the same steps.

- Why: at a real choice of looks, Alan did not know that only the colours and
  fonts were kept, nor that each screen is designed later in its own run. He
  disliked the sample screens and could not tell whether that mattered. The
  page showed the steps' names only, folded away.
- `about` is not given to the agent and is in no fingerprint: it can be written
  or changed without opening a new window of answers.
- A role without it still works; its steps show their names, and the stop says
  the role does not tell what its steps do.
- Open: `jaynes check` does not yet ask for an `about` on a stop.

## 2026-11-10: A role gives its agents tools, which Jaynes runs and records

**Decided.** A step can offer tools of its role: functions the agent calls while
it works, through an MCP server Jaynes starts for the turn. Jaynes runs each
call in its own process, outside the agent's box, and records it with the
turn: the tool, its arguments, whether it worked, what it said, and the files
it left. The pictures a tool returns reach the agent as images and stay among
the step's files, so the stop shows the person what the agent saw.

- Why: Alan asked what happens when he sends a reference link. The agent saw
  only the address: its box has no network and no browser. Doing every capture
  in a step Jaynes runs before the agent was the first idea, but a return with
  new links goes straight to the agent and skips that step. A tool works
  wherever the agent meets a link, in any role.
- Jaynes's browser gets `photograph`: a picture of a page on the public
  internet. Every request the page makes goes on only to public addresses, so a
  tool cannot reach this computer, its network or Jaynes's own page.
- A picture a stop shows can carry the page it was drawn from. The page link
  opens it in a sandbox with no script, no form and nothing loaded from
  elsewhere, since an agent wrote it.
- Only the Claude runtime offers tools. A step with tools on another runtime
  is refused, as a step that reads the web is.

**Open.** A check cannot yet see the calls: "every link the person named was
looked at" is an instruction, not a check.

## 2026-11-10: Choices of taste wait for the person; a plan names its specialists

**Decided.** A role can mark a stop as one that waits: `stop(budget,
waits=True)`. The run holds there until the person answers; it always asks,
and only the person answers. Every other stop still shows its output and lets
the run go on.

- Why: since stops stopped holding runs (2026-11-08), a role had no way to
  wait for a choice that is the person's to make. On a real project the identity
  applied a look Alan never saw and the screen design went on with moments and
  requirements the agent picked. Taste is an input, not a check.
- A plan's run may name the `specialist` who does it; the request page shows
  it. Specialists stay outside Jaynes, in the private repository; Jaynes only
  carries the name.
- The flag is left out of the stop's repr, so no role's fingerprint, and no
  stop's count, changes.

**Open.** Where the roles live. For now the new roles are private, in the
private repository's package; maybe public later. Alan, same day: the
commercial plan may become an open engine with a few usable roles, more roles
hosted in a registry as npm does, and the private specialists powered by
Jaynes as what he sells.

## 2026-11-10: Dismiss on every run, also while it works

**Decided.** Every unfinished run's page has Dismiss, not only idle and
blocked ones. A run no process holds ends at once; a run a process works on
ends when the step under way ends, and nothing after it runs. The turn under
way is not killed, so its workspace and log stay whole. Dismiss ends the run,
not only the person's review: open questions close, what was merged stays
until Undo, and in a plan the runs after it do not start. Still the person's
alone.

Why: Alan had runs of work projects he no longer wanted, and the button was
only where a run had stopped by itself.

## 2026-11-10: The person drops a run of a plan before it starts

**Decided.** The first piece of changing a plan once it started: on the
request page, Drop takes out a run that has not started and that no other run
builds on. It never starts, the request delivers without it, and the drop is an
event of the request (`RequestRunDropped`). Only the person drops, as only the
person dismisses a run; an agent's request is refused. A run another builds on
stays until that one is dropped first. Adding a run or changing one is still
open.

Why now: the direction planned a screen design of `/health` for a real project,
a page only Hangar reads, and Alan agreed it should go. Until now the only way
out was to dismiss it once it started.

## 2026-11-10: The docs live on jaynes.dev/docs; packages need no terminal

**Decided.** People rarely type Jaynes's commands: their agent does. So the
docs speak to two readers: the person, with what to say to the agent and where
to click on the page, and the agent, with every command in full.

- The docs are six pages on the site, `jaynes.dev/docs`: Get started, Runs and
  stops, Roles that come with Jaynes, Packages, Write a role, and Commands.
  Their sources are Markdown in `docs/manual/`; `docs/manual/build.py` writes
  `site/docs/`, and a test fails when the site differs from what it builds.
- The Commands page is built from the help `jaynes` itself prints, so it never
  says what a command no longer does, and a new command fails the test until
  it has a place there. Every command and option got a help text for it.
- The README keeps what Jaynes is, the guarantee, the status and the roadmap;
  how to use it moved to the docs, so there is one place to keep up.
- Packages need no terminal: the page's Roles shows what is installed, which
  projects use which, and adds or removes one: for the project the person
  names, never filled in, or for every project only when they tick it. An agent does the same through the API and the MCP
  tools `packages`, `add_package` and `remove_package`, only when the person
  asks, since a package's roles run code on their computer.
- Costs: one dev dependency, `markdown-it-py`, only to build the docs; the
  pages are static files, so the site still publishes as it did.

## 2026-11-10: Roles are packages, as libraries are for npm

**Decided.** The engine brings no role of its own. Every set of roles is a
package: a folder with `jaynes-package.toml`, which gives its name, version,
what it is for and the role API it is written against. `jaynes add` installs
one into the home and puts it in a project's list (`--project`) or in the
default list that projects without their own use; `jaynes remove` takes it out,
`jaynes packages` lists them, and `jaynes roles` lists the roles a project can
start by name, the package added last first.

- A package comes with Jaynes (`software` and `design`, both in the default
  list), or from a git repository, or from a folder of the person's, linked
  where it is so an edit applies at once, as `npm link` does.
- Every installed version keeps its own folder, and a run records the file it
  started with, so updating never changes a run under way. `jaynes update`
  brings packages from git to the newest commit they follow; the ones that
  come with Jaynes follow Jaynes.
- A package's Python files import each other relatively, so two versions of a
  package never mix in one process. What a role may import from Jaynes is the
  role API, listed in SPEC.md, version 1; a change that breaks a role bumps it,
  and Jaynes refuses a package written for another.
- A project's list lives in the person's home, never in the project, so a
  company's repository gets no file and never names a private package.
- Runs started before this find their role in the package that comes with
  Jaynes now.

**Why.** Alan asked for the roles outside the engine but tied to it, as npm
libraries are to Node. It also answers which specialists work where: a
project of a company the person works for uses only the public software
roles, and the person's own projects add their private team.

**Tradeoffs.**

- The role API is now a promise: changing it breaks other people's packages,
  as changing Node's API breaks libraries.
- The packages that come with Jaynes still live in its repository, under
  `packages/`, outside the engine's code and shipped with it, because their
  tests use the engine's test helpers and every engine change must be checked
  against them. A repository of their own can come when someone else
  publishes roles.
- A linked folder's Python runs when Jaynes lists the project's roles, so it
  must be the person's own.

**Built 2026-11-10.** The private direction and design roles are a linked
package of the private repository, used by the projects that list it.

## 2026-11-10: The person's own roles, and a role that plans

**Decided.** A person keeps their own roles, such as a company's private
specialists, outside Jaynes, and `jaynes roles` lists every role a run can
start by name, each with a sentence on what it is for, which a role now says
(`about`). A run records the role's file by its full path, so it continues
from any folder. **Changed the same day:** the folder became a package; see
"Roles are packages, as libraries are for npm".

A role plans a request when a step of it delivers a plan (`jaynes.plan.Plan`):
the runs, each with a role by its name, a task and the runs it builds on, and
why these roles. That step is shown the roles it can use, never another role
that plans, and Jaynes checks its plan as it checks a plan from the person's
agent, sending the step back with the reasons. Once the run delivered, Jaynes
records the request in the run's own task, the person's words, on the run's
project, with the agents the run was given, and starts it. The guide runs such
a role for every request that is more than a small change.

**Why.** The direction specialist decided on 2026-11-07 is private, so Jaynes
had to find roles that live outside it; and the plan is the specialist's
delivery, which only the engine can start, check and count.

**Tradeoffs.**

- Every request but a small one costs one more run, a minute or less.
- The plan's sample counts like any other: a return or a correction against
  its step is the direction's error. A return makes it plan again, but the new
  plan does not replace the one already started; changing a plan once it
  started is still not built.
- A Python file in an added folder runs when Jaynes lists the roles, so the
  folder must be the person's own.

**Built 2026-11-10.** The direction and the design roles (visual-identity,
redesign, screen-design) now live in the private repository and improve there;
their copies in Jaynes stay as examples. A real direction run with Claude Code
planned a request on a test landing page in 44 seconds: a dark look with
visual-identity, then the waitlist form with screen-design after it, with the
design method as its input, and said why build-feature was not needed.

## 2026-11-10: The page does not start at login

**Decided.** The Mac does not start the page when the person logs in. The page
keeps one address, http://127.0.0.1:8740/, which the person bookmarks; any run
or plan starts it, and `jaynes` alone in a terminal starts it if needed and
opens it.

**Why.** Alan never knew how to reach the page. A launch agent that started it
at login was built, and Alan turned it down: he wants the page easy to find,
not running by itself.

**Tradeoffs.**

- After a restart the address answers only once a run starts or the person
  runs `jaynes`; a run paused by a usage limit waits until then.

## 2026-11-09: Agents settle a wrong test between themselves

**Decided.** When the agent writing the code says a test it may not change is
wrong, Jaynes sends the tests back to the agent that wrote them, with what it
said, up to twice in a run. The person is called only when the two still
disagree, and then the page says so in plain words with two buttons: Fix the
test, or Keep the test.

**Why.** The run blocked and showed the person the agent's own words, full of
test names and paths, and asked them to write a note and pick a step. Alan
answered that it was hard to read and not what he wanted: on the loop, a
dispute between agents is theirs to settle first.

**Tradeoffs.**

- The agent writing the tests could give in to a wrong claim. The agent
  writing the code still never changes the tests, and the hidden cases still
  prove the result.
- Each round runs the tests and the code again, which costs time and tokens.

**Built 2026-11-09.**

## 2026-11-09: A guard against the irreversible comes with the plugin

**Decided.**

- Installing the plugin turns on a guard in every Claude Code session, not
  only in Jaynes runs. It comes in the plugin's own hooks and updates with it,
  so Jaynes never touches the person's settings. A switch in Setup turns it off.
- It holds only what cannot be undone in git and GitHub: push, reset --hard,
  git clean, branch -D, tag -d, creating, merging or closing a pull request,
  and creating, deleting or archiving a repository. Publishing a package,
  deploying and sending messages stay out for now.
- It asks instead of blocking: Claude Code's permission prompt opens and the
  person approves with one key. Where nobody can answer, as in a Jaynes run,
  the question becomes a block.
- Repositories owned by the person's GitHub user pass freely; Jaynes finds the
  user with gh. Each person sets their own list of owners in Setup, since
  Jaynes serves anyone.
- It stops the agent from editing its own files.
- The full action gate stays optional in Setup; the guard is the floor that
  always applies.

**Tradeoffs.**

- An agent can still change the guard by a path it does not catch, such as a
  script. The real lock is the system's, which only the person sets.
- When hooks disagree, the stricter one wins, so a hand-written hook that
  blocks the same actions hides the guard's question and has to go.

**Built 2026-11-09.** `hooks/guard.py`, a standard-library script the
plugin's hooks run before every shell command and edit. The person's GitHub
user is found once with gh and saved in `~/.jaynes/guard.json` with the
switch; Setup shows the guard first, with the switch and the free owners.

## 2026-11-09: The software roles stay public and keep improving

**Decided.** The roles that build software (build-feature, fix-bug and
small-change) stay in Jaynes and keep getting better there. The other
specialists, such as the design roles (visual-identity, screen-design and
redesign), still move to the private repository, and their copies in Jaynes
stay as examples at their current level. This changes the 2026-11-07 decision
for the software roles only.

**Why.** Whoever installs Jaynes gets software roles that do real work on day
one instead of examples, and these roles are where the engine's new parts, such
as the hidden cases and the merge alone, are proven.

**Changed 2026-11-10:** they stay public and keep improving, in the package
`software` that comes with Jaynes, outside the engine's code.

**Tradeoffs.**

- The best software roles are public, so anyone can copy them.
- What a role learns from a real project stays in that project, never in the
  role, since the repository is public.

## 2026-11-07: A safe real target, and where a step's error counts

Found by walking an invented request end to end: reminders by message one day
before a class, in a booking app.

**Decided.**

- In many areas the real run cannot be undone, such as sending a message or
  charging a card. Each project keeps a safe real target, such as the person's
  own number, a test account or a payment service's test mode. The role says
  it needs one; the project says which. The real run then becomes safe, and
  only switching on for real customers waits for the person.
- Counts exist only on stops, but an error can come from a step without one.
  The error counts on the first stop after the step that erred, because that
  stop let it through.

**Tradeoffs.**

- A test target can behave unlike the real one, such as a test account with
  limits the real one does not have.
- A stop late in the role collects the errors of every step before it, so its
  count falls for work it did not do.

## 2026-11-07: An API and an MCP server, and what an agent may release

**Decided.** Jaynes gets a local API with every action of its page,
and its page becomes one client of it; another frontend, such as the private
company's panel, uses the same API. An MCP server offers the same actions to
agents. An agent can be the first to handle an exception, and the person gets
only what it may not release.

- An agent may resolve two attempts that still fail, by trying another way,
  calling another specialist or rewriting the request, and a doubt.
- Only the person may release an action that cannot be undone and a braked
  specialist, and only the person answers samples: an agent's answer never
  counts as proof. Jaynes already records whether the person or an agent
  answered.

**Tradeoffs.**

- A frontend acting for the person holds the person's key, and agents on the
  same computer could read it, so the key must live where they cannot reach,
  such as the browser or the system keychain.
- An agent resolving exceptions can hide a pattern the person would have
  noticed; its resolutions show in the results.

**Built 2026-11-09.** The page's server answers `/api/` with every action of
the page as JSON, and the page's buttons go through the same functions. A
request carrying the page key's cookie is the person's, any other an agent's.
`jaynes mcp` offers the reads and the agent's actions over MCP, and the plugin
starts it in Claude Code. An agent answers a stop that holds its run, sends a
blocked run back with a note, continues a run or starts one; a sample, a floor,
a braked stop, dropping a run, filing a correction as another kind, Undo and
Resume answer it 403. Calling another specialist waits for the specialists,
and the person's key for another frontend still lives in a file the person's
agents could read, not yet in the keychain.

## 2026-11-07: What is Jaynes and what is the company built on it

**Decided.** A second test, next to engine against role: would someone who
installed Jaynes without the private company need it? If so, it is Jaynes.

- Jaynes's page shows what waits for the person, the samples to answer, the
  deliveries with their proof, what is running and the plan of runs of each
  request, with buttons to halt everything and to undo. All of it follows from
  the engine's ideas, and without it nobody else could use Jaynes.
- Running a company belongs to the company: which projects exist and which
  comes first, which requests to create, business results, the direction
  specialist and the rest of the team, and deciding what is worth telling the
  person and when.
- Jaynes warns the person outside the page only when something is held.

**Built 2026-11-09.** The page's home has the four parts: what holds a run (a stop a step left still needs, a stop that says it holds, a blocked or idle run), the samples (a stop the run went past, or one that only lets it finish), what was delivered since the person's last visit, and what runs now. A visit is their looks at the home page less than half an hour apart. The browser opens by itself, and the tab counts and notifies, only for what holds a run; `/next` opens what to answer first. The plan of runs of each request waits for runs that make plans.

## 2026-11-07: Waiting is the brake, not the default

**Decided.** When a stop's count shows too many errors, its runs wait for the
person's answer again, until the count recovers. Braking needs little proof,
such as two errors in the last ten runs, because braking without need only
costs waiting. Releasing the brake needs the statistical proof Jaynes uses
today, that the error rate is under the stop's budget. Other specialists go on as
before. The page gets a button to halt everything, for when the person wants to
stop without waiting for the count. Undoing a delivery uses the area's own
undo, which comes from the role, such as reverting the merge in software; where
the area cannot undo, the action already waited for the person.

**Why.** A merge can be undone, so it does not wait either. Without a brake, a
specialist that errs often would keep delivering and merging while its work
passes the checks, and showing a stop on the page stops nothing.

**Built 2026-11-08.** Halt all, in the page's sidebar, stops every run before
its next step and every merge; anyone may halt, an agent too with `jaynes
halt`, and only the person resumes. A turn already under way finishes, and
what it made waits. Merging alone was built the same day, with Alan's choice that it happens as
soon as the proofs pass, not only once the merge stop has proved itself. A
role says its delivery is a merge; Jaynes merges into the branch the project
had checked out when the run started, locally, never pushing, once every
hidden check passed and nothing holds the run. It lands only on a clean
checkout, and a conflict or uncommitted changes leave it to the person, said
on the run's page. A return after the merge corrects on top and merges only
what changed. Undo on the run's page reverts every merge of the run, with a
note. Two errors among a stop's last ten outcomes brake it. A braked stop
asks on every run, even for a small change and when certified, holds the run
there, and refuses an agent's answer. It is released when the outcomes since
the brake certify on their own; the errors before the release no longer count
towards the next brake, so a stop with a loose budget does not brake again at
once. While a stop holds the run, a later stop whose work was built on what was
since returned closes, and its step runs again after the answer, so the page
never offers work on top of something already sent back.

**Tradeoffs.**

- The statistical test alone brakes only on errors in a row: two in a row at a
  5% or 10% budget, three at 20%, while three errors in ten runs would not
  brake. The looser rule to brake covers that, and can brake a stop that was
  fine.
- Releasing takes at least 7 days, since only a delivery that held counts as
  right, and as many clean deliveries as certifying takes from zero.
- Until it brakes, a new specialist's errors reach the main line and have to
  be undone.

## 2026-11-07: A direction specialist plans each request

**Decided.** A request becomes a plan of runs, one per role. The delivery of one
run enters the next, with context chosen by address as decided; runs that do
not depend on each other run at the same time; the record ties them all to the
request. That much is the engine's. A direction specialist, private like the
others, reads the request and picks which specialists take part and in what
order. The plan does not wait for the person, who sees it in the result. The
direction has its own count: calling the wrong specialist or leaving one out is
its error.

**Why.** A real request often needs several specialists, such as a product
designer, a writer and engineering for one screen, and today one agent picks a
single role.

**Tradeoffs.**

- A wrong plan sends every later run the wrong way until someone sees it.
- One more agent per request costs tokens.

**Built 2026-11-10, the engine's part.** `jaynes plan` takes the request in the
person's words and its runs, each with a role, a task and the runs it builds
on, and Jaynes checks the plan before anything starts. Until the direction
specialist exists, the agent in Claude Code writes the plan, as the guide
says. Rules that came up while building it:

- A run starts once every run it builds on delivered: merged, on its branch
  when it does not merge or could not, or with its outputs when it has no
  project. It never waits for their samples, which correct on top later.
- It works from where the delivery landed: the person's branch once the
  earlier run merged, or the earlier run's branch when it did not. A run can
  build on at most one run whose delivery is a branch, since it works from one
  commit.
- Its task carries the request in the person's words and, for each earlier
  run, where it landed and the files it changed, or its outputs when it has
  no project.
- A run dropped, ended by its output or undone stops the runs after it.
- The run that delivers starts the next ones; the page looks again every 15
  seconds, so a plan goes on when that process dies, and starts again a run
  that has not recorded its start 60 seconds after it was started.
- The page shows each request with its plan, and the home lists the requests
  whose next runs wait for earlier ones.

Not built yet: starting a plan from the page or the API, and changing a plan
once it started. The direction specialist and its count came the same day, in
"The person's own roles, and a role that plans".

## 2026-11-07: A stop no longer holds the run

**Decided.** A run does not wait for the person's answer. It goes on with the
agent's best choice, and the answer can arrive later. An accept changes
nothing; a return corrects on top of the work, and after delivery it counts as
a correction. Only an action that cannot be undone, such as
publishing, sending or spending, waits for the person. "Always asks" now means
the stop is always shown, not that the run waits.

**Why.** The levels help over time, but a new project with a new specialist
starts with every stop at "always asks", so the start would stay as slow as
today. On the measured project, 67 of 71 hours were waits, mostly while the
person was away, not on hard questions.

A return corrects on top, not from scratch:

- The returned step gets its own earlier work and the note, and changes only
  what the note asks.
- A later step runs again only if what it received changed; the engine
  compares before and after. When it runs, it gets its earlier work and what
  changed before it, and adjusts.
- Every check runs again at the end, the hidden ones too.
- After delivery a return becomes a new correction on top of the delivery.
- The agent of the returned step may start that step from scratch, saying why,
  and the reason shows in the result.

**Built 2026-11-08,** except starting over from scratch with a reason, and a
return after delivery. A stop is shown and the run goes on to its branch; it
finishes once every stop is answered, so the merge stays with the person.
Each step records what it received, and only a step whose inputs changed runs
again, with its earlier output and its earlier change already applied. Answers
are recorded while the run runs. Still waiting for the person: a change to the
paths that run outside any box, an output that would end the run, and an
irreversible step. Two rules came up while building it:

- A stop with no output of its own passes on only the person's answer, so a
  step that needs it waits for that answer. The rest of the run does not. In
  screen-design, the step that turns the owner's notes into rules runs after
  the merge is answered, not before with no notes.
- At a stop where the person chooses among items, the run goes on with all of
  them in the agent's order, and runs again on the ones chosen. In
  screen-design that maps and walks every moment offered, then the chosen ones
  again.
- A run can now block at a later step while an earlier stop waits. When the
  person's answer there changes what the blocked step receives, it tries
  again by itself.

**Tradeoffs.**

- Correcting on top can leave leftovers of a wrong path when the error was at
  the base; starting over is allowed only with a stated reason.
- Running ahead of a choice costs tokens on what the person will drop.
- A misunderstood goal is built in full before anyone sees it.
- More results pile up for the person, so the queue and a way to halt and undo
  matter more.

## 2026-11-07: Proof the maker cannot see

**Decided.** Two parts, both before the delivery stop may rise a level.

- Hidden checks: a separate agent reads only the request, never the work, and
  writes the cases before the work starts. They sit in the engine's area of
  files no agent receives. The person sees them in the samples. In software
  they are usage scenarios; in marketing, what a piece must meet, such as the
  audience, the promise and what it must not say; in research, the questions
  the report must answer with a source.
- A real run, which each role defines for its area: the real command against
  the real service in software, the screen with real data in design. Where the
  real run cannot be undone, as spending money in marketing, it always asks and
  the proof comes from the outcome after delivery.

When a hidden check fails, the maker sees the case described in words, never
the check itself. After two attempts that still fail, it becomes an exception
and calls the person.

**Built 2026-11-08** for build-feature and fix-bug, with Alan's choice that a
case is a command that passes or fails, never an agent's judgment. The case
writer reads the task and the project as it was, before any step changes it,
and no agent step may receive what it wrote; the engine refuses such a role.
Jaynes serves the changed project with the run's preview in a box without
internet and runs each case there, so the box is the safe real target of a
role that reaches no real service. A failing case sends the maker back with
its sentence, twice at most; then the run goes on, the merge stop shows what
still fails, and nothing merges by itself. What the request asks that no
command can check, such as how a screen looks, is listed and proves nothing,
so such a run merges only with the person's accept. small-change has no
hidden cases yet: it has no stop to show a case that fails. A project's own
safe target, for a role that reaches a real service, waits for such a role.

**Tradeoffs.**

- Each failed attempt tells the maker a little about the hidden cases; two
  attempts limit how much.
- A case can be wrong, such as a word the request did not fix; it sends the
  maker back twice for nothing and then waits for the person.
- The case writer shares the model's blind spots, so the real run stays.
- One more agent per request costs tokens.

## 2026-11-07: Levels per stop, and when the count starts over

**Decided.** A stop has three levels:

1. Always asks: at the start, or after the count showed too many errors.
2. By sample: it has proven right and asks one time in ten, with a concrete
   question. Jaynes does this today.
3. Almost alone: it has proven right for much longer and asks one time in
   fifty, a number to tune. There is no level without samples, because some
   errors never become corrections: the person just lives with a mediocre
   result.

At levels 2 and 3 a stop also asks on doubt. The engine's signals hold in any
area: a check failed and the agent redid the work, the agent asked for
information it lacked, a reviewing agent disagrees, or the specialist works on
that project for the first time. A role adds the signals of its area, such as
touching files outside the agreed ones in software or spending above the usual
in marketing.

An action that cannot be undone always asks, at any level. The rule is the
engine's; each role names which actions of its area cannot be undone.

The count starts over when what the agent receives changes, such as the step's
text or its question. Adding a check does not start it over, because a check
only stops more work and does not make what passes worse.

**Why.** Today any change to a role's text, checks or question starts the
count over, so a specialist that improves every week would never earn trust.

**Tradeoffs.**

- A new check makes the agent retry, and a retry can turn out different, so
  keeping the count is a close approximation, not an exact guarantee.
- Doubt signals ask more often at the start of each project.

## 2026-11-07: Engine, role and specialist; corrections after delivery

**Decided.** The test for where something belongs: if it works the same for
marketing and for software, it is the engine; if it depends on the area, it is
the role.

- The engine runs the steps in order and gives each agent only its step,
  checks that each step delivered, runs the checks the role asked for, stops
  where the role says, records everything, counts right and wrong per stop and
  decides when a stop may ask less.
- A role is the method of one kind of work: its steps and their order, what
  each delivers, its checks, where it stops and with what question, and what a
  delivery is in that area.
- A specialist is several roles with knowledge, memory and a track record.

The engine offers slots and a role fills them with its own code, so someone
using Jaynes never has to change the engine, send it a change or keep a
modified copy. A role already decides, in its own code, a stop's question,
whether it asks at all, the options to choose and what it shows, as well as its
checks. Every "the role says" decided this day is a slot too: what a real run
is, how to find a corrected delivery, the doubt signals of its area, which
actions cannot be undone and how to undo. Where the engine cannot show
something, such as a sound or a video, the role sends its own view, a page the
Jaynes page shows in an isolated frame that cannot reach the person's key; the
answer still goes through the engine's buttons, so the engine alone records
right and wrong. The engine changes only when a slot that serves everyone is
missing, and that is work for whoever maintains Jaynes, not for its users.

When someone corrects a delivery afterwards, the count of the stop behind it
changes. That rule is the engine's. How to find which delivery a correction
touched is the role's: the code history in software, the screen in design, the
campaign in marketing.

A correction is one of three kinds:

- An error: it was wrong by what was already known, including a preference
  already written where the agent could read it. It counts as an error.
- A new preference: nobody knew. It does not count and becomes taste memory, of
  the project or general.
- New information: the world changed or a new fact appeared. It does not count.

The agent making the correction proposes the kind and which step it came from;
the person sees it in the result, with no stop. A delivery earns its right
answer after a fixed time per project, 7 days by default, and a correction
found later turns it into an error.

**Built 2026-11-08,** except the time per project, finding the delivery
from the code's history, and keeping a new preference as taste memory. An
accept counts as right only once its delivery held 7 days; a change or a
return counts as an error at once. The agent finds the delivery with `jaynes
delivered`, which lists the latest ones with their steps and files, and
records the correction with `jaynes correct`. The run's page shows it with
its kind, and the person can file it as another kind there. Three rules came
up while building it:

- Only the last release of a stop in a run counts, since only it was
  delivered.
- What an agent released, or a small change the role let through, never
  earns a right, since nobody looked; a correction still counts against it,
  so the count errs towards asking.
- A correction to a run still open is refused: there, a return from a stop
  corrects it.

**Tradeoffs.**

- The agent can file a real error as a new preference and keep the count too
  optimistic; the person sees the kind in the result, and samples catch part
  of it.
- A fixed wait ignores how much the delivery was used in that time.
- A stop now takes at least 7 days to become sampled, where it took one
  afternoon of accepts.
- Every project waits the same 7 days for now. A project file of Jaynes's own
  could set it; the `jaynes.toml` a project may have is the action gate's.

## 2026-11-07: Jaynes promises specialists of any area

**Decided.** The engine serves any area, and nothing in it may depend on code.
The public promise is open from the start: Jaynes runs specialists of any area,
not only software.

**Why.** The private company built on it needs every area, from marketing and
research to sales.

**Tradeoffs.**

- So far only software, and design for software, has been proven. In other
  areas the machine can check less by itself, so trust there rests mostly on
  the outcome after delivery.
- A promise wider than the proof can read as hype.

The positioning in one sentence: "An open engine for agent specialists in any
area. They work alone where they have proven right, and call you everywhere
else."

**Open.** How the page and the README show which areas have been proven.

## 2026-11-07: Jaynes is open, the specialists stay private for now

**Decided.** Jaynes is the open engine that anyone can use. The specialists
built on it, with their roles, checks, knowledge and memory, stay private for
now. Making it easy for anyone to create their own specialists through Jaynes
comes later.

**Tradeoffs.**

- Someone who installs Jaynes gets the engine and only the example roles, so
  they must build their own specialists before it does real work for them.
- Making specialist creation easy becomes a core part of Jaynes, not an extra.

The roles already inside Jaynes stay as public examples at their current
level. From now on specialist work happens in the private repository, and Jaynes
gets only engine improvements: the slots a role fills are public, the role
itself is private. Removing the roles was rejected, because the
repository history is public and removing them would not make them private.

**Changed 2026-11-09:** the software roles stay public and keep improving.

**Answered 2026-11-10.** Private specialists are a package the person adds to
the projects that use them with `jaynes add`; see "Roles are packages, as
libraries are for npm".

## 2026-11-07: What counts as proof that a step got it right

**Decided.** The main proof comes from outside: the real run passed and no
defect traced to that step appeared for a while afterwards. The person's answer
counts only when they actually looked, and is used for the samples; there the
stop asks a concrete question instead of offering only Accept.

**Why.** Today every Accept counts as a correct answer, and many were given
without reading, so the count said a step was good when nobody had checked it.
The only correction is sending a run back to an earlier step inside the same
run. A defect found in real use after delivery never reached the count.

**Tradeoffs.**

- A step earns trust more slowly, since it waits for the time after delivery.
- Someone has to say which step a later defect came from.

**Settled later the same day:** see the next entry above.

## 2026-11-07: Where a project's context lives, and how a step gets it

**Decided.** A job title is not the unit of a specialist; it needs finer pieces.
The same specialist works on many projects, each for a different owner, so
context sits in three layers:

- The specialist's own roles, checks, techniques and general lessons travel to
  every project.
- A project's identity, users, decisions, glossary, real data and the history
  of its runs live in that project's repository, versioned with its code.
- The person's taste splits in two: general taste travels, and taste about one
  project lives in that project's repository.

A step gets context by address, not by similarity. The run knows its project,
each step declares the context it receives, and the record keeps what it was
shown. The agent may read more files, but only inside its copy of that project.
No similarity search (RAG) for now.

**Why.** One project's context files are about 27 KB today, about 7 thousand
tokens, and fit whole. A search by similarity across projects can return a
plausible fact from another project, and makes it hard to say afterwards why a
passage was sent. Some projects hold data from the companies the person works
for, which must never reach another project.

**Tradeoffs.**

- Taste kept in each repository cannot be seen in one place across projects.
- A step that declares its context misses what nobody declared; reading its
  own copy of the project covers part of that.

**Later.** Similarity search only when a project's history or references
outgrow what fits, such as hundreds of runs or a large reference library; even
then only after filtering by project, and with what was sent recorded.

## 2026-11-06: Jaynes is the engine of a dark software factory

**Decided.** Agents conduct the development cycle. The person writes what they
want and supervises results and exceptions: on the loop, not in it. Work
focuses on the roles; the engine works and needs improvements, not a rewrite.

**Agreed direction, not started.**

- When the external proof passes, the merge happens alone and the person sees
  the result afterwards. Irreversible effects, such as publishing or sending a
  message, keep a stop.
- External proof comes before automatic merge. External proof means checks the
  building agent cannot see or shape, and a run against the real service.

**Why.** On one project built from nothing, 15 delivered runs took about 71
hours, about 67 of them at stops, while agents worked less than 4. The first
screen came about 44 hours in, because the first slices were internal packages.
The stops were accepted without being read. The serious defects appeared only
in real use: one feature passed every agreed example and left most messages
unprocessed on its first real run.

This is one project, one person and a second reader nearby, so it does not show
that stops fail in general.

**Tradeoffs.**

- Automatic merge without external proof would have shipped that defect.
- A misunderstood task surfaces after the build, not before it.
- Hidden checks and extra agents cost tokens, and Jaynes already cost 23% to 61%
  more than direct work in three single measurements.
- A reviewing agent shares the blind spots of the model it reviews, so it does
  not replace a real run.

**Open.**

- A role for new projects whose first slice shows something working.
- A level per step: Jaynes alone, ask on doubt, ask by sample, always ask.
- A reviewing agent that leaves a short recommendation.
- Who writes the hidden scenarios, and who names the real command.
- A queue that starts runs, and a way to halt and undo.

**Not copied from existing factories.** Behavioral clones of external services
(one real run is cheaper at this scale), and removing the person from every
decision.
