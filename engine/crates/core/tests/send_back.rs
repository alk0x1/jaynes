//! A run blocked because the agent spent its attempts goes back to the agent with the person's words: the attempts
//! come back and the agent's next attempt reads what the person wrote, beside what failed. Only the person sends a
//! run back so, only with words, and only where the agent works again (the real-user, 22/11: "o Continue devolve as
//! tentativas e bate no mesmo caso de novo").

use jaynes_core::digest::Digest;
use jaynes_core::event::{Event, Fault, Usage};
use jaynes_core::manifest::{CheckDecl, CheckKind, Effect, Executor, Manifest, ProjectAccess, Reach, SessionKind, StepDecl};
use jaynes_core::question::By;
use jaynes_core::risk::Risk;
use jaynes_core::run::Phase;
use jaynes_core::{decide, AgentTurn, AttemptOutcome, CheckResult, CheckVerdict, Command, Input, Run};
use std::collections::BTreeMap;

/// One agent step judged by a hidden check, with three attempts.
fn manifest() -> Manifest {
    let fix = StepDecl {
        name: "fix".to_string(),
        needs: Vec::new(),
        executor: Executor::Agent,
        session: SessionKind::Fresh,
        reach: Reach { project: ProjectAccess::Write, write: vec!["**".into()], ..Reach::default() },
        effect: Effect::None,
        checks: vec![CheckDecl { name: "proof".into(), kind: CheckKind::Hidden, about: String::new(), reads: Vec::new(), project_tests: false }],
        stop: None,
        from_risk: Risk::Low,
        attempts: 3,
        fixes: 0,
        hidden: false,
        idempotent: false,
        model: Default::default(),
        turns: None, delivered: true,
        about: String::new(),
    };
    Manifest { role: "fix-bug".into(), version: "1".into(), protocol: 1, steps: vec![fix], target: Default::default(), delivery: Default::default() }
}

fn feed(run: &mut Run, input: Input) -> (Vec<Event>, Vec<Command>) {
    let out = decide(Some(run), &input).unwrap_or_else(|r| panic!("refused {input:?}: {r}"));
    for e in &out.events {
        run.apply(e);
    }
    settle(run, out.events, out.commands)
}

fn refused(run: &Run, input: Input) -> String {
    match decide(Some(run), &input) {
        Ok(out) => panic!("not refused: {input:?} gave {:?}", out.events),
        Err(r) => r.to_string(),
    }
}

fn settle(run: &mut Run, mut events: Vec<Event>, mut cmds: Vec<Command>) -> (Vec<Event>, Vec<Command>) {
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

fn turn(cmds: &[Command]) -> Option<AgentTurn> {
    cmds.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.clone()) } else { None })
}

/// The turn delivers and the hidden case fails: an attempt spent.
fn fails(run: &mut Run, t: &AgentTurn) -> Vec<Command> {
    let output = Digest(format!("out-{}", t.attempt));
    let usage = Usage { seconds: 1, tokens: 10, cached: 0, uncounted: 0 };
    feed(run, Input::AttemptEnded { step: t.step.clone(), attempt: t.attempt, fix: 0, outcome: AttemptOutcome::Delivered { output, items: Vec::new(), changed: Vec::new(), usage, session: None, tree: None, reply: None } });
    let reasons = vec!["A check you cannot see found that this does not hold: greet says hi with the name".to_string()];
    let failed = CheckVerdict::Fail { reasons, detail: String::new(), cases: Vec::new() };
    feed(run, Input::ChecksEnded { step: t.step.clone(), attempt: t.attempt, fix: 0, results: vec![CheckResult { check: "proof".into(), verdict: failed, tree: None, tests_changed: Vec::new() }] }).1
}

/// A run blocked at fix: its three attempts spent at the hidden case.
fn blocked() -> Run {
    let out = decide(None, &Input::Start {
        run: "r".into(),
        request: "greet should say hi with the name".into(),
        manifest: manifest(),
        role_fingerprint: Digest("f".into()),
        agent: "claude:m1".into(),
        raise: None,
        attention: BTreeMap::new(),
        task: None,
        arm: None,
        never_merge: false,
        starter: Some("s1".into()),
    })
    .expect("start");
    let mut run = Run::fold(&out.events).expect("run");
    let (_, mut cmds) = settle(&mut run, Vec::new(), out.commands);
    while let Some(t) = turn(&cmds) {
        cmds = fails(&mut run, &t);
    }
    assert!(matches!(run.phase, Phase::Blocked { fault: Fault::Agent, .. }), "{:?}", run.phase);
    run
}

const WORDS: &str = "The case needs git's history, which the copy has: run it with the old render.py first";

// The real-user, 22/11: "o Continue devolve as tentativas e bate no mesmo caso de novo"
#[test]
fn asked_a_run_blocked_at_a_hidden_case_goes_back_to_the_agent_with_the_person_s_words() {
    let mut run = blocked();
    let (events, cmds) = feed(&mut run, Input::SendBack { by: By::Person, words: WORDS.into() });
    // the record keeps who sent it back and with what words
    assert!(events.iter().any(|e| matches!(e, Event::Continued { by: By::Person, words: Some(w) } if w == WORDS)), "{events:?}");
    // the attempts come back, and the next attempt reads the words beside what failed
    let t = turn(&cmds).unwrap_or_else(|| panic!("no turn: {cmds:?}"));
    assert_eq!((t.step.as_str(), t.attempt), ("fix", 4));
    assert!(t.notes.iter().any(|n| n.contains(WORDS)), "{:?}", t.notes);
    assert_eq!(run.steps[0].spent, 0);
    // and it never learns the case beyond its sentence, as before
    assert!(t.failure.iter().all(|f| !f.contains("grep")), "{:?}", t.failure);
}

#[test]
fn only_the_person_sends_a_run_back_and_only_with_words() {
    let run = blocked();
    // the agent that started the run: sending it back is the person's
    let said = refused(&run, Input::SendBack { by: By::Agent { who: "claude".into() }, words: WORDS.into() });
    assert!(said.contains("only you"), "{said}");
    // no words: the same attempt again, which failed three times; the person says what the agent should know
    for empty in ["", "   \n "] {
        let said = refused(&run, Input::SendBack { by: By::Person, words: empty.into() });
        assert!(said.contains("words"), "{said}");
    }
}

#[test]
fn a_run_blocked_where_the_agent_does_not_work_again_is_not_sent_back() {
    let mut run = blocked();
    // not blocked at all
    feed(&mut run, Input::Continue { by: By::Person, starter: None });
    let said = refused(&run, Input::SendBack { by: By::Person, words: WORDS.into() });
    assert!(said.contains("not blocked"), "{said}");
    // blocked by Jaynes: Continue judges the same change again, and no agent would read the words
    let mut jaynes = blocked();
    jaynes.phase = Phase::Blocked { step: Some("fix".into()), fault: Fault::Jaynes, reason: "the box was not there".into() };
    let said = refused(&jaynes, Input::SendBack { by: By::Person, words: WORDS.into() });
    assert!(said.contains("agent"), "{said}");
}

// The promises explore the person's Continue and not SendBack apart: the two go the same way, and differ only in the
// words, in the record and in the notes the next attempt reads.
#[test]
fn sending_back_with_words_goes_as_continue_does_but_for_the_words() {
    let mut words = blocked();
    let (by_words, cmds_words) = feed(&mut words, Input::SendBack { by: By::Person, words: WORDS.into() });
    let mut plain = blocked();
    let (by_continue, cmds_plain) = feed(&mut plain, Input::Continue { by: By::Person, starter: None });
    let mut turn_words = turn(&cmds_words).expect("turn");
    assert!(turn_words.notes.iter().any(|n| n.contains(WORDS)));
    turn_words.notes.retain(|n| !n.contains(WORDS));
    assert_eq!(Some(turn_words), turn(&cmds_plain));
    let continued = |e: &Event| matches!(e, Event::Continued { .. });
    assert_eq!(by_words.iter().filter(|e| !continued(e)).collect::<Vec<_>>(), by_continue.iter().filter(|e| !continued(e)).collect::<Vec<_>>());
    words.steps[0].notes.retain(|n| !n.contains(WORDS));
    assert_eq!(serde_json::to_value(&words).expect("json"), serde_json::to_value(&plain).expect("json"));
}
