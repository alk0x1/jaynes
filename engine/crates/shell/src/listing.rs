//! The runs as the person sees them in a list: what waits for them first, then each project's runs, newest first.
//! The page and the Jaynes app show the same list, in the same words.

use crate::store::Store;
use jaynes_core::event::Outcome;
use jaynes_core::question::QuestionKind;
use jaynes_core::run::{Phase, QuestionState, Run};
use std::path::Path;

/// How a run stands, as a mark beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// A question waits for the person.
    Needs,
    Working,
    Halted,
    Blocked,
    Delivered,
    Merged,
    Undone,
    Abandoned,
}

/// One run in the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    /// The request, short.
    pub title: String,
    /// The question that waits, or where the run is.
    pub line: String,
    pub mark: Mark,
}

/// Runs that wait for the person, then each project's runs; every run of the record once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub needs: Vec<Row>,
    /// A project's folder name and its runs, the project with the newest run first.
    pub projects: Vec<(String, Vec<Row>)>,
}

/// A run with what the list needs from outside its events.
pub struct Entry {
    pub run: Run,
    /// The project's folder, empty when the run has none.
    pub project: String,
    /// When the run last changed, in seconds.
    pub at: u64,
    /// Whether what it delivered was merged into the person's branch.
    pub merged: bool,
}

/// Every run of the record, as `jaynes runs` lists them, in the list.
pub fn read(home: &Path) -> Result<Listing, String> {
    let store = Store::open(&home.join("record.sqlite3"))?;
    let times = store.times()?;
    let mut entries = Vec::new();
    for id in store.runs()? {
        let Some(run) = store.state(&id)? else { continue };
        let project = store.setup(&id).ok().and_then(|s| s["project"].as_str().map(str::to_owned)).unwrap_or_default();
        let at = times.get(&id).map(|t| t.1).unwrap_or(0);
        let merged = !crate::drive::merges(&home.join("runs").join(&id)).is_empty();
        entries.push(Entry { run, project, at, merged });
    }
    Ok(arrange(entries))
}

pub fn arrange(mut entries: Vec<Entry>) -> Listing {
    entries.sort_by_key(|e| std::cmp::Reverse(e.at));
    let mut listing = Listing::default();
    for e in &entries {
        let r = &e.run;
        let request = short(&r.request);
        let waiting = if r.finished() { None } else { r.questions.values().find(|q| q.open()) };
        if let Some(q) = waiting {
            listing.needs.push(Row { id: r.id.clone(), title: request, line: title(q), mark: Mark::Needs });
            continue;
        }
        let (mark, line) = standing(e);
        let project = Path::new(&e.project).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "no project".into());
        let row = Row { id: r.id.clone(), title: request, line, mark };
        match listing.projects.iter_mut().find(|(p, _)| *p == project) {
            Some((_, rows)) => rows.push(row),
            None => listing.projects.push((project, vec![row])),
        }
    }
    listing
}

/// The mark and the line of a run nothing waits on.
fn standing(e: &Entry) -> (Mark, String) {
    let r = &e.run;
    match &r.phase {
        Phase::Finished { outcome: Outcome::Abandoned } => (Mark::Abandoned, "abandoned".into()),
        Phase::Finished { outcome: Outcome::Delivered } if r.undone => (Mark::Undone, "reverted by you".into()),
        Phase::Finished { outcome: Outcome::Delivered } if e.merged => (Mark::Merged, "merged".into()),
        Phase::Finished { outcome: Outcome::Delivered } => (Mark::Delivered, "delivered".into()),
        _ if r.halted => (Mark::Halted, format!("{}, halted", where_in(r))),
        Phase::Blocked { reason, .. } => (Mark::Blocked, format!("{}, blocked: {reason}", where_in(r))),
        Phase::Waiting { step: None, .. } => (Mark::Delivered, "delivered, answers open".into()),
        Phase::Waiting { why, .. } => (Mark::Working, format!("{}, {why}", where_in(r))),
        Phase::Running => (Mark::Working, format!("{}, running", where_in(r))),
    }
}

/// The request in at most 60 characters.
pub fn short(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= 60 { s.to_string() } else { format!("{}…", s.chars().take(59).collect::<String>()) }
}

/// Where the run is on its path: "step 2 of 5".
pub fn where_in(r: &Run) -> String {
    let path = r.path();
    let at = path.iter().position(|&i| !r.fresh(i)).map(|n| n + 1).unwrap_or(path.len());
    format!("step {at} of {}", path.len())
}

/// What a question asks, in a line.
pub fn title(q: &QuestionState) -> String {
    if !q.question.about.trim().is_empty() {
        q.question.about.clone()
    } else if q.question.kind == QuestionKind::Permission {
        format!("May the agent {}?", q.question.about)
    } else {
        format!("Does what {} made go on?", q.question.step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jaynes_core::manifest::{Executor, Manifest, Reach, SessionKind, StepDecl};
    use jaynes_core::question::{Question, QuestionKind};
    use jaynes_core::Input;

    fn run(id: &str, request: &str) -> Run {
        let step = |name: &str| StepDecl {
            name: name.into(), needs: vec![], executor: Executor::Agent, session: SessionKind::Fresh, reach: Reach::default(),
            effect: Default::default(), checks: vec![], stop: None, from_risk: Default::default(), attempts: 3, fixes: 2, hidden: false, about: String::new(),
        };
        let manifest = Manifest { role: "r".into(), version: "1".into(), protocol: 1, steps: vec![step("plan"), step("change")], delivery: Default::default() };
        let start = Input::Start { run: id.into(), request: request.into(), manifest, role_fingerprint: jaynes_core::Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default() };
        let out = jaynes_core::decide(None, &start).expect("start");
        Run::fold(&out.events).expect("run")
    }

    fn entry(run: Run, project: &str, at: u64) -> Entry {
        Entry { run, project: project.into(), at, merged: false }
    }

    #[test]
    fn what_waits_for_the_person_comes_first_and_each_project_lists_its_runs_newest_first() {
        let mut waits = run("r1", "Make the onboarding friendlier");
        waits.phase = Phase::Waiting { step: Some("plan".into()), on: vec!["q1".into()], why: "waits for your answer".into() };
        let question = Question {
            id: "q1".into(), step: "plan".into(), round: 1, kind: QuestionKind::Decision, items: vec![], offered: vec![], holds: true,
            about: "Which plan goes on?".into(), choose: false, pass: 0, passes: 0, back: vec![],
        };
        waits.questions.insert("q1".into(), QuestionState { question, answer: None, effects: None, superseded: false });
        let works = run("r2", "Fix the login redirect");
        let mut older = run("r3", "Rename the settings page");
        older.phase = Phase::Finished { outcome: Outcome::Delivered };
        let other = run("r4", "Add a dark theme");

        let listing = arrange(vec![entry(waits, "/p/app", 30), entry(older, "/p/app", 10), entry(works, "/p/app", 20), entry(other, "/p/site", 40)]);

        assert_eq!(listing.needs, vec![Row { id: "r1".into(), title: "Make the onboarding friendlier".into(), line: "Which plan goes on?".into(), mark: Mark::Needs }]);
        let names: Vec<&str> = listing.projects.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(names, ["site", "app"]);
        let app: Vec<(&str, Mark, &str)> = listing.projects[1].1.iter().map(|r| (r.id.as_str(), r.mark, r.line.as_str())).collect();
        assert_eq!(app, [("r2", Mark::Working, "step 1 of 2, running"), ("r3", Mark::Delivered, "delivered")]);
    }

    #[test]
    fn every_run_is_listed_once() {
        let mut halted = run("a", "x");
        halted.halted = true;
        let mut dropped = run("b", "y");
        dropped.phase = Phase::Finished { outcome: Outcome::Abandoned };
        let mut merged = run("c", "z");
        merged.phase = Phase::Finished { outcome: Outcome::Delivered };
        let listing = arrange(vec![entry(halted, "", 1), entry(dropped, "/q", 2), Entry { merged: true, ..entry(merged, "/q", 3) }]);
        let mut ids: Vec<&str> = listing.needs.iter().chain(listing.projects.iter().flat_map(|(_, rows)| rows)).map(|r| r.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["a", "b", "c"]);
        let marks: Vec<(String, Mark)> = listing.projects.iter().flat_map(|(p, rows)| rows.iter().map(move |r| (p.clone(), r.mark))).collect();
        assert_eq!(marks, [("q".to_string(), Mark::Merged), ("q".to_string(), Mark::Abandoned), ("no project".to_string(), Mark::Halted)]);
    }
}
