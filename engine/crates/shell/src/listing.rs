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
        let merged = !crate::drive::merges(&home.join("runs").join(&id)).is_empty();
        entries.push(Entry { run, project, title, started, at, merged });
    }
    Ok(entries)
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
        let waiting = if r.finished() { None } else { r.questions.values().find(|q| q.open()) };
        let (mark, line, needs) = match waiting {
            Some(q) => (Mark::Needs, self::title(q), true),
            None => {
                let (mark, line) = standing(e);
                (mark, line, false)
            }
        };
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
fn step_at(r: &Run) -> String {
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
    let title = match given.map(str::trim).filter(|g| !g.is_empty()) {
        Some(g) => g.to_string(),
        None => {
            let r = request.trim();
            let first_line = r.lines().next().unwrap_or("");
            let r = match first_line.find(": ") {
                Some(at) if first_line.to_lowercase().starts_with("agreed with") => r[at + 2..].trim_start(),
                _ => r,
            };
            let mut chars = first_sentence(r).chars();
            chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
        }
    };
    cut_at_word(&title, 40)
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

/// What a question asks, in a line. A choice with one option asks whether to go with it, by its name (its label
/// before the colon, as the app names a direction): the role's question goes on the line under it.
pub fn title(q: &QuestionState) -> String {
    if q.question.one_option() {
        let label = &q.question.items[0].label;
        let name = label.split_once(':').map_or(label.as_str(), |(n, _)| n).trim();
        format!("Go with “{name}”?")
    } else if !q.question.about.trim().is_empty() {
        q.question.about.clone()
    } else if q.question.kind == QuestionKind::Permission {
        format!("May the agent {}?", q.question.about)
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
            let tries = format!("{} attempt{}", s.attempt, if s.attempt == 1 { "" } else { "s" });
            let round = if s.round > 0 { format!(", round {}", s.round + 1) } else { String::new() };
            let answer = s.question.as_ref().and_then(|q| r.questions.get(q)).and_then(|q| q.answer.clone());
            let (class, detail) = if !path.contains(&i) {
                ("todo", "left out at this risk".to_string())
            } else if blocked == Some(d.name.as_str()) {
                // the agent spent every attempt it had: said so, as the run's tab says it
                ("blocked", if spent_all { format!("blocked: used its {tries}") } else { format!("blocked, {tries}") })
            } else {
                match &s.status {
                    Status::Running { attempt, .. } | Status::Checking { attempt, .. } if !r.finished() && r.halted => ("halted", format!("halted, attempt {attempt}{round}")),
                    Status::Probing if !r.finished() && r.halted => ("halted", "halted while testing what it reaches".to_string()),
                    Status::Running { attempt, .. } | Status::Checking { attempt, .. } if !r.finished() => ("running", format!("running, attempt {attempt}{round}")),
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

/// What a blocked run's tab says at its top: where it stopped, why in words, and what Continue does.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockedSaid {
    /// The step it stopped at, when one did.
    pub step: Option<String>,
    pub title: String,
    pub why: String,
    pub continue_does: String,
}

/// A blocked run's words, from the core's (`Phase::Blocked`): the step's name always in curly quotes, since a role's
/// "change" or "fix" reads as a verb; "change spent its 3 attempts: the agent exited with code 1: …" becomes
/// "“change” used its 3 attempts. The agent exited with code 1."; any other reason is the engine's, as a sentence.
/// Continue gives an agent's attempts back (the core's Continued sets them to 0 for Fault::Agent), else goes on.
pub fn blocked_said(step: Option<&str>, fault: jaynes_core::event::Fault, reason: &str) -> BlockedSaid {
    let quoted = |s: &str| format!("“{s}”");
    let sentence = |t: &str| {
        let t = t.trim();
        let mut c = t.chars();
        let up = c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default();
        if up.is_empty() || up.ends_with(['.', '!', '?']) { up } else { format!("{up}.") }
    };
    // a record keeps a reason as the engine said it then ("exited with Some(1)"): read as it says it now
    let reason = &in_words(reason);
    let spent = step.and_then(|s| reason.strip_prefix(&format!("{s} spent its ")).and_then(|rest| rest.split_once(" attempts")).map(|(n, failures)| (s, n, failures.trim_start_matches(':'))));
    let why = match spent {
        Some((s, n, failures)) => {
            // the first failure's own words, before its detail: "the agent exited with code 1"
            let head = failures.split("; ").next().unwrap_or("").split(": ").next().unwrap_or("").trim();
            if head.is_empty() { format!("{} used its {n} attempts.", quoted(s)) } else { format!("{} used its {n} attempts. {}", quoted(s), sentence(head)) }
        }
        None => sentence(reason),
    };
    BlockedSaid {
        step: step.map(str::to_string),
        title: step.map(|s| format!("Blocked at {}", quoted(s))).unwrap_or_else(|| "Blocked".into()),
        why,
        continue_does: match step {
            Some(s) if fault == jaynes_core::event::Fault::Agent => format!("tries {} again, with its attempts back", quoted(s)),
            Some(s) => format!("goes on from {}", quoted(s)),
            None => "goes on".into(),
        },
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

    fn run(id: &str, request: &str) -> Run {
        let step = |name: &str| StepDecl {
            name: name.into(), needs: vec![], executor: Executor::Agent, session: SessionKind::Fresh, reach: Reach::default(),
            effect: Default::default(), checks: vec![], stop: None, from_risk: Default::default(), attempts: 3, fixes: 2, hidden: false, idempotent: false, about: String::new(),
        };
        let manifest = Manifest { role: "r".into(), version: "1".into(), protocol: 1, steps: vec![step("plan"), step("change")], target: Default::default(), delivery: Default::default() };
        let start = Input::Start { run: id.into(), request: request.into(), manifest, role_fingerprint: jaynes_core::Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false };
        let out = jaynes_core::decide(None, &start).expect("start");
        Run::fold(&out.events).expect("run")
    }

    fn entry(run: Run, project: &str, at: u64) -> Entry {
        Entry { run, project: project.into(), title: None, started: 0, at, merged: false }
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
