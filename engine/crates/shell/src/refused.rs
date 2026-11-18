//! What the page refused: an attempt to answer or act on a run that did not go through, with when, on what, by whom
//! and why. An agent trying to answer as the person, a site or an app on the machine sending a request in the
//! person's place, a stale answer: each leaves a trace, which the run's screen and its summary show.
//!
//! The traces are kept in a table of their own beside the record, NOT in the record's chain: nothing proves them,
//! and they change nothing in the run. Read them as a trace, never as evidence.

use crate::store::Store;
use jaynes_core::question::By;
use serde_json::Value;
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

/// What the page's actions are called: a refused action is kept by one of these names, or as "an unknown action",
/// never by what the request called it.
pub const ACTIONS: &[&str] = &["halt", "resume", "continue", "abandon", "raise-risk", "raise-ceiling", "undo", "correct"];

/// What was tried, in the page's words: an action it knows, by its name; any other, as an unknown action.
pub fn action(said: &str) -> String {
    if ACTIONS.contains(&said) { format!("act {said}") } else { "an unknown action".into() }
}

/// Who sent a request from another origin, for the trace beside the record: the origin, when it has an origin's
/// shape (http or https, a host of letters, digits, dots and hyphens, a port), or else only that it came from another.
/// The summary, which leaves the machine, names no origin at all ([`Refusal::line_out`]).
pub fn from_origin(origin: Option<&str>) -> String {
    let shaped = origin.is_some_and(|o| {
        let Some(rest) = o.strip_prefix("https://").or_else(|| o.strip_prefix("http://")) else { return false };
        let (host, port) = rest.split_once(':').unwrap_or((rest, ""));
        o.len() <= 100
            && !host.is_empty()
            && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
            && port.len() <= 5
            && port.bytes().all(|b| b.is_ascii_digit())
    });
    match origin {
        Some(o) if shaped => format!("{ANOTHER}{o}"),
        Some(_) => format!("{ANOTHER}another origin"),
        None => "a request that is not JSON".into(),
    }
}

const ANOTHER: &str = "a request from ";

/// The page's own words in a request: the verdicts and actions it sends by name. What else a request sends is its own.
const PAGE_WORDS: &[&str] = &["keep", "skip", "drop", "change", "redo"];

/// The engine's reason for a refusal, with every word the request sent taken out: the engine's errors may name what
/// they were given (a question, an item, a step, a verdict), and a trace keeps the engine's words only. Each text the
/// request brought (a value, or a key inside one of its fields, as an item's id), of three characters or more and not
/// one of the page's own words, the longest first, is put out of the reason. The fields' own names are the page's.
pub fn scrub(reason: &str, said: &Value) -> String {
    fn texts(v: &Value, inside: bool, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Array(a) => a.iter().for_each(|x| texts(x, true, out)),
            Value::Object(m) => m.iter().for_each(|(k, x)| {
                if inside {
                    out.push(k.clone());
                }
                texts(x, true, out);
            }),
            _ => {}
        }
    }
    let mut all = Vec::new();
    texts(said, false, &mut all);
    all.retain(|t| t.chars().count() >= 3 && !PAGE_WORDS.contains(&t.as_str()) && !ACTIONS.contains(&t.as_str()));
    all.sort_by_key(|t| std::cmp::Reverse(t.len()));
    let mut out = reason.to_string();
    for t in all {
        out = out.replace(&t, "(what the request said)");
    }
    out
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
    /// The trace in a line, for the person's own screen: when, what, by whom, and why.
    pub fn line(&self) -> String {
        format!("{}, {}, by {}: {}", crate::summary::utc(self.at), self.what, self.who, self.reason)
    }

    /// The trace in a line for what leaves the machine, the run's summary: an origin is a name its sender chose and
    /// can carry words, so a request from another origin is said to be one, and not named.
    pub fn line_out(&self) -> String {
        let who = if self.who.starts_with(ANOTHER) { "a request from another origin" } else { self.who.as_str() };
        format!("{}, {}, by {who}: {}", crate::summary::utc(self.at), self.what, self.reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trace_says_what_was_tried_and_by_whom_in_the_page_s_words_only() {
        assert_eq!(action("undo"), "act undo");
        assert_eq!(action("rm -rf the partner's secrets"), "an unknown action");
        assert_eq!(from_origin(Some("https://example.com")), "a request from https://example.com");
        assert_eq!(from_origin(Some("http://localhost:8751")), "a request from http://localhost:8751");
        for odd in ["http://evil.example/a/path", "javascript:alert(1)", "http://a b", "null", "http://x:99999999", &format!("http://{}.example", "a".repeat(100))] {
            assert_eq!(from_origin(Some(odd)), "a request from another origin", "{odd}");
        }
        assert_eq!(from_origin(None), "a request that is not JSON");
        // the engine's reason, with what it was given taken out; a short text stays, and a longer one goes first
        let said = serde_json::json!({"question": "q-from-the-repo", "verdicts": {"item-from-the-repo": "keep"}, "x": "ab"});
        assert_eq!(
            scrub("there is no question q-from-the-repo; the question has no item item-from-the-repo; ab; keep", &said),
            "there is no question (what the request said); the question has no item (what the request said); ab; keep"
        );
        // the summary names no origin
        let t = Refusal { at: 0, run: "r".into(), what: "answer".into(), who: from_origin(Some("https://example.com")), reason: "it did not come from this page".into() };
        assert!(t.line().contains("by a request from https://example.com:"), "{}", t.line());
        assert!(t.line_out().contains("by a request from another origin:") && !t.line_out().contains("example.com"), "{}", t.line_out());
    }
    use jaynes_core::{Digest, Input, Manifest};

    fn home(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("jaynes-refused-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&dir);
        let manifest: Manifest = serde_json::from_value(serde_json::json!({"role": "r", "version": "1", "protocol": 1, "steps": [{"name": "plan", "executor": "agent"}]})).expect("manifest");
        let start = Input::Start { run: "r1".into(), request: "x".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false };
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
