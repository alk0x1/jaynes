import JaynesModel.Model
open Lean Jaynes

/-- Read cases, one JSON object per line: `{"run": …, "expected": "start 3"}`. Print each case the model decides
    differently, and how many agreed. Exit 1 on any difference. -/
def main : IO UInt32 := do
  let stdin ← IO.getStdin
  let mut agreed := 0
  let mut differed := 0
  let mut bad := 0
  repeat
    let line ← stdin.getLine
    if line.isEmpty then break
    if line.trimAscii.isEmpty then continue
    match Json.parse line >>= fun j => do
        let kind ← j.getObjValAs? String "kind"
        let expected ← j.getObjValAs? String "expected"
        let got ← match kind with
          | "move" => do pure (← j.getObjValAs? Run "run").next.render
          | "answer" => do pure (if (← j.getObjValAs? AnswerCase "case").accepts then "taken" else "refused")
          | "gate" => do pure (← j.getObjValAs? GateCase "case").outcome.render
          | other => (throw s!"no kind {other}" : Except String String)
        pure (got, expected) with
    | .error e =>
      bad := bad + 1
      if bad ≤ 3 then IO.eprintln s!"a case does not read: {e}"
    | .ok (got, expected) =>
      if got == expected then agreed := agreed + 1
      else
        differed := differed + 1
        if differed ≤ 5 then IO.eprintln s!"the model says {got}, the core said {expected}: {line.take 600}"
  IO.println s!"{agreed} agreed, {differed} differed, {bad} unreadable"
  return if differed == 0 && bad == 0 && agreed > 0 then 0 else 1
