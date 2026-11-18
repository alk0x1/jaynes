//! G18 and the proof of the person's device: a run leaves its box only when the person asked for it at the start and
//! approved it with a proof only they can give, checked by the core against the keys the run recorded, for this run
//! and once; acting again after an act asks the same proof; and a run with no device of the person's paired goes on
//! exactly as before.

use jaynes_core::digest::Digest;
use jaynes_core::event::Event;
use jaynes_core::io::StartPerson;
use jaynes_core::manifest::{Effect, Executor, Manifest, ProjectAccess, Reach, SessionKind, StepDecl};
use jaynes_core::proof::{self, KeyKind, PersonKey, Proof, Requires};
use jaynes_core::question::{Answer, By, QuestionKind, Verdict};
use jaynes_core::risk::Risk;
use jaynes_core::run::UNBOX;
use jaynes_core::{decide, Command, Input, Run};
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};

fn step(name: &str, executor: Executor) -> StepDecl {
    StepDecl {
        name: name.to_string(),
        needs: Vec::new(),
        executor,
        session: SessionKind::Fresh,
        reach: Reach { project: ProjectAccess::Write, write: vec!["**".into()], commands: true, ..Reach::default() },
        effect: Effect::None,
        checks: Vec::new(),
        stop: None,
        from_risk: Risk::Low,
        attempts: 3,
        fixes: 2,
        hidden: false,
        idempotent: false,
        about: String::new(),
    }
}

fn manifest(steps: Vec<StepDecl>) -> Manifest {
    Manifest { role: "r".into(), version: "1".into(), protocol: 1, steps, target: Default::default(), delivery: Default::default() }
}

fn device(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32].into()).expect("key")
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn keys() -> Vec<PersonKey> {
    vec![
        PersonKey { kind: KeyKind::Biometry, public: hex(device(5).verifying_key().to_encoded_point(false).as_bytes()) },
        PersonKey { kind: KeyKind::Presence, public: hex(device(6).verifying_key().to_encoded_point(false).as_bytes()) },
    ]
}

fn start(steps: Vec<StepDecl>, person: Option<StartPerson>) -> Result<(Run, Vec<Command>), String> {
    let out = decide(None, &Input::Start {
        run: "r".into(),
        request: "x".into(),
        manifest: manifest(steps),
        role_fingerprint: Digest("f".into()),
        agent: "claude:m".into(),
        raise: None,
        attention: Default::default(),
        task: None, arm: None, person,
    })
    .map_err(|r| r.to_string())?;
    Ok((Run::fold(&out.events).expect("fold"), out.commands))
}

fn person(unboxed: bool, nonce: &str) -> Option<StartPerson> {
    Some(StartPerson { keys: keys(), nonce: nonce.into(), unboxed })
}

fn runs_on_project(c: &Command) -> bool {
    matches!(c, Command::RunAgent(_) | Command::RunRole { .. } | Command::Probe { .. } | Command::RunChecks { .. })
}

fn feed(run: &mut Run, input: Input) -> Result<(Vec<Event>, Vec<Command>), String> {
    let out = decide(Some(run), &input).map_err(|r| r.to_string())?;
    for e in &out.events {
        run.apply(e);
    }
    Ok((out.events, out.commands))
}

fn answer(run: &Run, id: &str, item: &str, verdict: Verdict) -> Answer {
    let q = &run.questions[id].question;
    Answer { back: Default::default(), question: id.into(), shown: q.shown(), verdicts: [(item.to_string(), verdict)].into_iter().collect(), by: By::Person, context: String::new() }
}

/// The input, with a proof by the person's key of `kind`, over this run and `nonce`.
fn proved(run: &Run, input: Input, nonce: &str, kind: KeyKind, with: &SigningKey) -> Input {
    let message = proof::input_message(&run.id, &run.started, nonce, &input);
    let s: Signature = with.sign(&message);
    Input::Proved { signed: Box::new(input), nonce: nonce.into(), proof: Proof::Device { key: kind, signature: hex(s.to_der().as_bytes()) } }
}

#[test]
fn a_run_with_no_device_paired_goes_on_as_before_and_is_written_as_before() {
    let (run, cmds) = start(vec![step("change", Executor::Agent)], None).expect("start");
    assert!(!run.unboxed() && !run.questions.contains_key(UNBOX));
    assert!(cmds.iter().any(runs_on_project), "{cmds:?}");
    let out = decide(None, &Input::Start {
        run: "r".into(), request: "x".into(), manifest: manifest(vec![step("change", Executor::Agent)]), role_fingerprint: Digest("f".into()),
        agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, person: None,
    }).expect("start");
    let first = serde_json::to_value(&out.events[0]).expect("json");
    for field in ["person_keys", "nonce", "unboxed"] {
        assert!(first.get(field).is_none(), "{field} written: {first}");
    }
}

#[test]
fn with_no_device_paired_a_run_outside_its_box_is_refused() {
    let none = Some(StartPerson { keys: Vec::new(), nonce: "n".into(), unboxed: true });
    let refused = start(vec![step("change", Executor::Agent)], none).expect_err("refused");
    assert!(refused.contains("never runs outside its box"), "{refused}");
}

#[test]
fn a_run_that_asks_to_leave_its_box_waits_for_the_person_before_any_command_on_the_project() {
    let (run, cmds) = start(vec![step("runner", Executor::Role), step("change", Executor::Agent)], person(true, "s1")).expect("start");
    assert!(!cmds.iter().any(runs_on_project), "{cmds:?}");
    assert_eq!(run.questions[UNBOX].question.kind, QuestionKind::Unbox);
    assert!(run.questions[UNBOX].open() && !run.unboxed());
    assert_eq!(jaynes_core::decide::requires_to_release(&run, UNBOX), Requires::Presence);
}

#[test]
fn nothing_but_a_proof_of_the_person_s_device_for_this_run_opens_the_box_and_only_once() {
    let (mut run, _) = start(vec![step("runner", Executor::Role), step("change", Executor::Agent)], person(true, "s1")).expect("start");
    let keep = answer(&run, UNBOX, "box", Verdict::Keep);
    // the person's word, from the page or the terminal
    assert!(feed(&mut run, Input::Answer { answer: keep.clone() }).is_err_and(|e| e.contains("Jaynes app")));
    // a key the run does not know, said to be the device's
    assert!({ let i = proved(&run, Input::Answer { answer: keep.clone() }, "a", KeyKind::Presence, &device(9)); feed(&mut run, i) }.is_err());
    // a proof made for another run with the same id: it started otherwise
    let (other, _) = start(vec![step("runner", Executor::Role)], person(true, "s2")).expect("other");
    assert_ne!(other.started, run.started, "two starts of the same id are two runs");
    assert!(feed(&mut run, proved(&other, Input::Answer { answer: keep.clone() }, "b", KeyKind::Presence, &device(6))).is_err());
    // a proof over another answer
    let mut drop = keep.clone();
    drop.verdicts.insert("box".into(), Verdict::Drop);
    let over_drop = proved(&run, Input::Answer { answer: drop }, "c", KeyKind::Presence, &device(6));
    let Input::Proved { proof, .. } = over_drop else { panic!() };
    assert!(feed(&mut run, Input::Proved { signed: Box::new(Input::Answer { answer: keep.clone() }), nonce: "c".into(), proof }).is_err());
    // an agent's answer with the person's proof
    let mut agents = keep.clone();
    agents.by = By::Agent { who: "a".into() };
    assert!({ let i = proved(&run, Input::Answer { answer: agents }, "d", KeyKind::Presence, &device(6)); feed(&mut run, i) }.is_err());
    assert!(!run.unboxed() && run.questions[UNBOX].open());
    // the person's own, by their device key
    let (events, cmds) = { let i = proved(&run, Input::Answer { answer: keep.clone() }, "e", KeyKind::Presence, &device(6)); feed(&mut run, i) }.expect("taken");
    assert!(matches!(&events[0], Event::Proved { key: KeyKind::Presence, .. }), "{events:?}");
    assert!(run.unboxed());
    assert_eq!(run.proved_by.get(UNBOX), Some(&KeyKind::Presence));
    assert!(cmds.iter().any(runs_on_project), "the run goes on: {cmds:?}");
    // the same proof again: a nonce is used once
    assert!({ let i = proved(&run, Input::Answer { answer: keep.clone() }, "e", KeyKind::Presence, &device(6)); feed(&mut run, i) }.is_err_and(|e| e.contains("used already")));
    // reading the record again checks the proof again
    let again = Run::fold(&replay(&keep)).expect("fold");
    assert!(again.unboxed());
}

/// The record of a run that asked to leave its box and was let out by the person's device key.
fn replay(keep: &Answer) -> Vec<Event> {
    let out = decide(None, &Input::Start {
        run: "r".into(), request: "x".into(), manifest: manifest(vec![step("runner", Executor::Role), step("change", Executor::Agent)]),
        role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, person: person(true, "s1"),
    }).expect("start");
    let mut events = out.events;
    let r = Run::fold(&events).expect("fold");
    let more = decide(Some(&r), &proved(&r, Input::Answer { answer: keep.clone() }, "e", KeyKind::Presence, &device(6))).expect("taken");
    events.extend(more.events);
    events
}

#[test]
fn a_proof_written_into_the_record_by_anything_else_grants_nothing() {
    let (run, _) = start(vec![step("runner", Executor::Role)], person(true, "s1")).expect("start");
    let keep = answer(&run, UNBOX, "box", Verdict::Keep);
    let mut events = replay(&keep);
    // the proof's signature replaced, its event and the answer kept: what a hand on the record could write
    for e in events.iter_mut() {
        if let Event::Proved { proof: Proof::Device { signature, .. }, .. } = e {
            let forged: Signature = device(9).sign(b"anything");
            *signature = hex(forged.to_der().as_bytes());
        }
    }
    let folded = Run::fold(&events).expect("fold");
    assert!(!folded.unboxed(), "a forged proof in the record opened the box");
}

#[test]
fn keeping_the_box_takes_the_person_s_word_and_the_run_goes_on_in_it() {
    let (mut run, _) = start(vec![step("runner", Executor::Role), step("change", Executor::Agent)], person(true, "s1")).expect("start");
    let (_, cmds) = { let a = answer(&run, UNBOX, "box", Verdict::Drop); feed(&mut run, Input::Answer { answer: a }) }.expect("taken");
    assert!(!run.unboxed());
    assert!(cmds.iter().any(runs_on_project), "{cmds:?}");
}

#[test]
fn acting_again_takes_the_device_s_proof_when_a_device_is_paired_and_the_word_when_none_is() {
    let mut send = step("send", Executor::Role);
    send.effect = Effect::Irreversible { idempotent: false };
    for paired in [true, false] {
        let (mut run, cmds) = start(vec![send.clone()], if paired { person(false, "s1") } else { None }).expect("start");
        for c in cmds {
            if let Command::Probe { step, .. } = c {
                feed(&mut run, Input::ProbeEnded { step, ok: true, reason: String::new() }).expect("probed");
            }
        }
        let attempt = run.steps[0].attempt;
        feed(&mut run, Input::Recovered).expect("recovered");
        let _ = attempt;
        let (_, cmds) = feed(&mut run, Input::Continue { by: By::Person }).expect("continued");
        let q = cmds.iter().find_map(|c| if let Command::Ask { question } = c { Some(question.clone()) } else { None }).expect("asks about the act");
        assert_eq!(q.kind, QuestionKind::Acted);
        let again = answer(&run, &q.id, "send", Verdict::Redo { note: "go".into() });
        if paired {
            assert_eq!(jaynes_core::decide::requires_to_release(&run, &q.id), Requires::Presence);
            assert!(feed(&mut run, Input::Answer { answer: again.clone() }).is_err_and(|e| e.contains("Jaynes app")));
            { let i = proved(&run, Input::Answer { answer: again }, "x", KeyKind::Biometry, &device(5)); feed(&mut run, i) }.expect("taken with the fingerprint key");
        } else {
            assert_eq!(jaynes_core::decide::requires_to_release(&run, &q.id), Requires::Word, "nothing changes with no device paired");
            feed(&mut run, Input::Answer { answer: again }).expect("taken on the person's word");
        }
    }
}
