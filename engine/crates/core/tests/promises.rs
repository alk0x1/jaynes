//! A role's promises, proved by exploring every way its runs can go through the real core.

use jaynes_core::manifest::{CheckDecl, CheckKind, DeliveryKind, Executor, Manifest, ProjectAccess, Rate, Reach, StepDecl, StopDecl};
use jaynes_core::promise::{prove, Condition, Form, On, Promise, Proof};
use jaynes_core::question::VerdictKind;
use jaynes_core::risk::Risk;

fn step(name: &str, executor: Executor, needs: &[&str]) -> StepDecl {
    StepDecl {
        name: name.into(),
        needs: needs.iter().map(|s| s.to_string()).collect(),
        executor,
        session: Default::default(),
        reach: Reach::default(),
        effect: Default::default(),
        checks: Vec::new(),
        stop: None,
        from_risk: Risk::Low,
        attempts: 3,
        fixes: 2,
        hidden: false,
        about: String::new(),
    }
}

fn open(name: &str) -> CheckDecl {
    CheckDecl { name: name.into(), kind: CheckKind::Open, about: String::new(), reads: Vec::new() }
}

fn stop(from_risk: Risk, holds: bool) -> StopDecl {
    StopDecl { verdicts: vec![VerdictKind::Keep, VerdictKind::Redo], holds, from_risk, release: false, budget: None, audit_rate: Rate(0.1), about: "Keep it?".into() }
}

/// small-change as it ships: Jaynes finds the suite, the agent changes, its stop asks only from high risk.
fn small_change() -> Manifest {
    let mut runner = step("runner", Executor::Role, &[]);
    runner.reach = Reach { project: ProjectAccess::Read, ..Reach::default() };
    runner.attempts = 1;
    let mut change = step("change", Executor::Agent, &["runner"]);
    change.reach = Reach { project: ProjectAccess::Write, write: vec!["**".into()], commands: true, ..Reach::default() };
    change.checks = vec![open("suite-passes")];
    change.stop = Some(StopDecl { release: true, ..stop(Risk::High, false) });
    Manifest { role: "small-change".into(), version: "3.0.0".into(), protocol: 1, steps: vec![runner, change], delivery: DeliveryKind::Branch }
}

fn promise(says: &str, form: Form) -> Promise {
    Promise { says: says.into(), form }
}

#[test]
fn small_change_delivers_at_low_risk_without_asking_anyone_and_the_path_says_how() {
    let m = small_change();
    let p = promise("Nothing is delivered unless you answered at change", Form::Never {
        on: On::parse("deliver", &m).expect("event"),
        unless: Condition::parse("person answered at change", &m).expect("condition"),
    });
    let proofs = prove(&m, &[p]);
    let Proof::Broken { path } = &proofs[0] else { panic!("it should break: {proofs:?}") };
    let said = path.join(" → ");
    assert!(said.starts_with("low risk"), "{said}");
    assert!(said.contains("change delivers a small change"), "{said}");
    assert!(said.contains("change's checks pass"), "{said}");
    assert!(said.contains("change's stop asks only from high risk"), "{said}");
    assert!(said.ends_with("deliver, with no answer of yours at change"), "{said}");
}

#[test]
fn a_merge_that_waits_for_the_person_is_proved() {
    // a stop that holds holds the steps after it, not the delivery: a step of the person's after the change does
    let mut m = small_change();
    m.delivery = DeliveryKind::Merge;
    let mut approve = step("approve", Executor::Person, &["change"]);
    approve.stop = Some(stop(Risk::Low, true));
    m.steps.push(approve);
    let p = promise("Nothing is merged unless you answered at approve", Form::Never {
        on: On::parse("merge", &m).expect("event"),
        unless: Condition::parse("person answered at approve", &m).expect("condition"),
    });
    let proofs = prove(&m, &[p]);
    assert!(matches!(proofs[0], Proof::Proved { states } if states > 50), "{proofs:?}");
}

#[test]
fn a_stop_that_does_not_hold_lets_the_merge_go_before_the_answer() {
    let mut m = small_change();
    m.delivery = DeliveryKind::Merge;
    m.steps[1].stop = Some(stop(Risk::Low, false));
    let p = promise("Nothing is merged unless you answered at change", Form::Never {
        on: On::Merge,
        unless: Condition::PersonAnswered("change".into()),
    });
    let Proof::Broken { path } = &prove(&m, &[p])[0] else { panic!("it should break") };
    let said = path.join(" → ");
    assert!(said.contains("change asks you") && said.ends_with("merge, with no answer of yours at change"), "{said}");
}

/// fix-bug's shape: hidden cases, a reproduce whose stop may be sampled, a fix checked by a hidden check.
fn fix_bug() -> Manifest {
    let mut runner = step("runner", Executor::Role, &[]);
    runner.reach = Reach { project: ProjectAccess::Read, ..Reach::default() };
    runner.attempts = 1;
    let mut cases = step("cases", Executor::Agent, &[]);
    cases.hidden = true;
    cases.attempts = 2;
    cases.reach = Reach { project: ProjectAccess::Read, commands: true, ..Reach::default() };
    let mut reproduce = step("reproduce", Executor::Agent, &["runner"]);
    reproduce.reach = Reach { project: ProjectAccess::Write, write: vec!["tests/**".into()], commands: true, ..Reach::default() };
    reproduce.checks = vec![open("tests-fail-for-the-bug")];
    reproduce.stop = Some(StopDecl { budget: Some(Rate(0.2)), ..stop(Risk::Low, false) });
    let found = step("found", Executor::Role, &["reproduce"]);
    let mut fix = step("fix", Executor::Agent, &["runner", "reproduce", "found"]);
    fix.reach = Reach { project: ProjectAccess::Write, write: vec!["**".into(), "!tests/**".into()], commands: true, ..Reach::default() };
    fix.checks = vec![open("tests-pass"), CheckDecl { name: "proof".into(), kind: CheckKind::Hidden, about: String::new(), reads: vec!["cases".into()] }];
    fix.stop = Some(stop(Risk::Low, false));
    Manifest { role: "fix-bug".into(), version: "3.0.0".into(), protocol: 1, steps: vec![runner, cases, reproduce, found, fix], delivery: DeliveryKind::Merge }
}

#[test]
fn fix_bug_s_own_promises_are_proved_and_its_merge_does_not_wait() {
    let m = fix_bug();
    let ps = [
        promise("The agent of fix never sees the cases", Form::never_sees("fix", "cases", &m).expect("form")),
        promise("fix starts only after the tests failed for the bug", Form::OnlyAfter {
            on: On::parse("step fix starts", &m).expect("event"),
            after: Condition::parse("check tests-fail-for-the-bug of reproduce passed", &m).expect("condition"),
        }),
        promise("Nothing is merged before fix is done", Form::Never { on: On::Merge, unless: Condition::parse("step fix done", &m).expect("condition") }),
        promise("Nothing is merged unless you answered at fix", Form::Never { on: On::Merge, unless: Condition::PersonAnswered("fix".into()) }),
    ];
    let proofs = prove(&m, &ps);
    for (p, proof) in ps.iter().zip(&proofs).take(3) {
        assert!(matches!(proof, Proof::Proved { .. }), "{}: {proof:?}", p.says);
    }
    let Proof::Broken { path } = &proofs[3] else { panic!("fix's stop does not hold the run: {:?}", proofs[3]) };
    assert!(path.join(" → ").ends_with("merge, with no answer of yours at fix"), "{path:?}");
}

#[test]
fn a_promise_names_only_what_the_role_has() {
    let m = small_change();
    assert!(On::parse("merge", &m).is_err_and(|e| e.contains("delivers a branch")));
    assert!(On::parse("step nope starts", &m).is_err_and(|e| e.contains("nope is not a step")));
    assert!(Condition::parse("person answered at runner", &m).is_err_and(|e| e.contains("runner has no stop")));
    assert!(Condition::parse("check nope of change passed", &m).is_err_and(|e| e.contains("change has no check nope")));
    assert!(Form::never_sees("runner", "change", &m).is_err_and(|e| e.contains("not an agent's step")));
}

