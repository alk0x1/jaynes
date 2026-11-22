//! Waiting for the person tells them: every new wait on an answer of theirs comes with a notice, once; the same wait
//! again tells nothing more.

use jaynes_core::digest::Digest;
use jaynes_core::event::{Event, Usage};
use jaynes_core::manifest::{Executor, Manifest, Rate, Reach, SessionKind, StepDecl, StopDecl};
use jaynes_core::question::VerdictKind;
use jaynes_core::risk::Risk;
use jaynes_core::{decide, AttemptOutcome, Command, Input, Run};

fn step(name: &str, executor: Executor, needs: &[&str]) -> StepDecl {
    StepDecl {
        name: name.to_string(),
        needs: needs.iter().map(|s| s.to_string()).collect(),
        executor,
        session: SessionKind::Fresh,
        reach: Reach::default(),
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

fn holding() -> StopDecl {
    StopDecl {
        verdicts: vec![VerdictKind::Keep, VerdictKind::Redo],
        holds: true,
        from_risk: Risk::Low,
        release: false,
        budget: None,
        audit_rate: Rate(0.1),
        about: "Is it right?".into(),
        choose: false,
        redo: None,
        rounds: None,
    }
}

fn feed(run: &mut Run, input: Input) -> (Vec<Event>, Vec<Command>) {
    let out = decide(Some(run), &input).unwrap_or_else(|r| panic!("refused {input:?}: {r}"));
    for e in &out.events {
        run.apply(e);
    }
    (out.events, out.commands)
}

fn notices(cmds: &[Command]) -> Vec<String> {
    cmds.iter().filter_map(|c| if let Command::Notify { text } = c { Some(text.clone()) } else { None }).collect()
}

/// A run whose `make` step is an agent's with a stop that holds, and `look` the person's own step after it.
fn run() -> (Run, Vec<Command>) {
    let mut make = step("make", Executor::Agent, &[]);
    make.stop = Some(holding());
    let mut look = step("look", Executor::Person, &["make"]);
    look.stop = Some(holding());
    let manifest = Manifest { role: "r".into(), version: "1".into(), protocol: 1, steps: vec![make, look], target: Default::default(), delivery: Default::default() };
    let out = decide(None, &Input::Start {
        run: "r".into(),
        request: "x".into(),
        manifest,
        role_fingerprint: Digest("f".into()),
        agent: "claude:m".into(),
        raise: None,
        attention: Default::default(), task: None, arm: None, never_merge: false,
        starter: None,
    })
    .expect("start");
    let mut run = Run::fold(&out.events).expect("fold");
    let mut cmds = Vec::new();
    for c in out.commands {
        if let Command::Probe { step, .. } = c {
            cmds.extend(feed(&mut run, Input::ProbeEnded { step, ok: true, reason: String::new() }).1);
        } else {
            cmds.push(c);
        }
    }
    (run, cmds)
}

#[test]
fn a_wait_for_the_person_tells_them_once() {
    let (mut run, cmds) = run();
    assert!(notices(&cmds).is_empty(), "nothing waits for the person yet: {cmds:?}");
    let attempt = run.steps[0].attempt;
    let (_, cmds) = feed(&mut run, Input::AttemptEnded {
        step: "make".into(),
        attempt,
        fix: 0,
        outcome: AttemptOutcome::Delivered { output: Digest("o".into()), items: Vec::new(), changed: Vec::new(), usage: Usage::default(), session: None, tree: None, reply: None },
    });
    let told = notices(&cmds);
    assert_eq!(told.len(), 1, "the wait at make's stop tells the person: {cmds:?}");
    // the same wait again tells nothing more
    let (_, cmds) = feed(&mut run, Input::Tick);
    assert!(notices(&cmds).is_empty(), "{cmds:?}");
}

#[test]
fn a_new_wait_tells_the_person_of_the_questions_not_told_of_before_and_only_of_them() {
    let (mut run, _) = run();
    let attempt = run.steps[0].attempt;
    feed(&mut run, Input::AttemptEnded {
        step: "make".into(),
        attempt,
        fix: 0,
        outcome: AttemptOutcome::Delivered { output: Digest("o".into()), items: Vec::new(), changed: Vec::new(), usage: Usage::default(), session: None, tree: None, reply: None },
    });
    // the person answers make's stop; the run now waits on another answer of theirs, look's, and says so again
    let q = run.questions.values().find(|q| q.question.step == "make" && q.open()).expect("make asks").question.clone();
    let answer = jaynes_core::question::Answer { back: Default::default(), question: q.id.clone(), shown: q.shown(), verdicts: Default::default(), by: jaynes_core::question::By::Person, context: String::new() };
    let told_before = run.told.clone();
    let (events, cmds) = feed(&mut run, Input::Answer { answer });
    // a notice for each new wait on a question not told of before, and none for one already told
    let new = events.iter().filter(|e| matches!(e, Event::Waiting { on, .. } if on.iter().any(|id| !told_before.contains(id)))).count();
    assert_eq!(notices(&cmds).len(), new, "{events:?} {cmds:?}");
}
