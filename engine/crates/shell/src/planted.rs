//! Defects an evaluation plants on purpose (stage 2): someone of the partner's who does not review plants one into a
//! step's change before its stop shows it, to see whether the person who reviews looks, and takes it out before
//! anything is delivered. The record holds only that a defect is in, by its id and step, so the core holds the
//! delivery while one is; who planted it, the note, the patch and what became of it are kept here, in the run's own
//! folder, and leave only in the evaluation's export (`jaynes export --evaluation RUN`). Nothing the person who
//! reviews reads says a defect was planted: not the log, the run's screen, its page or its summary.

use jaynes_core::event::Event;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

/// A planted defect, as the evaluation keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Defect {
    pub id: String,
    pub step: String,
    /// Who planted it, in their own words.
    pub planter: String,
    /// What the defect is, for the evaluator.
    pub note: String,
    /// The patch as it was given.
    pub patch: String,
    /// The files it changes.
    pub files: Vec<String>,
    /// Seconds since 1970, when it went in.
    pub at: u64,
    /// The step's change as it was, and the change with the defect in it.
    pub original: String,
    pub planted: String,
    #[serde(default)]
    pub removed: Option<Removed>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Removed {
    pub by: String,
    pub at: u64,
    /// The step's change is again what its checks judged: the tree recorded with its output.
    pub verified: bool,
}

const FILE: &str = "evaluation.json";

/// The defects planted in the run whose folder is `dir`, oldest first.
pub fn read(dir: &Path) -> Vec<Defect> {
    std::fs::read_to_string(dir.join(FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn write(dir: &Path, defects: &[Defect]) -> Result<(), String> {
    let tmp = dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_string_pretty(defects).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dir.join(FILE)).map_err(|e| e.to_string())
}

/// The id the next defect planted in the run takes: d1, d2, and so on.
pub fn next_id(dir: &Path) -> String {
    format!("d{}", read(dir).len() + 1)
}

/// The defect of `step` still in, if any: its change with the defect and the change as it was.
pub fn active(dir: &Path, step: &str) -> Option<Defect> {
    read(dir).into_iter().find(|d| d.step == step && d.removed.is_none())
}

/// An entry of the record the person who reviews never reads: the log, the run's screen and its summary leave it out.
pub fn hidden(e: &Event) -> bool {
    matches!(e, Event::DefectPlanted { .. } | Event::DefectRemoved { .. })
}

/// The answer the person gave at the stop of `step` after the defect `id` went into it, as the record holds it: the
/// question, each item's verdict and whether it sent the work back. None while no such answer was given.
fn answered_after(events: &[Event], id: &str, step: &str) -> Option<(String, serde_json::Map<String, Value>, bool, bool)> {
    let from = events.iter().position(|e| matches!(e, Event::DefectPlanted { id: i, .. } if i == id))?;
    events[from..].iter().find_map(|e| match e {
        Event::AnswerRecorded { answer, .. } => {
            let asked = events.iter().find_map(|q| match q {
                Event::QuestionOpened { question } if question.id == answer.question => Some(question),
                _ => None,
            })?;
            (asked.step == step && asked.kind == jaynes_core::question::QuestionKind::Decision).then(|| {
                let verdicts = answer.verdicts.iter().map(|(item, v)| (item.clone(), serde_json::to_value(v.kind()).unwrap_or(Value::Null))).collect();
                (answer.question.clone(), verdicts, answer.verdicts.values().any(|v| v.kind().sends_back()), answer.by.person())
            })
        }
        _ => None,
    })
}

/// The steps whose stop the person kept while a defect an evaluation planted was in it, once that defect is out: what
/// a run's summary tells them after, read from the record alone. Before the defect is out, nothing says it.
pub fn missed(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::DefectPlanted { id, step, .. } if events.iter().any(|r| matches!(r, Event::DefectRemoved { id: i, .. } if i == id)) => {
                answered_after(events, id, step).filter(|(_, _, sent_back, person)| !sent_back && *person).map(|_| step.clone())
            }
            _ => None,
        })
        .collect()
}

/// The evaluation's export of a run: each defect planted in it with what became of it, and the answer the person
/// gave at the stop of its step after it went in, for the evaluator to judge: caught (sent back) or missed.
pub fn export(dir: &Path, events: &[Event]) -> Value {
    let planted_at = |id: &str| events.iter().position(|e| matches!(e, Event::DefectPlanted { id: i, .. } if i == id));
    let defects: Vec<Value> = read(dir)
        .iter()
        .map(|d| {
            let answered = answered_after(events, &d.id, &d.step).map(|(question, verdicts, sent_back, by_person)| {
                // what the evaluation counts: the stop that showed it sent the work back (caught), or did not (missed)
                json!({"question": question, "verdicts": verdicts, "sent_back": sent_back, "result": if sent_back { "caught" } else { "missed" }, "by_person": by_person})
            });
            // nothing went out while it was in: no delivery between its planting and its taking out, nor after it while
            // it is still in. The core holds that; the record says it
            let never_delivered = planted_at(&d.id).is_some_and(|from| {
                events[from..]
                    .iter()
                    .take_while(|e| !matches!(e, Event::DefectRemoved { id, .. } if *id == d.id))
                    .all(|e| !matches!(e, Event::DeliveryAsked { .. } | Event::Delivered { .. }))
            });
            // as the record holds it, which a process taking the defect out writes before this file
            let still_in = !events.iter().any(|e| matches!(e, Event::DefectRemoved { id, .. } if *id == d.id));
            json!({
                "id": d.id, "step": d.step, "planter": d.planter, "note": d.note, "files": d.files, "patch": d.patch,
                "at": d.at, "still_in": still_in, "removed": d.removed, "never_delivered": never_delivered, "answered": answered,
            })
        })
        .collect();
    json!({"defects": defects})
}
