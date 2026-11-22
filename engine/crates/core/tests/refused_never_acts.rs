//! A command that is refused never acts, and an abandon says what it may have left done: a delivery that failed waits
//! for the input that is accepted (the person's continue, or their resume of a halted run), never for any process
//! that happens to start; and abandoning a run while a step that acts on something real is under way records that
//! the step may have acted (G14).

use jaynes_core::digest::Digest;
use jaynes_core::event::{Event, Usage};
use jaynes_core::manifest::{Effect, Executor, Manifest, Reach, SessionKind, StepDecl};
use jaynes_core::question::By;
use jaynes_core::risk::Risk;
use jaynes_core::{decide, AttemptOutcome, Command, Input, Run};

fn feed(run: &mut Run, input: Input) -> (Vec<Event>, Vec<Command>) {
    let out = decide(Some(run), &input).unwrap_or_else(|r| panic!("refused {input:?}: {r}"));
    for e in &out.events {
        run.apply(e);
    }
    (out.events, out.commands)
}

fn delivers(cmds: &[Command]) -> bool {
    cmds.iter().any(|c| matches!(c, Command::Deliver { .. }))
}

/// A run of one step with no check and no stop, whose effect is `effect` (a role's own step when it acts), under way.
fn running(effect: Effect) -> (Run, u32) {
    let make = StepDecl {
        name: "make".into(),
        needs: Vec::new(),
        // only a role's own step acts on something real
        executor: if effect.acts() { Executor::Role } else { Executor::Agent },
        session: SessionKind::Fresh,
        reach: Reach::default(),
        effect,
        checks: Vec::new(),
        stop: None,
        from_risk: Risk::Low,
        attempts: 3,
        fixes: 2,
        hidden: false,
        idempotent: false,
        about: String::new(),
        model: Default::default(),
        turns: None, delivered: true,
    };
    let manifest = Manifest { role: "r".into(), version: "1".into(), protocol: 1, steps: vec![make], target: Default::default(), delivery: Default::default() };
    let out = decide(None, &Input::Start {
        run: "r".into(),
        request: "x".into(),
        manifest,
        role_fingerprint: Digest("f".into()),
        agent: "claude:m".into(),
        raise: None,
        attention: Default::default(),
        task: None,
        arm: None,
        never_merge: false,
        starter: None,
    })
    .expect("start");
    let mut run = Run::fold(&out.events).expect("fold");
    for c in out.commands {
        if let Command::Probe { step, .. } = c {
            feed(&mut run, Input::ProbeEnded { step, ok: true, reason: String::new() });
        }
    }
    let attempt = run.steps[0].attempt;
    (run, attempt)
}

/// The run of [`running`], its step done and its delivery asked: the digest of the delivery under way.
fn delivering() -> (Run, Digest) {
    let (mut run, attempt) = running(Effect::None);
    let outcome = AttemptOutcome::Delivered { output: Digest("o".into()), items: Vec::new(), changed: Vec::new(), usage: Usage::default(), session: None, tree: None, reply: None };
    let (_, cmds) = feed(&mut run, Input::AttemptEnded { step: "make".into(), attempt, fix: 0, outcome });
    assert!(delivers(&cmds), "the delivery is asked: {cmds:?}");
    let digest = run.delivering.clone().expect("delivering");
    (run, digest)
}

#[test]
fn a_delivery_that_failed_waits_for_the_person_s_continue_and_no_process_that_starts_makes_it() {
    let (mut run, digest) = delivering();
    let (events, _) = feed(&mut run, Input::DeliveryEnded { digest, ok: false, reference: "the copy changed".into(), merged: None });
    assert!(events.iter().any(|e| matches!(e, Event::Blocked { .. })), "{events:?}");
    // a process that starts (a continue that will be refused, a page, a crash) recovers first: it delivers nothing
    let (_, cmds) = feed(&mut run, Input::Recovered);
    assert!(!delivers(&cmds), "a process that started delivered without the person: {cmds:?}");
    assert!(decide(Some(&run), &Input::Continue { by: By::Agent { who: "an agent".into() }, starter: None }).is_err(), "an agent's continue is refused");
    // the person's continue is accepted, and it alone delivers again
    let (_, cmds) = feed(&mut run, Input::Continue { by: By::Person, starter: None });
    assert!(delivers(&cmds), "the person's continue delivers again: {cmds:?}");
}

#[test]
fn a_delivery_under_way_when_the_person_halted_waits_for_their_resume() {
    let (mut run, _) = delivering();
    feed(&mut run, Input::Halt { by: By::Person });
    let (_, cmds) = feed(&mut run, Input::Recovered);
    assert!(!delivers(&cmds), "a halted run delivered: {cmds:?}");
    let (_, cmds) = feed(&mut run, Input::Resume { by: By::Person });
    assert!(delivers(&cmds), "the resume delivers what was under way: {cmds:?}");
}

#[test]
fn a_delivery_under_way_when_jaynes_stopped_is_made_when_it_starts_again() {
    let (mut run, _) = delivering();
    let (_, cmds) = feed(&mut run, Input::Recovered);
    assert!(delivers(&cmds), "{cmds:?}");
}

#[test]
fn abandoning_a_run_while_a_step_that_acts_is_under_way_records_that_it_may_have_acted() {
    for effect in [Effect::Irreversible { idempotent: false }, Effect::Irreversible { idempotent: true }, Effect::Compensable { undo: "revert".into() }] {
        let (mut run, attempt) = running(effect.clone());
        let (events, _) = feed(&mut run, Input::Abandon { by: By::Person, starter: None });
        let acted = events.iter().position(|e| matches!(e, Event::MayHaveActed { step, attempt: a } if step == "make" && *a == attempt));
        let finished = events.iter().position(|e| matches!(e, Event::RunFinished { .. }));
        assert!(matches!(run.steps[0].status, jaynes_core::run::Status::Running { .. }) || acted.is_some(), "{effect:?}: the step was not under way");
        assert!(matches!((acted, finished), (Some(a), Some(f)) if a < f), "{effect:?}: {events:?}");
        assert!(run.steps[0].maybe_acted, "{effect:?}");
    }
    // a step that acts on nothing real leaves nothing to say
    let (mut run, _) = running(Effect::None);
    let (events, _) = feed(&mut run, Input::Abandon { by: By::Person, starter: None });
    assert!(!events.iter().any(|e| matches!(e, Event::MayHaveActed { .. })), "{events:?}");
}
