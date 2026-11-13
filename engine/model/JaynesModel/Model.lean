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
  /-- Jaynes tested what it reaches -/
  probed : Bool
  /-- the agent has attempts left this round -/
  attemptsLeft : Bool
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
  deriving Repr, FromJson, ToJson, Inhabited

inductive Move where
  | none
  | ask (i : Nat)
  | waitAt (i : Nat)
  | waitAnswers
  | invalidate (i : Nat)
  | block
  | probe (i : Nat)
  | start (i : Nat)
  | deliver
  | finish
  deriving DecidableEq, Repr

def Move.render : Move → String
  | .none => "none"
  | .ask i => s!"ask {i}"
  | .waitAt i => s!"wait {i}"
  | .waitAnswers => "wait answers"
  | .invalidate i => s!"invalidate {i}"
  | .block => "block"
  | .probe i => s!"probe {i}"
  | .start i => s!"start {i}"
  | .deliver => "deliver"
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
  (r.get i).sources.any fun j => (r.get j).question == .waiting && (r.get j).holds

end Run

/-- The first element of a list that `p` holds for. -/
def firstWhere (p : Nat → Bool) : List Nat → Option Nat
  | [] => none
  | x :: xs => if p x then some x else firstWhere p xs

namespace Run

/-- What to do with a step that has not run yet in this round. -/
def idleMove (r : Run) (i : Nat) : Move :=
  if (r.get i).exec == .person then (if (r.get i).question == .waiting then .waitAt i else .ask i)
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
  | .done => .invalidate i
  | .idle => r.idleMove i
  | _ => .none

/-- When every step of the path is done on what holds now. -/
def settled (r : Run) : Move :=
  if !r.deliveredCurrent then .deliver else if r.anyOpenQuestion then .waitAnswers else .finish

/-- The core's next move. -/
def next (r : Run) : Move :=
  if r.stopped then .none
  else if r.busy then .none
  else match firstWhere r.owed r.path with
    | some i => .ask i
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
  deriving Repr, FromJson, ToJson, Inhabited

def AnswerCase.accepts (a : AnswerCase) : Bool :=
  if a.who == .rule then false
  else if a.superseded then false
  else if !a.shownMatches then false
  else if !a.verdictsValid then false
  else if a.who == .agent && (a.hasContext || !a.decision || a.personStep || a.input || !a.release || !a.allKeep ||
      a.braked || a.answered) then false
  else if a.answered && !a.revisable then false
  else true

/-! ## The gate of the checks (`checks_ended` in `decide.rs`) -/

structure Item where
  id : Nat
  digest : Nat
  deriving DecidableEq, Repr, FromJson, ToJson, Inhabited

structure GateCase where
  /-- what the person kept, and what they dropped -/
  locks : List Item
  dropped : List Item
  /-- the items of the output under check -/
  items : List Item
  openFails : Bool
  hiddenFails : Bool
  /-- a check could not judge, or gave no verdict -/
  blocked : Bool
  roleStep : Bool
  fixesLeft : Bool
  session : Bool
  deriving Repr, FromJson, ToJson, Inhabited

inductive Gate where
  | done | fix | spent | jaynes | role
  deriving DecidableEq, Repr

def Gate.render : Gate → String
  | .done => "done" | .fix => "fix" | .spent => "spent" | .jaynes => "jaynes" | .role => "role"

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
