//! A stop as the Jaynes app shows it: the question that waits for the person, each item with what it shows, the
//! verdicts the person may give, what lies ahead, and when the app asks for the person's touch before the answer
//! goes: before an answer that leads to a step that cannot be undone, and once at the merge into their branch, never
//! at a leave. What an answer will do is still written by the core (`drive::effects`).

use jaynes_core::question::{Answer, By, QuestionKind, Verdict, VerdictKind};
use jaynes_core::run::Run;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub label: String,
    pub detail: String,
    /// The parts the person judges apart, each with its label.
    pub aspects: Vec<(String, String)>,
    /// Pictures of the item, as files.
    pub pictures: Vec<PathBuf>,
    /// The page the role kept of it, by name, served sandboxed by the page at `/page/<name>`.
    pub page: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stop {
    pub run: String,
    pub question: String,
    /// The fingerprint of the question as shown: an answer counts only for what the person saw (G10).
    pub shown: String,
    pub title: String,
    /// The role's question at this stop ("Which stack goes on?"), empty when it has none. With one option to choose
    /// the title asks whether to go with it, and this goes on the line under it.
    pub asks: String,
    pub step: String,
    pub permission: bool,
    /// The question's kind, for the names of its verdicts ([`label_for`]).
    pub kind: QuestionKind,
    pub choose: bool,
    pub offered: Vec<VerdictKind>,
    pub items: Vec<Item>,
    /// What the person dropped at this stop in earlier rounds: only they bring it back.
    pub back: Vec<Item>,
    /// The round of asking at this stop, from 1, when the role counts rounds.
    pub round: u32,
    /// What the stop's step does, as its role says it.
    pub about: String,
    /// What lies ahead of this stop, in a sentence, if anything does: a step that cannot be undone, or the merge into
    /// the person's branch, which asks them before it lands. Never at a leave, nor at the merge itself.
    pub ahead: Option<String>,
    /// Why macOS asks for the person's touch before this answer goes, when it does: an answer that leads to a step
    /// that cannot be undone, and the merge into their branch, once. Never a leave, nor a decision before a merge.
    pub touch: Option<String>,
    /// The branch the run merges into, at the stop before the merge; empty elsewhere, or when the copy does not say.
    pub into: String,
    /// What the merge lands, at the stop before it: every file with its lines, what runs later outside any box said.
    pub lands: Vec<Lands>,
}

/// A file the merge lands on the person's branch, as the stop before it shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Lands {
    pub path: String,
    pub added: u64,
    pub removed: u64,
    /// How it runs later outside any box, in plain words, when it does (G19: the merge names it with this).
    pub runs: Option<&'static str>,
}

/// The stop a run waits at: its first open question.
pub fn waiting(home: &Path, run: &Run) -> Option<Stop> {
    if run.finished() {
        return None;
    }
    let q = run.questions.values().find(|q| q.open())?;
    let store = crate::store::Store::open(&home.join("record.sqlite3")).ok();
    let shown = |it: &jaynes_core::question::Item| {
        let d = store.as_ref().map(|s| s.display(&it.digest)).unwrap_or_default();
        let aspects = d["aspects"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|a| (a["label"].as_str().unwrap_or("").to_string(), a["text"].as_str().unwrap_or("").to_string()))
            .collect();
        let names: Vec<String> = d["pictures"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p.as_str())
            .filter(|name| !name.contains('/') && !name.starts_with('.'))
            .map(str::to_string)
            .collect();
        let page = names.iter().find_map(|n| d["pages"][n.as_str()].as_str()).filter(|p| kept_page(p)).map(str::to_string);
        let pictures = names.iter().map(|name| home.join("pictures").join(name)).filter(|p| p.exists()).collect();
        Item { id: it.id.clone(), label: it.label.clone(), detail: d["detail"].as_str().unwrap_or("").to_string(), aspects, pictures, page }
    };
    let items = q.question.items.iter().map(&shown).collect();
    let back = q.question.back.iter().map(&shown).collect();
    let about = run.manifest.index(&q.question.step).map(|i| run.manifest.steps[i].about.clone()).unwrap_or_default();
    let merge = q.question.kind == QuestionKind::Merge;
    let into = if merge { crate::project::start_branch_of(&home.join("runs").join(&run.id).join("project")) } else { String::new() };
    let name = crate::listing::short(&run.request.split_whitespace().collect::<Vec<_>>().join(" "));
    let (ahead, touch) = match q.question.kind {
        QuestionKind::Permission => (None, None),
        QuestionKind::Merge => (None, Some(format!("merge “{name}” into {}", if into.is_empty() { "your branch" } else { &into }))),
        _ => match ahead_of(run, &q.question.step) {
            Some((a, true)) => (Some(a.clone()), Some(format!("send your answer to Jaynes: {a}"))),
            Some((a, false)) => (Some(a), None),
            None => (None, None),
        },
    };
    let lands = if merge {
        run.delivered_files()
            .into_iter()
            .filter(|(c, _)| q.question.items.iter().any(|x| x.id == c.what))
            .map(|(c, _)| Lands {
                added: c.size - c.removed.min(c.size),
                removed: c.removed.min(c.size),
                runs: jaynes_core::risk::holds_merge(run.manifest.target, &c).then(|| jaynes_core::risk::runs_how(&c.what)),
                path: c.what,
            })
            .collect()
    } else {
        Vec::new()
    };
    Some(Stop {
        run: run.id.clone(),
        question: q.question.id.clone(),
        shown: q.question.shown().0,
        title: if merge { format!("Merge into “{}”?", if into.is_empty() { "your branch" } else { &into }) } else { crate::listing::title(q) },
        asks: if merge { format!("“{name}” is done and checked. This is what lands on your branch.") } else { q.question.about.clone() },
        step: q.question.step.clone(),
        permission: q.question.kind == QuestionKind::Permission,
        kind: q.question.kind,
        choose: q.question.choose,
        offered: q.question.offered.clone(),
        items,
        back,
        round: q.question.pass,
        about,
        ahead,
        touch,
        into,
        lands,
    })
}

/// A page the role kept, named by its fingerprint: nothing else is served.
fn kept_page(name: &str) -> bool {
    // by bytes: the name comes from what a role kept, and need not cut at a letter's edge
    let b = name.as_bytes();
    b.len() == 69 && b[..64].iter().all(u8::is_ascii_hexdigit) && &b[64..] == b".html"
}

/// The answer the person's marks make: a verdict per item, with their words for a change or a redo.
pub fn answer(stop: &Stop, marks: &BTreeMap<String, (VerdictKind, String)>, context: &str) -> Answer {
    let verdicts = marks
        .iter()
        .map(|(id, (kind, words))| {
            let v = match kind {
                VerdictKind::Keep => Verdict::Keep,
                VerdictKind::Skip => Verdict::Skip,
                VerdictKind::Drop => Verdict::Drop,
                VerdictKind::Change => Verdict::Change { text: words.clone() },
                VerdictKind::Redo => Verdict::Redo { note: words.clone() },
            };
            (id.clone(), v)
        })
        .collect();
    Answer {
        question: stop.question.clone(),
        shown: jaynes_core::Digest(stop.shown.clone()),
        verdicts,
        // who answered is the engine's to say, once the person's key has signed it
        by: By::Agent { who: "the Jaynes app, unsigned".into() },
        context: context.to_string(),
        back: BTreeMap::new(),
    }
}

/// What lies ahead of `step`: a step whose effect cannot be undone, or a merge into the person's branch with no
/// other stop of theirs before it.
pub fn ahead(run: &Run, step: &str) -> Option<String> {
    ahead_of(run, step).map(|(a, _)| a)
}

/// What lies ahead of `step`, and whether an answer that leads there asks for the person's touch: a step that cannot
/// be undone does; the merge does not, since it asks the person itself before it lands, with their touch once.
fn ahead_of(run: &Run, step: &str) -> Option<(String, bool)> {
    let from = run.manifest.index(step)?;
    let path = run.path();
    let later: Vec<usize> = path.into_iter().filter(|&j| j > from).collect();
    if let Some(&j) = later.iter().find(|&&j| run.manifest.steps[j].effect.irreversible()) {
        return Some((format!("“{}” cannot be undone", run.manifest.steps[j].name), true));
    }
    let another_stop = later.iter().any(|&j| run.manifest.steps[j].stop.is_some() || run.manifest.steps[j].executor == jaynes_core::manifest::Executor::Person);
    (run.manifest.lands() && !another_stop && !run.never_merge).then(|| ("the run merges into your branch".to_string(), false))
}

/// How a verdict is offered, in a word.
pub fn label(v: VerdictKind, permission: bool) -> &'static str {
    label_for(v, if permission { QuestionKind::Permission } else { QuestionKind::Decision })
}

/// A verdict's name as the person reads it on a question of `kind`: a leave is allowed or refused, and a merge that
/// brings in what runs later (G19) is merged, or the change is taken as a branch only.
pub fn label_for(v: VerdictKind, kind: QuestionKind) -> &'static str {
    match (v, kind) {
        (VerdictKind::Keep, QuestionKind::Permission) => "Allow",
        (VerdictKind::Drop, QuestionKind::Permission) => "Refuse",
        (VerdictKind::Keep, QuestionKind::Merge) => "Merge",
        (VerdictKind::Drop, QuestionKind::Merge) => "Branch only",
        (VerdictKind::Keep, _) => "Keep",
        (VerdictKind::Change, _) => "Change",
        (VerdictKind::Redo, _) => "Redo",
        (VerdictKind::Drop, _) => "Drop",
        (VerdictKind::Skip, _) => "I don't know",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_merge_s_verdicts_say_merge_or_a_branch_only_and_a_leave_s_allow_or_refuse() {
        assert_eq!((label_for(VerdictKind::Keep, QuestionKind::Merge), label_for(VerdictKind::Drop, QuestionKind::Merge)), ("Merge", "Branch only"));
        assert_eq!((label(VerdictKind::Keep, true), label(VerdictKind::Drop, true)), ("Allow", "Refuse"));
        assert_eq!((label(VerdictKind::Keep, false), label_for(VerdictKind::Drop, QuestionKind::Decision)), ("Keep", "Drop"));
    }
    use jaynes_core::manifest::{DeliveryKind, Effect, Executor, Manifest, Reach, SessionKind, StepDecl};
    use jaynes_core::question::{Item as QItem, Question};
    use jaynes_core::run::{Phase, QuestionState};
    use jaynes_core::Input;

    fn run(steps: &[(&str, bool)], delivery: DeliveryKind) -> Run {
        let decl = |name: &str, irreversible: bool| StepDecl {
            name: name.into(), needs: vec![], executor: if irreversible { Executor::Role } else { Executor::Agent }, session: SessionKind::Fresh, reach: Reach::default(),
            effect: if irreversible { Effect::Irreversible { idempotent: false } } else { Default::default() },
            checks: vec![], stop: None, from_risk: Default::default(), attempts: 3, fixes: 2, hidden: false, idempotent: false, about: if name == "plan" { "Plans the work.".into() } else { String::new() },
        };
        let manifest = Manifest { role: "r".into(), version: "1".into(), protocol: 1, steps: steps.iter().map(|(n, i)| decl(n, *i)).collect(), target: Default::default(), delivery };
        let start = Input::Start { run: "r1".into(), request: "x".into(), manifest, role_fingerprint: jaynes_core::Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false };
        Run::fold(&jaynes_core::decide(None, &start).expect("start").events).expect("run")
    }

    fn asking(mut r: Run) -> Run {
        let item = |id: &str, label: &str| QItem { id: id.into(), label: label.into(), digest: jaynes_core::Digest(format!("d-{id}")) };
        let question = Question {
            id: "q1".into(), step: "plan".into(), round: 1, kind: QuestionKind::Decision, items: vec![item("a", "Plan A"), item("b", "Plan B")],
            offered: vec![VerdictKind::Keep, VerdictKind::Redo, VerdictKind::Drop], holds: true, about: "Which plan goes on?".into(),
            choose: false, pass: 2, passes: 0, back: vec![item("c", "Plan C")],
        };
        r.questions.insert("q1".into(), QuestionState { question, answer: None, effects: None, superseded: false });
        r.phase = Phase::Waiting { step: Some("plan".into()), on: vec!["q1".into()], why: "waits".into() };
        r
    }

    // Alan, 21/11: "entender onde o python entra antes de decidir"
    #[test]
    fn a_stop_with_one_option_asks_whether_to_go_with_it_and_keeps_the_role_s_question_under() {
        let home = std::env::temp_dir().join(format!("jaynes-stop-one-{}", std::process::id()));
        let mut r = asking(run(&[("plan", false), ("change", false)], DeliveryKind::Branch));
        let q = &mut r.questions.get_mut("q1").expect("asked").question;
        q.choose = true;
        q.items.truncate(1);
        q.items[0].label = "Python + FastAPI: small and known".into();
        let stop = waiting(&home, &r).expect("a stop");
        assert_eq!((stop.title.as_str(), stop.asks.as_str()), ("Go with “Python + FastAPI”?", "Which plan goes on?"));
    }

    #[test]
    fn a_waiting_run_is_a_stop_and_the_marks_make_its_answer() {
        let home = std::env::temp_dir().join(format!("jaynes-stop-{}", std::process::id()));
        let r = asking(run(&[("plan", false), ("change", false)], DeliveryKind::Branch));
        let stop = waiting(&home, &r).expect("a stop");
        assert_eq!((stop.question.as_str(), stop.title.as_str(), stop.step.as_str()), ("q1", "Which plan goes on?", "plan"));
        assert_eq!(stop.asks, "Which plan goes on?");
        assert_eq!(stop.shown, r.questions["q1"].question.shown().0);
        assert_eq!(stop.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["Plan A", "Plan B"]);
        assert_eq!(stop.offered, [VerdictKind::Keep, VerdictKind::Redo, VerdictKind::Drop]);
        assert_eq!(stop.ahead, None);
        assert_eq!(stop.back.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["Plan C"]);
        assert_eq!((stop.round, stop.about.as_str()), (2, "Plans the work."));
        let mut marks = BTreeMap::new();
        marks.insert("a".to_string(), (VerdictKind::Keep, String::new()));
        marks.insert("b".to_string(), (VerdictKind::Redo, "it misses the tests".to_string()));
        let a = answer(&stop, &marks, "go on");
        assert_eq!((a.question.as_str(), a.shown.0.as_str(), a.context.as_str()), ("q1", stop.shown.as_str(), "go on"));
        assert_eq!(a.verdicts["a"], Verdict::Keep);
        assert_eq!(a.verdicts["b"], Verdict::Redo { note: "it misses the tests".into() });
        assert!(!a.by.person(), "the engine sets who answered, never the app");
    }

    /// A run of one agent step that may write the whole project, delivered by merge, waiting at the merge after it
    /// changed `changed`, in a home whose copy merges into `main`.
    fn at_the_merge(home: &Path, changed: Vec<jaynes_core::risk::Changed>) -> Run {
        let mut build = StepDecl {
            name: "build".into(), needs: vec![], executor: Executor::Agent, session: SessionKind::Fresh, reach: Reach::default(), effect: Default::default(),
            checks: vec![], stop: None, from_risk: Default::default(), attempts: 3, fixes: 2, hidden: false, idempotent: false, about: String::new(),
        };
        build.reach = Reach { project: jaynes_core::manifest::ProjectAccess::Write, write: vec!["**".into()], ..Reach::default() };
        let manifest = Manifest { role: "build".into(), version: "1".into(), protocol: 1, steps: vec![build], target: Default::default(), delivery: DeliveryKind::Merge };
        let start = Input::Start { run: "r1".into(), request: "Scaffold the\nengram service".into(), manifest, role_fingerprint: jaynes_core::Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false };
        let feed = |r: &mut Option<Run>, input: Input| -> Vec<jaynes_core::Command> {
            let out = jaynes_core::decide(r.as_ref(), &input).expect("taken");
            match r {
                Some(run) => out.events.iter().for_each(|e| run.apply(e)),
                None => *r = Some(Run::fold(&out.events).expect("run")),
            }
            out.commands
        };
        let mut r = None;
        let c = feed(&mut r, start);
        let mut turn = Vec::new();
        for cmd in c {
            if let jaynes_core::Command::Probe { step, .. } = cmd {
                turn.extend(feed(&mut r, Input::ProbeEnded { step, ok: true, reason: String::new() }));
            }
        }
        let attempt = turn.iter().find_map(|c| if let jaynes_core::Command::RunAgent(t) = c { Some(t.attempt) } else { None }).expect("a turn");
        let outcome = jaynes_core::AttemptOutcome::Delivered { output: jaynes_core::Digest("o".into()), items: vec![], changed, usage: Default::default(), session: None, tree: None };
        feed(&mut r, Input::AttemptEnded { step: "build".into(), attempt, fix: 0, outcome });
        std::fs::create_dir_all(home.join("runs/r1/project.git")).expect("copy");
        std::fs::write(home.join("runs/r1/project.git/jaynes-start"), "main\n").expect("start");
        r.expect("run")
    }

    fn file(path: &str, added: u64, removed: u64) -> jaynes_core::risk::Changed {
        let mut c = jaynes_core::risk::Changed::file(path.into(), added + removed);
        c.removed = removed;
        c.digest = Some(format!("d-{path}"));
        c
    }

    // Alan, 21/11: "tive que dar uma porrada de allow com o meu touchid"
    #[test]
    fn a_leave_asks_no_touch_and_the_merge_asks_it_once_showing_what_lands() {
        let home = std::env::temp_dir().join(format!("jaynes-stop-touch-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        // a leave: no "Ahead", no touch, though the run merges later
        let mut r = asking(run(&[("plan", false), ("change", false)], DeliveryKind::Merge));
        r.questions.get_mut("q1").expect("asked").question.kind = QuestionKind::Permission;
        let stop = waiting(&home, &r).expect("a stop");
        assert_eq!((stop.ahead.as_deref(), stop.touch.as_deref()), (None, None), "a leave");
        // a decision before the merge: the merge is ahead, and asks itself, so no touch here
        let r = asking(run(&[("plan", false), ("change", false)], DeliveryKind::Merge));
        let stop = waiting(&home, &r).expect("a stop");
        assert_eq!((stop.ahead.as_deref(), stop.touch.as_deref()), (Some("the run merges into your branch"), None));
        // a step that cannot be undone keeps its touch
        let r = asking(run(&[("plan", false), ("send", true)], DeliveryKind::Branch));
        let stop = waiting(&home, &r).expect("a stop");
        assert_eq!((stop.ahead.as_deref(), stop.touch.as_deref()), (Some("“send” cannot be undone"), Some("send your answer to Jaynes: “send” cannot be undone")));
        // the merge: every file that lands with its lines, what runs later said in plain words, the touch once
        let r = at_the_merge(&home, vec![file("src/app.py", 64, 12), file("tests/conftest.py", 14, 0), file("Makefile", 3, 1)]);
        let stop = waiting(&home, &r).expect("the merge waits");
        assert_eq!(stop.kind, QuestionKind::Merge);
        assert_eq!(stop.title, "Merge into “main”?");
        assert_eq!(stop.asks, "“Scaffold the engram service” is done and checked. This is what lands on your branch.");
        assert_eq!((stop.ahead.as_deref(), stop.touch.as_deref()), (None, Some("merge “Scaffold the engram service” into main")));
        assert_eq!(stop.into, "main");
        assert_eq!(
            stop.lands,
            [
                Lands { path: "src/app.py".into(), added: 64, removed: 12, runs: None },
                Lands { path: "tests/conftest.py".into(), added: 14, removed: 0, runs: Some("runs on its own whenever the tests run") },
                Lands { path: "Makefile".into(), added: 3, removed: 1, runs: Some("runs when you run make") },
            ]
        );
        assert_eq!(stop.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["src/app.py", "tests/conftest.py", "Makefile"]);
        // one answer for every file, and what it does names the branch and only what runs later
        let marks: BTreeMap<String, (VerdictKind, String)> = stop.items.iter().map(|i| (i.id.clone(), (VerdictKind::Keep, String::new()))).collect();
        let q = &r.questions[&stop.question].question;
        let fx = crate::drive::effects(&home, &r, q, &answer(&stop, &marks, "")).expect("effects");
        assert_eq!(fx.sentences, ["The delivery merges into “main”, with tests/conftest.py and Makefile."]);
        let marks: BTreeMap<String, (VerdictKind, String)> = stop.items.iter().map(|i| (i.id.clone(), (VerdictKind::Drop, String::new()))).collect();
        let fx = crate::drive::effects(&home, &r, q, &answer(&stop, &marks, "")).expect("effects");
        assert_eq!(fx.sentences, ["Nothing lands on your branch: the change stays on the branch jaynes/r1, where you can look at it."]);
        // nothing that runs later: the same stop, nothing said of any file, the branch named
        let r = at_the_merge(&home, vec![file("src/app.py", 1, 0)]);
        let stop = waiting(&home, &r).expect("the merge waits");
        assert_eq!((stop.title.as_str(), stop.lands.len(), stop.lands[0].runs), ("Merge into “main”?", 1, None));
        let marks: BTreeMap<String, (VerdictKind, String)> = stop.items.iter().map(|i| (i.id.clone(), (VerdictKind::Keep, String::new()))).collect();
        let fx = crate::drive::effects(&home, &r, &r.questions[&stop.question].question, &answer(&stop, &marks, "")).expect("effects");
        assert_eq!(fx.sentences, ["The delivery merges into “main”."]);
        let _gone = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn what_cannot_be_undone_ahead_is_said() {
        assert_eq!(ahead(&run(&[("plan", false), ("change", false)], DeliveryKind::Branch), "plan"), None);
        let deploy = run(&[("plan", false), ("deploy", true)], DeliveryKind::Branch);
        // the step's name in curly quotes, as every sentence names a step ("Ahead: “deploy” cannot be undone.")
        assert_eq!(ahead(&deploy, "plan").as_deref(), Some("“deploy” cannot be undone"));
        let merge = run(&[("plan", false), ("change", false)], DeliveryKind::Merge);
        assert_eq!(ahead(&merge, "plan").as_deref(), Some("the run merges into your branch"));
    }
}
