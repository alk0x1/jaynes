//! A step a run at low risk does not take, read by a check of a step it does take: fix-bug's hidden cases only from
//! high risk, as the tech-lead cut the cost of a small fix (22/11, line 32 of the board). The check that reads them
//! runs only once they are there; a run whose risk goes up under the fix takes the cases and fixes again, judged by
//! them; and a promise that rests on what a low run skips is proved on the runs that take it only when its role says
//! what holds on the others.

use jaynes_core::digest::Digest;
use jaynes_core::event::Usage;
use jaynes_core::manifest::{CheckDecl, CheckKind, DeliveryKind, Executor, Manifest, ProjectAccess, Reach, StepDecl};
use jaynes_core::promise::{prove, Condition, Form, On, Promise, Proof};
use jaynes_core::risk::{Changed, Risk};
use jaynes_core::{decide, AgentTurn, AttemptOutcome, CheckResult, CheckVerdict, Command, Input, Run};
use std::collections::BTreeMap;

fn step(name: &str, needs: &[&str]) -> StepDecl {
    StepDecl {
        name: name.into(),
        needs: needs.iter().map(|s| s.to_string()).collect(),
        executor: Executor::Agent,
        session: Default::default(),
        reach: Reach::default(),
        effect: Default::default(),
        checks: Vec::new(),
        stop: None,
        from_risk: Risk::Low,
        attempts: 3,
        fixes: 2,
        hidden: false,
        idempotent: false,
        model: Default::default(),
        turns: None,
        about: String::new(),
    }
}

/// fix-bug cut short: the cases only from high risk, the fix judged by its tests and by the cases.
fn fix_bug() -> Manifest {
    let mut cases = step("cases", &[]);
    cases.hidden = true;
    cases.from_risk = Risk::High;
    cases.reach = Reach { project: ProjectAccess::Read, commands: true, ..Reach::default() };
    let mut fix = step("fix", &[]);
    fix.reach = Reach { project: ProjectAccess::Write, write: vec!["**".into()], commands: true, ..Reach::default() };
    fix.checks = vec![
        CheckDecl { name: "tests-pass".into(), kind: CheckKind::Open, about: String::new(), reads: Vec::new() },
        CheckDecl { name: "proof".into(), kind: CheckKind::Hidden, about: String::new(), reads: vec!["cases".into()] },
    ];
    Manifest { role: "fix-bug".into(), version: "5.0.0".into(), protocol: 1, steps: vec![cases, fix], target: Default::default(), delivery: DeliveryKind::Merge }
}

fn feed(run: &mut Run, input: Input) -> Vec<Command> {
    let out = decide(Some(run), &input).unwrap_or_else(|r| panic!("refused {input:?}: {r}"));
    for e in &out.events {
        run.apply(e);
    }
    let mut cmds = out.commands;
    while let Some(k) = cmds.iter().position(|c| matches!(c, Command::Probe { .. })) {
        let Command::Probe { step, .. } = cmds.remove(k) else { unreachable!() };
        let more = decide(Some(run), &Input::ProbeEnded { step, ok: true, reason: String::new() }).expect("probe");
        for e in &more.events {
            run.apply(e);
        }
        cmds.extend(more.commands);
    }
    cmds
}

fn started(raise: Option<Risk>) -> (Run, Vec<Command>) {
    let out = decide(None, &Input::Start {
        run: "r".into(),
        request: "the total is wrong".into(),
        manifest: fix_bug(),
        role_fingerprint: Digest("f".into()),
        agent: "claude:m1".into(),
        raise,
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
        cmds.extend(feed(&mut run, Input::ProbeEnded { step, ok: true, reason: String::new() }));
    }
    (run, cmds)
}

fn turn(cmds: &[Command]) -> AgentTurn {
    cmds.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.clone()) } else { None }).unwrap_or_else(|| panic!("no turn: {cmds:?}"))
}

/// The turn's agent delivers `changed`; the commands that follow, the checks the core asks for among them.
fn deliver(run: &mut Run, t: &AgentTurn, output: &str, changed: Vec<Changed>) -> Vec<Command> {
    feed(run, Input::AttemptEnded {
        step: t.step.clone(),
        attempt: t.attempt,
        fix: 0,
        outcome: AttemptOutcome::Delivered { output: Digest(output.into()), items: Vec::new(), changed, usage: Usage::default(), session: Some("s".into()), tree: None, reply: None },
    })
}

/// The checks the commands ask to run, and on what.
fn asked_checks(cmds: &[Command]) -> (Vec<String>, Vec<String>) {
    cmds.iter()
        .find_map(|c| match c {
            Command::RunChecks { checks, given, .. } => Some((checks.clone(), given.iter().map(|g| g.step.clone()).collect())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no checks asked: {cmds:?}"))
}

fn pass(run: &mut Run, step: &str, attempt: u32, checks: &[String]) -> Vec<Command> {
    let results = checks.iter().map(|c| CheckResult { check: c.clone(), verdict: CheckVerdict::Pass, tree: None }).collect();
    feed(run, Input::ChecksEnded { step: step.into(), attempt, fix: 0, results })
}

#[test]
fn a_check_may_read_a_step_a_run_at_its_risk_does_not_take() {
    fix_bug().validate().unwrap_or_else(|e| panic!("refused: {e:?}"));
}

#[test]
fn a_low_run_skips_the_cases_and_its_fix_is_judged_by_the_checks_it_takes() {
    let (mut run, cmds) = started(None);
    let t = turn(&cmds);
    assert_eq!(t.step, "fix", "a run at low risk does not take the cases");
    let cmds = deliver(&mut run, &t, "fix-1", vec![Changed::file("src/total.rs".into(), 3)]);
    let (checks, given) = asked_checks(&cmds);
    assert_eq!(checks, ["tests-pass"], "the proof reads the cases, which this run did not take");
    assert!(!given.contains(&"cases".to_string()), "{given:?}");
    pass(&mut run, "fix", t.attempt, &checks);
    assert!(run.steps[1].status == jaynes_core::run::Status::Done, "the fix passed every check it took: {:?}", run.steps[1].status);
    assert!(run.steps[0].done.is_none(), "the cases never ran");
}

#[test]
fn a_high_run_takes_the_cases_first_and_the_fix_is_judged_by_them() {
    let (mut run, cmds) = started(Some(Risk::High));
    let t = turn(&cmds);
    assert_eq!(t.step, "cases");
    let cmds = deliver(&mut run, &t, "cases-1", Vec::new());
    let t = turn(&cmds);
    assert_eq!(t.step, "fix");
    assert!(!t.given.iter().any(|g| g.step == "cases"), "the fix's agent never sees the cases");
    let cmds = deliver(&mut run, &t, "fix-1", vec![Changed::file("src/total.rs".into(), 3)]);
    let (checks, given) = asked_checks(&cmds);
    assert_eq!(checks, ["tests-pass", "proof"]);
    assert!(given.contains(&"cases".to_string()), "{given:?}");
}

#[test]
fn a_run_whose_risk_goes_up_under_the_fix_takes_the_cases_and_fixes_again_judged_by_them() {
    let (mut run, cmds) = started(None);
    let t = turn(&cmds);
    assert_eq!(t.step, "fix");
    // the fix turns out large: the run is now at high risk, and the cases are on its path, not yet written
    let cmds = deliver(&mut run, &t, "fix-1", vec![Changed::file("src/big.rs".into(), 80)]);
    assert_eq!(run.risk, Risk::High);
    let (checks, _) = asked_checks(&cmds);
    assert_eq!(checks, ["tests-pass"], "no case is written yet, so the proof has nothing to read");
    let cmds = pass(&mut run, "fix", t.attempt, &checks);
    let t = turn(&cmds);
    assert_eq!(t.step, "cases", "the run takes the cases now that it is at high risk");
    let cmds = deliver(&mut run, &t, "cases-1", Vec::new());
    let t = turn(&cmds);
    assert_eq!(t.step, "fix", "the fix was judged without the cases, so it is done again");
    let cmds = deliver(&mut run, &t, "fix-2", vec![Changed::file("src/big.rs".into(), 80)]);
    let (checks, given) = asked_checks(&cmds);
    assert_eq!(checks, ["tests-pass", "proof"]);
    assert!(given.contains(&"cases".to_string()), "{given:?}");
}

fn merged_after_proof(m: &Manifest, not_taken: Option<&str>) -> Promise {
    Promise {
        says: "Nothing is merged before the hidden cases passed on the fix".into(),
        form: Form::Never { on: On::parse("merge", m).expect("event"), unless: Condition::parse("check proof of fix passed", m).expect("condition") },
        not_taken: not_taken.map(str::to_string),
    }
}

#[test]
fn a_promise_on_the_cases_breaks_on_a_low_run_unless_its_role_says_what_holds_there() {
    let m = fix_bug();
    let proofs = prove(&m, &[merged_after_proof(&m, None), merged_after_proof(&m, Some("a small, low-risk fix has no hidden cases"))]);
    let Proof::Broken { path } = &proofs[0] else { panic!("a low run merges with no proof: {:?}", proofs[0]) };
    let said = path.join(" → ");
    assert!(said.starts_with("low risk") && said.ends_with("merge, before the check proof of fix passed"), "{said}");
    assert!(matches!(proofs[1], Proof::Proved { .. }), "every run that takes the cases merges only after they passed: {:?}", proofs[1]);
}

#[test]
fn a_condition_rests_on_the_highest_risk_of_what_it_reads() {
    let m = fix_bug();
    assert_eq!(Condition::parse("check proof of fix passed", &m).expect("condition").from_risk(&m), Risk::High);
    assert_eq!(Condition::parse("check tests-pass of fix passed", &m).expect("condition").from_risk(&m), Risk::Low);
    assert_eq!(Condition::parse("step cases done", &m).expect("condition").from_risk(&m), Risk::High);
    assert_eq!(Condition::parse("step fix done", &m).expect("condition").from_risk(&m), Risk::Low);
}
