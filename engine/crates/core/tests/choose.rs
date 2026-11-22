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
        model: Default::default(),
        turns: None,
    }
}

fn choosing() -> StopDecl {
    StopDecl { settled: None, holds_unless: None, vouched_by: None,
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
        answers: Default::default(),
        note: String::new(),
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
    Item { named: None, id: id.into(), label: id.into(), digest: Digest(d.into()) }
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
        outcome: AttemptOutcome::Delivered { output: Digest(output.into()), items, changed: Vec::new(), usage: Usage::default(), session: Some("s".into()), tree: None, reply: None },
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
        attention: BTreeMap::new(), task: None, arm: None, never_merge: false,
        starter: None,
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
    assert!(said.contains("“Sun” is shortlisted") && said.contains("you choose again"), "{said}");
    let t = turn(&cmds);
    assert_eq!(t.keep, None, "the agent develops the finalists");
    let notes = t.notes.join("\n");
    assert!(notes.contains("“Moss” is a finalist") && notes.contains("“Ink” was not chosen"), "{notes}");
    let (events, _) = deliver(&mut run, &t, "two", vec![item("Sun", "s2"), item("Moss", "m2")]);
    let q2 = events.iter().find_map(|e| if let Event::QuestionOpened { question } = e { Some(question.clone()) } else { None }).expect("asked again");
    assert_eq!(q2.items.len(), 2);
}

fn effects_of(run: &Run, q: &Question, verdicts: &[(&str, Verdict)]) -> Vec<String> {
    let Input::Answer { answer } = answer(q, verdicts) else { unreachable!("an answer") };
    jaynes_core::effects(run, q, &answer).sentences
}

#[test]
fn a_shortlisted_option_is_said_in_one_sentence_with_the_parts_that_stay() {
    let (run, q) = asked();
    let note = |rest: &str| Verdict::Change { text: format!("{}{rest}", jaynes_core::decide::SHORTLISTED) };
    let said = effects_of(&run, &q, &[("Sun", note(" Its palette stays.")), ("Moss", note(" Its palette and type stay.")), ("Ink", Verdict::Drop)]);
    assert!(said.contains(&"“Sun” is shortlisted: the agent develops it further, keeping what it is. Its palette stays.".to_string()), "{said:?}");
    assert!(said.contains(&"“Moss” is shortlisted: the agent develops it further, keeping what it is. Its palette and type stay.".to_string()), "{said:?}");
    // a note from before the word changed says the same; one of the person's own goes in without a quote inside the quote
    let old = Verdict::Change { text: "A finalist: develop it further, keeping what it is.".into() };
    let said = effects_of(&run, &q, &[("Sun", old), ("Moss", Verdict::Change { text: "warmer".into() }), ("Ink", Verdict::Drop)]);
    assert!(said.contains(&"“Sun” is shortlisted: the agent develops it further, keeping what it is.".to_string()), "{said:?}");
    assert!(said.contains(&"“Moss” is shortlisted: the agent develops it as you wrote: warmer.".to_string()), "{said:?}");
    assert!(said.iter().all(|s| !s.contains(".”.") && !s.contains(": “")), "{said:?}");
}

#[test]
fn before_it_is_sent_a_choice_says_it_is_being_made() {
    let (run, q) = asked();
    let said = effects_of(&run, &q, &[("Sun", Verdict::Keep), ("Moss", Verdict::Drop), ("Ink", Verdict::Drop)]);
    let chosen = said.iter().find(|s| s.starts_with("“Sun”")).expect("said");
    assert_eq!(chosen, "“Sun” is chosen: no later round of directions may change it.");
    assert_eq!(jaynes_core::decide::before_sending(chosen), "Choosing “Sun”: no later round of directions may change it.");
    // what is not the choice says what it says
    let gone = said.iter().find(|s| s.starts_with("“Ink”")).expect("said");
    assert_eq!(&jaynes_core::decide::before_sending(gone), gone);
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

/// A run whose directions step made one option only, as an agent often does: the stop asks whether it goes on.
fn asked_one(label: &str) -> (Run, Question) {
    let out = decide(None, &Input::Start {
        run: "r".into(),
        request: "a stack".into(),
        manifest: manifest(),
        role_fingerprint: Digest("f".into()),
        agent: "claude:m1".into(),
        raise: None,
        attention: BTreeMap::new(), task: None, arm: None, never_merge: false,
        starter: None,
    })
    .expect("start");
    let mut run = Run::fold(&out.events).expect("run");
    let (_, cmds) = feed(&mut run, Input::ProbeEnded { step: "directions".into(), ok: true, reason: String::new() });
    let t = turn(&cmds);
    let one = Item { named: None, id: "py".into(), label: label.into(), digest: Digest("p".into()) };
    let (events, _) = deliver(&mut run, &t, "one", vec![one]);
    let q = events.iter().find_map(|e| if let Event::QuestionOpened { question } = e { Some(question.clone()) } else { None }).expect("asked");
    (run, q)
}

// Alan, 21/11: "entender onde o python entra antes de decidir"
#[test]
fn asked_one_option_with_words_is_not_chosen_yet() {
    let (mut run, q) = asked_one("Python + FastAPI");
    let words = "entender onde o python entra antes de decidir";
    let change = || Verdict::Change { text: words.into() };
    let said = effects_of(&run, &q, &[("py", change())]);
    assert!(said.iter().all(|s| !s.contains("is chosen")), "{said:?}");
    assert!(said.contains(&format!("“Python + FastAPI” is not chosen yet: your words go to the agent at “directions”: “{words}”.")), "{said:?}");
    assert!(said.contains(&"The agent answers or changes it as you wrote, and asks you again.".to_string()), "{said:?}");
    // before it is sent it reads the same: nothing is being chosen
    assert!(said.iter().all(|s| &jaynes_core::decide::before_sending(s) == s && !s.starts_with("Choosing")), "{said:?}");
    let (events, cmds) = feed(&mut run, answer(&q, &[("py", change())]));
    let recorded = events.iter().find_map(|e| if let Event::AnswerRecorded { effects, .. } = e { Some(effects.sentences.clone()) } else { None }).expect("recorded");
    assert_eq!(recorded, said);
    // the agent is asked, not ordered to change it
    let t = turn(&cmds);
    assert_eq!(t.keep, None, "the agent works this round");
    let notes = t.notes.join("\n");
    assert!(notes.contains(&format!("“Python + FastAPI” is not chosen yet. The person wrote: {words}. Change it only as they ask. In your reply, say what you changed and why, and answer there any question they asked, never inside the option. Give it again with its name: they decide then.")), "{notes}");
    assert!(!notes.contains("was chosen"), "{notes}");
    // nothing is held: no later round is bound to it
    assert!(run.steps[0].locks.is_empty(), "{:?}", run.steps[0].locks);
    // the agent gives it again and the stop asks again
    let (events, _) = deliver(&mut run, &t, "one2", vec![Item { named: None, id: "py".into(), label: "Python + FastAPI".into(), digest: Digest("p2".into()) }]);
    let q2 = events.iter().find_map(|e| if let Event::QuestionOpened { question } = e { Some(question.clone()) } else { None }).expect("asked again");
    assert!(q2.choose);
    assert_eq!(q2.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["py"]);
    // going with it then is the choice, as before
    let said = effects_of(&run, &q2, &[("py", Verdict::Keep)]);
    assert!(said.contains(&"“Python + FastAPI” is chosen: no later round of directions may change it.".to_string()), "{said:?}");
}

// The design-lead, at the one-option stop: its title asks "Go with “Python + FastAPI”?", by the name before the colon,
// and what an answer does named the whole label, "“Python + FastAPI: small and known” is not chosen yet…".
#[test]
fn asked_one_option_s_sentences_name_it_as_its_title_does() {
    let (run, q) = asked_one("Python + FastAPI: small and known");
    assert_eq!(q.items[0].name(), "Python + FastAPI");
    let words = "where does python come in?";
    let said = effects_of(&run, &q, &[("py", Verdict::Change { text: words.into() })]);
    assert!(said.contains(&format!("“Python + FastAPI” is not chosen yet: your words go to the agent at “directions”: “{words}”.")), "{said:?}");
    let said = effects_of(&run, &q, &[("py", Verdict::Change { text: String::new() })]);
    assert_eq!(said[0], "“Python + FastAPI” is not chosen yet: write what to ask or change.");
    let said = effects_of(&run, &q, &[("py", Verdict::Keep)]);
    assert!(said.contains(&"“Python + FastAPI” is chosen: no later round of directions may change it.".to_string()), "{said:?}");
    assert!(said.iter().all(|s| !s.contains("small and known")), "{said:?}");
    // a label with no colon is its own name
    assert_eq!(Item { named: None, id: "a".into(), label: " Go ".into(), digest: Digest("d".into()) }.name(), "Go");
}

#[test]
fn a_change_without_words_says_what_to_write_in_its_field() {
    let empty = || Verdict::Change { text: String::new() };
    // one option: nothing is chosen yet
    let (run, q) = asked_one("Python + FastAPI");
    let said = effects_of(&run, &q, &[("py", empty())]);
    assert_eq!(said[0], "“Python + FastAPI” is not chosen yet: write what to ask or change.");
    // among several: the one is chosen and adjusted
    let (run, q) = asked();
    let said = effects_of(&run, &q, &[("Sun", empty()), ("Ink", Verdict::Drop), ("Moss", Verdict::Drop)]);
    assert!(said.contains(&"“Sun” is chosen and adjusted: say how in its field.".to_string()), "{said:?}");
    // with words, two or more read as before
    let said = effects_of(&run, &q, &[("Sun", Verdict::Change { text: "warmer".into() }), ("Ink", Verdict::Drop), ("Moss", Verdict::Drop)]);
    assert!(said.contains(&"“Sun” is chosen, and is adjusted as you wrote: “warmer”.".to_string()), "{said:?}");
    assert!(said.contains(&"The agent at directions adjusts what you chose, seeing every option it made, and you see the result.".to_string()), "{said:?}");
    // out of a choice: it is changed
    let mut plain = q.clone();
    plain.choose = false;
    let said = effects_of(&run, &plain, &[("Sun", empty())]);
    assert!(said.contains(&"“Sun” is changed: say how in its field.".to_string()), "{said:?}");
}
