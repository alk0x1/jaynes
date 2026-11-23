//! The runs as the app knows them: read from the engine's record by a thread of their own, once a second while
//! anything works (what the agent does changes without the record), and at once when the record gains an entry.
//! The list beside and the steps panel both show this model, and the run the person picked in it.

use gpui::*;
use jaynes_shell::listing::{self, Listing, Mark, Row, Stage};
use jaynes_shell::stop::{self, Stop};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

/// What the steps panel shows of one run.
#[derive(Clone, Debug, PartialEq)]
pub struct Detail {
    pub id: String,
    pub title: String,
    pub role: String,
    pub project: String,
    pub mark: Mark,
    pub line: String,
    /// Minutes since it started: until now while it goes on, until it last changed once it stopped.
    pub minutes: u64,
    pub stages: Vec<Stage>,
    /// What its agent does now, while it works.
    pub now: Option<String>,
    /// Already delivered while a stop of it is still open: the person's answer can still correct it.
    pub delivered: bool,
    /// Started on an older engine no longer on this Mac: what its tab says when no card of it is open.
    pub gone: Option<stop::Gone>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub listing: Listing,
    pub details: HashMap<String, Detail>,
    /// The stop each run that needs the person waits at.
    pub stops: HashMap<String, Stop>,
    /// How many files each finished run changed, as its done steps recorded them (the column's "delivered · 3 files").
    pub files: HashMap<String, usize>,
    /// Who halted each halted run, in the column's words: "halted by you" only when it was the person.
    pub halted: HashMap<String, String>,
    /// The conversation at each waiting stop, by run: its step's thread, and whether the agent answers a question of
    /// it now (`Run.asking`).
    pub threads: HashMap<String, Thread>,
}

/// A stop's conversation, as the record keeps it on its step.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Thread {
    pub entries: Vec<jaynes_core::run::Entry>,
    pub asking: bool,
}

pub use jaynes_shell::listing::halted_by;

/// How many files a run changed: each thing its done steps on the path changed, once, as the run's summary lists
/// what was delivered (jaynes_shell::summary).
pub fn delivered_files(r: &jaynes_core::run::Run) -> usize {
    let mut seen: Vec<&str> = Vec::new();
    for i in r.path() {
        for c in r.steps[i].done.as_ref().map(|d| d.changed.as_slice()).unwrap_or_default() {
            if !seen.contains(&c.what.as_str()) {
                seen.push(&c.what);
            }
        }
    }
    seen.len()
}

pub struct Runs {
    pub snap: Snapshot,
    pub selected: Option<String>,
    /// Why the record could not be read, shown instead of the list.
    pub trouble: Option<String>,
    /// Whether the record was read once: what changes after that is news.
    pub(crate) read_once: bool,
    _watch: Task<()>,
    _clicks: Task<()>,
}

impl Runs {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (tx, rx) = async_channel::bounded::<Result<Snapshot, String>>(1);
        std::thread::Builder::new()
            .name("record".into())
            .spawn(move || watch(tx))
            .expect("cannot start the thread that reads the record");
        let watch = cx.spawn(async move |this, cx| {
            while let Ok(read) = rx.recv().await {
                let gone = this
                    .update(cx, |this, cx| {
                        match read {
                            Ok(snap) => {
                                // what changed since the last reading is news; the first reading is what stood
                                if this.read_once {
                                    crate::notify::post(&crate::news::between(&this.snap, &snap), crate::notify::looking());
                                }
                                this.read_once = true;
                                crate::menubar::show(snap.listing.needs.len());
                                this.snap = snap;
                                this.trouble = None;
                                let known = this.selected.as_ref().is_some_and(|id| this.snap.details.contains_key(id));
                                if !known {
                                    this.selected = first(&this.snap.listing).map(|r| r.id.clone());
                                }
                            }
                            Err(e) => this.trouble = Some(e),
                        }
                        cx.notify();
                    })
                    .is_err();
                if gone {
                    break;
                }
            }
        });
        // a click on the menu bar brings the window forward on the first run that waits
        let clicks = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(300)).await;
            if !crate::menubar::clicked() {
                continue;
            }
            let gone = this
                .update(cx, |this, cx| {
                    if let Some(r) = this.snap.listing.needs.first() {
                        this.selected = Some(r.id.clone());
                    }
                    // a test that draws the window into a picture leaves the person's screen as it is
                    crate::show_main(cx);
                    cx.notify();
                })
                .is_err();
            if gone {
                break;
            }
        });
        Runs { snap: Snapshot::default(), selected: None, trouble: None, read_once: false, _watch: watch, _clicks: clicks }
    }

    pub fn select(&mut self, id: String, cx: &mut Context<Self>) {
        self.selected = Some(id);
        cx.notify();
    }

    pub fn chosen(&self) -> Option<&Detail> {
        self.selected.as_ref().and_then(|id| self.snap.details.get(id))
    }

    /// The stop the picked run waits at.
    pub fn stop(&self) -> Option<&Stop> {
        self.selected.as_ref().and_then(|id| self.snap.stops.get(id))
    }
}

/// The run shown first: the first that waits for the person, else the newest.
fn first(l: &Listing) -> Option<&Row> {
    l.needs.first().or_else(|| l.projects.first().and_then(|(_, rows)| rows.first()))
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A row's line in plain words: the backticks around a name are for the card, which sets it in mono.
fn plain_lines(listing: &mut Listing) {
    for r in listing.needs.iter_mut().chain(listing.projects.iter_mut().flat_map(|(_, rows)| rows.iter_mut())) {
        if r.line.contains('`') {
            r.line = crate::look::ticks(&r.line).into_iter().map(|(t, _)| t).collect();
        }
    }
}

/// Every run's list row and details, read from the record and the runs' folders.
fn snapshot(home: &Path) -> Result<Snapshot, String> {
    let entries = listing::entries(home)?;
    let mut listing = listing::arrange(&entries);
    plain_lines(&mut listing);
    let rows: HashMap<&str, &Row> =
        listing.rows().map(|r| (r.id.as_str(), r)).collect();
    let mut details = HashMap::new();
    let mut stops = HashMap::new();
    let mut files = HashMap::new();
    let mut halted = HashMap::new();
    let mut threads = HashMap::new();
    let halts: Vec<&str> = listing.projects.iter().flat_map(|(_, rows)| rows).filter(|r| r.mark == Mark::Halted).map(|r| r.id.as_str()).collect();
    if !halts.is_empty() {
        let store = jaynes_shell::store::Store::open(&home.join("record.sqlite3"))?;
        for id in halts {
            // a run whose process is gone has no halt in its record yet: Jaynes stopped while it ran
            let stopped = entries.iter().any(|e| e.run.id == id && e.stopped);
            let who = if stopped { jaynes_shell::drive::JAYNES_STOPPED_SAID.to_string() } else { halted_by(&store.events(id)?) };
            halted.insert(id.to_owned(), who);
        }
    }
    for e in &entries {
        if e.run.finished() {
            files.insert(e.run.id.clone(), delivered_files(&e.run));
        }
        if let Some(s) = stop::waiting(home, &e.run) {
            let at = e.run.manifest.steps.iter().position(|st| st.name == s.step);
            let entries = at.and_then(|i| e.run.steps.get(i)).map(|st| st.thread.clone()).unwrap_or_default();
            threads.insert(e.run.id.clone(), Thread { entries, asking: e.run.asking.values().any(|q| *q == s.question) });
            stops.insert(e.run.id.clone(), s);
        }
        let Some(row) = rows.get(e.run.id.as_str()) else { continue };
        let going = !e.run.finished() && !e.run.halted;
        let until = if going { now_seconds() } else { e.at };
        let project = Path::new(&e.project).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        details.insert(
            e.run.id.clone(),
            Detail {
                id: e.run.id.clone(),
                title: row.title.clone(),
                role: e.run.manifest.role.clone(),
                project,
                mark: row.mark,
                line: row.line.clone(),
                minutes: until.saturating_sub(e.started) / 60,
                // a run whose engine left: where it stopped in grey, and what never ran, as the page says it
                stages: if matches!(row.mark, Mark::Older | Mark::Moved) { listing::cannot_go_on(listing::stages(&e.run)) } else { listing::stages(&e.run) },
                now: if row.mark == Mark::Working { listing::now(home, &e.run.id) } else { None },
                delivered: !e.run.finished() && e.run.delivered.is_some(),
                gone: matches!(row.mark, Mark::Older | Mark::Moved).then(|| stop::gone(home, &e.run)).flatten(),
            },
        );
    }
    Ok(Snapshot { listing, details, stops, files, halted, threads })
}

/// Sends the runs at once, then whenever they change; looks once a second. The record's last event says it changed,
/// and so does the engine's list of the runs the person moved to Finished (`moved::FILE`), which is no event.
fn watch(tx: async_channel::Sender<Result<Snapshot, String>>) {
    let home = jaynes_shell::home();
    let mut seen: Option<((i64, Option<std::time::SystemTime>), Snapshot)> = None;
    loop {
        let moved = std::fs::metadata(home.join(jaynes_shell::moved::FILE)).and_then(|m| m.modified()).ok();
        let seq = jaynes_shell::store::Store::open(&home.join("record.sqlite3")).and_then(|s| s.last_seq()).map(|n| (n, moved));
        let working = seen.as_ref().is_some_and(|(_, s)| s.details.values().any(|d| d.mark == Mark::Working));
        let stale = match (&seq, &seen) {
            (Ok(n), Some((m, _))) => n != m || working,
            _ => true,
        };
        if stale {
            let read = seq.clone().and_then(|_| snapshot(&home));
            let changed = match (&read, &seen) {
                (Ok(snap), Some((_, old))) => snap != old,
                _ => true,
            };
            if let (Ok(n), Ok(snap)) = (&seq, &read) {
                seen = Some((*n, snap.clone()));
            }
            if changed && tx.send_blocking(read).is_err() {
                return;
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::{delivered_files, halted_by, plain_lines};
    use jaynes_core::event::Event;
    use jaynes_core::question::By;
    use jaynes_core::manifest::Manifest;
    use jaynes_core::risk::Changed;
    use jaynes_core::run::Run;
    use jaynes_core::{Digest, Input};

    /// The column shows a leave's question as words: the backticks the engine puts around a file name are for the
    /// card, which sets the name in mono; in a one-line row they were shown as they are (Alan's leave rows, 22/11:
    /// "May the agent read `/etc/hosts`?").
    #[test]
    fn asked_column_rows_drop_the_backticks() {
        use jaynes_shell::listing::{Listing, Mark, Row};
        let row = |line: &str| Row { id: "r".into(), project: String::new(), apart: None, title: "T".into(), line: line.into(), mark: Mark::Needs };
        let mut l = Listing { older: vec![], needs: vec![row("May the agent read `/etc/hosts`?")], projects: vec![("p".into(), vec![row("May the agent change `Makefile`?")])] };
        plain_lines(&mut l);
        assert_eq!(l.needs[0].line, "May the agent read /etc/hosts?");
        assert_eq!(l.projects[0].1[0].line, "May the agent change Makefile?");
    }

    fn done(step: &str, changed: &[&str]) -> Event {
        Event::StepDone { step: step.into(), round: 0, output: Digest("o".into()), items: vec![], reads: vec![], changed: changed.iter().map(|f| Changed::file((*f).into(), 4)).collect() }
    }

    #[test]
    fn a_delivered_run_counts_each_file_its_done_steps_changed_once() {
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "role": "fix-bug", "version": "1", "protocol": 1,
            "steps": [{"name": "change", "executor": "agent"}, {"name": "tests", "executor": "agent"}, {"name": "deliver", "executor": "role"}],
        }))
        .expect("manifest");
        let start = Input::Start {
            run: "r1".into(), request: "Fix it".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(),
            raise: None, attention: Default::default(), task: None, arm: None, never_merge: false,
            starter: None,
        };
        let mut events = jaynes_core::decide(None, &start).expect("start").events;
        events.extend([done("change", &["a.rs", "b.rs"]), done("tests", &["b.rs", "tests/c.rs"]), done("deliver", &[])]);
        let run = Run::fold(&events).expect("run");
        assert_eq!(delivered_files(&run), 3);
    }

    #[test]
    fn a_halted_run_says_halted_by_you_only_when_the_person_halted_it_last() {
        let halt = |by: By| Event::Halted { by };
        assert_eq!(halted_by(&[halt(By::Person)]), "halted by you");
        assert_eq!(halted_by(&[halt(By::Person), Event::Resumed { by: By::Person }, halt(By::Agent { who: "a program without a terminal".into() })]), "halted by an agent");
        assert_eq!(halted_by(&[halt(By::Rule { rule: "brake".into() })]), "halted by a rule");
        assert_eq!(halted_by(&[]), "halted");
    }
}
