//! What a change touches raises the run's risk only from an attempt its checks accepted (the roles-engineer's reading
//! of r1795460919-451 and -455, 23/11: RiskRaised was written at AttemptDelivered, before ChecksJudged). With the
//! roles' check that the tests stay defined as they began, a fix that touches pyproject.toml without the person's
//! leave is refused, and the refused attempt must not raise the risk and bring the hidden cases and a second fix. The
//! agent's next attempt starts from what the refused one left (the tech-lead, 22/11), so nothing of it escapes: that
//! attempt is measured again, and raises the risk if it keeps the file and is accepted.

use jaynes_core::digest::Digest;
use jaynes_core::event::{Event, Usage};
use jaynes_core::manifest::{CheckDecl, CheckKind, DeliveryKind, Executor, Manifest, ProjectAccess, Reach, StepDecl};
use jaynes_core::risk::{Changed, Risk};
use jaynes_core::{decide, AgentTurn, AttemptOutcome, CheckResult, CheckVerdict, Command, Input, Run};
use std::collections::BTreeMap;

/// A fix judged by whether the project's tests stay defined as they began.
fn manifest() -> Manifest {
    let fix = StepDecl {
        name: "fix".into(),
        needs: Vec::new(),
        executor: Executor::Agent,
        session: Default::default(),
        reach: Reach { project: ProjectAccess::Write, write: vec!["**".into()], commands: true, ..Reach::default() },
        effect: Default::default(),
        checks: vec![CheckDecl { name: "tests-defined-as-they-began".into(), kind: CheckKind::Open, about: String::new(), reads: Vec::new(), project_tests: false }],
        stop: None,
        from_risk: Risk::Low,
        attempts: 3,
        fixes: 0,
        hidden: false,
        idempotent: false,
        model: Default::default(),
        turns: None,
        delivered: true,
        about: String::new(),
    };
    Manifest { role: "fix-bug".into(), version: "5.0.0".into(), protocol: 1, steps: vec![fix], target: Default::default(), delivery: DeliveryKind::Merge }
}

/// Feed `input`, answering every probe; the events it recorded and the commands that follow.
fn feed(run: &mut Run, input: Input) -> (Vec<Event>, Vec<Command>) {
    let out = decide(Some(run), &input).unwrap_or_else(|r| panic!("refused {input:?}: {r}"));
    let mut events = out.events.clone();
    for e in &out.events {
        run.apply(e);
    }
    let mut cmds = out.commands;
    while let Some(k) = cmds.iter().position(|c| matches!(c, Command::Probe { .. })) {
        let Command::Probe { step, .. } = cmds.remove(k) else { unreachable!() };
        let (more, next) = feed(run, Input::ProbeEnded { step, ok: true, reason: String::new() });
        events.extend(more);
        cmds.extend(next);
    }
    (events, cmds)
}

fn started() -> (Run, Vec<Command>) {
    let out = decide(None, &Input::Start {
        run: "r".into(),
        request: "the month's total leaves out its last day".into(),
        manifest: manifest(),
        role_fingerprint: Digest("f".into()),
        agent: "claude:m1".into(),
        raise: None,
        attention: BTreeMap::new(),
        task: None,
        arm: None,
        never_merge: false,
        starter: None,
    })
    .expect("start");
    let mut run = Run::fold(&out.events).expect("run");
    let mut cmds = out.commands;
    while let Some(k) = cmds.iter().position(|c| matches!(c, Command::Probe { .. })) {
        let Command::Probe { step, .. } = cmds.remove(k) else { unreachable!() };
        cmds.extend(feed(&mut run, Input::ProbeEnded { step, ok: true, reason: String::new() }).1);
    }
    (run, cmds)
}

fn turn(cmds: &[Command]) -> AgentTurn {
    cmds.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.clone()) } else { None }).unwrap_or_else(|| panic!("no turn: {cmds:?}"))
}

/// The fix's attempt delivers `changed`, and its check judges it: passed, or refused with the check's reason.
fn attempt(run: &mut Run, t: &AgentTurn, output: &str, changed: Vec<Changed>, passed: bool) -> (Vec<Event>, Vec<Command>) {
    let outcome = AttemptOutcome::Delivered { output: Digest(output.into()), items: Vec::new(), changed, usage: Usage::default(), session: Some("s".into()), tree: None, reply: None };
    let (mut events, cmds) = feed(run, Input::AttemptEnded { step: t.step.clone(), attempt: t.attempt, fix: 0, outcome });
    assert!(cmds.iter().any(|c| matches!(c, Command::RunChecks { .. })), "the check judges it: {cmds:?}");
    let verdict = if passed {
        CheckVerdict::Pass
    } else {
        CheckVerdict::Fail { reasons: vec!["the tests are no longer defined as they began: pyproject.toml".into()], detail: String::new(), cases: Vec::new() }
    };
    let results = vec![CheckResult { check: "tests-defined-as-they-began".into(), verdict, tree: None, tests_changed: Vec::new() }];
    let (more, cmds) = feed(run, Input::ChecksEnded { step: t.step.clone(), attempt: t.attempt, fix: 0, results });
    events.extend(more);
    (events, cmds)
}

fn raised(events: &[Event]) -> bool {
    events.iter().any(|e| matches!(e, Event::RiskRaised { .. }))
}

fn source() -> Changed {
    Changed::file("contas/relatorio.py".into(), 2)
}

fn pyproject() -> Changed {
    Changed::file("pyproject.toml".into(), 2)
}

#[test]
fn an_attempt_its_checks_refuse_raises_no_risk_and_the_next_one_that_keeps_the_file_does() {
    let (mut run, cmds) = started();
    let t = turn(&cmds);
    // (1) refused with pyproject.toml: the run stays at low risk, and nothing brings the hidden cases
    let (events, cmds) = attempt(&mut run, &t, "fix-1", vec![source(), pyproject()], false);
    assert!(events.iter().any(|e| matches!(e, Event::AttemptFailed { .. })), "refused: {events:?}");
    assert!(!raised(&events), "a refused attempt raises no risk: {events:?}");
    assert_eq!(run.risk, Risk::Low);
    // (2) the next attempt starts from what it left, keeps pyproject.toml and is accepted: the risk goes up, once
    // judged, before the step is done
    let t = turn(&cmds);
    let (events, _) = attempt(&mut run, &t, "fix-2", vec![source(), pyproject()], true);
    let at = |f: fn(&Event) -> bool| events.iter().position(f).unwrap_or_else(|| panic!("{events:?}"));
    let (judged, up, done) = (at(|e| matches!(e, Event::ChecksJudged { .. })), at(|e| matches!(e, Event::RiskRaised { .. })), at(|e| matches!(e, Event::StepDone { .. })));
    assert!(judged < up && up < done, "raised from the accepted attempt, after its checks: {events:?}");
    let Event::RiskRaised { to, reasons, .. } = &events[up] else { unreachable!() };
    assert_eq!((*to, reasons.len()), (Risk::High, 1));
    assert!(reasons[0].contains("pyproject.toml"), "{reasons:?}");
    assert_eq!(run.risk, Risk::High);
}

#[test]
fn an_attempt_that_takes_the_file_out_after_a_refused_one_raises_no_risk_and_the_file_does_not_land() {
    let (mut run, cmds) = started();
    let t = turn(&cmds);
    let (events, cmds) = attempt(&mut run, &t, "fix-1", vec![source(), pyproject()], false);
    assert!(!raised(&events), "{events:?}");
    // (3) the next attempt takes pyproject.toml out and is accepted: still low, and it is not in what lands
    let t = turn(&cmds);
    let (events, _) = attempt(&mut run, &t, "fix-2", vec![source()], true);
    assert!(events.iter().any(|e| matches!(e, Event::StepDone { .. })), "{events:?}");
    assert!(!raised(&events), "{events:?}");
    assert_eq!(run.risk, Risk::Low);
    let lands: Vec<String> = run.delivered_files().into_iter().map(|(c, _)| c.what).collect();
    assert_eq!(lands, ["contas/relatorio.py"]);
}
