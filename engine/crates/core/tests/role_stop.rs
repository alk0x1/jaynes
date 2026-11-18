//! A stop on a step of Jaynes's own may send the work back to an agent's step it needs. The step of the role shows
//! what it made from that agent's work, such as the quotes of a note each in the page Jaynes downloaded; what the
//! person sends back goes to the agent that wrote the work, and the role's step makes its output again from what
//! comes back. The person's own steps still send nothing back, and every other rule of a stop holds as before.

use jaynes_core::digest::Digest;
use jaynes_core::event::{Event, Usage};
use jaynes_core::manifest::{Executor, Manifest, ProjectAccess, Rate, Reach, SessionKind, StepDecl, StopDecl};
use jaynes_core::question::{Answer, By, Question, Verdict, VerdictKind};
use jaynes_core::risk::Risk;
use jaynes_core::{decide, AttemptOutcome, CheckResult, CheckVerdict, Command, Input, Run};
use std::collections::BTreeMap;

fn step(name: &str, executor: Executor, needs: &[&str]) -> StepDecl {
    StepDecl {
        name: name.to_string(),
        needs: needs.iter().map(|s| s.to_string()).collect(),
        executor,
        session: SessionKind::Fresh,
        reach: if executor == Executor::Agent { Reach { project: ProjectAccess::Write, write: vec!["notes/**".into()], ..Reach::default() } } else { Reach::default() },
        effect: jaynes_core::manifest::Effect::None,
        checks: Vec::new(),
        stop: None,
        from_risk: Risk::Low,
        attempts: 3,
        fixes: 2,
        hidden: false, idempotent: false,
        about: String::new(),
    }
}

fn stop(redo: Option<&str>) -> StopDecl {
    StopDecl {
        verdicts: vec![VerdictKind::Keep, VerdictKind::Change],
        holds: false,
        from_risk: Risk::Low,
        release: false,
        budget: None,
        audit_rate: Rate(0.1),
        about: "Do the sources say what the note says?".into(),
        choose: false,
        redo: redo.map(str::to_string),
        rounds: None,
    }
}

/// read (an agent lists sources) → fetch (Jaynes downloads them) → note (an agent writes) → windows (Jaynes shows
/// each quote of the note in its page, and asks).
fn manifest(windows_stop: StopDecl) -> Manifest {
    let read = step("read", Executor::Agent, &[]);
    let fetch = step("fetch", Executor::Role, &["read"]);
    let note = step("note", Executor::Agent, &["read", "fetch"]);
    let mut windows = step("windows", Executor::Role, &["note", "fetch"]);
    windows.stop = Some(windows_stop);
    Manifest { role: "research".into(), version: "1".into(), protocol: 1, steps: vec![read, fetch, note, windows], target: Default::default(), delivery: Default::default() }
}

fn errors(m: &Manifest) -> String {
    m.validate().err().unwrap_or_default().join("\n")
}

#[test]
fn a_stop_of_jaynes_s_own_step_may_send_back_to_an_agent_s_step_it_needs() {
    assert_eq!(errors(&manifest(stop(Some("note")))), "", "a stop on a role's step sends back to the agent's note");
}

#[test]
fn where_it_sends_back_is_held_as_before() {
    // to a step of Jaynes's own, to a hidden step, to a step it does not need
    let e = errors(&manifest(stop(Some("fetch"))));
    assert!(e.contains("sends back to fetch"), "{e}");
    let mut hidden = manifest(stop(Some("note")));
    hidden.steps[2].hidden = true;
    hidden.steps[2].needs.clear();
    hidden.steps[3].needs = vec!["note".into(), "fetch".into()];
    assert!(errors(&hidden).contains("sends back to note"), "{}", errors(&hidden));
    let mut far = manifest(stop(Some("read")));
    far.steps[3].needs = vec!["note".into(), "fetch".into()];
    assert!(errors(&far).contains("sends back to read"), "{}", errors(&far));
}

#[test]
fn the_person_s_own_step_still_sends_nothing_back() {
    let mut m = manifest(stop(None));
    let mut look = step("look", Executor::Person, &["note"]);
    look.stop = Some(stop(Some("note")));
    m.steps.push(look);
    let e = errors(&m);
    assert!(e.contains("step look sends back to note"), "{e}");
}

#[test]
fn every_other_rule_of_a_stop_holds_on_a_step_of_jaynes_s_own_that_sends_back() {
    // released by an agent: allowed, as on any step of the role's, and still only with keep among its verdicts
    let mut released = stop(Some("note"));
    released.release = true;
    assert_eq!(errors(&manifest(released.clone())), "");
    released.verdicts = vec![VerdictKind::Change];
    assert!(errors(&manifest(released)).contains("can be released and offers no keep"));
    // an error budget: allowed, as on any step of the role's
    let mut budget = stop(Some("note"));
    budget.budget = Some(Rate(0.2));
    assert_eq!(errors(&manifest(budget)), "");
    // choosing among options: still only an agent's step makes them
    let mut choose = stop(Some("note"));
    choose.choose = true;
    choose.verdicts = vec![VerdictKind::Keep, VerdictKind::Change, VerdictKind::Drop];
    assert!(errors(&manifest(choose)).contains("only an agent's step makes them"));
}

// --- a run --------------------------------------------------------------------------------------------------

/// A world that answers every command at once: an agent's output is named by its step and how many times it ran;
/// a step of Jaynes's own makes its output from what it was given, so it comes out different only when that did.
struct World {
    run: Run,
    turns: BTreeMap<String, u32>,
    log: Vec<Event>,
    asked: Vec<Question>,
}

impl World {
    fn start(m: Manifest) -> World {
        let out = decide(None, &Input::Start {
            run: "r".into(),
            request: "a question".into(),
            manifest: m,
            role_fingerprint: Digest("f".into()),
            agent: "claude:m1".into(),
            raise: None,
            attention: BTreeMap::new(), task: None, arm: None, person: None,
        })
        .expect("start");
        let run = Run::fold(&out.events).expect("run");
        let mut w = World { run, turns: BTreeMap::new(), log: out.events, asked: Vec::new() };
        w.settle(out.commands);
        w
    }

    fn feed(&mut self, input: Input) -> Vec<Command> {
        let out = decide(Some(&self.run), &input).unwrap_or_else(|r| panic!("refused {input:?}: {r}"));
        for e in &out.events {
            self.run.apply(e);
            if let Event::QuestionOpened { question } = e {
                self.asked.push(question.clone());
            }
        }
        self.log.extend(out.events);
        out.commands
    }

    fn settle(&mut self, mut cmds: Vec<Command>) {
        while !cmds.is_empty() {
            let c = cmds.remove(0);
            let more = match c {
                Command::Probe { step, .. } => self.feed(Input::ProbeEnded { step, ok: true, reason: String::new() }),
                Command::RunAgent(t) => {
                    let n = self.turns.entry(t.step.clone()).or_default();
                    *n += 1;
                    let output = Digest(format!("{}-{n}", t.step));
                    self.feed(Input::AttemptEnded { step: t.step, attempt: t.attempt, fix: t.fix, outcome: AttemptOutcome::Delivered { output, items: Vec::new(), changed: Vec::new(), usage: Usage::default(), session: Some("s".into()) } })
                }
                Command::RunRole { step, attempt, given, .. } => {
                    *self.turns.entry(step.clone()).or_default() += 1;
                    let from: Vec<String> = given.iter().map(|r| r.output.0.clone()).collect();
                    let output = Digest(format!("{step}({})", from.join(",")));
                    self.feed(Input::AttemptEnded { step, attempt, fix: 0, outcome: AttemptOutcome::Delivered { output, items: Vec::new(), changed: Vec::new(), usage: Usage::default(), session: None } })
                }
                Command::RunChecks { step, attempt, fix, checks, .. } => {
                    let results = checks.into_iter().map(|check| CheckResult { check, verdict: CheckVerdict::Pass }).collect();
                    self.feed(Input::ChecksEnded { step, attempt, fix, results })
                }
                Command::Deliver { digest, .. } => self.feed(Input::DeliveryEnded { digest, ok: true, reference: "jaynes/r".into() }),
                Command::Ask { .. } | Command::Permit { .. } | Command::Notify { .. } => Vec::new(),
            };
            cmds.extend(more);
        }
    }

    fn send_back(&mut self, q: &Question, words: &str) {
        let verdicts = q.items.iter().map(|x| (x.id.clone(), Verdict::Change { text: words.into() })).collect();
        let cmds = self.feed(Input::Answer { answer: Answer { back: Default::default(), question: q.id.clone(), shown: q.shown(), verdicts, by: By::Person, context: String::new() } });
        self.settle(cmds);
    }
}

#[test]
fn what_the_person_sends_back_at_the_windows_goes_to_the_note_and_the_windows_are_made_again_from_it() {
    let mut w = World::start(manifest(stop(Some("note"))));
    let first = w.asked.last().cloned().expect("the windows ask");
    assert_eq!(first.step, "windows");
    assert_eq!(w.run.output(3).map(|d| d.0.clone()), Some("windows(fetch(read-1),note-1)".into()));
    w.send_back(&first, "S2 quotes the claim to deny it: drop that claim");
    // the note is written again with the person's words, and nothing before it runs again
    assert_eq!((w.turns["read"], w.turns["fetch"], w.turns["note"]), (1, 1, 2), "{:?}", w.turns);
    // the windows are made again from the new note, and ask again with them
    assert_eq!(w.turns["windows"], 2);
    assert_eq!(w.run.output(3).map(|d| d.0.clone()), Some("windows(fetch(read-1),note-2)".into()));
    let again = w.asked.last().cloned().expect("asked again");
    assert_eq!(again.step, "windows");
    assert_ne!(again.id, first.id, "a new question, about the new windows");
    assert!(w.log.iter().any(|e| matches!(e, Event::StepInvalidated { step, from: Some(q), .. } if step == "note" && *q == first.id)), "the note was sent back from the windows' question");
}

#[test]
fn keeping_at_the_windows_sends_nothing_back() {
    let mut w = World::start(manifest(stop(Some("note"))));
    let q = w.asked.last().cloned().expect("asked");
    let verdicts = q.items.iter().map(|x| (x.id.clone(), Verdict::Keep)).collect();
    let cmds = w.feed(Input::Answer { answer: Answer { back: Default::default(), question: q.id.clone(), shown: q.shown(), verdicts, by: By::Person, context: String::new() } });
    w.settle(cmds);
    assert_eq!((w.turns["note"], w.turns["windows"]), (1, 1));
    assert!(w.run.finished(), "kept: the run finishes");
}
