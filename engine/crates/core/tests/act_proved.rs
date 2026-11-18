//! G14 with the person's device paired: letting go on an answer that a step that cannot be undone rests on takes the
//! device's proof, the person's word sending the work back still counts, an answer nothing that cannot be undone
//! rests on still takes their word, and with no device paired everything goes as before.

use jaynes_core::digest::Digest;
use jaynes_core::event::{Event, Usage};
use jaynes_core::io::StartPerson;
use jaynes_core::manifest::{Effect, Executor, Manifest, Rate, Reach, SessionKind, StepDecl, StopDecl};
use jaynes_core::proof::{self, KeyKind, PersonKey, Proof, Requires};
use jaynes_core::question::{Answer, By, Verdict, VerdictKind};
use jaynes_core::risk::Risk;
use jaynes_core::{decide, AttemptOutcome, Command, Input, Run};
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};

fn step(name: &str, executor: Executor, needs: &[&str]) -> StepDecl {
    StepDecl {
        name: name.to_string(),
        needs: needs.iter().map(|s| s.to_string()).collect(),
        executor,
        session: SessionKind::Fresh,
        reach: Reach::default(),
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

fn holding() -> StopDecl {
    StopDecl {
        verdicts: vec![VerdictKind::Keep, VerdictKind::Redo],
        holds: true,
        from_risk: Risk::Low,
        release: false,
        budget: None,
        audit_rate: Rate(0.1),
        about: "Is it right?".into(),
        choose: false,
        redo: None,
        rounds: None,
    }
}

fn device() -> SigningKey {
    SigningKey::from_bytes(&[6; 32].into()).expect("key")
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn feed(run: &mut Run, input: Input) -> Result<Vec<Command>, String> {
    let out = decide(Some(run), &input).map_err(|r| r.to_string())?;
    for e in &out.events {
        run.apply(e);
    }
    Ok(out.commands)
}

/// A run where `make` (an agent's, with a stop that holds) feeds `send`, which cannot be undone, and `note` (an
/// agent's, with a stop) feeds nothing that cannot be undone; both have delivered and wait for the person.
fn run(paired: bool) -> Run {
    let mut make = step("make", Executor::Agent, &[]);
    make.stop = Some(holding());
    let mut note = step("note", Executor::Agent, &[]);
    note.stop = Some(holding());
    let mut send = step("send", Executor::Role, &["make"]);
    send.effect = Effect::Irreversible { idempotent: false };
    let manifest = Manifest { role: "r".into(), version: "1".into(), protocol: 1, steps: vec![make, note, send], target: Default::default(), delivery: Default::default() };
    let person = paired.then(|| StartPerson {
        keys: vec![PersonKey { kind: KeyKind::Presence, public: hex(device().verifying_key().to_encoded_point(false).as_bytes()) }],
        nonce: "s".into(),
        unboxed: false,
    });
    let out = decide(None, &Input::Start { run: "r".into(), request: "x".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, person }).expect("start");
    let mut run = Run::fold(&out.events).expect("fold");
    let mut pending = out.commands;
    while let Some(c) = pending.pop() {
        let next = match c {
            Command::Probe { step, .. } => feed(&mut run, Input::ProbeEnded { step, ok: true, reason: String::new() }),
            Command::RunAgent(a) => feed(&mut run, Input::AttemptEnded {
                step: a.step.clone(),
                attempt: a.attempt,
                fix: 0,
                outcome: AttemptOutcome::Delivered { output: Digest(format!("o-{}", a.step)), items: Vec::new(), changed: Vec::new(), usage: Usage::default(), session: None },
            }),
            _ => Ok(Vec::new()),
        };
        pending.extend(next.expect("goes on"));
    }
    run
}

fn question(run: &Run, step: &str) -> String {
    run.questions.values().find(|q| q.question.step == step && q.open()).unwrap_or_else(|| panic!("{step} asks: {:?}", run.questions)).question.id.clone()
}

fn answer(run: &Run, id: &str, verdict: Verdict) -> Answer {
    let q = &run.questions[id].question;
    Answer { back: Default::default(), question: id.into(), shown: q.shown(), verdicts: q.items.iter().map(|x| (x.id.clone(), verdict.clone())).collect(), by: By::Person, context: String::new() }
}

fn proved(run: &Run, input: Input, nonce: &str) -> Input {
    let m = proof::input_message(&run.id, &run.started, nonce, &input);
    let s: Signature = device().sign(&m);
    Input::Proved { signed: Box::new(input), nonce: nonce.into(), proof: Proof::Device { key: KeyKind::Presence, signature: hex(s.to_der().as_bytes()) } }
}

fn sends(cmds: &[Command]) -> bool {
    cmds.iter().any(|c| matches!(c, Command::RunRole { step, .. } | Command::Probe { step, .. } if step == "send"))
}

#[test]
fn letting_go_on_what_a_step_that_cannot_be_undone_rests_on_takes_the_device_s_proof() {
    let mut run = run(true);
    let make = question(&run, "make");
    let note = question(&run, "note");
    assert_eq!(run.requires.get(&make), Some(&Requires::Presence));
    assert_eq!(run.requires.get(&note), None, "nothing that cannot be undone rests on note");
    // the person's word lets nothing go on here
    let keep = answer(&run, &make, Verdict::Keep);
    assert!(feed(&mut run, Input::Answer { answer: keep.clone() }).is_err_and(|e| e.contains("Jaynes app")));
    // their word on note still counts
    let a = answer(&run, &note, Verdict::Keep);
    feed(&mut run, Input::Answer { answer: a }).expect("note on the person's word");
    // with the device's proof, the step that cannot be undone goes on
    let input = proved(&run, Input::Answer { answer: keep }, "n1");
    let cmds = feed(&mut run, input).expect("taken with the proof");
    assert!(sends(&cmds), "{cmds:?}");
}

#[test]
fn sending_the_work_back_still_takes_the_person_s_word() {
    let mut run = run(true);
    let make = question(&run, "make");
    let redo = answer(&run, &make, Verdict::Redo { note: "again".into() });
    feed(&mut run, Input::Answer { answer: redo }).expect("sent back on the person's word");
}

#[test]
fn with_no_device_paired_the_person_s_word_lets_it_go_on_as_before() {
    let mut run = run(false);
    let make = question(&run, "make");
    assert!(run.requires.is_empty());
    let keep = answer(&run, &make, Verdict::Keep);
    let cmds = feed(&mut run, Input::Answer { answer: keep }).expect("taken on the word");
    assert!(sends(&cmds), "{cmds:?}");
}

#[test]
fn an_answer_in_the_record_without_its_proof_lets_nothing_that_cannot_be_undone_run() {
    // the record says the person kept make on their word, with a device paired: something wrote it there that is
    // not this core, and the step that cannot be undone keeps waiting, saying why
    let mut run = run(true);
    let make = question(&run, "make");
    let keep = answer(&run, &make, Verdict::Keep);
    run.apply(&Event::AnswerRecorded { answer: keep, effects: Default::default() });
    let out = decide(Some(&run), &Input::Tick).expect("tick");
    assert!(!sends(&out.commands), "{:?}", out.commands);
    assert!(out.events.iter().chain(std::iter::empty()).all(|e| !matches!(e, Event::AttemptStarted { step, .. } if step == "send")));
    let waits = run.unconfirmed(run.manifest.index("send").expect("send"));
    assert_eq!(waits, vec![make]);
}
