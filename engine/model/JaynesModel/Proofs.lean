/-
  Proofs of the guarantees that rest on how the core chooses its next move. Each theorem names the guarantee of
  `docs/guarantees.md` it proves, for the model in `Model.lean`, for every run: not only those a test draws.
-/
import JaynesModel.Model

namespace Jaynes

/-! ## The first element a test holds for -/

theorem firstWhere_some {p : Nat → Bool} :
    ∀ {l : List Nat} {x : Nat}, firstWhere p l = some x →
      p x = true ∧ ∃ pre suf, l = pre ++ x :: suf ∧ ∀ y ∈ pre, p y = false
  | [], _, h => by simp [firstWhere] at h
  | a :: as, x, h => by
    by_cases hp : p a = true
    · simp [firstWhere, hp] at h
      subst h
      exact ⟨hp, [], as, rfl, by simp⟩
    · simp [firstWhere, hp] at h
      obtain ⟨hx, pre, suf, hl, hpre⟩ := firstWhere_some h
      refine ⟨hx, a :: pre, suf, by simp [hl], ?_⟩
      intro y hy
      simp at hy
      rcases hy with rfl | hy
      · simpa using hp
      · exact hpre y hy

theorem firstWhere_none {p : Nat → Bool} :
    ∀ {l : List Nat}, firstWhere p l = none → ∀ y ∈ l, p y = false
  | [], _, y, hy => by simp at hy
  | a :: as, h, y, hy => by
    by_cases hp : p a = true
    · simp [firstWhere, hp] at h
    · simp [firstWhere, hp] at h
      simp at hy
      rcases hy with rfl | hy
      · simpa using hp
      · exact firstWhere_none h y hy

/-! ## What a step reaches through what it reads -/

/-- The steps reachable from `frontier` through what each reads, within `fuel` levels. -/
def Run.reach (r : Run) : Nat → List Nat → List Nat
  | 0, _ => []
  | fuel + 1, frontier => frontier ++ frontier.flatMap fun j => r.reach fuel (r.get j).sources

/-- When `rests` says no, no step reachable through what is read waits for the person. -/
theorem rests_false (r : Run) :
    ∀ (fuel : Nat) (frontier : List Nat), r.rests fuel frontier = false →
      ∀ j ∈ r.reach fuel frontier, r.awaitsPerson j = false
  | 0, _, h, _, _ => by simp [Run.rests] at h
  | fuel + 1, frontier, h, j, hj => by
    simp only [Run.rests, List.any_eq_false, Bool.or_eq_true, not_or, Bool.not_eq_true] at h
    simp only [Run.reach, List.mem_append, List.mem_flatMap] at hj
    rcases hj with hj | ⟨k, hk, hj⟩
    · exact (h j hj).1
    · exact rests_false r fuel _ (h k hk).2 j hj

/-! ## The moves -/

theorem moveAt_cases (r : Run) (i : Nat) :
    ((r.get i).status = .awaiting ∧ r.moveAt i = .waitAt i) ∨
    ((r.get i).status = .done ∧ (r.get i).irreversible = false ∧ r.moveAt i = .invalidate i) ∨
    ((r.get i).status = .idle ∧ r.moveAt i = r.idleMove i) ∨
    r.moveAt i = .none ∨
    ((r.get i).status = .done ∧ (r.moveAt i = .waitAt i ∨ r.moveAt i = .ask i)) := by
  cases hst : (r.get i).status <;> simp [Run.moveAt, hst]
  cases hirr : (r.get i).irreversible <;> cases hh : (r.get i).held <;> simp [hirr, hh]

theorem idleMove_cases (r : Run) (i : Nat) :
    r.idleMove i = .waitAt i ∨ r.idleMove i = .ask i ∨ r.idleMove i = .block ∨ r.idleMove i = .probe i ∨
      r.idleMove i = .start i := by
  unfold Run.idleMove
  repeat' split
  all_goals simp

theorem owedMove_cases (r : Run) (i : Nat) : r.owedMove i = .ask i ∨ r.owedMove i = .skip i := by
  unfold Run.owedMove
  split <;> simp

theorem settled_cases (r : Run) :
    r.settled = .deliver ∨ r.settled = .waitAnswers ∨ r.settled = .finish ∨ r.settled = .askMerge := by
  unfold Run.settled
  repeat' split
  all_goals simp

/-- What `idleMove` needs to start a step. -/
theorem idleMove_start {r : Run} {i j : Nat} (h : r.idleMove i = .start j) :
    j = i ∧ (r.get i).exec ≠ .person ∧ r.holdsAbove i = false ∧
      ((r.get i).irreversible = true → r.rests (r.steps.length + 1) (r.get i).sources = false) ∧
      r.ceilingReached = false ∧ (r.get i).attemptsLeft = true ∧ (r.get i).probed = true := by
  unfold Run.idleMove at h
  split at h
  · split at h <;> simp at h
  rename_i hp
  split at h
  · split at h <;> simp at h
  rename_i _hap
  split at h
  · simp at h
  rename_i hh
  split at h
  · simp at h
  rename_i hi
  split at h
  · simp at h
  rename_i hc
  split at h
  · simp at h
  rename_i ha
  split at h
  · simp at h
  rename_i hpr
  simp at h
  refine ⟨h.symm, by simpa using hp, by simpa using hh, ?_, by simpa using hc, by simpa using ha, by simpa using hpr⟩
  intro hirr
  simpa [hirr] using hi

/-- The shape of every move but none: the run is not stopped nor busy, and the move comes from the first step owed
    a question, or the first step not done on what holds now, or the path being settled. -/
theorem next_cases {r : Run} {m : Move} (h : r.next = m) (hm : m ≠ .none) :
    r.stopped = false ∧ r.busy = false ∧
      ((∃ i, firstWhere r.owed r.path = some i ∧ m = r.owedMove i) ∨
       (firstWhere r.owed r.path = none ∧
         ((firstWhere (fun j => !r.fresh j) r.path = none ∧ m = r.settled) ∨
          (∃ i, firstWhere (fun j => !r.fresh j) r.path = some i ∧ m = r.moveAt i)))) := by
  unfold Run.next at h
  by_cases hs : r.stopped = true
  · simp [hs] at h; exact absurd h.symm hm
  by_cases hb : r.busy = true
  · simp [hs, hb] at h; exact absurd h.symm hm
  simp only [hs, hb, Bool.false_eq_true, ↓reduceIte] at h
  refine ⟨by simpa using hs, by simpa using hb, ?_⟩
  split at h
  · rename_i i hi
    exact Or.inl ⟨i, hi, h.symm⟩
  · rename_i hn
    refine Or.inr ⟨hn, ?_⟩
    split at h
    · rename_i hz
      exact Or.inl ⟨hz, h.symm⟩
    · rename_i k hk
      exact Or.inr ⟨k, hk, h.symm⟩

/-- How the core came to start a step. -/
theorem next_start {r : Run} {i : Nat} (h : r.next = .start i) :
    r.busy = false ∧ firstWhere (fun j => !r.fresh j) r.path = some i ∧ (r.get i).status = .idle ∧
      r.idleMove i = .start i := by
  obtain ⟨_, hb, hc⟩ := next_cases h (by simp)
  rcases hc with ⟨k0, _, he⟩ | ⟨_, ⟨_, he⟩ | ⟨k, hk, he⟩⟩
  · rcases owedMove_cases r k0 with h' | h' <;> rw [h'] at he <;> simp at he
  · rcases settled_cases r with h' | h' | h' | h' <;> rw [h'] at he <;> simp at he
  · rcases moveAt_cases r k with ⟨_, hm⟩ | ⟨_, _, hm⟩ | ⟨hst, hm⟩ | hm | ⟨_, hm | hm⟩ <;> rw [hm] at he
    · simp at he
    · simp at he
    · have hj := (idleMove_start he.symm).1
      subst hj
      exact ⟨hb, hk, hst, he.symm⟩
    · simp at he
    · simp at he
    · simp at he

/-! ## The guarantees -/

/-- **G1.** A step starts only when no step is busy and every step before it in the run's path is done on what
    holds now. -/
theorem g1_order {r : Run} {i : Nat} (h : r.next = .start i) :
    r.busy = false ∧ ∃ pre suf, r.path = pre ++ i :: suf ∧ ∀ j ∈ pre, r.fresh j = true := by
  obtain ⟨hb, hf, _, _⟩ := next_start h
  obtain ⟨_, pre, suf, hl, hpre⟩ := firstWhere_some hf
  exact ⟨hb, pre, suf, hl, fun j hj => by simpa using hpre j hj⟩

/-- **G4.** No attempt starts before Jaynes tested what the step reaches. -/
theorem g4_probed {r : Run} {i : Nat} (h : r.next = .start i) : (r.get i).probed = true :=
  (idleMove_start (next_start h).2.2.2).2.2.2.2.2.2

/-- **G5.** No attempt starts once the ceiling is reached, nor once the agent spent its attempts. -/
theorem g5_ceiling {r : Run} {i : Nat} (h : r.next = .start i) :
    r.ceilingReached = false ∧ (r.get i).attemptsLeft = true :=
  let hs := idleMove_start (next_start h).2.2.2
  ⟨hs.2.2.2.2.1, hs.2.2.2.2.2.1⟩

/-- A step does not start while a step it reads has an open question that holds the run. -/
theorem holds_wait {r : Run} {i : Nat} (h : r.next = .start i) :
    ∀ j ∈ (r.get i).sources, ¬((r.get j).question = .waiting ∧ (r.get j).holds = true) := by
  have hh := (idleMove_start (next_start h).2.2.2).2.2.1
  intro j hj ⟨hq, hho⟩
  simp only [Run.holdsAbove, List.any_eq_false] at hh
  have := hh j hj
  simp [hq, hho] at this

/-- **G14.** A step that cannot be undone starts only when no step it rests on, through what it reads at any depth,
    has a question waiting for the person: none open, and none answered only by an agent. -/
theorem g14_irreversible {r : Run} {i : Nat} (h : r.next = .start i) (hirr : (r.get i).irreversible = true) :
    ∀ j ∈ r.reach (r.steps.length + 1) (r.get i).sources, r.awaitsPerson j = false :=
  rests_false r _ _ ((idleMove_start (next_start h).2.2.2).2.2.2.1 hirr)

/-- **G13**, the first half: a step runs again only when what it read is no longer what it would read now. -/
theorem g13_invalidate {r : Run} {i : Nat} (h : r.next = .invalidate i) :
    r.fresh i = false ∧ (r.get i).status = .done := by
  obtain ⟨_, _, hc⟩ := next_cases h (by simp)
  rcases hc with ⟨k0, _, he⟩ | ⟨_, ⟨_, he⟩ | ⟨k, hk, he⟩⟩
  · rcases owedMove_cases r k0 with h' | h' <;> rw [h'] at he <;> simp at he
  · rcases settled_cases r with h' | h' | h' | h' <;> rw [h'] at he <;> simp at he
  · obtain ⟨hnf, _⟩ := firstWhere_some hk
    rcases moveAt_cases r k with ⟨_, hm⟩ | ⟨hst, _, hm⟩ | ⟨_, hm⟩ | hm | ⟨_, hm | hm⟩ <;> rw [hm] at he
    · simp at he
    · simp at he
      subst he
      exact ⟨by simpa using hnf, hst⟩
    · rcases idleMove_cases r k with h' | h' | h' | h' | h' <;> rw [h'] at he <;> simp at he
    · simp at he
    · simp at he
    · simp at he

/-- **G14**, at most once: the core's next move never makes a step that cannot be undone do its act again. A done
    step that is not done on what holds now runs again only when it can be undone; one that cannot holds the run, and
    the person is asked. -/
theorem g14_never_redone {r : Run} {i : Nat} (h : r.next = .invalidate i) : (r.get i).irreversible = false := by
  obtain ⟨_, _, hc⟩ := next_cases h (by simp)
  rcases hc with ⟨k0, _, he⟩ | ⟨_, ⟨_, he⟩ | ⟨k, _, he⟩⟩
  · rcases owedMove_cases r k0 with h' | h' <;> rw [h'] at he <;> simp at he
  · rcases settled_cases r with h' | h' | h' | h' <;> rw [h'] at he <;> simp at he
  · rcases moveAt_cases r k with ⟨_, hm⟩ | ⟨_, hirr, hm⟩ | ⟨_, hm⟩ | hm | ⟨_, hm | hm⟩ <;> rw [hm] at he
    · simp at he
    · simp at he
      subst he
      exact hirr
    · rcases idleMove_cases r k with h' | h' | h' | h' | h' <;> rw [h'] at he <;> simp at he
    · simp at he
    · simp at he
    · simp at he

/-- **G14**, no recovery acts again: a step that acted in this intent and is not done (its checks did not pass after
    the act) is never started again by the core's move; only the person's order of a second act does. -/
theorem g14_acted_not_restarted {r : Run} {i : Nat} (h : r.next = .start i) : (r.get i).actedPending = false := by
  have hi := (next_start h).2.2.2
  unfold Run.idleMove at hi
  split at hi
  · split at hi <;> simp at hi
  split at hi
  · split at hi <;> simp at hi
  rename_i hap
  simpa using hap

/-- **G13**, the second half: the run delivers, finishes, or waits only for answers, only when every step of its
    path is done on what holds now. -/
theorem g13_settled {r : Run} (h : r.next = .deliver ∨ r.next = .finish ∨ r.next = .waitAnswers) :
    ∀ j ∈ r.path, r.fresh j = true := by
  have key : firstWhere (fun i => !r.fresh i) r.path = none := by
    rcases h with h | h | h <;>
    · obtain ⟨_, _, hc⟩ := next_cases h (by simp)
      rcases hc with ⟨k0, _, he⟩ | ⟨_, ⟨hz, _⟩ | ⟨k, _, he⟩⟩
      · rcases owedMove_cases r k0 with h' | h' <;> rw [h'] at he <;> simp at he
      · exact hz
      · exfalso
        rcases moveAt_cases r k with ⟨_, hm⟩ | ⟨_, _, hm⟩ | ⟨_, hm⟩ | hm | ⟨_, hm | hm⟩ <;> rw [hm] at he
        · simp at he
        · simp at he
        · rcases idleMove_cases r k with h' | h' | h' | h' | h' <;> rw [h'] at he <;> simp at he
        · simp at he
        · simp at he
        · simp at he
  intro j hj
  simpa using firstWhere_none key j hj

end Jaynes

namespace Jaynes

/-! ## Who answers -/

/-- **G9.** A rule never answers from outside the core. -/
theorem g9_no_rule (a : AnswerCase) (h : a.who = .rule) : a.accepts = false := by
  simp [AnswerCase.accepts, AnswerCase.acceptsActed, AnswerCase.acceptsMerge, h]

/-- **G9.** An agent's answer is taken only to let a result go on as it is, at a stop the role marks a release,
    never at the person's own step nor at a braked stop, never carrying context, and never over an answer. -/
theorem g9_agent (a : AnswerCase) (h : a.accepts = true) (ha : a.who = .agent) :
    a.decision = true ∧ a.personStep = false ∧ a.input = false ∧ a.release = true ∧ a.allKeep = true ∧
      a.braked = false ∧ a.hasContext = false ∧ a.answered = false := by
  unfold AnswerCase.accepts AnswerCase.acceptsActed AnswerCase.acceptsMerge at h
  simp only [ha] at h
  repeat' (split at h <;> try simp at h)
  all_goals simp_all

/-- **G10.** An answer is taken only for the question as it stands: not replaced, and as it was shown. -/
theorem g10_shown (a : AnswerCase) (h : a.accepts = true) : a.shownMatches = true ∧ a.superseded = false := by
  unfold AnswerCase.accepts AnswerCase.acceptsActed AnswerCase.acceptsMerge at h
  repeat' (split at h <;> try simp at h)
  all_goals simp_all

/-- **G14**, the person's order: what becomes of an act already done is the person's alone to decide, once. -/
theorem g14_acted_answer (a : AnswerCase) (hx : a.acted = true) (h : a.accepts = true) :
    a.who = .person ∧ a.answered = false := by
  unfold AnswerCase.accepts at h
  simp only [hx, ↓reduceIte] at h
  unfold AnswerCase.acceptsActed at h
  cases hw : a.who <;> cases ha : a.answered <;> simp_all

/-! ## What the person kept -/

/-- **G12.** An output passes the gate only when it keeps every item the person kept, unchanged, and brings back
    no item they dropped, by id or by content. Dropped is what stays dropped: an item the person brought back has
    left that list, so only the person undoes a drop. -/
theorem g12_kept (g : GateCase) (h : g.outcome = .done) :
    (∀ l ∈ g.locks, ∃ x ∈ g.items, x.id = l.id ∧ x.digest = l.digest) ∧
      (∀ d ∈ g.dropped, ∀ x ∈ g.items, x.id ≠ d.id ∧ x.digest ≠ d.digest) := by
  have hb : g.breaks = false := by
    unfold GateCase.outcome at h
    repeat' (split at h <;> try simp at h)
    all_goals simp_all
  simp only [GateCase.breaks, Bool.or_eq_false_iff] at hb
  obtain ⟨hl, hd⟩ := hb
  constructor
  · intro l hl'
    have := List.any_eq_false.mp hl l hl'
    simpa using this
  · intro d hd' x hx
    have := List.any_eq_false.mp hd d hd'
    simp only [Bool.not_eq_true, List.any_eq_false] at this
    have := this x hx
    simpa [not_or] using this

/-- **G2** and **G3.** An output passes only when every check passed; a hidden check that failed is never sent
    back to be fixed in place, where the agent would be told more than its sentence. -/
theorem g2_g3_gate (g : GateCase) :
    (g.outcome = .done → g.openFails = false ∧ g.hiddenFails = false ∧ g.blocked = false) ∧
      (g.outcome = .fix → g.hiddenFails = false) := by
  constructor
  · intro h
    unfold GateCase.outcome at h
    repeat' (split at h <;> try simp at h)
    all_goals simp_all
  · intro h
    unfold GateCase.outcome at h
    repeat' (split at h <;> try simp at h)
    all_goals simp_all

end Jaynes

namespace Jaynes

/-- **G17**, the rule: a stop owed a question skips asking only when its count is certified within its budget, it is
    not braked, and the run's coin falls outside its sample. A braked stop always asks. -/
theorem g17_skip {r : Run} {i : Nat} (h : r.next = .skip i) :
    (r.get i).certified = true ∧ (r.get i).braked = false ∧ (r.get i).coinOutside = true := by
  obtain ⟨_, _, hc⟩ := next_cases h (by simp)
  rcases hc with ⟨k, _, he⟩ | ⟨_, ⟨_, he⟩ | ⟨k, _, he⟩⟩
  · unfold Run.owedMove at he
    split at he
    · rename_i hs
      simp at he
      subst he
      simp at hs
      exact ⟨hs.1.1, hs.1.2, hs.2⟩
    · simp at he
  · rcases settled_cases r with h' | h' | h' | h' <;> rw [h'] at he <;> simp at he
  · rcases moveAt_cases r k with ⟨_, hm⟩ | ⟨_, _, hm⟩ | ⟨_, hm⟩ | hm | ⟨_, hm | hm⟩ <;> rw [hm] at he
    · simp at he
    · simp at he
    · rcases idleMove_cases r k with h' | h' | h' | h' | h' <;> rw [h'] at he <;> simp at he
    · simp at he
    · simp at he
    · simp at he

/-- **G11.** What an answer says will run again is what runs again: the same steps, when every item at the
    person's own step stands for a step, as the core makes them. -/
theorem g11_said_is_done (c : EffectCase)
    (hs : c.personStep = true → ∀ it ∈ c.items, it.standsFor.isSome = true) :
    ∀ x, x ∈ c.said ↔ x ∈ c.did := by
  intro x
  unfold EffectCase.said EffectCase.did EffectCase.targets
  by_cases hp : (c.permission || c.input || c.merge) = true
  · simp [hp]
  simp only [hp, Bool.false_eq_true, ↓reduceIte]
  by_cases hperson : c.personStep = true
  · have hall := hs hperson
    simp only [hperson, ↓reduceIte, Bool.true_and]
    by_cases hb : c.items.any (·.back) = true
    · obtain ⟨it, hit, hback⟩ := List.any_eq_true.mp hb
      obtain ⟨s, hsome⟩ := Option.isSome_iff_exists.mp (hall it hit)
      have hne : (c.items.filterMap fun it => if it.back = true then it.standsFor else none).isEmpty = false := by
        rw [List.isEmpty_eq_false_iff_exists_mem]
        exact ⟨s, List.mem_filterMap.mpr ⟨it, hit, by simp [hback, hsome]⟩⟩
      simp only [hne, Bool.not_false, ↓reduceIte, hb, List.mem_cons, List.mem_append, List.not_mem_nil, or_false]
      exact Or.comm
    · have hnone : (c.items.filterMap fun it => if it.back = true then it.standsFor else none) = [] := by
        rw [List.filterMap_eq_nil_iff]
        intro it hit
        have : it.back = false := by
          simp only [List.any_eq_true, not_exists, not_and, Bool.not_eq_true] at hb
          exact hb it hit
        simp [this]
      simp [hnone, hb]
  · have hperson' : c.personStep = false := by simpa using hperson
    simp only [hperson', Bool.false_and, Bool.false_eq_true, ↓reduceIte]
    by_cases hb : c.items.any (·.back) = true
    · simp only [hb, ↓reduceIte, List.mem_singleton, List.mem_filterMap]
      constructor
      · rintro ⟨it, _, h⟩
        split at h <;> simp_all
      · rintro rfl
        obtain ⟨it, hit, hback⟩ := List.any_eq_true.mp hb
        exact ⟨it, hit, by simp [hback]⟩
    · simp only [hb, Bool.false_eq_true, ↓reduceIte, List.not_mem_nil, iff_false, List.mem_filterMap, not_exists, not_and]
      intro it hit
      have : it.back = false := by
        simp only [List.any_eq_true, not_exists, not_and, Bool.not_eq_true] at hb
        exact hb it hit
      simp [this]

end Jaynes

namespace Jaynes

/-! ## G19: what lands on the person's branch -/

/-- **G19.** A delivery that merges into the person's branch and brings in what runs or is obeyed later outside any
    box goes out only once the person answered about this very delivery. -/
theorem g19_merge_waits {r : Run} (h : r.next = .deliver) (hh : r.mergeHeld = true) : r.mergeAnswered = true := by
  obtain ⟨_, _, hc⟩ := next_cases h (by simp)
  rcases hc with ⟨k0, _, he⟩ | ⟨_, ⟨_, he⟩ | ⟨k, _, he⟩⟩
  · rcases owedMove_cases r k0 with h' | h' <;> rw [h'] at he <;> simp at he
  · unfold Run.settled at he
    cases hd : r.deliveredCurrent <;> cases ha : r.mergeAnswered <;> cases hq : r.mergeAsked <;>
      cases ho : r.anyOpenQuestion <;> simp [hd, ha, hq, ho, hh] at he ⊢
  · exfalso
    rcases moveAt_cases r k with ⟨_, hm⟩ | ⟨_, _, hm⟩ | ⟨_, hm⟩ | hm | ⟨_, hm | hm⟩ <;> rw [hm] at he
    · simp at he
    · simp at he
    · rcases idleMove_cases r k with h' | h' | h' | h' | h' <;> rw [h'] at he <;> simp at he
    · simp at he
    · simp at he
    · simp at he

/-- **G19.** The answer about a merge is the person's alone, given once, about the delivery that would land: no
    agent and no rule gives it, and an answer to a question about an earlier delivery is not taken. -/
theorem g19_merge_answer (a : AnswerCase) (hm : a.merge = true) (hx : a.acted = false) (h : a.accepts = true) :
    a.who = .person ∧ a.current = true ∧ a.answered = false := by
  unfold AnswerCase.accepts at h
  simp only [hx, hm, ↓reduceIte, Bool.false_eq_true] at h
  unfold AnswerCase.acceptsMerge at h
  cases hw : a.who <;> simp_all

/-- **G11**, at G19's question: the answer about a merge, Merge or Branch only, says nothing will run again, and
    nothing does. -/
theorem g19_merge_answer_reruns_nothing (c : EffectCase) (h : c.merge = true) : c.said = [] ∧ c.did = [] := by
  simp [EffectCase.said, EffectCase.did, h]

/-! ## The merge's touch keeps the stops still open -/

/-- **G9** and **G19**, at the merge's touch: the stops it keeps are kept by the person, by the answer taken about the
    very delivery that would land, as it was shown, and only when it merges. -/
theorem g9_touch_is_the_persons (t : TouchCase) (h : t.kept ≠ []) :
    t.answer.who = .person ∧ t.answer.current = true ∧ t.answer.shownMatches = true ∧ t.lands = true := by
  unfold TouchCase.kept at h
  split at h
  · rename_i hc
    simp only [Bool.and_eq_true, Bool.not_eq_true'] at hc
    obtain ⟨⟨⟨hm, hx⟩, ha⟩, hl⟩ := hc
    unfold AnswerCase.accepts at ha
    simp only [hx, hm, ↓reduceIte, Bool.false_eq_true] at ha
    unfold AnswerCase.acceptsMerge at ha
    cases hw : t.answer.who <;> simp_all
  · simp at h

/-- **G19**, the other way out: branch only keeps nothing. -/
theorem g19_branch_only_keeps_nothing (t : TouchCase) (h : t.lands = false) : t.kept = [] := by
  simp [TouchCase.kept, h]

/-- **G17**: a stop that holds the run, a braked one among them, is never one the merge's touch keeps. -/
theorem g17_touch_never_keeps_what_holds (opens : List Open) (s : Nat) (h : s ∈ ridesOf opens) :
    ∃ o ∈ opens, o.step = s ∧ o.holds = false ∧ o.decision = true := by
  simp only [ridesOf, List.mem_map, List.mem_filter] at h
  obtain ⟨o, ⟨ho, hr⟩, hs⟩ := h
  simp only [Open.rides, Bool.and_eq_true, Bool.not_eq_true'] at hr
  exact ⟨o, ho, hs, hr.1.1.2, hr.1.1.1⟩

end Jaynes
