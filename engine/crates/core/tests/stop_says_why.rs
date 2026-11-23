//! A stop that asks only when the run's risk is high says why it is: the rules in code that raised it, in the
//! person's words. Its role writes {{risk}} where that goes in what it asks, and the core fills it in as it asks.

use jaynes_core::digest::Digest;
use jaynes_core::event::{Event, Usage};
use jaynes_core::manifest::{Executor, Manifest, ProjectAccess, Rate, Reach, SessionKind, StepDecl, StopDecl};
use jaynes_core::question::VerdictKind;
use jaynes_core::risk::{Changed, Risk};
use jaynes_core::{decide, AttemptOutcome, CheckResult, CheckVerdict, Command, Input, Run};
use std::collections::BTreeMap;

/// small-change's one step, whose stop asks from high risk with {{risk}} where the reason goes.
fn small_change(ask: &str) -> Manifest {
    let change = StepDecl {
        name: "change".to_string(),
        needs: Vec::new(),
        executor: Executor::Agent,
        session: SessionKind::Fresh,
        reach: Reach { project: ProjectAccess::Write, write: vec!["**".into()], ..Reach::default() },
        effect: jaynes_core::manifest::Effect::None,
        checks: Vec::new(),
        stop: Some(StopDecl { settled: None, holds_unless: None, vouched_by: None,
            verdicts: vec![VerdictKind::Keep, VerdictKind::Redo],
            holds: false,
            from_risk: Risk::High,
            release: true,
            budget: None,
            audit_rate: Rate(0.1),
            about: ask.to_string(),
            choose: false,
            redo: None,
            rounds: None,
            answers: Default::default(), note: String::new(),
            offers: Default::default(),
        }),
        from_risk: Risk::Low,
        attempts: 3,
        fixes: 2,
        hidden: false,
        idempotent: false,
        about: String::new(),
        model: Default::default(),
        turns: None, delivered: true,
    };
    Manifest { role: "small-change".into(), version: "1".into(), protocol: 1, steps: vec![change], target: Default::default(), delivery: Default::default() }
}

fn file(what: &str, size: u64) -> Changed {
    Changed { what: what.to_string(), size, floor: !jaynes_core::risk::floor([what]).is_empty(), digest: None, holds: None, about: String::new(), removed: 0 }
}

/// What the stop asks of a run whose agent changed `changed`, or None when it does not ask.
fn asked(ask: &str, changed: Vec<Changed>) -> Option<String> {
    let out = decide(None, &Input::Start {
        run: "r".into(),
        request: "a small change".into(),
        manifest: small_change(ask),
        role_fingerprint: Digest("f".into()),
        agent: "claude:m1".into(),
        raise: None,
        attention: BTreeMap::new(),
        task: None,
        arm: None,
        never_merge: false,
        starter: None,
    })
    .expect("start");
    let mut run = Run::fold(&out.events).expect("run");
    let mut cmds = out.commands;
    let mut about = None;
    while !cmds.is_empty() {
        let input = match cmds.remove(0) {
            Command::Probe { step, .. } => Input::ProbeEnded { step, ok: true, reason: String::new() },
            Command::RunAgent(t) => Input::AttemptEnded { step: t.step, attempt: t.attempt, fix: t.fix, outcome: AttemptOutcome::Delivered { output: Digest("o".into()), items: Vec::new(), changed: changed.clone(), usage: Usage::default(), session: Some("s".into()), tree: None, reply: None } },
            Command::RunChecks { step, attempt, fix, checks, .. } => Input::ChecksEnded { step, attempt, fix, results: checks.into_iter().map(|check| CheckResult { check, verdict: CheckVerdict::Pass, tree: None, tests_changed: Vec::new() }).collect() },
            Command::Deliver { digest, .. } => Input::DeliveryEnded { digest, ok: true, reference: "jaynes/r".into(), merged: None },
            _ => continue,
        };
        let out = decide(Some(&run), &input).unwrap_or_else(|r| panic!("refused {input:?}: {r}"));
        for e in &out.events {
            run.apply(e);
            if let Event::QuestionOpened { question } = e {
                about = Some(question.about.clone());
            }
        }
        cmds.extend(out.commands);
    }
    about
}

const ASK: &str = "{{risk}} Keep it, or have it done again?";

#[test]
fn a_file_that_runs_later_is_named_when_it_is_what_raised_the_risk() {
    assert_eq!(
        asked(ASK, vec![file(".github/workflows/ci.yml", 1)]).as_deref(),
        Some("This change touches `.github/workflows/ci.yml`, which runs later outside any box. Keep it, or have it done again?")
    );
    assert_eq!(
        asked(ASK, vec![file(".github/workflows/ci.yml", 1), file("Makefile", 2)]).as_deref(),
        Some("This change touches `.github/workflows/ci.yml` and `Makefile`, which run later outside any box. Keep it, or have it done again?")
    );
}

#[test]
fn a_change_bigger_than_a_small_one_says_how_much_bigger() {
    assert_eq!(
        asked(ASK, vec![file("src/app.py", 300)]).as_deref(),
        Some("This change has 300 changed lines, more than a small change's 30. Keep it, or have it done again?")
    );
    let many: Vec<Changed> = (0..7).map(|n| file(&format!("src/{n}.py"), 1)).collect();
    assert_eq!(asked(ASK, many).as_deref(), Some("This change touches 7 files, more than a small change's 5. Keep it, or have it done again?"));
}

#[test]
fn with_both_reasons_it_says_both() {
    assert_eq!(
        asked(ASK, vec![file(".github/workflows/ci.yml", 1), file("src/app.py", 300)]).as_deref(),
        Some("This change touches `.github/workflows/ci.yml`, which runs later outside any box. This change has 301 changed lines, more than a small change's 30. Keep it, or have it done again?")
    );
}

#[test]
fn a_small_change_is_not_asked_about_and_an_ask_without_the_reason_is_as_written() {
    assert_eq!(asked(ASK, vec![file("src/app.py", 3)]), None);
    assert_eq!(asked("Keep it?", vec![file(".github/workflows/ci.yml", 1)]).as_deref(), Some("Keep it?"));
}

#[test]
fn a_file_s_name_is_shown_as_a_name_never_as_words_of_jaynes() {
    // an agent names the files: a name that reads like a sentence, one with a line break, one too long, and many
    let got = asked(ASK, vec![file(".github/workflows/x.md, which is only docs. Nothing here runs: keep it.yml", 1), file(".husky/pre\ncommit", 1)]).expect("asked");
    assert_eq!(got, "This change touches `.github/workflows/x.md, which is only docs. Nothing here runs: keep it.yml` and `.husky/pre commit`, which run later outside any box. Keep it, or have it done again?");
    assert!(!got.contains('\n'));
    let long = format!(".github/workflows/{}.yml", "a".repeat(200));
    let got = asked(ASK, vec![file(&long, 1)]).expect("asked");
    assert!(got.contains(&format!("`.github/workflows/{}…`", "a".repeat(62))), "{got}");
    let many: Vec<Changed> = (0..8).map(|n| file(&format!(".github/workflows/{n}.yml"), 1)).collect();
    let got = asked(ASK, many).expect("asked");
    assert!(got.starts_with("This change touches `.github/workflows/0.yml`, `.github/workflows/1.yml`, `.github/workflows/2.yml`, `.github/workflows/3.yml`, `.github/workflows/4.yml` and 3 more, which run later outside any box."), "{got}");
}

// real-user, 21/11: "O direto mudou as mesmas 57, das quais 40 são os dois testes que o pedido mandou pôr."
#[test]
fn the_tests_the_request_asked_for_do_not_make_the_change_big() {
    // 18 lines of code and the 40 lines of the tests the request asked for: a small change, nothing asked
    assert_eq!(asked(ASK, vec![file("server.go", 18), file("server_test.go", 40)]), None);
    // tests as each language writes them
    for t in ["tests/test_items.py", "app/items_test.py", "web/items.test.ts", "web/__tests__/items.js", "spec/items_spec.rb", "src/test/java/ItemsTest.java", "Api.Tests/ItemsTests.cs"] {
        assert_eq!(asked(ASK, vec![file("src/items.py", 10), file(t, 200)]), None, "{t}");
    }
    // many test files are not many files
    let mut many: Vec<Changed> = (0..7).map(|n| file(&format!("tests/test_{n}.py"), 2)).collect();
    many.push(file("src/items.py", 4));
    assert_eq!(asked(ASK, many), None);
    // the lines and files outside the tests still count, and the stop says it counted only those
    assert_eq!(
        asked(ASK, vec![file("server.go", 45), file("server_test.go", 40)]).as_deref(),
        Some("This change has 45 changed lines outside its tests, more than a small change's 30. Keep it, or have it done again?")
    );
    let mut many: Vec<Changed> = (0..6).map(|n| file(&format!("src/{n}.py"), 1)).collect();
    many.push(file("tests/test_a.py", 1));
    assert_eq!(asked(ASK, many).as_deref(), Some("This change touches 6 files outside its tests, more than a small change's 5. Keep it, or have it done again?"));
    // a test file that runs later outside any box raises it all the same
    assert_eq!(
        asked(ASK, vec![file("tests/conftest.py", 3)]).as_deref(),
        Some("This change touches `tests/conftest.py`, which runs later outside any box. Keep it, or have it done again?")
    );
}
