//! What the person's proof grants when the record is read again, and what it never grants: a run's start that brought
//! keys of its own is folded with the keys the record paired; a proof stands for the answer it was over, once, and
//! only for an answer of the person's.

use jaynes_core::digest::Digest;
use jaynes_core::event::{Event, Usage};
use jaynes_core::io::StartPerson;
use jaynes_core::manifest::{Effect, Executor, Manifest, Rate, Reach, SessionKind, StepDecl, StopDecl};
use jaynes_core::proof::{self, KeyKind, PersonKey, Proof};
use jaynes_core::question::{Answer, By, Verdict, VerdictKind};
use jaynes_core::risk::Risk;
use jaynes_core::run::UNBOX;
use jaynes_core::{decide, AttemptOutcome, Command, Input, Run};
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};

fn device(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32].into()).expect("key")
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn key(seed: u8) -> PersonKey {
    PersonKey { kind: KeyKind::Presence, public: hex(device(seed).verifying_key().to_encoded_point(false).as_bytes()) }
}

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

fn stop(release: bool) -> StopDecl {
    StopDecl {
        verdicts: vec![VerdictKind::Keep, VerdictKind::Redo],
        holds: true,
        from_risk: Risk::Low,
        release,
        budget: None,
        audit_rate: Rate(0.1),
        about: "Is it right?".into(),
        choose: false,
        redo: None,
        rounds: None,
    }
}

fn feed(run: &mut Run, events: &mut Vec<Event>, input: Input) -> Result<Vec<Command>, String> {
    let out = decide(Some(run), &input).map_err(|r| r.to_string())?;
    for e in &out.events {
        run.apply(e);
    }
    events.extend(out.events);
    Ok(out.commands)
}

fn proved_with(run: &Run, input: Input, signed_nonce: &str, sent_nonce: &str, seed: u8) -> Input {
    let m = proof::input_message(&run.id, &run.started, signed_nonce, &input);
    let s: Signature = device(seed).sign(&m);
    Input::Proved { signed: Box::new(input), nonce: sent_nonce.into(), proof: Proof::Device { key: KeyKind::Presence, signature: hex(s.to_der().as_bytes()) } }
}

fn proved(run: &Run, input: Input, nonce: &str) -> Input {
    proved_with(run, input, nonce, nonce, 6)
}

fn answer(run: &Run, id: &str, item: Option<&str>, verdict: Verdict) -> Answer {
    let q = &run.questions[id].question;
    let verdicts = match item {
        Some(i) => [(i.to_string(), verdict)].into_iter().collect(),
        None => q.items.iter().map(|x| (x.id.clone(), verdict.clone())).collect(),
    };
    Answer { back: Default::default(), question: id.into(), shown: q.shown(), verdicts, by: By::Person, context: String::new() }
}

/// A run asking to leave its box, started with `keys`, and its record so far.
fn unboxed(keys: Vec<PersonKey>) -> (Run, Vec<Event>) {
    let manifest = Manifest { role: "r".into(), version: "1".into(), protocol: 1, steps: vec![step("runner", Executor::Role, &[])], target: Default::default(), delivery: Default::default() };
    let out = decide(None, &Input::Start {
        run: "r".into(), request: "x".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None,
        attention: Default::default(), task: None, arm: None, person: Some(StartPerson { keys, nonce: "s".into(), unboxed: true }),
    }).expect("start");
    (Run::fold(&out.events).expect("fold"), out.events)
}

#[test]
fn a_start_that_brought_keys_the_record_did_not_pair_is_folded_with_the_paired_ones() {
    // the record paired key 6; a start written by anything else brought key 9 and a proof by it
    let (mut run, mut events) = unboxed(vec![key(9)]);
    let keep = answer(&run, UNBOX, Some("box"), Verdict::Keep);
    let input = proved_with(&run, Input::Answer { answer: keep }, "n", "n", 9);
    feed(&mut run, &mut events, input).expect("taken, by the keys the start brought");
    assert!(run.unboxed(), "folded as it started, the box opens");
    let folded = Run::fold_paired(&events, &[key(6)]).expect("fold");
    assert!(folded.keys_differ && !folded.unboxed(), "folded with the keys the record paired, it does not");
    // a start that brought no key, where the record paired one, takes the person's proof all the same
    let mut make = step("make", Executor::Agent, &[]);
    make.stop = Some(stop(false));
    let mut send = step("send", Executor::Role, &["make"]);
    send.effect = Effect::Irreversible { idempotent: false };
    let manifest = Manifest { role: "r".into(), version: "1".into(), protocol: 1, steps: vec![make, send], target: Default::default(), delivery: Default::default() };
    let out = decide(None, &Input::Start { run: "q".into(), request: "x".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, person: None }).expect("start");
    let folded = Run::fold_paired(&out.events, &[key(6)]).expect("fold");
    assert!(folded.keys_differ && folded.person_keys == vec![key(6)]);
    assert!(!Run::fold_paired(&out.events, &[]).expect("fold").keys_differ, "no key paired: as it started");
}

#[test]
fn a_proof_stands_for_the_answer_it_was_over_and_not_for_one_written_after_it() {
    let (mut run, mut events) = unboxed(vec![key(6)]);
    let drop = answer(&run, UNBOX, Some("box"), Verdict::Drop);
    let input = proved(&run, Input::Answer { answer: drop }, "n");
    feed(&mut run, &mut events, input).expect("kept in its box, with the proof");
    assert!(run.proved_by.contains_key(UNBOX));
    // a keep written into the record after it, with no proof of its own
    let mut keep = answer(&run, UNBOX, Some("box"), Verdict::Keep);
    keep.shown = run.questions[UNBOX].question.shown();
    events.push(Event::AnswerRecorded { answer: keep, effects: Default::default() });
    let folded = Run::fold(&events).expect("fold");
    assert!(!folded.unboxed(), "a later answer without its proof opened the box");
    assert!(!folded.proved_by.contains_key(UNBOX));
}

#[test]
fn a_nonce_proves_once_when_the_record_is_read_again() {
    // two runs' worth of questions in one: a and b each wait for the person; a's answer is proved with nonce n, and
    // an answer to b with the same nonce, its signature right, is written in after it
    let mut a = step("a", Executor::Agent, &[]);
    a.stop = Some(stop(false));
    let mut b = step("b", Executor::Agent, &[]);
    b.stop = Some(stop(false));
    let manifest = Manifest { role: "r".into(), version: "1".into(), protocol: 1, steps: vec![a, b], target: Default::default(), delivery: Default::default() };
    let out = decide(None, &Input::Start { run: "r".into(), request: "x".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, person: Some(StartPerson { keys: vec![key(6)], nonce: "s".into(), unboxed: false }) }).expect("start");
    let mut events = out.events;
    let mut run = Run::fold(&events).expect("fold");
    let mut pending = out.commands;
    while let Some(c) = pending.pop() {
        let next = match c {
            Command::Probe { step, .. } => feed(&mut run, &mut events, Input::ProbeEnded { step, ok: true, reason: String::new() }),
            Command::RunAgent(x) => feed(&mut run, &mut events, Input::AttemptEnded {
                step: x.step.clone(), attempt: x.attempt, fix: 0,
                outcome: AttemptOutcome::Delivered { output: Digest(format!("o-{}", x.step)), items: Vec::new(), changed: Vec::new(), usage: Usage::default(), session: None },
            }),
            _ => Ok(Vec::new()),
        };
        pending.extend(next.expect("goes on"));
    }
    let qa = run.questions.values().find(|q| q.question.step == "a" && q.open()).expect("a asks").question.id.clone();
    let qb = run.questions.values().find(|q| q.question.step == "b" && q.open()).expect("b asks").question.id.clone();
    let ka = answer(&run, &qa, None, Verdict::Keep);
    let input = proved(&run, Input::Answer { answer: ka }, "n");
    feed(&mut run, &mut events, input).expect("a, proved");
    let kb = answer(&run, &qb, None, Verdict::Keep);
    // the core refuses the nonce again
    let again = proved(&run, Input::Answer { answer: kb.clone() }, "n");
    assert!(decide(Some(&run), &again).is_err_and(|r| r.to_string().contains("used already")));
    // written into the record, it grants nothing when the record is read again
    let Input::Proved { proof, .. } = again else { panic!() };
    events.push(Event::Proved { nonce: "n".into(), key: KeyKind::Presence, input: Box::new(Input::Answer { answer: kb.clone() }), proof });
    events.push(Event::AnswerRecorded { answer: kb, effects: Default::default() });
    let folded = Run::fold(&events).expect("fold");
    assert!(folded.proved_by.contains_key(&qa) && !folded.proved_by.contains_key(&qb), "{:?}", folded.proved_by);
}

#[test]
fn a_proof_holds_only_for_the_nonce_it_was_signed_with() {
    let (mut run, mut events) = unboxed(vec![key(6)]);
    let keep = answer(&run, UNBOX, Some("box"), Verdict::Keep);
    // signed with nonce a, sent with nonce b
    let input = proved_with(&run, Input::Answer { answer: keep.clone() }, "a", "b", 6);
    assert!(feed(&mut run, &mut events, input.clone()).is_err());
    // written into the record as it came, it grants nothing
    let Input::Proved { proof, .. } = input else { panic!() };
    let mut written = events.clone();
    written.push(Event::Proved { nonce: "b".into(), key: KeyKind::Presence, input: Box::new(Input::Answer { answer: keep.clone() }), proof });
    written.push(Event::AnswerRecorded { answer: keep.clone(), effects: Default::default() });
    assert!(!Run::fold(&written).expect("fold").unboxed());
    // an empty nonce is refused, signed or not
    let input = proved(&run, Input::Answer { answer: keep }, "");
    assert!(feed(&mut run, &mut events, input).is_err());
}

#[test]
fn a_proof_stands_only_for_an_answer_of_the_person_s() {
    // a stop an agent may release: an agent's answer is taken there, but never with the person's proof
    let mut make = step("make", Executor::Agent, &[]);
    make.stop = Some(stop(true));
    let manifest = Manifest { role: "r".into(), version: "1".into(), protocol: 1, steps: vec![make], target: Default::default(), delivery: Default::default() };
    let out = decide(None, &Input::Start { run: "r".into(), request: "x".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, person: Some(StartPerson { keys: vec![key(6)], nonce: "s".into(), unboxed: false }) }).expect("start");
    let mut events = out.events;
    let mut run = Run::fold(&events).expect("fold");
    let mut pending = out.commands;
    while let Some(c) = pending.pop() {
        let next = match c {
            Command::Probe { step, .. } => feed(&mut run, &mut events, Input::ProbeEnded { step, ok: true, reason: String::new() }),
            Command::RunAgent(x) => feed(&mut run, &mut events, Input::AttemptEnded {
                step: x.step.clone(), attempt: x.attempt, fix: 0,
                outcome: AttemptOutcome::Delivered { output: Digest("o".into()), items: Vec::new(), changed: Vec::new(), usage: Usage::default(), session: None },
            }),
            _ => Ok(Vec::new()),
        };
        pending.extend(next.expect("goes on"));
    }
    let q = run.questions.values().find(|q| q.open()).expect("make asks").question.id.clone();
    let mut agents = answer(&run, &q, None, Verdict::Keep);
    agents.by = By::Agent { who: "claude".into() };
    assert!(decide(Some(&run), &Input::Answer { answer: agents.clone() }).is_ok(), "an agent releases this stop on its own");
    let input = proved(&run, Input::Answer { answer: agents }, "n");
    assert!(decide(Some(&run), &input).is_err_and(|r| r.to_string().contains("only for an answer of yours")));
}
