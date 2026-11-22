//! The person is asked once, at the merge, never once per file the agent writes: a write inside what the step may
//! write is answered by a rule, and every merge into the person's branch waits for them, showing what lands.

use jaynes_core::decide::{routine, WRITE_THE_STEP_MAY_WRITE};
use jaynes_core::event::Usage;
use jaynes_core::manifest::{DeliveryKind, Effect, Executor, Manifest, ProjectAccess, Reach, SessionKind, StepDecl};
use jaynes_core::question::{Answer, Ask, AskKind, By, QuestionKind, Ride, Verdict};
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
        turns: None,
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

/// A fix-bug run whose reproduce stop does not hold: the bug as the request says it goes on, and the stop stays open.
fn reproduce_then_fix(starter: Option<String>, holds: bool) -> (Run, Vec<Command>) {
    use jaynes_core::manifest::StopDecl;
    use jaynes_core::question::VerdictKind;
    let stop = StopDecl {
        verdicts: vec![VerdictKind::Keep, VerdictKind::Redo],
        holds,
        from_risk: Default::default(),
        release: false,
        budget: None,
        audit_rate: jaynes_core::manifest::Rate(0.1),
        about: "Is this the bug?".to_string(),
        choose: false,
        redo: None,
        rounds: None,
        answers: [(VerdictKind::Redo, "Not this bug".to_string())].into_iter().collect(),
        settled: Some("the bug".to_string()),
        note: String::new(),
    };
    let reproduce = StepDecl { name: "reproduce".to_string(), stop: Some(stop), ..build() };
    let fix = StepDecl { name: "fix".to_string(), needs: vec!["reproduce".to_string()], ..build() };
    let manifest = Manifest { role: "fix-bug".to_string(), version: "1".to_string(), protocol: 1, steps: vec![reproduce, fix], target: Default::default(), delivery: DeliveryKind::Merge };
    let out = decide(None, &Input::Start { run: "r".to_string(), request: "the comma breaks the CSV".to_string(), manifest, role_fingerprint: Digest("f".to_string()), agent: "claude:m1".to_string(), raise: None, attention: BTreeMap::new(), task: None, arm: None, never_merge: false, starter }).expect("start");
    let mut run = Run::fold(&out.events).expect("run");
    let mut todo = out.commands;
    let mut seen = Vec::new();
    while let Some(cmd) = (!todo.is_empty()).then(|| todo.remove(0)) {
        match &cmd {
            Command::Probe { step, .. } => todo.extend(feed(&mut run, Input::ProbeEnded { step: step.clone(), ok: true, reason: String::new() })),
            Command::RunAgent(t) => {
                let (items, changed) = if t.step == "reproduce" {
                    (vec![jaynes_core::question::Item { id: "bug".to_string(), label: "A note with a comma breaks the CSV".to_string(), digest: Digest("b".to_string()) }], vec![file("tests/test_comma.py", 9, 0, "t")])
                } else {
                    (Vec::new(), vec![file("src/export.py", 3, 1, "e")])
                };
                let outcome = AttemptOutcome::Delivered { output: Digest(format!("o-{}", t.step)), items, changed, usage: Usage::default(), session: None, tree: None, reply: None };
                todo.extend(feed(&mut run, Input::AttemptEnded { step: t.step.clone(), attempt: t.attempt, fix: 0, outcome }));
            }
            _ => {}
        }
        seen.push(cmd);
    }
    (run, seen)
}

// The tech-lead, 22/11: the cut in cost leaves the person one moment, the merge, with one touch. A stop the request
// already answers does not hold the run; the merge's card lists it, and the touch that merges keeps it, by the person,
// with its own answer in the record.
#[test]
fn asked_one_moment_at_the_merge() {
    let (mut run, seen) = reproduce_then_fix(None, false);
    // the run went on past the stop that does not hold: the fix was made, and the person was asked only the merge
    assert!(seen.iter().any(|c| matches!(c, Command::RunAgent(t) if t.step == "fix")), "{seen:?}");
    assert_eq!(asked(&seen), [QuestionKind::Decision, QuestionKind::Merge]);
    let open = run.open_questions().into_iter().map(|q| q.question.clone()).collect::<Vec<_>>();
    let bug = open.iter().find(|q| q.step == "reproduce").expect("the stop is still open").clone();
    assert!(!bug.holds);
    let merge = open.iter().find(|q| q.kind == QuestionKind::Merge).expect("the merge waits").clone();
    // the merge's question names each open stop its touch keeps, with what it holds now in Jaynes's words
    assert_eq!(merge.rides, vec![Ride { question: bug.id.clone(), step: "reproduce".to_string(), said: "the bug, as your request says".to_string() }]);
    // one touch, the person's: the stop is kept first, by the person, then the merge goes out
    let keep = Answer { back: Default::default(), question: merge.id.clone(), shown: merge.shown(), verdicts: merge.items.iter().map(|x| (x.id.clone(), Verdict::Keep)).collect(), by: By::Person, context: String::new() };
    let out = decide(Some(&run), &Input::Answer { answer: keep }).expect("the touch");
    let answered: Vec<(&str, &By)> = out.events.iter().filter_map(|e| if let jaynes_core::Event::AnswerRecorded { answer, .. } = e { Some((answer.question.as_str(), &answer.by)) } else { None }).collect();
    assert_eq!(answered, [(bug.id.as_str(), &By::Person), (merge.id.as_str(), &By::Person)]);
    for e in &out.events {
        run.apply(e);
    }
    let kept = run.questions[&bug.id].answer.clone().expect("kept");
    assert_eq!((kept.shown, kept.verdicts.get("bug")), (bug.shown(), Some(&Verdict::Keep)));
    assert_eq!(delivered_merge(&out.commands), Some(true), "{:?}", out.commands);
    // nothing ran again: keeping it as it is sends nothing back
    assert!(!out.commands.iter().any(|c| matches!(c, Command::RunAgent(_))), "{:?}", out.commands);
}

#[test]
fn branch_only_keeps_nothing_and_a_stop_that_holds_is_never_in_the_merge() {
    // the other way out of the merge answers nothing else: the stop stays open for the person
    let (mut run, _) = reproduce_then_fix(None, false);
    let merge = run.open_questions().into_iter().map(|q| q.question.clone()).find(|q| q.kind == QuestionKind::Merge).expect("merge");
    let drop = Answer { back: Default::default(), question: merge.id.clone(), shown: merge.shown(), verdicts: merge.items.iter().map(|x| (x.id.clone(), Verdict::Drop)).collect(), by: By::Person, context: String::new() };
    assert_eq!(delivered_merge(&feed(&mut run, Input::Answer { answer: drop })), Some(false));
    assert!(run.open_questions().iter().any(|q| q.question.step == "reproduce"), "branch only keeps nothing");
    // a run an agent started says whose request it was
    let (run, _) = reproduce_then_fix(Some("4242@1".to_string()), false);
    let merge = run.open_questions().into_iter().map(|q| q.question.clone()).find(|q| q.kind == QuestionKind::Merge).expect("merge");
    assert_eq!(merge.rides[0].said, "the bug, as the request the agent that started the run wrote says");
    // a stop that holds is answered on its own before anything goes on: never one the merge's touch keeps
    let (run, seen) = reproduce_then_fix(None, true);
    assert!(!seen.iter().any(|c| matches!(c, Command::RunAgent(t) if t.step == "fix")), "the stop held: {seen:?}");
    assert!(run.open_questions().iter().all(|q| q.question.kind != QuestionKind::Merge && q.question.rides.is_empty()));
}
