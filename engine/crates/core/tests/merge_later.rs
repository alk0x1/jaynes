//! A run whose role delivers on a branch of its own is merged later by the person, with one input (the real-user,
//! 22/11: "Finished: delivered", and the person's folder unchanged, the change only on jaynes/r…; nobody should need a
//! terminal or git to receive what they asked for). The merge opens the same stop every merge waits at (G19, Keep and
//! the files that land), and the record says whether it landed, so Undo and the summary read it from the chain.

use jaynes_core::digest::Digest;
use jaynes_core::event::{Event, Outcome, Usage};
use jaynes_core::manifest::{DeliveryKind, Effect, Executor, Manifest, ProjectAccess, Reach, SessionKind, StepDecl};
use jaynes_core::question::{Answer, By, QuestionKind, Verdict};
use jaynes_core::risk::{Changed, Risk};
use jaynes_core::run::Phase;
use jaynes_core::{decide, AttemptOutcome, Command, Input, Run};

thread_local! {
    /// Every event this thread's run recorded, in order: its record.
    static RECORD: std::cell::RefCell<Vec<Event>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn feed(run: &mut Run, input: Input) -> (Vec<Event>, Vec<Command>) {
    let out = decide(Some(run), &input).unwrap_or_else(|r| panic!("refused {input:?}: {r}"));
    for e in &out.events {
        run.apply(e);
    }
    RECORD.with(|r| r.borrow_mut().extend(out.events.iter().cloned()));
    (out.events, out.commands)
}

fn refused(run: &Run, input: Input) -> String {
    match decide(Some(run), &input) {
        Ok(out) => panic!("accepted {input:?}: {:?}", out.events),
        Err(r) => r.0,
    }
}

fn delivery(cmds: &[Command]) -> Option<(Digest, bool)> {
    cmds.iter().find_map(|c| match c {
        Command::Deliver { digest, merge, .. } => Some((digest.clone(), *merge)),
        _ => None,
    })
}

/// A run of one agent step that changes `files` in the project, delivered as `kind`, taken to its delivery: the run
/// and the delivery's digest, its `Deliver` command asked.
fn to_delivery(kind: DeliveryKind, files: &[&str]) -> (Run, Digest, bool) {
    let make = StepDecl {
        name: "change".into(),
        needs: Vec::new(),
        executor: Executor::Agent,
        session: SessionKind::Fresh,
        reach: Reach { project: ProjectAccess::Write, write: vec!["**".into()], ..Reach::default() },
        effect: Effect::None,
        checks: Vec::new(),
        stop: None,
        from_risk: Risk::Low,
        attempts: 3,
        fixes: 2,
        hidden: false,
        idempotent: false,
        about: String::new(),
        model: Default::default(),
        turns: None,
    };
    let manifest = Manifest { role: "small-change".into(), version: "1".into(), protocol: 1, steps: vec![make], target: Default::default(), delivery: kind };
    let out = decide(None, &Input::Start {
        run: "r".into(),
        request: "change the title".into(),
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
    RECORD.with(|r| *r.borrow_mut() = out.events.clone());
    for c in out.commands {
        if let Command::Probe { step, .. } = c {
            feed(&mut run, Input::ProbeEnded { step, ok: true, reason: String::new() });
        }
    }
    let attempt = run.steps[0].attempt;
    let changed = files.iter().map(|f| Changed { what: f.to_string(), size: 1, removed: 0, floor: false, digest: Some(format!("d-{f}")), holds: None, about: String::new() }).collect();
    let outcome = AttemptOutcome::Delivered { output: Digest("o".into()), items: Vec::new(), changed, usage: Usage::default(), session: None, tree: None, reply: None };
    let (_, cmds) = feed(&mut run, Input::AttemptEnded { step: "change".into(), attempt, fix: 0, outcome });
    // a role that merges waits at its merge first: no delivery is asked yet
    let (digest, merge) = delivery(&cmds).unwrap_or((Digest(String::new()), false));
    (run, digest, merge)
}

/// The run of [`to_delivery`] on a branch, delivered and finished.
fn on_a_branch(files: &[&str]) -> (Run, Digest) {
    let (mut run, digest, merge) = to_delivery(DeliveryKind::Branch, files);
    assert!(!merge, "a branch delivery merges nothing");
    feed(&mut run, Input::DeliveryEnded { digest: digest.clone(), ok: true, reference: "jaynes/r at 1234".into(), merged: None });
    assert!(run.finished(), "{:?}", run.phase);
    (run, digest)
}

fn keep_all(run: &Run, question: &str, by: By) -> Input {
    let q = &run.questions[question].question;
    Input::Answer { answer: Answer { back: Default::default(), question: q.id.clone(), shown: q.shown(), verdicts: q.items.iter().map(|x| (x.id.clone(), Verdict::Keep)).collect(), by, context: String::new() } }
}

fn merge_question(run: &Run) -> String {
    run.open_questions().iter().find(|q| q.question.kind == QuestionKind::Merge).map(|q| q.question.id.clone()).expect("the merge waits")
}

#[test]
fn the_person_merges_a_branch_delivery_at_the_stop_every_merge_waits_at() {
    let (mut run, digest) = on_a_branch(&["public/index.html"]);
    let (events, _) = feed(&mut run, Input::Merge { by: By::Person });
    assert!(events.iter().any(|e| matches!(e, Event::MergeAsked { by: By::Person })), "in the record: {events:?}");
    // the same stop as a merge role's: what lands, Keep or Drop, and the run waits on it
    let q = merge_question(&run);
    assert_eq!(run.questions[&q].question.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["public/index.html"]);
    assert!(matches!(&run.phase, Phase::Waiting { on, .. } if *on == vec![q.clone()]), "{:?}", run.phase);
    // a second Merge with it open opens no other
    assert!(refused(&run, Input::Merge { by: By::Person }).contains("already waits"));
    // Keep lands it: the delivery again, merged, and the run finished again, merged in the record
    let (_, cmds) = { let k = keep_all(&run, &q, By::Person); feed(&mut run, k) };
    assert_eq!(delivery(&cmds), Some((digest.clone(), true)), "{cmds:?}");
    let (events, cmds) = feed(&mut run, Input::DeliveryEnded { digest: digest.clone(), ok: true, reference: "jaynes/r at 1234; merged as abc".into(), merged: Some(true) });
    assert!(events.iter().any(|e| matches!(e, Event::Merged { digest: d } if *d == digest)), "{events:?}");
    assert!(delivery(&cmds).is_none(), "delivered once: {cmds:?}");
    assert!(run.merged && matches!(run.phase, Phase::Finished { outcome: Outcome::Delivered }), "{:?}", run.phase);
    // merged: no second merge, and Undo takes it back
    assert!(refused(&run, Input::Merge { by: By::Person }).contains("already merged"));
    let (events, _) = feed(&mut run, Input::Undo { note: String::new(), by: By::Person });
    assert!(events.iter().any(|e| matches!(e, Event::MergeUndone { .. })));
    assert!(refused(&run, Input::Merge { by: By::Person }).contains("undid"));
}

#[test]
fn a_merge_that_did_not_land_is_merged_again_with_one_input() {
    // a checkout with changes not committed: the delivery went out, nothing landed, and the person merges again
    let (mut run, digest) = on_a_branch(&["public/index.html"]);
    feed(&mut run, Input::Merge { by: By::Person });
    let q = merge_question(&run);
    { let k = keep_all(&run, &q, By::Person); feed(&mut run, k) };
    feed(&mut run, Input::DeliveryEnded { digest: digest.clone(), ok: true, reference: "jaynes/r at 1234; not merged: your checkout of main has uncommitted changes".into(), merged: Some(false) });
    assert!(!run.merged && run.finished(), "{:?}", run.phase);
    assert!(refused(&run, Input::Undo { note: String::new(), by: By::Person }).contains("merged nothing"), "nothing to undo");
    // kept already: merging again delivers at once, with no stop to answer again
    let (_, cmds) = feed(&mut run, Input::Merge { by: By::Person });
    assert_eq!(delivery(&cmds), Some((digest.clone(), true)), "{cmds:?}");
    feed(&mut run, Input::DeliveryEnded { digest, ok: true, reference: "jaynes/r at 1234; merged as abc".into(), merged: Some(true) });
    assert!(run.merged && run.finished());
}

#[test]
fn a_merge_dropped_at_its_stop_lands_nothing_and_may_be_asked_again() {
    let (mut run, _) = on_a_branch(&["public/index.html"]);
    feed(&mut run, Input::Merge { by: By::Person });
    let q = merge_question(&run);
    let item = run.questions[&q].question.items[0].id.clone();
    let shown = run.questions[&q].question.shown();
    let drop = Input::Answer { answer: Answer { back: Default::default(), question: q.clone(), shown, verdicts: [(item, Verdict::Drop)].into(), by: By::Person, context: String::new() } };
    let (_, cmds) = feed(&mut run, drop);
    assert!(delivery(&cmds).is_none(), "nothing lands: {cmds:?}");
    assert!(!run.merged && run.finished(), "{:?}", run.phase);
    // the person changes their mind: the stop asks again
    feed(&mut run, Input::Merge { by: By::Person });
    assert_ne!(merge_question(&run), q, "a new stop");
}

#[test]
fn a_merge_is_refused_saying_why_wherever_it_does_not_apply() {
    let (run, _) = on_a_branch(&["public/index.html"]);
    assert!(refused(&run, Input::Merge { by: By::Agent { who: "claude:1@x".into() } }).contains("only you merge"));
    assert!(refused(&run, Input::Merge { by: By::Rule { rule: "auto".into() } }).contains("only you merge"));
    // a run still going: its delivery is not made yet
    let (going, _, _) = to_delivery(DeliveryKind::Branch, &["public/index.html"]);
    assert!(refused(&going, Input::Merge { by: By::Person }).contains("not finished"));
    // a role that merges, merged at its own stop: the record says so, as it says Undo's
    let mut merging = to_delivery_merging();
    assert!(merging.merged, "a merge role's landing is in the record too");
    assert!(refused(&merging, Input::Merge { by: By::Person }).contains("already merged"));
    feed(&mut merging, Input::Undo { note: String::new(), by: By::Person });
    assert!(refused(&merging, Input::Merge { by: By::Person }).contains("undid"));
    // a delivery that changes nothing has nothing to merge
    let (empty, _) = on_a_branch(&[]);
    assert!(refused(&empty, Input::Merge { by: By::Person }).contains("changes nothing"));
    // abandoned before it delivered
    let (mut gone, _, _) = to_delivery(DeliveryKind::Branch, &["public/index.html"]);
    feed(&mut gone, Input::Abandon { by: By::Person, starter: None });
    assert!(refused(&gone, Input::Merge { by: By::Person }).contains("delivered nothing"));
}

#[test]
fn a_record_without_the_new_events_folds_as_it_did() {
    // a run of an engine before Merge: delivered on a branch, finished, and nothing says it merged
    let (run, _) = on_a_branch(&["public/index.html"]);
    let record = RECORD.with(|r| r.borrow().clone());
    let again = Run::fold(&record).expect("fold");
    assert_eq!(again, run, "the record folds to the same state");
    assert!(!again.merged && again.merge_asked.is_none() && again.finished());
    let old = serde_json::json!({"input": "delivery_ended", "digest": "x", "ok": true, "reference": "jaynes/r"});
    let read: Input = serde_json::from_value(old).expect("an old input reads");
    assert!(matches!(read, Input::DeliveryEnded { merged: None, .. }));
}

/// A run of a role that merges, kept at its merge stop and landed.
fn to_delivery_merging() -> Run {
    let (mut run, _, _) = to_delivery(DeliveryKind::Merge, &["public/index.html"]);
    let q = merge_question(&run);
    let (_, cmds) = { let k = keep_all(&run, &q, By::Person); feed(&mut run, k) };
    let (digest, merge) = delivery(&cmds).expect("delivers once kept");
    assert!(merge);
    feed(&mut run, Input::DeliveryEnded { digest, ok: true, reference: "jaynes/r; merged as abc".into(), merged: Some(true) });
    run
}
