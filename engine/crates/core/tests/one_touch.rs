//! The person is asked once, at the merge, never once per file the agent writes: a write inside what the step may
//! write is answered by a rule, and every merge into the person's branch waits for them, showing what lands.

use jaynes_core::decide::{routine, WRITE_THE_STEP_MAY_WRITE};
use jaynes_core::event::Usage;
use jaynes_core::manifest::{DeliveryKind, Effect, Executor, Manifest, ProjectAccess, Reach, SessionKind, StepDecl};
use jaynes_core::question::{Answer, Ask, AskKind, By, QuestionKind, Verdict};
use jaynes_core::risk::Changed;
use jaynes_core::run::Run;
use jaynes_core::{decide, AttemptOutcome, Command, Digest, Input};
use std::collections::BTreeMap;

const SEVEN: [&str; 7] = [
    "tests/conftest.py",
    "tests/fakes.py",
    "tests/test_postgres.py",
    "tests/test_qdrant.py",
    "tests/test_embeddings_fake.py",
    "tests/test_health.py",
    "tests/test_skeleton.py",
];

fn writes_everything() -> Reach {
    Reach { project: ProjectAccess::Write, write: vec!["**".to_string()], ..Reach::default() }
}

fn build() -> StepDecl {
    StepDecl {
        name: "build".to_string(),
        needs: Vec::new(),
        executor: Executor::Agent,
        session: SessionKind::Fresh,
        reach: writes_everything(),
        effect: Effect::None,
        checks: Vec::new(),
        stop: None,
        from_risk: Default::default(),
        attempts: 3,
        fixes: 2,
        hidden: false,
        idempotent: false,
        about: String::new(),
        model: Default::default(),
        turns: None, delivered: true,
    }
}

fn feed(run: &mut Run, input: Input) -> Vec<Command> {
    let out = decide(Some(run), &input).unwrap_or_else(|r| panic!("refused {input:?}: {r}"));
    for e in &out.events {
        run.apply(e);
    }
    out.commands
}

/// A scaffold run of one agent step that may write the whole project, delivered by merge, at its first turn.
fn scaffold() -> (Run, u32) {
    let manifest = Manifest { role: "build".to_string(), version: "1".to_string(), protocol: 1, steps: vec![build()], target: Default::default(), delivery: DeliveryKind::Merge };
    let out = decide(None, &Input::Start { run: "r".to_string(), request: "scaffold it".to_string(), manifest, role_fingerprint: Digest("f".to_string()), agent: "claude:m1".to_string(), raise: None, attention: BTreeMap::new(), task: None, arm: None, never_merge: false, starter: None }).expect("start");
    let mut run = Run::fold(&out.events).expect("run");
    let mut c = Vec::new();
    for cmd in out.commands {
        match cmd {
            Command::Probe { step, .. } => c.extend(feed(&mut run, Input::ProbeEnded { step, ok: true, reason: String::new() })),
            other => c.push(other),
        }
    }
    let attempt = c.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.attempt) } else { None }).expect("a turn");
    (run, attempt)
}

fn asked(c: &[Command]) -> Vec<QuestionKind> {
    c.iter().filter_map(|c| if let Command::Ask { question } = c { Some(question.kind) } else { None }).collect()
}

fn delivered_merge(c: &[Command]) -> Option<bool> {
    c.iter().find_map(|c| if let Command::Deliver { merge, .. } = c { Some(*merge) } else { None })
}

fn ends(run: &mut Run, attempt: u32, changed: Vec<Changed>) -> Vec<Command> {
    let outcome = AttemptOutcome::Delivered { output: Digest("o".to_string()), items: Vec::new(), changed, usage: Usage::default(), session: None, tree: None, reply: None };
    feed(run, Input::AttemptEnded { step: "build".to_string(), attempt, fix: 0, outcome })
}

fn file(path: &str, added: u64, removed: u64, content: &str) -> Changed {
    let mut c = Changed::file(path.to_string(), added + removed);
    c.removed = removed;
    c.digest = Some(content.to_string());
    c
}

// Alan, 21/11: "nem li nada pq não dá pra entender nada só dei allow"
#[test]
fn asked_no_allow_i_cannot_read() {
    // a write inside what the step may write is not a stop: a rule answers it, files that run later outside any box
    // included, since the merge asks about those
    let (mut run, attempt) = scaffold();
    for path in SEVEN {
        let c = feed(&mut run, Input::PermissionAsked { step: "build".to_string(), attempt, ask: Ask { kind: AskKind::Write, target: path.to_string() } });
        assert!(c.iter().any(|c| matches!(c, Command::Permit { allow: true, .. })), "{path}: {c:?}");
        assert_eq!(asked(&c), [], "{path} asked the person");
    }
    assert!(run.open_questions().is_empty(), "a leave waits for the person");
    assert!(run.questions.values().all(|q| q.answer.as_ref().map(|a| &a.by) == Some(&By::Rule { rule: WRITE_THE_STEP_MAY_WRITE.to_string() })));
    // outside what the step may write, outside the project, in a step that only reads, or in the agent's own
    // settings: still the person's
    let tests_only = Reach { project: ProjectAccess::Write, write: vec!["tests/**".to_string()], ..Reach::default() };
    let write = |target: &str| Ask { kind: AskKind::Write, target: target.to_string() };
    assert_eq!(routine(&tests_only, &write("tests/conftest.py")), Some(WRITE_THE_STEP_MAY_WRITE));
    assert_eq!(routine(&tests_only, &write("src/main.py")), None);
    let kept_out = Reach { write: vec!["**".to_string(), "!secrets/**".to_string()], ..writes_everything() };
    assert_eq!(routine(&kept_out, &write("secrets/key.pem")), None);
    for outside in ["/etc/hosts", "~/.zshrc", "../other/x.py", "tests/../../x.py", ""] {
        assert_eq!(routine(&writes_everything(), &write(outside)), None, "{outside}");
    }
    let reads = Reach { project: ProjectAccess::Read, write: vec!["**".to_string()], ..Reach::default() };
    assert_eq!(routine(&reads, &write("tests/conftest.py")), None, "a step that only reads");
    // at any depth too: a nested settings file holds when the agent runs in that folder
    for own in [".claude/settings.json", ".claude/settings.local.json", ".mcp.json", ".codex/config.toml", ".CLAUDE/settings.json", "sub/.claude/settings.json", "pkg/.mcp.json", "a/b/.cursor/rules", "svc/.gemini/settings.json"] {
        assert_eq!(routine(&writes_everything(), &write(own)), None, "{own}: the agent's own settings");
    }
}

// Alan, 21/11: "tive que dar uma porrada de allow com o meu touchid"
#[test]
fn asked_one_touch_not_a_touch_per_allow() {
    // a scaffold run that writes seven files in tests/ asks the person nothing while it writes them
    let (mut run, attempt) = scaffold();
    for path in SEVEN {
        let c = feed(&mut run, Input::PermissionAsked { step: "build".to_string(), attempt, ask: Ask { kind: AskKind::Write, target: path.to_string() } });
        assert_eq!(asked(&c), [], "{path} asked the person");
    }
    // and once, before the merge: one question showing every file that lands, each bound to its content, the one
    // that runs later outside any box among them
    let changed: Vec<Changed> = SEVEN.iter().enumerate().map(|(k, p)| file(p, 10 + k as u64, k as u64, &format!("c{k}"))).collect();
    let c = ends(&mut run, attempt, changed);
    assert_eq!(delivered_merge(&c), None, "the merge went out unasked: {c:?}");
    assert_eq!(asked(&c), [QuestionKind::Merge]);
    let q = run.open_questions().into_iter().map(|q| q.question.clone()).next().expect("the merge waits");
    assert_eq!(q.items.iter().map(|x| (x.id.as_str(), x.digest.0.as_str())).collect::<Vec<_>>(), SEVEN.iter().enumerate().map(|(k, p)| (*p, format!("c{k}"))).collect::<Vec<_>>().iter().map(|(p, d)| (*p, d.as_str())).collect::<Vec<_>>());
    assert!(q.holds);
    // one answer, the person's, merges it all
    let keep = Answer { back: Default::default(), question: q.id.clone(), shown: q.shown(), verdicts: q.items.iter().map(|x| (x.id.clone(), Verdict::Keep)).collect(), by: By::Person, context: String::new() };
    let c = feed(&mut run, Input::Answer { answer: keep });
    assert_eq!(delivered_merge(&c), Some(true));
    assert_eq!(run.questions.values().filter(|q| q.answer.as_ref().is_some_and(|a| a.by.person())).count(), 1, "the person answered once");
}

#[test]
fn every_merge_waits_for_the_person_even_with_nothing_that_runs_later() {
    // nothing on the floor: the run still waits before the merge, showing what lands, and only the person answers
    let (mut run, attempt) = scaffold();
    let c = ends(&mut run, attempt, vec![file("src/app.py", 64, 12, "a"), file("src/app_test.py", 88, 0, "b")]);
    assert_eq!(delivered_merge(&c), None, "the merge went out unasked: {c:?}");
    let q = run.open_questions().into_iter().map(|q| q.question.clone()).next().expect("the merge waits");
    assert_eq!(q.kind, QuestionKind::Merge);
    assert_eq!(q.items.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["src/app.py", "src/app_test.py"]);
    assert!(q.about.contains("lands on your branch"), "{}", q.about);
    for by in [By::Agent { who: "claude".to_string() }, By::Rule { rule: WRITE_THE_STEP_MAY_WRITE.to_string() }] {
        let a = Answer { back: Default::default(), question: q.id.clone(), shown: q.shown(), verdicts: q.items.iter().map(|x| (x.id.clone(), Verdict::Keep)).collect(), by, context: String::new() };
        assert!(decide(Some(&run), &Input::Answer { answer: a }).is_err(), "only the person answers a merge");
    }
    let drop = Answer { back: Default::default(), question: q.id.clone(), shown: q.shown(), verdicts: q.items.iter().map(|x| (x.id.clone(), Verdict::Drop)).collect(), by: By::Person, context: String::new() };
    assert_eq!(delivered_merge(&feed(&mut run, Input::Answer { answer: drop })), Some(false), "branch only");
    // a run that delivers a branch never asks
    let manifest = Manifest { delivery: DeliveryKind::Branch, ..run.manifest.clone() };
    let out = decide(None, &Input::Start { run: "b".to_string(), request: "x".to_string(), manifest, role_fingerprint: Digest("f".to_string()), agent: "claude:m1".to_string(), raise: None, attention: BTreeMap::new(), task: None, arm: None, never_merge: false, starter: None }).expect("start");
    let mut branch = Run::fold(&out.events).expect("run");
    let mut c = Vec::new();
    for cmd in out.commands {
        if let Command::Probe { step, .. } = cmd {
            c.extend(feed(&mut branch, Input::ProbeEnded { step, ok: true, reason: String::new() }));
        }
    }
    let attempt = c.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.attempt) } else { None }).expect("a turn");
    assert_eq!(delivered_merge(&ends(&mut branch, attempt, vec![file("src/app.py", 1, 0, "a")])), Some(false));
}
