//! The agent that started a run may do the little that redoes nothing in it: continue a block that is Jaynes's
//! fault, where Jaynes only judges again the same change, and set the run aside as abandoned, its work kept, which
//! only the person brings back or abandons for good. Each tells the person, as a notice, not a question. Who started it
//! is the opaque `starter` the start recorded; a run with none, or another agent, is the person's alone.

use jaynes_core::digest::Digest;
use jaynes_core::event::{Fault, Outcome, Usage};
use jaynes_core::manifest::{CheckDecl, CheckKind, Effect, Executor, Manifest, ProjectAccess, Reach, SessionKind, StepDecl};
use jaynes_core::question::By;
use jaynes_core::run::Phase;
use jaynes_core::{decide, AttemptOutcome, CheckResult, CheckVerdict, Command, Input, Run};
use std::collections::BTreeMap;

const OUTER: &str = "claude:4242@1763770000";

fn step() -> StepDecl {
    StepDecl {
        name: "change".to_string(),
        needs: Vec::new(),
        executor: Executor::Agent,
        session: SessionKind::Fresh,
        reach: Reach { project: ProjectAccess::Write, write: vec!["**".into()], ..Reach::default() },
        effect: Effect::None,
        checks: vec![CheckDecl { name: "tests".into(), kind: CheckKind::Open, about: String::new(), reads: Vec::new() }],
        stop: None,
        from_risk: Default::default(),
        attempts: 3,
        fixes: 2,
        hidden: false,
        idempotent: false,
        about: String::new(),
    }
}

fn feed(run: &mut Run, input: Input) -> Vec<Command> {
    let out = decide(Some(run), &input).unwrap_or_else(|r| panic!("refused {input:?}: {r}"));
    for e in &out.events {
        run.apply(e);
    }
    out.commands
}

fn refused(run: &Run, input: Input) -> String {
    match decide(Some(run), &input) {
        Err(r) => r.0,
        Ok(out) => panic!("{input:?} was taken: {:?}", out.events),
    }
}

/// A run of one agent step with a check, started by `starter`, at its first turn.
fn started(starter: Option<&str>) -> (Run, u32) {
    let manifest = Manifest { role: "small".into(), version: "1".into(), protocol: 1, steps: vec![step()], target: Default::default(), delivery: Default::default() };
    let out = decide(None, &Input::Start {
        run: "r".into(),
        request: "fix it".into(),
        manifest,
        role_fingerprint: Digest("f".into()),
        agent: "claude:m1".into(),
        raise: None,
        attention: BTreeMap::new(),
        task: None,
        arm: None,
        never_merge: false,
        starter: starter.map(str::to_string),
    })
    .expect("start");
    let mut run = Run::fold(&out.events).expect("run");
    let mut c = Vec::new();
    for cmd in out.commands {
        if let Command::Probe { step, .. } = cmd {
            c.extend(feed(&mut run, Input::ProbeEnded { step, ok: true, reason: String::new() }));
        }
    }
    let attempt = c.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.attempt) } else { None }).expect("a turn");
    (run, attempt)
}

/// Blocked by Jaynes on a change its check could not judge: going on judges the same change again.
fn check_could_not_judge(starter: Option<&str>) -> Run {
    let (mut run, attempt) = started(starter);
    let outcome = AttemptOutcome::Delivered { output: Digest("o".into()), items: Vec::new(), changed: Vec::new(), usage: Usage::default(), session: Some("s".into()), tree: None, reply: None };
    feed(&mut run, Input::AttemptEnded { step: "change".into(), attempt, fix: 0, outcome });
    let blocked = CheckResult::new("tests", CheckVerdict::Blocked { reason: "the test runner did not start".into() });
    feed(&mut run, Input::ChecksEnded { step: "change".into(), attempt, fix: 0, results: vec![blocked] });
    assert!(matches!(run.phase, Phase::Blocked { fault: Fault::Jaynes, .. }), "{:?}", run.phase);
    assert!(run.checks_again().is_some());
    run
}

fn agent(starter: Option<&str>) -> (By, Option<String>) {
    (By::Agent { who: "an agent through the Jaynes plugin".into() }, starter.map(str::to_string))
}

fn notice(c: &[Command]) -> Option<&str> {
    c.iter().find_map(|c| if let Command::Notify { text } = c { Some(text.as_str()) } else { None })
}

#[test]
fn the_agent_that_started_the_run_continues_a_block_that_is_jaynes_s_fault() {
    let mut run = check_could_not_judge(Some(OUTER));
    let (by, starter) = agent(Some(OUTER));
    let c = feed(&mut run, Input::Continue { by: by.clone(), starter });
    // the same change is judged again, the agent does not work again, and the person is told who went on
    assert!(c.iter().any(|c| matches!(c, Command::RunChecks { step, .. } if step == "change")), "{c:?}");
    assert!(!c.iter().any(|c| matches!(c, Command::RunAgent(_))), "{c:?}");
    let said = notice(&c).unwrap_or_else(|| panic!("no notice: {c:?}"));
    assert!(said.contains("The agent that started the run") && said.contains("“change”") && said.contains("Nothing is redone"), "{said}");
    assert!(run.questions.is_empty(), "a notice, not a question");
}

#[test]
fn another_agent_is_refused_the_continue() {
    let run = check_could_not_judge(Some(OUTER));
    // another agent, an agent that gives no starter, and the agent of a run the start recorded no starter for
    for (by, starter) in [agent(Some("claude:999@1763771111")), agent(None)] {
        assert!(refused(&run, Input::Continue { by, starter }).contains("only you"));
    }
    let nobody = check_could_not_judge(None);
    let (by, starter) = agent(Some(OUTER));
    assert!(refused(&nobody, Input::Continue { by, starter }).contains("only you"));
    // the person, as today
    let mut run = run;
    feed(&mut run, Input::Continue { by: By::Person, starter: None });
}

#[test]
fn an_agent_inside_a_run_owns_nothing_of_the_run_that_started_it() {
    // the agent Jaynes starts in a run has itself as its nearest agent ancestor, never the agent that started the run
    let run = check_could_not_judge(Some(OUTER));
    let (by, starter) = agent(Some("claude:5151@1763772222"));
    assert!(refused(&run, Input::Continue { by: by.clone(), starter: starter.clone() }).contains("only you"));
    assert!(refused(&run, Input::Abandon { by, starter }).contains("only you"));
}

#[test]
fn a_block_that_needs_work_is_refused_to_the_agent_that_started_the_run() {
    // the agent program did not start: going on starts a turn, so the agent would work again
    let (mut run, attempt) = started(Some(OUTER));
    feed(&mut run, Input::AttemptEnded { step: "change".into(), attempt, fix: 0, outcome: AttemptOutcome::Fault { usage: Default::default(), reason: "the agent did not start".into() } });
    assert!(matches!(run.phase, Phase::Blocked { fault: Fault::Jaynes, .. }), "{:?}", run.phase);
    assert!(run.checks_again().is_none());
    let (by, starter) = agent(Some(OUTER));
    let said = refused(&run, Input::Continue { by, starter });
    assert!(said.contains("only you") && said.contains("work again"), "{said}");
    // the person goes on, as today
    let c = feed(&mut run, Input::Continue { by: By::Person, starter: None });
    assert!(c.iter().any(|c| matches!(c, Command::RunAgent(_)) || matches!(c, Command::Probe { .. })), "{c:?}");
}

#[test]
fn abandoning_its_own_run_sets_it_aside_with_its_work_kept() {
    let (mut run, _) = started(Some(OUTER));
    let (by, starter) = agent(Some(OUTER));
    let c = feed(&mut run, Input::Abandon { by: by.clone(), starter });
    assert!(!run.finished(), "an agent's abandon is not final: {:?}", run.phase);
    assert!(run.halted, "nothing goes on in it");
    assert_eq!(run.set_aside.as_ref(), Some(&by));
    let said = notice(&c).unwrap_or_else(|| panic!("no notice: {c:?}"));
    assert!(said.contains("The agent that started the run") && said.contains("abandoned") && said.contains("kept"), "{said}");
    assert!(run.questions.is_empty(), "a notice, not a question");
}

#[test]
fn abandoning_another_s_run_is_refused() {
    let (run, _) = started(Some(OUTER));
    for (by, starter) in [agent(Some("claude:999@1763771111")), agent(None)] {
        assert!(refused(&run, Input::Abandon { by, starter }).contains("only you"));
    }
    let (nobody, _) = started(None);
    let (by, starter) = agent(Some(OUTER));
    assert!(refused(&nobody, Input::Abandon { by, starter }).contains("only you"));
}

#[test]
fn the_person_brings_back_a_run_an_agent_abandoned_and_it_goes_on_from_where_it_was() {
    let mut run = check_could_not_judge(Some(OUTER));
    let (by, starter) = agent(Some(OUTER));
    feed(&mut run, Input::Abandon { by, starter });
    let c = feed(&mut run, Input::Resume { by: By::Person });
    assert!(!run.halted && run.set_aside.is_none(), "brought back");
    // where it was: blocked on the change its check could not judge, which the person continues
    assert!(matches!(run.phase, Phase::Blocked { fault: Fault::Jaynes, .. }), "{:?} {c:?}", run.phase);
    let c = feed(&mut run, Input::Continue { by: By::Person, starter: None });
    assert!(c.iter().any(|c| matches!(c, Command::RunChecks { .. })), "{c:?}");
    // and the person's abandon is final, as today, set aside or not
    let (mut again, _) = started(Some(OUTER));
    let (by, starter) = agent(Some(OUTER));
    feed(&mut again, Input::Abandon { by, starter });
    feed(&mut again, Input::Abandon { by: By::Person, starter: None });
    assert!(matches!(again.phase, Phase::Finished { outcome: Outcome::Abandoned }), "{:?}", again.phase);
}

#[test]
fn no_agent_brings_back_nor_abandons_for_good_a_run_an_agent_set_aside() {
    let (mut run, _) = started(Some(OUTER));
    let (by, starter) = agent(Some(OUTER));
    feed(&mut run, Input::Abandon { by: by.clone(), starter: starter.clone() });
    // the agent that started it, and any other
    for (by, starter) in [(by, starter), agent(Some("claude:999@1763771111")), agent(None)] {
        assert!(refused(&run, Input::Resume { by: by.clone() }).contains("only you"));
        assert!(refused(&run, Input::Abandon { by: by.clone(), starter: starter.clone() }).contains("only you"));
        assert!(refused(&run, Input::Continue { by, starter }).contains("only you"));
    }
    assert!(!run.finished() && run.set_aside.is_some());
}
