//! How the app answers a stop: it builds the same body the page sends, so the engine reads it with the page's own
//! code (`page::answer_in`); what the answer will do is the engine's sentences (G11); the person's words are read by
//! the page's own reader (`page::understand_in`); and an answer goes signed with the person's key, after their touch
//! when something that cannot be undone lies ahead.

use crate::key::Key;
use jaynes_core::manifest::Offer;
use jaynes_core::question::By;
use jaynes_shell::stop::Stop;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

/// The body of an answer: a verdict per item (`keep`, `change`, `redo`, `drop`, `skip`), the person's words for a
/// change or a redo, what they bring back with their words on how, and their words for the agents after.
pub fn body(stop: &Stop, verdicts: &BTreeMap<String, String>, notes: &BTreeMap<String, String>, back: &BTreeMap<String, String>, context: &str) -> Value {
    json!({
        "run": stop.run,
        "question": stop.question,
        "shown": stop.shown,
        "verdicts": verdicts,
        "notes": notes,
        "back": back,
        "context": context.trim(),
    })
}

/// What the answer would do, in the engine's sentences, and why the engine would refuse it, if it would.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reckoned {
    pub sentences: Vec<String>,
    pub refused: Option<String>,
}

pub fn reckon(body: &Value) -> Reckoned {
    let home = jaynes_shell::home();
    // what it does as the person's answer, which it becomes once signed
    let (run, q, answer) = match jaynes_shell::page::answer_in(&home, body, By::Person) {
        Ok(read) => read,
        Err(e) => return Reckoned { sentences: Vec::new(), refused: Some(e) },
    };
    // a run whose engine left the Mac: the card says so in place of its question, the engine's words in Details; the
    // foot has nothing to add (the design-lead, 22/11)
    if jaynes_shell::stop::gone(&home, &run).is_some() {
        return Reckoned::default();
    }
    let sentences = match jaynes_shell::page::effects_of(&home, &run, &q.question, &answer, body) {
        // before it goes: a choice is being made, not made yet ("Choosing “Harbor”: …")
        Ok(fx) => fx.sentences.iter().map(|s| jaynes_core::decide::before_sending(s)).collect(),
        Err(e) => vec![format!("Jaynes cannot say what this does: {e}")],
    };
    let refused = (run.engine == jaynes_core::ENGINE).then(|| jaynes_core::decide(Some(&run), &jaynes_core::Input::Answer { answer }).err().map(|r| r.0)).flatten();
    Reckoned { sentences, refused }
}

/// The person's words read into the answer they mean: a proposal, with what it does, never sent by itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Understood {
    pub body: Value,
    pub sentences: Vec<String>,
    pub refused: Option<String>,
    pub unsure: String,
    pub repeats: Vec<String>,
}

/// Reads the person's words with the run's model (the page's reader). Slow: a model reads them.
pub fn understand(stop: &Stop, words: &str, pressed: Option<&str>) -> Result<Understood, String> {
    let home = jaynes_shell::home();
    let mut asked = Map::new();
    asked.insert("run".into(), json!(stop.run));
    asked.insert("question".into(), json!(stop.question));
    asked.insert("shown".into(), json!(stop.shown));
    asked.insert("text".into(), json!(words));
    if let Some(p) = pressed {
        asked.insert("pressed".into(), json!(p));
    }
    let r = jaynes_shell::page::understand_in(&home, &Value::Object(asked), By::Person)?;
    Ok(Understood {
        body: r["body"].clone(),
        sentences: r["sentences"].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_string)).collect(),
        refused: r["refused"].as_str().map(str::to_string),
        unsure: r["unsure"].as_str().unwrap_or("").to_string(),
        repeats: r["repeats"].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_string)).collect(),
    })
}

/// The engine program that carries a run on after an answer: JAYNES_BIN, else the installed `jaynes`.
pub fn engine_program() -> PathBuf {
    std::env::var_os("JAYNES_BIN").map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        home.join(".local/bin/jaynes")
    })
}

/// What macOS says when it asks for the person's touch before this stop's answer goes, as the engine says it: before
/// an answer that leads to a step that cannot be undone, and once at the merge into their branch. None at a leave and
/// at a decision before a merge, and then no touch is asked.
pub fn touch_of(stop: &Stop) -> Option<&str> {
    stop.touch.as_deref()
}

/// One answer for every item of the stop, all `v`: the merge's Merge (keep) or Branch only (drop).
pub fn all(stop: &Stop, v: jaynes_core::question::VerdictKind) -> Value {
    let verdicts = stop.items.iter().map(|i| (i.id.clone(), crate::island::word(v).to_string())).collect();
    body(stop, &verdicts, &BTreeMap::new(), &BTreeMap::new(), "")
}

/// Sends the person's question to the agent at a stop, about one of its items or the whole: the agent answers in a
/// turn that only reads, and nothing is redone or chosen, so no touch is asked. Signed with the app's key over the
/// engine's ask (`person::ask_message`), never an answer's, so it cannot stand for one.
pub async fn ask(key: Option<Arc<Key>>, run: String, question: String, item: Option<String>, text: String) -> Result<(), String> {
    let key = key.ok_or("the app has no key of yours, so it cannot ask as you")?;
    let home = jaynes_shell::home();
    let state = jaynes_shell::store::Store::open(&home.join("record.sqlite3"))?.state(&run)?.ok_or_else(|| format!("there is no run {run}"))?;
    let signature = key.sign(&jaynes_shell::person::ask_message(&run, &question, item.as_deref(), &text));
    jaynes_shell::person::submit_ask(&home, &engine_program(), &state, &question, item.as_deref(), &text, &key.public(), &signature).map(|_| ())
}

/// Moves runs whose engine left the Mac to Finished, as the person's: signed with the app's key over the engine's move
/// (`person::move_message`), never an answer's. All or none; says how many were moved now.
pub async fn move_to_finished(key: Option<Arc<Key>>, runs: Vec<String>) -> Result<usize, String> {
    let key = key.ok_or("the app has no key of yours, so it cannot move runs as you")?;
    let home = jaynes_shell::home();
    let signature = key.sign(&jaynes_shell::person::move_message(&runs));
    jaynes_shell::person::submit_move(&home, &runs, &key.public(), &signature)
}

/// What the stop offers that the person marked, as the page's act "grant" for the stop `body` answers: the person's
/// alone, and the core refuses it from anyone else (the engineer lead's T5, 23/11).
pub fn grant_body(body: &Value, offer: Offer) -> Value {
    json!({"run": body["run"], "action": "grant", "question": body["question"], "offer": offer})
}

/// Sends the answer as the person's: their touch first when the stop asks it (`touch_of`), with its reason; then what
/// they marked of what the stop offers (`grants`), as theirs; then the answer, signed with their key. Resolves to what
/// to tell them.
pub async fn send(key: Option<Arc<Key>>, touch: Option<String>, grants: Vec<Offer>, body: Value) -> Result<(), String> {
    let key = key.ok_or("the app has no key of yours, so it cannot answer as you")?;
    if let Some(reason) = touch {
        crate::presence::ask(&reason).recv().await.unwrap_or_else(|_| Err("no answer from macOS".into()))?;
    }
    let home = jaynes_shell::home();
    for offer in grants {
        jaynes_shell::page::act_in(&home, &engine_program(), &grant_body(&body, offer), By::Person)?;
    }
    let (run, q, answer) = jaynes_shell::page::answer_in(&home, &body, By::Agent { who: "the Jaynes app, before it is signed".into() })?;
    // the person's leave to change how the tests are defined goes first, signed over their words, with an answer that
    // sets an agent to work (T1)
    if let Some(words) = jaynes_shell::page::leave_in(&home, &run, &q.question, &answer, &body) {
        let signed = key.sign(&jaynes_shell::person::leave_message(&run.id, &words));
        jaynes_shell::person::submit_leave(&home, &engine_program(), &run, &words, &key.public(), &signed)?;
    }
    let signature = key.sign(&jaynes_shell::person::message(&run.id, &answer));
    jaynes_shell::person::submit(&home, &engine_program(), &run, answer, &key.public(), &signature).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::grant_body;
    use jaynes_core::manifest::Offer;
    use serde_json::json;

    // the engineer lead's T5 (23/11): a marked box goes before the answer as the page's own act "grant", for the run
    // and the stop the answer is for, and the core reads its offer by name
    #[test]
    fn asked_a_marked_box_goes_as_the_page_s_act_grant_for_the_stop_answered() {
        let body = json!({"run": "new-1", "question": "q1-stacks", "shown": "s", "verdicts": {}});
        assert_eq!(grant_body(&body, Offer::TestServers), json!({"run": "new-1", "action": "grant", "question": "q1-stacks", "offer": "test_servers"}));
    }
}
