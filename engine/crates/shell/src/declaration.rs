//! What a finished run did, in a format for machines: the run's summary as fields, for an auditor's tools and for
//! whoever checks it again. It says what the record holds and nothing more: what an auditor asks for and the record
//! does not hold is listed as absent, each with what will fill it. It is not signed, and does not call itself an
//! attestation.
//!
//! [`declare`] is a function of the record (the run's entries with their places in the chain, the person's presence,
//! the page's refusals, the proofs of the person's device) and of the run's role, whose promises are proved again.
//! [`differences`] makes it again from the record and names every field a file says otherwise: `jaynes verify-summary`.
//!
//! It holds the person's words: the request, what they wrote at the stops, their notes on a correction or an undo.
//! The file is theirs to send; it says so at its top, and without words (`--without-words`) it leaves them out and
//! says that it did.

use crate::store::Store;
use jaynes_core::event::{Event, Outcome};
use jaynes_core::question::{By, Verdict};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

pub const FORMAT: &str = "jaynes-run-summary/1";

/// The sentence a file without the person's words carries, and the one the command says when they are in.
pub const WORDS_LEFT_OUT: &str = "the person's words are left out: the request, what they wrote at the stops, and their notes on a correction or an undo";
pub const HOLDS_WORDS: &str = "this file holds the person's words (the request, what they wrote at the stops, their notes on a correction or an undo): it is theirs to send, or to make again with --without-words";

#[derive(Serialize)]
pub struct Declaration {
    pub format: &'static str,
    /// Whether the person's words are in the file.
    pub holds_words: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub words_left_out: Option<&'static str>,
    /// The program that made the file: compared by no check, since another version may check it again.
    pub made_by: MadeBy,
    pub record: Place,
    pub run: RunPart,
    pub request: Request,
    pub role: Role,
    pub agent: Agent,
    /// Each step of the role as the run held it: who does it, what it may reach, what checks it. The instructions are
    /// in the role, whose fingerprint is above.
    pub configuration: Vec<Value>,
    pub author: Author,
    pub delivery: Delivery,
    pub steps: Vec<Step>,
    pub approvals: Vec<Approval>,
    pub changed: Vec<Changed>,
    pub promises: Value,
    pub not_proved: Vec<String>,
    pub refused: Vec<String>,
    pub cost: Cost,
    pub people: People,
    pub absent: Vec<Absent>,
}

#[derive(Serialize)]
pub struct MadeBy {
    pub engine: String,
    pub version: String,
}

/// Where the run is in the record: its first and last entries, and the chain's hash after its last, which no entry
/// can change without changing.
#[derive(Serialize)]
pub struct Place {
    pub first: u64,
    pub last: u64,
    pub head: String,
}

#[derive(Serialize)]
pub struct RunPart {
    pub id: String,
    pub started: String,
    pub ended: String,
    pub outcome: &'static str,
    /// What became of it, as the export of numbers says it (`export::RESULTS`).
    pub result: &'static str,
}

#[derive(Serialize)]
pub struct Request {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub digest: String,
}

#[derive(Serialize)]
pub struct Role {
    pub name: String,
    pub version: String,
    pub fingerprint: String,
}

#[derive(Serialize)]
pub struct Agent {
    pub program: String,
    /// The model, for an agent that names one.
    pub model: Option<String>,
    /// A command run as the agent is named by a fingerprint: its line holds this machine's paths.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_digest: Option<String>,
}

#[derive(Serialize)]
pub struct Author {
    pub who: &'static str,
    /// Who that is in a directory: Jaynes knows none.
    pub identity: Option<String>,
}

#[derive(Serialize)]
pub struct Delivery {
    pub kind: &'static str,
    pub delivered: bool,
    /// What the delivery was, in the target's own words for it.
    pub reference: Option<String>,
    pub corrected: Vec<Note>,
    pub undone: Vec<Note>,
}

/// A correction or an undo: when, by whom, the step it names, and the person's note.
#[derive(Serialize)]
pub struct Note {
    pub at: String,
    pub by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Serialize)]
pub struct Step {
    pub name: String,
    pub by: &'static str,
    pub state: &'static str,
    pub attempts: u32,
    pub rounds: u32,
    /// The checks of its last judgment, each with how it went.
    pub checks: Vec<Check>,
}

#[derive(Serialize)]
pub struct Check {
    pub name: String,
    pub kind: Value,
    pub passed: bool,
    /// It could not judge: a fault of Jaynes or of the machine.
    pub blocked: bool,
}

/// An answer to a question of the run: who gave it, with what proof, to what they were shown, and whether it came
/// before the delivery.
#[derive(Serialize)]
pub struct Approval {
    pub question: String,
    pub step: String,
    pub kind: Value,
    pub at: String,
    pub by: String,
    /// "word": the person's word, from a channel only they hold; "device KEY": signed by a device of theirs, by the
    /// start of its key's fingerprint; "none": a rule's or an agent's.
    pub proof: String,
    /// The fingerprint of the question they were shown.
    pub shown: String,
    pub verdicts: BTreeMap<String, VerdictOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// What they brought back of what they dropped earlier, by id, with their words on how.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub brought_back: BTreeMap<String, Option<String>>,
    pub before_delivery: bool,
}

#[derive(Serialize)]
pub struct VerdictOut {
    pub verdict: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Serialize)]
pub struct Changed {
    pub what: String,
    pub size: u64,
    /// It runs or is obeyed later outside any box, or cannot be taken back.
    pub floor: bool,
}

#[derive(Serialize)]
pub struct Cost {
    pub agent_seconds: u64,
    pub tokens: u64,
    pub attempts: u32,
    pub spent: u32,
    pub fixes: u32,
}

#[derive(Serialize)]
pub struct People {
    pub stops: u32,
    pub sent_back: u32,
    pub acts: u32,
    pub presence_seconds: Option<u64>,
    pub unmeasured: u32,
    pub waited_seconds: u64,
}

/// What an auditor asks of a change and the record does not hold, with what will fill it.
#[derive(Serialize)]
pub struct Absent {
    pub what: &'static str,
    pub filled_by: &'static str,
}

/// What an auditor asks of a change and no run's record holds yet.
pub const ABSENT: [(&str, &str); 9] = [
    ("the approver's identity in a directory", "nothing planned yet: Jaynes knows a device's key, not a person's account"),
    ("that the approver is not the author", "whoever started the run counts as its author, and in a team who made a change does not approve it (planned with running without a person)"),
    ("a trusted time for each approval", "nothing planned yet: the times are this machine's clock"),
    ("the tree each check judged", "the checks judging the output's tree, which the record will name for each check"),
    ("the record anchored outside this machine", "the delivery's commit carrying the head of the record's chain"),
    ("a signature over this summary", "a command that exports the summary signed"),
    ("what reached the branch without a run", "jaynes changes: the history of the person's repository, each change matched with its run"),
    ("the deployment", "nothing planned yet: Jaynes does not deploy"),
    ("who could change the rules of approval", "nothing planned yet"),
];

/// A time of the record, in UTC, to the second.
fn iso(ts: u64) -> String {
    let day = crate::summary::utc(ts);
    format!("{}T{}:{:02}Z", &day[..10], &day[11..16], ts % 60)
}

fn who(by: &By) -> String {
    match by {
        By::Person => "person".into(),
        By::Rule { rule } => format!("rule {rule}"),
        By::Agent { who } => format!("agent {who}"),
    }
}

/// The declaration of a finished run of the record in `home`; without the person's words when `words` is false. The
/// role's promises are proved again before it answers.
pub fn declare(home: &Path, run: &str, words: bool) -> Result<Declaration, String> {
    let store = Store::open(&home.join("record.sqlite3"))?;
    let chained = store.chained(run)?;
    let (Some(first), Some(last)) = (chained.first(), chained.last()) else { return Err(format!("there is no run {run}")) };
    let events: Vec<(u64, Event)> = chained.iter().map(|(_, ts, _, e)| (*ts, e.clone())).collect();
    let looks = crate::presence::looks(home, run)?;
    let s = crate::summary::summarize(&events, &looks, crate::summary::Promises::Unread)?;
    let r = jaynes_core::Run::fold(events.iter().map(|(_, e)| e)).ok_or("the run has no entry in the record")?;
    let proofs: BTreeMap<String, String> = store.proof_keys(run)?.into_iter().collect();
    let corrected_any = events.iter().any(|(_, e)| matches!(e, Event::DeliveryCorrected { .. }));
    let result = if s.abandoned {
        "abandoned"
    } else if s.undone {
        "undone"
    } else if corrected_any {
        "corrected"
    } else {
        "accepted"
    };
    let delivered_at = chained.iter().find(|(_, _, _, e)| matches!(e, Event::Delivered { .. })).map(|(seq, ..)| *seq);
    let mut asked: BTreeMap<&str, &jaynes_core::question::Question> = BTreeMap::new();
    let (mut approvals, mut corrected, mut undone) = (Vec::new(), Vec::new(), Vec::new());
    let said = |text: &str| (words && !text.trim().is_empty()).then(|| text.to_string());
    for (seq, ts, _, e) in &chained {
        match e {
            Event::QuestionOpened { question } => {
                asked.insert(&question.id, question);
            }
            Event::AnswerRecorded { answer, .. } => {
                let q = asked.get(answer.question.as_str());
                let proof = match (&answer.by, proofs.get(&answer.question)) {
                    (By::Person, Some(key)) => format!("device {}", &jaynes_core::Digest::of_bytes(key.as_bytes()).0[..16]),
                    (By::Person, None) => "word".into(),
                    _ => "none".into(),
                };
                let verdicts = answer
                    .verdicts
                    .iter()
                    .map(|(item, v)| {
                        let (verdict, note) = match v {
                            Verdict::Keep => ("keep", None),
                            Verdict::Change { text } => ("change", said(text)),
                            Verdict::Redo { note } => ("redo", said(note)),
                            Verdict::Drop => ("drop", None),
                            Verdict::Skip => ("skip", None),
                        };
                        (item.clone(), VerdictOut { verdict, note })
                    })
                    .collect();
                approvals.push(Approval {
                    question: answer.question.clone(),
                    step: q.map(|q| q.step.clone()).unwrap_or_default(),
                    kind: q.map(|q| serde_json::to_value(q.kind).unwrap_or(Value::Null)).unwrap_or(Value::Null),
                    at: iso(*ts),
                    by: who(&answer.by),
                    proof,
                    shown: answer.shown.0.clone(),
                    verdicts,
                    context: said(&answer.context),
                    brought_back: answer.back.iter().map(|(id, how)| (id.clone(), said(how))).collect(),
                    before_delivery: delivered_at.is_none_or(|d| *seq < d),
                });
            }
            Event::DeliveryCorrected { note, by, step } => corrected.push(Note { at: iso(*ts), by: who(by), step: step.clone(), note: said(note) }),
            Event::MergeUndone { note, by } => undone.push(Note { at: iso(*ts), by: who(by), step: None, note: said(note) }),
            _ => {}
        }
    }
    let (program, model) = r.agent.split_once(':').map(|(p, m)| (p.to_string(), Some(m.to_string()))).unwrap_or((r.agent.clone(), None));
    let agent = if program == "command" {
        Agent { program, model: None, command_digest: Some(jaynes_core::Digest::of_bytes(r.agent.as_bytes()).0) }
    } else {
        Agent { program, model, command_digest: None }
    };
    let configuration = r
        .manifest
        .steps
        .iter()
        .map(|d| {
            serde_json::json!({
                "name": d.name, "executor": d.executor, "needs": d.needs, "reach": d.reach, "effect": d.effect, "checks": d.checks,
                "stop": d.stop, "from_risk": d.from_risk, "hidden": d.hidden,
            })
        })
        .collect();
    let steps = s
        .steps
        .iter()
        .enumerate()
        .map(|(i, line)| Step {
            name: line.name.clone(),
            by: line.by,
            state: line.state,
            attempts: line.attempts,
            rounds: line.rounds,
            checks: r.steps[i]
                .checks
                .iter()
                .map(|c| Check { name: c.check.clone(), kind: serde_json::to_value(c.kind).unwrap_or(Value::Null), passed: c.passed, blocked: c.blocked })
                .collect(),
        })
        .collect();
    let promises = match crate::summary::promises(home, run, true) {
        crate::summary::Promises::Unread => Value::String("unread: the run's copy of the role is gone".into()),
        crate::summary::Promises::Proving => Value::String("being proved".into()),
        crate::summary::Promises::Told(told) => told.iter().map(|p| serde_json::json!({"says": p.says, "proved": p.proved})).collect(),
    };
    Ok(Declaration {
        format: FORMAT,
        holds_words: words,
        words_left_out: (!words).then_some(WORDS_LEFT_OUT),
        made_by: MadeBy { engine: jaynes_core::ENGINE.to_string(), version: crate::install::version() },
        record: Place { first: first.0, last: last.0, head: last.2.clone() },
        run: RunPart {
            id: r.id.clone(),
            started: iso(s.started),
            ended: iso(s.ended),
            outcome: if matches!(r.phase, jaynes_core::run::Phase::Finished { outcome: Outcome::Abandoned }) { "abandoned" } else { "delivered" },
            result,
        },
        request: Request { text: words.then(|| r.request.clone()), digest: jaynes_core::Digest::of_bytes(r.request.as_bytes()).0 },
        role: Role { name: r.manifest.role.clone(), version: r.manifest.version.clone(), fingerprint: r.role_fingerprint.0.clone() },
        agent,
        configuration,
        author: Author { who: "the person who started the run", identity: None },
        delivery: Delivery {
            kind: match r.manifest.delivery {
                jaynes_core::manifest::DeliveryKind::Branch => "branch",
                jaynes_core::manifest::DeliveryKind::Merge => "merge",
            },
            delivered: s.delivered.is_some(),
            reference: s.delivered.clone(),
            corrected,
            undone,
        },
        steps,
        approvals,
        changed: s.changed.iter().map(|(what, size)| Changed { what: what.clone(), size: *size, floor: s.floor.contains(what) }).collect(),
        promises,
        not_proved: s.unproved.clone(),
        refused: crate::refused::of_run(home, run).iter().map(crate::refused::Refusal::line_out).collect(),
        cost: Cost { agent_seconds: s.agent_seconds, tokens: s.tokens, attempts: s.attempts, spent: s.spent, fixes: s.fixes },
        people: People {
            stops: s.person.stops,
            sent_back: s.person.sent_back,
            acts: s.person.acts,
            presence_seconds: s.person.seconds,
            unmeasured: s.person.unmeasured,
            waited_seconds: s.person.waited,
        },
        absent: ABSENT.iter().map(|&(what, filled_by)| Absent { what, filled_by }).collect(),
    })
}

/// Every field where `file` says other than the record does now, by its path in the file ("approvals[0].by"); none:
/// the file is what the record says. The record's own chain is checked first, and the program that made the file is
/// not compared. A file without the person's words is made again without them.
pub fn differences(home: &Path, file: &Value) -> Result<Vec<String>, String> {
    if file["format"] != FORMAT {
        return Err(format!("this is not a run's summary in {FORMAT}: its format is {}", file["format"]));
    }
    let run = file["run"]["id"].as_str().ok_or("the file names no run")?;
    let store = Store::open(&home.join("record.sqlite3"))?;
    if let Some((seq, _)) = store.verify()? {
        return Err(format!("the record was changed at entry {seq}: nothing it says can check a file"));
    }
    let words = file["holds_words"].as_bool().ok_or("the file does not say whether it holds the person's words")?;
    let again = serde_json::to_value(declare(home, run, words)?).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    compare("", file, &again, &mut out);
    out.retain(|d| !d.starts_with("made_by"));
    Ok(out)
}

/// The paths where `a` (the file) and `b` (the record) differ, each with what both say.
fn compare(at: &str, a: &Value, b: &Value, out: &mut Vec<String>) {
    let join = |k: &str| if at.is_empty() { k.to_string() } else { format!("{at}.{k}") };
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let keys: std::collections::BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for k in keys {
                compare(&join(k), x.get(k).unwrap_or(&Value::Null), y.get(k).unwrap_or(&Value::Null), out);
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                compare(&format!("{at}[{i}]"), p, q, out);
            }
        }
        _ if a != b => out.push(format!("{at}: the file says {}, the record says {}", short(a), short(b))),
        _ => {}
    }
}

/// A value in at most 80 characters.
fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() <= 80 { s } else { format!("{}…", s.chars().take(79).collect::<String>()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jaynes_core::event::{Cost as RunCost, Usage};
    use jaynes_core::manifest::Manifest;
    use jaynes_core::question::{Answer, Item, Question, QuestionKind, VerdictKind};
    use jaynes_core::{Digest, Input};

    /// The person's words, planted wherever they write.
    const WORD: &str = "PLANTEDword";

    /// A home whose record holds one finished run: an agent planned, the person changed it with a note and a
    /// context and brought back what they dropped, it was delivered, then corrected and undone with notes.
    fn home(name: &str) -> std::path::PathBuf {
        let home = std::env::temp_dir().join(format!("jaynes-declaration-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("runs/d1")).expect("dir");
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "role": "plan-it", "version": "2", "protocol": 1, "delivery": "merge",
            "steps": [{"name": "plan", "executor": "agent", "checks": [{"name": "shape", "kind": "open"}], "stop": {"verdicts": ["keep", "change"], "holds": true}}],
        }))
        .expect("manifest");
        let start = Input::Start {
            run: "d1".into(), request: format!("{WORD} make the plan"), manifest, role_fingerprint: Digest("fp".into()),
            agent: "claude:some-model".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false,
            starter: None,
        };
        let mut events = jaynes_core::decide(None, &start).expect("start").events;
        let item = Item { named: None, id: "a".into(), label: "the plan".into(), digest: Digest("d".into()) };
        events.push(Event::AttemptDelivered {
            step: "plan".into(), attempt: 1, fix: 0, output: Digest("o".into()), items: vec![item.clone()],
            changed: vec![jaynes_core::risk::Changed::file("plan.md".into(), 4)], usage: Usage { seconds: 70, tokens: 900, cached: 0, ..Default::default() }, session: None, tree: None,
            reply: None,
        });
        let question = Question { rides: Vec::new(),
            id: "q1".into(), step: "plan".into(), round: 0, kind: QuestionKind::Decision, items: vec![item],
            offered: vec![VerdictKind::Keep, VerdictKind::Change], holds: true, about: "Is the plan right?".into(), choose: false, pass: 0, passes: 0, back: vec![],
        };
        let verdicts = [("a".to_string(), Verdict::Change { text: format!("{WORD} shorter") })].into_iter().collect();
        let back = [("old".to_string(), format!("{WORD} as it was"))].into_iter().collect();
        let answer = Answer { back, context: format!("{WORD} the context"), question: "q1".into(), shown: question.shown(), verdicts, by: By::Person };
        events.push(Event::QuestionOpened { question });
        events.push(Event::AnswerRecorded { answer, effects: Default::default() });
        events.push(Event::Delivered { digest: Digest("o".into()), reference: "jaynes/d1 at abc123; merged as def456".into() });
        events.push(Event::RunFinished { outcome: Outcome::Delivered, cost: RunCost::default() });
        events.push(Event::DeliveryCorrected { note: format!("{WORD} it was wrong"), by: By::Person, step: Some("plan".into()) });
        events.push(Event::MergeUndone { note: format!("{WORD} take it back"), by: By::Person });
        Store::open(&home.join("record.sqlite3")).expect("store").append("d1", &events).expect("append");
        home
    }

    fn value(home: &Path, words: bool) -> Value {
        serde_json::to_value(declare(home, "d1", words).expect("declared")).expect("json")
    }

    #[test]
    fn a_run_s_declaration_says_what_its_record_holds_and_a_field_changed_in_the_file_is_named() {
        let h = home("fields");
        let d = declare(&h, "d1", true).expect("declared");
        let text = serde_json::to_string(&d).expect("json");
        // the format first, then whether the person's words are in it
        assert!(text.starts_with(r#"{"format":"jaynes-run-summary/1","holds_words":true,"made_by":"#), "{text}");
        let v: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(v["run"]["result"], "undone");
        assert_eq!(v["request"]["text"], format!("{WORD} make the plan"));
        assert_eq!((v["agent"]["program"].as_str(), v["agent"]["model"].as_str()), (Some("claude"), Some("some-model")));
        assert_eq!(v["delivery"]["kind"], "merge");
        let a = &v["approvals"][0];
        assert_eq!((a["by"].as_str(), a["proof"].as_str(), a["before_delivery"].as_bool()), (Some("person"), Some("word"), Some(true)), "{a}");
        assert_eq!(a["verdicts"]["a"]["verdict"], "change");
        assert_eq!(v["steps"][0]["checks"], Value::Array(vec![]), "no judgment was recorded");
        assert!(v["absent"].as_array().expect("absent").iter().any(|x| x["what"] == "the tree each check judged"), "{}", v["absent"]);
        assert!(!text.contains("attestation"));
        // the file is what the record says
        assert_eq!(differences(&h, &v), Ok(vec![]));
        // every field changed is named, and the program that made the file is never compared
        for (path, to) in [
            ("/approvals/0/verdicts/a/verdict", Value::from("keep")),
            ("/approvals/0/before_delivery", Value::from(false)),
            ("/approvals/0/proof", Value::from("device 0011223344556677")),
            ("/request/text", Value::from("another request")),
            ("/run/result", Value::from("accepted")),
            ("/cost/tokens", Value::from(1)),
        ] {
            let mut changed = v.clone();
            *changed.pointer_mut(path).expect(path) = to;
            let said = differences(&h, &changed).expect("compared");
            let field = path.trim_start_matches('/').replace("/0/", "[0].").replace('/', ".");
            assert!(said.len() == 1 && said[0].starts_with(&format!("{field}: the file says ")), "{path}: {said:?}");
        }
        let mut newer = v.clone();
        newer["made_by"]["version"] = "jaynes 9.9.9".into();
        assert_eq!(differences(&h, &newer), Ok(vec![]));
        let _gone = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn without_words_none_of_the_person_s_words_is_in_the_file_and_it_still_checks() {
        let h = home("words");
        let all = serde_json::to_string(&value(&h, true)).expect("json");
        // with words: the request, the change's note, the context, what was brought back, the correction and the undo
        assert_eq!(all.matches(WORD).count(), 6, "{all}");
        let text = serde_json::to_string(&declare(&h, "d1", false).expect("declared")).expect("json");
        let v: Value = serde_json::from_str(&text).expect("json");
        assert!(!text.contains(WORD), "{text}");
        assert!(text.starts_with(r#"{"format":"jaynes-run-summary/1","holds_words":false,"words_left_out":"the person's words are left out"#), "{text}");
        // what is not words stays: the request's fingerprint, the verdicts, the notes' places
        assert_eq!(v["request"]["digest"], Digest::of_bytes(format!("{WORD} make the plan").as_bytes()).0);
        assert_eq!(v["approvals"][0]["verdicts"]["a"]["verdict"], "change");
        assert!(v["approvals"][0]["brought_back"]["old"].is_null());
        assert_eq!(v["delivery"]["corrected"][0]["step"], "plan");
        assert_eq!(differences(&h, &v), Ok(vec![]));
        // a word put back in is not what the record says without them
        let mut put_back = v.clone();
        put_back["request"]["text"] = format!("{WORD} make the plan").into();
        assert_eq!(differences(&h, &put_back).expect("compared").len(), 1);
        let _gone = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn an_answer_says_whose_it_was_and_what_proved_it() {
        let h = home("proofs");
        Store::open(&h.join("record.sqlite3")).expect("store").put_proof("d1", "q1", "a-public-key", "a-signature").expect("proof");
        let v = value(&h, true);
        assert_eq!(v["approvals"][0]["proof"], format!("device {}", &Digest::of_bytes(b"a-public-key").0[..16]));
        let _gone = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn a_changed_record_checks_no_file() {
        let h = home("tampered");
        let v = value(&h, true);
        let store = Store::open(&h.join("record.sqlite3")).expect("store");
        let first = v["record"]["first"].as_u64().expect("first");
        store.tamper(first, r#"{"type":"RunStarted"}"#);
        let refused = differences(&h, &v).expect_err("a changed record");
        assert!(refused.starts_with(&format!("the record was changed at entry {first}")), "{refused}");
        assert!(differences(&h, &serde_json::json!({"format": "something else"})).expect_err("not ours").contains("is not a run's summary"));
        let _gone = std::fs::remove_dir_all(&h);
    }
}
