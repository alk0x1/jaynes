//! What a stop offers the person to grant beside their answer, such as the project's test servers at the first stop
//! of a new project (the tech-lead's T5, 23/11): only the person grants it, only while the stop is open, only what the
//! stop offers, and once; the record says it.

use jaynes_core::digest::Digest;
use jaynes_core::event::{Event, Usage};
use jaynes_core::manifest::{Executor, Manifest, Offer, Offers, Rate, Reach, SessionKind, StepDecl, StopDecl};
use jaynes_core::question::{Answer, By, VerdictKind};
use jaynes_core::risk::Risk;
use jaynes_core::{decide, AttemptOutcome, Command, Input, Run};

fn step(name: &str, needs: &[&str]) -> StepDecl {
    StepDecl {
        name: name.to_string(),
        needs: needs.iter().map(|s| s.to_string()).collect(),
        executor: Executor::Agent,
        session: SessionKind::Fresh,
        reach: Reach::default(),
        effect: jaynes_core::manifest::Effect::None,
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
    }
}

fn stop(offers: Vec<Offer>) -> StopDecl {
    StopDecl {
        verdicts: vec![VerdictKind::Keep, VerdictKind::Redo],
        holds: true,
        from_risk: Risk::Low,
        release: true,
        budget: None,
        audit_rate: Rate(0.1),
        about: "Which stack?".into(),
        choose: false,
        redo: None,
        rounds: None,
        answers: Default::default(),
        note: String::new(),
        offers: Offers { list: offers, marked_by: Default::default() },
    }
}

fn feed(run: &mut Run, input: Input) -> Result<Vec<Event>, String> {
    let out = decide(Some(run), &input).map_err(|r| r.0)?;
    for e in &out.events {
        run.apply(e);
    }
    Ok(out.events)
}

/// A run of a new project: `stacks` asks which stack, offering `offers`, and `build` comes after it.
fn run(offers: Vec<Offer>) -> Run {
    let mut stacks = step("stacks", &[]);
    stacks.stop = Some(stop(offers));
    let build = step("build", &["stacks"]);
    let manifest = Manifest { role: "scaffold".into(), version: "1".into(), protocol: 1, steps: vec![stacks, build], target: Default::default(), delivery: Default::default() };
    assert!(manifest.validate().is_ok(), "{:?}", manifest.validate());
    let out = decide(None, &Input::Start {
        run: "r".into(),
        request: "a Go service".into(),
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
            feed(&mut run, Input::ProbeEnded { step, ok: true, reason: String::new() }).expect("probe");
        }
    }
    let attempt = run.steps[0].attempt;
    feed(&mut run, Input::AttemptEnded {
        step: "stacks".into(),
        attempt,
        fix: 0,
        outcome: AttemptOutcome::Delivered { output: Digest("o".into()), items: Vec::new(), changed: Vec::new(), usage: Usage::default(), session: None, tree: None, reply: None },
    })
    .expect("stacks delivered");
    run
}

fn asked(run: &Run) -> String {
    run.questions.values().find(|q| q.question.step == "stacks" && q.open()).expect("stacks asks").question.id.clone()
}

fn grant(question: &str, by: By) -> Input {
    Input::Grant { question: question.into(), offer: Offer::TestServers, by }
}

#[test]
fn the_person_grants_the_project_s_test_servers_at_a_stop_that_offers_them_and_the_record_says_so() {
    let mut run = run(vec![Offer::TestServers]);
    let q = asked(&run);
    let events = feed(&mut run, grant(&q, By::Person)).expect("the person grants");
    assert_eq!(events, [Event::Granted { question: q.clone(), offer: Offer::TestServers, by: By::Person }]);
    assert!(run.granted.contains(&Offer::TestServers));
    // the stop stays open for the answer, and nothing else moved
    assert!(run.questions[&q].open());
    // once
    assert_eq!(feed(&mut run, grant(&q, By::Person)).expect_err("refused"), "you already let this project's tests start their own test servers in this run");
}

#[test]
fn an_agent_or_a_rule_never_grants_even_at_a_stop_an_agent_may_release() {
    let mut run = run(vec![Offer::TestServers]);
    let q = asked(&run);
    for by in [By::Agent { who: "claude".into() }, By::Rule { rule: "routine".into() }] {
        let refused = feed(&mut run, grant(&q, by.clone())).expect_err("refused");
        assert_eq!(refused, "only you let this project's tests start their own test servers: an agent never does", "{by:?}");
    }
    assert!(run.granted.is_empty());
}

#[test]
fn only_what_an_open_stop_offers_is_granted() {
    // a stop that offers nothing
    let mut plain = run(Vec::new());
    let q = asked(&plain);
    assert_eq!(feed(&mut plain, grant(&q, By::Person)).expect_err("refused"), "this stop does not offer that");
    // a question there is not
    let mut run = run(vec![Offer::TestServers]);
    assert_eq!(feed(&mut run, grant("nope", By::Person)).expect_err("refused"), "there is no question nope");
    // a stop already answered
    let q = asked(&run);
    let shown = run.questions[&q].question.shown();
    let answer = Answer { back: Default::default(), question: q.clone(), shown, verdicts: Default::default(), by: By::Person, context: String::new() };
    feed(&mut run, Input::Answer { answer }).expect("answered");
    assert_eq!(feed(&mut run, grant(&q, By::Person)).expect_err("refused"), "this stop was already answered, or replaced by a newer one");
}

#[test]
fn a_role_marks_beforehand_only_what_its_stop_offers_and_offers_each_once() {
    let mut stacks = step("stacks", &[]);
    let mut twice = stop(vec![Offer::TestServers, Offer::TestServers]);
    twice.offers.marked_by.insert(Offer::TestServers, "serves_http".into());
    stacks.stop = Some(twice);
    let m = Manifest { role: "scaffold".into(), version: "1".into(), protocol: 1, steps: vec![stacks.clone()], target: Default::default(), delivery: Default::default() };
    assert!(m.validate().expect_err("refused").iter().any(|e| e.contains("offers the same thing twice")), "{:?}", m.validate());
    let mut unoffered = stop(Vec::new());
    unoffered.offers.marked_by.insert(Offer::TestServers, "serves_http".into());
    stacks.stop = Some(unoffered);
    let m = Manifest { steps: vec![stacks], ..m };
    assert!(m.validate().expect_err("refused").iter().any(|e| e.contains("marks TestServers beforehand and does not offer it")), "{:?}", m.validate());
}

#[test]
fn a_stop_that_offers_nothing_is_written_as_before() {
    // roles already pinned keep their fingerprint: an empty offer is never written
    let s = serde_json::to_value(stop(Vec::new())).expect("json");
    assert!(s.get("offers").is_none(), "{s}");
    let s = serde_json::to_value(stop(vec![Offer::TestServers])).expect("json");
    assert_eq!(s["offers"], serde_json::json!({"list": ["test_servers"]}));
}
