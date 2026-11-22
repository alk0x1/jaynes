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
    /// Set aside by the agent that started it: it waits for no one (Resume and Abandon stay on its page).
    SetAside,
    /// Started on an engine no longer on this Mac: it cannot go on, and waits apart until the person moves it.
    Older,
    /// One of those, moved to Finished by the person.
    Moved,
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
            Mark::SetAside => "set aside",
            Mark::Older => "can't go on",
            Mark::Moved => "moved by you",
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
    /// Runs started on an engine no longer on this Mac, not yet moved to Finished: "From an older Jaynes", newest
    /// first. Never in what waits for the person, nor counted with it.
    pub older: Vec<Row>,
}

impl Listing {
    /// Every row once: what waits, the runs from an older Jaynes, then each project's.
    pub fn rows(&self) -> impl Iterator<Item = &Row> {
        self.needs.iter().chain(&self.older).chain(self.projects.iter().flat_map(|(_, rows)| rows))
    }
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
    /// The version of the engine that started it, when that engine is no longer on this Mac and the run cannot go
    /// on (`stop::left_behind`).
    pub gone: Option<String>,
    /// When the person moved it to Finished, if they did (`moved`).
    pub moved: Option<u64>,
    /// Jaynes stopped while it ran: a signal stopped the process driving it, which halted it, or that process is gone
    /// and the run still says it runs (`drive::left_running`). The person resumes it.
    pub stopped: bool,
    /// What a box of the run waits for now, in the box's words: a port another run holds (`port_wait`).
    pub waiting: Option<String>,
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

/// Run `run` with what its mark needs from outside its events (whether it was merged, into which branch, and
/// whether its engine left the Mac, and if so whether the person moved it to Finished); no project, title or times.
pub fn entry_of(home: &Path, run: Run) -> Entry {
    let merged = !crate::drive::merges(&home.join("runs").join(&run.id)).is_empty();
    let into = into_of(home, &run.id);
    let gone = crate::stop::left_behind(home, &run, jaynes_core::ENGINE).then(|| engine_version(&run.engine).to_string());
    let moved = gone.as_ref().and_then(|_| crate::moved::when(&crate::moved::read(home).unwrap_or_default(), &run.id));
    let stopped = stopped_by_jaynes(&run) || crate::drive::left_running(home, &run);
    let waiting = crate::port_wait::live(&home.join("runs").join(&run.id));
    Entry { run, project: String::new(), title: None, started: 0, at: 0, merged, into, gone, moved, stopped, waiting }
}

/// An engine's version, without the build after its "+": "0.2.3" of "0.2.3+5a1c0de".
pub fn engine_version(engine: &str) -> &str {
    engine.split('+').next().unwrap_or(engine)
}

/// Where what a run made is, as its card says it to a run that cannot go on. The engine names the place; no screen
/// builds a path by hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// Delivered on a branch of its own, not merged.
    Branch { branch: String, project: String },
    /// Merged into the person's branch.
    Into { branch: String, project: String },
    /// Not delivered: what it made so far, in its copy.
    Copy { folder: String },
}

impl Place {
    /// The sentence in three parts, the place in the middle, shown in mono with a Copy beside it: "What it delivered
    /// is on the branch “" "jaynes/r1" "” of tidepool."
    pub fn parts(&self) -> (String, String, String) {
        match self {
            Place::Branch { branch, project } => ("What it delivered is on the branch “".into(), branch.clone(), format!("” of {project}.")),
            Place::Into { branch, project } => ("What it delivered is on “".into(), branch.clone(), format!("” of {project}.")),
            Place::Copy { folder } => ("What it made so far is in its copy: ".into(), folder.clone(), String::new()),
        }
    }
}

/// Where what run `e` made is: on the branch it delivered (`branch`, its setup's), on the branch it merged into, or
/// in its copy at `copy` when it made something there; None when it made nothing yet.
pub fn place(e: &Entry, branch: &str, copy: &Path) -> Option<Place> {
    let project = Path::new(&e.project).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "your project".into());
    if e.merged && !e.into.is_empty() {
        return Some(Place::Into { branch: e.into.clone(), project });
    }
    if e.run.delivered.is_some() && !branch.is_empty() {
        return Some(Place::Branch { branch: branch.to_string(), project });
    }
    let made = e.run.steps.iter().any(|s| s.attempt > 0 || s.done.is_some());
    (made && copy.is_dir()).then(|| Place::Copy { folder: copy.display().to_string() })
}

/// `place` of a run of the record in `home`, its branch read from its setup.
pub fn place_of(home: &Path, e: &Entry) -> Option<Place> {
    let setup = Store::open(&home.join("record.sqlite3")).and_then(|s| s.setup(&e.run.id)).unwrap_or_default();
    place(e, setup["branch"].as_str().unwrap_or(""), &home.join("runs").join(&e.run.id).join("project"))
}

/// `place_of` a run alone, its project read from its setup: what the card of a run whose engine left says.
pub fn run_place(home: &Path, run: &Run) -> Option<Place> {
    let mut e = entry_of(home, run.clone());
    e.project = Store::open(&home.join("record.sqlite3")).and_then(|s| s.setup(&run.id)).ok().and_then(|s| s["project"].as_str().map(str::to_string)).unwrap_or_default();
    place_of(home, &e)
}

/// What a run whose engine left the Mac says in place of its question (the design-lead, motor-velho.md): its
/// version alone, without the build.
pub fn older_head(version: &str) -> String {
    format!("This run can't go on: it started on Jaynes {version}, which is no longer on this Mac.")
}

/// Under `older_head`, dim.
pub const OLDER_HOW: &str = "No answer reaches it, Halt included. To do this work, start it again: a new run starts on this Jaynes.";

/// The group of runs whose engine left the Mac, below what waits for the person and above Finished.
pub const OLDER_GROUP: &str = "From an older Jaynes";

/// The card's button, neutral, never red, and what it says under it.
pub const MOVE_TO_FINISHED: &str = "Move to Finished";
pub const MOVE_SAYS: &str = "It leaves Needs you for good. What it already delivered stays as it is, and its record is kept.";

/// The group's link to move them all, shown only for two or more, and the line it opens under the group's head.
pub const MOVE_ALL: &str = "Move all to Finished";
pub fn move_all_asks(n: usize) -> String {
    format!("Move these {n} runs to Finished for good? What they delivered stays as it is.")
}
pub fn move_all_button(n: usize) -> String {
    format!("Move {n}")
}

/// The line of a run whose engine left the Mac: "Jaynes 0.2.3" while it waits apart, its group's head saying the
/// rest, so a narrow column never cuts the version (the design-lead, 22/11); "started on Jaynes 0.2.3, no longer on
/// this Mac" once moved to Finished, where nothing else says why it is there.
pub fn older_line(version: &str, moved: bool) -> String {
    if moved { format!("started on Jaynes {version}, no longer on this Mac") } else { format!("Jaynes {version}") }
}

/// How a run stands in the list: its mark, its line, and whether it waits for the person. A blocked run says so first:
/// its open questions stay inside it, never in its line (the tech-lead's rule, for every role).
pub fn stands(e: &Entry) -> (Mark, String, bool) {
    let r = &e.run;
    // its engine left the Mac: no answer reaches it, so it never waits for the person
    if let Some(version) = &e.gone {
        let moved = e.moved.is_some();
        return (if moved { Mark::Moved } else { Mark::Older }, older_line(version, moved), false);
    }
    // set aside, it waits for no one, not even at an open stop
    let waiting = if r.finished() || r.blocked() || r.set_aside.is_some() { None } else { r.questions.values().find(|q| q.open() && crate::stop::went_on(r, q).is_none()) };
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
        if row.mark == Mark::Older {
            listing.older.push(row);
            continue;
        }
        match listing.projects.iter_mut().find(|(p, _)| *p == row.project) {
            Some((_, rows)) => rows.push(row),
            None => listing.projects.push((row.project.clone(), vec![row])),
        }
    }
    listing
}

/// An engine's version without its fingerprint: "0.2.4" of "0.2.4+6ab1636".
pub fn release_of(engine: &str) -> &str {
    engine.split('+').next().unwrap_or(engine)
}

/// What `jaynes update` says before it installs: the open runs it will leave behind (`left`, which is
/// `stop::left_behind` of the version it installs: another engine started them and its program is not kept; never a
/// run already gone, which cannot go on before the update either), for each version (the newest first) how many, up to five by name with how each stands, and how to finish them. The
/// design-lead's words (0.2.5, motor-velho.md); nothing when it leaves none behind.
pub fn older_runs_said(entries: &[Entry], left: impl Fn(&Run) -> bool) -> Vec<String> {
    // a run whose engine already left the Mac cannot go on before the update either ("From an older Jaynes"): the
    // update changes nothing for it, and the person could not finish it on that version
    let older: Vec<Entry> = entries.iter().filter(|e| !e.run.finished() && e.gone.is_none() && left(&e.run)).cloned().collect();
    let mut versions: Vec<&str> = older.iter().map(|e| release_of(&e.run.engine)).collect();
    versions.sort_by_key(|a| std::cmp::Reverse(version_key(a)));
    versions.dedup();
    let mut said = Vec::new();
    for v in versions {
        let of: Vec<Entry> = older.iter().filter(|e| release_of(&e.run.engine) == v).cloned().collect();
        let listing = arrange(&of);
        let rows: Vec<&Row> = listing.rows().collect();
        let one = rows.len() == 1;
        let (them, their) = if one { ("it", "Its") } else { ("them", "Their") };
        said.push(format!(
            "{} run{} started on Jaynes {v} can't go on after this update: no answer, Halt or Abandon will reach {them}.",
            rows.len(),
            if one { "" } else { "s" }
        ));
        for row in rows.iter().take(5) {
            let e = of.iter().find(|e| e.run.id == row.id).expect("the row's run");
            let stands = match row.mark {
                Mark::Needs => "waits for your answer".to_string(),
                Mark::Working => format!("working, at “{}”", step_at(&e.run)),
                other => other.word().to_string(),
            };
            said.push(format!("  {} ({stands})", name(row).join(" · ")));
        }
        if rows.len() > 5 {
            said.push(format!("  and {} more", rows.len() - 5));
        }
        said.push(format!("To finish {them} on {v}, answer or abandon {them} before you update. {their} work stays where it is either way,"));
        said.push(format!("and the app lists {them} under “From an older Jaynes”."));
    }
    said
}

/// A version's numbers, to put the newest first: "0.2.10" after "0.2.9".
fn version_key(v: &str) -> Vec<u64> {
    v.split('.').map(|n| n.parse().unwrap_or(0)).collect()
}

/// The stops runs went on without, newest run first, each as a row whose line says so: "At “reproduce”: Is this the
/// bug? · The run went on without your answer." They come after what waits for the person, under "Can still answer".
pub fn can_still_answer(entries: &[Entry]) -> Vec<Row> {
    let folders: Vec<&str> = entries.iter().map(|e| e.project.as_str()).collect();
    let mut newest: Vec<&Entry> = entries.iter().collect();
    newest.sort_by_key(|e| std::cmp::Reverse(e.at));
    newest
        .into_iter()
        // a run whose engine left takes no answer, late ones included
        .filter(|e| e.gone.is_none())
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

/// A sentence after a comma: its first letter small.
fn lowered(said: &str) -> String {
    let mut c = said.chars();
    c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default()
}

/// A run's short name, as another run that waits for it says it: "estoque · Fix login".
pub fn short_name(project: &str, title: Option<&str>, request: &str) -> String {
    format!("{} · {}", project_name(project, &[]), run_title(title, request))
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

/// What a blocked run's line says after its step: the reason in words, or, for a run stopped at its start for a held
/// port, the case (the design-lead's of 22/11): "blocked: its tests need port 3306".
pub fn blocked_words(step: Option<&str>, fault: jaynes_core::event::Fault, reason: &str) -> String {
    if let Some(h) = held_at_start_of(step, fault, reason) {
        return format!("blocked: its tests need {}", ports_named(&h.ports));
    }
    if let Some(b) = served_at_start_of(step, fault, reason) {
        let fixed = fixed_ports(&b);
        return if fixed.is_empty() { "blocked: its tests start a test server".into() } else { format!("blocked: its tests need {}", ports_named(&fixed)) };
    }
    format!("blocked: {}", in_words(reason))
}

/// What the box refused a run stopped at its start: a fault of Jaynes at a menu step whose reason is the box's
/// refusal of a local port, as the runner's find-suite says it when the project's tests serve and the person has not
/// let them (the tech-lead's T5, 23/11).
fn served_at_start_of<'a>(step: Option<&str>, fault: jaynes_core::event::Fault, reason: &'a str) -> Option<Boxed<'a>> {
    if fault != jaynes_core::event::Fault::Jaynes {
        return None;
    }
    let inner = reason.strip_prefix(&format!("a fault of Jaynes or of the machine stopped {}: ", step?))?;
    let inner = inner.strip_prefix("the role program could not answer: ").unwrap_or(inner);
    boxed(inner).filter(|b| !b.ports.is_empty() && !b.open_did_not_help && !b.servers_did_not_help)
}

/// The ports a refusal names but the one the system picks.
fn fixed_ports(b: &Boxed<'_>) -> Vec<u16> {
    b.ports.iter().copied().filter(|&p| p != 0).collect()
}

/// The held port of a run stopped at its start: a fault of Jaynes at a menu step whose reason is the box's held port.
fn held_at_start_of<'a>(step: Option<&str>, fault: jaynes_core::event::Fault, reason: &'a str) -> Option<Held<'a>> {
    if fault != jaynes_core::event::Fault::Jaynes {
        return None;
    }
    let inner = reason.strip_prefix(&format!("a fault of Jaynes or of the machine stopped {}: ", step?))?;
    held(inner).filter(|h| !h.ports.is_empty())
}

/// The mark and the line of a run nothing waits on.
fn standing(e: &Entry) -> (Mark, String) {
    let r = &e.run;
    match &r.phase {
        Phase::Finished { outcome: Outcome::Abandoned } => (Mark::Abandoned, "abandoned".into()),
        Phase::Finished { outcome: Outcome::Delivered } if r.undone => (Mark::Undone, "reverted by you".into()),
        Phase::Finished { outcome: Outcome::Delivered } if e.merged => (Mark::Merged, "merged".into()),
        Phase::Finished { outcome: Outcome::Delivered } => (Mark::Delivered, "delivered".into()),
        _ if r.set_aside.is_some() => (Mark::SetAside, format!("set aside by the agent that started it · {}", where_in(r))),
        _ if e.stopped => (Mark::Halted, format!("{}, {}", where_in(r), crate::drive::JAYNES_STOPPED_SAID)),
        _ if r.halted => (Mark::Halted, format!("{}, halted", where_in(r))),
        Phase::Blocked { step, fault, reason } => (Mark::Blocked, format!("{}, {}", where_in(r), blocked_words(step.as_deref(), *fault, reason))),
        Phase::Waiting { step: None, .. } => (Mark::Delivered, "delivered, answers open".into()),
        Phase::Waiting { why, .. } => (Mark::Working, format!("{}, {why}", where_in(r))),
        Phase::Running => match &e.waiting {
            // a box of the run waits for a port another run holds: said where the person looks, never a fault
            Some(w) => (Mark::Working, format!("{}, {}", where_in(r), lowered(w))),
            None => (Mark::Working, format!("{}, running", where_in(r))),
        },
    }
}

/// What a run spent of its ceiling, as the page and the app say it: "Spent 58 s of 20 min" under a minute, then
/// "Spent 3 of 20 min".
pub fn spent(seconds: u64, ceiling_seconds: u64) -> String {
    let ceiling = ceiling_seconds / 60;
    if seconds < 60 { format!("Spent {seconds} s of {ceiling} min") } else { format!("Spent {} of {ceiling} min", seconds / 60) }
}

/// The request in at most 60 characters.
pub fn short(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= 60 { s.to_string() } else { format!("{}…", s.chars().take(59).collect::<String>()) }
}

/// Where the run is on its path: "step 2 of 5". A run held at a step's stop is at that step, though the step is done:
/// the next one has not begun.
pub fn where_in(r: &Run) -> String {
    let path = r.path();
    let held = match &r.phase {
        Phase::Waiting { step: Some(name), .. } => r.manifest.index(name).and_then(|i| path.iter().position(|&j| j == i)),
        _ => None,
    };
    let at = held.or_else(|| path.iter().position(|&i| !r.fresh(i))).map(|n| n + 1).unwrap_or(path.len());
    format!("step {at} of {}", path.len())
}

/// Where the step that asks is on the run's path: "step 3 of 4", the count of a question, at its stop and in the list
/// of what waits, wherever the run is now; "step 0 of 4" for a step its risk does not take.
pub fn asked_at(r: &Run, step: &str) -> String {
    let path = r.path();
    let at = r.manifest.index(step).and_then(|i| path.iter().position(|&j| j == i)).map(|n| n + 1).unwrap_or(0);
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

/// Where a run whose engine left the Mac stopped, in its list of steps.
pub const STOPPED_HERE: &str = "stopped here · can't go on";
/// A step after it, which no engine will run.
pub const NEVER_RAN: &str = "never ran";

/// The steps of a run whose engine left the Mac, as the page and the app show them (the design-lead, 22/11): the step
/// it stopped at, waiting, blocked or under way, is "gone", grey, "stopped here · can't go on", never "waiting for
/// you"; with none of those, between two steps, the first that had not run. The steps after it that had not run
/// "never ran". A step left out at the run's risk, or one that ran in an earlier round, keeps what it says.
pub fn cannot_go_on(mut stages: Vec<Stage>) -> Vec<Stage> {
    let stopped = stages.iter().position(|s| matches!(s.class, "waiting" | "blocked" | "running" | "halted"));
    let Some(at) = stopped.or_else(|| stages.iter().position(|s| s.class == "todo" && s.detail != "left out at this risk")) else { return stages };
    stages[at].class = "gone";
    stages[at].detail = STOPPED_HERE.into();
    for s in &mut stages[at + 1..] {
        if matches!(s.class, "todo" | "waiting") && (s.detail.is_empty() || s.detail == "next, after your answer" || s.detail.starts_with("waiting for you")) {
            s.class = "todo";
            s.detail = NEVER_RAN.into();
        }
    }
    stages
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
    if v["type"].as_str() == Some("user") {
        return refused(v);
    }
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

/// The marks of the writes outside the step's folder Jaynes refused, in a tool's result Claude Code gave the agent
/// (never in what the agent wrote itself): the path refused, and the copy's file it was told to write.
const REFUSED_MARK: &str = "\u{1}refused\u{1}";

fn refused(v: &serde_json::Value) -> Vec<String> {
    let blocks = v["message"]["content"].as_array().cloned().unwrap_or_default();
    blocks
        .iter()
        .filter(|b| b["type"] == "tool_result")
        .filter_map(|b| {
            let text = match &b["content"] {
                serde_json::Value::String(t) => t.clone(),
                serde_json::Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
                _ => return None,
            };
            let (path, told) = crate::permit::refused_said(&text)?;
            Some(format!("{REFUSED_MARK}{path}{REFUSED_MARK}{}", told.unwrap_or_default()))
        })
        .collect()
}

/// A step's story with each write Jaynes refused said in the design-lead's words (permissao.md): one line, grey with
/// its ◇, "◇ Jaynes refused a write outside tidepool's copy: ~/Projects/tidepool/test/screens.test.js", its path
/// whole with the home as ~, and under it "told the agent to write test/screens.test.js in its copy" when it was told
/// so. The same refusal again in the step is the same line, with " · 3 times" at its end.
fn with_refusals(said: Vec<String>, project: Option<&str>) -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let tilde = |p: &str| match p.strip_prefix(home.as_str()) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => format!("~{rest}"),
        _ => p.to_string(),
    };
    let copy = project.map(|p| format!("{p}'s copy")).unwrap_or_else(|| "the run's copy".to_string());
    let mut out: Vec<String> = Vec::new();
    // where each refusal's line is in `out`, and how many times it came
    let mut seen: Vec<(String, usize, usize)> = Vec::new();
    for line in said {
        let Some(mark) = line.strip_prefix(REFUSED_MARK) else {
            out.push(line);
            continue;
        };
        if let Some(s) = seen.iter_mut().find(|s| s.0 == mark) {
            s.2 += 1;
            continue;
        }
        let (path, told) = mark.split_once(REFUSED_MARK).unwrap_or((mark, ""));
        seen.push((mark.to_string(), out.len(), 1));
        out.push(format!("◇ Jaynes refused a write outside {copy}: {}", tilde(path)));
        if !told.is_empty() {
            out.push(format!("told the agent to write {told} in its copy"));
        }
    }
    for (_, at, n) in seen {
        if n > 1 {
            out[at].push_str(&format!(" · {n} times"));
        }
    }
    out
}

/// A stop's conversation, one line each, as the run's story tells it on the page and in the app: "you asked: …",
/// "agent answered, nothing redone: …", "agent answered after the stop closed, nothing changed: …", "agent could not
/// answer: …".
pub fn talk(s: &jaynes_core::run::StepState) -> Vec<String> {
    use jaynes_core::run::EntryKind;
    s.thread
        .iter()
        .map(|e| {
            let said = match e.kind {
                EntryKind::Change => "you:",
                EntryKind::Ask => "you asked:",
                EntryKind::Answer if e.late => "agent answered after the stop closed, nothing changed:",
                EntryKind::Answer => "agent answered, nothing redone:",
                EntryKind::AskFailed => "agent could not answer:",
                EntryKind::Reply => "agent:",
            };
            format!("{said} {}", e.text)
        })
        .collect()
}

/// The story's line of a stop that did not hold the run: what the request settled there, then that the run went on,
/// or that the person's touch at the merge kept it ("◇ At “reproduce”: the bug, as your request says · the run went
/// on"). None for a stop the person answers on its own.
pub fn settled_line(run: &Run, q: &jaynes_core::run::QuestionState) -> Option<String> {
    let s = &q.question;
    if s.kind != jaynes_core::question::QuestionKind::Decision || s.holds {
        return None;
    }
    let how = if q.open() && (run.finished() || crate::stop::went_on(run, q).is_some()) {
        "the run went on"
    } else if run.kept_at_merge(&s.id) {
        "kept when you merged"
    } else {
        return None;
    };
    Some(format!("◇ At “{}”: {} · {how}", s.step, run.settled_said(s)))
}

/// What a stop the request settled showed, under its line in the run's story: the first two parts of its first item,
/// each with its label, the one leading to the other ("now: Exporting a note with a comma breaks the CSV. → after the
/// fix: The comma stays inside its quotes."). `display` is how the item is shown (Store::display); none without parts.
pub fn settled_shows(display: &serde_json::Value) -> Option<String> {
    let parts: Vec<String> = display["aspects"]
        .as_array()?
        .iter()
        .filter_map(|a| {
            let (label, text) = (a["label"].as_str()?.trim(), a["text"].as_str()?.trim());
            let mut l = label.chars();
            let label: String = l.next().map(|c| c.to_lowercase().chain(l).collect()).unwrap_or_default();
            (!label.is_empty() && !text.is_empty()).then(|| format!("{label}: {text}"))
        })
        .take(2)
        .collect();
    (!parts.is_empty()).then(|| parts.join(" → "))
}

/// The person's tests a diff changes: the test files ([`jaynes_core::risk::test_file`]) that were there before, in
/// the diff's order. A test the run wrote anew is not one: it shows among the files as any other.
pub fn changed_tests(diff: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    let push = |file: Option<String>, out: &mut Vec<String>| {
        if let Some(f) = file.filter(|f| jaynes_core::risk::test_file(f)) {
            out.push(f);
        }
    };
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            push(current.take(), &mut out);
            current = rest.split(" b/").last().map(str::to_string);
        } else if line.starts_with("new file mode") {
            current = None;
        }
    }
    push(current, &mut out);
    out
}

/// A run's story: each step its agents worked, in the order they started, with what they did there, turn after turn,
/// read from the transcripts the engine writes as the agents speak, then what was said at its stop. The app shows it
/// in the run's tab.
pub fn story(home: &Path, run: &str) -> Vec<(String, Vec<String>)> {
    let record = home.join("record.sqlite3");
    let state = if record.exists() { Store::open(&record).ok().and_then(|s| s.state(run).ok().flatten()) } else { None };
    story_of(home, run, state.as_ref())
}

/// [`story`] with the run's state at hand: what was said at each step's stop follows what its agent did there.
pub fn story_of(home: &Path, run: &str, state: Option<&Run>) -> Vec<(String, Vec<String>)> {
    let dir = home.join("runs").join(run);
    let project = dir.join("project");
    let setup = Store::open(&home.join("record.sqlite3")).and_then(|s| s.setup(run)).unwrap_or_default();
    let named = setup["project"].as_str().and_then(|p| Path::new(p).file_name()).map(|n| n.to_string_lossy().into_owned());
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
            let mut said: Vec<String> = turns
                .iter()
                .filter_map(|t| tail(t, 1024 * 1024))
                .flat_map(|text| text.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).flat_map(|v| actions(&v, &project)).collect::<Vec<_>>())
                .collect();
            if let Some(r) = state {
                if let Some(s) = r.manifest.index(&name).map(|i| &r.steps[i]) {
                    said.extend(talk(s));
                }
                said.extend(r.questions.values().filter(|q| q.question.step == name && !q.superseded).filter_map(|q| settled_line(r, q)));
            }
            (name, with_refusals(said, named.as_deref()))
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
/// what ended the attempts it spent, the only ones a step's limit counts: a check that refused it, by name, or nothing
/// to check (it ended without a result). Never whose fault.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Work {
    pub attempts: u32,
    pub fixes: u32,
    /// Spent attempts a check refused.
    pub refused: u32,
    /// Spent attempts that ended without a result: no check refused them.
    pub ended: u32,
    /// Each check that refused attempts: its name, whether it is hidden from the agent, how many; most first.
    pub by: Vec<(String, bool, u32)>,
}

/// What the agents of a run did, from its steps: an attempt is a step's go from the start, numbered by the core.
pub fn work(r: &Run) -> Work {
    use jaynes_core::manifest::Executor;
    let attempts = r.manifest.steps.iter().zip(&r.steps).filter(|(d, _)| d.executor == Executor::Agent).map(|(_, s)| s.attempt).sum();
    let mut by: Vec<(String, bool, u32)> = r.counts.refused_by.iter().map(|(name, n)| (name.clone(), n.hidden, n.attempts)).collect();
    by.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    Work { attempts, fixes: r.counts.fixes, refused: r.counts.refused, ended: r.counts.spent.saturating_sub(r.counts.refused), by }
}

impl Work {
    /// "3 attempts by the agent, 1 refused by the check “tests”, 2 fixes"; with more checks, "3 refused by checks:
    /// “criteria-hold” 2, “scope” 1" (three named, then how many more), "(hidden from the agent)" after a hidden one's
    /// name; "1 ended without a result". A part that is none is not said, but the attempts.
    pub fn said(&self) -> String {
        let plural = |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let named = |(name, hidden, _): &(String, bool, u32)| format!("“{name}”{}", if *hidden { " (hidden from the agent)" } else { "" });
        let mut out = if self.attempts == 0 { "no attempt by the agent".to_string() } else { format!("{} by the agent", plural(self.attempts, "attempt", "attempts")) };
        if self.refused > 0 {
            if let [one] = self.by.as_slice() {
                out.push_str(&format!(", {} refused by the check {}", self.refused, named(one)));
            } else {
                let first: Vec<String> = self.by.iter().take(3).map(|c| format!("{} {}", named(c), c.2)).collect();
                let more = self.by.len().saturating_sub(3);
                out.push_str(&format!(", {} refused by checks: {}{}", self.refused, first.join(", "), if more > 0 { format!(" and {more} more") } else { String::new() }));
            }
        }
        if self.ended > 0 {
            out.push_str(&format!(", {} ended without a result", self.ended));
        }
        if self.fixes > 0 {
            out.push_str(&format!(", {}", plural(self.fixes, "fix", "fixes")));
        }
        out
    }

    /// The same, short, for the run's summary: "3 attempts (1 refused by a check), 2 fixes".
    pub fn short(&self) -> String {
        let plural = |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut why = Vec::new();
        if self.refused > 0 {
            why.push(format!("{} refused by {}", self.refused, if self.refused == 1 { "a check" } else { "checks" }));
        }
        if self.ended > 0 {
            why.push(format!("{} ended without a result", self.ended));
        }
        let mut out = plural(self.attempts, "attempt", "attempts");
        if !why.is_empty() {
            out.push_str(&format!(" ({})", why.join(", ")));
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
/// Where the person's Raise the ceiling takes a run, and its button's words: to the ceiling of the run's risk when a
/// risk that rose during the run left it below that (a rule never raises it, T6), or else 30 minutes and 2,000,000
/// tokens over the one it has.
pub fn ceiling_raise(r: &Run) -> (jaynes_core::risk::Ceiling, String) {
    let (c, at) = (r.ceiling, r.ceilings.at(r.risk));
    if at.seconds > c.seconds || at.tokens > c.tokens {
        let to = jaynes_core::risk::Ceiling { seconds: at.seconds.max(c.seconds), tokens: at.tokens.max(c.tokens) };
        return (to, format!("Raise the ceiling to {} min, as a {}-risk run's", to.seconds / 60, r.risk.word()));
    }
    (jaynes_core::risk::Ceiling { seconds: c.seconds + 1800, tokens: c.tokens + 2_000_000 }, "Raise the ceiling by 30 min".to_string())
}

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
    /// The hidden cases that failed at the step's last attempt, for the person: hidden from the agent, never from
    /// them. Read by [`blocked`] from what the engine kept (see [`keep_failed_cases`]); [`blocked_said`] has none.
    pub cases: Vec<CaseSaid>,
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

/// A hidden case that failed, in the card's lines (the design-lead's words, 22/11), top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseSaid {
    /// What the case says, in curly quotes: "“greet says hi with the name”".
    pub says: String,
    /// "Written at “cases”, by the agent there.": an agent wrote it, and it may be wrong.
    pub written: String,
    /// "Ran: `sh greet.sh ana | grep -q hi`".
    pub ran: String,
    /// "Expected: exit 0."
    pub expected: String,
    /// "Got: exit 1."
    pub got: String,
    /// The end of what it printed, a line each, drawn as text: the first [`PRINTED_SHOWN`] shown, the rest under
    /// [`ALL_OF_IT`].
    pub printed: Vec<String>,
    /// "Failed in 3 of 3 attempts."
    pub failed_in: String,
}

/// The heading over the hidden cases that failed, one and more.
pub const CASE_HEADING: &str = "The case that failed";
pub const CASES_HEADING: &str = "The cases that failed";
/// How many lines of what a case printed show before the fold.
pub const PRINTED_SHOWN: usize = 6;
/// The fold over the rest of what a case printed.
pub const ALL_OF_IT: &str = "▸ all of it";

/// The code a case's command exits with when the check stops it for running too long (the menu's timeout).
const TIMED_OUT: i32 = 124;
/// What came, for a case the check stopped for running too long (the design-lead, 22/11).
const GOT_TIMED_OUT: &str = "Got: it did not finish in time, so the check stopped it (exit 124).";

/// Where the engine keeps the hidden cases that failed at an attempt of a step, in the run's folder: beside the
/// folders that hold an agent's step, which no agent reads (agent.rs `beside`).
pub fn failed_cases_file(run_dir: &Path, step: &str, attempt: u32) -> std::path::PathBuf {
    run_dir.join("cases").join(step).join(format!("attempt-{attempt}.json"))
}

/// Keep the hidden cases that failed at `attempt` of `step`, written at the hidden step `at`, for the person's card.
pub fn keep_failed_cases(run_dir: &Path, step: &str, attempt: u32, at: &str, cases: &[jaynes_core::FailedCase]) -> Result<(), String> {
    let file = failed_cases_file(run_dir, step, attempt);
    let dir = file.parent().ok_or("no folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let body = serde_json::json!({"at": at, "cases": cases});
    std::fs::write(&file, body.to_string()).map_err(|e| format!("{}: {e}", file.display()))
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
        if !self.cases.is_empty() {
            out.push(if self.cases.len() == 1 { CASE_HEADING } else { CASES_HEADING }.into());
            for c in &self.cases {
                out.extend([c.says.clone(), c.written.clone(), c.ran.clone(), c.expected.clone(), c.got.clone()]);
                out.extend(c.printed.iter().take(PRINTED_SHOWN).cloned());
                if c.printed.len() > PRINTED_SHOWN {
                    out.push(ALL_OF_IT.into());
                }
                out.push(c.failed_in.clone());
            }
        }
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
/// The words of a check whose command tried to start a browser of the person's (jaynes_menu.py), which no port helps.
const TRIED_BROWSER: &str = " It tried to start a browser of this Mac";
const OWN_CHOOSING: &str = "0, a port of its own choosing";
const REACH_ONLY: &str = " A check's commands bind and reach only the ports in JAYNES_PORTS (";
const OPEN_DID_NOT_HELP: &str = " The person's open ports did not help here.";
const SERVERS_DID_NOT_HELP: &str = " The project's test servers did not help here.";
const OPEN_PORTS: &str = "`jaynes project open-ports ";
/// The words of a check whose test server hit a port Jaynes holds (jaynes_menu.py): never the project's doing.
const HELD_KEPT: &str = "Jaynes held a port the command tried to serve on, so its failure says nothing about the project: ";
const HELD_TRIED: &str = " It tried held port ";
const HELD_LET: &str = "`jaynes project test-servers ";
/// The words of a check whose own test server got a port macOS gave past the ones the box let the tests reach
/// (jaynes_menu.py), and what the driver adds when the same block comes back on the same change after a Continue.
const PAST_TRIED: &str = " It tried port ";
const PAST_GAVE: &str = ", which macOS gave after the ";
pub const PAST_AGAIN: &str = " It happened again.";

/// Whether a check's reason says its own test server got a port past the box's window (see [`PAST_AGAIN`]).
pub fn past_window_said(reason: &str) -> bool {
    reason.contains(BOX_REFUSED) && reason.contains(PAST_GAVE)
}

/// A blocked run's words, with what its agent said last at the step it stopped at; None when it is not blocked.
pub fn blocked(home: &Path, r: &Run) -> Option<BlockedSaid> {
    let Phase::Blocked { step, fault, reason } = &r.phase else { return None };
    let mut said = blocked_said(step.as_deref(), *fault, reason);
    if let Some(s) = step {
        said.agent_said = agent_said(home, &r.id, s);
    }
    // a hidden case failed at the last attempt: the person sees it, as the agent never does
    if let (Some(s), jaynes_core::event::Fault::Agent) = (step, fault)
        && reason.contains(HIDDEN_FAILED)
        && let Some(i) = r.manifest.index(s)
    {
        let (attempt, spent) = (r.steps[i].attempt, r.steps[i].spent);
        let (cases, each) = cases_said(&home.join("runs").join(&r.id), s, attempt, spent);
        if !cases.is_empty() {
            let failed = if each { "Each time, a case the agent does not see failed." } else { "A case the agent does not see failed." };
            said.why = format!("“{s}” used its {spent} attempts. {failed}");
            said.cases = cases;
        }
    }
    // stopped at its start for a held port: Continue names the step the run goes on to
    if said.continue_does == HELD_GOES_ON
        && let Some(next) = step.as_deref().and_then(|s| r.manifest.index(s)).and_then(|i| r.manifest.steps.get(i + 1))
    {
        said.continue_does = format!("{} to “{}”.", HELD_GOES_ON.trim_end_matches('.'), next.name);
    }
    // the engine keeps the change a check could not judge: Continue judges it again, the agent working no more
    if r.checks_again().is_some() {
        said = said.checks_again();
    }
    Some(said)
}

/// The words a hidden case that failed is told to the agent in, by the menu's check (jaynes_menu.py): the card
/// finds such a block by them.
const HIDDEN_FAILED: &str = "A check you cannot see found that this does not hold: ";

/// The hidden cases that failed at `attempt` of `step`, the last of the `spent` attempts of the block, each with the
/// number of those attempts it failed in; and whether every one of them failed at a hidden case. None at all when
/// the engine kept none for `attempt`.
fn cases_said(run_dir: &Path, step: &str, attempt: u32, spent: u32) -> (Vec<CaseSaid>, bool) {
    let read = |n: u32| -> Option<(String, Vec<jaynes_core::FailedCase>)> {
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(failed_cases_file(run_dir, step, n)).ok()?).ok()?;
        Some((v["at"].as_str().unwrap_or_default().to_string(), serde_json::from_value(v["cases"].clone()).ok()?))
    };
    let Some((at, last)) = read(attempt) else { return (Vec::new(), false) };
    let window: Vec<Vec<jaynes_core::FailedCase>> = (attempt.saturating_sub(spent.max(1)) + 1..=attempt).filter_map(read).map(|(_, c)| c).collect();
    let each = window.len() as u32 == spent.max(1);
    let line = |l: &str| {
        let l = one_line(l);
        if l.chars().count() > 400 { format!("{}…", l.chars().take(399).collect::<String>()) } else { l }
    };
    let mut out: Vec<CaseSaid> = Vec::new();
    let mut seen = Vec::new();
    for c in &last {
        if seen.contains(&(&c.says, &c.run)) {
            continue;
        }
        seen.push((&c.says, &c.run));
        let failed = window.iter().filter(|w| w.iter().any(|x| x.says == c.says && x.run == c.run)).count();
        out.push(CaseSaid {
            says: format!("“{}”", one_line(&c.says)),
            written: if at.is_empty() { "Written by an agent.".into() } else { format!("Written at “{at}”, by the agent there.") },
            ran: format!("Ran: `{}`", one_line(&c.run)),
            expected: "Expected: exit 0.".into(),
            got: if c.code == TIMED_OUT { GOT_TIMED_OUT.to_string() } else { format!("Got: exit {}.", c.code) },
            printed: c.printed.lines().map(line).filter(|l| !l.is_empty()).collect(),
            failed_in: format!("Failed in {failed} of {} attempts.", spent.max(1)),
        });
    }
    (out, each)
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
    /// The sentence of the person's browser it tried to start, which the box keeps every check's command from.
    browser: Option<&'a str>,
}

fn boxed(said: &str) -> Option<Boxed<'_>> {
    let rest = said.strip_prefix(BOX_REFUSED)?;
    let end = [TRIED_PORT, REACH_ONLY, OPEN_DID_NOT_HELP, SERVERS_DID_NOT_HELP, TRIED_BROWSER].iter().filter_map(|m| rest.find(m)).min().unwrap_or(rest.len());
    let (lines, tail) = rest.split_at(end);
    let numbers = |t: &str| t.split(", ").filter_map(|p| p.trim().parse().ok()).collect::<Vec<u16>>();
    Some(Boxed {
        refused: lines.split("; ").map(str::trim).filter(|l| !l.is_empty()).collect(),
        ports: tail.split_once(TRIED_PORT).map(|(_, t)| numbers(t.replace(OWN_CHOOSING, "0").split('.').next().unwrap_or(""))).unwrap_or_default(),
        given: tail.split_once(REACH_ONLY).and_then(|(_, t)| t.split_once(')')).map(|(g, _)| numbers(g)).unwrap_or_default(),
        project: tail.split_once(OPEN_PORTS).and_then(|(_, t)| t.split_once('`')).map(|(p, _)| p),
        open_did_not_help: tail.contains(OPEN_DID_NOT_HELP),
        servers_did_not_help: tail.contains(SERVERS_DID_NOT_HELP),
        browser: tail.find(TRIED_BROWSER).map(|at| tail[at..].trim()),
    })
}

/// A check whose own test server got a port macOS gave past the ones the box let the tests reach, from what the box's
/// check said: the lines it printed, the sentence of the port tried, how many the box let, the project's folder, and
/// whether it came back on the same change after a Continue.
struct Past<'a> {
    lines: Vec<&'a str>,
    tried: &'a str,
    let_reach: &'a str,
    project: Option<&'a str>,
    again: bool,
}

fn past(said: &str) -> Option<Past<'_>> {
    let rest = said.strip_prefix(BOX_REFUSED)?;
    let (lines, tail) = rest.split_at(rest.find(PAST_TRIED)?);
    let tried = tail[..tail.find(").")? + 2].trim();
    let let_reach = tried.split_once(PAST_GAVE)?.1.split(' ').next()?;
    Some(Past {
        lines: lines.split("; ").map(str::trim).filter(|l| !l.is_empty()).collect(),
        tried,
        let_reach,
        project: tail.split_once(OPEN_PORTS).and_then(|(_, t)| t.split_once('`')).map(|(p, _)| p),
        // at the reason's end, its period goes with "No attempt was spent" (`unspent`)
        again: tail.contains(PAST_AGAIN.trim_end_matches('.')),
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
    // a role program's step (the runner's find-suite) says it after its own words
    let said = said.strip_prefix("the role program could not answer: ").unwrap_or(said);
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
pub fn shell_word(s: &str) -> String {
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

/// The held ports a project's own files write as a number of their own ("PORT = 5432", "localhost:6379"), in the
/// order of [`HELD_PORTS`]: where its tests may serve on one. The rules of the box's check (jaynes_menu.py
/// held_named): what git keeps, Jaynes's own, installed dependencies and lock files are never read, nor a link, nor
/// a file past 256 KiB or one not in UTF-8, and at most 2000 files.
pub fn held_named(project: &Path) -> Vec<u16> {
    const NOT_READ: &[&str] = &[".git", ".jaynes", "__pycache__", "node_modules", ".venv", "venv", "vendor", "bower_components"];
    const LOCKS: &[&str] = &["uv.lock", "poetry.lock", "Pipfile.lock", "package-lock.json", "yarn.lock", "pnpm-lock.yaml", "bun.lock", "Cargo.lock", "go.sum", "composer.lock", "Gemfile.lock", "mix.lock"];
    let mut texts = Vec::new();
    let mut dirs = vec![project.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() && !NOT_READ.contains(&name.as_str()) {
                dirs.push(e.path());
            } else if kind.is_file() && !LOCKS.contains(&name.as_str()) && texts.len() < 2000
                && e.metadata().is_ok_and(|m| m.len() <= 256 * 1024)
                && let Ok(text) = std::fs::read_to_string(e.path())
            {
                texts.push(text);
            }
        }
    }
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let named = |port: u16| {
        let p = port.to_string();
        texts.iter().any(|t| {
            t.match_indices(&p).any(|(at, _)| {
                let before = t[..at].chars().next_back();
                let mut after = t[at + p.len()..].chars();
                let next = after.next();
                !before.is_some_and(|c| word(c) || c == '.') && !next.is_some_and(word) && !(next == Some('.') && after.next().is_some_and(|c| c.is_ascii_digit()))
            })
        })
    };
    HELD_PORTS.iter().map(|(p, _)| *p).filter(|p| named(*p)).collect()
}

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

/// A step of Jaynes's own that ran the project's tests before any agent worked and found they serve on ports Jaynes
/// holds while the checks run (the tech-lead's, 22/11): every check would fail for that, whatever the change, so the
/// run stopped there; the way out is one line, and Continue runs the tests again.
/// What Continue does at a run stopped at its start for a held port; [`blocked`] names the step it goes on to.
const HELD_GOES_ON: &str = "Continue: Jaynes runs the project's tests again. If they start, the run goes on.";

fn held_at_start(said: &mut BlockedSaid, h: &Held<'_>) {
    // the design-lead's words (bloqueada.md, 22/11): what happened first, then the port as the check's card says it
    said.what = "The run stopped before any agent worked.".into();
    said.why = format!(
        "This project's tests start a test server on {}, which Jaynes holds while the checks run so no test stands in for {}. Every check would fail, whatever the change.",
        ports_named(&h.ports),
        held_kind(&h.ports)
    );
    said.continue_does = HELD_GOES_ON.into();
    // what already says no agent worked (the design-lead's of 22/11)
    said.holds = None;
    if let Some(project) = h.project {
        said.ways.push(Way {
            says: format!("Let this project's tests use {}, then Continue.", ports_named(&h.ports)),
            then: Some(freed_risk(&h.ports)),
            command: Some(format!("jaynes project test-servers {} {}", shell_word(project), h.ports.iter().map(u16::to_string).collect::<Vec<_>>().join(" "))),
            still_open: Vec::new(),
        });
    }
    said.ways.push(Way { says: "Or abandon: the run ends. Your project stays as it was.".into(), then: None, command: None, still_open: Vec::new() });
    let mut details: Vec<String> = once(h.lines.iter().copied());
    details.push(format!("It tried {}, which Jaynes holds while the checks run.", ports_named(&h.ports)));
    said.details = details;
}

/// A step of Jaynes's own that ran the project's tests before any agent worked and found the box kept them from the
/// port they serve on (the tech-lead's T5, 23/11): every check would fail for that, whatever the change, so the run
/// stopped there; the way out is the one line that serves what they tried, and Continue runs the tests again.
fn served_at_start(said: &mut BlockedSaid, b: &Boxed<'_>) {
    let fixed = fixed_ports(b);
    // one way out, as a check's card gives it: a port the system picks gets the project's test servers
    let own = fixed.is_empty() && cfg!(target_os = "macos") && TEST_SERVERS;
    said.what = "The run stopped before any agent worked.".into();
    said.why = if fixed.is_empty() {
        "This project's tests start a test server on a port the system picks, and the box Jaynes checks in allows only its own ports. Every check would fail, whatever the change.".into()
    } else {
        format!("This project's tests reach {} on {}, and the box Jaynes checks in allows only its own ports. Every check would fail, whatever the change.", ports_named(&fixed), this_machine())
    };
    said.continue_does = HELD_GOES_ON.into();
    // what already says no agent worked
    said.holds = None;
    if let Some(project) = b.project {
        said.ways.push(port_way(project, own));
    }
    said.ways.push(Way { says: "Or abandon: the run ends. Your project stays as it was.".into(), then: None, command: None, still_open: Vec::new() });
    let mut details = once(b.refused.iter().map(|l| go_annotation_off(l)));
    let tried: Vec<String> = b.ports.iter().map(|&p| if p == 0 { OWN_CHOOSING.to_string() } else { p.to_string() }).collect();
    details.push(format!("It tried port{} {}.", if tried.len() == 1 { "" } else { "s" }, tried.join(", ")));
    said.details = details;
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
    if let Some(p) = past(inner) {
        // the design-lead's words of 22/11: the tests' own server, on a port macOS gave after the box's window; the
        // engine cannot tell whether the suite or another program took the ports, so the first time Continue comes
        // first, and when it comes back on the same change the tests themselves likely did
        let mac = this_machine();
        said.why = format!("The project's tests started a test server after more than {} connections on {mac} during the same command, so macOS gave it a port past the ones the box lets the tests reach.", p.let_reach);
        if p.again {
            said.why.push_str(" It happened again, so the tests themselves likely make that many connections.");
        } else {
            said.ways.push(Way { says: format!("Continue to check again: if other programs on {mac} used those ports, the tests may fit this time."), then: None, command: None, still_open: Vec::new() });
        }
        if let Some(project) = p.project {
            let mut open = port_way(project, false);
            if !p.again {
                open.says = "If it happens again, open this project's ports, then Continue.".into();
            }
            said.ways.push(open);
        }
        said.ways.push(Way { says: "Or abandon: the run ends and its change is not kept. Your project stays as it was.".into(), then: None, command: None, still_open: Vec::new() });
        details.extend(once(p.lines.iter().copied()));
        details.push(p.tried.to_string());
        details.push("Or the suite takes its test servers' ports from JAYNES_PORTS, and the box stays closed.".into());
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
            details.extend(b.browser.map(sentence));
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
        cases: Vec::new(),
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
                if let Some(h) = held_at_start_of(step, fault, reason) {
                    held_at_start(&mut said, &h);
                } else if let Some(b) = served_at_start_of(step, fault, reason) {
                    served_at_start(&mut said, &b);
                } else if let Some(why) = after(step.map(|s| format!("a fault of Jaynes or of the machine stopped {s}: "))) {
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
    use jaynes_core::FailedCase;

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

    /// The reason for a check whose own test server got a port macOS gave past the window the box let it reach, once
    /// or `again` after a Continue on the same change.
    fn past_window(project: &str, again: bool) -> String {
        format!(
            "the check suite-passes of change could not judge: the box refused what the command tried, so its failure says nothing about the project: the test server on ::1:53342 out of reach: [Errno 1] Operation not permitted It tried port 53342, which macOS gave after the 4,000 the box let the tests reach (49200 to 53199). The person may open this project's ports with `jaynes project open-ports {project}`.{}. No attempt was spent",
            if again { PAST_AGAIN } else { "" }
        )
    }

    #[test]
    fn a_check_that_tried_to_start_a_browser_says_so_apart_and_offers_no_ports() {
        // the box keeps every check's command from starting the person's browsers; no port the person opens changes
        // that, so the card offers none, and the browser's sentence is a line of its own under Details
        let program = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
        let reason = format!(
            "the check tests of change could not judge: the box refused what the command tried, so its failure says nothing about the project: /bin/sh: {program}: Operation not permitted It tried to start a browser of this Mac, which no check's command starts: {program}.. No attempt was spent"
        );
        let b = blocked_said(Some("change"), Fault::Jaynes, &reason);
        assert_eq!(b.what, "The change is done. Jaynes could not check it.");
        assert!(b.ways.iter().all(|w| w.command.is_none()), "{b:?}");
        assert_eq!(b.details, [
            format!("/bin/sh: {program}: Operation not permitted"),
            format!("It tried to start a browser of this Mac, which no check's command starts: {program}."),
        ]);
    }

    #[test]
    fn a_test_server_past_the_ports_the_box_lets_says_so_and_offers_to_check_again_then_to_open_the_ports() {
        // the design-lead's words of 22/11: the tests' own server, never a service of the person's
        let b = blocked_said(Some("change"), Fault::Jaynes, &past_window("/Users/x/Projects/app", false));
        assert_eq!(b.what, "The change is done. Jaynes could not check it.");
        let mac = this_machine();
        let why = format!("The project's tests started a test server after more than 4,000 connections on {mac} during the same command, so macOS gave it a port past the ones the box lets the tests reach.");
        assert_eq!(b.why, why);
        assert_eq!(b.ways, [
            Way { says: format!("Continue to check again: if other programs on {mac} used those ports, the tests may fit this time."), then: None, command: None, still_open: Vec::new() },
            Way {
                says: "If it happens again, open this project's ports, then Continue.".into(),
                then: Some(format!("Its checks may then reach any service on {mac}, the Jaynes page among them.")),
                command: Some("jaynes project open-ports /Users/x/Projects/app".into()),
                still_open: Vec::new(),
            },
            abandon(),
        ]);
        assert_eq!(b.details, [
            "the test server on ::1:53342 out of reach: [Errno 1] Operation not permitted".to_string(),
            "It tried port 53342, which macOS gave after the 4,000 the box let the tests reach (49200 to 53199).".to_string(),
            "Or the suite takes its test servers' ports from JAYNES_PORTS, and the box stays closed.".to_string(),
        ]);
        assert_eq!(b.holds.as_deref(), Some(NO_ATTEMPT_USED));
        // the same block again on the same change: the tests themselves, so no Continue, the ports first
        let again = blocked_said(Some("change"), Fault::Jaynes, &past_window("/Users/x/my app", true));
        assert_eq!(again.why, format!("{why} It happened again, so the tests themselves likely make that many connections."));
        assert_eq!(again.ways, [
            Way {
                says: "Open this project's ports, then Continue.".into(),
                then: Some(format!("Its checks may then reach any service on {mac}, the Jaynes page among them.")),
                command: Some("jaynes project open-ports '/Users/x/my app'".into()),
                still_open: Vec::new(),
            },
            abandon(),
        ]);
        assert_eq!(again.details, b.details);
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
    fn tests_that_serve_on_a_port_jaynes_holds_stop_the_run_before_any_agent_with_the_one_line_way_out() {
        // the tech-lead's, 22/11 (the real-user's estoque): the suite of the start serves on a port Jaynes holds, so
        // every check would fail for Jaynes, never for the run; the run stops at its first step, the agent never works
        let bind = "E       OSError: [Errno 48] Address already in use";
        // as a real run says it: the role program's words before the box's
        let inner = format!("the role program could not answer: Jaynes held a port the command tried to serve on, so its failure says nothing about the project: {bind} It tried held port 5432. The person lets this project's tests use them with `jaynes project test-servers /Users/x/Projects/estoque`, naming the ports.");
        let b = blocked_said(Some("runner"), Fault::Jaynes, &format!("a fault of Jaynes or of the machine stopped runner: {inner}. No attempt was spent"));
        // the design-lead's words (bloqueada.md, 22/11): what happened first, the port in why, as the check's card says it
        assert_eq!(b.what, "The run stopped before any agent worked.");
        assert_eq!(b.why, "This project's tests start a test server on port 5432, which Jaynes holds while the checks run so no test stands in for a database. Every check would fail, whatever the change.");
        assert_eq!(b.ways, [
            Way {
                says: "Let this project's tests use port 5432, then Continue.".into(),
                then: Some("Only if nothing of yours runs on 5432 while the tests run: the tests could reach it, or answer in its place.".into()),
                command: Some("jaynes project test-servers /Users/x/Projects/estoque 5432".into()),
                still_open: Vec::new(),
            },
            Way { says: "Or abandon: the run ends. Your project stays as it was.".into(), then: None, command: None, still_open: Vec::new() },
        ]);
        assert_eq!(b.details, [bind.to_string(), "It tried port 5432, which Jaynes holds while the checks run.".to_string()]);
        assert_eq!(b.continue_does, "Continue: Jaynes runs the project's tests again. If they start, the run goes on.");
        assert_eq!(b.waits.as_deref(), Some(BLOCKED_WAITS));
        // the design-lead's of 22/11: what already says no agent worked, so the card keeps no "No attempt was used."
        assert_eq!(b.holds, None);
        // the line of the Inbox and the column says the case, one port or several
        let reason = format!("a fault of Jaynes or of the machine stopped runner: {inner}. No attempt was spent");
        assert_eq!(blocked_words(Some("runner"), Fault::Jaynes, &reason), "blocked: its tests need port 5432");
        let two = reason.replace("held port 5432.", "held port 3306, 5432.");
        assert_eq!(blocked_words(Some("runner"), Fault::Jaynes, &two), "blocked: its tests need ports 3306 and 5432");
        // any other block keeps its reason in words
        assert_eq!(blocked_words(Some("change"), Fault::Agent, "change exited with Some(2): "), "blocked: change exited with code 2");
        // with the run, Continue names the step it goes on to
        let mut r = super::tests::run("r1", "Say hi");
        r.phase = Phase::Blocked { step: Some("runner".into()), fault: Fault::Jaynes, reason: format!("a fault of Jaynes or of the machine stopped runner: {inner}. No attempt was spent") };
        r.manifest.steps.insert(0, jaynes_core::manifest::StepDecl { name: "runner".into(), ..r.manifest.steps[0].clone() });
        let next = r.manifest.steps[1].name.clone();
        let home = std::env::temp_dir().join(format!("jaynes-held-start-{}", std::process::id()));
        let carded = blocked(&home, &r).expect("blocked");
        let _gone = std::fs::remove_dir_all(&home);
        assert_eq!(carded.continue_does, format!("Continue: Jaynes runs the project's tests again. If they start, the run goes on to “{next}”."));
        // a cache says so, two ports are named together
        let cache = blocked_said(Some("runner"), Fault::Jaynes, &format!("a fault of Jaynes or of the machine stopped runner: {}. No attempt was spent", inner.replace("5432", "6379")));
        assert!(cache.why.contains("on port 6379, which Jaynes holds while the checks run so no test stands in for a cache."), "{}", cache.why);
        // any other fault of Jaynes at a step of its own keeps its words
        let other = blocked_said(Some("runner"), Fault::Jaynes, "a fault of Jaynes or of the machine stopped runner: the disk is full. No attempt was spent");
        assert_eq!(other.why, "The disk is full.");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn tests_that_start_their_own_test_server_without_the_person_s_yes_stop_the_run_before_any_agent_with_the_line_that_lets_them() {
        // the tech-lead's T5, 23/11 (the real-user's vazio-x): the suite of the start serves on a port the system
        // picks and the person has not let the project's test servers, so every check would fail for it; the run
        // stops at its first step with the one line that lets them, never in the middle
        let bind = "panic: httptest: failed to listen on a port: listen tcp6 [::1]:0: bind: operation not permitted [recovered]";
        let inner = format!("the role program could not answer: {BOX_REFUSED}{bind} It tried local port 0, a port of its own choosing. A check's commands bind and reach only the ports in JAYNES_PORTS (52000, 52001) and the ones the check names in its local_ports, never its step's. Two ways out, the first the better: (1) the suite takes its test servers' ports from JAYNES_PORTS, and the box stays closed; (2) the person opens this project with `jaynes project open-ports /Users/x/Projects/recados`, and its checks then reach every service listening on this machine, the Jaynes page among them.");
        let reason = format!("a fault of Jaynes or of the machine stopped runner: {inner}. No attempt was spent");
        let b = blocked_said(Some("runner"), Fault::Jaynes, &reason);
        assert_eq!(b.what, "The run stopped before any agent worked.");
        assert_eq!(b.why, "This project's tests start a test server on a port the system picks, and the box Jaynes checks in allows only its own ports. Every check would fail, whatever the change.");
        assert_eq!(b.ways, [
            port_way("/Users/x/Projects/recados", true),
            Way { says: "Or abandon: the run ends. Your project stays as it was.".into(), then: None, command: None, still_open: Vec::new() },
        ]);
        assert_eq!(b.ways[0].command.as_deref(), Some("jaynes project test-servers /Users/x/Projects/recados"));
        assert_eq!(b.continue_does, HELD_GOES_ON);
        assert_eq!(b.holds, None, "what already says no agent worked");
        assert_eq!(b.details.first().map(String::as_str), Some("panic: httptest: failed to listen on a port: listen tcp6 [::1]:0: bind: operation not permitted"));
        assert_eq!(blocked_words(Some("runner"), Fault::Jaynes, &reason), "blocked: its tests start a test server");
        // with the run, Continue names the step it goes on to
        let mut r = super::tests::run("r1", "Say hi");
        r.phase = Phase::Blocked { step: Some("runner".into()), fault: Fault::Jaynes, reason: reason.clone() };
        r.manifest.steps.insert(0, jaynes_core::manifest::StepDecl { name: "runner".into(), ..r.manifest.steps[0].clone() });
        let next = r.manifest.steps[1].name.clone();
        let home = std::env::temp_dir().join(format!("jaynes-serves-start-{}", std::process::id()));
        let carded = blocked(&home, &r).expect("blocked");
        let _gone = std::fs::remove_dir_all(&home);
        assert_eq!(carded.continue_does, format!("Continue: Jaynes runs the project's tests again. If they start, the run goes on to “{next}”."));
        // a check's own refusal, later in a run, keeps its card
        let later = blocked_said(Some("change"), Fault::Jaynes, &format!("the check suite-passes could not judge: {}", inner.trim_start_matches("the role program could not answer: ")));
        assert_eq!(later.what, "The change is done. Jaynes could not check it.");
    }

    #[test]
    fn the_held_ports_a_project_s_files_name_are_found_as_the_box_s_check_finds_them() {
        // the same rules as jaynes_menu.py held_named (python/tests/test_held_ports.py): a number of its own, never
        // digits inside another, a version or a hash; locks, dependencies and git unread; the box's order, each once
        let root = std::env::temp_dir().join(format!("jaynes-held-named-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&root);
        let write = |path: &str, text: &str| {
            let p = root.join(path);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("dir");
            std::fs::write(p, text).expect("write");
        };
        write("estoque/db.py", "HOST = \"127.0.0.1\"\nPORT = 5432\n");
        write("compose.yml", "redis: 6379\n");
        write("b.env", "PG=5432\n");
        write("a.py", "N = 15432\nX = 54321\nV = '1.3306'\nH = 'sha256:ab27017cd'\nREADME = 'on 11211.'\n");
        for (path, text) in [("uv.lock", "9200"), ("node_modules/pg/index.js", "5672"), (".git/config", "9092"), (".venv/x.py", "1433"), ("go.sum", "1521")] {
            write(path, text);
        }
        let found = held_named(&root);
        let _gone = std::fs::remove_dir_all(&root);
        assert_eq!(found, [5432, 6379, 11211]);
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
        assert_eq!(work(&r), Work { attempts: 1, ..Default::default() });
        assert_eq!(work(&r).said(), "1 attempt by the agent");
        assert_eq!(stages(&r)[1].detail, "blocked, 1 attempt");
        assert!(!stages(&r).iter().any(|s| s.name == "plan" && s.detail.contains("attempt")), "{:?}", stages(&r));

        for e in [
            Event::Continued { by: By::Person, words: None },
            started("change", 2),
            Event::FixAsked { step: "change".into(), attempt: 2, fix: 1, reasons: vec!["the suite fails".into()] },
        ] {
            r.apply(&e);
        }
        // the agent worked twice, its second attempt with a fix in it
        assert_eq!(work(&r).said(), "2 attempts by the agent, 1 fix");
        assert_eq!(stages(&r)[1].detail, "running, attempt 2");
    }

    // The tech-lead and the design-lead (22/11, quatro-itens.md, item 6): "3 failed through its own fault" blamed the
    // agent, and in the finding the criterion was what was wrong. The count says what ended each attempt: the check
    // that refused it, by name (one the agent cannot see said so), or that it ended without a result; the summary
    // says it short.
    #[test]
    fn asked_the_attempts_say_what_refused_them_never_whose_fault() {
        use jaynes_core::event::{CheckRecord, Event, Usage};
        use jaynes_core::manifest::CheckKind;
        let mut r = super::tests::run("r1", "Filter the items by price");
        let started = |attempt: u32| Event::AttemptStarted { step: "change".into(), round: 0, attempt, fix: 0, session: None, deadline_seconds: 0, tokens_left: 0, key: None };
        let delivered = |attempt: u32| Event::AttemptDelivered { step: "change".into(), attempt, fix: 0, output: jaynes_core::Digest("o".into()), items: vec![], changed: vec![], usage: Usage::default(), session: None, tree: None, reply: None };
        let check = |name: &str, kind: CheckKind, passed: bool| CheckRecord { check: name.into(), kind, passed, blocked: false, read: vec![], tree: None };
        let judged = |attempt: u32, checks: Vec<CheckRecord>| Event::ChecksJudged { step: "change".into(), attempt, fix: 0, checks };
        let failed = |attempt: u32| Event::AttemptFailed { step: "change".into(), attempt, reasons: vec!["no".into()], usage: Usage::default(), spent: true };
        for e in [started(1), delivered(1), judged(1, vec![check("tests", CheckKind::Open, false), check("scope", CheckKind::Open, true)]), failed(1)] {
            r.apply(&e);
        }
        assert_eq!(work(&r).said(), "1 attempt by the agent, 1 refused by the check “tests”");
        for e in [
            started(2),
            delivered(2),
            judged(2, vec![check("criteria-hold", CheckKind::Hidden, false), check("scope", CheckKind::Open, true)]),
            failed(2),
            started(3),
            delivered(3),
            judged(3, vec![check("criteria-hold", CheckKind::Hidden, false), check("scope", CheckKind::Open, false)]),
            failed(3),
            // the agent's turn ended with nothing to check: no check refused it
            started(4),
            failed(4),
        ] {
            r.apply(&e);
        }
        let w = work(&r);
        assert_eq!(
            w.said(),
            "4 attempts by the agent, 3 refused by checks: “criteria-hold” (hidden from the agent) 2, “scope” 1, “tests” 1, 1 ended without a result"
        );
        assert_eq!(w.short(), "4 attempts (3 refused by checks, 1 ended without a result)");
        // past three checks, how many more; fixes as fixes; one refused, said short
        let more = Work { attempts: 5, fixes: 2, refused: 5, ended: 0, by: ["a", "b", "c", "d", "e"].iter().map(|n| (n.to_string(), false, 1)).collect() };
        assert_eq!(more.said(), "5 attempts by the agent, 5 refused by checks: “a” 1, “b” 1, “c” 1 and 2 more, 2 fixes");
        let one = Work { attempts: 3, fixes: 2, refused: 1, ended: 0, by: vec![("tests".into(), false, 1)] };
        assert_eq!(one.said(), "3 attempts by the agent, 1 refused by the check “tests”, 2 fixes");
        assert_eq!(one.short(), "3 attempts (1 refused by a check), 2 fixes");
        assert_eq!(Work { attempts: 1, ..Default::default() }.short(), "1 attempt");
        assert!(!w.said().contains("fault"));
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
        for words in [BOX_REFUSED, TRIED_PORT, OWN_CHOOSING, REACH_ONLY, OPEN_DID_NOT_HELP, SERVERS_DID_NOT_HELP, OPEN_PORTS, HIDDEN_FAILED, PAST_TRIED, PAST_GAVE, TRIED_BROWSER] {
            assert!(menu.contains(words.trim_start()), "jaynes_menu.py no longer says {words:?}");
        }
    }

    /// A hidden case as a test writes it: what it says, what it runs, the code it exited with, what it printed.
    type Case<'a> = (&'a str, &'a str, i32, &'a str);

    /// A home whose run `r1` kept, for step `change`, the hidden cases that failed at each of `attempts`.
    fn failed_cases(name: &str, attempts: &[(u32, &[Case])]) -> std::path::PathBuf {
        let home = std::env::temp_dir().join(format!("jaynes-cases-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        for (attempt, cases) in attempts {
            let cases: Vec<FailedCase> = cases.iter().map(|(says, run, code, printed)| FailedCase { says: says.to_string(), run: run.to_string(), code: *code, printed: printed.to_string() }).collect();
            keep_failed_cases(&home.join("runs/r1"), "change", *attempt, "cases", &cases).expect("keep");
        }
        home
    }

    // The real-user, 22/11 (r1795330821-771): "O cartão do bloqueio diz só "A check you cannot see found that this
    // does not hold". A pessoa não vê o caso, não sabe o que está errado e não tem o que fazer". The case is hidden
    // from the agent, never from the person: the card shows the case that failed, what it expected and what came.
    #[test]
    fn asked_a_run_blocked_by_a_hidden_case_shows_the_person_the_case_what_it_expected_and_what_came() {
        let history = "with the old render.py of commit 8527993 the suite must fail";
        let git = "git show 8527993:render.py > render.py && ! make test";
        let printed = (1..=9).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
        let other = "the total counts the quantity";
        let home = failed_cases("each", &[
            (1, &[(history, git, 128, "fatal: bad object 8527993")]),
            (2, &[(history, git, 128, "fatal: bad object 8527993"), (other, "make test", 2, &printed)]),
            (3, &[(history, git, 128, "fatal: bad object 8527993\x1b[2J")]),
        ]);
        let mut r = super::tests::run("r1", "Fix the total");
        let i = r.manifest.index("change").expect("step");
        r.steps[i].attempt = 3;
        r.steps[i].spent = 3;
        let reason = format!("change spent its 3 attempts: A check you cannot see found that this does not hold: {history}");
        r.phase = Phase::Blocked { step: Some("change".into()), fault: Fault::Agent, reason };

        let b = blocked(&home, &r).expect("blocked");
        let _gone = std::fs::remove_dir_all(&home);

        assert_eq!(b.what, "The agent could not finish “change”.");
        // each of the three attempts failed at a hidden case: "Each time"
        assert_eq!(b.why, "“change” used its 3 attempts. Each time, a case the agent does not see failed.");
        assert!(!b.why.contains("A check you cannot see"), "{}", b.why);
        // the case of the last attempt, in full: what it says, who wrote it, what ran, what it expected, what came
        assert_eq!(b.cases, [CaseSaid {
            says: format!("“{history}”"),
            written: "Written at “cases”, by the agent there.".into(),
            ran: format!("Ran: `{git}`"),
            expected: "Expected: exit 0.".into(),
            got: "Got: exit 128.".into(),
            // what the project's code printed is drawn as text, never as the terminal's own
            printed: vec!["fatal: bad object 8527993 [2J".into()],
            failed_in: "Failed in 3 of 3 attempts.".into(),
        }]);
        let text = b.text();
        for line in ["The case that failed", &format!("“{history}”"), "Written at “cases”, by the agent there.", &format!("Ran: `{git}`"), "Expected: exit 0.", "Got: exit 128.", "Failed in 3 of 3 attempts."] {
            assert!(text.lines().any(|l| l == line), "{line:?} not a line of the card:\n{text}");
        }
        // the case comes after why, and before what the person can do
        let at = |l: &str| text.lines().position(|x| x == l).unwrap_or_else(|| panic!("{l:?} in\n{text}"));
        assert!(at(&b.why) < at("The case that failed") && at("The case that failed") < at(&b.continue_does), "{text}");

        // two cases at the last attempt, one failing once: no "Each time", and what it printed beyond six lines folds
        let home = failed_cases("two", &[
            (2, &[(history, git, 128, "fatal: bad object 8527993")]),
            (3, &[(history, git, 128, "fatal: bad object 8527993"), (other, "make test", 124, &printed)]),
        ]);
        let b = blocked(&home, &r).expect("blocked");
        let _gone = std::fs::remove_dir_all(&home);
        assert_eq!(b.why, "“change” used its 3 attempts. A case the agent does not see failed.");
        assert_eq!(b.cases.iter().map(|c| c.failed_in.as_str()).collect::<Vec<_>>(), ["Failed in 2 of 3 attempts.", "Failed in 1 of 3 attempts."]);
        assert_eq!(b.cases[1].printed.len(), 9);
        // a case the check stopped because it did not finish in time says so, not a bare code (the design-lead, 22/11)
        assert_eq!(b.cases[1].got, "Got: it did not finish in time, so the check stopped it (exit 124).");
        assert_eq!(b.cases[0].got, "Got: exit 128.");
        let text = b.text();
        assert!(text.lines().any(|l| l == "The cases that failed"), "{text}");
        assert!(text.contains("line 6\n▸ all of it") && !text.contains("line 7"), "six lines, then the fold:\n{text}");

        // a block whose last attempt failed elsewhere shows no case, even with cases kept from earlier attempts
        let home = failed_cases("elsewhere", &[(2, &[(history, git, 128, "fatal")])]);
        r.phase = Phase::Blocked { step: Some("change".into()), fault: Fault::Agent, reason: "change spent its 3 attempts: the agent exited with code 1".into() };
        let b = blocked(&home, &r).expect("blocked");
        let _gone = std::fs::remove_dir_all(&home);
        assert!(b.cases.is_empty(), "{:?}", b.cases);
        assert_eq!(b.why, "“change” used its 3 attempts. The agent exited with code 1.");
    }
}

/// Halted by Jaynes's own rule: the process driving it was stopped while it ran, by nobody's halt.
pub fn stopped_by_jaynes(r: &Run) -> bool {
    matches!(&r.halted_by, Some(jaynes_core::question::By::Rule { rule }) if rule == crate::drive::JAYNES_STOPPED)
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
        Some(By::Rule { rule }) if rule == crate::drive::JAYNES_STOPPED => crate::drive::JAYNES_STOPPED_SAID,
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
            model: Default::default(),
            turns: None, delivered: true,
        };
        let manifest = Manifest { role: "r".into(), version: "1".into(), protocol: 1, steps: vec![step("plan"), step("change")], target: Default::default(), delivery: Default::default() };
        let start = Input::Start { run: id.into(), request: request.into(), manifest, role_fingerprint: jaynes_core::Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        let out = jaynes_core::decide(None, &start).expect("start");
        Run::fold(&out.events).expect("run")
    }

    /// T6 (23/11): the run kept the ceiling it started with when a rule raised its risk; the person's raise takes it to
    /// the ceiling of that risk, and says so.
    #[test]
    fn asked_raising_the_ceiling_of_a_run_whose_risk_rose_takes_it_to_that_risk_s() {
        let mut r = run("r", "x");
        let (to, words) = ceiling_raise(&r);
        assert_eq!((to.seconds, to.tokens, words.as_str()), (r.ceiling.seconds + 1800, r.ceiling.tokens + 2_000_000, "Raise the ceiling by 30 min"));
        r.risk = jaynes_core::risk::Risk::High;
        let (to, words) = ceiling_raise(&r);
        assert_eq!(to, r.ceilings.high);
        assert_eq!(words, "Raise the ceiling to 120 min, as a high-risk run's");
    }

    fn entry(run: Run, project: &str, at: u64) -> Entry {
        Entry { run, project: project.into(), title: None, started: 0, at, merged: false, into: String::new(), gone: None, moved: None, stopped: false, waiting: None }
    }

    // The engineer lead and the tech-lead, 22/11 (row 28): a run whose engine left the Mac never waits for the person
    // nor counts with what does; it waits apart, "From an older Jaynes", until the person moves it to Finished, which
    // this engine keeps in a dated list of its own, outside the run's folder, and never in the run's record. A run
    // this engine carries on is refused, and nothing is written.
    #[test]
    fn asked_a_run_whose_engine_left_waits_apart_and_the_person_moves_it_to_finished_22_11() {
        let home = std::env::temp_dir().join(format!("jaynes-listing-older-{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        std::fs::create_dir_all(&home).expect("home");
        let asks = |r: &mut Run| {
            let question = Question { rides: Vec::new(),
                id: "q1".into(), step: "plan".into(), round: 1, kind: QuestionKind::Decision, items: vec![], offered: vec![], holds: true,
                about: "Which plan goes on?".into(), choose: false, pass: 0, passes: 0, back: vec![],
            };
            r.questions.insert("q1".into(), QuestionState { question, answer: None, effects: None, superseded: false });
            r.phase = Phase::Waiting { step: Some("plan".into()), on: vec!["q1".into()], why: "waits".into() };
        };
        // a made-up engine that never was on this Mac, and a run of this one, both at a stop
        let mut old = run("old", "Fix the export");
        asks(&mut old);
        old.engine = "0.0.1+nowhere".into();
        let mut mine = run("mine", "Make the onboarding friendlier");
        asks(&mut mine);
        let listed = |home: &Path| {
            let at = |e: Entry, n: u64| Entry { project: "/p/app".into(), at: n, ..e };
            arrange(&[at(entry_of(home, old.clone()), 2), at(entry_of(home, mine.clone()), 1)])
        };
        let ids = |rows: &[Row]| rows.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
        let l = listed(&home);
        assert_eq!(ids(&l.needs), ["mine"], "only the run that can take an answer waits for the person");
        assert_eq!(ids(&l.older), ["old"], "the other waits apart");
        assert!(l.projects.iter().all(|(_, rows)| rows.iter().all(|r| r.id != "old")), "and once only");
        // its version whole in the column, never cut: the group's head says the rest (the design-lead, 22/11)
        assert_eq!(said(&l.older[0]), "can't go on · Jaynes 0.0.1");
        // its steps: where it stopped, in grey, and what came after never ran; never "waiting for you"
        let told = |r: &Run| cannot_go_on(stages(r)).into_iter().map(|s| (s.name, s.class, s.detail)).collect::<Vec<_>>();
        assert_eq!(told(&old), [("plan".to_string(), "gone", "stopped here · can't go on".to_string()), ("change".to_string(), "todo", "never ran".to_string())]);
        // between two steps, the first that had not run is where it stopped; one left out at its risk says so still
        let stage = |name: &str, class: &'static str, detail: &str| Stage { index: 0, name: name.into(), about: String::new(), class, detail: detail.into() };
        let between = cannot_go_on(vec![stage("plan", "checked", "checked, 1 attempt"), stage("lint", "todo", "left out at this risk"), stage("change", "todo", ""), stage("review", "todo", "")]);
        assert_eq!(between.iter().map(|s| (s.class, s.detail.as_str())).collect::<Vec<_>>(), [("checked", "checked, 1 attempt"), ("todo", "left out at this risk"), ("gone", STOPPED_HERE), ("todo", NEVER_RAN)]);
        assert!(can_still_answer(&[entry_of(&home, old.clone())]).is_empty(), "no late answer reaches it either");
        // a run of this engine does not move, and neither do the others moved with it
        let err = crate::moved::mark(&home, &[&old, &mine], None, 1_795_000_000).expect_err("refused");
        assert!(err.contains("mine") && err.contains("goes on with this Jaynes"), "{err}");
        assert!(!home.join(crate::moved::FILE).exists(), "nothing written");
        // moved: in Finished, with its own line, never apart again
        assert_eq!(crate::moved::mark(&home, &[&old], Some("5ig"), 1_795_000_000), Ok(1));
        let l = listed(&home);
        assert!(l.older.is_empty() && ids(&l.needs) == ["mine"]);
        let row = l.projects.iter().flat_map(|(_, rows)| rows).find(|r| r.id == "old").expect("listed");
        assert_eq!(row.mark, Mark::Moved);
        assert_eq!(said(row), "moved by you · started on Jaynes 0.0.1, no longer on this Mac");
        // moved again, it keeps its first date; the list is the engine's, outside every run's folder
        assert_eq!(crate::moved::mark(&home, &[&old], None, 1_795_100_000), Ok(0));
        let kept = crate::moved::read(&home).expect("read");
        assert_eq!(kept.iter().map(|m| (m.run.as_str(), m.engine.as_str(), m.at, m.signature.as_deref())).collect::<Vec<_>>(), [("old", "0.0.1+nowhere", 1_795_000_000, Some("5ig"))]);
        assert!(crate::moved::moved_on(1_795_000_000).starts_with("You moved it to Finished on ") && crate::moved::moved_on(1_795_000_000).ends_with('.'));
        assert!(!home.join("runs").exists(), "no run's folder touched");
        // where its work is, from the engine: nothing made yet says nothing; made, its copy; delivered, its branch;
        // merged, the branch it went into
        let e = Entry { project: "/p/tidepool".into(), ..entry_of(&home, old.clone()) };
        let copy = home.join("runs/old/project");
        assert_eq!(place(&e, "jaynes/old", &copy), None, "nothing made yet");
        std::fs::create_dir_all(&copy).expect("copy");
        let mut made = e.clone();
        made.run.steps[0].attempt = 1;
        let at = place(&made, "jaynes/old", &copy).expect("made something").parts();
        assert_eq!(format!("{}{}{}", at.0, at.1, at.2), format!("What it made so far is in its copy: {}", copy.display()));
        made.run.delivered = Some(jaynes_core::Digest("d".into()));
        let at = place(&made, "jaynes/old", &copy).expect("delivered").parts();
        assert_eq!((at.0.as_str(), at.1.as_str(), at.2.as_str()), ("What it delivered is on the branch “", "jaynes/old", "” of tidepool."));
        let merged = Entry { merged: true, into: "main".into(), ..made };
        let at = place(&merged, "jaynes/old", &copy).expect("merged").parts();
        assert_eq!(format!("{}{}{}", at.0, at.1, at.2), "What it delivered is on “main” of tidepool.");
        std::fs::remove_dir_all(&home).ok();
    }

    // The tech-lead, 22/11: one moment, the merge. A stop the request answers does not wait; the run's story says what
    // it holds and that the run went on, then that the person's touch at the merge kept it.
    #[test]
    fn asked_one_moment_at_the_merge_the_story_says_what_a_stop_that_did_not_wait_holds() {
        use jaynes_core::question::{Answer, By, Item, Ride, Verdict};
        let mut r = run("one-moment", "the CSV breaks on a comma");
        r.manifest.steps[0].stop = Some(jaynes_core::manifest::StopDecl { settled: Some("the bug".into()), holds_unless: None, vouched_by: None, verdicts: vec![], holds: false, from_risk: Default::default(), release: false, budget: None, audit_rate: jaynes_core::manifest::Rate(0.1), about: String::new(), choose: false, redo: None, rounds: None, answers: Default::default(), note: String::new() });
        let bug = Question { rides: Vec::new(),
            id: "q-bug".into(), step: "plan".into(), round: 1, kind: QuestionKind::Decision,
            items: vec![Item { named: None, id: "bug".into(), label: "A note with a comma breaks the CSV".into(), digest: jaynes_core::Digest("b".into()) }],
            offered: vec![], holds: false, about: "Is this the bug?".into(), choose: false, pass: 0, passes: 0, back: vec![],
        };
        r.questions.insert(bug.id.clone(), QuestionState { question: bug.clone(), answer: None, effects: None, superseded: false });
        r.phase = Phase::Waiting { step: Some("change".into()), on: vec![], why: String::new() };
        let line = |r: &Run| settled_line(r, &r.questions["q-bug"]);
        assert_eq!(line(&r).as_deref(), Some("◇ At “plan”: the bug, as your request says · the run went on"));
        // the touch at the merge kept it
        let keep = Answer { question: bug.id.clone(), shown: bug.shown(), verdicts: [("bug".to_string(), Verdict::Keep)].into(), by: By::Person, context: String::new(), back: Default::default() };
        r.questions.get_mut("q-bug").expect("asked").answer = Some(keep.clone());
        let merge = Question { rides: vec![Ride { question: bug.id.clone(), step: "plan".into(), said: "the bug, as your request says".into() }],
            id: "q-merge".into(), step: "change".into(), round: 0, kind: QuestionKind::Merge, items: vec![], offered: vec![], holds: true,
            about: String::new(), choose: false, pass: 0, passes: 0, back: vec![],
        };
        let merged = Answer { question: merge.id.clone(), shown: merge.shown(), ..keep.clone() };
        r.questions.insert(merge.id.clone(), QuestionState { question: merge, answer: Some(merged), effects: None, superseded: false });
        assert_eq!(line(&r).as_deref(), Some("◇ At “plan”: the bug, as your request says · kept when you merged"));
        // answered at the stop itself: the person's own answer, told by the stop's line as before
        r.questions.get_mut("q-merge").expect("asked").question.rides.clear();
        assert_eq!(line(&r), None);
        // an agent started the run: whose request it was
        r.questions.get_mut("q-bug").expect("asked").answer = None;
        r.starter = Some("4242@1".into());
        assert_eq!(line(&r).as_deref(), Some("◇ At “plan”: the bug, as the request the agent that started the run wrote says · the run went on"));
        // a stop that holds is the person's to answer: never a line of these
        r.questions.get_mut("q-bug").expect("asked").question.holds = true;
        assert_eq!(line(&r), None);
    }

    // Alan, 22/11, at a stop where he asked the agent: "ele regerou tudo?". He wanted to see his question stay: the run's
    // story in the app says what he asked and what the agent answered, with nothing redone, in the page's words and order.
    #[test]
    fn asked_the_runs_story_keeps_what_the_person_asked_and_what_the_agent_answered() {
        use jaynes_core::run::{Entry as Said, EntryKind};
        let home = std::env::temp_dir().join(format!("jaynes-story-talk-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        let plan = home.join("runs/r1/steps/plan");
        std::fs::create_dir_all(&plan).expect("dir");
        let did = serde_json::json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Two directions for the stack."}]}}).to_string();
        std::fs::write(plan.join("turn-1-0.jsonl"), did + "\n").expect("write");
        let said = |kind: EntryKind, text: &str, late: bool| Said { round: 1, question: "q1".into(), kind, text: text.into(), item: None, late, ask: Some("q1-a1".into()) };
        let mut r = run("r1", "a stack");
        r.steps[0].thread = vec![
            said(EntryKind::Ask, "ele regerou tudo?", false),
            said(EntryKind::Answer, "Não, nada foi refeito: só respondi.", false),
            said(EntryKind::Ask, "e se fosse Go só na API?", false),
            said(EntryKind::AskFailed, "the agent's turn ended without a reply", false),
            said(EntryKind::Answer, "Daria, mas o ganho é pequeno.", true),
        ];
        // a step whose agent has not worked yet tells nothing, though words were said at it
        r.steps[1].thread = vec![said(EntryKind::Ask, "and here?", false)];
        assert_eq!(talk(&r.steps[0]), [
            "you asked: ele regerou tudo?",
            "agent answered, nothing redone: Não, nada foi refeito: só respondi.",
            "you asked: e se fosse Go só na API?",
            "agent could not answer: the agent's turn ended without a reply",
            "agent answered after the stop closed, nothing changed: Daria, mas o ganho é pequeno.",
        ]);
        let told = story_of(&home, "r1", Some(&r));
        assert_eq!(told.len(), 1, "{told:?}");
        assert_eq!(told[0].0, "plan");
        assert_eq!(told[0].1[0], "Two directions for the stack.", "what the agent did comes first");
        assert_eq!(told[0].1[1..], talk(&r.steps[0])[..], "then the conversation at its stop, in order");
        // the story the app reads by the run's id reads its state from the record; no record, the transcripts alone
        assert_eq!(story(&home, "r1"), vec![("plan".to_string(), vec!["Two directions for the stack.".to_string()])]);
        let _gone = std::fs::remove_dir_all(&home);
    }

    fn of_engine(mut r: Run, engine: &str) -> Run {
        r.engine = engine.into();
        r
    }

    // The tech-lead's ask for 0.2.5 (the engineer lead, 22/11): before it installs, `jaynes update` says how many
    // open runs an engine that will not carry them on started, and of which version, in the design-lead's words.
    #[test]
    fn asked_the_update_says_which_open_runs_an_older_jaynes_started_before_it_installs() {
        let mut waits = of_engine(run("r1", "Choose the stack"), "0.2.4+aaa");
        let question = Question {
            id: "q1".into(), step: "plan".into(), round: 1, kind: QuestionKind::Decision, items: vec![], offered: vec![], holds: false,
            about: "Which stack?".into(), choose: false, pass: 0, passes: 0, back: vec![],
        };
        waits.questions.insert("q1".into(), QuestionState { question, answer: None, effects: None, superseded: false });
        let working = of_engine(run("r2", "Login with passkey"), "0.2.4+aaa");
        let mut blocked = of_engine(run("r3", "Fix the export"), "0.2.4+aaa");
        blocked.phase = Phase::Blocked { step: Some("change".into()), fault: jaynes_core::event::Fault::Agent, reason: "no".into() };
        let mut done = of_engine(run("r4", "Old and finished"), "0.2.4+aaa");
        done.phase = Phase::Finished { outcome: Outcome::Abandoned };
        let current = of_engine(run("r5", "On the new one"), "0.2.5+bbb");
        let entries = [entry(waits, "/p/tidepool", 50), entry(working, "/p/harbor-api", 40), entry(blocked, "/p/tidepool", 30), entry(done, "/p/tidepool", 20), entry(current, "/p/tidepool", 10)];
        // what the update leaves behind, as stop::left_behind says it for the version it installs: an open run of
        // another version whose engine's program is not kept (0.2.3's is, here)
        let left = |v: &'static str| move |r: &Run| release_of(&r.engine) != v && !r.engine.starts_with("0.2.3");
        let said = older_runs_said(&entries, left("0.2.5"));
        assert_eq!(said, [
            "3 runs started on Jaynes 0.2.4 can't go on after this update: no answer, Halt or Abandon will reach them.",
            "  tidepool · Choose the stack (waits for your answer)",
            "  harbor-api · Login with passkey (working, at “plan”)",
            "  tidepool · Fix the export (blocked)",
            "To finish them on 0.2.4, answer or abandon them before you update. Their work stays where it is either way,",
            "and the app lists them under “From an older Jaynes”.",
        ]);
        // one run: the singular; the version alone, never its fingerprint
        let said = older_runs_said(&entries[2..], left("0.2.5"));
        assert_eq!(said[0], "1 run started on Jaynes 0.2.4 can't go on after this update: no answer, Halt or Abandon will reach it.");
        assert_eq!(said[2..], ["To finish it on 0.2.4, answer or abandon it before you update. Its work stays where it is either way,", "and the app lists it under “From an older Jaynes”."]);
        // the same version again (the app put back): nothing to say; and no open run of an older one: nothing either
        assert!(older_runs_said(&entries, left("0.2.4")).iter().all(|l| !l.contains("0.2.4")), "{:?}", older_runs_said(&entries, left("0.2.4")));
        assert!(older_runs_said(&entries[3..], left("0.2.5")).is_empty(), "a finished run and one of the version installed are not said");
        // past five, "and N more"; two older versions, the newest first; a run whose engine is kept goes on, unsaid
        let many: Vec<Entry> = (0..7).map(|i| entry(of_engine(run(&format!("m{i}"), &format!("Task {i}")), "0.2.4+aaa"), "/p/tidepool", 100 + i))
            .chain([entry(of_engine(run("o1", "Older yet"), "0.2.2+ccc"), "/p/tidepool", 1), entry(of_engine(run("k1", "Kept"), "0.2.3+ddd"), "/p/tidepool", 2)]).collect();
        let said = older_runs_said(&many, left("0.2.5"));
        assert_eq!(said[0], "7 runs started on Jaynes 0.2.4 can't go on after this update: no answer, Halt or Abandon will reach them.");
        assert_eq!(said[6], "  and 2 more");
        let at = said.iter().position(|l| l == "1 run started on Jaynes 0.2.2 can't go on after this update: no answer, Halt or Abandon will reach it.").expect("0.2.2");
        assert!(at > 6 && !said.iter().any(|l| l.contains("0.2.3") || l.contains("Kept")), "{said:?}");
        // a build of the newest source, whose version is not known: every open run is said
    }

    // The tech-lead's rule, 21/11 (Alan's run r1795301086-403): a run that went on without the answer to a stop shows
    // where it is now, never the stop's invitation; the stop goes to "Can still answer", under what waits.
    #[test]
    fn asked_a_stop_the_run_passed_is_listed_under_can_still_answer_and_the_run_shows_now() {
        let mut went = run("r1", "Fix the login redirect");
        went.phase = Phase::Blocked { step: Some("change".into()), fault: jaynes_core::event::Fault::Agent, reason: "the change could not be checked".into() };
        let question = Question { rides: Vec::new(),
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

    // The real-user's round 2026-11-22-01 (scaffold, its stop at "stacks"): the stop's head said "step 1 of 2" and
    // the column "step 2 of 2". A run held at a step's stop is at that step: its step is done and waits for the
    // person, so the next one has not begun. One count everywhere: where the run is.
    #[test]
    fn asked_a_run_held_at_a_stop_is_at_the_stop_s_step() {
        let mut held = run("r1", "Build the notes service");
        held.steps[0].status = jaynes_core::run::Status::Done;
        held.steps[0].done = Some(jaynes_core::run::Done { round: 1, output: jaynes_core::Digest("o".into()), items: vec![], reads: held.reads(0), changed: vec![] });
        held.phase = Phase::Waiting { step: Some("plan".into()), on: vec!["q1".into()], why: "waits for your answer".into() };
        assert_eq!(where_in(&held), "step 1 of 2", "held at plan's stop, the run is at plan");
        // once the person answers, the run goes on to the next step
        held.phase = Phase::Running;
        assert_eq!(where_in(&held), "step 2 of 2");
    }

    // The real-user's round 2026-11-22-01, after b336961b (identity): "Na lista ao lado, a pergunta aparece como
    // "identity · step 4 of 4 · the run waits here", e a página da pergunta diz "STEP CRITIQUE · STEP 3 OF 4"". An item
    // of Waiting for you is a question: it counts the step that asks, as the stop's head does, wherever the run is.
    #[test]
    fn asked_a_question_counts_the_step_that_asks_wherever_the_run_is() {
        let mut on = run("r1", "Pick the colours");
        on.steps[0].status = jaynes_core::run::Status::Done;
        on.steps[0].done = Some(jaynes_core::run::Done { round: 1, output: jaynes_core::Digest("o".into()), items: vec![], reads: on.reads(0), changed: vec![] });
        // plan's stop is open and the run went on: it is at the second step
        on.phase = Phase::Running;
        assert_eq!(where_in(&on), "step 2 of 2");
        assert_eq!(asked_at(&on, "plan"), "step 1 of 2", "plan asks");
        // held at the second step, waiting for plan's answer: plan still asks
        on.phase = Phase::Waiting { step: Some("change".into()), on: vec!["q1".into()], why: "waits for your answer".into() };
        assert_eq!(where_in(&on), "step 2 of 2");
        assert_eq!(asked_at(&on, "plan"), "step 1 of 2");
        assert_eq!(asked_at(&on, "change"), "step 2 of 2");
    }

    // The real-user's app, 22/11 (recibo, fix-bug): the run was blocked at "fix", and "Needs you" showed it as "Is
    // this the bug?", the old question still open at "reproduce". The tech-lead's rule, for every role: a blocked run
    // says so first; its open questions stay inside, never in its line.
    #[test]
    fn asked_a_blocked_run_says_it_is_blocked_before_its_open_question() {
        let mut stuck = run("r1", "O total do recibo sai errado");
        stuck.phase = Phase::Blocked { step: Some("change".into()), fault: jaynes_core::event::Fault::Agent, reason: "change spent its 3 attempts".into() };
        let question = Question { rides: Vec::new(),
            id: "q1".into(), step: "plan".into(), round: 1, kind: QuestionKind::Decision, items: vec![], offered: vec![], holds: true,
            about: "Is this the bug?".into(), choose: false, pass: 0, passes: 0, back: vec![],
        };
        stuck.questions.insert("q1".into(), QuestionState { question, answer: None, effects: None, superseded: false });
        let e = entry(stuck, "/w/recibo", 30);
        let (mark, line, _) = stands(&e);
        assert_eq!(mark, Mark::Blocked, "blocked first");
        assert!(line.contains("blocked: change spent its 3 attempts"), "{line}");
        assert!(!line.contains("Is this the bug?"), "the open question stays inside: {line}");
        let listing = arrange(&[e]);
        assert!(listing.needs.is_empty(), "listed as blocked, not as its question");
        assert_eq!(said(&listing.projects[0].1[0]), format!("blocked · {line}"), "jaynes runs says the same");
    }

    #[test]
    fn a_run_whose_box_waits_for_a_port_another_run_holds_says_so_in_its_line_and_still_works() {
        // T10, 23/11: where the person looks (the page, the app, jaynes show), and never as a fault
        let said = "Waiting for port 5432: another run holds it while its commands run (horta · Add a ping)";
        let e = Entry { waiting: Some(said.into()), ..entry(run("r1", "Fix login"), "/w/estoque", 30) };
        let (mark, line, _) = stands(&e);
        assert_eq!(mark, Mark::Working);
        assert!(line.ends_with(", waiting for port 5432: another run holds it while its commands run (horta · Add a ping)"), "{line}");
        assert_eq!(short_name("/w/estoque", None, "Fix login. Then more."), "estoque · Fix login");
    }

    #[test]
    fn what_waits_for_the_person_comes_first_and_each_project_lists_its_runs_newest_first() {
        let mut waits = run("r1", "Make the onboarding friendlier");
        waits.phase = Phase::Waiting { step: Some("plan".into()), on: vec!["q1".into()], why: "waits for your answer".into() };
        let question = Question { rides: Vec::new(),
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
        let question = Question { rides: Vec::new(),
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
            question: Question { rides: Vec::new(),
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
        let item = |id: &str, label: &str| jaynes_core::question::Item { named: None, id: id.into(), label: label.into(), digest: jaynes_core::Digest(id.into()) };
        let asked = |items, choose| QuestionState {
            question: Question { rides: Vec::new(),
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

    /// The tech-lead and the design-lead (22/11, quatro-itens.md, item 7): a run the agent that started it set aside
    /// read "halted", in amber, among what waits for the person. It reads "set aside", why first, and waits for no
    /// one: not even an open stop puts it in "Waiting for you" (Resume and Abandon stay on its page).
    #[test]
    fn asked_a_run_set_aside_says_so_and_waits_for_no_one() {
        use jaynes_core::question::By;
        let mut r = run("aside", "x");
        let question = Question {
            id: "q1".into(), step: "plan".into(), round: 1, kind: QuestionKind::Decision, items: vec![], offered: vec![], holds: true,
            about: "Which plan goes on?".into(), choose: false, pass: 0, passes: 0, back: vec![],
        };
        r.questions.insert("q1".into(), QuestionState { question, answer: None, effects: None, superseded: false });
        r.phase = Phase::Waiting { step: Some("plan".into()), on: vec!["q1".into()], why: "waits for your answer".into() };
        r.halted = true;
        r.set_aside = Some(By::Agent { who: "claude:1@x".into() });
        let e = entry(r, "/p", 1);
        assert_eq!(stands(&e), (Mark::SetAside, "set aside by the agent that started it · step 1 of 2".to_string(), false));
        let listing = arrange(std::slice::from_ref(&e));
        assert!(listing.needs.is_empty(), "{:?}", listing.needs);
        assert_eq!(listing.projects[0].1[0].mark.word(), "set aside");
    }

    /// Item 10 of 0.2.5: `jaynes runs` said "running" of a halted run, where the app said "halted". Every state the
    /// list knows reads the same in `jaynes runs` (said), on the page (phase_word) and in the app (the row's mark).
    #[test]
    fn asked_every_state_reads_the_same_in_jaynes_runs_on_the_page_and_in_the_app() {
        use jaynes_core::question::By;
        let stop = |r: &mut Run| {
            let question = Question { rides: Vec::new(),
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
        cases.push(("set aside", entry(r, "/p", 1), "set aside by the agent that started it · step 1 of 2"));
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

    // The engineer lead, 0.2.5 (the re-measure): a run under a minute said "Spent 0 of 20 min" though it worked; the
    // seconds are said until the first minute, then the minutes, as before, on the page and in the app.
    #[test]
    fn what_a_run_spent_is_said_in_seconds_under_a_minute() {
        assert_eq!(spent(58, 1200), "Spent 58 s of 20 min");
        assert_eq!(spent(0, 1200), "Spent 0 s of 20 min");
        assert_eq!(spent(60, 1200), "Spent 1 of 20 min");
        assert_eq!(spent(199, 3000), "Spent 3 of 50 min");
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
