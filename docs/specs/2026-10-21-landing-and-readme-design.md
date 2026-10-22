# Landing page and README: design

Date: 2026-10-21. Status: draft for Alan's review. Nothing here is built yet
except where marked "works today".

This document fixes what Jaynes is, who it is for, and how it works, as told by
the landing page and the README. It supersedes the framing of `SPEC.md`, which
describes only the action gate built in M1. `SPEC.md` gets rewritten after this
document is approved.

## 1. Decisions taken

| Question | Decision |
|---|---|
| Who the page is for | People who direct a team of AI agents doing real work, the way a one-person company run by agents does |
| What they review | Two layers: the steps and deliveries of each role on top, individual tool actions underneath |
| Where review happens | A local web page (`jaynes review`), plus a CLI command to approve quickly |
| Page form | A story told in scenes over a few weeks, with one interactive element at the end |
| Language | English, for the page, the README and the code |
| Name of the first user | "A one-person company run by agents" (never "Helm") |
| Agents supported | Claude Code and Codex, both promised from the first release |
| Theory | A "Foundations" section near the end of the page, verified citations only |

## 2. The question Jaynes answers

> How do I guarantee, or at least raise the odds, that my agents' work is good,
> while spending my attention only where it matters?

The answer has three layers, and the page is built around them:

1. **The method is enforceable.** A role follows mandatory steps, each with a
   required output, and cannot advance without it.
2. **Part of the result is checkable.** Tests, contrast, design tokens,
   forbidden terms: deterministic checks.
3. **The rest is probability.** Your own judgments, and later judges
   calibrated against them, decide which checkpoints still need you.

## 3. The problem, from the first user's baseline

Measured on the one-person company before Jaynes (source:
`helm/work/jaynes-baseline.md`, 2026-10-21):

| Measure | Value |
|---|---|
| Tool calls by agents, Sep 1 to Oct 21 | 11,894 |
| Of those, seen by a person | 25 (0.2%) |
| Error rate of the rest | unknown |
| Design screens whose agent did the method's first four steps | 0 of 8 |
| Screens that brought requirements to the person for approval | 0 of 8 |
| Screens that passed every automated check and were still rejected as "rough" | Processes, three rounds in a row |
| Tokens per screen, method budget vs measured | 150k vs 170k to 480k |
| Agents' self-reported cost vs system meter | 85k and 135k reported, more measured |

These are the "before" numbers. The page shows them as such. No "after"
number appears until it is measured.

## 4. How Jaynes works

### 4.1 Concepts

| Term | Meaning |
|---|---|
| **Role** | A kind of work an agent does: screen design, copy review, bug fix |
| **Method** | The ordered steps of a role |
| **Step** | One unit of the method, with a required output in a fixed schema |
| **Check** | Code that validates a step's output before the next step may start |
| **Witness** | The value `Checked[T]` that only a check can construct; the next step takes it as input, so "the step happened" is a fact in the log, not a claim |
| **Stop** | A step where the method needs a person; the run waits for an answer |
| **Label** | A person's answer at a stop: accepted as is, changed, or returned |
| **Budget** | The error rate a stop may have once it stops being mandatory |
| **Sampled stop** | A stop that earned enough evidence and now asks only a random share of the time |
| **Judge** | A model with a rubric that scores outputs, calibrated against labels (later) |
| **Floor** | An action or stop that never runs without a person |

### 4.2 The flow

1. **You describe a role as a method.** Steps, the output each must produce,
   the checks, and which steps are stops.
2. **Jaynes conducts the agent step by step.** It runs Claude Code or Codex for
   one step at a time, with write access only to that step's output. The next
   step starts only when the check passes. Evidence the agent could fake is
   produced by Jaynes instead; for example, the walkthrough of a screen is
   executed in a browser by Jaynes from a closed list of actions, and the
   agent only proposes the path.
3. **At a stop, the run waits for you.** The question and its evidence appear
   in the review page.
4. **The result goes through deterministic checks:** tests, contrast, tokens,
   before and after against the last approved version.
5. **Every answer at a stop is a label.** When a stop collects enough accepts
   without changes, it becomes sampled: it asks one time in ten. A change or a
   return makes it mandatory again.
6. **Underneath, every tool action goes through the gate.** Floors never run.
   This part works today (section 4.4).
7. **Everything is recorded** in an append-only event log, and the review page
   shows per role: steps fulfilled, rounds until acceptance, cost per task and
   where your attention went.

How fast a stop becomes sampled depends on its budget (zero changes, 95%
confidence, anytime-valid test, values from `jaynes.bounds`):

| Budget | Accepts in a row |
|---|---|
| 5% | 88 |
| 10% | 43 |
| 20% | 20 |
| 30% | 13 |

A low-risk stop ("pick the moments of use") can use 20%. Accepting a finished
screen stays at 5% or is never sampled.

### 4.3 Architecture consequence

To make skipping impossible, Jaynes has to be the conductor: it invokes the
agent per step and owns the write permissions. A hook alone cannot stop an
agent from not doing something. The first user already has a working
prototype of this layer (`helm/work/factory-pipeline/design_flow.py`): fixed
step order, JSON schemas with checks, writes limited to the step's file, stops
that wait, walkthrough executed by the coordinator, Codex as the agent. It is
the reference for layer 1.

### 4.4 What works today

The action gate from M1: a Claude Code hook that classifies each tool call,
enforces floors, samples audits, reads the person's answers from the
transcript and certifies categories with an anytime-valid test. Verified on
real history (11,894 calls) and on Claude Code 2.1.306. On the page it is the
"safety underneath" scene, not the headline.

## 5. Foundations

Each entry says what it gives the user. Citations verified on 2026-10-21.

| Layer | Work | What it gives |
|---|---|---|
| 1 | Strom and Yemini, "Typestate", IEEE TSE 12(1), 1986 | The method as a type: which operations are valid depends on the state the work is in |
| 1 | Honda, "Types for Dyadic Interaction", CONCUR 1993 | Session types: a protocol between parties checked as a type; the stops are the person's side of the protocol |
| 1 | Miller, "Robust Composition", PhD thesis, Johns Hopkins, 2006 | Object capabilities: an agent can only touch what it was handed, which is how write access per step is enforced |
| 1, 3 | Plotkin and Pretnar, "Handlers of Algebraic Effects", ESOP 2009 | One mechanism for gate, audit and replay: an agent's action is an effect, and a handler decides to run it, ask, record it, or replay history |
| 3 | Ramdas, Grünwald, Vovk and Shafer, "Game-Theoretic Statistics and Safe Anytime-Valid Inference", Statistical Science 38(4), 2023 | The guarantee holds even though evidence is checked after every label |
| 3 | Grünwald, de Heide and Koolen, "Safe Testing", JRSS B 86(5), 2024 | E-values as the evidence measure behind certification |
| 3 | Angelopoulos, Bates, Fannjiang, Jordan and Zrnic, "Prediction-Powered Inference", Science, 2023 | A judge labels everything cheaply; your few labels correct its bias, so the guarantee needs fewer of your answers |
| 3 | Zrnic and Candès, "Active Statistical Inference", ICML 2024 | Chooses which items to show you: the ones that most reduce uncertainty. "Probability decides where to spend attention", as an algorithm |
| 3 | Shin, Ramdas and Rinaldo, "E-detectors", NEJSDS 2(2), 2024 | Detects with a guarantee that a role got worse, so a sampled stop goes back to mandatory for a principled reason |
| ref | Bornholt, Mytkowicz and McKinley, "Uncertain<T>", ASPLOS 2014 | Antecedent of a typed value with uncertainty (`Decision[T]`) |
| ref | Geifman and El-Yaniv, "Selective Classification for Deep Neural Networks", NeurIPS 2017; Angelopoulos et al., "Learn then Test", Annals of Applied Statistics 19(2), 2025 | Risk control when an item has a score: release only the items the judge rates well |
| ref | Dawid and Skene, JRSS C 28(1), 1979 | Combining several noisy judges |
| ref | Mozannar and Sontag, "Consistent Estimators for Learning to Defer to an Expert", ICML 2020 | The neighbouring field: learning when to hand a decision to a person |

Rule: prediction-powered and active inference change the number of labels the
page promises. They appear on the page as "how it gets cheaper" only after a
simulation shows the gain on the first user's data, the same way the
anytime-valid test was validated (27.4% false certification for the naive
rule, 3.7% for the anytime-valid one, 4,000 runs at error rate equal to the
budget).

## 6. Landing page

### 6.1 Principles

- One person's story, told in scenes; each scene has a short text and an image
  of the review page at that moment.
- Real numbers only, labelled with their source; illustrative numbers marked
  as such; no "after" number before it exists.
- The reader understands the mechanism without the word "statistics" until the
  Foundations section.
- Claude Code and Codex named on the first screen.

### 6.2 Scenes

| # | Scene | Text direction | Image |
|---|---|---|---|
| 0 | Hero | Your agents do the work the right way, and only call you where it matters | The review page with one stop waiting, evidence beside it |
| 1 | The problem | Agents say "done". They skipped half the process, passed the checks, and the result is still wrong, so you fix everything. Baseline numbers from section 3 | An agent report claiming completion with no evidence |
| 2 | Write the method once | A role becomes steps, each with a required output and a check | The design role definition, short |
| 3 | The agent cannot skip | Jaynes runs one step at a time; the next one needs the previous one's witness | A step timeline with a blocked attempt to jump to implementation; walkthrough screenshots taken by Jaynes |
| 4 | It stops where you are needed | The question and the evidence, side by side | The stop "pick the moments of use" |
| 5 | The result is checked, then you decide | Tests, contrast, before and after; accept or return with a note | Evidence panel with check results |
| 6 | Weeks later, your attention moves | A stop with 20 accepts in a row becomes sampled; one line on judges reducing how many answers are needed | Evidence meter per stop crossing its threshold |
| 7 | When it goes wrong, it comes back | You change a sampled item; the stop is mandatory again, with the reason recorded | Status flip with its reason |
| 8 | What you see every week | Per role: steps fulfilled, rounds until acceptance, cost per task, attention spent; the baseline as "before" | Weekly role panel |
| 9 | Try it | A simulated queue of three items; click and watch a stop gain evidence | Interactive |
| 10 | Safety underneath | Every tool action goes through a gate; floors never run without you. Works today | Short action log |
| 11 | Foundations | One line per work from section 5 | None |
| 12 | Get started | Install, Claude Code and Codex, what exists today and what does not | Commands |

### 6.3 Open for the page build

- Exact headline and sub-headline: written with the page, two or three options
  for Alan.
- Visual direction: chosen with the design skill when the page is built; the
  review-page images are also the first design of the real review page.
- Hosting: GitHub Pages from the repository, decided at launch.

## 7. README outline

1. Title and one sentence.
2. The problem in four lines.
3. How it works in five steps, with a short role definition (marked "planned
   API").
4. Concepts table (section 4.1, shortened).
5. The guarantee in one paragraph.
6. Status: what works today, what is in progress, what is next.
7. Install and try what works today.
8. Foundations: link to the design document and the short list.
9. License.

## 8. Success on the first user

Jaynes is validated when, against the baseline in section 3:

- all design screens complete the method's steps, with requirements approved
  by the person;
- rounds until acceptance per screen go down;
- "rough" complaints after automated checks pass go down;
- tokens per screen go down and are measured by the system, not reported by
  the agent;
- the person can state an error rate per role and per stop.

These become the launch post.

## 9. Open questions

- Role definitions in Python code or in a declarative file. The README sketch
  uses Python; the first user's prototype uses JSON schemas plus Python checks.
- Whether a stop can take a judge's verdict as a provisional answer before the
  person answers.
- Codex integration path for the action gate (the conductor covers Codex; the
  hook does not).
