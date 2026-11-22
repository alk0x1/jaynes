//! What a partner of an evaluation sends out each week: the numbers of each finished run, and nothing else. No word of
//! the record leaves in it: not the request, a note, a step's, an item's or a file's name, a path, the agent's
//! command, nor the run's id. Each line has fixed fields, and each field is a number, a word from a fixed list, or a
//! label the person gave at the start (`jaynes start --task ID --arm NAME`), which is letters, digits and hyphens and
//! so cannot carry a sentence ([`jaynes_core::io::is_label`]). A run started with no task's id is named by a
//! fingerprint of its id and a salt kept on this machine, so lines of one run can be told apart from another's while
//! nobody outside can tell which run it was.
//!
//! The partner reads the file before sending it: the fields are few so that reading it takes a minute.

use crate::presence::Look;
use crate::store::Store;
use jaynes_core::event::Event;
use jaynes_core::io::is_label;
use serde::Serialize;
use std::path::Path;

/// One finished run, as the export says it. Fields in this order, every one on every line.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Numbers {
    /// The partner's id of the task, or the first 12 characters of a fingerprint of the run.
    pub task: String,
    /// The arm of the evaluation, or "none".
    pub arm: String,
    /// Minutes a person was on the screens of the run's stops while they were open, to a tenth of a minute. None: no
    /// presence was recorded, which is not measured, never zero.
    pub presence_minutes: Option<f64>,
    /// Stops a person answered. A rule's answers and an agent's are not theirs.
    pub stops: u32,
    /// Of those, the ones answered with no presence recorded at them: their time is not in the minutes.
    pub stops_unmeasured: u32,
    /// [`RESULTS`]: accepted, corrected, undone or abandoned.
    pub result: &'static str,
    /// Why a person abandoned it, from [`REASONS`]; None when it was not abandoned.
    pub reason: Option<&'static str>,
    /// The agents' working minutes, to a tenth.
    pub agent_minutes: f64,
    pub tokens: u64,
}

/// What became of a finished run: delivered and left as it was, delivered and recorded as wrong (`jaynes correct`),
/// delivered and reverted by the person (`jaynes undo`), or abandoned. Reverted counts over recorded as wrong.
pub const RESULTS: [&str; 4] = ["accepted", "corrected", "undone", "abandoned"];

/// Why a run was abandoned. The record keeps no reason yet: every abandoned run says so.
pub const REASONS: [&str; 1] = ["not recorded"];

/// The numbers of one finished run, from its entries with their times and the spans of a person's presence at its
/// stops. `salt` names a run started with no task's id.
pub fn numbers(events: &[(u64, Event)], looks: &[Look], salt: &str) -> Result<Numbers, String> {
    let Some((_, Event::RunStarted { run, task, arm, .. })) = events.first() else {
        return Err("a run's entries begin with its start".into());
    };
    // the core keeps a label only; a record that holds anything else was not written by it, and nothing of it leaves
    if task.as_deref().is_some_and(|t| !is_label(t)) || arm.as_deref().is_some_and(|a| !is_label(a)) {
        return Err("a run's task or arm in the record is not a label: the record was not written by Jaynes, and nothing of it is exported".into());
    }
    let s = crate::summary::summarize(events, looks, crate::summary::Promises::Unread)?;
    let corrected = events.iter().any(|(_, e)| matches!(e, Event::DeliveryCorrected { .. }));
    let result = if s.abandoned {
        RESULTS[3]
    } else if s.undone {
        RESULTS[2]
    } else if corrected {
        RESULTS[1]
    } else {
        RESULTS[0]
    };
    Ok(Numbers {
        task: task.clone().unwrap_or_else(|| fingerprint(salt, run)),
        arm: arm.clone().unwrap_or_else(|| "none".into()),
        presence_minutes: s.person.seconds.map(minutes),
        stops: s.person.stops,
        stops_unmeasured: s.person.unmeasured,
        result,
        reason: s.abandoned.then_some(REASONS[0]),
        agent_minutes: minutes(s.agent_seconds),
        tokens: s.tokens,
    })
}

/// The name of a run started with no task's id: the start of a fingerprint of the salt and the run's id.
fn fingerprint(salt: &str, run: &str) -> String {
    jaynes_core::Digest::of_bytes(format!("jaynes export task\n{salt}\n{run}").as_bytes()).0.chars().take(12).collect()
}

/// Seconds as minutes, to a tenth.
fn minutes(seconds: u64) -> f64 {
    (seconds as f64 / 6.0).round() / 10.0
}

/// The numbers of every finished run of the record in `home` started at or after `since` (seconds since 1970, UTC),
/// in the order they started; and how many runs were left out because they are not finished yet.
pub fn export(home: &Path, since: Option<u64>) -> Result<(Vec<Numbers>, u32), String> {
    let store = Store::open(&home.join("record.sqlite3"))?;
    let mut runs: Vec<(String, Vec<(u64, Event)>)> = Vec::new();
    let mut at: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (ts, run, e) in store.history()? {
        let i = *at.entry(run.clone()).or_insert_with(|| {
            runs.push((run, Vec::new()));
            runs.len() - 1
        });
        runs[i].1.push((ts, e));
    }
    let salt = crate::install::secret(&home.join("export.salt"))?;
    let (mut out, mut unfinished) = (Vec::new(), 0u32);
    for (run, events) in &runs {
        if since.is_some_and(|from| events.first().is_none_or(|(ts, _)| *ts < from)) {
            continue;
        }
        if !jaynes_core::Run::fold(events.iter().map(|(_, e)| e)).is_some_and(|r| r.finished()) {
            unfinished += 1;
            continue;
        }
        out.push(numbers(events, &crate::presence::looks(home, run)?, &salt)?);
    }
    Ok((out, unfinished))
}

/// The first second of a day given as YYYY-MM-DD, in UTC.
pub fn day(date: &str) -> Result<u64, String> {
    let bad = || format!("--since takes a day as YYYY-MM-DD (UTC), such as 2026-11-16, not {date:?}");
    let parts: Vec<&str> = date.split('-').collect();
    let [y, m, d] = parts.as_slice() else { return Err(bad()) };
    if y.len() != 4 || m.len() != 2 || d.len() != 2 || !date.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        return Err(bad());
    }
    let (Ok(y), Ok(m), Ok(d)) = (y.parse::<i64>(), m.parse::<i64>(), d.parse::<i64>()) else { return Err(bad()) };
    if y < 1970 || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(bad());
    }
    // Howard Hinnant's days_from_civil
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let ts = ((era * 146_097 + doe - 719_468) * 86_400) as u64;
    // a day the month does not have (the 30th of February) comes back as another
    if !crate::summary::utc(ts).starts_with(date) {
        return Err(bad());
    }
    Ok(ts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jaynes_core::event::{Cost, Usage};
    use jaynes_core::manifest::Manifest;
    use jaynes_core::risk::Changed;
    use jaynes_core::question::{Answer, By, Item, Question, QuestionKind, Verdict, VerdictKind};
    use jaynes_core::{Digest, Input};
    use crate::store;

    const PLANTED: &str = "PLANTEDsecret";

    /// A run whose every text is planted: its id, its request, its role's, version's and step's names, its agent's
    /// command, a failure's reasons, a session, a changed file's path, an item's label, an answer's context and change,
    /// the delivery's reference, a correction's note. The agent worked 150 seconds over two attempts, for 4,321 tokens.
    fn planted(task: Option<&str>, arm: Option<&str>) -> Vec<(u64, Event)> {
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "role": format!("{PLANTED}-role"), "version": format!("{PLANTED}-1"), "protocol": 1,
            "steps": [{"name": format!("{PLANTED}-step"), "executor": "agent", "stop": {"verdicts": ["keep", "change"], "holds": true}}],
        }))
        .expect("manifest");
        let start = Input::Start {
            run: format!("{PLANTED}-run"), request: format!("{PLANTED} the request /Users/someone/{PLANTED}/path"), manifest,
            role_fingerprint: Digest("f".into()), agent: format!("command:cat /{PLANTED}/x"), raise: None, attention: Default::default(),
            task: task.map(str::to_string), arm: arm.map(str::to_string), never_merge: false,
            starter: None,
        };
        let mut events = jaynes_core::decide(None, &start).expect("start").events;
        let step = format!("{PLANTED}-step");
        let item = Item { id: format!("{PLANTED}-item"), label: format!("{PLANTED} src/{PLANTED}.rs"), digest: Digest("d".into()) };
        events.push(Event::AttemptFailed { step: step.clone(), attempt: 1, reasons: vec![format!("{PLANTED} failed")], usage: Usage { seconds: 60, tokens: 1_000, cached: 7 }, spent: true });
        events.push(Event::AttemptDelivered {
            step: step.clone(), attempt: 2, fix: 0, output: Digest("o".into()), items: vec![item.clone()],
            changed: vec![Changed::file(format!("src/{PLANTED}.rs"), 3)], usage: Usage { seconds: 90, tokens: 3_321, cached: 0 }, session: Some(format!("{PLANTED}-session")),
            tree: None,
            reply: None,
        });
        let question = Question { rides: Vec::new(),
            id: format!("{PLANTED}-q"), step: step.clone(), round: 0, kind: QuestionKind::Decision, items: vec![item],
            offered: vec![VerdictKind::Keep, VerdictKind::Change], holds: true, about: format!("{PLANTED} about"), choose: false, pass: 0, passes: 0, back: vec![],
        };
        let verdicts = [(format!("{PLANTED}-item"), Verdict::Change { text: format!("{PLANTED} change it") })].into_iter().collect();
        let answer = Answer { back: Default::default(), context: format!("{PLANTED} context"), question: question.id.clone(), shown: question.shown(), verdicts, by: By::Person };
        events.push(Event::QuestionOpened { question });
        events.push(Event::AnswerRecorded { answer, effects: Default::default() });
        events.push(Event::Delivered { digest: Digest("o".into()), reference: format!("jaynes/{PLANTED}-run at {PLANTED}") });
        events.push(Event::RunFinished { outcome: jaynes_core::event::Outcome::Delivered, cost: Cost::default() });
        events.push(Event::DeliveryCorrected { note: format!("{PLANTED} it was wrong"), by: By::Person, step: Some(step) });
        // ten minutes between entries
        events.into_iter().enumerate().map(|(i, e)| (1_000 + 600 * i as u64, e)).collect()
    }

    /// When the run's question opened.
    fn opened(events: &[(u64, Event)]) -> u64 {
        events.iter().find_map(|(ts, e)| matches!(e, Event::QuestionOpened { .. }).then_some(*ts)).expect("asked")
    }

    #[test]
    fn no_word_of_the_record_is_in_a_run_s_numbers() {
        let events = planted(Some("T-17"), Some("with-jaynes"));
        let all = serde_json::to_string(&events.iter().map(|(_, e)| e).collect::<Vec<_>>()).expect("json");
        // the planted text is in the record everywhere a text can be
        assert!(all.matches(PLANTED).count() >= 16, "{all}");
        // 90 seconds on the stop's screen while it was open
        let at = opened(&events);
        let looks = [Look { run: format!("{PLANTED}-run"), question: format!("{PLANTED}-q"), from: at + 10, to: at + 100 }];
        let n = numbers(&events, &looks, "salt").expect("numbers");
        let line = serde_json::to_string(&n).expect("json");
        assert!(!line.contains(PLANTED) && !line.to_lowercase().contains("planted"), "{line}");
        assert_eq!(
            line,
            r#"{"task":"T-17","arm":"with-jaynes","presence_minutes":1.5,"stops":1,"stops_unmeasured":0,"result":"corrected","reason":null,"agent_minutes":2.5,"tokens":4321}"#
        );
    }

    #[test]
    fn a_run_with_no_task_s_id_is_named_by_a_fingerprint_no_one_outside_can_turn_back() {
        let events = planted(None, None);
        let n = numbers(&events, &[], "salt-a").expect("numbers");
        assert_eq!(n.arm, "none");
        assert!(n.task.len() == 12 && n.task.bytes().all(|b| b.is_ascii_hexdigit()), "{}", n.task);
        // the run's id alone does not give it: it takes the machine's salt
        assert_ne!(n.task, numbers(&events, &[], "salt-b").expect("numbers").task);
        assert_ne!(n.task, Digest::of_bytes(format!("{PLANTED}-run").as_bytes()).0[..12].to_string());
        assert_eq!(n.task, numbers(&events, &[], "salt-a").expect("numbers").task);
        // no presence recorded is not measured, never zero
        assert_eq!((n.presence_minutes, n.stops, n.stops_unmeasured), (None, 1, 1));
    }

    #[test]
    fn what_became_of_a_run_is_one_word_of_a_fixed_list() {
        let mut events = planted(Some("T-1"), None);
        assert_eq!(numbers(&events, &[], "s").expect("numbers").result, "corrected");
        events.push((9_000, Event::MergeUndone { note: format!("{PLANTED} undo"), by: By::Person }));
        assert_eq!(numbers(&events, &[], "s").expect("numbers").result, "undone");
        // delivered and left as it was
        let kept: Vec<(u64, Event)> = planted(Some("T-1"), None).into_iter().filter(|(_, e)| !matches!(e, Event::DeliveryCorrected { .. })).collect();
        let n = numbers(&kept, &[], "s").expect("numbers");
        assert_eq!((n.result, n.reason), ("accepted", None));
        // abandoned, with its reason, which the record does not keep yet
        let mut abandoned: Vec<(u64, Event)> = kept.into_iter().filter(|(_, e)| !matches!(e, Event::Delivered { .. } | Event::RunFinished { .. })).collect();
        abandoned.push((9_000, Event::RunFinished { outcome: jaynes_core::event::Outcome::Abandoned, cost: Cost::default() }));
        let n = numbers(&abandoned, &[], "s").expect("numbers");
        assert_eq!((n.result, n.reason), ("abandoned", Some("not recorded")));
        let line = serde_json::to_string(&n).expect("json");
        assert!(line.contains(r#""result":"abandoned","reason":"not recorded""#) && !line.contains(PLANTED), "{line}");
    }

    #[test]
    fn the_export_says_each_finished_run_and_leaves_out_one_not_finished() {
        let home = std::env::temp_dir().join(format!("jaynes-export-unit-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("home");
        let mut record = Store::open(&home.join("record.sqlite3")).expect("store");
        let done = planted(Some("T-2"), None);
        record.append(&format!("{PLANTED}-run"), &done.iter().map(|(_, e)| e.clone()).collect::<Vec<_>>()).expect("append");
        // a second run, under way: the first one's entries before it asked, its start naming another id
        let mut under_way: Vec<Event> = done.into_iter().map(|(_, e)| e).take_while(|e| !matches!(e, Event::QuestionOpened { .. })).collect();
        let mut start = serde_json::to_value(&under_way[0]).expect("json");
        start["run"] = "other".into();
        under_way[0] = serde_json::from_value(start).expect("a start");
        record.append("other", &under_way).expect("append");
        let (lines, unfinished) = export(&home, None).expect("export");
        assert_eq!((lines.len(), unfinished), (1, 1));
        assert_eq!((lines[0].task.as_str(), lines[0].result), ("T-2", "corrected"));
        // a day after the runs began leaves both out, and counts nothing as not finished
        assert_eq!(export(&home, Some(store::now() + 86_400)).map(|(l, u)| (l.len(), u)), Ok((0, 0)));
        let _gone = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_task_or_an_arm_that_is_not_a_label_never_starts_a_run() {
        for bad in ["", "two words", "a/b", "é", "x\ny", &"a".repeat(65), "T-1;", "<b>", "--arm", "-1"] {
            for (task, arm) in [(Some(bad), None), (None, Some(bad))] {
                let manifest: Manifest = serde_json::from_value(serde_json::json!({"role": "r", "version": "1", "protocol": 1, "steps": [{"name": "s", "executor": "agent"}]})).expect("manifest");
                let start = Input::Start {
                    run: "r".into(), request: "x".into(), manifest, role_fingerprint: Digest("f".into()), agent: "a".into(), raise: None, attention: Default::default(),
                    task: task.map(str::to_string), arm: arm.map(str::to_string), never_merge: false,
                    starter: None,
                };
                let refused = jaynes_core::decide(None, &start).expect_err(bad);
                assert!(refused.0.contains("is a label: 1 to 64 letters, digits and hyphens, the first a letter or a digit"), "{refused:?}");
            }
        }
        for good in ["T-1", "PROJ-1234", "with-jaynes", "a", &"a".repeat(64)] {
            assert!(is_label(good), "{good}");
        }
    }

    #[test]
    fn a_day_is_read_strictly() {
        assert_eq!(day("1970-01-01"), Ok(0));
        assert_eq!(day("2026-11-16").map(crate::summary::utc), Ok("2026-11-16 00:00 UTC".to_string()));
        assert_eq!(day("2024-02-29").map(crate::summary::utc), Ok("2024-02-29 00:00 UTC".to_string()));
        for bad in ["2026-02-30", "2025-02-29", "2026-13-01", "2026-1-01", "26-11-16", "2026/11/16", "yesterday", "", "2026-11-16x", "+026-11-16"] {
            assert!(day(bad).is_err(), "{bad}");
        }
    }
}
