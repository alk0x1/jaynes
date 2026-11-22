//! The thread of a stop: the person asks the agent of the step something about what it shows, and the agent answers
//! in a turn that only reads. Nothing is redone and nothing is chosen: the stop stays open in the same round, with the
//! same items. The asks, the answers, the person's words and the agent's few words beside each new round are kept by
//! the step, round by round, and the next round's agent is given what was asked and answered.

use jaynes_core::digest::Digest;
use jaynes_core::event::{Event, Usage};
use jaynes_core::io::{AskOutcome, Asked};
use jaynes_core::manifest::{CheckDecl, CheckKind, Effect, Executor, Manifest, Network, ProjectAccess, Rate, Reach, SessionKind, StepDecl, StopDecl};
use jaynes_core::question::{Answer, By, Item, Question, Verdict, VerdictKind};
use jaynes_core::risk::Risk;
use jaynes_core::run::{EntryKind, Status};
use jaynes_core::{decide, AgentTurn, AskTurn, AttemptOutcome, CheckResult, CheckVerdict, Command, Input, Run};
use std::collections::BTreeMap;

fn step(name: &str, executor: Executor) -> StepDecl {
    StepDecl {
        name: name.to_string(),
        needs: Vec::new(),
        executor,
        session: SessionKind::Fresh,
        reach: Reach { project: ProjectAccess::Write, write: vec!["design/**".into()], ..Reach::default() },
        effect: Effect::None,
        checks: vec![CheckDecl { name: "shape".into(), kind: CheckKind::Open, about: String::new(), reads: Vec::new(), project_tests: false }],
        stop: Some(StopDecl { settled: None, holds_unless: None, vouched_by: None,
            verdicts: vec![VerdictKind::Keep, VerdictKind::Change, VerdictKind::Drop],
            holds: true,
            from_risk: Risk::Low,
            release: false,
            budget: None,
            audit_rate: Rate(0.1),
            about: "Is this the look?".into(),
            choose: false,
            redo: None,
            rounds: None,
            answers: Default::default(), note: String::new(),
            offers: Default::default(),
        }),
        from_risk: Risk::Low,
        attempts: 3,
        fixes: 2,
        hidden: false,
        idempotent: false,
        about: String::new(),
        model: Default::default(),
        turns: None, delivered: true,
    }
}

fn feed(run: &mut Run, input: Input) -> (Vec<Event>, Vec<Command>) {
    let out = decide(Some(run), &input).unwrap_or_else(|r| panic!("refused {input:?}: {r}"));
    for e in &out.events {
        run.apply(e);
    }
    (out.events, out.commands)
}

fn refused(run: &Run, input: Input) -> String {
    match decide(Some(run), &input) {
        Ok(out) => panic!("taken: {input:?}: {:?}", out.events),
        Err(r) => r.to_string(),
    }
}

fn turn(cmds: &[Command]) -> AgentTurn {
    cmds.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.clone()) } else { None }).unwrap_or_else(|| panic!("no turn: {cmds:?}"))
}

fn ask_turn(cmds: &[Command]) -> AskTurn {
    cmds.iter().find_map(|c| if let Command::AnswerAsk(t) = c { Some(t.clone()) } else { None }).unwrap_or_else(|| panic!("no ask: {cmds:?}"))
}

fn usage(tokens: u64) -> Usage {
    Usage { tokens, ..Usage::default() }
}

/// Deliver for the turn under way, with the agent's few words, and pass its checks; the events that follow.
fn deliver(run: &mut Run, t: &AgentTurn, output: &str, reply: Option<&str>) -> Vec<Event> {
    let (_, cmds) = feed(run, Input::AttemptEnded {
        step: t.step.clone(),
        attempt: t.attempt,
        fix: t.fix,
        outcome: AttemptOutcome::Delivered {
            output: Digest(output.into()),
            items: vec![Item { named: None, id: "go".into(), label: "Go".into(), digest: Digest(format!("{output}-go")) }],
            changed: Vec::new(),
            usage: Usage::default(),
            session: Some("s1".into()),
            tree: None,
            reply: reply.map(str::to_string),
        },
    });
    assert!(cmds.iter().any(|c| matches!(c, Command::RunChecks { .. })), "{cmds:?}");
    feed(run, Input::ChecksEnded { step: t.step.clone(), attempt: t.attempt, fix: t.fix, results: vec![CheckResult { check: "shape".into(), verdict: CheckVerdict::Pass, tree: None }] }).0
}

/// A run whose one step made one option and asks about it.
fn asked_with(executor: Executor) -> (Run, Question) {
    let manifest = Manifest { role: "look".into(), version: "1".into(), protocol: 1, steps: vec![step("look", executor)], target: Default::default(), delivery: Default::default() };
    let out = decide(None, &Input::Start {
        run: "r".into(),
        request: "a look".into(),
        manifest,
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
    let role_attempt = out.commands.iter().find_map(|c| if let Command::RunRole { attempt, .. } = c { Some(*attempt) } else { None });
    let events = match executor {
        Executor::Agent => {
            let (_, cmds) = feed(&mut run, Input::ProbeEnded { step: "look".into(), ok: true, reason: String::new() });
            deliver(&mut run, &turn(&cmds), "one", None)
        }
        _ if role_attempt.is_none() => {
            let (_, cmds) = feed(&mut run, Input::ProbeEnded { step: "look".into(), ok: true, reason: String::new() });
            let attempt = cmds.iter().find_map(|c| if let Command::RunRole { attempt, .. } = c { Some(*attempt) } else { None }).unwrap_or_else(|| panic!("no role turn: {cmds:?}"));
            let item = Item { named: None, id: "go".into(), label: "Go".into(), digest: Digest("one-go".into()) };
            let outcome = AttemptOutcome::Delivered { output: Digest("one".into()), items: vec![item], changed: Vec::new(), usage: Usage::default(), session: None, tree: None, reply: None };
            feed(&mut run, Input::AttemptEnded { step: "look".into(), attempt, fix: 0, outcome });
            let results = vec![CheckResult { check: "shape".into(), verdict: CheckVerdict::Pass, tree: None }];
            feed(&mut run, Input::ChecksEnded { step: "look".into(), attempt, fix: 0, results }).0
        }
        _ => {
            let attempt = role_attempt.unwrap_or_else(|| panic!("no role turn: {:?}", out.commands));
            let item = Item { named: None, id: "go".into(), label: "Go".into(), digest: Digest("one-go".into()) };
            let outcome = AttemptOutcome::Delivered { output: Digest("one".into()), items: vec![item], changed: Vec::new(), usage: Usage::default(), session: None, tree: None, reply: None };
            feed(&mut run, Input::AttemptEnded { step: "look".into(), attempt, fix: 0, outcome });
            let results = vec![CheckResult { check: "shape".into(), verdict: CheckVerdict::Pass, tree: None }];
            feed(&mut run, Input::ChecksEnded { step: "look".into(), attempt, fix: 0, results }).0
        }
    };
    let q = events.iter().find_map(|e| if let Event::QuestionOpened { question } = e { Some(question.clone()) } else { None }).unwrap_or_else(|| panic!("asked: {events:?}"));
    // a stop does not hold the run: its branch goes out while the stop waits, and a change comes back on top
    if let Some(digest) = run.delivering.clone() {
        feed(&mut run, Input::DeliveryEnded { digest, ok: true, reference: "jaynes/r".into(), merged: None });
    }
    (run, q)
}

fn asked() -> (Run, Question) {
    asked_with(Executor::Agent)
}

fn ask(q: &Question, text: &str) -> Input {
    Input::AskAgent { question: q.id.clone(), item: Some("go".into()), text: text.into(), by: By::Person }
}

fn answered(ask: &str, text: &str, session: Option<&str>) -> Input {
    Input::AskEnded { ask: ask.into(), outcome: AskOutcome::Answered { text: text.into(), usage: usage(100), session: session.map(str::to_string) } }
}

fn answer(q: &Question, v: Verdict) -> Input {
    Input::Answer {
        answer: Answer { back: Default::default(), question: q.id.clone(), shown: q.shown(), verdicts: [("go".to_string(), v)].into(), by: By::Person, context: String::new() },
    }
}

#[test]
fn asked_question_is_answered_without_redoing_the_step() {
    let (mut run, q) = asked();
    let spent = run.spent.tokens;
    // the person asks "e se fosse Go?": a turn that only reads, in the session that made the option
    let (events, cmds) = feed(&mut run, ask(&q, "e se fosse Go?"));
    assert!(events.iter().any(|e| matches!(e, Event::AgentAsked { .. })), "{events:?}");
    assert!(!cmds.iter().any(|c| matches!(c, Command::RunAgent(_) | Command::RunChecks { .. })), "nothing is redone: {cmds:?}");
    let t = ask_turn(&cmds);
    assert_eq!((t.question.as_str(), t.step.as_str(), t.round, t.output.0.as_str()), (q.id.as_str(), "look", 0, "one"));
    assert_eq!((t.item.as_deref(), t.label.as_str(), t.text.as_str(), t.session.as_deref()), (Some("go"), "Go", "e se fosse Go?", Some("s1")));
    assert!(t.reach.project == ProjectAccess::Read && t.reach.write.is_empty() && t.reach.network == Network::default() && !t.reach.commands, "it only reads: {:?}", t.reach);
    assert!(t.tokens_left > 0 && t.deadline_seconds > 0);
    // the stop stays open, in the same round, with the same items
    assert!(run.questions[&q.id].open() && run.questions[&q.id].question.shown() == q.shown());
    // the agent answers: the answer is in the thread, the stop is as it was, the tokens count
    let (events, cmds) = feed(&mut run, answered(&t.ask, "Go keeps the type and swaps the green for a warm grey.", Some("s2")));
    assert!(events.iter().any(|e| matches!(e, Event::AgentAnswered { late: false, .. })), "{events:?}");
    assert!(cmds.iter().all(|c| matches!(c, Command::Notify { .. })), "nothing starts: {cmds:?}");
    assert!(run.questions[&q.id].open() && run.questions[&q.id].question.shown() == q.shown());
    assert_eq!(run.steps[0].done.as_ref().map(|d| d.output.0.as_str()), Some("one"), "the same output");
    assert_eq!(run.spent.tokens, spent + 100);
    let kinds: Vec<EntryKind> = run.steps[0].thread.iter().map(|e| e.kind).collect();
    assert_eq!(kinds, [EntryKind::Ask, EntryKind::Answer]);
    assert_eq!(run.steps[0].thread[1].text, "Go keeps the type and swaps the green for a warm grey.");
    assert!(run.steps[0].thread.iter().all(|e| e.round == 0 && e.question == q.id));
    // the session goes on from the answer, so the next round knows what was said
    assert_eq!(run.steps[0].session.as_deref(), Some("s2"));
    // the person asks for the change: the next round is given what was asked and answered, and the agent's words
    // beside its new output join the thread at round 1
    let (events, cmds) = feed(&mut run, answer(&q, Verdict::Change { text: "go with Go".into() }));
    assert!(cmds.iter().any(|c| matches!(c, Command::RunAgent(_))), "{events:?} {cmds:?}");
    let next = turn(&cmds);
    assert_eq!(next.asked, [Asked { text: "e se fosse Go?".into(), answer: Some("Go keeps the type and swaps the green for a warm grey.".into()) }]);
    deliver(&mut run, &next, "two", Some("I swapped the green for a warm grey, as you asked."));
    let kinds: Vec<(u32, EntryKind)> = run.steps[0].thread.iter().map(|e| (e.round, e.kind)).collect();
    assert_eq!(kinds, [(0, EntryKind::Ask), (0, EntryKind::Answer), (0, EntryKind::Change), (1, EntryKind::Reply)]);
    assert_eq!(run.steps[0].thread[2].text, "go with Go");
    assert_eq!(run.steps[0].thread[3].text, "I swapped the green for a warm grey, as you asked.");
    // the next round goes on in the session the answer left
    assert_eq!(next.session.as_deref(), Some("s2"));
}

#[test]
fn only_the_person_asks_an_open_stop_of_an_agent_step_and_one_ask_at_a_time() {
    let (mut run, q) = asked();
    let agent = Input::AskAgent { question: q.id.clone(), item: None, text: "why?".into(), by: By::Agent { who: "claude".into() } };
    assert!(refused(&run, agent).contains("only you"));
    let nowhere = Input::AskAgent { question: "nope".into(), item: None, text: "why?".into(), by: By::Person };
    assert!(refused(&run, nowhere).contains("no such question"));
    let other_item = Input::AskAgent { question: q.id.clone(), item: Some("moss".into()), text: "why?".into(), by: By::Person };
    assert!(refused(&run, other_item).contains("not among"));
    for text in ["", "   ", &"x".repeat(2001)] {
        assert!(refused(&run, ask(&q, text)).contains("words"), "{text:?}");
    }
    // a question about the whole output is taken, and a second waits for the first
    let whole = Input::AskAgent { question: q.id.clone(), item: None, text: "why this one?".into(), by: By::Person };
    let (_, cmds) = feed(&mut run, whole);
    assert_eq!(ask_turn(&cmds).label, "");
    assert!(refused(&run, ask(&q, "and Go?")).contains("is answering"));
    // a stop answered takes no ask
    let (mut done, q) = asked();
    feed(&mut done, answer(&q, Verdict::Change { text: "warmer".into() }));
    assert!(refused(&done, ask(&q, "why?")).contains("not open"));
    let (mut finished, q) = asked();
    feed(&mut finished, answer(&q, Verdict::Keep));
    assert!(finished.finished() && refused(&finished, ask(&q, "why?")).contains("finished"));
    // nor a stop of a step the role makes, which has no agent to ask
    let (role, q) = asked_with(Executor::Role);
    assert!(refused(&role, ask(&q, "why?")).contains("no agent"));
}

#[test]
fn an_ask_that_fails_says_why_in_the_thread_and_may_be_asked_again() {
    let (mut run, q) = asked();
    let (_, cmds) = feed(&mut run, ask(&q, "e se fosse Go?"));
    let t = ask_turn(&cmds);
    let failed = Input::AskEnded { ask: t.ask.clone(), outcome: AskOutcome::Failed { reason: "the agent did not start".into(), usage: usage(7) } };
    let (events, _) = feed(&mut run, failed.clone());
    assert!(events.iter().any(|e| matches!(e, Event::AskFailed { .. })));
    assert_eq!(run.steps[0].thread.last().map(|e| (e.kind, e.text.as_str())), Some((EntryKind::AskFailed, "the agent did not start")));
    assert_eq!(run.steps[0].session.as_deref(), Some("s1"), "the session stays");
    // an ask that ended is not ended twice, nor one never asked
    assert!(refused(&run, failed).contains("no such ask"));
    assert!(refused(&run, answered("nope", "hi", None)).contains("no such ask"));
    // asked again, with a new id
    let (_, cmds) = feed(&mut run, ask(&q, "e se fosse Go?"));
    assert_ne!(ask_turn(&cmds).ask, t.ask);
}

#[test]
fn an_answer_after_the_stop_was_answered_is_kept_late_and_changes_nothing() {
    let (mut run, q) = asked();
    let (_, cmds) = feed(&mut run, ask(&q, "e se fosse Go?"));
    let t = ask_turn(&cmds);
    // the person chooses meanwhile, and the run finishes
    feed(&mut run, answer(&q, Verdict::Keep));
    assert!(run.finished());
    let before = run.steps[0].clone();
    let (events, cmds) = feed(&mut run, answered(&t.ask, "Go would be warmer.", Some("s9")));
    assert!(events.iter().any(|e| matches!(e, Event::AgentAnswered { late: true, .. })), "{events:?}");
    assert!(cmds.is_empty(), "{cmds:?}");
    assert_eq!(run.steps[0].session, before.session, "a late answer moves no session");
    assert_eq!(run.steps[0].status, before.status);
    assert!(run.steps[0].thread.last().is_some_and(|e| e.kind == EntryKind::Answer && e.late));
    assert!(run.asking.is_empty(), "no ask is left under way");
    // nothing else is taken by the finished run
    assert!(refused(&run, Input::Tick).contains("finished"));
}

#[test]
fn jaynes_stopped_while_the_agent_answered_says_ask_again() {
    let (mut run, q) = asked();
    let (_, cmds) = feed(&mut run, ask(&q, "e se fosse Go?"));
    let t = ask_turn(&cmds);
    let (events, _) = feed(&mut run, Input::Recovered);
    assert!(events.iter().any(|e| matches!(e, Event::AskFailed { ask, reason, .. } if *ask == t.ask && reason.contains("ask again"))), "{events:?}");
    assert!(run.steps[0].status == Status::Done && run.questions[&q.id].open());
    feed(&mut run, ask(&q, "e se fosse Go?"));
}

#[test]
fn no_ask_once_the_tokens_are_spent() {
    let (mut run, q) = asked();
    run.spent.tokens = run.ceiling.tokens;
    assert!(refused(&run, ask(&q, "e se fosse Go?")).contains("tokens"));
}

