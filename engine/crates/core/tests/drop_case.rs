//! The person takes out of a run a hidden case they saw fail: the case is wrong, not the fix (the real-user, 22/11:
//! a case that expects what the project never did blocked fix-bug three times). The design the tech-lead approved:
//! the case comes out only from the card that shows it failing, one at a time; only the person takes it out, and the
//! agent never learns of it; it is said in the record, in the run's claims and on the merge before the person's
//! touch; and it holds for that run alone, never for the role. The last attempt spent is judged again without the
//! case, with no agent and no attempt spent.

use jaynes_core::digest::Digest;
use jaynes_core::event::{Event, Fault, Usage};
use jaynes_core::manifest::{CheckDecl, CheckKind, DeliveryKind, Executor, Manifest, ProjectAccess, Reach, StepDecl};
use jaynes_core::question::{By, QuestionKind};
use jaynes_core::risk::{Changed, Risk};
use jaynes_core::run::{Phase, Status};
use jaynes_core::{decide, AgentTurn, AttemptOutcome, CheckResult, CheckVerdict, Command, Input, Run};
use std::collections::BTreeMap;

const ROUNDS: &str = "totals round half up";
const REFUNDS: &str = "refunds are negative";

fn said(case: &str) -> String {
    format!("A check you cannot see found that this does not hold: {case}")
}

fn step(name: &str) -> StepDecl {
    StepDecl {
        name: name.into(),
        needs: Vec::new(),
        executor: Executor::Agent,
        session: Default::default(),
        reach: Reach::default(),
        effect: Default::default(),
        checks: Vec::new(),
        stop: None,
        from_risk: Risk::Low,
        attempts: 1,
        fixes: 0,
        hidden: false,
        idempotent: false,
        about: String::new(),
    }
}

/// fix-bug: hidden cases, then a fix judged by its tests and by the cases, one attempt.
fn fix_bug() -> Manifest {
    let mut cases = step("cases");
    cases.hidden = true;
    cases.reach = Reach { project: ProjectAccess::Read, commands: true, ..Reach::default() };
    let mut fix = step("fix");
    fix.reach = Reach { project: ProjectAccess::Write, write: vec!["**".into()], commands: true, ..Reach::default() };
    fix.checks = vec![
        CheckDecl { name: "tests-pass".into(), kind: CheckKind::Open, about: String::new(), reads: Vec::new() },
        CheckDecl { name: "proof".into(), kind: CheckKind::Hidden, about: String::new(), reads: vec!["cases".into()] },
    ];
    Manifest { role: "fix-bug".into(), version: "5.0.0".into(), protocol: 1, steps: vec![cases, fix], target: Default::default(), delivery: DeliveryKind::Merge }
}

fn feed(run: &mut Run, input: Input) -> (Vec<Event>, Vec<Command>) {
    let out = decide(Some(run), &input).unwrap_or_else(|r| panic!("refused {input:?}: {r}"));
    for e in &out.events {
        run.apply(e);
    }
    let mut events = out.events;
    let mut cmds = out.commands;
    while let Some(k) = cmds.iter().position(|c| matches!(c, Command::Probe { .. })) {
        let Command::Probe { step, .. } = cmds.remove(k) else { unreachable!() };
        let more = decide(Some(run), &Input::ProbeEnded { step, ok: true, reason: String::new() }).expect("probe");
        for e in &more.events {
            run.apply(e);
        }
        events.extend(more.events);
        cmds.extend(more.commands);
    }
    (events, cmds)
}

fn refused(run: &Run, input: Input) -> String {
    match decide(Some(run), &input) {
        Err(r) => r.to_string(),
        Ok(out) => panic!("{input:?} was taken: {:?}", out.events),
    }
}

fn turn(cmds: &[Command]) -> AgentTurn {
    cmds.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.clone()) } else { None }).unwrap_or_else(|| panic!("no turn: {cmds:?}"))
}

fn deliver(run: &mut Run, t: &AgentTurn, output: &str) -> Vec<Command> {
    let changed = if t.step == "fix" { vec![Changed::file("src/total.rs".into(), 3)] } else { Vec::new() };
    feed(run, Input::AttemptEnded {
        step: t.step.clone(),
        attempt: t.attempt,
        fix: t.fix,
        outcome: AttemptOutcome::Delivered { output: Digest(output.into()), items: Vec::new(), changed, usage: Usage::default(), session: Some("s".into()), tree: None, reply: None },
    })
    .1
}

/// What the commands ask the checks: the step, its attempt, the output, the checks, and the cases left out.
fn asked(cmds: &[Command]) -> (String, u32, Digest, Vec<String>, Vec<String>) {
    cmds.iter()
        .find_map(|c| match c {
            Command::RunChecks { step, attempt, output, checks, without, .. } => Some((step.clone(), *attempt, output.clone(), checks.clone(), without.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no checks asked: {cmds:?}"))
}

/// The checks of the fix give: its tests pass, and the hidden cases named fail.
fn judged(run: &mut Run, attempt: u32, failing: &[&str]) -> (Vec<Event>, Vec<Command>) {
    let proof = if failing.is_empty() {
        CheckVerdict::Pass
    } else {
        CheckVerdict::Fail { reasons: failing.iter().map(|c| said(c)).collect(), detail: String::new(), cases: Vec::new() }
    };
    feed(run, Input::ChecksEnded {
        step: "fix".into(),
        attempt,
        fix: 0,
        results: vec![
            CheckResult { check: "tests-pass".into(), verdict: CheckVerdict::Pass, tree: None },
            CheckResult { check: "proof".into(), verdict: proof, tree: None },
        ],
    })
}

/// A run whose one attempt at the fix failed the hidden cases named: blocked by the agent's fault.
fn blocked_by(failing: &[&str]) -> Run {
    let out = decide(None, &Input::Start {
        run: "r".into(),
        request: "the total is wrong".into(),
        manifest: fix_bug(),
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
    let t = turn(&cmds);
    assert_eq!(t.step, "cases");
    let cmds = deliver(&mut run, &t, "cases-1");
    let t = turn(&cmds);
    assert_eq!(t.step, "fix");
    let cmds = deliver(&mut run, &t, "fix-1");
    asked(&cmds);
    judged(&mut run, t.attempt, failing);
    assert!(matches!(run.phase, Phase::Blocked { step: Some(ref s), fault: Fault::Agent, .. } if s == "fix"), "{:?}", run.phase);
    run
}

fn drop(case: &str) -> Input {
    Input::DropCase { by: By::Person, step: "fix".into(), case: case.into() }
}

/// The merge that waits for the person, delivering as it is asked.
fn merge_about(run: &mut Run, mut cmds: Vec<Command>) -> String {
    for _ in 0..10 {
        if let Some(q) = cmds.iter().find_map(|c| match c {
            Command::Ask { question } if question.kind == QuestionKind::Merge => Some(question.clone()),
            _ => None,
        }) {
            return q.about;
        }
        let Some(digest) = cmds.iter().find_map(|c| if let Command::Deliver { digest, .. } = c { Some(digest.clone()) } else { None }) else {
            panic!("no merge asked: {cmds:?}");
        };
        cmds = feed(run, Input::DeliveryEnded { digest, ok: true, reference: "jaynes/r".into() }).1;
    }
    panic!("no merge asked")
}

#[test]
fn the_person_takes_out_a_case_they_saw_fail_and_the_last_attempt_is_judged_again_without_it_and_without_the_agent() {
    let mut run = blocked_by(&[ROUNDS]);
    let spent = run.counts.spent;
    let (events, cmds) = feed(&mut run, drop(ROUNDS));
    assert!(events.iter().any(|e| matches!(e, Event::CaseDropped { by: By::Person, step, case } if step == "fix" && case == ROUNDS)), "{events:?}");
    assert!(!cmds.iter().any(|c| matches!(c, Command::RunAgent(_))), "no agent works on it: {cmds:?}");
    let (step, attempt, output, checks, without) = asked(&cmds);
    assert_eq!((step.as_str(), attempt, output, checks), ("fix", 1, Digest("fix-1".into()), vec!["tests-pass".to_string(), "proof".to_string()]));
    assert_eq!(without, [ROUNDS]);
    let (_, cmds) = judged(&mut run, 1, &[]);
    assert!(matches!(run.steps[1].status, Status::Done), "{:?}", run.steps[1].status);
    assert_eq!(run.steps[1].done.as_ref().map(|d| d.output.clone()), Some(Digest("fix-1".into())));
    assert_eq!(run.counts.spent, spent, "judging again spends no attempt");
    // said on the merge, before the person's touch
    let about = merge_about(&mut run, cmds);
    assert!(about.contains(&format!("You took out a hidden case after seeing it fail, so the change was judged without it: “{ROUNDS}” (fix).")), "{about}");
}

#[test]
fn a_case_comes_out_only_where_the_person_saw_it_fail_and_one_at_a_time() {
    let run = blocked_by(&[ROUNDS, REFUNDS]);
    let e = refused(&run, drop("dates are in UTC"));
    assert!(e.contains("the hidden checks of fix did not say “dates are in UTC” failed"), "{e}");
    let e = refused(&run, Input::DropCase { by: By::Person, step: "cases".into(), case: ROUNDS.into() });
    assert!(e.contains("the run is not blocked at cases"), "{e}");
    let e = refused(&run, drop(""));
    assert!(e.contains("which case?"), "{e}");
    // a run under way, or blocked by a fault of Jaynes, has no case for the person to judge
    let mut running = run.clone();
    feed(&mut running, Input::Continue { by: By::Person, starter: None });
    let e = refused(&running, drop(ROUNDS));
    assert!(e.contains("the run is not blocked at fix"), "{e}");
    // once out, it is out
    let mut out = run.clone();
    feed(&mut out, drop(ROUNDS));
    judged(&mut out, 1, &[REFUNDS]);
    let e = refused(&out, drop(ROUNDS));
    assert!(e.contains(&format!("“{ROUNDS}” is already out of this run")), "{e}");
}

#[test]
fn only_the_person_takes_a_case_out_and_the_agent_never_learns_of_it() {
    let run = blocked_by(&[ROUNDS, REFUNDS]);
    let e = refused(&run, Input::DropCase { by: By::Agent { who: "claude:m1".into() }, step: "fix".into(), case: ROUNDS.into() });
    assert!(e.contains("only you take a hidden case out of a run, after seeing it fail"), "{e}");
    let e = refused(&run, Input::DropCase { by: By::Rule { rule: "any".into() }, step: "fix".into(), case: ROUNDS.into() });
    assert!(e.contains("only you take a hidden case out of a run"), "{e}");
    // another case still fails without it: the run blocks again, with no attempt spent
    let mut run = run;
    let spent = run.counts.spent;
    feed(&mut run, drop(ROUNDS));
    let (events, _) = judged(&mut run, 1, &[REFUNDS]);
    assert!(matches!(run.phase, Phase::Blocked { step: Some(ref s), fault: Fault::Agent, .. } if s == "fix"), "{:?}", run.phase);
    assert_eq!(run.counts.spent, spent, "{events:?}");
    // sent back to the agent, its next attempt reads what failed without the case taken out, and nothing of it
    let (_, cmds) = feed(&mut run, Input::SendBack { by: By::Person, words: "look at refunds".into() });
    let t = turn(&cmds);
    let told = format!("{:?} {:?} {:?}", t.failure, t.notes, t.context);
    assert!(told.contains(REFUNDS) && !told.contains(ROUNDS), "{told}");
    // and every later check of the fix in this run leaves it out
    let cmds = deliver(&mut run, &t, "fix-2");
    let (_, attempt, _, _, without) = asked(&cmds);
    assert_eq!((attempt, without), (t.attempt, vec![ROUNDS.to_string()]));
}

#[test]
fn a_case_taken_out_holds_for_that_run_alone() {
    let mut run = blocked_by(&[ROUNDS]);
    feed(&mut run, drop(ROUNDS));
    assert_eq!(run.manifest, fix_bug(), "the role is as it was");
    // another run of the same role checks every case
    let mut other = blocked_by(&[REFUNDS]);
    let (_, cmds) = feed(&mut other, Input::Continue { by: By::Person, starter: None });
    let t = turn(&cmds);
    let cmds = deliver(&mut other, &t, "fix-2");
    let (_, _, _, _, without) = asked(&cmds);
    assert!(without.is_empty(), "{without:?}");
}
