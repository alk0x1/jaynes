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

impl Mark {
    /// The mark in words, as the app's column, the page and `jaynes runs` say it.
    pub fn word(self) -> &'static str {
        match self {
            Mark::Needs => "needs you",
            Mark::Working => "working",
            Mark::Halted => "halted",
            Mark::Blocked => "blocked",
            Mark::Delivered => "delivered",
            Mark::Merged => "merged",
            Mark::Undone => "undone",
            Mark::Abandoned => "abandoned",
        }
    }
}

/// How a run stands, as `jaynes runs` says it: the mark in words, then the row's line when it says more ("halted ·
/// step 2 of 3, halted", "needs you · Which plan goes on?", "merged").
pub fn said(row: &Row) -> String {
    let word = row.mark.word();
    if row.line.is_empty() || row.line == word { word.to_string() } else { format!("{word} · {}", row.line) }
}

/// One run in the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    /// The project's folder name, with the folder above it when another project has a folder of the same name;
    /// "no project" for a run without one.
    pub project: String,
    /// The run's title: the one given at its start, or the first sentence of its request (`run_title`).
    pub title: String,
    /// What tells it apart from another run of the same title in the same project: "at “{step}”", or "started HH:MM"
    /// when the step is the same too. None when nothing of the same name is in its project.
    pub apart: Option<String>,
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
#[derive(Clone)]
pub struct Entry {
    pub run: Run,
    /// The project's folder, empty when the run has none.
    pub project: String,
    /// The title given at the run's start, as its setup keeps it.
    pub title: Option<String>,
    /// When the run started and last changed, in seconds.
    pub started: u64,
    pub at: u64,
    /// Whether what it delivered was merged into the person's branch.
    pub merged: bool,
    /// The branch the run merges into, the one checked out when it started; empty when unknown.
    pub into: String,
}

/// Every run of the record, as `jaynes runs` lists them, in the list.
pub fn read(home: &Path) -> Result<Listing, String> {
    Ok(arrange(&entries(home)?))
}

/// Every run of the record, as `jaynes runs` lists them, with what the list needs from outside its events.
pub fn entries(home: &Path) -> Result<Vec<Entry>, String> {
    let store = Store::open(&home.join("record.sqlite3"))?;
    let times = store.times()?;
    let mut entries = Vec::new();
    for id in store.runs()? {
        let Some(run) = store.state(&id)? else { continue };
        let setup = store.setup(&id).unwrap_or_default();
        let project = setup["project"].as_str().map(str::to_owned).unwrap_or_default();
        let title = setup["title"].as_str().map(str::to_owned);
        let (started, at) = times.get(&id).copied().unwrap_or((0, 0));
        entries.push(Entry { project, title, started, at, ..entry_of(home, run) });
    }
    Ok(entries)
}

/// Run `run` with what its mark needs from outside its events (whether it was merged, and into which branch); no
/// project, title or times.
pub fn entry_of(home: &Path, run: Run) -> Entry {
    let merged = !crate::drive::merges(&home.join("runs").join(&run.id)).is_empty();
    let into = into_of(home, &run.id);
    Entry { run, project: String::new(), title: None, started: 0, at: 0, merged, into }
}

/// How a run stands in the list: its mark, its line, and whether it waits for the person.
pub fn stands(e: &Entry) -> (Mark, String, bool) {
    let r = &e.run;
    let waiting = if r.finished() { None } else { r.questions.values().find(|q| q.open() && crate::stop::went_on(r, q).is_none()) };
    match waiting {
        Some(q) => (Mark::Needs, title_into(q, &e.into), true),
        None => {
            let (mark, line) = standing(e);
            (mark, line, false)
        }
    }
}

pub fn arrange(entries: &[Entry]) -> Listing {
    let mut entries: Vec<&Entry> = entries.iter().collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.at));
    let folders: Vec<&str> = entries.iter().map(|e| e.project.as_str()).collect();
    // each run's row, and whether it waits for the person
    let mut rows: Vec<(Row, bool, &Entry)> = Vec::new();
    for e in &entries {
        let r = &e.run;
        let project = project_name(&e.project, &folders);
        let title = run_title(e.title.as_deref(), &r.request);
        let (mark, line, needs) = stands(e);
        rows.push((Row { id: r.id.clone(), project, title, apart: None, line, mark }, needs, e));
    }
    // two runs of one title in one project: the step each is at, or when it started when the step is the same
    let apart: Vec<Option<String>> = rows
        .iter()
        .map(|(row, _, e)| {
            let same: Vec<&Entry> = rows.iter().filter(|(o, _, _)| o.id != row.id && o.project == row.project && o.title == row.title).map(|(_, _, o)| *o).collect();
            if same.is_empty() {
                return None;
            }
            let step = step_at(&e.run);
            if same.iter().any(|o| step_at(&o.run) == step) { Some(format!("started {}", clock(e.started))) } else { Some(format!("at “{step}”")) }
        })
        .collect();
    let mut listing = Listing::default();
    for ((mut row, needs, _), apart) in rows.into_iter().zip(apart) {
        row.apart = apart;
        if needs {
            listing.needs.push(row);
            continue;
        }
        match listing.projects.iter_mut().find(|(p, _)| *p == row.project) {
            Some((_, rows)) => rows.push(row),
            None => listing.projects.push((row.project.clone(), vec![row])),
        }
    }
    listing
}

/// The stops runs went on without, newest run first, each as a row whose line says so: "At “reproduce”: Is this the
/// bug? · The run went on without your answer." They come after what waits for the person, under "Can still answer".
pub fn can_still_answer(entries: &[Entry]) -> Vec<Row> {
    let folders: Vec<&str> = entries.iter().map(|e| e.project.as_str()).collect();
    let mut newest: Vec<&Entry> = entries.iter().collect();
    newest.sort_by_key(|e| std::cmp::Reverse(e.at));
    newest
        .into_iter()
        .flat_map(|e| {
            let r = &e.run;
            let (project, title) = (project_name(&e.project, &folders), run_title(e.title.as_deref(), &r.request));
            let mark = standing(e).0;
            r.questions.values().filter(|q| crate::stop::went_on(r, q).is_some()).map(move |q| Row {
                id: r.id.clone(),
                project: project.clone(),
                title: title.clone(),
                apart: None,
                line: passed_line(&q.question.step, &self::title(q)),
                mark,
            })
        })
        .collect()
}

/// The line of a stop the run went on without: "At “reproduce”: Is this the bug? · The run went on without your answer."
pub fn passed_line(step: &str, title: &str) -> String {
    format!("At “{step}”: {title} · The run went on without your answer.")
}

/// A project's name in the list: its folder's name, with the folder above it when another project of `folders` has
/// a folder of the same name somewhere else; "no project" for a run without one.
fn project_name(folder: &str, folders: &[&str]) -> String {
    let path = Path::new(folder);
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else { return "no project".into() };
    let twin = folders.iter().any(|f| *f != folder && Path::new(f).file_name().is_some_and(|n| n.to_string_lossy() == name));
    match path.parent().and_then(|p| p.file_name()) {
        Some(above) if twin => format!("{}/{name}", above.to_string_lossy()),
        _ => name,
    }
}

/// The step a run is at: the first on its path whose output is not fresh, or its last.
pub(crate) fn step_at(r: &Run) -> String {
    let path = r.path();
    let at = path.iter().find(|&&i| !r.fresh(i)).or(path.last());
    at.map(|&i| r.manifest.steps[i].name.clone()).unwrap_or_default()
}

/// The hour and minute of a moment on this machine's clock, as the person reads it: "14:02".
fn clock(ts: u64) -> String {
    let flags: [&str; 2] = if cfg!(target_os = "macos") { ["-r", ""] } else { ["-d", "@"] };
    let said = std::process::Command::new("date").args([flags[0].to_string(), format!("{}{ts}", flags[1]), "+%H:%M".into()]).output();
    match said {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => format!("{:02}:{:02} UTC", ts % 86_400 / 3600, ts % 3600 / 60),
    }
}

/// A run's name, in its parts in order: its project, its title, and what tells it apart when another run of its
/// project has the same title. The app and the page show a run by these, never by a name of their own.
pub fn name(row: &Row) -> Vec<String> {
    let mut parts = vec![row.project.clone(), row.title.clone()];
    parts.extend(row.apart.clone());
    parts
}

/// A run's title: the one given at its start, or the first sentence of its request without an opening "Agreed with
/// … on …:"; past 40 characters, cut at a word.
pub fn run_title(given: Option<&str>, request: &str) -> String {
    let given = given.map(one_line).filter(|g| !g.is_empty());
    let title = match given {
        Some(g) => g,
        None => {
            let r = request.trim();
            let first_line = r.lines().next().unwrap_or("");
            let r = match first_line.find(": ") {
                Some(at) if first_line.to_lowercase().starts_with("agreed with") => r[at + 2..].trim_start(),
                _ => r,
            };
            let first = one_line(first_sentence(r));
            let mut chars = first.chars();
            chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
        }
    };
    cut_at_word(&title, 40)
}

/// `text` on one line: every run of white space, control characters and marks that change how text is drawn
/// (direction, zero width) becomes one space, so nothing in a title reads as a line of the engine's own.
fn one_line(text: &str) -> String {
    let hidden = |c: char| matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}');
    let spaced: String = text.chars().map(|c| if c.is_whitespace() || c.is_control() || hidden(c) { ' ' } else { c }).collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first sentence of `text`: up to its first full stop (left out), question or exclamation mark (kept), or line end.
fn first_sentence(text: &str) -> &str {
    let mut end = text.len();
    for (i, c) in text.char_indices() {
        let next = text[i + c.len_utf8()..].chars().next();
        if c == '\n' || (matches!(c, '.' | '?' | '!') && next.is_none_or(char::is_whitespace)) {
            end = if matches!(c, '?' | '!') { i + 1 } else { i };
            break;
        }
    }
    text[..end].trim()
}

/// `text` in at most `max` characters: past it, cut at the last word that fits, with "…".
fn cut_at_word(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max - 1).collect();
    let at_word = text.chars().nth(max - 1).is_some_and(char::is_whitespace);
    let kept = if at_word { kept.as_str() } else { kept.rfind(' ').map(|i| &kept[..i]).unwrap_or(&kept) };
    format!("{}…", kept.trim_end_matches(|c: char| c.is_whitespace() || ",;:-".contains(c)))
}

/// A reason as the engine says it now: before 0.2.4 it said how a program ended as Rust's debug text ("exited with
/// Some(1)", "exited with None"), and a record keeps a reason as it was said.
pub fn in_words(reason: &str) -> String {
    let mut out = reason.replace("exited with None", "was stopped by a signal, with no exit code");
    while let Some(at) = out.find("exited with Some(") {
        let after = &out[at + "exited with Some(".len()..];
        let Some((code, rest)) = after.split_once(')').filter(|(c, _)| c.parse::<i32>().is_ok()) else { break };
        out = format!("{}exited with code {code}{rest}", &out[..at]);
    }
    // a program that said nothing after how it ended left ": " at the end
    out.trim_end().trim_end_matches(':').trim_end().to_string()
}

/// The mark and the line of a run nothing waits on.
fn standing(e: &Entry) -> (Mark, String) {
    let r = &e.run;
    match &r.phase {
        Phase::Finished { outcome: Outcome::Abandoned } => (Mark::Abandoned, "abandoned".into()),
        Phase::Finished { outcome: Outcome::Delivered } if r.undone => (Mark::Undone, "reverted by you".into()),
        Phase::Finished { outcome: Outcome::Delivered } if e.merged => (Mark::Merged, "merged".into()),
        Phase::Finished { outcome: Outcome::Delivered } => (Mark::Delivered, "delivered".into()),
        _ if r.set_aside.is_some() => (Mark::Halted, format!("{}, set aside by the agent that started it", where_in(r))),
        _ if r.halted => (Mark::Halted, format!("{}, halted", where_in(r))),
        Phase::Blocked { reason, .. } => (Mark::Blocked, format!("{}, blocked: {}", where_in(r), in_words(reason))),
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

/// The branch run `run` merges into, the one checked out when it started; empty when its copy does not say.
pub fn into_of(home: &Path, run: &str) -> String {
    crate::project::start_branch_of(&home.join("runs").join(run).join("project"))
}

/// What a merge asks, naming the branch it merges into, as the app's card and the page say it.
pub fn merge_title(into: &str) -> String {
    if into.is_empty() { "Merge into your branch?".to_string() } else { format!("Merge into “{into}”?") }
}

/// What a question asks, in a line, a merge naming the branch `into` it merges into.
pub fn title_into(q: &QuestionState, into: &str) -> String {
    if q.question.kind == QuestionKind::Merge && !q.question.one_option() { merge_title(into) } else { title(q) }
}

/// What a question asks, in a line. A choice with one option asks whether to go with it, by its name (its label
/// before the colon, as the app names a direction): the role's question goes on the line under it.
pub fn title(q: &QuestionState) -> String {
    if q.question.one_option() {
        format!("Go with “{}”?", q.question.items[0].name())
    } else if q.question.kind == QuestionKind::Merge {
        merge_title("")
    } else if q.question.kind == QuestionKind::Permission {
        // what the leave asks, as the core says it ("change tests/conftest.py"): the verb, then its target in backticks
        match q.question.about.split_once(' ') {
            Some((verb, target)) => format!("May the agent {verb} `{target}`?"),
            None => format!("May the agent {}?", q.question.about),
        }
    } else if !q.question.about.trim().is_empty() {
        q.question.about.clone()
    } else {
        format!("Does what {} made go on?", q.question.step)
    }
}

/// One step of a run, as the page's list and the app's panel show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stage {
    pub index: usize,
    pub name: String,
    pub about: String,
    /// done ones are "checked", "accepted", "sampled" or "jaynes"; then "running", "halted" (a step under way in a run
    /// the person halted: nothing works on it until they resume), "waiting", "blocked", "todo".
    pub class: &'static str,
    pub detail: String,
}

/// Every step of the run in order, with how it stands.
pub fn stages(r: &Run) -> Vec<Stage> {
    use jaynes_core::manifest::Executor;
    use jaynes_core::question::By;
    use jaynes_core::run::Status;
    let path = r.path();
    let (blocked, spent_all) = match &r.phase {
        Phase::Blocked { step: Some(s), fault, .. } => (Some(s.as_str()), *fault == jaynes_core::event::Fault::Agent),
        _ => (None, false),
    };
    // a question a step asked and the person has not answered: that step is the one that waits, and the step the
    // engine names as waiting is only the next to run, once the answer comes; once the run is over, nothing waits
    let asks = |i: usize| matches!(r.steps[i].status, Status::AwaitingInput { .. }) || (r.fresh(i) && r.open_question(i).is_some());
    let someone_asks = (0..r.steps.len()).any(asks);
    r.manifest
        .steps
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let s = &r.steps[i];
            // only an agent's step has attempts to say, counted as the run's work counts them
            let agent = d.executor == Executor::Agent;
            let tries = format!("{} attempt{}", s.attempt, if s.attempt == 1 { "" } else { "s" });
            let try_n = |attempt: &u32| if agent { format!(", attempt {attempt}") } else { String::new() };
            let round = if s.round > 0 { format!(", round {}", s.round + 1) } else { String::new() };
            let answer = s.question.as_ref().and_then(|q| r.questions.get(q)).and_then(|q| q.answer.clone());
            let (class, detail) = if !path.contains(&i) {
                ("todo", "left out at this risk".to_string())
            } else if blocked == Some(d.name.as_str()) {
                // the agent spent every attempt it had: said so, as the run's tab says it
                ("blocked", if spent_all { format!("blocked: used its {tries}") } else if agent { format!("blocked, {tries}") } else { "blocked".to_string() })
            } else {
                match &s.status {
                    Status::Running { attempt, .. } | Status::Checking { attempt, .. } if !r.finished() && r.halted => ("halted", format!("halted{}{round}", try_n(attempt))),
                    Status::Probing if !r.finished() && r.halted => ("halted", "halted while testing what it reaches".to_string()),
                    Status::Running { attempt, .. } | Status::Checking { attempt, .. } if !r.finished() => ("running", format!("running{}{round}", try_n(attempt))),
                    Status::Probing if !r.finished() => ("running", "testing what it reaches".to_string()),
                    _ if asks(i) && r.finished() => ("todo", "left unanswered".to_string()),
                    _ if asks(i) => ("waiting", format!("waiting for you{round}")),
                    Status::Done if r.fresh(i) && s.skipped == Some(s.round) => ("sampled", "not asked, sampled".to_string()),
                    Status::Done if r.fresh(i) => match (&answer, d.executor) {
                        (Some(a), _) if matches!(a.by, By::Agent { .. }) => ("checked", "an agent let it go on".to_string()),
                        (Some(_), Executor::Person) => ("accepted", "you answered".to_string()),
                        (Some(_), _) => ("accepted", format!("you kept it, {tries}")),
                        (None, Executor::Role) => ("jaynes", "by jaynes".to_string()),
                        (None, _) => ("checked", format!("checked, {tries}{round}")),
                    },
                    _ if matches!(&r.phase, Phase::Waiting { step: Some(w), .. } if *w == d.name) => {
                        if someone_asks { ("todo", "next, after your answer".to_string()) } else { ("waiting", "waiting for you".to_string()) }
                    }
                    _ if s.done.is_some() => ("todo", format!("to run again{round}")),
                    _ => ("todo", String::new()),
                }
            };
            Stage { index: i, name: d.name.clone(), about: d.about.clone(), class, detail }
        })
        .collect()
}

/// What a finished run did, as the page and the app show it to copy: [`crate::summary::Summary::markdown`] is the
/// text. Neither waits for the role's promises to be proved: they are proved beside, and told once they are.
pub fn summary(home: &Path, run: &str) -> Result<crate::summary::Summary, String> {
    crate::summary::read(home, run, false)
}

/// What the agent did last in a turn's transcript (Claude Code's stream): the tool it called with what it called it
/// on, else what it said, else that it thinks. Paths inside `project` are shown from there.
pub fn last_action(transcript: &str, project: &Path) -> Option<String> {
    for line in transcript.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        match (v["type"].as_str(), v["subtype"].as_str()) {
            (Some("result"), _) => return Some("finished its turn".into()),
            (Some("system"), Some("thinking_tokens")) => return Some("thinking".into()),
            _ => {
                if let Some(last) = actions(&v, project).pop() {
                    return Some(last);
                }
            }
        }
    }
    None
}

/// What one line of the stream shows the agent doing: each tool it calls with what it calls it on, and each thing it
/// says (its first line), in order. Paths inside `project` are shown from there.
fn actions(v: &serde_json::Value, project: &Path) -> Vec<String> {
    let inside = format!("{}/", project.display());
    let shown = |v: &serde_json::Value| -> String {
        let text = v.as_str().unwrap_or_default();
        let text = text.strip_prefix(inside.as_str()).unwrap_or(text);
        text.lines().next().unwrap_or_default().trim().chars().take(160).collect()
    };
    if v["type"].as_str() != Some("assistant") {
        return Vec::new();
    }
    let blocks = v["message"]["content"].as_array().cloned().unwrap_or_default();
    blocks
        .iter()
        .filter_map(|b| match b["type"].as_str() {
            Some("tool_use") => {
                let name = b["name"].as_str().unwrap_or("a tool");
                let input = &b["input"];
                let on = ["file_path", "command", "pattern", "url", "query", "path", "description"].iter().map(|k| shown(&input[*k])).find(|s| !s.is_empty()).unwrap_or_default();
                Some(if on.is_empty() { name.to_string() } else { format!("{name} {on}") })
            }
            Some("text") if !shown(&b["text"]).is_empty() => Some(shown(&b["text"])),
            _ => None,
        })
        .collect()
}

/// A run's story: each step its agents worked, in the order they started, with what they did there, turn after turn,
/// read from the transcripts the engine writes as the agents speak. The app shows it in the run's tab.
pub fn story(home: &Path, run: &str) -> Vec<(String, Vec<String>)> {
    let dir = home.join("runs").join(run);
    let project = dir.join("project");
    let when = |p: &Path| p.metadata().and_then(|m| m.modified()).ok();
    let mut steps: Vec<(Option<std::time::SystemTime>, String, Vec<std::path::PathBuf>)> = std::fs::read_dir(dir.join("steps"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|step| {
            let mut turns: Vec<std::path::PathBuf> = std::fs::read_dir(step.path())
                .ok()?
                .flatten()
                .map(|f| f.path())
                .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("turn-")))
                .collect();
            turns.sort_by_key(|p| (when(p), p.clone()));
            let first = turns.first().and_then(|p| when(p));
            (!turns.is_empty()).then(|| (first, step.file_name().to_string_lossy().into_owned(), turns))
        })
        .collect();
    steps.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    steps
        .into_iter()
        .map(|(_, name, turns)| {
            let said = turns
                .iter()
                .filter_map(|t| tail(t, 1024 * 1024))
                .flat_map(|text| text.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).flat_map(|v| actions(&v, &project)).collect::<Vec<_>>())
                .collect();
            (name, said)
        })
        .collect()
}

/// What the agent of a run is doing now: the last action in its newest transcript.
pub fn now(home: &Path, run: &str) -> Option<String> {
    let dir = home.join("runs").join(run);
    let newest = std::fs::read_dir(dir.join("steps"))
        .ok()?
        .flatten()
        .flat_map(|step| std::fs::read_dir(step.path()).into_iter().flatten().flatten())
        .filter(|f| f.file_name().to_string_lossy().starts_with("turn-"))
        .max_by_key(|f| f.metadata().and_then(|m| m.modified()).ok())?;
    last_action(&tail(&newest.path(), 256 * 1024)?, &dir.join("project"))
}

/// The last `max` bytes of a file, from the first whole line in them.
fn tail(path: &Path, max: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(max))).ok()?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Some(if len > max { text.split_once('\n').map(|(_, rest)| rest.to_string()).unwrap_or_default() } else { text })
}

/// What the agents of a run did, counted one way for the page, the app and the summary: the attempts at the steps
/// an agent works (a step Jaynes or the person does is no attempt), the fixes it made inside them after a check, and
/// the attempts that failed by its own fault, the only ones a step's limit counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Work {
    pub attempts: u32,
    pub fixes: u32,
    pub own_failures: u32,
}

/// What the agents of a run did, from its steps: an attempt is a step's go from the start, numbered by the core.
pub fn work(r: &Run) -> Work {
    use jaynes_core::manifest::Executor;
    let attempts = r.manifest.steps.iter().zip(&r.steps).filter(|(d, _)| d.executor == Executor::Agent).map(|(_, s)| s.attempt).sum();
    Work { attempts, fixes: r.counts.fixes, own_failures: r.counts.spent }
}

impl Work {
    /// "2 attempts by the agent, 1 failed through its own fault, 1 fix"; a part that is none is not said, but the
    /// attempts.
    pub fn said(&self) -> String {
        let plural = |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut out = if self.attempts == 0 { "no attempt by the agent".to_string() } else { format!("{} by the agent", plural(self.attempts, "attempt", "attempts")) };
        if self.own_failures > 0 {
            out.push_str(&format!(", {} failed through its own fault", if self.attempts == 1 { "it".to_string() } else { self.own_failures.to_string() }));
        }
        if self.fixes > 0 {
            out.push_str(&format!(", {}", plural(self.fixes, "fix", "fixes")));
        }
        out
    }
}

/// "Ask me at more steps": raising a low-risk run's risk, in the app and on the page.
pub const ASK_AT_MORE_STEPS: &str = "Ask me at more steps";
/// What raising the risk does: the full path, with more stops and the higher limit (core risk.rs and decide.rs).
pub const ASK_AT_MORE_STEPS_TIP: &str = "Takes this run on the full path: more steps and stops, and the time and token limit of a high-risk run.";
/// What abandoning a run already delivered does: the core's Abandon finishes the run, never undoes it.
pub const ABANDON_DELIVERED_TIP: &str = "Ends the run here. What it delivered stays as it is, and you can no longer correct it at its stops.";

/// Whether the high risk takes steps the run's risk leaves out: without them, raising it changes only the limit, and
/// "Ask me at more steps" is not offered.
pub fn risk_adds_steps(r: &Run) -> bool {
    r.manifest.path(jaynes_core::risk::Risk::High).len() > r.path().len()
}

/// The step a halted run was under way at (running, checking or probing); with none under way in its record, the
/// first on its path not done yet, where it goes on when resumed.
pub fn halted_step(r: &Run) -> Option<String> {
    use jaynes_core::run::Status;
    let path = r.path();
    let under_way = path.iter().copied().find(|&i| matches!(r.steps[i].status, Status::Running { .. } | Status::Checking { .. } | Status::Probing));
    under_way.or_else(|| path.iter().copied().find(|&i| r.steps[i].status != Status::Done)).map(|i| r.manifest.steps[i].name.clone())
}

/// A way out of a blocked run: what to do, what follows from it, and the command it takes in a terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Way {
    /// "Open this project's ports, then Continue."
    pub says: String,
    /// What follows from it, said under it.
    pub then: Option<String>,
    /// A command for the person's terminal, shown in a box with Copy.
    pub command: Option<String>,
    /// What stays open once the person takes it, under [`STILL_OPEN_HEADING`].
    pub still_open: Vec<String>,
}

/// What a blocked run's tab and the page say at its top, in parts the app and the page lay out alike, top to bottom:
/// where it stopped, what happened, why, what the run keeps, what the agent said there, what the person can do,
/// what Continue does, that the run waits, and the raw reason under a closed Details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockedSaid {
    /// The step it stopped at, when one did.
    pub step: Option<String>,
    /// "Blocked at “change”".
    pub title: String,
    /// What happened, said first: "The change is done. Jaynes could not check it."
    pub what: String,
    /// Why, in the person's words: never the box's own terms (JAYNES_PORTS, local_ports) nor the raw error, which
    /// are in `details`.
    pub why: String,
    /// What the run keeps: "No attempt was used.", when none was.
    pub holds: Option<String>,
    /// The agent's last words at the step: only what it said, never a tool's line, in at most three lines. Read by
    /// [`blocked`] from the transcripts; [`blocked_said`] has none.
    pub agent_said: Vec<String>,
    /// What the person can do, the best first; empty when Continue and Abandon are all there is.
    pub ways: Vec<Way>,
    /// The line under the buttons: "Continue tries “change” again, with its attempts back."
    pub continue_does: String,
    /// "The run waits here until you continue or abandon.", unless it goes on by itself.
    pub waits: Option<String>,
    /// The raw reason, each thing once: one said more than once ends with "(3 times)".
    pub details: Vec<String>,
}

/// The heading over what the agent said, and over the ways out.
pub const AGENT_SAID_HEADING: &str = "The agent said";
pub const WAYS_HEADING: &str = "What you can do";
/// The heading over what a way out leaves open.
pub const STILL_OPEN_HEADING: &str = "Still open while the tests run:";
/// The heading of the raw reason, closed and open.
pub const DETAILS_CLOSED: &str = "▸ Details";
pub const DETAILS_OPEN: &str = "▾ Details";

impl BlockedSaid {
    /// The card as plain text, top to bottom as the app and the page lay it out, Details closed.
    pub fn text(&self) -> String {
        let mut out = vec![self.title.clone(), self.what.clone(), self.why.clone()];
        out.extend(self.holds.clone());
        if !self.agent_said.is_empty() {
            out.push(AGENT_SAID_HEADING.into());
            out.extend(self.agent_said.iter().cloned());
        }
        if !self.ways.is_empty() {
            out.push(WAYS_HEADING.into());
            for w in &self.ways {
                out.push(w.says.clone());
                out.extend(w.then.clone());
                out.extend(w.command.clone());
                if !w.still_open.is_empty() {
                    out.push(STILL_OPEN_HEADING.into());
                    out.extend(w.still_open.iter().cloned());
                }
            }
        }
        out.push(self.continue_does.clone());
        out.extend(self.waits.clone());
        out.retain(|l| !l.is_empty());
        out.join("\n")
    }

    /// The words of Continue when the engine checks the change a check could not judge again, without the agent.
    pub fn checks_again(mut self) -> Self {
        self.continue_does = CONTINUE_CHECKS_AGAIN.into();
        self
    }
}

/// The words a check the box kept from judging is told in, by the box's check (jaynes_menu.py): the card finds its
/// parts by them.
const BOX_REFUSED: &str = "the box refused what the command tried, so its failure says nothing about the project: ";
const TRIED_PORT: &str = " It tried local port ";
const OWN_CHOOSING: &str = "0, a port of its own choosing";
const REACH_ONLY: &str = " A check's commands bind and reach only the ports in JAYNES_PORTS (";
const OPEN_DID_NOT_HELP: &str = " The person's open ports did not help here.";
const SERVERS_DID_NOT_HELP: &str = " The project's test servers did not help here.";
const OPEN_PORTS: &str = "`jaynes project open-ports ";
/// The words of a check whose test server hit a port Jaynes holds (jaynes_menu.py): never the project's doing.
const HELD_KEPT: &str = "Jaynes held a port the command tried to serve on, so its failure says nothing about the project: ";
const HELD_TRIED: &str = " It tried held port ";
const HELD_LET: &str = "`jaynes project test-servers ";

/// A blocked run's words, with what its agent said last at the step it stopped at; None when it is not blocked.
pub fn blocked(home: &Path, r: &Run) -> Option<BlockedSaid> {
    let Phase::Blocked { step, fault, reason } = &r.phase else { return None };
    let mut said = blocked_said(step.as_deref(), *fault, reason);
    if let Some(s) = step {
        said.agent_said = agent_said(home, &r.id, s);
    }
    // the engine keeps the change a check could not judge: Continue judges it again, the agent working no more
    if r.checks_again().is_some() {
        said = said.checks_again();
    }
    Some(said)
}

/// What the agent said last at a step of a run, from its transcripts: its last message, never a tool's line, in at
/// most three lines.
pub fn agent_said(home: &Path, run: &str, step: &str) -> Vec<String> {
    let dir = home.join("runs").join(run).join("steps").join(step);
    let when = |p: &Path| p.metadata().and_then(|m| m.modified()).ok();
    let mut turns: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|f| f.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("turn-")))
        .collect();
    turns.sort_by_key(|p| (when(p), p.clone()));
    let last = turns
        .iter()
        .filter_map(|t| tail(t, 1024 * 1024))
        .flat_map(|text| text.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).filter_map(|v| said_in(&v)).collect::<Vec<_>>())
        .last();
    let line = |l: &str| if l.chars().count() > 400 { format!("{}…", l.chars().take(399).collect::<String>()) } else { l.to_string() };
    last.map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).take(3).map(line).collect()).unwrap_or_default()
}

/// What the agent says in one line of Claude Code's stream, its text blocks together; None when it only calls tools.
fn said_in(v: &serde_json::Value) -> Option<String> {
    if v["type"].as_str() != Some("assistant") {
        return None;
    }
    let text: Vec<&str> = v["message"]["content"].as_array()?.iter().filter(|b| b["type"].as_str() == Some("text")).filter_map(|b| b["text"].as_str()).filter(|t| !t.trim().is_empty()).collect();
    (!text.is_empty()).then(|| text.join("\n"))
}

/// What the run keeps when Jaynes stopped it and no attempt was spent.
pub const NO_ATTEMPT_USED: &str = "No attempt was used.";
/// The last line of a blocked run's card, unless it goes on by itself.
pub const BLOCKED_WAITS: &str = "The run waits here until you continue or abandon.";
/// What Continue does when the engine checks the change a check could not judge again, without the agent.
pub const CONTINUE_CHECKS_AGAIN: &str = "Continue: Jaynes checks the same change again. The agent does not work again.";
/// The core's words when a block spent no attempt (decide.rs).
const UNSPENT: &str = "No attempt was spent";

/// This Mac, or the machine Jaynes runs on elsewhere.
fn this_machine() -> &'static str {
    if cfg!(target_os = "macos") { "this Mac" } else { "this machine" }
}

fn sentence(t: &str) -> String {
    let t = t.trim();
    let mut c = t.chars();
    let up = c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default();
    if up.is_empty() || up.ends_with(['.', '!', '?']) { up } else { format!("{up}.") }
}

/// A reason without the core's "No attempt was spent", wherever it says it, and whether it said it.
fn unspent(reason: &str) -> (String, bool) {
    let Some(at) = reason.find(UNSPENT) else { return (reason.to_string(), false) };
    let before = reason[..at].trim_end().trim_end_matches('.');
    let after = reason[at + UNSPENT.len()..].trim_start_matches('.').trim_start();
    (if after.is_empty() { before.to_string() } else { format!("{before}. {after}") }, true)
}

/// `s` split at `sep`, never inside curly quotes, where a label may say anything ("“Footer: links” was dropped").
fn split_outside_quotes<'a>(s: &'a str, sep: &str) -> Vec<&'a str> {
    let (mut out, mut depth, mut start, mut skip) = (Vec::new(), 0u32, 0, 0);
    for (i, c) in s.char_indices() {
        if i < skip {
            continue;
        }
        match c {
            '“' => depth += 1,
            '”' => depth = depth.saturating_sub(1),
            _ if depth == 0 && s[i..].starts_with(sep) => {
                out.push(&s[start..i]);
                start = i + sep.len();
                skip = start;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// What a failure says before its detail: "the agent exited with code 1" of "the agent exited with code 1: boom".
fn head_of(failure: &str) -> &str {
    split_outside_quotes(failure, ": ").first().copied().unwrap_or("").trim()
}

/// Each thing said once, in the order first said: one said more than once ends with "(3 times)".
fn once<'a>(items: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen: Vec<(&str, usize)> = Vec::new();
    for item in items.into_iter().map(str::trim).filter(|i| !i.is_empty()) {
        match seen.iter_mut().find(|(s, _)| *s == item) {
            Some((_, n)) => *n += 1,
            None => seen.push((item, 1)),
        }
    }
    seen.into_iter().map(|(s, n)| if n > 1 { format!("{s} ({n} times)") } else { s.to_string() }).collect()
}

/// A check that could not judge, from the core's reason: what its check said, and the files it was not given.
fn could_not_judge(reason: &str) -> Option<(&str, Option<&str>)> {
    let (_, inner) = reason.strip_prefix("the check ")?.split_once(" could not judge: ")?;
    const NOT_GIVEN: &str = "; files the run made that were not given to this check, since git ignores them and the delivery does not carry them: ";
    Some(match inner.rsplit_once(NOT_GIVEN) {
        Some((said, named)) => (said, Some(named)),
        None => (inner, None),
    })
}

/// What the box refused a check's command, from what the box's check said: the lines the command printed for it,
/// the local ports it tried (0 for one of its own choosing), the ports a check is given, and the project's folder.
struct Boxed<'a> {
    refused: Vec<&'a str>,
    ports: Vec<u16>,
    given: Vec<u16>,
    project: Option<&'a str>,
    open_did_not_help: bool,
    servers_did_not_help: bool,
}

fn boxed(said: &str) -> Option<Boxed<'_>> {
    let rest = said.strip_prefix(BOX_REFUSED)?;
    let end = [TRIED_PORT, REACH_ONLY, OPEN_DID_NOT_HELP, SERVERS_DID_NOT_HELP].iter().filter_map(|m| rest.find(m)).min().unwrap_or(rest.len());
    let (lines, tail) = rest.split_at(end);
    let numbers = |t: &str| t.split(", ").filter_map(|p| p.trim().parse().ok()).collect::<Vec<u16>>();
    Some(Boxed {
        refused: lines.split("; ").map(str::trim).filter(|l| !l.is_empty()).collect(),
        ports: tail.split_once(TRIED_PORT).map(|(_, t)| numbers(t.replace(OWN_CHOOSING, "0").split('.').next().unwrap_or(""))).unwrap_or_default(),
        given: tail.split_once(REACH_ONLY).and_then(|(_, t)| t.split_once(')')).map(|(g, _)| numbers(g)).unwrap_or_default(),
        project: tail.split_once(OPEN_PORTS).and_then(|(_, t)| t.split_once('`')).map(|(p, _)| p),
        open_did_not_help: tail.contains(OPEN_DID_NOT_HELP),
        servers_did_not_help: tail.contains(SERVERS_DID_NOT_HELP),
    })
}

/// A check whose test server hit ports Jaynes holds, from what the box's check said: the lines it printed for it, the
/// ports, and the project's folder.
struct Held<'a> {
    lines: Vec<&'a str>,
    ports: Vec<u16>,
    project: Option<&'a str>,
}

fn held(said: &str) -> Option<Held<'_>> {
    let rest = said.strip_prefix(HELD_KEPT)?;
    let (lines, tail) = rest.split_at(rest.find(HELD_TRIED)?);
    let ports = tail[HELD_TRIED.len()..].split('.').next().unwrap_or("").split(", ").filter_map(|p| p.trim().parse().ok()).collect();
    Some(Held {
        lines: lines.split("; ").map(str::trim).filter(|l| !l.is_empty()).collect(),
        ports,
        project: tail.split_once(HELD_LET).and_then(|(_, t)| t.split_once('`')).map(|(p, _)| p),
    })
}

/// A word for the person's shell: as it is when it needs no quotes, else in single quotes.
fn shell_word(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-~+@%:,=".contains(c)) { s.to_string() } else { format!("'{}'", s.replace('\'', r"'\''")) }
}

/// Ports as a person reads them: "65520 to 65527" when one after another, else "5432, 6379".
fn port_range(ports: &[u16]) -> String {
    let mut p = ports.to_vec();
    p.sort_unstable();
    match (p.first(), p.last()) {
        (Some(a), Some(b)) if p.len() > 1 && p.windows(2).all(|w| w[1] == w[0] + 1) => format!("{a} to {b}"),
        _ => p.iter().map(u16::to_string).collect::<Vec<_>>().join(", "),
    }
}

/// "port 5432", "ports 5432 and 6379".
pub fn ports_named(ports: &[u16]) -> String {
    let p: Vec<String> = ports.iter().map(u16::to_string).collect();
    match p.as_slice() {
        [one] => format!("port {one}"),
        [rest @ .., last] => format!("ports {} and {last}", rest.join(", ")),
        [] => String::new(),
    }
}

/// What letting the project's tests use held `ports` risks, where the person lets them and on the card (the
/// design-lead's words of 22/11): the tests reach a service of theirs there, or stand in for it.
pub fn freed_risk(ports: &[u16]) -> String {
    let (them, their) = if ports.len() == 1 { ("it", "its") } else { ("them", "their") };
    format!("Only if nothing of yours runs on {} while the tests run: the tests could reach {them}, or answer in {their} place.", ports_bare(ports))
}

/// "5432", "5432 and 6379": ports named without the word.
pub fn ports_bare(ports: &[u16]) -> String {
    ports_named(ports).trim_start_matches("ports ").trim_start_matches("port ").to_string()
}

/// The files git ignores that a check was not given, as Details says it.
fn not_given_said(named: &str) -> String {
    if named.contains(", ") || named.contains(" and ") {
        format!("{named} were not given to the check: git ignores them, and the delivery does not carry them.")
    } else {
        format!("{named} was not given to the check: git ignores it, and the delivery does not carry it.")
    }
}

/// Whether `jaynes project test-servers` is in this Jaynes: the rd-engineer's window for a test server on a port the
/// system picks (macOS's box). Until it is, such a port gets the project's open ports, since a card never says to
/// run a command that does not exist; the commit that brings the command turns this on.
const TEST_SERVERS: bool = true;

/// The ports of the usual databases, caches and queues that macOS's box holds, where free, while a project's own test
/// servers run, so a test cannot stand in for a service of the person's that is not running; with what each is for.
/// The box's list (jaynes_box.py HELD_PORTS), in its order, the best known first: a test checks both say the same.
/// The ports a test brings its own app up on (3000, 5173, 8000, 8080...) are not here.
pub const HELD_PORTS: &[(u16, &str)] = &[
    (5432, "database"),
    (3306, "database"),
    (6379, "cache"),
    (27017, "database"),
    (11211, "cache"),
    (9200, "database"),
    (5672, "queue"),
    (9092, "queue"),
    (1433, "database"),
    (1521, "database"),
    (26257, "database"),
    (9042, "database"),
    (5984, "database"),
    (7687, "database"),
    (4222, "queue"),
    (2379, "database"),
    (8200, "secrets"),
];

/// The ports Jaynes holds, as a person reads them: the first four, then "and 13 more".
pub fn held_ports_said() -> String {
    let first: Vec<String> = HELD_PORTS.iter().take(4).map(|(p, _)| p.to_string()).collect();
    match HELD_PORTS.len().saturating_sub(4) {
        0 => first.join(", "),
        more => format!("{} and {more} more", first.join(", ")),
    }
}

/// What the held `ports` stand for, in "no test stands in for …": "a database", "a cache", else "a service".
pub fn held_kind(ports: &[u16]) -> &'static str {
    let kinds: std::collections::BTreeSet<&str> = ports.iter().map(|p| HELD_PORTS.iter().find(|(h, _)| h == p).map(|(_, k)| *k).unwrap_or("")).collect();
    match kinds.into_iter().collect::<Vec<_>>().as_slice() {
        ["database"] => "a database",
        ["cache"] => "a cache",
        _ => "a service",
    }
}

/// What a project's test servers say of the held ports the person let its tests use: "; its tests may also use port
/// 5432", or nothing.
pub fn freed_said(freed: &[u16]) -> String {
    if freed.is_empty() { String::new() } else { format!("; its tests may also use {}", ports_named(freed)) }
}

/// What stays open while a project's tests run with their own test servers, in the person's words: on the card and
/// where the person chooses (`jaynes project test-servers`), the same.
pub fn test_servers_still_open() -> Vec<String> {
    let mac = this_machine();
    vec![
        format!("A test server listens on every network of {mac}, so another machine on your network could reach it."),
        format!("A service that starts on {mac} while the tests run may get a port they can reach."),
        format!("A test server may take the port of a service of {mac} that is not running. Jaynes holds the ports of the usual databases and caches ({}) while the checks run, so a test cannot stand in for those; for any other service, it could.", held_ports_said()),
    ]
}

/// The way out for a command the box kept from a local port: the project's test servers (`own`, a port the system
/// picks) or its open ports.
fn port_way(project: &str, own: bool) -> Way {
    let mac = this_machine();
    if own {
        Way {
            says: "Let this project's tests start their own test servers, then Continue.".into(),
            then: Some("As when you run the tests yourself, but they still cannot reach your services or the Jaynes page.".into()),
            command: Some(format!("jaynes project test-servers {}", shell_word(project))),
            still_open: test_servers_still_open(),
        }
    } else {
        Way {
            says: "Open this project's ports, then Continue.".into(),
            then: Some(format!("Its checks may then reach any service on {mac}, the Jaynes page among them.")),
            command: Some(format!("jaynes project open-ports {}", shell_word(project))),
            still_open: Vec::new(),
        }
    }
}

/// A check that could not judge: the change is done and Jaynes could not check it. When the box kept the check's
/// command from a local port, why in the person's words and the way out that serves what it tried; the box's own
/// terms and the raw error go under Details.
fn judged(said: &mut BlockedSaid, inner: &str, not_given: Option<&str>) {
    said.what = "The change is done. Jaynes could not check it.".into();
    said.holds = Some(NO_ATTEMPT_USED.into());
    let mut details = Vec::new();
    if let Some(h) = held(inner).filter(|h| !h.ports.is_empty()) {
        // the design-lead's words of 22/11: it was Jaynes, which port, and how to let it
        said.why = format!(
            "The project's tests start a test server on {}, which Jaynes holds while the checks run so no test stands in for {}.",
            ports_named(&h.ports),
            held_kind(&h.ports)
        );
        if let Some(project) = h.project {
            said.ways.push(Way {
                says: format!("Let this project's tests use {}, then Continue.", ports_named(&h.ports)),
                then: Some(freed_risk(&h.ports)),
                command: Some(format!("jaynes project test-servers {} {}", shell_word(project), h.ports.iter().map(u16::to_string).collect::<Vec<_>>().join(" "))),
                still_open: Vec::new(),
            });
        }
        said.ways.push(Way { says: "Or abandon: the run ends and its change is not kept. Your project stays as it was.".into(), then: None, command: None, still_open: Vec::new() });
        details.extend(once(h.lines.iter().copied()));
        details.push(format!("It tried {}, which Jaynes holds while the checks run.", ports_named(&h.ports)));
        details.extend(not_given.map(not_given_said));
        said.details = details;
        return;
    }
    match boxed(inner) {
        Some(b) if !b.ports.is_empty() && !b.open_did_not_help => {
            let fixed: Vec<u16> = b.ports.iter().copied().filter(|&p| p != 0).collect();
            let mac = this_machine();
            // one way out, the one that serves what the command tried, never two: a port the system picks gets the
            // project's test servers (macOS's box, the rd-engineer's), a fixed one the project's open ports
            let own = fixed.is_empty() && cfg!(target_os = "macos") && TEST_SERVERS && !b.servers_did_not_help;
            said.why = if fixed.is_empty() {
                "The project's tests start a test server on a port the system picks, and the box Jaynes checks in allows only its own ports.".into()
            } else {
                format!("The project's tests reach {} on {mac}, and the box Jaynes checks in allows only its own ports.", ports_named(&fixed))
            };
            if let Some(project) = b.project {
                said.ways.push(port_way(project, own));
            }
            said.ways.push(Way { says: "Or abandon: the run ends and its change is not kept. Your project stays as it was.".into(), then: None, command: None, still_open: Vec::new() });
            details.extend(once(b.refused.iter().map(|l| go_annotation_off(l))));
            let tried: Vec<String> = b.ports.iter().map(|&p| if p == 0 { OWN_CHOOSING.to_string() } else { p.to_string() }).collect();
            let mut ports = format!("It tried port{} {}.", if tried.len() == 1 { "" } else { "s" }, tried.join(", "));
            if !b.given.is_empty() {
                ports.push_str(&format!(" A check's commands reach only the ports in JAYNES_PORTS ({}) and the ones the check names in its local_ports.", port_range(&b.given)));
            }
            details.push(ports);
            if !b.given.is_empty() {
                details.push("Or the suite takes its test servers' ports from JAYNES_PORTS, and the box stays closed.".into());
            }
        }
        Some(b) => {
            // the person's words for the box's own (the design-lead's, 21/11): what was not judged is the change
            said.why = "The project's checks tried something the box Jaynes checks in does not allow, so their failure says nothing about the change.".into();
            details.extend(once(b.refused.iter().map(|l| go_annotation_off(l))));
            if b.open_did_not_help {
                details.push(OPEN_DID_NOT_HELP.trim().into());
            }
            if b.servers_did_not_help {
                details.push(SERVERS_DID_NOT_HELP.trim().into());
            }
        }
        None => match head_of(inner) {
            first if first.len() < inner.trim().len() => {
                said.why = sentence(first);
                details.push(inner.trim().to_string());
            }
            _ => said.why = sentence(inner),
        },
    }
    details.extend(not_given.map(not_given_said));
    said.details = details;
}

/// A line Go printed without the note it adds to a panic recovered and raised again ("[recovered, repanicked]").
fn go_annotation_off(line: &str) -> &str {
    match line.rfind(" [recovered") {
        Some(at) if line.ends_with(']') => &line[..at],
        _ => line,
    }
}

/// A blocked run's words, from the core's (`Phase::Blocked`), in parts (see [`BlockedSaid`]): the step's name always
/// in curly quotes, since a role's "change" or "fix" reads as a verb. What happened comes first, in the words of its
/// fault; "change spent its 3 attempts: the agent exited with code 1: …" says why as "“change” used its 3 attempts.
/// The agent exited with code 1."; a check that could not judge says why in the person's words; any other reason is
/// the engine's, as a sentence. Continue gives an agent's attempts back (the core's Continued sets them to 0 for
/// Fault::Agent), else goes on.
pub fn blocked_said(step: Option<&str>, fault: jaynes_core::event::Fault, reason: &str) -> BlockedSaid {
    use jaynes_core::event::Fault;
    let quoted = |s: &str| format!("“{s}”");
    // a record keeps a reason as the engine said it then ("exited with Some(1)"): read as it says it now
    let (reason, unspent) = unspent(&in_words(reason));
    let reason = reason.as_str();
    let at = step.map(quoted).unwrap_or_else(|| "the run".into());
    let mut said = BlockedSaid {
        step: step.map(str::to_string),
        title: step.map(|s| format!("Blocked at {}", quoted(s))).unwrap_or_else(|| "Blocked".into()),
        what: String::new(),
        why: sentence(reason),
        holds: (unspent || fault == Fault::Jaynes).then(|| NO_ATTEMPT_USED.to_string()),
        agent_said: Vec::new(),
        ways: Vec::new(),
        continue_does: match step {
            Some(s) if fault == Fault::Agent => format!("Continue tries {} again, with its attempts back.", quoted(s)),
            Some(s) => format!("Continue goes on from {}.", quoted(s)),
            None => "Continue goes on.".into(),
        },
        waits: Some(BLOCKED_WAITS.into()),
        details: Vec::new(),
    };
    // the engine's reason after the words that say what happened, which the card says first
    let after = |prefix: Option<String>| prefix.and_then(|p| reason.strip_prefix(p.as_str()).map(sentence));
    match fault {
        Fault::Agent => {
            said.what = format!("The agent could not finish {at}.");
            let spent = step.and_then(|s| reason.strip_prefix(&format!("{s} spent its ")).and_then(|rest| rest.split_once(" attempts")).map(|(n, failures)| (s, n, failures.trim_start_matches(':'))));
            if let Some((s, n, failures)) = spent {
                // the first failure's own words, before its detail: "the agent exited with code 1"; a label in
                // curly quotes is never cut
                let failures = split_outside_quotes(failures, "; ");
                let head = head_of(failures.first().copied().unwrap_or(""));
                said.why = if head.is_empty() { format!("{} used its {n} attempts.", quoted(s)) } else { format!("{} used its {n} attempts. {}", quoted(s), sentence(head)) };
                said.details = once(failures.iter().copied());
                if said.details == [head] {
                    said.details.clear();
                }
            }
        }
        Fault::Jaynes => match could_not_judge(reason) {
            Some((inner, not_given)) => judged(&mut said, inner, not_given),
            None => {
                said.what = format!("Jaynes or {} stopped {at}. It is not the agent's fault.", this_machine());
                if let Some(why) = after(step.map(|s| format!("a fault of Jaynes or of the machine stopped {s}: "))) {
                    said.why = why;
                }
            }
        },
        Fault::Role => {
            said.what = format!("The role says {at} cannot be done.");
            if let Some(why) = after(step.map(|s| format!("the role says {s} cannot be done: "))) {
                said.why = why;
            }
        }
        Fault::Ceiling => said.what = "The run reached its time or token limit.".into(),
        Fault::MayHaveActed => said.what = sentence(&format!("{at} may have acted before Jaynes stopped")),
        Fault::Paused => {
            said.what = "The agent program paused, for its quota or from outside. The run goes on when it passes.".into();
            said.waits = None;
        }
    }
    said
}

#[cfg(test)]
mod blocked_tests {
    use super::*;
    use jaynes_core::event::Fault;

    /// This Mac, or the machine Jaynes runs on elsewhere.
    fn machine() -> &'static str {
        if cfg!(target_os = "macos") { "this Mac" } else { "this machine" }
    }

    /// The reason the core keeps for a check the box kept from judging, as the run of 21/11 said it.
    fn refused_port_0(project: &str) -> String {
        let panic = "panic: httptest: failed to listen on a port: listen tcp6 [::1]:0: bind: operation not permitted [recovered, repanicked]";
        refused(&format!("{panic}; {panic}; {panic}"), "0, a port of its own choosing", project)
    }

    /// The reason for a check whose command printed `lines` when the box refused it the local ports `tried`.
    fn refused(lines: &str, tried: &str, project: &str) -> String {
        let not_given = crate::project::not_given(&[".claude/".to_string()]).expect("a sentence");
        format!(
            "the check suite-passes of change could not judge: the box refused what the command tried, so its failure says nothing about the project: {lines} It tried local port {tried}. A check's commands bind and reach only the ports in JAYNES_PORTS (65520, 65521, 65522, 65523, 65524, 65525, 65526, 65527) and the ones the check names in its local_ports, never its step's. Two ways out, the first the better: (1) the suite takes its test servers' ports from JAYNES_PORTS, and the box stays closed; (2) the person opens this project with `jaynes project open-ports {project}`, and its checks then reach every service listening on this machine, the Jaynes page among them.; {not_given}. No attempt was spent"
        )
    }

    fn said(text: &str) -> String {
        serde_json::json!({"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}}).to_string()
    }

    fn tool(name: &str, command: &str) -> String {
        serde_json::json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": name, "input": {"command": command}}]}}).to_string()
    }

    // Alan, 21/11: "confuso pra caralho"
    #[test]
    fn asked_blocked_check_says_what_happened_first() {
        let home = std::env::temp_dir().join(format!("jaynes-blocked-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        let step = home.join("runs/r1/steps/change");
        std::fs::create_dir_all(&step).expect("dir");
        let turn = [
            r#"{"type":"system","subtype":"init"}"#.to_string(),
            said("I will run the whole suite first."),
            tool("Bash", "go test ./..."),
            said("Done. The whole suite passes."),
            tool("Bash", "echo $?"),
            r#"{"type":"result","subtype":"success"}"#.to_string(),
        ];
        std::fs::write(step.join("turn-1-0.jsonl"), turn.join("\n") + "\n--- stderr ---\n").expect("write");
        let mut r = super::tests::run("r1", "Show the plan's log");
        r.phase = Phase::Blocked { step: Some("change".into()), fault: Fault::Jaynes, reason: refused_port_0("/Users/x/Projects/app") };

        let b = blocked(&home, &r).expect("blocked");
        let _gone = std::fs::remove_dir_all(&home);

        // what happened comes first, under the title
        assert_eq!(b.title, "Blocked at “change”");
        assert_eq!(b.what, "The change is done. Jaynes could not check it.");
        assert_eq!(b.text().lines().take(2).collect::<Vec<_>>(), ["Blocked at “change”", "The change is done. Jaynes could not check it."]);
        // why in the person's words: no box terms, no raw error
        assert_eq!(b.why, "The project's tests start a test server on a port the system picks, and the box Jaynes checks in allows only its own ports.");
        for word in ["JAYNES_PORTS", "local_ports", "panic"] {
            assert!(!b.why.contains(word), "{word} in the why: {}", b.why);
        }
        assert_eq!(b.holds.as_deref(), Some("No attempt was used."));
        // the agent's words, never a tool's line
        assert_eq!(b.agent_said, ["Done. The whole suite passes."]);
        assert!(!b.agent_said.iter().any(|l| ["Bash", "Read", "Edit", "Write", "Grep"].iter().any(|t| l.starts_with(t))), "{:?}", b.agent_said);
        // the ways out, one a line, the best first: for a port the system picks, the project's test servers
        // (macOS's box), never its open ports too
        let first = port_way("/Users/x/Projects/app", TEST_SERVERS && cfg!(target_os = "macos"));
        assert_eq!(b.ways, [first, abandon()]);
        assert_eq!(b.waits.as_deref(), Some("The run waits here until you continue or abandon."));
        // the raw reason under Details, the panic once
        assert_eq!(b.details, [
            "panic: httptest: failed to listen on a port: listen tcp6 [::1]:0: bind: operation not permitted (3 times)",
            "It tried port 0, a port of its own choosing. A check's commands reach only the ports in JAYNES_PORTS (65520 to 65527) and the ones the check names in its local_ports.",
            "Or the suite takes its test servers' ports from JAYNES_PORTS, and the box stays closed.",
            ".claude/ was not given to the check: git ignores it, and the delivery does not carry it.",
        ]);
        assert_eq!(b.details.iter().filter(|d| d.contains("panic")).count(), 1);
        // and nothing raw on the card while Details is closed
        assert!(!b.text().contains("panic") && !b.text().contains("JAYNES_PORTS"), "{}", b.text());
    }

    fn open_ports(project: &str) -> Way {
        Way {
            says: "Open this project's ports, then Continue.".into(),
            then: Some(format!("Its checks may then reach any service on {}, the Jaynes page among them.", machine())),
            command: Some(format!("jaynes project open-ports {project}")),
            still_open: Vec::new(),
        }
    }

    fn abandon() -> Way {
        Way { says: "Or abandon: the run ends and its change is not kept. Your project stays as it was.".into(), then: None, command: None, still_open: Vec::new() }
    }

    #[test]
    fn a_test_that_reaches_a_fixed_port_says_the_port_and_opens_the_project_s_ports() {
        let dial = "dial tcp 127.0.0.1:5432: connect: operation not permitted";
        let b = blocked_said(Some("change"), Fault::Jaynes, &refused(dial, "5432", "/Users/x/Projects/app"));
        assert_eq!(b.why, format!("The project's tests reach port 5432 on {}, and the box Jaynes checks in allows only its own ports.", machine()));
        assert_eq!(b.ways, [open_ports("/Users/x/Projects/app"), abandon()]);
        assert_eq!(b.details[..2], [dial.to_string(), "It tried port 5432. A check's commands reach only the ports in JAYNES_PORTS (65520 to 65527) and the ones the check names in its local_ports.".to_string()]);
        // a fixed port beside one the system picks: the open ports serve both, and only they are offered
        let both = blocked_said(Some("change"), Fault::Jaynes, &refused(dial, "5432, 0, a port of its own choosing", "/p"));
        assert_eq!(both.why, format!("The project's tests reach port 5432 on {}, and the box Jaynes checks in allows only its own ports.", machine()));
        assert_eq!(both.ways, [open_ports("/p"), abandon()]);
    }

    #[test]
    fn a_folder_with_a_space_is_quoted_in_the_command_to_copy() {
        let b = blocked_said(Some("change"), Fault::Jaynes, &refused_port_0("/Users/x/My Projects/app"));
        let command = b.ways[0].command.clone().unwrap_or_default();
        assert!(command.ends_with(" '/Users/x/My Projects/app'"), "{command}");
    }

    #[test]
    fn continue_checks_the_same_change_again_once_the_engine_keeps_it() {
        use jaynes_core::event::{Event, Usage};
        let home = std::env::temp_dir().join(format!("jaynes-checks-again-{}", std::process::id()));
        let mut r = super::tests::run("r1", "Filter the items by price");
        let started = Event::AttemptStarted { step: "change".into(), round: 0, attempt: 1, fix: 0, session: None, deadline_seconds: 0, tokens_left: 0, key: None };
        let delivered = Event::AttemptDelivered { step: "change".into(), attempt: 1, fix: 0, output: jaynes_core::Digest("o".into()), items: vec![], changed: vec![], usage: Usage::default(), session: None, tree: Some("t".into()), reply: None };
        let blocked_at = Event::Blocked { step: Some("change".into()), fault: Fault::Jaynes, reason: refused_port_0("/p") };
        for e in [&started, &delivered, &blocked_at] {
            r.apply(e);
        }
        // a block the engine does not keep the change of: Continue as before
        assert_eq!(r.checks_again(), None);
        assert_eq!(blocked(&home, &r).expect("blocked").continue_does, "Continue goes on from “change”.");
        // the check could not judge the change, which the engine keeps: Continue checks it again, the agent idle
        let mut r = super::tests::run("r1", "Filter the items by price");
        for e in [&started, &delivered, &Event::ChecksUnjudged { step: "change".into(), attempt: 1, fix: 0 }, &blocked_at] {
            r.apply(e);
        }
        assert!(r.checks_again().is_some());
        let b = blocked(&home, &r).expect("blocked");
        assert_eq!(b.continue_does, "Continue: Jaynes checks the same change again. The agent does not work again.");
        assert_eq!(b.what, "The change is done. Jaynes could not check it.");
    }

    #[test]
    fn a_port_the_system_picks_offers_test_servers_only_once_jaynes_has_them() {
        // `jaynes project test-servers` is the rd-engineer's, not yet in main: a card never says to run a command
        // that does not exist, so until TEST_SERVERS a port the system picks gets the project's open ports
        let b = blocked_said(Some("change"), Fault::Jaynes, &refused_port_0("/Users/x/Projects/app"));
        let first = if TEST_SERVERS && cfg!(target_os = "macos") { port_way("/Users/x/Projects/app", true) } else { open_ports("/Users/x/Projects/app") };
        assert_eq!(b.ways, [first, abandon()]);
        // and the day it comes, its words, with what stays open
        assert_eq!(port_way("/Users/x/Projects/app", true), Way {
            says: "Let this project's tests start their own test servers, then Continue.".into(),
            then: Some("As when you run the tests yourself, but they still cannot reach your services or the Jaynes page.".into()),
            command: Some("jaynes project test-servers /Users/x/Projects/app".into()),
            still_open: vec![
                format!("A test server listens on every network of {}, so another machine on your network could reach it.", machine()),
                format!("A service that starts on {} while the tests run may get a port they can reach.", machine()),
                test_servers_still_open()[2].clone(),
            ],
        });
        assert_eq!(port_way("/Users/x/Projects/app", false), open_ports("/Users/x/Projects/app"));
    }

    /// The reason for a check whose command hit ports Jaynes held, as the box's check says it.
    fn hit_held(lines: &str, ports: &str, project: &str) -> String {
        format!(
            "the check suite-passes of change could not judge: Jaynes held a port the command tried to serve on, so its failure says nothing about the project: {lines} It tried held port {}. The person lets this project's tests use them with `jaynes project test-servers {project}`, naming the ports.. No attempt was spent",
            ports.replace(' ', ", ")
        )
    }

    #[test]
    fn a_test_server_on_a_port_jaynes_holds_says_it_was_jaynes_and_how_to_let_it() {
        // the design-lead's words of 22/11: it was Jaynes, which port, and the command to let it
        let bind = "listen tcp 127.0.0.1:5432: bind: address already in use";
        let b = blocked_said(Some("change"), Fault::Jaynes, &hit_held(bind, "5432", "/Users/x/Projects/app"));
        assert_eq!(b.what, "The change is done. Jaynes could not check it.");
        assert_eq!(b.why, "The project's tests start a test server on port 5432, which Jaynes holds while the checks run so no test stands in for a database.");
        assert_eq!(b.ways, [
            Way {
                says: "Let this project's tests use port 5432, then Continue.".into(),
                then: Some("Only if nothing of yours runs on 5432 while the tests run: the tests could reach it, or answer in its place.".into()),
                command: Some("jaynes project test-servers /Users/x/Projects/app 5432".into()),
                still_open: Vec::new(),
            },
            abandon(),
        ]);
        assert_eq!(b.details, [bind.to_string(), "It tried port 5432, which Jaynes holds while the checks run.".to_string()]);
        assert_eq!(b.holds.as_deref(), Some(NO_ATTEMPT_USED));
        // a cache says so, and a folder with a space is quoted
        let cache = blocked_said(Some("change"), Fault::Jaynes, &hit_held("Error: listen EADDRINUSE: address already in use :::6379", "6379", "/Users/x/my app"));
        assert!(cache.why.ends_with("so no test stands in for a cache."), "{}", cache.why);
        assert_eq!(cache.ways[0].command.as_deref(), Some("jaynes project test-servers '/Users/x/my app' 6379"));
        // two ports are named together, and a database with a cache is a service
        let two = blocked_said(Some("change"), Fault::Jaynes, &hit_held(bind, "5432 6379", "/Users/x/Projects/app"));
        assert_eq!(two.why, "The project's tests start a test server on ports 5432 and 6379, which Jaynes holds while the checks run so no test stands in for a service.");
        assert_eq!(two.ways[0].says, "Let this project's tests use ports 5432 and 6379, then Continue.");
        assert_eq!(two.ways[0].then.as_deref(), Some("Only if nothing of yours runs on 5432 and 6379 while the tests run: the tests could reach them, or answer in their place."));
        assert_eq!(two.ways[0].command.as_deref(), Some("jaynes project test-servers /Users/x/Projects/app 5432 6379"));
        assert_eq!(two.details[1], "It tried ports 5432 and 6379, which Jaynes holds while the checks run.");
    }

    #[test]
    fn what_stays_open_names_the_ports_jaynes_holds() {
        let held: Vec<String> = HELD_PORTS.iter().map(|(p, _)| p.to_string()).collect();
        assert_eq!(&held[..4], ["5432", "3306", "6379", "27017"]);
        assert_eq!(test_servers_still_open()[2], format!(
            "A test server may take the port of a service of {} that is not running. Jaynes holds the ports of the usual databases and caches (5432, 3306, 6379, 27017 and {} more) while the checks run, so a test cannot stand in for those; for any other service, it could.",
            machine(), HELD_PORTS.len() - 4));
        // none of the ports a test brings its own app up on
        for web in [3000, 4200, 5000, 5173, 8000, 8080, 8443, 8888] {
            assert!(HELD_PORTS.iter().all(|(p, _)| *p != web), "{web}");
        }
    }

    #[test]
    fn the_box_holds_the_ports_this_list_names() {
        // the box (jaynes_box.py) holds them, the card names them: one list, written in both
        let box_py = include_str!("../../../python/jaynes_box.py");
        let at = box_py.find("HELD_PORTS = {").expect("HELD_PORTS in jaynes_box.py");
        let body = &box_py[at..at + box_py[at..].find('}').expect("end")];
        let in_box: Vec<(u16, String)> = body.lines().skip(1).filter_map(|l| {
            let (port, kind) = l.trim().split_once(':')?;
            Some((port.trim().parse().ok()?, kind.split('"').nth(1)?.to_string()))
        }).collect();
        let here: Vec<(u16, String)> = HELD_PORTS.iter().map(|(p, k)| (*p, k.to_string())).collect();
        assert_eq!(in_box, here);
    }

    #[test]
    fn every_block_says_what_happened_first_in_its_own_words() {
        let what = |step: Option<&str>, fault, reason: &str| blocked_said(step, fault, reason);
        let agent = what(Some("change"), Fault::Agent, "change spent its 3 attempts: the agent exited with code 1: boom; the agent exited with code 1: boom");
        assert_eq!(agent.what, "The agent could not finish “change”.");
        assert_eq!(agent.why, "“change” used its 3 attempts. The agent exited with code 1.");
        assert_eq!(agent.details, ["the agent exited with code 1: boom (2 times)"]);
        assert_eq!(agent.holds, None);
        assert_eq!(agent.continue_does, "Continue tries “change” again, with its attempts back.");
        // a failure with nothing more than the why says has no details
        assert!(what(Some("fix"), Fault::Agent, "fix spent its 2 attempts: the agent exited with code 1").details.is_empty());

        let other = what(Some("change"), Fault::Jaynes, "a fault of Jaynes or of the machine stopped change: the agent program did not start. No attempt was spent");
        assert_eq!(other.what, format!("Jaynes or {} stopped “change”. It is not the agent's fault.", machine()));
        assert_eq!(other.why, "The agent program did not start.");
        assert_eq!(other.holds.as_deref(), Some("No attempt was used."));
        assert_eq!(other.continue_does, "Continue goes on from “change”.");
        // said in the middle, it leaves the rest whole
        let acted = what(Some("send"), Fault::Jaynes, "a fault of Jaynes or of the machine stopped send: lost. No attempt was spent. send acts on something real and its outcome is not known, so it may have acted: continuing asks you whether to act again");
        assert_eq!(acted.why, "Lost. send acts on something real and its outcome is not known, so it may have acted: continuing asks you whether to act again.");

        // a check that could not judge for another cause: the first clause, the rest under Details
        let unjudged = what(Some("change"), Fault::Jaynes, "the check suite of change could not judge: there is no box to run the project's commands in: sandbox-exec: not found. No attempt was spent");
        assert_eq!(unjudged.what, "The change is done. Jaynes could not check it.");
        assert_eq!(unjudged.why, "There is no box to run the project's commands in.");
        assert_eq!(unjudged.details, ["there is no box to run the project's commands in: sandbox-exec: not found"]);
        assert!(unjudged.ways.is_empty());

        let role = what(Some("change"), Fault::Role, "the role says change cannot be done: the brief names no page");
        assert_eq!((role.what.as_str(), role.why.as_str()), ("The role says “change” cannot be done.", "The brief names no page."));
        let ceiling = what(Some("change"), Fault::Ceiling, "the run reached its ceiling: 3600 of 3600 seconds, 10 of 900000 tokens");
        assert_eq!(ceiling.what, "The run reached its time or token limit.");
        assert_eq!(ceiling.why, "The run reached its ceiling: 3600 of 3600 seconds, 10 of 900000 tokens.");
        let may = what(Some("send"), Fault::MayHaveActed, "send, which acts on something real, was stopped from outside before its outcome was known: it may have acted (killed). Look, then continue or abandon the run");
        assert_eq!(may.what, "“send” may have acted before Jaynes stopped.");
        let paused = what(Some("change"), Fault::Paused, "change was stopped from outside: the agent program's quota");
        assert_eq!(paused.what, "The agent program paused, for its quota or from outside. The run goes on when it passes.");
        // a paused run goes on by itself, so it does not say it waits
        assert_eq!(paused.waits, None);
        for b in [&agent, &other, &unjudged, &role, &ceiling, &may] {
            assert_eq!(b.waits.as_deref(), Some("The run waits here until you continue or abandon."), "{b:?}");
        }
        let none = what(None, Fault::Role, "nothing to deliver.");
        assert_eq!((none.title.as_str(), none.why.as_str(), none.continue_does.as_str()), ("Blocked", "Nothing to deliver.", "Continue goes on."));
    }

    #[test]
    fn a_box_refusal_with_no_port_says_the_change_was_not_judged() {
        let b = blocked_said(Some("change"), Fault::Jaynes, "the check suite of change could not judge: the box refused what the command tried, so its failure says nothing about the project: deny file-write* /etc/hosts. No attempt was spent");
        assert_eq!(b.why, "The project's checks tried something the box Jaynes checks in does not allow, so their failure says nothing about the change.");
        assert_eq!(b.details, ["deny file-write* /etc/hosts"]);
    }

    #[test]
    fn a_label_in_curly_quotes_is_never_cut_in_the_middle() {
        // the copy-review of the scene: the agent gives back an item the person dropped, three times
        let gone = "“Footer: quieter links” was dropped by the person and may not come back";
        let b = blocked_said(Some("copy"), Fault::Agent, &format!("copy spent its 3 attempts: {gone}; {gone}; {gone}"));
        assert_eq!(b.why, format!("“copy” used its 3 attempts. {gone}."));
        assert_eq!(b.details, [format!("{gone} (3 times)")]);
        // a label with "; " in it stays one failure, and its detail after the quotes is still cut off the why
        let semi = "the check “a; b: c” failed: 2 of 3";
        let s2 = blocked_said(Some("copy"), Fault::Agent, &format!("copy spent its 2 attempts: {semi}"));
        assert_eq!(s2.why, "“copy” used its 2 attempts. The check “a; b: c” failed.");
        assert_eq!(s2.details, [semi]);
        let unjudged = blocked_said(Some("copy"), Fault::Jaynes, "the check fits of copy could not judge: “Plan: v2” has no picture: none found. No attempt was spent");
        assert_eq!(unjudged.why, "“Plan: v2” has no picture.");
    }

    // uso real, 21/11: "Os contadores se contradizem: "2 attempts, 0 spent by the agent" ao lado de "blocked, 1 attempt";
    // depois do Continue, "3 attempts, 0 spent by the agent, 1 fixes", com o agente tendo trabalhado duas vezes."
    #[test]
    fn the_counters_never_contradict_each_other() {
        use jaynes_core::event::{Event, Usage};
        use jaynes_core::manifest::Executor;
        use jaynes_core::question::By;
        let mut r = super::tests::run("r1", "Filter the items by price");
        // a step Jaynes does itself first (the role's runner), then the agent's
        r.manifest.steps[0].executor = Executor::Role;
        let started = |step: &str, attempt: u32| Event::AttemptStarted { step: step.into(), round: 0, attempt, fix: 0, session: None, deadline_seconds: 0, tokens_left: 0, key: None };
        for e in [
            started("plan", 1),
            Event::StepDone { step: "plan".into(), round: 0, output: jaynes_core::Digest("o".into()), items: vec![], reads: vec![], changed: vec![] },
            started("change", 1),
            Event::AttemptFailed { step: "change".into(), attempt: 1, reasons: vec!["could not judge".into()], usage: Usage::default(), spent: false },
            Event::Blocked { step: Some("change".into()), fault: Fault::Jaynes, reason: "the check suite of change could not judge: x. No attempt was spent".into() },
        ] {
            r.apply(&e);
        }
        // the run's count and the step's line say the same: one attempt, the agent's
        assert_eq!(work(&r), Work { attempts: 1, fixes: 0, own_failures: 0 });
        assert_eq!(work(&r).said(), "1 attempt by the agent");
        assert_eq!(stages(&r)[1].detail, "blocked, 1 attempt");
        assert!(!stages(&r).iter().any(|s| s.name == "plan" && s.detail.contains("attempt")), "{:?}", stages(&r));

        for e in [
            Event::Continued { by: By::Person },
            started("change", 2),
            Event::FixAsked { step: "change".into(), attempt: 2, fix: 1, reasons: vec!["the suite fails".into()] },
        ] {
            r.apply(&e);
        }
        // the agent worked twice, its second attempt with a fix in it
        assert_eq!(work(&r).said(), "2 attempts by the agent, 1 fix");
        assert_eq!(stages(&r)[1].detail, "running, attempt 2");
        // a failure of its own is said as such, and the fixes as fixes
        let mine = Work { attempts: 3, fixes: 2, own_failures: 1 };
        assert_eq!(mine.said(), "3 attempts by the agent, 1 failed through its own fault, 2 fixes");
        assert_eq!(Work { attempts: 1, fixes: 0, own_failures: 1 }.said(), "1 attempt by the agent, it failed through its own fault");
    }

    #[test]
    fn the_agent_said_is_its_last_message_in_at_most_three_lines() {
        let home = std::env::temp_dir().join(format!("jaynes-blocked-said-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        let step = home.join("runs/r1/steps/change");
        std::fs::create_dir_all(&step).expect("dir");
        std::fs::write(step.join("turn-1-0.jsonl"), [said("First try."), tool("Bash", "make")].join("\n")).expect("write");
        std::thread::sleep(std::time::Duration::from_millis(20));
        let last = said("The tab shows the log.\n\n- **Log:** newest first.\n- **Checklist:** after it.\n- one more");
        std::fs::write(step.join("turn-2-0.jsonl"), [said("Second try."), tool("Read", "/x"), last].join("\n")).expect("write");
        let told = agent_said(&home, "r1", "change");
        assert_eq!(told, ["The tab shows the log.", "- **Log:** newest first.", "- **Checklist:** after it."]);
        assert!(agent_said(&home, "r1", "other").is_empty());
        let _gone = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn the_words_the_card_reads_are_the_ones_the_box_check_writes() {
        // jaynes_menu.py says why the box kept a check from judging; the card finds its parts by these words
        let menu = include_str!("../../../python/jaynes_menu.py");
        for words in [BOX_REFUSED, TRIED_PORT, OWN_CHOOSING, REACH_ONLY, OPEN_DID_NOT_HELP, SERVERS_DID_NOT_HELP, OPEN_PORTS] {
            assert!(menu.contains(words.trim_start()), "jaynes_menu.py no longer says {words:?}");
        }
    }
}

/// Who halted a run, from its entries: the last halt's author, as the column says it.
pub fn halted_by(events: &[jaynes_core::event::Event]) -> String {
    use jaynes_core::question::By;
    let by = events.iter().rev().find_map(|e| match e {
        jaynes_core::event::Event::Halted { by } => Some(by),
        _ => None,
    });
    match by {
        Some(By::Person) => "halted by you",
        Some(By::Agent { .. }) => "halted by an agent",
        Some(By::Rule { .. }) => "halted by a rule",
        None => "halted",
    }
    .into()
}

/// The title of a halted run's block: who halted it, as the column says it ("halted by an agent"), and at which step.
pub fn halted_said(who: &str, step: Option<&str>) -> String {
    let mut c = who.chars();
    let who = c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default();
    match step {
        Some(s) => format!("{who} at “{s}”"),
        None => who,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jaynes_core::manifest::{Executor, Manifest, Reach, SessionKind, StepDecl};
    use jaynes_core::question::{Question, QuestionKind};
    use jaynes_core::Input;

    pub(super) fn run(id: &str, request: &str) -> Run {
        let step = |name: &str| StepDecl {
            name: name.into(), needs: vec![], executor: Executor::Agent, session: SessionKind::Fresh, reach: Reach::default(),
            effect: Default::default(), checks: vec![], stop: None, from_risk: Default::default(), attempts: 3, fixes: 2, hidden: false, idempotent: false, about: String::new(),
        };
        let manifest = Manifest { role: "r".into(), version: "1".into(), protocol: 1, steps: vec![step("plan"), step("change")], target: Default::default(), delivery: Default::default() };
        let start = Input::Start { run: id.into(), request: request.into(), manifest, role_fingerprint: jaynes_core::Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        let out = jaynes_core::decide(None, &start).expect("start");
        Run::fold(&out.events).expect("run")
    }

    fn entry(run: Run, project: &str, at: u64) -> Entry {
        Entry { run, project: project.into(), title: None, started: 0, at, merged: false, into: String::new() }
    }

    // The tech-lead's rule, 21/11 (Alan's run r1795301086-403): a run that went on without the answer to a stop shows
    // where it is now, never the stop's invitation; the stop goes to "Can still answer", under what waits.
    #[test]
    fn asked_a_stop_the_run_passed_is_listed_under_can_still_answer_and_the_run_shows_now() {
        let mut went = run("r1", "Fix the login redirect");
        went.phase = Phase::Blocked { step: Some("change".into()), fault: jaynes_core::event::Fault::Agent, reason: "the change could not be checked".into() };
        let question = Question {
            id: "q1".into(), step: "plan".into(), round: 1, kind: QuestionKind::Decision, items: vec![], offered: vec![], holds: false,
            about: "Is this the bug?".into(), choose: false, pass: 0, passes: 0, back: vec![],
        };
        went.questions.insert("q1".into(), QuestionState { question, answer: None, effects: None, superseded: false });
        let entries = [entry(went, "/p/app", 30)];
        let listing = arrange(&entries);
        assert!(listing.needs.is_empty(), "the stop it passed does not make it wait for the person");
        assert_eq!(listing.projects[0].1.iter().map(|r| r.mark).collect::<Vec<_>>(), [Mark::Blocked]);
        let still = can_still_answer(&entries);
        assert_eq!(still.iter().map(|r| (r.id.as_str(), r.title.as_str(), r.line.as_str())).collect::<Vec<_>>(), [("r1", "Fix the login redirect", "At “plan”: Is this the bug? · The run went on without your answer.")]);
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

        let listing = arrange(&[entry(waits, "/p/app", 30), entry(older, "/p/app", 10), entry(works, "/p/app", 20), entry(other, "/p/site", 40)]);

        assert_eq!(listing.needs, vec![Row { id: "r1".into(), project: "app".into(), title: "Make the onboarding friendlier".into(), apart: None, line: "Which plan goes on?".into(), mark: Mark::Needs }]);
        let names: Vec<&str> = listing.projects.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(names, ["site", "app"]);
        let app: Vec<(&str, Mark, &str)> = listing.projects[1].1.iter().map(|r| (r.id.as_str(), r.mark, r.line.as_str())).collect();
        assert_eq!(app, [("r2", Mark::Working, "step 1 of 2, running"), ("r3", Mark::Delivered, "delivered")]);
    }

    /// "não consigo saber fácil qual run é de qual projeto" (Alan, 21/11/2026): two runs showed side by side as
    /// "Agreed with Alan on 2026-11-21…". Every run's name starts with its project, then its title; two runs of one
    /// title in one project are told apart by the step each is at, and by when it started when that is the same too.
    #[test]
    fn asked_which_run_is_which_project() {
        let agreed = "Agreed with Alan on 2026-11-21 (relayed by Otto): the plan tab of the page shows the plan's log and its checklist. Then the rest.";
        let mut waits = run("r1", agreed);
        waits.phase = Phase::Waiting { step: Some("plan".into()), on: vec!["q1".into()], why: "waits for your answer".into() };
        let question = Question {
            id: "q1".into(), step: "plan".into(), round: 1, kind: QuestionKind::Decision, items: vec![], offered: vec![], holds: true,
            about: "Which plan goes on?".into(), choose: false, pass: 0, passes: 0, back: vec![],
        };
        waits.questions.insert("q1".into(), QuestionState { question, answer: None, effects: None, superseded: false });
        let other = run("r2", agreed);
        // the same title in one project: at different steps, then at the same step
        let at_change = |id: &str| {
            let mut r = run(id, "Fix the export");
            r.steps[0].status = jaynes_core::run::Status::Done;
            r.steps[0].done = Some(jaynes_core::run::Done { round: 1, output: jaynes_core::Digest("o".into()), items: vec![], reads: r.reads(0), changed: vec![] });
            r
        };
        let titled = |r: Run, project: &str, at: u64, started: u64| Entry { title: Some("Fix the export".into()), started, ..entry(r, project, at) };
        let entries = [
            entry(waits, "/work/tidepool", 50),
            entry(other, "/work/harbor-api", 40),
            titled(run("r3", "x"), "/work/tidepool", 30, 1_000),
            titled(at_change("r4"), "/work/tidepool", 20, 2_000),
            titled(at_change("r5"), "/work/tidepool", 10, 3_000),
            // a folder of the same name somewhere else is another project: the folder above it tells them apart
            entry(run("r6", "Rename the footer link"), "/side/harbor-api", 5),
        ];
        let listing = arrange(&entries);
        let rows: Vec<&Row> = listing.needs.iter().chain(listing.projects.iter().flat_map(|(_, rows)| rows)).collect();
        let named = |id: &str| name(rows.iter().find(|r| r.id == id).expect(id));
        // the request without its opening "Agreed with … on …:", its first sentence, cut at a word within 40
        assert_eq!(named("r1"), ["tidepool", "The plan tab of the page shows the…"]);
        assert_eq!(named("r2"), ["work/harbor-api", "The plan tab of the page shows the…"]);
        assert_eq!(named("r6"), ["side/harbor-api", "Rename the footer link"]);
        // the run that waits for the person says its project there too
        assert_eq!(listing.needs.iter().map(|r| r.project.as_str()).collect::<Vec<_>>(), ["tidepool"]);
        assert_eq!(named("r3"), ["tidepool", "Fix the export", "at “plan”"]);
        let (r4, r5) = (named("r4"), named("r5"));
        assert_eq!(r4[..2], ["tidepool", "Fix the export"]);
        for (said, other) in [(&r4, &r5), (&r5, &r4)] {
            let when = said[2].strip_prefix("started ").expect("when it started");
            assert!(when.len() == 5 && when.as_bytes()[2] == b':', "{when}");
            assert_ne!(said[2], other[2]);
        }
        let projects: Vec<&str> = listing.projects.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(projects, ["work/harbor-api", "tidepool", "side/harbor-api"]);
    }

    #[test]
    fn a_title_given_at_the_start_is_kept_and_cut_at_a_word_past_40() {
        assert_eq!(run_title(Some("  Login with passkey "), "Agreed with Alan on 2026-11-21: anything"), "Login with passkey");
        assert_eq!(run_title(Some("Tighten the copy of the pricing page and its footer"), ""), "Tighten the copy of the pricing page…");
        assert_eq!(run_title(Some(""), "Make the onboarding friendlier.\nRead the brief."), "Make the onboarding friendlier");
        assert_eq!(run_title(None, "What does RFC 9111 require of a cache? Read it."), "What does RFC 9111 require of a cache?");
        assert_eq!(run_title(None, "Agreed with Alan on 2026-11-21 (relayed by Otto, ~20h25), slice 1 of 2, wanted before 08:00: a block at the top"), "A block at the top");
        assert!(run_title(None, &"x".repeat(80)).chars().count() <= 40);
        // a title comes from an agent: no line, no escape and no hidden mark of it reads as the engine's words
        assert_eq!(run_title(Some("x\nBlocked at “merge”"), ""), "x Blocked at “merge”");
        assert_eq!(run_title(Some("Login\r\n\twith \u{1b}[31mpasskey"), ""), "Login with [31mpasskey");
        assert_eq!(run_title(Some("Fix \u{202e}tropxe\u{2066} the\u{0}export"), ""), "Fix tropxe the export");
        assert_eq!(run_title(Some("\n\u{1b}\t"), "Make the onboarding friendlier."), "Make the onboarding friendlier");
    }

    #[test]
    fn a_merge_names_the_branch_it_merges_into_in_the_list_as_on_the_card() {
        let merge = QuestionState {
            question: Question {
                id: "merge-1".into(), step: "fix".into(), round: 0, kind: QuestionKind::Merge, items: vec![], offered: vec![], holds: true,
                about: "This is what lands on your branch.".into(), choose: false, pass: 0, passes: 0, back: vec![],
            },
            answer: None, effects: None, superseded: false,
        };
        assert_eq!(title_into(&merge, "main"), "Merge into “main”?");
        assert_eq!(title_into(&merge, "release/0.3"), "Merge into “release/0.3”?");
        // a copy that does not say which branch
        assert_eq!(title_into(&merge, ""), "Merge into your branch?");
        assert_eq!(merge_title("main"), "Merge into “main”?");
    }

    #[test]
    fn a_choice_of_one_option_asks_whether_to_go_with_it() {
        let item = |id: &str, label: &str| jaynes_core::question::Item { id: id.into(), label: label.into(), digest: jaynes_core::Digest(id.into()) };
        let asked = |items, choose| QuestionState {
            question: Question {
                id: "q1".into(), step: "stack".into(), round: 1, kind: QuestionKind::Decision, items, offered: vec![], holds: true,
                about: "Which stack goes on?".into(), choose, pass: 0, passes: 0, back: vec![],
            },
            answer: None, effects: None, superseded: false,
        };
        // the name, before the colon of its label: the app and the page read the same line
        assert_eq!(title(&asked(vec![item("py", "Python + FastAPI: small and known")], true)), "Go with “Python + FastAPI”?");
        assert_eq!(title(&asked(vec![item("go", "Go")], true)), "Go with “Go”?");
        // two or more, and one item of a stop that does not choose: the role's question
        assert_eq!(title(&asked(vec![item("py", "Python"), item("go", "Go")], true)), "Which stack goes on?");
        assert_eq!(title(&asked(vec![item("py", "Python")], false)), "Which stack goes on?");
    }

    /// Item 10 of 0.2.5: `jaynes runs` said "running" of a halted run, where the app said "halted". Every state the
    /// list knows reads the same in `jaynes runs` (said), on the page (phase_word) and in the app (the row's mark).
    #[test]
    fn asked_every_state_reads_the_same_in_jaynes_runs_on_the_page_and_in_the_app() {
        use jaynes_core::question::By;
        let stop = |r: &mut Run| {
            let question = Question {
                id: "q1".into(), step: "plan".into(), round: 1, kind: QuestionKind::Decision, items: vec![], offered: vec![], holds: true,
                about: "Which plan goes on?".into(), choose: false, pass: 0, passes: 0, back: vec![],
            };
            r.questions.insert("q1".into(), QuestionState { question, answer: None, effects: None, superseded: false });
            r.phase = Phase::Waiting { step: Some("plan".into()), on: vec!["q1".into()], why: "waits for your answer".into() };
        };
        let mut cases: Vec<(&str, Entry, &str)> = Vec::new();
        let mut r = run("needs", "x");
        stop(&mut r);
        cases.push(("needs you", entry(r, "/p", 1), "Which plan goes on?"));
        let mut r = run("works", "x");
        r.steps[0].status = jaynes_core::run::Status::Running { attempt: 1, fix: 0 };
        cases.push(("working", entry(r, "/p", 1), "step 1 of 2, running"));
        // the run of the item: halted while a step was under way, its phase still Running
        let mut r = run("halted", "x");
        r.steps[0].status = jaynes_core::run::Status::Running { attempt: 1, fix: 0 };
        r.halted = true;
        cases.push(("halted", entry(r, "/p", 1), "step 1 of 2, halted"));
        // halted at an open stop: the person can still answer it, so it waits for them
        let mut r = run("halted-at-stop", "x");
        stop(&mut r);
        r.halted = true;
        cases.push(("needs you", entry(r, "/p", 1), "Which plan goes on?"));
        let mut r = run("aside", "x");
        r.halted = true;
        r.set_aside = Some(By::Agent { who: "claude:1@x".into() });
        cases.push(("halted", entry(r, "/p", 1), "step 1 of 2, set aside by the agent that started it"));
        let mut r = run("blocked", "x");
        r.phase = Phase::Blocked { step: Some("plan".into()), fault: jaynes_core::event::Fault::Agent, reason: "no tests".into() };
        cases.push(("blocked", entry(r, "/p", 1), "step 1 of 2, blocked: no tests"));
        let mut r = run("open", "x");
        r.phase = Phase::Waiting { step: None, on: vec![], why: String::new() };
        cases.push(("delivered", entry(r, "/p", 1), "delivered, answers open"));
        let mut r = run("delivered", "x");
        r.phase = Phase::Finished { outcome: Outcome::Delivered };
        cases.push(("delivered", entry(r.clone(), "/p", 1), "delivered"));
        r.id = "merged".into();
        cases.push(("merged", Entry { merged: true, ..entry(r.clone(), "/p", 1) }, "merged"));
        r.id = "undone".into();
        r.undone = true;
        cases.push(("undone", Entry { merged: true, ..entry(r.clone(), "/p", 1) }, "reverted by you"));
        let mut r = run("abandoned", "x");
        r.phase = Phase::Finished { outcome: Outcome::Abandoned };
        cases.push(("abandoned", entry(r, "/p", 1), "abandoned"));
        for (word, e, line) in &cases {
            let listing = arrange(std::slice::from_ref(e));
            let row = listing.needs.iter().chain(listing.projects.iter().flat_map(|(_, rows)| rows)).next().expect("row");
            let id = &e.run.id;
            assert_eq!((row.mark.word(), row.line.as_str()), (*word, *line), "{id}: the app's mark and line");
            let said = said(row);
            assert!(said.starts_with(word) && said.contains(line), "{id}: jaynes runs says {said:?}");
            assert!(!(said.contains("running") && *word != "working"), "{id}: jaynes runs says {said:?}");
            let page = crate::page::phase_word(e);
            assert!(page.contains(&format!(">{word}")) || page == *word, "{id}: the page says {page:?}");
        }
    }

    #[test]
    fn every_run_is_listed_once() {
        let mut halted = run("a", "x");
        halted.halted = true;
        let mut dropped = run("b", "y");
        dropped.phase = Phase::Finished { outcome: Outcome::Abandoned };
        let mut merged = run("c", "z");
        merged.phase = Phase::Finished { outcome: Outcome::Delivered };
        let listing = arrange(&[entry(halted, "", 1), entry(dropped, "/q", 2), Entry { merged: true, ..entry(merged, "/q", 3) }]);
        let mut ids: Vec<&str> = listing.needs.iter().chain(listing.projects.iter().flat_map(|(_, rows)| rows)).map(|r| r.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["a", "b", "c"]);
        let marks: Vec<(String, Mark)> = listing.projects.iter().flat_map(|(p, rows)| rows.iter().map(move |r| (p.clone(), r.mark))).collect();
        assert_eq!(marks, [("q".to_string(), Mark::Merged), ("q".to_string(), Mark::Abandoned), ("no project".to_string(), Mark::Halted)]);
    }

    #[test]
    fn each_step_says_how_it_stands() {
        let mut r = run("s", "x");
        r.steps[0].status = jaynes_core::run::Status::Running { attempt: 2, fix: 0 };
        let stages = stages(&r);
        let seen: Vec<(&str, &str, &str)> = stages.iter().map(|s| (s.name.as_str(), s.class, s.detail.as_str())).collect();
        assert_eq!(seen, [("plan", "running", "running, attempt 2"), ("change", "todo", "")]);
        // halted, the step under way says so, and is drawn as nothing working on it
        r.halted = true;
        for (status, detail) in [
            (jaynes_core::run::Status::Running { attempt: 2, fix: 0 }, "halted, attempt 2"),
            (jaynes_core::run::Status::Checking { attempt: 3, fix: 1 }, "halted, attempt 3"),
            (jaynes_core::run::Status::Probing, "halted while testing what it reaches"),
        ] {
            r.steps[0].status = status;
            let first = &super::stages(&r)[0];
            assert_eq!((first.class, first.detail.as_str()), ("halted", detail));
        }
    }

    #[test]
    fn a_reason_said_before_codes_were_words_reads_as_they_are_said_now() {
        assert_eq!(in_words("change spent its 3 attempts: the agent exited with Some(1): boom"), "change spent its 3 attempts: the agent exited with code 1: boom");
        assert_eq!(in_words("Codex exited with None: x"), "Codex was stopped by a signal, with no exit code: x");
        assert_eq!(in_words("the agent exited with code 1"), "the agent exited with code 1");
        assert_eq!(in_words("a role said Some(thing) else"), "a role said Some(thing) else");
        // the line of a blocked run in the list says it so
        let mut r = run("b", "Rename the footer link");
        r.phase = Phase::Blocked { step: Some("change".into()), fault: jaynes_core::event::Fault::Agent, reason: "change spent its 3 attempts: the agent exited with Some(1):".into() };
        let row = arrange(&[entry(r, "/p/site", 1)]).projects[0].1[0].clone();
        // nothing the agent said after it: no colon left hanging
        assert_eq!(row.line, "step 1 of 2, blocked: change spent its 3 attempts: the agent exited with code 1");
        assert_eq!(in_words("change spent its 3 attempts: the agent exited with Some(1):   "), "change spent its 3 attempts: the agent exited with code 1");
    }

    #[test]
    fn a_step_blocked_by_its_attempts_says_it_used_them_and_another_cause_says_its_attempts() {
        let mut r = run("b", "x");
        r.steps[1].attempt = 3;
        r.phase = Phase::Blocked { step: Some("change".into()), fault: jaynes_core::event::Fault::Agent, reason: "change spent its 3 attempts: the agent exited with code 1".into() };
        let blocked = &stages(&r)[1];
        assert_eq!((blocked.class, blocked.detail.as_str()), ("blocked", "blocked: used its 3 attempts"));
        r.phase = Phase::Blocked { step: Some("change".into()), fault: jaynes_core::event::Fault::Jaynes, reason: "no box".into() };
        assert_eq!(stages(&r)[1].detail, "blocked, 3 attempts");
    }

    #[test]
    fn only_the_step_whose_question_is_open_waits_and_nothing_waits_once_the_run_is_over() {
        use jaynes_core::run::Status;
        let mut r = run("w", "x");
        r.steps[0].status = Status::AwaitingInput { question: "q1".into() };
        // the engine names the next step as the one that waits: it runs once the person answers plan's question
        r.phase = Phase::Waiting { step: Some("change".into()), on: vec!["q1".into()], why: "waits for your answer about plan".into() };
        let seen: Vec<(String, &str)> = stages(&r).into_iter().map(|s| (s.name, s.class)).collect();
        assert_eq!(seen, [("plan".to_string(), "waiting"), ("change".to_string(), "todo")]);
        assert_eq!(stages(&r)[1].detail, "next, after your answer");

        r.phase = Phase::Finished { outcome: Outcome::Abandoned };
        let seen: Vec<(String, &str, String)> = stages(&r).into_iter().map(|s| (s.name, s.class, s.detail)).collect();
        assert_eq!(seen, [("plan".to_string(), "todo", "left unanswered".to_string()), ("change".to_string(), "todo", String::new())]);
    }

    #[test]
    fn the_last_thing_the_agent_did_is_its_last_tool_or_what_it_said() {
        let project = Path::new("/home/x/.jaynes/engine/runs/r1/project");
        let lines = [
            r#"{"type":"system","subtype":"init","session_id":"s"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"I will read the brief first.\nThen draw."}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/home/x/.jaynes/engine/runs/r1/project/docs/brief.md"}}]}}"#,
        ];
        assert_eq!(last_action(&lines[..2].join("\n"), project).as_deref(), Some("I will read the brief first."));
        assert_eq!(last_action(&lines.join("\n"), project).as_deref(), Some("Read docs/brief.md"));
        let bash = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"npm test -- --run\nnpm run lint","description":"Run the tests"}}]}}"#;
        assert_eq!(last_action(bash, project).as_deref(), Some("Bash npm test -- --run"));
        let thinking = format!("{bash}\n{}", r#"{"type":"system","subtype":"thinking_tokens","tokens":12}"#);
        assert_eq!(last_action(&thinking, project).as_deref(), Some("thinking"));
        let ended = format!("{bash}\n{}", r#"{"type":"result","subtype":"success"}"#);
        assert_eq!(last_action(&ended, project).as_deref(), Some("finished its turn"));
        assert_eq!(last_action("not json\n--- stderr ---\n", project), None);
    }

    #[test]
    fn a_runs_story_is_what_each_step_did_in_the_order_the_steps_worked() {
        let home = std::env::temp_dir().join(format!("jaynes-story-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        let steps = home.join("runs/r1/steps");
        let project = home.join("runs/r1/project");
        let said = |text: &str| format!(r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{text}"}}]}}}}"#);
        let tool = |name: &str, path: &str| format!(r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","name":"{name}","input":{{"file_path":"{}/{path}"}}}}]}}}}"#, project.display());
        let write = |step: &str, turn: &str, lines: &[String]| {
            std::fs::create_dir_all(steps.join(step)).expect("dir");
            std::fs::write(steps.join(step).join(turn), lines.join("\n") + "\n--- stderr ---\n").expect("write");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        write("reproduce", "turn-1-0.jsonl", &[r#"{"type":"system","subtype":"init"}"#.into(), said("I will run the failing test."), tool("Read", "auth/google.go")]);
        write("fix", "turn-1-0.jsonl", &[tool("Edit", "auth/google.go"), r#"{"type":"system","subtype":"thinking_tokens"}"#.into()]);
        write("fix", "turn-2-0.jsonl", &[said("Done: the token refreshes."), r#"{"type":"result","subtype":"success"}"#.into()]);

        let told = story(&home, "r1");

        assert_eq!(told, vec![
            ("reproduce".to_string(), vec!["I will run the failing test.".to_string(), "Read auth/google.go".to_string()]),
            ("fix".to_string(), vec!["Edit auth/google.go".to_string(), "Done: the token refreshes.".to_string()]),
        ]);
        assert!(story(&home, "no-such-run").is_empty());
        let _gone = std::fs::remove_dir_all(&home);
    }
}
