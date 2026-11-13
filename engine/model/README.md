# The model of the core

The rules by which the core chooses its next move, takes an answer, and lets an output through its checks, written
a second time in Lean 4, with proofs of the guarantees that rest on them (`docs/guarantees.md`).

| File | What |
|---|---|
| `JaynesModel/Model.lean` | the model: `Run.next`, `AnswerCase.accepts`, `GateCase.outcome`, `EffectCase.said` and `did` |
| `JaynesModel/Proofs.lean` | G1, G2, G3, G4, G5, G9, G10, G11, G12, G13, G14 and the rule of G17, each named in its theorem |
| `Main.lean` | `check`: reads cases from the core and says where the model decides otherwise |

`lake build` checks every proof. `cargo test -p jaynes-core the_lean_model` builds the model, feeds it every state,
answer and pass through the checks of 800 random runs of the core, and requires the same decision each time. The
proofs use no `sorry`; `#print axioms` shows only Lean's standard `propext` and `Quot.sound`.

What is proved is about the model. That the core is the model rests on the comparison, which covers the states the
random runs reach, not every state. What the model takes as given: which steps a step reads, and the facts the core
computes about a question (such as whether its fingerprint matches).
