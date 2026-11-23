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
    let check = |name: &str| CheckDecl { name: name.into(), kind: CheckKind::Open, about: String::new(), reads: Vec::new(), project_tests: false };
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
    let outcome = AttemptOutcome::Delivered {
        output: Digest("o".into()),
        items: Vec::new(),
        changed: Vec::new(),
        usage: Usage::default(),
        session: None,
        tree: tree.map(str::to_string),
        reply: None,
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
        // the attempt is not spent: it waits at its checks for the person's continue, and the block says why
        assert!(events.iter().any(|e| matches!(e, Event::ChecksUnjudged { .. })), "{trees:?}: {events:?}");
        assert!(!events.iter().any(|e| matches!(e, Event::AttemptFailed { .. })), "{trees:?}: {events:?}");
        assert_eq!(run.counts.spent, 0);
        let reason = events.iter().find_map(|e| if let Event::Blocked { reason, .. } = e { Some(reason.clone()) } else { None }).expect("blocked");
        assert!(reason.contains("not the state of the project the step left (t1)"), "{reason}");
    }
}

#[test]
fn an_output_measured_at_no_tree_is_judged_as_before() {
    let (mut run, attempt) = checking(None);
    assert!(done(&judged(&mut run, attempt, [None, None])));
    let (mut run, attempt) = checking(None);
    assert!(done(&judged(&mut run, attempt, [Some("t9"), None])), "no tree to hold the checks to: their verdicts stand");
}

fn person() -> jaynes_core::question::By {
    jaynes_core::question::By::Person
}

fn verdicts(run: &mut Run, attempt: u32, verdict: CheckVerdict) -> Vec<Event> {
    let results = ["suite", "shape"].iter().map(|c| CheckResult { check: c.to_string(), verdict: verdict.clone(), tree: Some("t1".into()) }).collect();
    feed(run, Input::ChecksEnded { step: "make".into(), attempt, fix: 0, results }).0
}

/// A check that could not judge leaves the attempt at its checks, and the person's continue has them judge the
/// same output again: the agent does not work again, and no attempt is spent.
#[test]
fn continue_after_a_check_that_could_not_judge_checks_the_same_change_again_and_the_agent_does_not_work_again() {
    let (mut run, attempt) = checking(Some("t1"));
    let events = judged(&mut run, attempt, [Some("t1"), Some("t2")]);
    assert!(run.blocked(), "{events:?}");
    assert!(!events.iter().any(|e| matches!(e, Event::AttemptFailed { .. })), "the attempt waits at its checks, it did not fail: {events:?}");
    assert_eq!(run.checks_again(), Some(0), "the run knows a change waits to be checked again");
    assert_eq!(run.counts.spent, 0);
    let (events, commands) = feed(&mut run, Input::Continue { by: person(), starter: None });
    assert!(!events.iter().any(|e| matches!(e, Event::AttemptStarted { .. } | Event::ProbeAsked { .. })), "{events:?}");
    let again = commands.iter().find_map(|c| match c {
        Command::RunChecks { step, attempt: a, fix, output, .. } => Some((step.clone(), *a, *fix, output.clone())),
        _ => None,
    });
    assert_eq!(again, Some(("make".to_string(), attempt, 0, Digest("o".into()))), "{commands:?}");
    assert!(!commands.iter().any(|c| matches!(c, Command::RunAgent(_) | Command::Probe { .. })), "{commands:?}");
    assert_eq!(run.checks_again(), None, "the checks are under way again");
    let events = judged(&mut run, attempt, [Some("t1"), Some("t1")]);
    assert!(done(&events), "the same output passes: {events:?}");
    assert_eq!(run.steps[0].done.as_ref().map(|d| d.output.clone()), Some(Digest("o".into())));
    assert_eq!((run.counts.attempts, run.counts.spent), (1, 0), "{:?}", run.counts);
}

/// Judged again, an output that truly fails goes the way of today: the attempt is spent and a new one starts.
#[test]
fn a_change_that_fails_when_it_is_checked_again_spends_its_attempt_as_any_failure() {
    let (mut run, attempt) = checking(Some("t1"));
    verdicts(&mut run, attempt, CheckVerdict::Blocked { reason: "a port was refused".into() });
    assert!(run.blocked());
    feed(&mut run, Input::Continue { by: person(), starter: None });
    let events = verdicts(&mut run, attempt, CheckVerdict::Fail { reasons: vec!["it does not hold".into()], detail: String::new(), cases: vec![] });
    let spent = events.iter().find_map(|e| if let Event::AttemptFailed { spent, .. } = e { Some(*spent) } else { None });
    assert_eq!(spent, Some(true), "{events:?}");
    assert_eq!(run.counts.spent, 1);
    assert_eq!(run.steps[0].pending, None);
}

/// Only the person's continue has the checks judge again: a restart of the shell does not, and a verdict that
/// comes while the run waits for the person is a late report.
#[test]
fn nothing_but_the_person_s_continue_has_the_checks_judge_again() {
    let (mut run, attempt) = checking(Some("t1"));
    verdicts(&mut run, attempt, CheckVerdict::Blocked { reason: "the tool did not start".into() });
    let (events, commands) = feed(&mut run, Input::Recovered);
    assert!(!commands.iter().any(|c| matches!(c, Command::RunChecks { .. })), "a restart judged again: {commands:?} {events:?}");
    assert!(run.blocked());
    let events = verdicts(&mut run, attempt, CheckVerdict::Pass);
    assert!(matches!(events.as_slice(), [Event::StaleReport { .. }]), "{events:?}");
    assert!(run.blocked() && run.checks_again() == Some(0));
    let refused = decide(Some(&run), &Input::Continue { by: jaynes_core::question::By::Agent { who: "claude".into() }, starter: None });
    assert!(refused.is_err(), "an agent continued: {refused:?}");
}

/// What the person chose for the project's ports after the start is a fact of the record, with when, and the
/// setup stays as it was: the same choice twice is nothing new.
#[test]
fn the_project_s_ports_opened_or_closed_after_the_start_are_recorded_with_when() {
    let (mut run, _) = checking(Some("t1"));
    assert_eq!(run.ports, None, "as the run's setup says");
    let (events, _) = feed(&mut run, Input::PortsChosen { any: true, servers: false, opened: 1_700_000_000, freed_ports: vec![] });
    assert!(matches!(events.as_slice(), [Event::PortsChosen { any: true, servers: false, opened: 1_700_000_000, .. }]), "{events:?}");
    assert_eq!(run.ports, Some(jaynes_core::run::Ports { any: true, servers: false, opened: 1_700_000_000, freed_ports: vec![] }));
    assert!(decide(Some(&run), &Input::PortsChosen { any: true, servers: false, opened: 1_700_000_000, freed_ports: vec![] }).is_err(), "the same choice again");
    let (events, _) = feed(&mut run, Input::PortsChosen { any: false, servers: false, opened: 0, freed_ports: vec![] });
    assert!(matches!(events.as_slice(), [Event::PortsChosen { any: false, .. }]), "{events:?}");
    assert_eq!(run.ports, Some(jaynes_core::run::Ports { any: false, servers: false, opened: 0, freed_ports: vec![] }));
    // the test servers are a choice of their own, and closing takes them out too
    let (events, _) = feed(&mut run, Input::PortsChosen { any: false, servers: true, opened: 1_700_000_100, freed_ports: vec![] });
    assert!(matches!(events.as_slice(), [Event::PortsChosen { any: false, servers: true, opened: 1_700_000_100, .. }]), "{events:?}");
    assert_eq!(run.ports.as_ref().map(|p| p.servers), Some(true));
    // a port Jaynes holds for them, let to the project's tests, is a choice of its own, each port once
    let (events, _) = feed(&mut run, Input::PortsChosen { any: false, servers: true, opened: 1_700_000_100, freed_ports: vec![6379, 5432, 6379] });
    assert!(matches!(events.as_slice(), [Event::PortsChosen { servers: true, freed_ports, .. }] if freed_ports == &[5432, 6379]), "{events:?}");
    assert!(decide(Some(&run), &Input::PortsChosen { any: false, servers: true, opened: 1_700_000_100, freed_ports: vec![5432, 6379] }).is_err(), "the same choice again");
    // and stands only with the test servers
    let (events, _) = feed(&mut run, Input::PortsChosen { any: true, servers: false, opened: 1_700_000_200, freed_ports: vec![5432] });
    assert!(matches!(events.as_slice(), [Event::PortsChosen { freed_ports, .. }] if freed_ports.is_empty()), "{events:?}");
    let (_, _) = feed(&mut run, Input::PortsChosen { any: false, servers: false, opened: 0, freed_ports: vec![] });
    assert_eq!(run.ports, Some(jaynes_core::run::Ports { any: false, servers: false, opened: 0, freed_ports: vec![] }));
    // a record of before the test servers reads as it was, and an event without them is written as before
    let old: Event = serde_json::from_str(r#"{"type":"PortsChosen","any":true,"opened":5}"#).expect("old event");
    assert_eq!(old, Event::PortsChosen { any: true, servers: false, opened: 5, freed_ports: vec![] });
    assert!(!serde_json::to_string(&old).expect("json").contains("servers"));
    assert!(!serde_json::to_string(&old).expect("json").contains("freed_ports"));
}
