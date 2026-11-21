//! A stop as the Jaynes app shows it: the question that waits for the person, each item with what it shows, the
//! verdicts the person may give, and what lies ahead that cannot be undone, so the app asks for the person's touch
//! before an answer that leads there. What an answer will do is still written by the core (`drive::effects`).

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
    /// What lies ahead of this stop that cannot be undone, in a sentence, if anything does.
    pub ahead: Option<String>,
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
    Some(Stop {
        run: run.id.clone(),
        question: q.question.id.clone(),
        shown: q.question.shown().0,
        title: crate::listing::title(q),
        step: q.question.step.clone(),
        permission: q.question.kind == QuestionKind::Permission,
        kind: q.question.kind,
        choose: q.question.choose,
        offered: q.question.offered.clone(),
        items,
        back,
        round: q.question.pass,
        about,
        ahead: ahead(run, &q.question.step),
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

/// What lies ahead of `step` that cannot be undone: a step whose effect cannot be, or a merge into the person's
/// branch with no other stop of theirs before it.
pub fn ahead(run: &Run, step: &str) -> Option<String> {
    let from = run.manifest.index(step)?;
    let path = run.path();
    let later: Vec<usize> = path.into_iter().filter(|&j| j > from).collect();
    if let Some(&j) = later.iter().find(|&&j| run.manifest.steps[j].effect.irreversible()) {
        return Some(format!("“{}” cannot be undone", run.manifest.steps[j].name));
    }
    let another_stop = later.iter().any(|&j| run.manifest.steps[j].stop.is_some() || run.manifest.steps[j].executor == jaynes_core::manifest::Executor::Person);
    (run.manifest.lands() && !another_stop).then(|| "the run merges into your branch".to_string())
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

    #[test]
    fn a_waiting_run_is_a_stop_and_the_marks_make_its_answer() {
        let home = std::env::temp_dir().join(format!("jaynes-stop-{}", std::process::id()));
        let r = asking(run(&[("plan", false), ("change", false)], DeliveryKind::Branch));
        let stop = waiting(&home, &r).expect("a stop");
        assert_eq!((stop.question.as_str(), stop.title.as_str(), stop.step.as_str()), ("q1", "Which plan goes on?", "plan"));
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
