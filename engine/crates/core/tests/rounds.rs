//! Sending work back, for any role: what the person sends back goes to the step that made the work, not to the one
//! that reviewed it; who redoes it sees the reviewer's word and everything the person asked before; and a stop sends
//! back at most a few times, so making and rejecting never goes on forever.

use jaynes_core::digest::Digest;
use jaynes_core::event::{Event, Usage};
use jaynes_core::manifest::{CheckDecl, CheckKind, Effect, Executor, Manifest, ProjectAccess, Rate, Reach, SessionKind, StepDecl, StopDecl};
use jaynes_core::question::{Answer, By, Item, Question, Verdict, VerdictKind};
use jaynes_core::risk::Risk;
use jaynes_core::{decide, AgentTurn, AttemptOutcome, CheckResult, CheckVerdict, Command, Input, Run};
use std::collections::BTreeMap;

fn step(name: &str, needs: &[&str]) -> StepDecl {
    StepDecl {
        name: name.to_string(),
        needs: needs.iter().map(|s| s.to_string()).collect(),
        executor: Executor::Agent,
        session: SessionKind::Fresh,
        reach: Reach { project: ProjectAccess::Write, write: vec!["design/**".into()], ..Reach::default() },
        effect: Effect::None,
        checks: vec![CheckDecl { name: "shape".into(), kind: CheckKind::Open, about: String::new(), reads: Vec::new() }],
        stop: None,
        from_risk: Risk::Low,
        attempts: 3,
        fixes: 2,
        hidden: false,
        about: String::new(),
    }
}

fn stop(choose: bool, redo: Option<&str>) -> StopDecl {
    StopDecl {
        verdicts: vec![VerdictKind::Keep, VerdictKind::Change, VerdictKind::Drop],
        holds: true,
        from_risk: Risk::Low,
        release: false,
        budget: None,
        audit_rate: Rate(0.1),
        about: "Which one?".into(),
        choose,
        redo: redo.map(str::to_string),
        rounds: None,
    }
}

/// draw (the maker) → review (the reviewer, asking the person) → apply.
fn manifest(choose: bool, redo: bool) -> Manifest {
    let draw = step("draw", &[]);
    let mut review = step("review", &["draw"]);
    review.stop = Some(stop(choose, redo.then_some("draw")));
    let apply = step("apply", &["review"]);
    Manifest { role: "layouts".into(), version: "1".into(), protocol: 1, steps: vec![draw, review, apply], delivery: Default::default() }
}

fn feed(run: &mut Run, input: Input) -> (Vec<Event>, Vec<Command>) {
    let out = decide(Some(run), &input).unwrap_or_else(|r| panic!("refused {input:?}: {r}"));
    for e in &out.events {
        run.apply(e);
    }
    settle(run, out.events, out.commands)
}

/// Pass every probe the commands ask for.
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

fn refused(run: &Run, input: Input) -> String {
    match decide(Some(run), &input) {
        Ok(_) => panic!("taken: {input:?}"),
        Err(r) => r.to_string(),
    }
}

fn item(id: &str, d: &str) -> Item {
    Item { id: id.into(), label: id.into(), digest: Digest(d.into()) }
}

fn turn(cmds: &[Command]) -> AgentTurn {
    cmds.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.clone()) } else { None }).unwrap_or_else(|| panic!("no turn: {cmds:?}"))
}

fn deliver(run: &mut Run, t: &AgentTurn, output: &str, items: Vec<Item>) -> (Vec<Event>, Vec<Command>) {
    feed(run, Input::AttemptEnded {
        step: t.step.clone(),
        attempt: t.attempt,
        fix: 0,
        outcome: AttemptOutcome::Delivered { output: Digest(output.into()), items, changed: Vec::new(), usage: Usage::default(), session: Some("s".into()) },
    });
    feed(run, Input::ChecksEnded { step: t.step.clone(), attempt: t.attempt, fix: 0, results: vec![CheckResult { check: "shape".into(), verdict: CheckVerdict::Pass }] })
}

fn asked(events: &[Event]) -> Question {
    events.iter().find_map(|e| if let Event::QuestionOpened { question } = e { Some(question.clone()) } else { None }).expect("asked")
}

/// A run whose draw and review are done, and whose review asks the person.
fn started(choose: bool, redo: bool) -> (Run, Question) {
    let out = decide(None, &Input::Start {
        run: "r".into(),
        request: "a screen".into(),
        manifest: manifest(choose, redo),
        role_fingerprint: Digest("f".into()),
        agent: "claude:m1".into(),
        raise: None,
        attention: BTreeMap::new(),
    })
    .expect("start");
    let mut run = Run::fold(&out.events).expect("run");
    let (_, cmds) = settle(&mut run, Vec::new(), out.commands);
    let (_, cmds) = deliver(&mut run, &turn(&cmds), "draw-1", vec![item("A", "a1"), item("B", "b1")]);
    let (events, _) = deliver(&mut run, &turn(&cmds), "review-1", vec![item("A", "ra1"), item("B", "rb1")]);
    (run, asked(&events))
}

fn answer(q: &Question, verdicts: &[(&str, Verdict)]) -> Input {
    Input::Answer {
        answer: Answer {
            question: q.id.clone(),
            shown: q.shown(),
            verdicts: verdicts.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
            by: By::Person,
            context: String::new(),
        },
    }
}

#[test]
fn what_the_person_sends_back_goes_to_the_step_that_made_it_with_the_reviewer_s_word() {
    let (mut run, q) = started(true, true);
    let (events, cmds) = feed(&mut run, answer(&q, &[("B", Verdict::Change { text: "the menu on the left".into() }), ("A", Verdict::Drop)]));
    let fx = events.iter().find_map(|e| if let Event::AnswerRecorded { effects, .. } = e { Some(effects.clone()) } else { None }).expect("recorded");
    assert_eq!(fx.reruns, ["draw"], "{:?}", fx.sentences);
    assert!(fx.sentences.join(" ").contains("review looks at it again"), "{:?}", fx.sentences);
    let t = turn(&cmds);
    assert_eq!(t.step, "draw", "the maker redoes it, not the reviewer");
    assert!(t.notes.join("\n").contains("the menu on the left"), "{:?}", t.notes);
    assert_eq!(t.review.as_ref().map(|r| (r.step.as_str(), r.output.0.as_str())), Some(("review", "review-1")), "it sees what the reviewer said");
    // the reviewer looks again at what the maker made, before the person sees it
    let (_, cmds) = deliver(&mut run, &t, "draw-2", vec![item("B", "b2")]);
    let r = turn(&cmds);
    assert_eq!(r.step, "review");
    let (events, _) = deliver(&mut run, &r, "review-2", vec![item("B", "rb2")]);
    assert_eq!(asked(&events).items.len(), 1);
}

#[test]
fn an_option_sent_away_may_not_come_back_from_the_maker() {
    let (mut run, q) = started(true, true);
    let (_, cmds) = feed(&mut run, answer(&q, &[("B", Verdict::Change { text: "warmer".into() }), ("A", Verdict::Drop)]));
    let (events, _) = deliver(&mut run, &turn(&cmds), "draw-2", vec![item("A", "a1"), item("B", "b2")]);
    assert!(!events.iter().any(|e| matches!(e, Event::StepDone { .. })), "A came back: {events:?}");
}

#[test]
fn who_redoes_sees_everything_the_person_asked_before() {
    let (mut run, q) = started(true, true);
    let (_, cmds) = feed(&mut run, answer(&q, &[("B", Verdict::Change { text: "the menu on the left".into() }), ("A", Verdict::Drop)]));
    let (_, cmds) = deliver(&mut run, &turn(&cmds), "draw-2", vec![item("B", "b2")]);
    let (events, _) = deliver(&mut run, &turn(&cmds), "review-2", vec![item("B", "rb2")]);
    let q2 = asked(&events);
    let (_, cmds) = feed(&mut run, answer(&q2, &[("B", Verdict::Change { text: "bigger numbers".into() })]));
    let t = turn(&cmds);
    assert!(t.notes.join("\n").contains("bigger numbers"), "{:?}", t.notes);
    assert!(t.earlier.join("\n").contains("the menu on the left"), "{:?}", t.earlier);
}

#[test]
fn a_stop_sends_back_three_times_at_most_then_the_person_keeps_one_or_abandons() {
    let (mut run, mut q) = started(true, true);
    assert_eq!((q.pass, q.passes), (1, 3));
    for n in 2..=3 {
        let (_, cmds) = feed(&mut run, answer(&q, &[("B", Verdict::Change { text: format!("round {n}") }), ("A", Verdict::Drop)].into_iter().filter(|(id, _)| q.items.iter().any(|x| x.id == *id)).collect::<Vec<_>>()));
        let (_, cmds) = deliver(&mut run, &turn(&cmds), &format!("draw-{n}"), vec![item("B", &format!("b{n}"))]);
        let (events, _) = deliver(&mut run, &turn(&cmds), &format!("review-{n}"), vec![item("B", &format!("rb{n}"))]);
        q = asked(&events);
        assert_eq!((q.pass, q.passes), (n, 3));
    }
    let e = refused(&run, answer(&q, &[("B", Verdict::Change { text: "once more".into() })]));
    assert!(e.contains("last round"), "{e}");
    let (_, cmds) = feed(&mut run, answer(&q, &[("B", Verdict::Keep)]));
    assert_eq!(turn(&cmds).step, "apply");
}

#[test]
fn a_stop_without_a_maker_named_sends_back_to_itself_as_before() {
    let (mut run, q) = started(false, false);
    let (_, cmds) = feed(&mut run, answer(&q, &[("B", Verdict::Change { text: "warmer".into() })]));
    assert_eq!(turn(&cmds).step, "review");
}
