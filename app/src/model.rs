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
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub listing: Listing,
    pub details: HashMap<String, Detail>,
    /// The stop each run that needs the person waits at.
    pub stops: HashMap<String, Stop>,
}

pub struct Runs {
    pub snap: Snapshot,
    pub selected: Option<String>,
    /// Why the record could not be read, shown instead of the list.
    pub trouble: Option<String>,
    /// Whether the record was read once: what changes after that is news.
    read_once: bool,
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

/// Every run's list row and details, read from the record and the runs' folders.
fn snapshot(home: &Path) -> Result<Snapshot, String> {
    let entries = listing::entries(home)?;
    let listing = listing::arrange(&entries);
    let rows: HashMap<&str, &Row> =
        listing.needs.iter().chain(listing.projects.iter().flat_map(|(_, rows)| rows)).map(|r| (r.id.as_str(), r)).collect();
    let mut details = HashMap::new();
    let mut stops = HashMap::new();
    for e in &entries {
        if let Some(s) = stop::waiting(home, &e.run) {
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
                stages: listing::stages(&e.run),
                now: if row.mark == Mark::Working { listing::now(home, &e.run.id) } else { None },
            },
        );
    }
    Ok(Snapshot { listing, details, stops })
}

/// Sends the runs at once, then whenever they change; looks once a second.
fn watch(tx: async_channel::Sender<Result<Snapshot, String>>) {
    let home = jaynes_shell::home();
    let mut seen: Option<(i64, Snapshot)> = None;
    loop {
        let seq = jaynes_shell::store::Store::open(&home.join("record.sqlite3")).and_then(|s| s.last_seq());
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
