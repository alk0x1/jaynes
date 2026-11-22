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
        model: Default::default(),
        turns: None, delivered: true,
    }
}

fn holding() -> StopDecl {
    StopDecl { settled: None, holds_unless: None, vouched_by: None,
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
        answers: Default::default(),
        note: String::new(),
        offers: Default::default(),
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



/// Feed an input, and answer every probe it asks for as passed, as the shell does: all events and other commands.
fn drive(run: &mut Run, input: Input) -> (Vec<Event>, Vec<Command>) {
    let (mut events, cmds) = feed(run, input);
    let mut out = Vec::new();
    let mut pending: std::collections::VecDeque<Command> = cmds.into();
    while let Some(c) = pending.pop_front() {
        if let Command::Probe { step, .. } = c {
            let (e, more) = feed(run, Input::ProbeEnded { step, ok: true, reason: String::new() });
            events.extend(e);
            pending.extend(more);
        } else {
            out.push(c);
        }
    }
    (events, out)
}

fn delivered(run: &mut Run, step: &str) -> (Vec<Event>, Vec<Command>) {
    let attempt = run.steps[run.manifest.index(step).expect("step")].attempt;
    drive(run, Input::AttemptEnded {
        step: step.into(),
        attempt,
        fix: 0,
        outcome: AttemptOutcome::Delivered { output: Digest(format!("{step}-out")), items: Vec::new(), changed: Vec::new(), usage: Usage::default(), session: None, tree: None, reply: None },
    })
}

fn started(steps: Vec<StepDecl>) -> Run {
    let manifest = Manifest { role: "r".into(), version: "1".into(), protocol: 1, steps, target: Default::default(), delivery: Default::default() };
    let out = decide(None, &Input::Start { run: "r".into(), request: "x".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None }).expect("start");
    let mut run = Run::fold(&out.events).expect("fold");
    for c in out.commands {
        if let Command::Probe { step, .. } = c {
            drive(&mut run, Input::ProbeEnded { step, ok: true, reason: String::new() });
        }
    }
    run
}

fn answer(run: &mut Run, step: &str, by: jaynes_core::question::By) -> (Vec<Event>, Vec<Command>) {
    let q = run.questions.values().find(|q| q.question.step == step && q.open()).expect("asked").question.clone();
    let a = jaynes_core::question::Answer { back: Default::default(), question: q.id.clone(), shown: q.shown(), verdicts: Default::default(), by, context: String::new() };
    drive(run, Input::Answer { answer: a })
}

/// A wait on an answer an agent gave in the person's place, which the person must still give, tells them.
#[test]
fn a_wait_on_an_answer_only_an_agent_gave_tells_the_person() {
    // make's stop does not hold and an agent may let it go on; other runs meanwhile; push cannot be undone and
    // needs both, so it waits for the person's own answer at make
    let mut make = step("make", Executor::Agent, &[]);
    make.stop = Some(StopDecl { holds: false, release: true, verdicts: vec![VerdictKind::Keep], ..holding() });
    let other = step("other", Executor::Agent, &[]);
    let mut push = step("push", Executor::Role, &["make", "other"]);
    push.effect = jaynes_core::manifest::Effect::Irreversible { idempotent: false };
    let mut run = started(vec![make, other, push]);
    let (_, cmds) = delivered(&mut run, "make");
    assert!(notices(&cmds).is_empty(), "make's stop holds nothing, and other runs: {cmds:?}");
    // an agent lets make go on before push is reached
    answer(&mut run, "make", jaynes_core::question::By::Agent { who: "claude".into() });
    // other is done: push waits for the person's own answer at make, and the person is never told
    let (events, cmds) = delivered(&mut run, "other");
    assert!(events.iter().any(|e| matches!(e, Event::Waiting { step: Some(s), .. } if s == "push")), "push waits for the person: {events:?}");
    assert_eq!(notices(&cmds).len(), 1, "the run waits for the person and does not tell them: {events:?} {cmds:?}");
}

/// A question the person was already told of is not told again because another one was answered.
#[test]
fn a_question_already_told_of_is_not_told_again_when_another_is_answered() {
    let mut a = step("a", Executor::Agent, &[]);
    a.stop = Some(StopDecl { holds: false, ..holding() });
    let mut b = step("b", Executor::Agent, &[]);
    b.stop = Some(StopDecl { holds: false, ..holding() });
    let mut run = started(vec![a, b]);
    delivered(&mut run, "a");
    // b is done, the run delivers, and waits on both questions: one notice
    let (_, cmds) = delivered(&mut run, "b");
    let mut pending = cmds;
    let mut told = notices(&pending).len();
    while let Some(c) = pending.pop() {
        if let Command::Deliver { digest, .. } = c {
            let (_, more) = drive(&mut run, Input::DeliveryEnded { digest, ok: true, reference: "branch".into(), merged: None });
            told += notices(&more).len();
        }
    }
    assert_eq!(told, 1, "delivered with two answers owed: one notice");
    // the person answers a; b's question, which they were told of, is told again
    let (events, cmds) = answer(&mut run, "a", jaynes_core::question::By::Person);
    assert!(notices(&cmds).is_empty(), "b's question was told once already, and is told again: {events:?} {cmds:?}");
}
