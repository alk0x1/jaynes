# What the engine guarantees

**Status**: draft for the owner to read and approve. Each sentence is a promise of the engine's core, whatever role runs on it. Each has a number, and the test that proves it carries the same number (`engine/crates/core/tests/guarantees.rs`). The agent that writes the engine's code may not change this page: a change here is the owner's.

What the page does not promise is said at the end, so nobody reads more into it.

## Order and checks

- **G1.** A run takes the steps of its role in the role's order, one at a time, and a step
  starts only when every step it needs is done.
- **G2.** A step's output reaches a later step only after every check of that step passed.
- **G3.** A check marked hidden is never given to the agent: not its name, not its inputs. When
  it fails, the agent gets only the sentence the check wrote for it.

## Faults and cost

- **G4.** A fault of Jaynes or of the machine (a box that refuses, a program that does not
  start, a check that cannot judge) never spends one of the agent's attempts. The run stops at
  once and says the fault is Jaynes's.
- **G5.** Every run has a ceiling of time and of tokens, written down before its first step.
  No attempt starts once either is reached, and the time and tokens of every attempt are
  recorded.
- **G6.** Every attempt ends with exactly one recorded outcome, and a late report from an
  older attempt changes nothing.

## Risk

- **G7.** Rules in code set how risky a run is, from what the change touches. The level only
  goes up during a run. You may raise it; an agent never lowers it.
- **G8.** A riskier run takes every step a less risky one takes, and more.

## Your answers

- **G9.** Every answer records who gave it: you, an agent, or a named rule. A rule answers only
  routine permissions. An agent may only let a result go on as it is, at a stop the role marks
  as not a matter of taste or direction: it never sends anything back, never answers one of
  your own steps, and nothing that cannot be undone rests on its answer. You can change what
  an agent let through.
- **G10.** An answer counts only for what you were shown: if the question changed after you
  saw it, the answer is refused.
- **G11.** What each answer will do is written by Jaynes, not by the role, before you send it,
  and Jaynes does what it said.
- **G12.** Something you kept stays as it is unless you change it, and something you dropped
  never comes back in the run.

## Going on before you answer

- **G13.** When you change an answer, only the steps that used what changed run again. Every
  step kept as it was used the values that hold now.
- **G14.** Nothing that cannot be undone runs while it depends on an answer you have not
  given: the run waits just before it.

## The record

- **G15.** The record only grows, and each entry carries the fingerprint of the one before it,
  so a change to any past entry shows.
- **G16.** Reading the record again gives the same state, and a run stays on the version of
  its role and of the engine it started with.

## What this page does not promise

- That a step is confined. Jaynes sets the agent program's own guards for each step and
  records the settings it applied; those guards are partial (in Claude Code the sandbox covers
  shell commands only), and Jaynes does not claim more than them.
- That the agent's work is right. The checks say what they checked; you say the rest.
- That a correction is the same as a run started over with the corrected answer. Steps run
  again on top of their previous work.
