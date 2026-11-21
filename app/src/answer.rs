//! How the app answers a stop: it builds the same body the page sends, so the engine reads it with the page's own
//! code (`page::answer_in`); what the answer will do is the engine's sentences (G11); the person's words are read by
//! the page's own reader (`page::understand_in`); and an answer goes signed with the person's key, after their touch
//! when something that cannot be undone lies ahead.

use crate::key::Key;
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
    let sentences = match jaynes_shell::drive::effects(&home, &run, &q.question, &answer) {
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

/// What macOS says when it asks for the person's touch: only when something that cannot be undone lies ahead.
pub fn touch_reason(ahead: Option<&str>) -> Option<String> {
    ahead.map(|a| format!("send your answer to Jaynes: {a}"))
}

/// Sends the answer as the person's: their touch first when something that cannot be undone lies ahead, then signed
/// with their key. Resolves to what to tell them.
pub async fn send(key: Option<Arc<Key>>, ahead: Option<String>, body: Value) -> Result<(), String> {
    let key = key.ok_or("the app has no key of yours, so it cannot answer as you")?;
    if let Some(reason) = touch_reason(ahead.as_deref()) {
        crate::presence::ask(&reason).recv().await.unwrap_or_else(|_| Err("no answer from macOS".into()))?;
    }
    let home = jaynes_shell::home();
    let (run, _, answer) = jaynes_shell::page::answer_in(&home, &body, By::Agent { who: "the Jaynes app, before it is signed".into() })?;
    let signature = key.sign(&jaynes_shell::person::message(&run.id, &answer));
    jaynes_shell::person::submit(&home, &engine_program(), &run, answer, &key.public(), &signature).map(|_| ())
}
