//! A step passes only when every check judged the state of the project its output was measured at: a check that
//! judged another state, or none where there was one, could not judge the output, and no attempt is spent.

use jaynes_core::digest::Digest;
use jaynes_core::event::{Event, Usage};
use jaynes_core::manifest::{CheckDecl, CheckKind, Executor, Manifest, Reach, SessionKind, StepDecl};
use jaynes_core::risk::Risk;
use jaynes_core::{decide, AttemptOutcome, CheckResult, CheckVerdict, Command, Input, Run};

fn feed(run: &mut Run, input: Input) -> (Vec<Event>, Vec<Command>) {
    let out = decide(Some(run), &input).unwrap_or_else(|r| panic!("refused {input:?}: {r}"));
    for e in &out.events {
        run.apply(e);
    }
    (out.events, out.commands)
}

/// A run of one agent's step with two open checks, whose attempt delivered an output measured at `tree`.
fn checking(tree: Option<&str>) -> (Run, u32) {
    let check = |name: &str| CheckDecl { name: name.into(), kind: CheckKind::Open, about: String::new(), reads: Vec::new() };
    let make = StepDecl {
        name: "make".into(),
        needs: Vec::new(),
        executor: Executor::Agent,
        session: SessionKind::Fresh,
        reach: Reach::default(),
        effect: jaynes_core::manifest::Effect::None,
        checks: vec![check("suite"), check("shape")],
        stop: None,
        from_risk: Risk::Low,
        attempts: 3,
        fixes: 2,
        hidden: false,
        idempotent: false,
        about: String::new(),
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
    })
    .expect("start");
    let mut run = Run::fold(&out.events).expect("fold");
    for c in out.commands {
        if let Command::Probe { step, .. } = c {
            feed(&mut run, Input::ProbeEnded { step, ok: true, reason: String::new() });
        }
    }
    let attempt = run.steps[0].attempt;
    let outcome = AttemptOutcome::Delivered {
        output: Digest("o".into()),
        items: Vec::new(),
        changed: Vec::new(),
        usage: Usage::default(),
        session: None,
        tree: tree.map(str::to_string),
    };
    let (events, cmds) = feed(&mut run, Input::AttemptEnded { step: "make".into(), attempt, fix: 0, outcome });
    let recorded = events.iter().find_map(|e| if let Event::AttemptDelivered { tree, .. } = e { Some(tree.clone()) } else { None });
    assert_eq!(recorded, Some(tree.map(str::to_string)), "the attempt records the tree it was measured at");
    assert!(cmds.iter().any(|c| matches!(c, Command::RunChecks { .. })), "{cmds:?}");
    (run, attempt)
}

fn judged(run: &mut Run, attempt: u32, trees: [Option<&str>; 2]) -> Vec<Event> {
    let results = ["suite", "shape"]
        .iter()
        .zip(trees)
        .map(|(c, t)| CheckResult { check: c.to_string(), verdict: CheckVerdict::Pass, tree: t.map(str::to_string) })
        .collect();
    feed(run, Input::ChecksEnded { step: "make".into(), attempt, fix: 0, results }).0
}

fn done(events: &[Event]) -> bool {
    events.iter().any(|e| matches!(e, Event::StepDone { .. }))
}

#[test]
fn a_step_passes_when_every_check_judged_the_tree_its_output_was_measured_at() {
    let (mut run, attempt) = checking(Some("t1"));
    let events = judged(&mut run, attempt, [Some("t1"), Some("t1")]);
    assert!(done(&events), "{events:?}");
    let records = events.iter().find_map(|e| if let Event::ChecksJudged { checks, .. } = e { Some(checks.clone()) } else { None }).expect("judged");
    assert!(records.iter().all(|r| r.tree.as_deref() == Some("t1") && r.passed), "each check records the tree it judged: {records:?}");
}

#[test]
fn a_check_that_judged_another_tree_or_none_could_not_judge_the_output() {
    for trees in [[Some("t1"), Some("t2")], [None, Some("t1")]] {
        let (mut run, attempt) = checking(Some("t1"));
        let events = judged(&mut run, attempt, trees);
        assert!(!done(&events), "{trees:?}: {events:?}");
        let records = events.iter().find_map(|e| if let Event::ChecksJudged { checks, .. } = e { Some(checks.clone()) } else { None }).expect("judged");
        assert!(records.iter().any(|r| r.blocked && !r.passed), "{trees:?}: the check is recorded as one that could not judge: {records:?}");
        let failed = events.iter().find_map(|e| if let Event::AttemptFailed { reasons, spent, .. } = e { Some((reasons.clone(), *spent)) } else { None });
        let (reasons, spent) = failed.expect("the attempt ends unspent");
        assert!(!spent, "{events:?}");
        assert!(reasons.iter().any(|r| r.contains("not the state of the project the step left (t1)")), "{reasons:?}");
    }
}

#[test]
fn an_output_measured_at_no_tree_is_judged_as_before() {
    let (mut run, attempt) = checking(None);
    assert!(done(&judged(&mut run, attempt, [None, None])));
    let (mut run, attempt) = checking(None);
    assert!(done(&judged(&mut run, attempt, [Some("t9"), None])), "no tree to hold the checks to: their verdicts stand");
}
