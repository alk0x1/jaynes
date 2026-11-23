# What the engine guarantees

**Status**: approved by the owner on 2026-11-12; G9 amended and G17 approved by the owner on 2026-11-13; G12 amended by the owner on 2026-11-16, so the person can bring back what they dropped. G17 amended, and a limit added below, by the owner on 2026-11-18. G14 amended by the owner on 2026-11-18, so an act happens at most once. G19 added by the owner on 2026-11-18. G5 amended on 2026-11-22 in the tech-lead's words, so an attempt that dies or passes the ceiling counts what it spent. Each sentence is a promise of the engine's core, whatever role runs on it. Each has a number, and the test that proves it carries the same number (`engine/crates/core/tests/guarantees.rs`). The agent that writes the engine's code may not change this page: a change here is the owner's.

What the page does not promise is said at the end, so nobody reads more into it.

G1, G2, G3, G4, G5, G9, G10, G11, G12, G13, G14 (its order, and that the core never does an act again) and G19 (a delivery that holds a merge goes out only with the person's answer about it), and the rule of G17 (when a stop may skip asking), are also
proved, in Lean, for a model of the core's rules that a test compares with the core on every state of 800 random
runs (`engine/model`). The chance in G17 is not proved there; a simulation of 3,000 streams checks it.

## Order and checks

- **G1.** A run takes the steps of its role in the role's order, one at a time, and a step
  starts only when every step it needs is done.
- **G2.** A step's output reaches a later step only after every check of that step passed. A
  step a role runs only at high risk (such as fix-bug's hidden cases) does not run in a
  low-risk run, and its checks are not checks of that run: the run's claims say so in the
  role's words. If the risk rises to high during the run, that step runs, and its checks judge
  the change before anything is delivered.
- **G3.** A check marked hidden is never given to the agent: not its name, not its inputs. When
  it fails, the agent gets only the sentence the check wrote for it.

## Faults and cost

- **G4.** A fault of Jaynes or of the machine (a box that refuses, a program that does not
  start, a check that cannot judge) never spends one of the agent's attempts. The run stops at
  once and says the fault is Jaynes's.
- **G5.** Every run has a ceiling of time and of tokens, written down before its first step.
  No attempt starts once either is reached, and an attempt that passes either while it runs is
  stopped there, counted from its agent's stream as it goes. Every attempt's time and tokens
  are recorded as its agent program reported them, an attempt that died before its end
  included. Tokens read back from the cache are recorded apart and do not count toward the
  ceiling. An attempt whose program died before reporting what it spent shows "not counted
  yet", never 0. Codex reports what an attempt spent only when the attempt ends, so a Codex
  attempt is not stopped midway for its tokens: the ceiling stops the attempt after it.
- **G6.** Every attempt ends with exactly one recorded outcome, and a late report from an
  older attempt changes nothing.

## Risk

- **G7.** Rules in code set how risky a run is, from what the change touches. The level only
  goes up during a run. You may raise it; an agent never lowers it.
- **G8.** A riskier run takes every step a less risky one takes, and more.

## Your answers

- **G9.** Every answer records who gave it: you, an agent, or a named rule. A rule answers only
  routine permissions, or a permission you already answered in the same run, with your same answer. An agent may only let a result go on as it is, at a stop the role marks
  as not a matter of taste or direction: it never sends anything back, never answers one of
  your own steps, and nothing that cannot be undone rests on its answer. You can change what
  an agent let through.
- **G10.** An answer counts only for what you were shown: if the question changed after you
  saw it, the answer is refused.
- **G11.** What each answer will do is written by Jaynes, not by the role, before you send it,
  and Jaynes does what it said.
- **G12.** Something you kept stays as it is unless you change it, and something you dropped
  never comes back in the run unless you bring it back. Only you undo your own decisions: what
  you dropped stays in view, and you can bring it back in a later round.

## Asking less

- **G17.** A stop asks you less only on evidence. With an error budget, it asks only a sample of
  runs once its record proves, by a test that holds however often it is checked, that its error
  rate is below the budget; if the true rate is at or above it, the chance of that ever happening
  is at most 5%. What counts is what happened after delivery: an accept counts as right only
  after its delivery held 7 days, and a send-back or a correction counts as an error at once.
  What an agent lets go on in your name goes on, and earns nothing until you confirm it. An
  answer that does not judge ("I don't know") counts as neither. Two
  errors among its last ten outcomes make it ask on every run and hold the run, and then only
  you answer it. A new model of the agent starts with the stop's errors and none of its rights.

## Going on before you answer

- **G13.** When you change an answer, only the steps that used what changed run again. Every
  step kept as it was used the values that hold now.
- **G14.** Nothing that cannot be undone runs while it depends on an answer you have not
  given: the run waits just before it. And it acts at most once in a run: once done, no answer,
  correction or recovery runs it again on its own. When what it rested on changes, the run holds
  there and shows you what was done and what would be done now; only you decide to keep it as it
  is, to act again (a second act, which you approve as such), or to run the correction the role
  declares. After a fault, an act whose outcome was not recorded counts as one that may have
  happened: it is never started again on its own, unless the role marks it safe to repeat, and
  then only with the same key.

## The record

- **G15.** The record only grows, and each entry carries the fingerprint of the one before it,
  so a change to any past entry shows.
- **G16.** Reading the record again gives the same state, and a run stays on the version of
  its role and of the engine it started with.

## What lands on your branch

- **G19.** A delivery that lands on your branch never brings in, without your answer, a file that other programs run or obey later outside any box: agents' and editors' settings and instructions, git hooks and attributes, CI, new packages or packages from a new source, environment and package scripts, as the engine lists them. The run waits before the merge, names each such file and shows what changes in it; you merge it, or take the change as a branch only. No agent and no rule answers it, and an answer counts only for the files and the changes it was shown.

- **G20.** Checks judge the run's tree. What a run delivers is that tree without the files its role names as shown only (by name, never everything); those stay in the run's record and on its page. Where the role's checks ran the project's tests, they run once more on what is delivered, before the merge.

## What this page does not promise

- That Jaynes holds against a program that runs as you, outside any box, and rewrites Jaynes's own files.
- That the list of such files is complete: it holds what is known to run by itself today, and a file a tool starts to obey later is not on it until it is added.
- That an agent's step is confined to its folder beyond these guards. With Claude Code as the agent, Jaynes sets
  Claude Code's own guards for each step and records the settings it applied: the tools it may
  use and the paths they may touch. On macOS, every shell command of the agent runs in Jaynes's
  own box, the one below, which Jaynes puts around each command before it runs, and a command it
  could not put there does not run: the turn then ends as Jaynes's fault. There the commands write
  only in the step's folder, read none of your home but the run's copy and the folders the role
  names, reach no network but the local ports the run reserved or the step names, the ports you let the project's
  tests use and, in a step that installs packages, the package registries through Jaynes's own
  proxy; where you opened a project's ports yourself, every local port, the page's among them.
  They cannot read the engine's folder, the other runs, your secrets or the temporary folder your
  Claude Code sessions share, open no app and start none of those browsers, Safari among them.
  Elsewhere, Claude Code's sandbox holds the agent's shell commands (writing only in the step's
  folder, no network but what the step declares, the engine's folder, the other runs, your secrets
  and that temporary folder hidden), and these browsers' apps are hidden from them, so Chrome, Chromium, Firefox, Edge, Brave, Arc, Opera, Vivaldi, Zen, Orion, LibreWolf, Waterfox, Floorp and Thorium do not
  start. Claude Code's file tools are held by its permission rules, not by the operating system, and Jaynes claims no more than those guards.
- That what Jaynes runs itself is held beyond its box. On macOS, a project's test suite, a
  check's commands and a role's own scripts run in Jaynes's own box: they write only in the
  step's folders; they reach no network but the local ports the run reserved or the step
  names, and with the project's test servers the ports you let its tests use and the ports the
  system picks for them, never the Jaynes page's; where you opened a project's ports yourself,
  every local port, the page's among them; they send no UDP, cannot read the engine's folder, the other
  runs, your secrets or those browsers' profiles, cannot use your clipboard or keychain, open no
  app and write to no terminal. A project's test suite and a check's commands, as Jaynes runs
  them, start none of Chrome, Chromium, Firefox, Edge, Brave, Arc, Opera, Vivaldi, Zen, Orion, LibreWolf, Waterfox, Floorp and Thorium, nor Safari. Jaynes photographs pages with one Chrome of this Mac, the one
  `JAYNES_CHROME` names or else the one you installed, with no window and a profile of its own,
  and a role's own scripts may start that same Chrome to draw their pages; so where a role's
  script runs the project's code in its step, its app or its tests, that code can start that
  Chrome too, still without your profiles. The agent's own commands do not start it. Neither the agent's box nor the check's box reads a browser profile of the person's: cookies, saved logins and history of any browser or browser tool (Chrome, Safari, Firefox, Arc, Edge, Brave, Playwright, Puppeteer). A browser's installed program may be reached; its profile never. They can still read your other files that are not on the list of secrets, take a
  port they are allowed in a way other machines on your network can reach while they run, and
  use as much CPU, memory and disk as the step's time allows. Elsewhere there is no box yet,
  and these commands do not run.
- That the agent's work is right. The checks say what they checked; you say the rest.
- That a correction is the same as a run started over with the corrected answer. Steps run
  again on top of their previous work.
