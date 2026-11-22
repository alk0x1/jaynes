//! A step the person sends back with their words goes on in the session of its last attempt that delivered: the agent
//! knows what it made and why, and does not read it all again. A new session instead after an attempt the agent spent,
//! when the person dropped everything the step showed (they ask for new ideas, which the old conversation would hold
//! back), and when the session already carries more than RESUME_CEILING tokens.

use jaynes_core::decide::RESUME_CEILING;
use jaynes_core::digest::Digest;
use jaynes_core::event::{Event, Usage};
use jaynes_core::manifest::{CheckDecl, CheckKind, Effect, Executor, Manifest, ProjectAccess, Rate, Reach, SessionKind, StepDecl, StopDecl};
use jaynes_core::question::{Answer, By, Item, Question, Verdict, VerdictKind};
use jaynes_core::risk::Risk;
use jaynes_core::{decide, AgentTurn, AttemptOutcome, CheckResult, CheckVerdict, Command, Input, Run};
use std::collections::BTreeMap;

/// An agent step that asks the person about what it made, and one after it.
fn manifest() -> Manifest {
    let stacks = StepDecl {
        name: "stacks".to_string(),
        needs: Vec::new(),
        executor: Executor::Agent,
        session: SessionKind::Fresh,
        reach: Reach { project: ProjectAccess::Write, write: vec!["design/**".into()], ..Reach::default() },
        effect: Effect::None,
        checks: vec![CheckDecl { name: "shape".into(), kind: CheckKind::Open, about: String::new(), reads: Vec::new() }],
        stop: Some(StopDecl {
            verdicts: vec![VerdictKind::Keep, VerdictKind::Change, VerdictKind::Drop],
            holds: true,
            from_risk: Risk::Low,
            release: false,
            budget: None,
            audit_rate: Rate(0.1),
            about: "Which stack goes on?".into(),
            choose: false,
            redo: None,
            rounds: None,
            answers: Default::default(),
        }),
        from_risk: Risk::Low,
        attempts: 3,
        fixes: 2,
        hidden: false,
        idempotent: false,
        about: String::new(),
    };
    let mut scaffold = stacks.clone();
    scaffold.name = "scaffold".into();
    scaffold.needs = vec!["stacks".into()];
    scaffold.stop = None;
    Manifest { role: "greenfield".into(), version: "1".into(), protocol: 1, steps: vec![stacks, scaffold], target: Default::default(), delivery: Default::default() }
}

fn feed(run: &mut Run, input: Input) -> (Vec<Event>, Vec<Command>) {
    let out = decide(Some(run), &input).unwrap_or_else(|r| panic!("refused {input:?}: {r}"));
    for e in &out.events {
        run.apply(e);
    }
    settle(run, out.events, out.commands)
}

fn settle(run: &mut Run, mut events: Vec<Event>, mut cmds: Vec<Command>) -> (Vec<Event>, Vec<Command>) {
    // pass every probe on the way
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

fn turn(cmds: &[Command]) -> AgentTurn {
    cmds.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.clone()) } else { None }).unwrap_or_else(|| panic!("no turn: {cmds:?}"))
}

fn items() -> Vec<Item> {
    ["go", "next"].iter().map(|id| Item { id: id.to_string(), label: id.to_string(), digest: Digest(format!("{id}-d")) }).collect()
}

/// The turn delivers in `session`, having used `tokens`, and its checks pass: the person is asked.
fn deliver(run: &mut Run, t: &AgentTurn, session: &str, tokens: u64) -> Question {
    let usage = Usage { seconds: 1, tokens, cached: 0 };
    let output = Digest(format!("out-{}-{}", t.round, t.attempt));
    feed(run, Input::AttemptEnded { step: t.step.clone(), attempt: t.attempt, fix: 0, outcome: AttemptOutcome::Delivered { output, items: items(), changed: Vec::new(), usage, session: Some(session.into()), tree: None, reply: None } });
    let (events, _) = feed(run, Input::ChecksEnded { step: t.step.clone(), attempt: t.attempt, fix: 0, results: vec![CheckResult { check: "shape".into(), verdict: CheckVerdict::Pass, tree: None }] });
    events.iter().find_map(|e| if let Event::QuestionOpened { question } = e { Some(question.clone()) } else { None }).expect("asked")
}

fn answer(run: &mut Run, q: &Question, verdicts: &[(&str, Verdict)]) -> AgentTurn {
    let answer = Answer {
        back: Default::default(),
        question: q.id.clone(),
        shown: q.shown(),
        verdicts: verdicts.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        by: By::Person,
        context: String::new(),
    };
    turn(&feed(run, Input::Answer { answer }).1)
}

fn started() -> (Run, AgentTurn) {
    let out = decide(None, &Input::Start {
        run: "r".into(),
        request: "a habit tracker".into(),
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
    let (_, cmds) = settle(&mut run, Vec::new(), out.commands);
    let t = turn(&cmds);
    (run, t)
}

fn change(text: &str) -> Verdict {
    Verdict::Change { text: text.to_string() }
}

#[test]
fn a_step_sent_back_with_the_person_s_words_goes_on_in_its_session() {
    let (mut run, first) = started();
    assert_eq!(first.session, None, "the first round starts afresh");
    let q = deliver(&mut run, &first, "s1", 40_000);
    let again = answer(&mut run, &q, &[("go", change("use htmx")), ("next", Verdict::Keep)]);
    assert_eq!((again.round, again.attempt, again.session.as_deref()), (1, 2, Some("s1")));
    assert!(again.notes.join("\n").contains("use htmx"), "the words still come in the turn: {:?}", again.notes);
}

#[test]
fn after_an_attempt_the_agent_spent_the_step_starts_afresh() {
    let (mut run, first) = started();
    let q = deliver(&mut run, &first, "s1", 40_000);
    let again = answer(&mut run, &q, &[("go", change("use htmx")), ("next", Verdict::Keep)]);
    assert_eq!(again.session.as_deref(), Some("s1"));
    let failed = AttemptOutcome::Failed { reasons: vec!["no output".to_string()], usage: Usage::default() };
    let (_, cmds) = feed(&mut run, Input::AttemptEnded { step: "stacks".into(), attempt: again.attempt, fix: 0, outcome: failed });
    let third = turn(&cmds);
    assert_eq!((third.round, third.attempt, third.session.as_deref()), (1, 3, None));
    assert!(third.notes.join("\n").contains("use htmx"), "a new session gets the words all the same: {:?}", third.notes);
}

#[test]
fn when_the_person_drops_everything_the_step_starts_afresh() {
    let (mut run, first) = started();
    let q = deliver(&mut run, &first, "s1", 40_000);
    let again = answer(&mut run, &q, &[("go", Verdict::Drop), ("next", Verdict::Drop)]);
    assert_eq!((again.round, again.session.as_deref()), (1, None), "new ideas, without the conversation that made these");
    // one dropped and one changed is not everything dropped
    let (mut run, first) = started();
    let q = deliver(&mut run, &first, "s1", 40_000);
    let again = answer(&mut run, &q, &[("go", Verdict::Drop), ("next", change("smaller"))]);
    assert_eq!(again.session.as_deref(), Some("s1"));
}

#[test]
fn a_session_that_carries_more_than_the_ceiling_is_not_resumed() {
    let (mut run, first) = started();
    let q = deliver(&mut run, &first, "s1", 60_000);
    let second = answer(&mut run, &q, &[("go", change("use htmx")), ("next", Verdict::Keep)]);
    assert_eq!(second.session.as_deref(), Some("s1"), "60 thousand carried");
    // the resumed turn adds to the same conversation: past the ceiling, the next round starts afresh
    let q = deliver(&mut run, &second, "s1", RESUME_CEILING - 60_000 + 1);
    let third = answer(&mut run, &q, &[("go", change("add the backup")), ("next", Verdict::Keep)]);
    assert_eq!((third.round, third.session.as_deref()), (2, None));
    // and the new session counts from nothing
    let q = deliver(&mut run, &third, "s2", 30_000);
    let fourth = answer(&mut run, &q, &[("go", change("two processes")), ("next", Verdict::Keep)]);
    assert_eq!((fourth.round, fourth.session.as_deref()), (3, Some("s2")));
}
