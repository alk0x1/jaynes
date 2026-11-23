/-
  A model of how the engine's core chooses its next move (`move_once` in `crates/core/src/decide.rs`), written a
  second time, in Lean, so the guarantees that rest on it can be proved for every run, not only for the runs a test
  draws. The Rust code is compared with this model by a test that feeds both the same states
  (`crates/core/tests/model.rs`); the proofs are in `Proofs.lean`.

  The model sees a run as the core does when it chooses: for each step, who does it, where it is, what it read when
  it was done and what the steps it reads hold now, and the state of the question about its output. Outputs are
  numbers here: only their equality matters.
-/
import Lean.Data.Json
open Lean

namespace Jaynes

inductive Exec where
  | agent | role | person
  deriving DecidableEq, Repr, FromJson, ToJson, Inhabited

inductive Status where
  | idle | probing | running | checking | awaiting | done
  deriving DecidableEq, Repr, FromJson, ToJson, Inhabited

/-- The question about a step's current output. -/
inductive Q where
  /-- none was asked yet -/
  | none
  /-- asked, and nobody answered -/
  | waiting
  /-- the person answered it -/
  | person
  /-- only an agent answered it: it let the result go on -/
  | agent
  /-- replaced by a newer question, since the step ran again -/
  | superseded
  deriving DecidableEq, Repr, FromJson, ToJson, Inhabited

/-- What a step read when it was done: an earlier step's output. -/
structure Read where
  step : Nat
  output : Nat
  deriving DecidableEq, Repr, FromJson, ToJson, Inhabited

structure Step where
  exec : Exec
  status : Status
  /-- the steps it reads, in order: those it needs, earlier steps that change the project, what its checks read -/
  sources : List Nat
  /-- its output that passed its checks, if any -/
  output : Option Nat
  /-- what it read when it was done -/
  reads : List Read
  /-- it has a stop that asks at the run's risk -/
  stopApplies : Bool
  question : Q
  /-- the question holds the run -/
  holds : Bool
  /-- a certified stop skipped asking this round -/
  skipped : Bool
  /-- what it does cannot be undone -/
  irreversible : Bool
  /-- it cannot be undone, already acted, and the person is asked about the act (keep it, or act again) -/
  held : Bool
  /-- it acts on something real, is not safe to repeat, and acted, or may have, in this intent without being done:
      its checks did not pass after the act, or the attempt's outcome was never recorded -/
  actedPending : Bool
  /-- Jaynes tested what it reaches -/
  probed : Bool
  /-- the agent has attempts left this round -/
  attemptsLeft : Bool
  /-- its stop has a budget and its count is certified within it -/
  certified : Bool
  /-- its stop is braked after errors -/
  braked : Bool
  /-- the run's coin for it falls outside its sample -/
  coinOutside : Bool
  deriving Repr, FromJson, ToJson, Inhabited

structure Run where
  steps : List Step
  /-- the steps the run takes at its risk, in order -/
  path : List Nat
  /-- finished, blocked, halted, or delivering -/
  stopped : Bool
  ceilingReached : Bool
  /-- the last delivery holds what the path holds now -/
  deliveredCurrent : Bool
  anyOpenQuestion : Bool
  /-- G19: the delivery merges into the person's branch and brings files in: every such merge waits for the person,
      what runs later outside any box among them named -/
  mergeHeld : Bool
  /-- G19: the person answered about this very delivery -/
  mergeAnswered : Bool
  /-- G19: a question about this very delivery waits for the person -/
  mergeAsked : Bool
  /-- The person clicked Merge on the run once it had delivered on a branch, and no delivery went out since: what it
      delivered is not merged yet, nor undone, nor started never to merge (`MergeCase`) -/
  mergeLater : Bool
  /-- What the merge would bring lands: the person kept it, or it brings nothing in -/
  mergeKept : Bool
  deriving Repr, FromJson, ToJson, Inhabited

inductive Move where
  | none
  | ask (i : Nat)
  | skip (i : Nat)
  | waitAt (i : Nat)
  | waitAnswers
  | invalidate (i : Nat)
  | block
  | probe (i : Nat)
  | start (i : Nat)
  | deliver
  /-- G19: ask the person about what the merge would bring in -/
  | askMerge
  | finish
  deriving DecidableEq, Repr

def Move.render : Move → String
  | .none => "none"
  | .ask i => s!"ask {i}"
  | .skip i => s!"skip {i}"
  | .waitAt i => s!"wait {i}"
  | .waitAnswers => "wait answers"
  | .invalidate i => s!"invalidate {i}"
  | .block => "block"
  | .probe i => s!"probe {i}"
  | .start i => s!"start {i}"
  | .deliver => "deliver"
  | .askMerge => "ask merge"
  | .finish => "finish"

namespace Run

def get (r : Run) (i : Nat) : Step := r.steps.getD i default

/-- What step `i` would read now: each source that has an output, with that output. -/
def currentReads (r : Run) (i : Nat) : List Read :=
  (r.get i).sources.filterMap fun j => (r.get j).output.map fun o => ⟨j, o⟩

/-- Done, on exactly what it would read now. -/
def fresh (r : Run) (i : Nat) : Bool :=
  (r.get i).status == .done && (r.get i).reads == r.currentReads i

/-- The question about step `j`'s output waits for the person: open, or answered only by an agent. -/
def awaitsPerson (r : Run) (j : Nat) : Bool :=
  (r.get j).question == .waiting || (r.get j).question == .agent

/-- Whether any step in `frontier`, or any step they read, directly or through others, waits for the person.
    Out of fuel it says yes, so the model errs towards waiting; with as much fuel as there are steps it never runs
    out, since a step reads only earlier ones. -/
def rests (r : Run) : Nat → List Nat → Bool
  | 0, _ => true
  | fuel + 1, frontier => frontier.any fun j => r.awaitsPerson j || r.rests fuel (r.get j).sources

/-- A done step whose stop asks at the run's risk and has not asked about this output. -/
def owed (r : Run) (i : Nat) : Bool :=
  (r.get i).exec != .person && (r.get i).stopApplies && r.fresh i && (r.get i).question == .none && !(r.get i).skipped

def busy (r : Run) : Bool :=
  r.path.any fun i =>
    let s := (r.get i).status
    s == .running || s == .checking || s == .probing

def holdsAbove (r : Run) (i : Nat) : Bool :=
  (r.get i).sources.any fun j => ((r.get j).question == .waiting && (r.get j).holds) || (r.get j).held

end Run

/-- The first element of a list that `p` holds for. -/
def firstWhere (p : Nat → Bool) : List Nat → Option Nat
  | [] => none
  | x :: xs => if p x then some x else firstWhere p xs

namespace Run

/-- What to do with a step that has not run yet in this round. -/
def idleMove (r : Run) (i : Nat) : Move :=
  if (r.get i).exec == .person then (if (r.get i).question == .waiting then .waitAt i else .ask i)
  -- G14: a step that acted is not started again on its own: the run asks the person, then waits for them
  else if (r.get i).actedPending then (if (r.get i).held then .waitAt i else .ask i)
  else if r.holdsAbove i then .waitAt i
  else if (r.get i).irreversible && r.rests (r.steps.length + 1) (r.get i).sources then .waitAt i
  else if r.ceilingReached then .block
  else if !(r.get i).attemptsLeft then .block
  else if !(r.get i).probed then .probe i
  else .start i

/-- What to do with step `i`, the first step of the path that is not done on what holds now. -/
def moveAt (r : Run) (i : Nat) : Move :=
  match (r.get i).status with
  | .awaiting => .waitAt i
  -- G14: an act already done is never done again on its own: the run asks the person, then waits for them
  | .done => if (r.get i).irreversible then (if (r.get i).held then .waitAt i else .ask i) else .invalidate i
  | .idle => r.idleMove i
  | _ => .none

/-- When every step of the path is done on what holds now. G19: a merge that brings files into the person's branch,
    what runs later outside any box among them, waits for the person's answer about this very delivery. -/
def settled (r : Run) : Move :=
  if !r.deliveredCurrent then
    (if r.mergeHeld && !r.mergeAnswered then (if r.mergeAsked then .waitAnswers else .askMerge) else .deliver)
  -- the person's Merge of what went out on a branch: asked at the merge's stop, then delivered merged once, when kept
  else if r.mergeLater && !r.mergeKept && !r.mergeAnswered then (if r.mergeAsked then .waitAnswers else .askMerge)
  else if r.mergeLater && r.mergeKept && (r.mergeAnswered || !r.mergeHeld) then .deliver
  else if r.anyOpenQuestion then .waitAnswers else .finish

/-- A stop owed a question skips it only when its count is certified, it is not braked, and the coin falls outside
    its sample. -/
def owedMove (r : Run) (i : Nat) : Move :=
  if (r.get i).certified && !(r.get i).braked && (r.get i).coinOutside then .skip i else .ask i

/-- The core's next move. -/
def next (r : Run) : Move :=
  if r.stopped then .none
  else if r.busy then .none
  else match firstWhere r.owed r.path with
    | some i => r.owedMove i
    | none =>
      match firstWhere (fun i => !r.fresh i) r.path with
      | none => r.settled
      | some i => r.moveAt i

end Run
end Jaynes

/-! ## Taking an answer (`answer` in `decide.rs`) -/

namespace Jaynes

inductive Who where
  | person | rule | agent
  deriving DecidableEq, Repr, FromJson, ToJson, Inhabited

/-- An answer as the core judges it: who gave it, and the facts about its question it reads. -/
structure AnswerCase where
  who : Who
  /-- a decision, not a leave -/
  decision : Bool
  /-- the question is the person's own step -/
  personStep : Bool
  /-- the role asked it inside its step -/
  input : Bool
  /-- the role marks the stop a release -/
  release : Bool
  /-- the stop is braked after errors -/
  braked : Bool
  superseded : Bool
  /-- the answer carries the fingerprint of the question as it stands -/
  shownMatches : Bool
  /-- each item exists, each verdict is offered, and a change says what -/
  verdictsValid : Bool
  allKeep : Bool
  hasContext : Bool
  /-- the question was answered before -/
  answered : Bool
  /-- that earlier answer may still be changed: the step is done on it and it is the step's current question -/
  revisable : Bool
  /-- the question is about an act that cannot be undone and already acted -/
  acted : Bool
  /-- G19: the question holds a merge -/
  merge : Bool
  /-- G19: the delivery it was asked about is the one that would land now -/
  current : Bool
  deriving Repr, FromJson, ToJson, Inhabited

/-- An act already done is the person's alone to keep or to order again, once. -/
def AnswerCase.acceptsActed (a : AnswerCase) : Bool :=
  a.who == .person && !a.answered && !a.superseded && a.shownMatches && a.verdictsValid

/-- G19: what lands on the person's branch is theirs alone to decide, once, about the delivery that would land. -/
def AnswerCase.acceptsMerge (a : AnswerCase) : Bool :=
  a.who == .person && !a.answered && !a.superseded && a.current && a.shownMatches && a.verdictsValid

def AnswerCase.accepts (a : AnswerCase) : Bool :=
  if a.acted then a.acceptsActed
  else if a.merge then a.acceptsMerge
  else if a.who == .rule then false
  else if a.superseded then false
  else if !a.shownMatches then false
  else if !a.verdictsValid then false
  else if a.who == .agent && (a.hasContext || !a.decision || a.personStep || a.input || !a.release || !a.allKeep ||
      a.braked || a.answered) then false
  else if a.answered && !a.revisable then false
  else true

/-! ## The person's Merge of a finished run (`merge` in `decide.rs`) -/

/-- A click on Merge, as the core judges it: who clicked, and what the run is when they do. -/
structure MergeCase where
  who : Who
  /-- the merge's stop already waits for the person -/
  mergeOpen : Bool
  finished : Bool
  /-- the run delivered something -/
  delivered : Bool
  /-- the person undid the run's merge -/
  undone : Bool
  /-- what it delivered is already merged -/
  merged : Bool
  /-- the run was started never to merge -/
  neverMerge : Bool
  /-- its delivery changes no file -/
  changesNothing : Bool
  deriving Repr, FromJson, ToJson, Inhabited

/-- Merge is taken only from the person, of a run on a branch whose delivery is not merged yet. -/
def MergeCase.accepts (c : MergeCase) : Bool :=
  c.who == .person && !c.mergeOpen && c.finished && c.delivered && !c.undone && !c.merged && !c.neverMerge &&
    !c.changesNothing

/-! ## The gate of the checks (`checks_ended` in `decide.rs`) -/

structure Item where
  id : Nat
  digest : Nat
  deriving DecidableEq, Repr, FromJson, ToJson, Inhabited

/-- A check of the step, as one pass through the gate meets it. -/
structure GateCheck where
  hidden : Bool
  /-- every step it reads has an output, so the pass takes it: one that reads a step the run does not take at its
      risk (fix-bug's hidden cases at low risk) is not a check of that run (`Run::checks_taken`) -/
  taken : Bool
  /-- it gave a verdict, pass or fail: not one that could not judge, nor none -/
  gave : Bool
  passed : Bool
  deriving Repr, FromJson, ToJson, Inhabited

structure GateCase where
  /-- what the person kept, and what they dropped -/
  locks : List Item
  dropped : List Item
  /-- the items of the output under check -/
  items : List Item
  /-- the step's checks, in the role's order -/
  checks : List GateCheck
  roleStep : Bool
  fixesLeft : Bool
  session : Bool
  deriving Repr, FromJson, ToJson, Inhabited

inductive Gate where
  | done | fix | spent | jaynes | role
  deriving DecidableEq, Repr

def Gate.render : Gate → String
  | .done => "done" | .fix => "fix" | .spent => "spent" | .jaynes => "jaynes" | .role => "role"

/-- A check the pass takes failed: an open one, a hidden one. -/
def GateCase.openFails (g : GateCase) : Bool :=
  g.checks.any fun c => c.taken && !c.hidden && c.gave && !c.passed

def GateCase.hiddenFails (g : GateCase) : Bool :=
  g.checks.any fun c => c.taken && c.hidden && c.gave && !c.passed

/-- A check the pass takes could not judge, or gave no verdict. -/
def GateCase.blocked (g : GateCase) : Bool :=
  g.checks.any fun c => c.taken && !c.gave

/-- Whether the output breaks what the person kept or dropped. -/
def GateCase.breaks (g : GateCase) : Bool :=
  g.locks.any (fun l => !(g.items.any fun x => x.id == l.id && x.digest == l.digest)) ||
    g.dropped.any (fun d => g.items.any fun x => x.id == d.id || x.digest == d.digest)

def GateCase.outcome (g : GateCase) : Gate :=
  if g.blocked then .jaynes
  else if !g.openFails && !g.breaks && !g.hiddenFails then .done
  else if g.roleStep then .role
  else if !g.hiddenFails && g.fixesLeft && g.session then .fix
  else .spent

end Jaynes

/-! ## What an answer says it will do, and what it does (`effects` and `answer` in `decide.rs`) -/

namespace Jaynes

structure Verdicted where
  /-- the verdict sends work back -/
  back : Bool
  /-- on the person's own step, the step whose output the item stands for -/
  standsFor : Option Nat
  deriving Repr, FromJson, ToJson, Inhabited

structure EffectCase where
  step : Nat
  personStep : Bool
  input : Bool
  permission : Bool
  /-- G19: the answer about what a merge brings in -/
  merge : Bool
  items : List Verdicted
  deriving Repr, FromJson, ToJson, Inhabited

/-- The steps an item's verdict sends back to. -/
def EffectCase.targets (c : EffectCase) : List Nat :=
  c.items.filterMap fun it => if it.back then (if c.personStep then it.standsFor else some c.step) else none

/-- The steps the answer says will run again, before it is sent (`Effects::reruns`). -/
def EffectCase.said (c : EffectCase) : List Nat :=
  if c.permission || c.input || c.merge then []
  else if c.personStep && !c.targets.isEmpty then c.step :: c.targets else c.targets

/-- The steps the answer makes run again once sent. -/
def EffectCase.did (c : EffectCase) : List Nat :=
  if c.permission || c.input || c.merge then []
  else if c.items.any (·.back) then
    (if c.personStep then (c.items.filterMap fun it => if it.back then it.standsFor else none) ++ [c.step] else [c.step])
  else []

end Jaynes

/-! ## What the merge's touch keeps (`rides` and `answer_merge` in `decide.rs`) -/

namespace Jaynes

/-- An open stop as the merge's question finds it. -/
structure Open where
  step : Nat
  decision : Bool
  holds : Bool
  keepOffered : Bool
  /-- a keep of every item answers it: it chooses nothing, or chooses with one option -/
  keepAnswers : Bool
  deriving Repr, FromJson, ToJson, Inhabited

/-- Whether the merge's touch keeps this stop: an open decision that does not hold the run, which a keep answers. A
    braked stop holds, so it is never one: the person answers it on its own. -/
def Open.rides (o : Open) : Bool := o.decision && !o.holds && o.keepOffered && o.keepAnswers

/-- The stops the merge's question names, in the path's order. -/
def ridesOf (opens : List Open) : List Nat := (opens.filter Open.rides).map (·.step)

/-- A stop the merge's question named, and whether it is still open at the touch. -/
structure Rode where
  step : Nat
  stillOpen : Bool
  deriving Repr, FromJson, ToJson, Inhabited

structure TouchCase where
  /-- the person's answer about the merge, as the core judges it -/
  answer : AnswerCase
  /-- every file shown is kept: it merges -/
  lands : Bool
  rides : List Rode
  deriving Repr, FromJson, ToJson, Inhabited

/-- The stops the touch keeps, each with its own answer: only when the answer about the merge is taken and it merges,
    and only those still open. -/
def TouchCase.kept (t : TouchCase) : List Nat :=
  if t.answer.merge && !t.answer.acted && t.answer.accepts && t.lands then (t.rides.filter (·.stillOpen)).map (·.step)
  else []

end Jaynes

/-! ## When a stop holds the run (`ask_output` and `named` in `decide.rs`) -/

namespace Jaynes

/-- A stop's question as it opens: what its stop and its step say. -/
structure Hold where
  /-- it asks about what an agent's or the role's step made: its stop's question. Any other (the person's own step,
      what a role asks, a step that cannot be done) holds the run -/
  ofOutput : Bool
  holds : Bool
  /-- the stop does not hold when the request or the brief named the one option (`holds_unless`) -/
  unlessNamed : Bool
  /-- the step has the check that vouches for those words -/
  vouchDeclared : Bool
  /-- the output offers one option, and it carries the words that named it -/
  oneNamed : Bool
  braked : Bool
  deriving Repr, FromJson, ToJson, Inhabited

/-- Whether the question holds the run: the stop holds unless the one option was named and vouched for; a braked stop
    holds whatever its role says (G17). -/
def Hold.holdsNow (h : Hold) : Bool :=
  !h.ofOutput || (h.holds && !(h.unlessNamed && h.vouchDeclared && h.oneNamed)) || h.braked

end Jaynes
