//! The person's leave to change how the project's tests are defined (T1, the real-user's estoque, three times): read
//! from their own words, recorded with them, and only theirs. A check that compares the suite with the one the run
//! began with then judges it as the agent left it; without the leave nothing changes.

use jaynes_core::digest::Digest;
use jaynes_core::event::Event;
use jaynes_core::manifest::Manifest;
use jaynes_core::question::By;
use jaynes_core::{decide, Input, Run};

const WORDS: &str = "pode mudar o teste, usa uma porta livre pro banco falso";

fn started() -> Run {
    let manifest: Manifest = serde_json::from_value(serde_json::json!({
        "role": "small-change", "version": "1", "protocol": 1,
        "steps": [{"name": "change", "executor": "agent"}],
    }))
    .expect("manifest");
    let start = Input::Start {
        run: "r1".into(), request: "ping pelo ESTOQUE_DB".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(),
        raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None,
    };
    Run::fold(&decide(None, &start).expect("start").events).expect("run")
}

#[test]
fn asked_only_the_person_lets_a_run_change_how_its_tests_are_defined_and_their_words_are_kept() {
    let mut run = started();
    assert_eq!(run.tests_leave, None, "no leave until the person gives it");
    let agent = decide(Some(&run), &Input::LeaveTests { words: WORDS.into(), by: By::Agent { who: "claude".into() } });
    assert!(agent.is_err_and(|r| r.0.contains("only you")), "an agent never lets itself change the tests");
    assert!(decide(Some(&run), &Input::LeaveTests { words: "  ".into(), by: By::Person }).is_err(), "a leave is given in words");
    let out = decide(Some(&run), &Input::LeaveTests { words: WORDS.into(), by: By::Person }).expect("the person's");
    assert_eq!(out.events, [Event::TestsLeft { words: WORDS.into(), by: By::Person }]);
    assert!(out.commands.is_empty(), "it redoes nothing by itself");
    for e in &out.events {
        run.apply(e);
    }
    assert_eq!(run.tests_leave.as_deref(), Some(WORDS));
    assert!(decide(Some(&run), &Input::LeaveTests { words: WORDS.into(), by: By::Person }).is_err(), "given once");
}

#[test]
fn asked_the_files_a_check_judged_with_the_leave_are_the_run_s_and_the_merge_says_so() {
    use jaynes_core::event::CheckRecord;
    use jaynes_core::manifest::CheckKind;
    use jaynes_core::question::{Answer, Question, QuestionKind, VerdictKind};
    let mut run = started();
    assert!(run.tests_changed().is_empty());
    let record = |passed: bool| CheckRecord {
        check: "suite-passes".into(), kind: CheckKind::Open, passed, blocked: false, read: vec![], tree: None,
        tests_changed: if passed { vec!["tests/conftest.py".into()] } else { vec![] },
    };
    run.steps[0].checks = vec![record(true)];
    assert_eq!(run.tests_changed(), ["tests/conftest.py"]);
    let merge = Question {
        id: "merge-1".into(), step: "change".into(), round: 0, kind: QuestionKind::Merge, items: vec![], offered: vec![VerdictKind::Keep, VerdictKind::Drop],
        holds: true, about: String::new(), choose: false, pass: 0, passes: 0, back: vec![], rides: vec![],
    };
    let keep = Answer { back: Default::default(), question: merge.id.clone(), shown: merge.shown(), verdicts: Default::default(), by: By::Person, context: String::new() };
    let says = jaynes_core::effects(&run, &merge, &keep).sentences;
    assert_eq!(says, ["The delivery merges into your branch.", "Tests changed with your leave: tests/conftest.py."]);
    run.steps[0].checks = vec![record(false)];
    assert!(run.tests_changed().is_empty(), "only what a check that passed judged");
}
