//! What the page refused: an attempt to answer or act on a run that did not go through, with when, on what, by whom
//! and why. An agent trying to answer as the person, a site or an app on the machine sending a request in the
//! person's place, a stale answer: each leaves a trace, which the run's screen and its summary show.
//!
//! The traces are kept in a table of their own beside the record, NOT in the record's chain: nothing proves them,
//! and they change nothing in the run. Read them as a trace, never as evidence.

use crate::store::Store;
use jaynes_core::question::By;
use std::path::Path;

/// One refused attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub at: u64,
    pub run: String,
    /// What was tried: "answer to STEP", "answer", "act ACTION".
    pub what: String,
    /// Who tried, as the page knew them: "the person", an agent's name, or where the request came from.
    pub who: String,
    pub reason: String,
}

/// Who an answer or an action came from, in the words a refusal keeps.
pub fn who(by: &By) -> String {
    match by {
        By::Person => "the person".to_string(),
        By::Rule { rule } => format!("the rule {rule}"),
        By::Agent { who } => who.clone(),
    }
}

/// Keep a trace of a refused attempt. A trace that cannot be kept is lost, never a reason to answer otherwise.
pub fn record(home: &Path, r: &Refusal) {
    if r.run.trim().is_empty() {
        return;
    }
    if let Ok(store) = Store::open(&home.join("record.sqlite3")) {
        let _kept = store.put_refusal(r.at, &r.run, &r.what, &r.who, &r.reason);
    }
}

/// Every refused attempt on `run`, oldest first.
pub fn of_run(home: &Path, run: &str) -> Vec<Refusal> {
    let Ok(store) = Store::open(&home.join("record.sqlite3")) else { return Vec::new() };
    store
        .refusals(run)
        .unwrap_or_default()
        .into_iter()
        .map(|(at, what, who, reason)| Refusal { at, run: run.to_string(), what, who, reason })
        .collect()
}

impl Refusal {
    /// The trace in a line: when, what, by whom, and why.
    pub fn line(&self) -> String {
        format!("{}, {}, by {}: {}", crate::summary::utc(self.at), self.what, self.who, self.reason)
    }
}
