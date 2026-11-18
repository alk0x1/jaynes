//! A stop that chooses: the step makes several options, the person chooses one with a click, and may write how to
//! adjust it. Without a note the run goes on with the chosen one alone, kept by Jaynes and not the agent, and the
//! stop does not ask again. With a note the agent works again, seeing every option, and the stop shows only what it
//! made.

use jaynes_core::digest::Digest;
use jaynes_core::event::{Event, Usage};
use jaynes_core::manifest::{CheckDecl, CheckKind, Effect, Executor, Manifest, ProjectAccess, Rate, Reach, SessionKind, StepDecl, StopDecl};
use jaynes_core::question::{Answer, By, Item, Question, Verdict, VerdictKind};
use jaynes_core::risk::Risk;
use jaynes_core::run::Phase;
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

fn choosing() -> StopDecl {
    StopDecl {
        verdicts: vec![VerdictKind::Keep, VerdictKind::Change, VerdictKind::Drop],
        holds: true,
        from_risk: Risk::Low,
        release: false,
        budget: None,
        audit_rate: Rate(0.1),
        about: "Which direction?".into(),
        choose: true,
        redo: None,
        rounds: None,
    }
}

fn manifest() -> Manifest {
    let mut directions = step("directions", &[]);
    directions.stop = Some(choosing());
    let apply = step("apply", &["directions"]);
    Manifest { role: "identity".into(), version: "1".into(), protocol: 1, steps: vec![directions, apply], target: Default::default(), delivery: Default::default() }
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
        Ok(_) => panic!("taken: {input:?}"),
        Err(r) => r.to_string(),
    }
}

fn item(id: &str, d: &str) -> Item {
    Item { id: id.into(), label: id.into(), digest: Digest(d.into()) }
}

/// Pass every probe the commands ask for; the commands that follow.
fn probed(run: &mut Run, cmds: Vec<Command>) -> Vec<Command> {
    let mut out = Vec::new();
    for c in cmds {
        match c {
            Command::Probe { step, .. } => {
                let next = feed(run, Input::ProbeEnded { step, ok: true, reason: String::new() }).1;
                out.extend(probed(run, next));
            }
            other => out.push(other),
        }
    }
    out
}

fn turn(cmds: &[Command]) -> AgentTurn {
    cmds.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.clone()) } else { None }).unwrap_or_else(|| panic!("no turn: {cmds:?}"))
}

/// Deliver `items` for the turn under way and pass its checks; the commands that follow.
fn deliver(run: &mut Run, t: &AgentTurn, output: &str, items: Vec<Item>) -> (Vec<Event>, Vec<Command>) {
    let (_, cmds) = feed(run, Input::AttemptEnded {
        step: t.step.clone(),
        attempt: t.attempt,
        fix: 0,
        outcome: AttemptOutcome::Delivered { output: Digest(output.into()), items, changed: Vec::new(), usage: Usage::default(), session: Some("s".into()), tree: None },
    });
    assert!(cmds.iter().any(|c| matches!(c, Command::RunChecks { .. })), "{cmds:?}");
    feed(run, Input::ChecksEnded { step: t.step.clone(), attempt: t.attempt, fix: 0, results: vec![CheckResult { check: "shape".into(), verdict: CheckVerdict::Pass, tree: None }] })
}

/// A run whose directions step made three options and asks which goes on.
fn asked() -> (Run, Question) {
    let out = decide(None, &Input::Start {
        run: "r".into(),
        request: "a look".into(),
        manifest: manifest(),
        role_fingerprint: Digest("f".into()),
        agent: "claude:m1".into(),
        raise: None,
        attention: BTreeMap::new(), task: None, arm: None, never_merge: false
    })
    .expect("start");
    let mut run = Run::fold(&out.events).expect("run");
    let (_, cmds) = feed(&mut run, Input::ProbeEnded { step: "directions".into(), ok: true, reason: String::new() });
    let t = turn(&cmds);
    let (events, _) = deliver(&mut run, &t, "three", vec![item("Ink", "i"), item("Sun", "s"), item("Moss", "m")]);
    let q = events.iter().find_map(|e| if let Event::QuestionOpened { question } = e { Some(question.clone()) } else { None }).expect("asked");
    (run, q)
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
fn the_question_of_a_choosing_stop_says_it_chooses() {
    let (_, q) = asked();
    assert!(q.choose);
    assert_eq!(q.items.len(), 3);
}

#[test]
fn choosing_without_a_note_keeps_the_chosen_one_alone_without_the_agent_and_asks_nothing_more() {
    let (mut run, q) = asked();
    let (events, cmds) = feed(&mut run, answer(&q, &[("Sun", Verdict::Keep), ("Ink", Verdict::Drop), ("Moss", Verdict::Drop)]));
    let effects = events.iter().find_map(|e| if let Event::AnswerRecorded { effects, .. } = e { Some(effects.clone()) } else { None }).expect("recorded");
    let said = effects.sentences.join(" ");
    assert!(said.contains("“Sun” is chosen") && said.contains("keeps only what you chose, without the agent"), "{said}");
    // the round that keeps the chosen one is Jaynes's: the turn names what to keep
    let t = turn(&cmds);
    assert_eq!(t.keep, Some(vec!["Sun".to_string()]));
    assert_eq!(t.previous, Some(Digest("three".into())));
    let (events, cmds) = deliver(&mut run, &t, "sun", vec![item("Sun", "s")]);
    assert!(!events.iter().any(|e| matches!(e, Event::QuestionOpened { .. })), "nothing new to ask: {events:?}");
    // the run goes straight on to apply, which reads the chosen one alone
    let next = turn(&probed(&mut run, cmds));
    assert_eq!(next.step, "apply");
    assert_eq!(next.given[0].output, Digest("sun".into()));
    // the choice is not revised once acted on
    let again = refused(&run, answer(&q, &[("Moss", Verdict::Keep), ("Ink", Verdict::Drop), ("Sun", Verdict::Drop)]));
    assert!(again.contains("already acted on"), "{again}");
}

#[test]
fn choosing_with_a_note_has_the_agent_combine_seeing_every_option_and_asks_about_the_result_alone() {
    let (mut run, q) = asked();
    let note = Verdict::Change { text: "with the font of Ink".into() };
    let (_, cmds) = feed(&mut run, answer(&q, &[("Sun", note), ("Ink", Verdict::Drop), ("Moss", Verdict::Drop)]));
    let t = turn(&cmds);
    assert_eq!(t.keep, None, "the agent works this round");
    assert_eq!(t.previous, Some(Digest("three".into())), "it sees every option it made");
    let notes = t.notes.join("\n");
    assert!(notes.contains("“Sun” was chosen") && notes.contains("with the font of Ink"), "{notes}");
    assert!(notes.contains("“Ink” was not chosen"), "{notes}");
    // taking the font of Ink does not bring Ink back: the result is Sun, changed
    let (events, _) = deliver(&mut run, &t, "sun2", vec![item("Sun", "s2")]);
    let q2 = events.iter().find_map(|e| if let Event::QuestionOpened { question } = e { Some(question.clone()) } else { None }).expect("asked again");
    assert_eq!(q2.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["Sun"]);
    // confirming it lets the run go on
    let (_, cmds) = feed(&mut run, answer(&q2, &[("Sun", Verdict::Keep)]));
    assert_eq!(turn(&probed(&mut run, cmds)).step, "apply");
}

#[test]
fn a_choice_keeps_or_changes_exactly_one_option_and_drops_the_rest() {
    let (run, q) = asked();
    let two = refused(&run, answer(&q, &[("Sun", Verdict::Keep), ("Ink", Verdict::Keep), ("Moss", Verdict::Drop)]));
    assert!(two.contains("choose one"), "{two}");
    let kept_and_changed = refused(&run, answer(&q, &[("Sun", Verdict::Keep), ("Ink", Verdict::Change { text: "warmer".into() }), ("Moss", Verdict::Drop)]));
    assert!(kept_and_changed.contains("choose one"), "{kept_and_changed}");
}

#[test]
fn a_round_that_keeps_more_than_was_chosen_does_not_pass() {
    let (mut run, q) = asked();
    let (_, cmds) = feed(&mut run, answer(&q, &[("Sun", Verdict::Keep), ("Ink", Verdict::Drop), ("Moss", Verdict::Drop)]));
    let t = turn(&cmds);
    let (events, _) = deliver(&mut run, &t, "both", vec![item("Sun", "s"), item("Ink", "i")]);
    assert!(!events.iter().any(|e| matches!(e, Event::StepDone { .. })), "{events:?}");
    assert!(matches!(run.phase, Phase::Blocked { .. }), "the role's fault, not the agent's: {:?}", run.phase);
}

#[test]
fn finalists_go_back_to_the_agent_to_be_developed_and_the_rest_leave() {
    let (mut run, q) = asked();
    let more = || Verdict::Change { text: "develop it further".into() };
    let (events, cmds) = feed(&mut run, answer(&q, &[("Sun", more()), ("Moss", more()), ("Ink", Verdict::Drop)]));
    let said = events.iter().find_map(|e| if let Event::AnswerRecorded { effects, .. } = e { Some(effects.sentences.join(" ")) } else { None }).expect("recorded");
    assert!(said.contains("“Sun” is a finalist") && said.contains("you choose again"), "{said}");
    let t = turn(&cmds);
    assert_eq!(t.keep, None, "the agent develops the finalists");
    let notes = t.notes.join("\n");
    assert!(notes.contains("“Moss” is a finalist") && notes.contains("“Ink” was not chosen"), "{notes}");
    let (events, _) = deliver(&mut run, &t, "two", vec![item("Sun", "s2"), item("Moss", "m2")]);
    let q2 = events.iter().find_map(|e| if let Event::QuestionOpened { question } = e { Some(question.clone()) } else { None }).expect("asked again");
    assert_eq!(q2.items.len(), 2);
}

#[test]
fn finalists_are_developed_never_kept_as_they_are() {
    let (run, q) = asked();
    let e = refused(&run, answer(&q, &[("Sun", Verdict::Keep), ("Moss", Verdict::Keep), ("Ink", Verdict::Drop)]));
    assert!(e.contains("choose one"), "{e}");
}

#[test]
fn none_sends_every_option_away_with_the_person_s_words_and_new_ones_are_made() {
    let (mut run, q) = asked();
    let mut none = answer(&q, &[("Sun", Verdict::Drop), ("Moss", Verdict::Drop), ("Ink", Verdict::Drop)]);
    let e = refused(&run, none.clone());
    assert!(e.contains("say what was missing"), "{e}");
    if let Input::Answer { answer } = &mut none {
        answer.context = "too cold, I want something warmer".into();
    }
    let (events, cmds) = feed(&mut run, none);
    let said = events.iter().find_map(|e| if let Event::AnswerRecorded { effects, .. } = e { Some(effects.sentences.join(" ")) } else { None }).expect("recorded");
    assert!(said.contains("makes new options"), "{said}");
    let t = turn(&cmds);
    assert!(t.context.iter().any(|c| c.contains("warmer")), "{:?}", t.context);
    assert!(t.notes.join("\n").contains("None was chosen"), "{:?}", t.notes);
    // an option sent away does not come back
    let (events, _) = deliver(&mut run, &t, "again", vec![item("Ink", "i"), item("Ember", "e")]);
    assert!(!events.iter().any(|e| matches!(e, Event::QuestionOpened { .. })), "Ink came back: {events:?}");
}
