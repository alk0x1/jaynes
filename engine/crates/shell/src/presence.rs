//! A person's presence at a stop: what the people's minutes of a run's summary are measured from.
//!
//! The screen of a stop (the page's, and the app's) sends a signal every [`EVERY`] seconds while the person is on it:
//! the screen in front, its window focused, and a key, the pointer or a scroll within the last minute. Each signal
//! says the person was there for the [`EVERY`] seconds before it. Signals that follow each other within [`GAP`]
//! seconds make one span; a longer silence starts another.
//!
//! What this is: time on Jaynes's screens, a floor. It does not see the person reading the change in their editor,
//! a page the agent made opened in a tab of its own, nor an answer given from the terminal.
//!
//! The spans are kept in a table of their own beside the record, NOT in the record's chain: they are a measure, not
//! a guarantee, and nothing proves them. Read them as an estimate, never as evidence.
//!
//! [`look`] is the one way in: the page's endpoint and the app both call it.

use crate::store::Store;
use jaynes_core::question::By;
use std::path::Path;

/// Seconds between two signals of a screen, and the presence each signal stands for.
pub const EVERY: u64 = 15;
/// Seconds of silence after which the next signal starts a new span.
pub const GAP: u64 = 30;

/// A span of a person's presence at a stop, in seconds of the record's clock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Look {
    pub run: String,
    pub question: String,
    pub from: u64,
    pub to: u64,
}

/// Record that the person is on the screen of `question` of `run` at `at`. Only the person's signal counts, and
/// only while the question is open: a signal that comes after the answer, or for a question that does not wait, is
/// dropped. Whether it was recorded comes back.
pub fn look(home: &Path, run: &str, question: &str, by: &By, at: u64) -> Result<bool, String> {
    if !by.person() {
        return Err("only the person's presence counts".into());
    }
    let mut store = Store::open(&home.join("record.sqlite3"))?;
    let open = store.state(run)?.is_some_and(|r| !r.finished() && r.questions.get(question).is_some_and(|q| q.open()));
    if !open {
        return Ok(false);
    }
    store.put_look(run, question, at, EVERY, GAP)?;
    Ok(true)
}

/// Every span of presence recorded for `run`.
pub fn looks(home: &Path, run: &str) -> Result<Vec<Look>, String> {
    let store = Store::open(&home.join("record.sqlite3"))?;
    Ok(store.looks(run)?.into_iter().map(|(question, from, to)| Look { run: run.to_string(), question, from, to }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jaynes_core::event::Effects;
    use jaynes_core::question::{Answer, Item, Question, QuestionKind, VerdictKind};
    use jaynes_core::{Digest, Event, Input, Manifest};

    fn home(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("jaynes-presence-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A run whose plan's question q1 is open.
    fn asked(home: &Path) {
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "role": "r", "version": "1", "protocol": 1,
            "steps": [{"name": "plan", "executor": "agent", "stop": {"verdicts": ["keep", "redo"], "holds": true}}],
        }))
        .expect("manifest");
        let start = Input::Start { run: "r1".into(), request: "x".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        let mut events = jaynes_core::decide(None, &start).expect("start").events;
        let question = Question { rides: Vec::new(),
            id: "q1".into(), step: "plan".into(), round: 0, kind: QuestionKind::Decision, items: vec![Item { named: None, options: Vec::new(), id: "a".into(), label: "a".into(), digest: Digest("d".into()) }],
            offered: vec![VerdictKind::Keep], holds: true, about: String::new(), choose: false, pass: 0, passes: 0, back: vec![],
        };
        events.push(Event::QuestionOpened { question });
        Store::open(&home.join("record.sqlite3")).expect("store").append("r1", &events).expect("append");
    }

    fn answered(home: &Path) {
        let answer = Answer { question: "q1".into(), shown: Digest("s".into()), verdicts: Default::default(), by: By::Person, context: String::new(), back: Default::default() };
        Store::open(&home.join("record.sqlite3")).expect("store").append("r1", &[Event::AnswerRecorded { answer, effects: Effects::default() }]).expect("append");
    }

    fn span(from: u64, to: u64) -> Look {
        Look { run: "r1".into(), question: "q1".into(), from, to }
    }

    #[test]
    fn signals_close_together_make_one_span_and_a_silence_starts_another() {
        let h = home("spans");
        asked(&h);
        for at in [1_000, 1_015, 1_030, 1_100, 1_115] {
            assert_eq!(look(&h, "r1", "q1", &By::Person, at), Ok(true), "{at}");
        }
        // each signal stands for the 15 s before it; 70 s of silence is more than the gap
        assert_eq!(looks(&h, "r1"), Ok(vec![span(985, 1_030), span(1_085, 1_115)]));
    }

    #[test]
    fn only_the_person_s_signal_counts() {
        let h = home("person");
        asked(&h);
        let agent = By::Agent { who: "a client without the page's key".into() };
        assert_eq!(look(&h, "r1", "q1", &agent, 1_000), Err("only the person's presence counts".to_string()));
        assert_eq!(look(&h, "r1", "q1", &By::Rule { rule: "x".into() }, 1_000), Err("only the person's presence counts".to_string()));
        assert_eq!(looks(&h, "r1"), Ok(vec![]));
    }

    #[test]
    fn a_signal_after_the_answer_or_for_no_such_question_is_dropped() {
        let h = home("late");
        asked(&h);
        assert_eq!(look(&h, "r1", "q1", &By::Person, 1_000), Ok(true));
        answered(&h);
        assert_eq!(look(&h, "r1", "q1", &By::Person, 1_015), Ok(false));
        assert_eq!(look(&h, "r1", "q9", &By::Person, 1_015), Ok(false));
        assert_eq!(look(&h, "r9", "q1", &By::Person, 1_015), Ok(false));
        assert_eq!(looks(&h, "r1"), Ok(vec![span(985, 1_000)]));
    }
}
