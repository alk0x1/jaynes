//! What the page refused: an attempt to answer or act on a run that did not go through, with when, on what, by whom
//! and why. An agent trying to answer as the person, a site or an app on the machine sending a request in the
//! person's place, a stale answer: each leaves a trace, which the run's screen and its summary show.
//!
//! The traces are kept in a table of their own beside the record, NOT in the record's chain: nothing proves them,
//! and they change nothing in the run. Read them as a trace, never as evidence.

use crate::store::Store;
use jaynes_core::question::By;
use std::path::Path;

/// Traces kept per run, the latest: whoever is refused writes them, so a program on the machine could otherwise fill
/// the table without end.
pub const MOST_PER_RUN: usize = 200;
/// Characters kept of what a refused request says (what it tried, who it was, why it was refused).
pub const MOST_SAID: usize = 200;

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

/// Keep a trace of a refused attempt, on a run that exists only: the request names its run, and could name any. What
/// it says is cut to [`MOST_SAID`], and the run keeps its latest [`MOST_PER_RUN`]. A trace that cannot be kept is
/// lost, never a reason to answer otherwise.
pub fn record(home: &Path, r: &Refusal) {
    let Ok(mut store) = Store::open(&home.join("record.sqlite3")) else { return };
    if r.run.trim().is_empty() || !store.state(&r.run).is_ok_and(|s| s.is_some()) {
        return;
    }
    let _kept = store.put_refusal(r.at, &r.run, &cut(&r.what), &cut(&r.who), &cut(&r.reason), MOST_PER_RUN);
}

/// A text from outside, at most [`MOST_SAID`] characters, saying so when it was cut.
fn cut(text: &str) -> String {
    if text.chars().count() <= MOST_SAID { text.to_string() } else { format!("{}…", text.chars().take(MOST_SAID - 1).collect::<String>()) }
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

#[cfg(test)]
mod tests {
    use super::*;
    use jaynes_core::{Digest, Input, Manifest};

    fn home(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("jaynes-refused-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&dir);
        let manifest: Manifest = serde_json::from_value(serde_json::json!({"role": "r", "version": "1", "protocol": 1, "steps": [{"name": "plan", "executor": "agent"}]})).expect("manifest");
        let start = Input::Start { run: "r1".into(), request: "x".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default() };
        let events = jaynes_core::decide(None, &start).expect("start").events;
        Store::open(&dir.join("record.sqlite3")).expect("store").append("r1", &events).expect("append");
        dir
    }

    fn tried(run: &str, n: u64, long: usize) -> Refusal {
        Refusal { at: n, run: run.into(), what: format!("act {}", "x".repeat(long)), who: format!("a request from {}", "o".repeat(long)), reason: "r".repeat(long) }
    }

    #[test]
    fn a_thousand_refusals_keep_only_the_latest_and_never_grow_past_the_ceiling() {
        let h = home("ceiling");
        for n in 0..1000 {
            record(&h, &tried("r1", n, 10));
        }
        let kept = of_run(&h, "r1");
        assert_eq!(kept.len(), MOST_PER_RUN);
        assert_eq!(kept.first().map(|r| r.at), Some(1000 - MOST_PER_RUN as u64), "the oldest go first");
        assert_eq!(kept.last().map(|r| r.at), Some(999));
    }

    #[test]
    fn what_a_refused_request_says_is_kept_cut_and_only_for_a_run_that_exists() {
        let h = home("cut");
        record(&h, &tried("r1", 1, 100_000));
        let kept = of_run(&h, "r1");
        assert_eq!(kept.len(), 1);
        for text in [&kept[0].what, &kept[0].who, &kept[0].reason] {
            assert!(text.chars().count() <= MOST_SAID, "{}", text.chars().count());
            assert!(text.ends_with('…'), "a cut text says it was cut");
        }
        // a run nobody started: a request names any id it likes, and none is kept
        for n in 0..50 {
            record(&h, &tried(&format!("made-up-{n}"), n, 10));
        }
        assert!(of_run(&h, "made-up-0").is_empty());
        assert_eq!(Store::open(&h.join("record.sqlite3")).expect("store").refusals("made-up-7").expect("read").len(), 0);
    }
}
