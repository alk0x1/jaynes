//! Sending work back, for any role: what the person sends back goes to the step that made the work, not to the one
//! that reviewed it; who redoes it sees the reviewer's word and everything the person asked before; rounds the person
//! opens have no ceiling, since the person decides when to stop; and agents redoing without the person stop at their
//! attempts and call the person, so making and rejecting never goes on forever without them.

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
        hidden: false, idempotent: false,
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
    Manifest { role: "layouts".into(), version: "1".into(), protocol: 1, steps: vec![draw, review, apply], target: Default::default(), delivery: Default::default() }
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
        attention: BTreeMap::new(), task: None, arm: None, person: None,
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
        answer: Answer { back: Default::default(),
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
fn rounds_the_person_opens_have_no_ceiling_so_a_fifth_adjustment_is_taken() {
    let (mut run, mut q) = started(true, true);
    assert_eq!(q.pass, 1);
    for n in 2..=5 {
        let verdicts: Vec<(&str, Verdict)> = [("B", Verdict::Change { text: format!("adjust {n}") }), ("A", Verdict::Drop)]
            .into_iter()
            .filter(|(id, _)| q.items.iter().any(|x| x.id == *id))
            .collect();
        let (_, cmds) = feed(&mut run, answer(&q, &verdicts));
        let (_, cmds) = deliver(&mut run, &turn(&cmds), &format!("draw-{n}"), vec![item("B", &format!("b{n}"))]);
        let (events, _) = deliver(&mut run, &turn(&cmds), &format!("review-{n}"), vec![item("B", &format!("rb{n}"))]);
        q = asked(&events);
        assert_eq!(q.pass, n, "the round is still counted, as information");
        assert_eq!(q.passes, 0, "a round the person opened has no ceiling");
    }
    // the fifth adjustment in a row is taken like the first, and goes back to the maker
    let (_, cmds) = feed(&mut run, answer(&q, &[("B", Verdict::Change { text: "adjust 6".into() })]));
    let t = turn(&cmds);
    assert_eq!(t.step, "draw");
    assert!(t.notes.join("\n").contains("adjust 6"), "{:?}", t.notes);
    assert!(t.earlier.join("\n").contains("adjust 2"), "{:?}", t.earlier);
    assert!(!run.finished());
}

#[test]
fn a_stop_without_a_maker_named_sends_back_to_itself_as_before() {
    let (mut run, q) = started(false, false);
    let (_, cmds) = feed(&mut run, answer(&q, &[("B", Verdict::Change { text: "warmer".into() })]));
    assert_eq!(turn(&cmds).step, "review");
}

#[test]
fn the_person_s_context_at_a_stop_reaches_the_maker_it_sends_back_to() {
    let (mut run, q) = started(true, true);
    let Input::Answer { mut answer } = answer(&q, &[("A", Verdict::Drop), ("B", Verdict::Drop)]) else { unreachable!() };
    answer.context = "I like the colours, but money should be green".into();
    let (events, cmds) = feed(&mut run, Input::Answer { answer });
    let fx = events.iter().find_map(|e| if let Event::AnswerRecorded { effects, .. } = e { Some(effects.clone()) } else { None }).expect("recorded");
    assert!(fx.sentences.join(" ").contains("goes with it to draw"), "{:?}", fx.sentences);
    // what the page promised: the maker, before the stop, reads the person's words too
    let t = turn(&cmds);
    assert_eq!(t.step, "draw");
    assert!(t.context.iter().any(|c| c.contains("money should be green")), "{:?}", t.context);
}

#[test]
fn agents_redoing_without_the_person_stop_at_their_ceiling_and_call_the_person_without_ending_the_run() {
    use jaynes_core::run::Phase;
    let out = decide(None, &Input::Start {
        run: "r".into(),
        request: "a screen".into(),
        manifest: manifest(true, true),
        role_fingerprint: Digest("f".into()),
        agent: "claude:m1".into(),
        raise: None,
        attention: BTreeMap::new(), task: None, arm: None, person: None,
    })
    .expect("start");
    let mut run = Run::fold(&out.events).expect("run");
    let (_, mut cmds) = settle(&mut run, Vec::new(), out.commands);
    // the reviewing check turns the maker's work down every time, and nobody answers in between
    let mut turns = 0;
    let mut notified = false;
    while let Some(t) = cmds.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.clone()) } else { None }) {
        turns += 1;
        assert!(turns < 20, "agents went on without the person");
        assert_eq!(t.step, "draw");
        feed(&mut run, Input::AttemptEnded {
            step: t.step.clone(),
            attempt: t.attempt,
            fix: t.fix,
            outcome: AttemptOutcome::Delivered { output: Digest(format!("draw-{turns}")), items: vec![item("A", "a")], changed: Vec::new(), usage: Usage::default(), session: Some("s".into()) },
        });
        let reasons = vec!["the director turns it down".to_string()];
        let (_, next) = feed(&mut run, Input::ChecksEnded { step: t.step.clone(), attempt: t.attempt, fix: t.fix, results: vec![CheckResult { check: "shape".into(), verdict: CheckVerdict::Fail { reasons, detail: String::new() } }] });
        notified |= next.iter().any(|c| matches!(c, Command::Notify { .. }));
        cmds = next;
    }
    assert!(matches!(run.phase, Phase::Blocked { .. }), "the run waits for the person: {:?}", run.phase);
    assert!(!run.finished(), "the run is not ended");
    assert!(notified, "the person is called");
    assert_eq!(turns, 9, "three attempts of three tries each, then the person");
}

fn answer_bringing_back(q: &Question, verdicts: &[(&str, Verdict)], back: &[(&str, &str)]) -> Input {
    let Input::Answer { mut answer } = answer(q, verdicts) else { unreachable!() };
    answer.back = back.iter().map(|(k, w)| (k.to_string(), w.to_string())).collect();
    Input::Answer { answer }
}

/// None of the first two: both go, with what was missing; the maker makes C, which the person is asked about.
fn after_none() -> (Run, Question) {
    let (mut run, q) = started(true, true);
    let Input::Answer { mut answer } = answer(&q, &[("A", Verdict::Drop), ("B", Verdict::Drop)]) else { unreachable!() };
    answer.context = "something calmer".into();
    let (_, cmds) = feed(&mut run, Input::Answer { answer });
    let (_, cmds) = deliver(&mut run, &turn(&cmds), "draw-2", vec![item("C", "c2")]);
    let (events, _) = deliver(&mut run, &turn(&cmds), "review-2", vec![item("C", "rc2")]);
    (run, asked(&events))
}

#[test]
fn what_the_person_dropped_stays_in_view_and_they_may_bring_it_back() {
    let (mut run, q) = after_none();
    assert_eq!(q.back.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["A", "B"], "what was dropped stays in view");
    // the person changes their mind: A comes back, adjusted, and C goes
    let (events, cmds) = feed(&mut run, answer_bringing_back(&q, &[("C", Verdict::Drop)], &[("A", "money in green")]));
    let fx = events.iter().find_map(|e| if let Event::AnswerRecorded { effects, .. } = e { Some(effects.clone()) } else { None }).expect("recorded");
    assert!(fx.sentences.join(" ").contains("“A”, which you dropped in an earlier round, comes back"), "{:?}", fx.sentences);
    let t = turn(&cmds);
    assert_eq!(t.step, "draw", "the maker brings it back");
    let back: Vec<(&str, &str, &str)> = t.back.iter().map(|b| (b.item.id.as_str(), b.output.0.as_str(), b.words.as_str())).collect();
    assert_eq!(back, [("A", "draw-1", "money in green")], "with the output it was in");
    assert!(!t.dropped.iter().any(|x| x.id == "A"), "A may come back: {:?}", t.dropped);
    assert!(t.dropped.iter().any(|x| x.id == "B") && t.dropped.iter().any(|x| x.id == "C"), "what was not brought back stays out: {:?}", t.dropped);
    assert!(t.notes.join("\n").contains("money in green"), "{:?}", t.notes);
    // A comes back and passes; B, not brought back, still may not
    let (events, _) = deliver(&mut run, &t, "draw-3", vec![item("A", "a3"), item("B", "b1")]);
    assert!(!events.iter().any(|e| matches!(e, Event::StepDone { .. })), "B came back unasked: {events:?}");
}

#[test]
fn bringing_one_back_alone_is_a_choice_of_it_made_by_the_maker() {
    let (mut run, q) = after_none();
    let (_, cmds) = feed(&mut run, answer_bringing_back(&q, &[("C", Verdict::Drop)], &[("B", "")]));
    let t = turn(&cmds);
    assert_eq!(t.step, "draw");
    assert!(t.notes.join("\n").contains("“B”, which the person dropped in an earlier round, comes back as it was"), "{:?}", t.notes);
}

#[test]
fn only_the_person_brings_back_and_only_what_was_dropped() {
    let (run, q) = after_none();
    let Input::Answer { answer: mut a } = answer_bringing_back(&q, &[], &[("A", "")]) else { unreachable!() };
    a.by = By::Agent { who: "claude".into() };
    let e = match decide(Some(&run), &Input::Answer { answer: a }) { Ok(_) => panic!("an agent brought back"), Err(r) => r.to_string() };
    assert!(e.contains("yours"), "{e}");
    let e = match decide(Some(&run), &answer_bringing_back(&q, &[], &[("Z", "")])) { Ok(_) => panic!("brought back what never was"), Err(r) => r.to_string() };
    assert!(e.contains("no earlier option Z"), "{e}");
}
