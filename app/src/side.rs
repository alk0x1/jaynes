//! The column at the left, in the app's familiar look: the Inbox with how many things need the person, then the
//! person's spaces with their agents (read from herdr-core, as Herdr arranges them), each with its dot and a short word,
//! then the Jaynes runs. The runs' list is the engine's own (`jaynes_shell::listing`), read from the record, so it says
//! what `jaynes runs` and the page say.
//!
//! Narrower than its names (bars.rs), the column is a rail, as Herdr's: the Inbox's icon and count, the spaces
//! numbered with a dot where something waits or works, the agents of one space numbered with their mark, the runs
//! that wait or work as their dot, and a » that gives the column its names back. Each says its name on a tooltip and
//! opens as in the column.

use crate::agents::{self, Agent, Agents, State};
use crate::look;
use crate::model::Runs;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use jaynes_shell::listing::{Mark, Row};
use std::collections::HashSet;

pub struct Side {
    runs: Entity<Runs>,
    agents: Entity<Agents>,
    tabs: Entity<crate::tabs::Tabs>,
    /// The spaces and the sections the person folded (a section by its key: "§needs", "§work", "§spaces").
    folded: HashSet<String>,
    /// Whether the finished runs show, under their count.
    show_finished: bool,
    /// The finished runs the person cleared from the column (by id), and whether they show, faint, to bring back.
    cleared: HashSet<String>,
    /// The finished runs the person has seen, once the record is read; until then, what the window kept.
    seen: Option<HashSet<String>>,
    seen_kept: Option<Vec<String>>,
    show_cleared: bool,
    /// Whether the column is the rail (bars.rs): the window says so, as the person drags its edge.
    pub rail: bool,
    /// The person's key, that signs a move to Finished; whether "Move all to Finished" stands open, asking; and what
    /// came of the last move, when it did not go.
    key: Option<std::sync::Arc<crate::key::Key>>,
    moving_all: bool,
    move_said: Option<String>,
}

/// The rail's » pressed: the column takes its names back (its own width).
pub struct Widen;

impl EventEmitter<Widen> for Side {}

/// The person cleared a run from the column, or brought one back: the window keeps it (app_state.rs).
pub struct Cleared;

impl EventEmitter<Cleared> for Side {}

/// The column in the order the person chose ("Needs you first"): what waits for them, runs and agents in one list;
/// then the runs at work; then the finished runs, folded to a count; then the spaces with the agents that do not
/// wait for them. Each label says what is expected of the person, or nothing: an agent done is "done · 5m", grey,
/// and out of the Inbox's number; a blocked or halted run waits for them ("blocked · open it", "halted · open it").
#[derive(Debug, Default, PartialEq)]
struct Order {
    /// The runs whose question is open, then the blocked and the halted ones; then the agents that ask or failed, with their
    /// space's name.
    needs: Vec<Row>,
    asking: Vec<(String, Agent)>,
    /// The runs started on an older Jaynes no longer on this Mac, which cannot go on: apart, out of "Needs you" and
    /// the Inbox's number, until the person moves them to Finished.
    older: Vec<Row>,
    /// The runs not finished that wait on nothing: working.
    at_work: Vec<Row>,
    /// The finished runs (delivered, merged, undone, abandoned), by project as the record lists them, but the
    /// cleared ones, which are apart: a run that waits for the person or works is never cleared.
    finished: Vec<(String, Vec<Row>)>,
    cleared: Vec<Row>,
    /// Each space with its agents that are not in `asking`.
    spaces: Vec<(agents::Space, Vec<Agent>)>,
}

impl Order {
    fn finished_count(&self) -> usize {
        self.finished.iter().map(|(_, rows)| rows.len()).sum()
    }
}

/// The Inbox's number: what needs the person, runs and agents; never what is done.
fn inbox_count(o: &Order) -> usize {
    o.needs.len() + o.asking.len()
}

/// Whether an agent waits for the person: it asks, or it failed.
fn asks(a: &Agent) -> bool {
    matches!(a.state, State::Waiting | State::Error)
}

fn finished(m: Mark) -> bool {
    matches!(m, Mark::Delivered | Mark::Merged | Mark::Undone | Mark::Abandoned | Mark::Moved)
}

/// The line under a run from an older Jaynes, grey, in its group ("can't go on · Jaynes 0.2.3") or in
/// Finished once the person moved it ("moved by you · started on Jaynes 0.2.3, no longer on this Mac"); the others
/// keep theirs.
fn older_line(row: &Row) -> String {
    match row.mark {
        Mark::Older | Mark::Moved => format!("{} · {}", look::word(row.mark), row.line),
        _ => row.line.clone(),
    }
}

/// The finished runs the person has seen: those the window kept that are still in the record; the first time, with
/// none kept, every run finished then, so the runs of before do not fill the Inbox's "Finished".
fn seen_from(kept: Option<Vec<String>>, listing: &jaynes_shell::listing::Listing) -> HashSet<String> {
    let rows = || listing.projects.iter().flat_map(|(_, rows)| rows);
    match kept {
        None => rows().filter(|r| finished(r.mark)).map(|r| r.id.clone()).collect(),
        Some(kept) => kept.into_iter().filter(|id| rows().any(|r| &r.id == id)).collect(),
    }
}

fn order(listing: &jaynes_shell::listing::Listing, spaces: &[agents::Space], cleared: &HashSet<String>) -> Order {
    let mut o = Order { needs: listing.needs.clone(), older: listing.older.clone(), ..Order::default() };
    for (project, rows) in &listing.projects {
        let (done, open): (Vec<&Row>, Vec<&Row>) = rows.iter().partition(|r| finished(r.mark));
        // blocked or halted: the person decides what comes next
        let (stopped, open): (Vec<&Row>, Vec<&Row>) = open.into_iter().partition(|r| matches!(r.mark, Mark::Blocked | Mark::Halted));
        o.needs.extend(stopped.into_iter().cloned());
        o.at_work.extend(open.into_iter().cloned());
        let (gone, done): (Vec<&Row>, Vec<&Row>) = done.into_iter().partition(|r| cleared.contains(&r.id));
        o.cleared.extend(gone.into_iter().cloned());
        if !done.is_empty() {
            o.finished.push((project.clone(), done.into_iter().cloned().collect()));
        }
    }
    for space in spaces {
        let (asking, rest): (Vec<&Agent>, Vec<&Agent>) = space.agents.iter().partition(|a| asks(a));
        o.asking.extend(asking.into_iter().map(|a| (space.name.clone(), a.clone())));
        o.spaces.push((space.clone(), rest.into_iter().cloned().collect()));
    }
    o
}

/// The cleared runs as kept: those still finished in the record, once it was read (before, or when it cannot be
/// read, every one, so none is lost to a record not read yet).
fn kept_cleared(cleared: &HashSet<String>, listing: Option<&jaynes_shell::listing::Listing>) -> Vec<String> {
    let mut kept: Vec<String> = match listing {
        Some(l) => cleared.iter().filter(|id| l.projects.iter().flat_map(|(_, rows)| rows).any(|r| &r.id == *id && finished(r.mark))).cloned().collect(),
        None => cleared.iter().cloned().collect(),
    };
    kept.sort();
    kept
}

/// What a run at work says at the right of its name: where it is ("step 2 of 4").
fn at_work_tag(row: &Row) -> String {
    row.line.split(", ").next().unwrap_or_default().to_owned()
}

/// What a run in "Needs you" says at the right of its name: a blocked or halted one, what the person can do, as an agent
/// that failed says it; one that asks, nothing (its question is under it).
fn needs_tag(row: &Row) -> Option<(String, u32)> {
    match row.mark {
        Mark::Blocked => Some(("blocked · open it".into(), look::red())),
        Mark::Halted => Some(("halted · open it".into(), look::amber())),
        _ => None,
    }
}

/// The line under a run in "Needs you": a halted one, who halted it (`halted`, from the record: "halted by you" only
/// when the person did) and where it is; one already delivered, that the person can still correct it; the others,
/// their own line (the question, or why it is blocked).
pub(crate) fn needs_line(row: &Row, delivered: bool, halted: Option<&String>) -> String {
    match row.mark {
        Mark::Halted => format!("{} · {}", halted.map(String::as_str).unwrap_or("halted"), at_work_tag(row)),
        _ if delivered => "delivered · you can still correct it".into(),
        Mark::Blocked => blocked_line(&row.line),
        _ => row.line.clone(),
    }
}

/// A blocked run's line: one whose step spent its attempts says where, as its tab does ("at “change”: used its 3
/// attempts"); another cause keeps its own line.
fn blocked_line(line: &str) -> String {
    let reason = line.split_once("blocked: ").map(|(_, r)| r).unwrap_or(line);
    let spent = reason.split_once(" spent its ").and_then(|(step, rest)| rest.split_once(" attempts").map(|(n, _)| (step, n)));
    match spent {
        Some((step, n)) if !step.contains(' ') && n.parse::<u32>().is_ok() => format!("at “{step}”: used its {n} attempts"),
        _ => line.to_string(),
    }
}

/// What a finished run says at the right of its name: a delivered or merged one, how many files it changed, as the
/// record holds them (`files`, counted for finished runs only); the others say nothing there, their line says it.
fn finished_tag(row: &Row, files: Option<usize>) -> Option<String> {
    let word = match row.mark {
        Mark::Delivered => "delivered",
        Mark::Merged => "merged",
        _ => return None,
    };
    files.map(|n| format!("{word} · {n} {}", if n == 1 { "file" } else { "files" }))
}

/// What an agent says beside its name, and its colour: what is expected of the person when it waits or failed, how
/// it stands otherwise; one done says since when, neutral.
fn agent_word(a: &Agent, now: u64) -> (String, u32) {
    match a.state {
        State::Waiting => ("asks you · open it".into(), look::amber()),
        State::Error => ("failed · open it".into(), look::red()),
        State::ToReview => (format!("done · {}", agents::age(a.since_ms, now)), look::faint()),
        s => (s.word().into(), colour(s)),
    }
}

/// The line under an agent in "Needs you": what it asks, in the Inbox's words (ask::what), a whole sentence even
/// without its words, so two that wait never read the same; one that failed, what it said, if anything.
fn asking_line(a: &Agent) -> Option<String> {
    match a.state {
        State::Waiting => Some(crate::ask::what(a.kind.as_deref(), &a.msg)),
        // failed: what it said, or that it stopped
        State::Error => Some(if a.msg.trim().is_empty() { "it stopped with an error".to_string() } else { a.msg.trim().to_string() }),
        _ => (!a.msg.trim().is_empty()).then(|| a.msg.trim().to_string()),
    }
}

/// A run's name in the column, as listing::name gives it, in the three parts the row draws: its project with the
/// " · " after it (never cut), its title (cut when there is no room), and what tells it apart, with the " · " before
/// it. Without a project, the title alone.
pub fn name_parts(row: &Row) -> (String, String, Option<String>) {
    let project = if row.project.is_empty() { String::new() } else { format!("{} · ", row.project) };
    (project, row.title.clone(), row.apart.as_ref().map(|a| format!(" · {a}")))
}

/// What a run's dot on the narrow rail says on hovering: its name and what it waits for, in the column's words.
fn rail_tip(row: &Row, halted: Option<&String>) -> String {
    let what = match row.mark {
        Mark::Needs => "asks you".to_string(),
        Mark::Blocked => "blocked · open it".to_string(),
        Mark::Working => format!("working, {}", row.line.split(", ").next().unwrap_or_default()),
        Mark::Halted => halted.cloned().unwrap_or_else(|| "halted".into()),
        _ => row.line.clone(),
    };
    let (project, title, apart) = name_parts(row);
    format!("{project}{title}{} · {what}", apart.unwrap_or_default())
}

/// A thing on the rail: centred, its name on a tooltip.
fn rail_item(id: SharedString, on: bool, tip: String) -> Stateful<Div> {
    div()
        .id(id)
        .mx(px(6.))
        .h(px(26.))
        .flex_none()
        .rounded(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(4.))
        .text_size(px(12.))
        .text_color(rgb(if on { look::text() } else { look::dim() }))
        .when(on, |d| d.bg(rgb(look::selected())))
        .hover(|d| d.bg(rgb(look::selected())).text_color(rgb(look::text())))
        .cursor_pointer()
        .tooltip(look::tip(tip))
}

fn rail_rule() -> Div {
    div().mx(px(14.)).my(px(6.)).h(px(1.)).flex_none().bg(rgb(look::line()))
}

/// An agent's state as the column colours it.
fn colour(state: State) -> u32 {
    match state {
        State::Waiting => look::amber(),
        State::Error => look::red(),
        State::Working => look::blue(),
        State::ToReview => look::grey(),
        State::Idle => look::faint(),
    }
}

fn dot(color: u32) -> Div {
    div().size(px(8.)).rounded_full().flex_none().bg(rgb(color))
}

impl Side {
    pub fn new(runs: Entity<Runs>, agents: Entity<Agents>, tabs: Entity<crate::tabs::Tabs>, cleared: Vec<String>, seen: Option<Vec<String>>, key: Option<std::sync::Arc<crate::key::Key>>, cx: &mut Context<Self>) -> Self {
        cx.observe(&runs, |this: &mut Self, runs, cx| {
            // the line that asks to move them all goes with the group when fewer than two are left
            if runs.read(cx).snap.listing.older.len() < 2 {
                this.moving_all = false;
            }
            // what was seen is known once the record is read: kept, or every run finished then
            if this.seen.is_none() && runs.read(cx).read_once && runs.read(cx).trouble.is_none() {
                this.seen = Some(seen_from(this.seen_kept.take(), &runs.read(cx).snap.listing));
                cx.emit(Cleared);
            }
            cx.notify();
        })
        .detach();
        cx.observe(&agents, |_, _, cx| cx.notify()).detach();
        cx.observe(&tabs, |_, _, cx| cx.notify()).detach();
        Side { runs, agents, tabs, folded: HashSet::new(), show_finished: false, cleared: cleared.into_iter().collect(), seen: None, seen_kept: seen, show_cleared: false, rail: false, key, moving_all: false, move_said: None }
    }

    /// Whether "Move all to Finished" stands open, asking: Esc cancels it (main.rs).
    pub fn moving_all(&self) -> bool {
        self.moving_all
    }

    pub fn cancel_move_all(&mut self, cx: &mut Context<Self>) {
        self.moving_all = false;
        cx.notify();
    }

    fn older_ids(&self, cx: &App) -> Vec<String> {
        self.runs.read(cx).snap.listing.older.iter().map(|r| r.id.clone()).collect()
    }

    /// Every run of "From an older Jaynes" to Finished, signed with the person's key (answer::move_to_finished).
    fn move_all(&mut self, cx: &mut Context<Self>) {
        let moved = crate::answer::move_to_finished(self.key.clone(), self.older_ids(cx));
        cx.spawn(async move |this, cx| {
            let moved = moved.await;
            let _ = this.update(cx, |this, cx| {
                this.moving_all = moved.is_err() && this.moving_all;
                this.move_said = moved.err().map(|e| format!("Not moved: {e}"));
                cx.notify();
            });
        })
        .detach();
    }

    /// The group's head: its name and count, grey, as the other sections; under it, for two or more, the quiet link
    /// that asks to move them all, on a line of its own as "Clear finished", since the column has no room for both.
    fn older_head(&self, n: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let link = (n > 1 && !self.moving_all).then(|| {
            div().mx(px(8.)).px(px(10.)).py(px(3.)).flex().child(div().flex_1()).child(
                div()
                    .id("move-all")
                    .px(px(4.))
                    .rounded(px(4.))
                    .text_size(px(11.5))
                    .text_color(rgb(look::faint()))
                    .hover(|d| d.bg(rgb(look::line())).text_color(rgb(look::text())))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.moving_all = true;
                        this.move_said = None;
                        cx.notify();
                    }))
                    .child(jaynes_shell::listing::MOVE_ALL),
            )
        });
        div().flex().flex_col().child(self.section("§older", jaynes_shell::listing::OLDER_GROUP, Some(n), look::faint(), cx)).children(link)
    }

    /// The line "Move all to Finished" opens under the group's head, no window over the column: what it does, then
    /// Move N (neutral) and Cancel; Esc cancels too.
    fn move_all_line(&self, n: usize, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .mx(px(8.))
            .px(px(10.))
            .py(px(6.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(div().text_size(px(12.)).text_color(rgb(look::dim())).child(jaynes_shell::listing::move_all_asks(n)))
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(crate::island::button("move-all-yes".into(), &jaynes_shell::listing::move_all_button(n), false, look::grey()).on_click(cx.listener(|this, _, _, cx| this.move_all(cx))))
                    .child(crate::island::button("move-all-no".into(), "Cancel", false, look::grey()).on_click(cx.listener(|this, _, _, cx| this.cancel_move_all(cx)))),
            )
            .when_some(self.move_said.clone(), |d, s| d.child(div().text_size(px(12.)).text_color(rgb(look::red())).child(s)))
    }

    /// The finished runs the person has seen, once the record is read.
    pub fn seen(&self) -> Option<&HashSet<String>> {
        self.seen.as_ref()
    }

    /// The seen runs, as the window keeps them: what was kept until the record is read.
    pub fn seen_to_keep(&self) -> Option<Vec<String>> {
        self.seen.as_ref().map(|s| s.iter().cloned().collect()).or_else(|| self.seen_kept.clone())
    }

    /// A finished run the person saw, by opening its tab or with Seen in the Inbox: out of "Finished" there. A run
    /// not finished is not marked, or it would never come to the Inbox once it is.
    pub fn mark_seen(&mut self, id: &str, cx: &mut Context<Self>) {
        let done = self.runs.read(cx).snap.listing.projects.iter().flat_map(|(_, rows)| rows).any(|r| r.id == id && finished(r.mark));
        if done && let Some(seen) = &mut self.seen
            && seen.insert(id.to_owned())
        {
            cx.emit(Cleared);
            cx.notify();
        }
    }

    /// The cleared runs, as the window keeps them.
    pub fn cleared_kept(&self, cx: &App) -> Vec<String> {
        let runs = self.runs.read(cx);
        let read = runs.read_once && runs.trouble.is_none();
        kept_cleared(&self.cleared, read.then_some(&runs.snap.listing))
    }

    /// Every finished run of the record, cleared or not.
    fn finished_ids(&self, cx: &App) -> Vec<String> {
        self.runs.read(cx).snap.listing.projects.iter().flat_map(|(_, rows)| rows).filter(|r| finished(r.mark)).map(|r| r.id.clone()).collect()
    }

    fn clear(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        self.cleared.extend(ids);
        cx.emit(Cleared);
        cx.notify();
    }

    fn bring_back(&mut self, id: &str, cx: &mut Context<Self>) {
        self.cleared.remove(id);
        cx.emit(Cleared);
        cx.notify();
    }

    /// At the right of a finished run: an x that clears it from the column, or, on a cleared one, ↺ to bring it back.
    fn row_control(&self, id: &str, back: bool, cx: &mut Context<Self>) -> AnyElement {
        let id = id.to_owned();
        div()
            .id(SharedString::from(format!("{}-{id}", if back { "back" } else { "clear" })))
            .flex_none()
            .px(px(4.))
            .rounded(px(4.))
            .text_size(px(11.))
            .text_color(rgb(look::faint()))
            .hover(|d| d.bg(rgb(look::line())).text_color(rgb(look::text())))
            .cursor_pointer()
            .tooltip(look::tip(if back { "Bring back to the column" } else { "Clear from the column: the run stays in the record" }))
            .on_click(cx.listener(move |this, _, _, cx| {
                // the press is the control's, not the row's, which would open the run
                cx.stop_propagation();
                if back { this.bring_back(&id, cx) } else { this.clear(vec![id.clone()], cx) }
            }))
            .child(if back { "↺".into_any_element() } else { look::icon(look::Icon::Close, 10., look::faint()).into_any_element() })
            .into_any_element()
    }

    fn fold(&mut self, key: &str, cx: &mut Context<Self>) {
        if key == "§finished" {
            self.show_finished = !self.show_finished;
        } else if !self.folded.remove(key) {
            self.folded.insert(key.to_owned());
        }
        cx.notify();
    }

    fn inbox(&self, need: usize, on: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.tabs.clone();
        div()
            .id("side-inbox")
            .mx(px(8.))
            .mt(px(8.))
            .px(px(10.))
            .py(px(7.))
            .rounded(px(7.))
            .flex()
            .items_center()
            .gap(px(10.))
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(if on { look::text() } else { look::dim() }))
            .when(on, |d| d.bg(rgb(look::selected())))
            .hover(|d| d.bg(rgb(look::selected())).text_color(rgb(look::text())))
            .cursor_pointer()
            .on_click(cx.listener(move |_, _, _, cx| tabs.update(cx, |t, cx| t.show_inbox(cx))))
            .child(look::icon(look::Icon::Inbox, 14., look::dim()))
            .child("Inbox")
            .child(div().flex_1())
            .when(need > 0, |d| {
                d.child(
                    div()
                        .px(px(7.))
                        .rounded(px(9.))
                        .bg(rgb(look::amber()))
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(look::on_fill()))
                        .child(need.to_string()),
                )
            })
    }

    /// A section's name that folds it, with its count.
    fn section(&self, key: &'static str, name: &str, count: Option<usize>, color: u32, cx: &mut Context<Self>) -> impl IntoElement {
        // "Finished" is folded until the person shows it
        let folded = if key == "§finished" { !self.show_finished } else { self.folded.contains(key) };
        div()
            .id(key)
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(18.))
            .pt(px(16.))
            .pb(px(4.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.fold(key, cx)))
            .child(div().w(px(10.)).text_size(px(10.)).text_color(rgb(look::faint())).child(if folded { "▸" } else { "▾" }))
            .child(look::label(name, color))
            .child(div().flex_1())
            .children(count.map(|n| div().text_size(px(11.)).text_color(rgb(look::faint())).child(n.to_string())))
    }

    fn space_head(&self, id: &str, name: &str, folded: bool, waits: bool, count: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let key = id.to_owned();
        div()
            .id(SharedString::from(format!("space-{id}")))
            .mx(px(8.))
            .mt(px(6.))
            .px(px(10.))
            .py(px(5.))
            .rounded(px(6.))
            .flex()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(look::dim()))
            .hover(|d| d.text_color(rgb(look::text())))
            .on_click(cx.listener(move |this, _, _, cx| this.fold(&key, cx)))
            .child(div().w(px(10.)).text_color(rgb(look::faint())).child(if folded { "▸" } else { "▾" }))
            .child(div().flex_1().min_w_0().truncate().child(name.to_owned()))
            // a folded space still says that something in it waits, and how many agents it holds
            .when(folded && waits, |d| d.child(dot(look::amber())))
            .when(folded && count > 0, |d| d.child(div().text_size(px(11.)).text_color(rgb(look::faint())).child(count.to_string())))
    }

    fn agent(&self, a: &Agent, on: bool, now: u64, cx: &mut Context<Self>) -> impl IntoElement {
        let icon = agent_icon(a, "agent");
        let right = if a.shell || a.state == State::Idle {
            div().text_color(rgb(look::faint())).child(agents::age(a.since_ms, now))
        } else {
            let (word, c) = agent_word(a, now);
            div().text_color(rgb(c)).child(word)
        };
        let (tabs, address, title) = (self.tabs.clone(), a.address.clone(), a.title.clone());
        div()
            .id(SharedString::from(format!("agent-{}", a.address)))
            .mx(px(8.))
            .pl(px(26.))
            .pr(px(10.))
            .py(px(6.))
            .rounded(px(7.))
            .flex()
            .items_center()
            .gap(px(9.))
            .when(on, |d| d.bg(rgb(look::selected())))
            .hover(|d| d.bg(rgb(look::selected())))
            .cursor_pointer()
            .on_click(cx.listener(move |_, _, _, cx| tabs.update(cx, |t, cx| t.show_agent(address.clone(), title.clone(), cx))))
            .child(div().w(px(14.)).flex_none().flex().items_center().justify_center().child(icon))
            .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).text_color(rgb(look::text())).child(a.title.clone()))
            .child(right.flex_none().text_size(px(11.5)))
    }

    /// An agent that waits for the person, in "Needs you": its space and name, what it asks under them.
    fn asking(&self, space: &str, a: &Agent, on: bool, now: u64, cx: &mut Context<Self>) -> impl IntoElement {
        let (word, c) = agent_word(a, now);
        let (tabs, address, title) = (self.tabs.clone(), a.address.clone(), a.title.clone());
        div()
            .id(SharedString::from(format!("asking-{}", a.address)))
            .mx(px(8.))
            .px(px(10.))
            .py(px(7.))
            .rounded(px(7.))
            .flex()
            .gap(px(10.))
            .items_start()
            .when(on, |d| d.bg(rgb(look::selected())))
            .hover(|d| d.bg(rgb(look::selected())))
            .cursor_pointer()
            .on_click(cx.listener(move |_, _, _, cx| tabs.update(cx, |t, cx| t.show_agent(address.clone(), title.clone(), cx))))
            .child(div().mt(px(3.)).size(px(12.)).flex_none().flex().items_center().justify_center().child(agent_icon(a, "asking")))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).text_color(rgb(look::text())).child(format!("{space} · {}", a.title)))
                            .child(div().flex_none().text_size(px(11.5)).text_color(rgb(c)).child(word)),
                    )
                    .when_some(asking_line(a), |d, line| d.child(div().truncate().text_size(px(11.5)).text_color(rgb(look::amber())).child(line))),
            )
    }

    /// A run: its mark and its request, with a word at the right (`tag`) and its line under it (`line`).
    fn row(&self, row: &Row, on: bool, tag: Option<(String, u32)>, line: bool, end: Option<AnyElement>, cx: &mut Context<Self>) -> impl IntoElement {
        let id = row.id.clone();
        let tabs = self.tabs.clone();
        div()
            .id(SharedString::from(format!("run-{}", row.id)))
            .mx(px(8.))
            .px(px(10.))
            .py(px(7.))
            .rounded(px(7.))
            .flex()
            .gap(px(10.))
            .items_start()
            .when(on, |d| d.bg(rgb(look::selected())))
            .hover(|d| d.bg(rgb(look::selected())))
            .cursor_pointer()
            .on_click(cx.listener(move |_, _, _, cx| tabs.update(cx, |tabs, cx| tabs.show(Some(id.clone()), cx))))
            .child(div().mt(px(3.)).size(px(12.)).flex_none().flex().items_center().justify_center().child(run_icon(row, "spin")))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child({
                                // the project and what tells the run apart stay whole; the title gives way
                                let (project, title, apart) = name_parts(row);
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .text_size(px(13.))
                                    .when(!project.is_empty(), |d| d.child(div().flex_none().whitespace_nowrap().text_color(rgb(look::dim())).child(project)))
                                    .child(div().min_w_0().truncate().text_color(rgb(look::text())).child(title))
                                    .children(apart.map(|a| div().flex_none().whitespace_nowrap().text_color(rgb(look::dim())).child(a)))
                            })
                            .children(tag.map(|(t, c)| div().flex_none().text_size(px(11.5)).text_color(rgb(c)).child(t))),
                    )
                    .when(line, |d| d.child(div().truncate().text_size(px(11.5)).text_color(rgb(if row.mark == Mark::Needs { look::amber() } else { look::dim() })).child(row.line.clone()))),
            )
            .children(end)
    }

    /// In the shown "Finished": Clear finished, which takes every finished run off the column.
    fn clear_finished_line(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div().mx(px(8.)).px(px(10.)).py(px(3.)).flex().child(div().flex_1()).child(
            div()
                .id("clear-finished")
                .px(px(4.))
                .rounded(px(4.))
                .text_size(px(11.5))
                .text_color(rgb(look::faint()))
                .hover(|d| d.bg(rgb(look::line())).text_color(rgb(look::text())))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    let ids = this.finished_ids(cx);
                    this.clear(ids, cx);
                }))
                .child("Clear finished"),
        )
    }

    /// The cleared runs' count, that shows them faint, each with ↺ to bring it back.
    fn cleared_line(&self, n: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = self.show_cleared;
        div()
            .id("cleared")
            .mx(px(8.))
            .px(px(10.))
            .py(px(5.))
            .flex()
            .gap(px(6.))
            .cursor_pointer()
            .text_size(px(11.5))
            .text_color(rgb(look::faint()))
            .hover(|d| d.text_color(rgb(look::dim())))
            .on_click(cx.listener(|this, _, _, cx| {
                this.show_cleared = !this.show_cleared;
                cx.notify();
            }))
            .child(div().w(px(10.)).child(if shown { "▾" } else { "▸" }))
            .child(format!("{n} cleared · {}", if shown { "Hide" } else { "Show" }))
    }
}

#[cfg(feature = "test-driver")]
impl Side {
    /// What a test does in the column, as the clicks do: `finished` (the "N finished" line), `cleared` (the "M cleared"
    /// line), `clear <run>` (a finished run's x), `clear-finished`, `back <run>` (a cleared run's ↺).
    pub fn driven(&mut self, what: &str, cx: &mut Context<Self>) {
        let (verb, rest) = what.split_once(' ').unwrap_or((what, ""));
        match verb {
            "finished" => {
                self.show_finished = !self.show_finished;
                tracing::info!("column: finished {}: {:?}", if self.show_finished { "shown" } else { "hidden" }, self.finished_ids(cx));
                cx.notify();
            }
            "cleared" => {
                self.show_cleared = !self.show_cleared;
                cx.notify();
            }
            "clear" => self.clear(vec![rest.to_owned()], cx),
            "clear-finished" => {
                let ids = self.finished_ids(cx);
                self.clear(ids, cx);
            }
            "back" => self.bring_back(rest, cx),
            "move-all" => {
                self.moving_all = true;
                cx.notify();
            }
            "move-all-yes" => self.move_all(cx),
            "move-all-no" => self.cancel_move_all(cx),
            _ => return tracing::warn!("column: no verb {verb}"),
        }
        tracing::info!("column: {what}: {} cleared", self.cleared.len());
    }
}

/// A run's mark: a spinner while it works, else a dot in its mark's colour. `at` keeps spinners apart.
fn run_icon(row: &Row, at: &str) -> AnyElement {
    if row.mark == Mark::Working {
        look::spinner(SharedString::from(format!("{at}-{}", row.id)), 11., look::blue()).into_any_element()
    } else {
        dot(look::colour(row.mark)).into_any_element()
    }
}

/// An agent's mark, in the column and in its tab: a prompt for a shell, a spinner while it works, a ring when idle,
/// else a dot in its state's colour. `at` keeps the spinners of the column and the tabs apart.
pub fn agent_icon(a: &Agent, at: &str) -> AnyElement {
    if a.shell {
        div().flex_none().whitespace_nowrap().font_family(look::MONO_FONT).text_size(px(10.)).text_color(rgb(look::faint())).child("›_").into_any_element()
        } else if a.state == State::Working {
            look::spinner(SharedString::from(format!("{at}-spin-{}", a.address)), 11., look::blue()).into_any_element()
        } else if a.state == State::Idle {
            div().size(px(8.)).rounded_full().flex_none().border(px(1.5)).border_color(rgb(look::idle_ring())).into_any_element()
        } else {
            dot(colour(a.state)).into_any_element()
        }
}

/// A section's name that does not fold.
fn heading(name: &str, color: u32) -> impl IntoElement {
    div().px(px(18.)).pt(px(16.)).pb(px(4.)).child(look::label(name, color))
}

impl Side {
    /// The column as a rail, in the column's order: the Inbox; what needs the person; the runs at work; the spaces
    /// numbered, the agents of an open one under it; and the » that gives the names back.
    fn rail(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let runs = self.runs.read(cx);
        let (listing, selected, halted) = (runs.snap.listing.clone(), runs.selected.clone(), runs.snap.halted.clone());
        let agents = self.agents.read(cx);
        let o = order(&listing, &agents.spaces, &self.cleared);
        let tabs = self.tabs.read(cx);
        let (open, inbox_on, open_run) = (tabs.agent.as_ref().map(|(a, _)| a.clone()), tabs.inbox, tabs.middle().run().map(str::to_string));
        let mut list = div().id("rail-list").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().pt(px(8.)).pb(px(8.)).gap(px(2.));

        let need = inbox_count(&o);
        let now = agents::now_ms();
        let tabs = self.tabs.clone();
        list = list.child(
            rail_item("rail-inbox".into(), inbox_on, if need > 0 { format!("Inbox · {need} need you") } else { "Inbox".into() })
                .relative()
                .on_click(move |_, _, cx| tabs.update(cx, |t, cx| t.show_inbox(cx)))
                .child(look::icon(look::Icon::Inbox, 14., look::dim()))
                .when(need > 0, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(-3.))
                            .right(px(1.))
                            .px(px(4.))
                            .rounded(px(7.))
                            .bg(rgb(look::amber()))
                            .text_size(px(9.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(look::on_fill()))
                            .child(need.to_string()),
                    )
                }),
        );

        let run = |row: &Row| {
            let (tabs, id) = (self.tabs.clone(), row.id.clone());
            // the open run's dot has a ring, so four amber dots tell which one shows
            let ring = open_run.as_deref() == Some(row.id.as_str());
            rail_item(SharedString::from(format!("rail-run-{}", row.id)), selected.as_deref() == Some(row.id.as_str()), rail_tip(row, halted.get(&row.id)))
                .on_click(move |_, _, cx| tabs.update(cx, |t, cx| t.show(Some(id.clone()), cx)))
                .child(div().p(px(2.)).rounded_full().border(px(1.5)).border_color(if ring { rgb(look::text()) } else { rgba(0) }).child(run_icon(row, "rail-spin")))
        };
        let agent = |a: &Agent, tip: String| {
            let (tabs, address, title) = (self.tabs.clone(), a.address.clone(), a.title.clone());
            rail_item(SharedString::from(format!("rail-agent-{}", a.address)), open.as_deref() == Some(a.address.as_str()), tip)
                .on_click(move |_, _, cx| tabs.update(cx, |t, cx| t.show_agent(address.clone(), title.clone(), cx)))
                .child(agent_icon(a, "rail"))
        };

        if !o.needs.is_empty() || !o.asking.is_empty() {
            list = list.child(rail_rule());
            for row in &o.needs {
                list = list.child(run(row));
            }
            for (space, a) in &o.asking {
                list = list.child(agent(a, format!("{space} · {} · {}", a.title, agent_word(a, now).0)));
            }
        }
        if !o.at_work.is_empty() {
            list = list.child(rail_rule());
            for row in &o.at_work {
                list = list.child(run(row));
            }
        }
        if !o.spaces.is_empty() {
            list = list.child(rail_rule());
        }
        for (i, (space, rest)) in o.spaces.iter().enumerate() {
            let folded = self.folded.contains(&space.id);
            let key = space.id.clone();
            list = list.child(
                rail_item(SharedString::from(format!("rail-space-{}", space.id)), false, space.name.clone())
                    .on_click(cx.listener(move |this, _, _, cx| this.fold(&key, cx)))
                    .child(
                        div()
                            .size(px(20.))
                            .rounded(px(5.))
                            .border_1()
                            .border_color(rgb(look::line()))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(11.))
                            .child((i + 1).to_string()),
                    ),
            );
            if !folded {
                for a in rest {
                    list = list.child(agent(a, format!("{} · {}", a.title, if a.shell || a.state == State::Idle { a.state.word().to_owned() } else { agent_word(a, now).0 })));
                }
            }
        }

        let widen = rail_item("rail-widen".into(), false, "Show names".into()).on_click(cx.listener(|_, _, _, cx| cx.emit(Widen))).child("»");
        div().size_full().flex().flex_col().min_h_0().font_family(look::UI_FONT).child(list).child(div().flex_none().pb(px(8.)).child(widen)).into_any_element()
    }
}

impl Render for Side {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.rail {
            return self.rail(cx);
        }
        let runs = self.runs.read(cx);
        let (listing, selected, trouble) = (runs.snap.listing.clone(), runs.selected.clone(), runs.trouble.clone());
        let agents = self.agents.read(cx);
        let core_trouble = agents.trouble.clone();
        let o = order(&listing, &agents.spaces, &self.cleared);
        let now = agents::now_ms();
        let (files, halted) = (runs.snap.files.clone(), runs.snap.halted.clone());
        let delivered: HashSet<String> = runs.snap.details.values().filter(|d| d.delivered).map(|d| d.id.clone()).collect();
        let open = self.tabs.read(cx).agent.as_ref().map(|(a, _)| a.clone());
        let inbox_on = self.tabs.read(cx).inbox;
        let mut list = div().id("side-list").flex().flex_col().flex_1().overflow_y_scroll().pb(px(12.));
        list = list.child(self.inbox(inbox_count(&o), inbox_on, cx));

        // what waits for the person, runs and agents in one list
        let needs = o.needs.len() + o.asking.len();
        if needs > 0 {
            list = list.child(self.section("§needs", "Needs you", Some(needs), look::amber(), cx));
            if !self.folded.contains("§needs") {
                for row in &o.needs {
                    let shown = Row { line: needs_line(row, delivered.contains(&row.id), halted.get(&row.id)), ..row.clone() };
                    list = list.child(self.row(&shown, selected.as_deref() == Some(row.id.as_str()), needs_tag(row), true, None, cx));
                }
                for (space, a) in &o.asking {
                    list = list.child(self.asking(space, a, open.as_deref() == Some(a.address.as_str()), now, cx));
                }
            }
        }

        // the runs from an older Jaynes, which cannot go on: apart and grey, open until the person folds it, gone when
        // none is left
        if !o.older.is_empty() {
            list = list.child(self.older_head(o.older.len(), cx));
            if self.moving_all {
                list = list.child(self.move_all_line(o.older.len(), cx));
            }
            if !self.folded.contains("§older") {
                for row in &o.older {
                    let shown = Row { line: older_line(row), ..row.clone() };
                    list = list.child(self.row(&shown, selected.as_deref() == Some(row.id.as_str()), None, true, None, cx));
                }
            }
        }

        // the runs: at work, then the finished folded to their count
        if let Some(trouble) = trouble {
            list = list.child(heading("Runs", look::faint()));
            list = list.child(div().px(px(18.)).py(px(6.)).text_size(px(12.)).text_color(rgb(look::red())).child(format!("The record cannot be read: {trouble}")));
        } else if listing.needs.is_empty() && listing.projects.is_empty() && listing.older.is_empty() {
            list = list.child(heading("Runs", look::faint()));
            list = list.child(div().px(px(18.)).py(px(6.)).text_size(px(12.)).text_color(rgb(look::faint())).child("No runs yet."));
        } else {
            if !o.at_work.is_empty() {
                list = list.child(self.section("§work", "Runs at work", Some(o.at_work.len()), look::faint(), cx));
            }
            if !self.folded.contains("§work") {
                for row in &o.at_work {
                    let tag = (at_work_tag(row), if row.mark == Mark::Working { look::blue() } else { look::faint() });
                    list = list.child(self.row(row, selected.as_deref() == Some(row.id.as_str()), Some(tag), false, None, cx));
                }
            }
            // the finished runs: a section of their own, folded by default, the cleared ones in it
            if o.finished_count() > 0 || !o.cleared.is_empty() {
                list = list.child(self.section("§finished", "Finished", Some(o.finished_count()), look::faint(), cx));
            }
            if self.show_finished {
                if o.finished_count() > 0 {
                    list = list.child(self.clear_finished_line(cx));
                    {
                        for (project, rows) in &o.finished {
                            list = list.child(heading(project, look::faint()));
                            for row in rows {
                                let x = self.row_control(&row.id, false, cx);
                                let tag = finished_tag(row, files.get(&row.id).copied()).map(|t| (t, look::faint()));
                                // a tag says what the line under would: one of the two
                                let line = tag.is_none();
                                let row = &Row { line: older_line(row), ..row.clone() };
                                list = list.child(self.row(row, selected.as_deref() == Some(row.id.as_str()), tag, line, Some(x), cx));
                            }
                        }
                    }
                }
                // cleared from the column, still in the record: shown faint on asking, each to bring back
                if !o.cleared.is_empty() {
                    list = list.child(self.cleared_line(o.cleared.len(), cx));
                    if self.show_cleared {
                        for row in &o.cleared {
                            let back = self.row_control(&row.id, true, cx);
                            list = list.child(div().opacity(0.55).child(self.row(row, selected.as_deref() == Some(row.id.as_str()), None, true, Some(back), cx)));
                        }
                    }
                }
            }
        }

        // the spaces, with the agents that do not wait for the person
        list = list.child(self.section("§spaces", "Spaces", None, look::faint(), cx));
        if !self.folded.contains("§spaces") {
            if let Some(trouble) = core_trouble {
                list = list.child(div().px(px(18.)).py(px(6.)).text_size(px(12.)).text_color(rgb(look::red())).child(format!("herdr-core cannot be read: {trouble}")));
            } else if o.spaces.is_empty() {
                list = list.child(div().px(px(18.)).py(px(6.)).text_size(px(12.)).text_color(rgb(look::faint())).child("No spaces yet."));
            }
            for (space, rest) in &o.spaces {
                let folded = self.folded.contains(&space.id);
                let waits = space.agents.iter().any(asks);
                list = list.child(self.space_head(&space.id, &space.name, folded, waits, space.agents.len(), cx));
                if !folded {
                    for a in rest {
                        list = list.child(self.agent(a, open.as_deref() == Some(a.address.as_str()), now, cx));
                    }
                }
            }
        }
        // the column's width, its header and its line at the right are the window's (main.rs)
        div().size_full().flex().flex_col().min_h_0().font_family(look::UI_FONT).child(list).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // named, not `super::*`: that brings gpui's own `test` attribute in, over the standard one
    use super::{Agent, Mark, Order, Row, State, agent_word, older_line, agents, asking_line, at_work_tag, finished_tag, inbox_count, kept_cleared, name_parts, needs_line, needs_tag, order, rail_tip, seen_from};

    /// The tech-lead and the design-lead (22/11, quatro-itens.md, item 8): two runs of the same title read alike in
    /// the column. A run is named there as the page names it (listing::name): its project first, always, then its
    /// title, then what tells it apart when its project has another of the same title ("at “fix”", "started 11:49").
    /// The project and the " · " before the title are never cut; the title is.
    #[test]
    fn asked_the_column_names_a_run_by_its_project_and_what_tells_it_apart() {
        let row = |project: &str, apart: Option<&str>| Row { id: "r".into(), project: project.into(), apart: apart.map(str::to_string), title: "Monta o começo".into(), line: String::new(), mark: Mark::Working };
        assert_eq!(name_parts(&row("horta", None)), ("horta · ".to_string(), "Monta o começo".to_string(), None));
        assert_eq!(name_parts(&row("horta", Some("at “fix”"))), ("horta · ".to_string(), "Monta o começo".to_string(), Some(" · at “fix”".to_string())));
        // a run with no project: its title alone
        assert_eq!(name_parts(&row("", None)), (String::new(), "Monta o começo".to_string(), None));
        // the narrow rail's tip says the whole name too
        let mut asks = row("horta", Some("started 11:49"));
        asks.mark = Mark::Needs;
        assert_eq!(rail_tip(&asks, None), "horta · Monta o começo · started 11:49 · asks you");
    }

    #[test]
    fn an_agent_that_waits_says_in_the_column_what_it_asks_in_the_inbox_s_words() {
        let agent = |state: State, kind: Option<&str>, msg: &str| Agent {
            address: "t1".into(), title: "claude".into(), state, since_ms: 0, shell: false, turn: 0, kind: kind.map(str::to_string), msg: msg.into(), cwd: String::new(),
        };
        let waiting = |kind: Option<&str>, msg: &str| agent(State::Waiting, kind, msg);
        assert_eq!(asking_line(&waiting(Some("permission"), "approve Bash: npm run build")).as_deref(), Some("asks you to approve Bash: npm run build"));
        // without its words, a whole sentence still: never two agents that read the same, "waiting"
        assert_eq!(asking_line(&waiting(Some("permission"), "")).as_deref(), Some("wants your permission"));
        assert_eq!(asking_line(&waiting(Some("question"), "  ")).as_deref(), Some("asks you something"));
        assert_eq!(asking_line(&waiting(Some("auth"), "")).as_deref(), Some("needs you to sign in"));
        assert_eq!(asking_line(&waiting(None, "")).as_deref(), Some("waits for you"));
        // one that failed keeps what it said, or nothing
        let failed = |msg: &str| agent(State::Error, None, msg);
        assert_eq!(asking_line(&failed("exited 1")).as_deref(), Some("exited 1"));
        assert_eq!(asking_line(&failed("")).as_deref(), Some("it stopped with an error"));
    }

    #[test]
    fn a_dot_on_the_narrow_rail_says_its_run_and_what_it_waits_for() {
        let row = |mark: Mark, line: &str| Row { id: "r".into(), project: String::new(), apart: None, title: "Tighten the site's copy".into(), line: line.into(), mark };
        assert_eq!(rail_tip(&row(Mark::Needs, "Which of these copy changes go in?"), None), "Tighten the site's copy · asks you");
        assert_eq!(rail_tip(&row(Mark::Blocked, "step 2 of 2, blocked: no room"), None), "Tighten the site's copy · blocked · open it");
        assert_eq!(rail_tip(&row(Mark::Working, "step 1 of 2, running"), None), "Tighten the site's copy · working, step 1 of 2");
        assert_eq!(rail_tip(&row(Mark::Halted, "step 1 of 2, halted"), Some(&"halted by you".to_string())), "Tighten the site's copy · halted by you");
        assert_eq!(rail_tip(&row(Mark::Delivered, "delivered"), None), "Tighten the site's copy · delivered");
    }
    use std::collections::HashSet;
    use jaynes_shell::listing::Listing;

    #[test]
    fn what_was_seen_starts_from_every_run_finished_when_the_app_first_reads_the_record() {
        let row = |id: &str, mark: Mark| Row { id: id.into(), project: String::new(), apart: None, title: id.into(), line: String::new(), mark };
        let listing = Listing { older: vec![], needs: vec![], projects: vec![("p".into(), vec![row("d1", Mark::Delivered), row("w1", Mark::Working), row("a1", Mark::Abandoned)])] };
        // first time: everything finished is taken as seen, so no old run floods the Inbox
        let first = seen_from(None, &listing);
        assert_eq!(first, HashSet::from(["d1".to_string(), "a1".to_string()]));
        // after: what was kept, without runs gone from the record
        let kept = seen_from(Some(vec!["d1".into(), "gone".into()]), &listing);
        assert_eq!(kept, HashSet::from(["d1".to_string()]));
    }

    #[test]
    fn a_run_delivered_while_its_stop_is_open_says_in_the_column_it_can_still_be_corrected() {
        let row = Row { id: "r".into(), project: String::new(), apart: None, title: "Tighten the site's copy".into(), line: "Which of these copy changes go in?".into(), mark: Mark::Needs };
        assert_eq!(needs_line(&row, false, None), "Which of these copy changes go in?");
        assert_eq!(needs_line(&row, true, None), "delivered · you can still correct it");
    }

    fn agent(address: &str, state: State) -> Agent {
        Agent { address: address.into(), title: address.into(), state, since_ms: 0, shell: false, turn: 0, kind: None, msg: String::new(), cwd: String::new() }
    }

    fn row(id: &str, mark: Mark, line: &str) -> Row {
        Row { id: id.into(), project: String::new(), apart: None, title: id.into(), line: line.into(), mark }
    }

    // Alan, 18/11, picking sketch 1 of the column (the board's words): "Needs you first"
    #[test]
    fn asked_needs_you_first_runs_and_agents_together_leaving_their_space() {
        let listing = Listing { older: vec![], needs: vec![row("q", Mark::Needs, "Keep this fix?")], projects: vec![] };
        let spaces = vec![agents::Space {
            id: "w1".into(),
            name: "jaynes".into(),
            folder: String::new(),
            agents: vec![agent("w1:t1", State::Working), agent("w1:t2", State::Waiting), agent("w1:t3", State::Error), agent("w1:t4", State::ToReview)],
            tabs: Vec::new(),
        }];
        let o = order(&listing, &spaces, &HashSet::new());
        assert_eq!(o.needs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["q"]);
        assert_eq!(o.asking.iter().map(|(s, a)| format!("{s} {}", a.address)).collect::<Vec<_>>(), ["jaynes w1:t2", "jaynes w1:t3"]);
        // to review is not waiting: it stays in its space, with the one at work
        assert_eq!(o.spaces[0].1.iter().map(|a| a.address.as_str()).collect::<Vec<_>>(), ["w1:t1", "w1:t4"]);
    }

    #[test]
    fn the_runs_at_work_come_before_the_finished_ones_which_keep_their_projects() {
        let listing = Listing { older: vec![],
            needs: vec![],
            projects: vec![
                ("my-app".into(), vec![row("a", Mark::Working, "step 2 of 4, running"), row("b", Mark::Merged, "merged"), row("c", Mark::Halted, "step 1 of 3, halted")]),
                ("api".into(), vec![row("d", Mark::Abandoned, "abandoned"), row("e", Mark::Blocked, "step 3 of 3, blocked: no room")]),
                ("notes".into(), vec![row("f", Mark::Undone, "reverted by you"), row("g", Mark::Delivered, "delivered")]),
            ],
        };
        let o = order(&listing, &[], &HashSet::new());
        // a halted run is not at work: it waits for the person, in Needs you
        assert_eq!(o.at_work.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["a"]);
        assert_eq!(o.needs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["c", "e"]);
        let finished: Vec<(String, Vec<String>)> = o.finished.iter().map(|(p, rows)| (p.clone(), rows.iter().map(|r| r.id.clone()).collect())).collect();
        assert_eq!(finished, [("my-app".to_string(), vec!["b".to_string()]), ("api".into(), vec!["d".into()]), ("notes".into(), vec!["f".into(), "g".into()])]);
        assert_eq!(o.finished_count(), 4);
        assert_eq!(at_work_tag(&o.at_work[0]), "step 2 of 4");
        // halted by anyone: the person decides what comes next, and the line under says who halted it and where
        assert_eq!(needs_tag(&o.needs[0]).map(|t| t.0).as_deref(), Some("halted · open it"));
        assert_eq!(needs_line(&o.needs[0], false, Some(&"halted by an agent".to_string())), "halted by an agent · step 1 of 3");
        assert_eq!(needs_line(&o.needs[0], false, None), "halted · step 1 of 3");
        assert_eq!(needs_line(&o.needs[1], false, None), "step 3 of 3, blocked: no room");
        // one that spent its attempts says where, as its tab does
        let spent = Row { line: "step 2 of 2, blocked: change spent its 3 attempts: the agent exited with code 1".into(), ..o.needs[1].clone() };
        assert_eq!(needs_line(&spent, false, None), "at “change”: used its 3 attempts");
    }

    #[test]
    fn a_blocked_run_needs_the_person_after_the_open_questions_and_says_what_they_can_do() {
        let listing = Listing { older: vec![],
            needs: vec![row("q", Mark::Needs, "Keep this fix?")],
            projects: vec![("api".into(), vec![row("e", Mark::Blocked, "step 3 of 3, blocked: no room"), row("a", Mark::Working, "step 1 of 2, running")])],
        };
        let o = order(&listing, &[], &HashSet::new());
        assert_eq!(o.needs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["q", "e"]);
        assert_eq!(o.at_work.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["a"]);
        // a run that asks needs no tag: its question is under it; a blocked one says what to do, as "failed · open it"
        assert_eq!(needs_tag(&o.needs[0]), None);
        assert_eq!(needs_tag(&o.needs[1]).map(|t| t.0), Some("blocked · open it".to_string()));
        // the Inbox counts what needs the person, the blocked run too, and never what is done
        let spaces = vec![agents::Space {
            id: "w1".into(),
            name: "jaynes".into(),
            folder: String::new(),
            agents: vec![agent("w1:t1", State::Waiting), agent("w1:t2", State::ToReview), agent("w1:t3", State::Error)],
            tabs: Vec::new(),
        }];
        assert_eq!(inbox_count(&order(&listing, &spaces, &HashSet::new())), 4);
    }

    // Alan, 18/11 (the rule the tech-lead wrote from what he found at the demo, as the board keeps it): "cada etiqueta
    // da coluna diz o que se espera da pessoa, ou nada"
    #[test]
    fn asked_each_column_label_says_what_is_expected_of_me_or_nothing() {
        // a run stopped says what the person can do; one that asks says nothing, its question is under it
        assert_eq!(needs_tag(&row("e", Mark::Blocked, "step 3 of 3, blocked: no room")).map(|t| t.0).as_deref(), Some("blocked · open it"));
        assert_eq!(needs_tag(&row("q", Mark::Needs, "Keep this fix?")), None);
        // a finished one says what it left, or nothing
        assert_eq!(finished_tag(&row("d", Mark::Abandoned, "abandoned"), Some(0)), None);
        let now = 10 * 60_000;
        let mut done = agent("t", State::ToReview);
        done.since_ms = 5 * 60_000;
        assert_eq!(agent_word(&done, now).0, "done · 5m");
        assert_eq!(agent_word(&agent("t", State::Error), now).0, "failed · open it");
        assert_eq!(agent_word(&agent("t", State::Waiting), now).0, "asks you · open it");
        assert_eq!(agent_word(&agent("t", State::Working), now).0, "working");
    }

    #[test]
    fn a_delivered_run_says_how_many_files_and_the_other_finished_ones_say_nothing_at_the_right() {
        assert_eq!(finished_tag(&row("g", Mark::Delivered, "delivered"), Some(3)).as_deref(), Some("delivered · 3 files"));
        assert_eq!(finished_tag(&row("g", Mark::Delivered, "delivered"), Some(1)).as_deref(), Some("delivered · 1 file"));
        assert_eq!(finished_tag(&row("b", Mark::Merged, "merged"), Some(2)).as_deref(), Some("merged · 2 files"));
        // not counted (a run still answering): its line says it
        assert_eq!(finished_tag(&row("g", Mark::Delivered, "delivered, answers open"), None), None);
        assert_eq!(finished_tag(&row("d", Mark::Abandoned, "abandoned"), Some(0)), None);
        assert_eq!(finished_tag(&row("f", Mark::Undone, "reverted by you"), Some(2)), None);
    }

    // Alan, 18/11, after testing the hardened runtime (the board's words for what he asked): "limpar da coluna execuções
    // terminadas, abandonadas ou desfeitas (x na linha, "Clear finished", filtro "Show finished"; somem da coluna, não
    // do registro; o que espera a pessoa nunca some)"
    #[test]
    fn asked_clear_finished_runs_from_the_column_never_one_that_waits_or_works() {
        let listing = Listing { older: vec![],
            needs: vec![row("q", Mark::Needs, "Keep this fix?")],
            projects: vec![("my-app".into(), vec![row("a", Mark::Working, "step 2 of 4, running"), row("b", Mark::Merged, "merged"), row("c", Mark::Abandoned, "abandoned"), row("d", Mark::Undone, "reverted by you")])],
        };
        // q and a are asked to be cleared too, as a stale id would be: they stay where they are
        let cleared: HashSet<String> = ["q", "a", "b", "d"].map(String::from).into();
        let o = order(&listing, &[], &cleared);
        assert_eq!(o.needs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["q"]);
        assert_eq!(o.at_work.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["a"]);
        assert_eq!(o.finished.iter().flat_map(|(_, rows)| rows).map(|r| r.id.as_str()).collect::<Vec<_>>(), ["c"]);
        assert_eq!(o.cleared.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["b", "d"]);
    }

    #[test]
    fn the_cleared_runs_are_kept_while_finished_in_the_record_and_all_kept_before_it_is_read() {
        let listing = Listing { older: vec![], needs: vec![row("q", Mark::Needs, "?")], projects: vec![("p".into(), vec![row("b", Mark::Merged, "merged"), row("a", Mark::Working, "step 1 of 2, running")])] };
        let cleared: HashSet<String> = ["b", "a", "q", "gone"].map(String::from).into();
        assert_eq!(kept_cleared(&cleared, Some(&listing)), ["b"]);
        // the record not read yet, or not readable: nothing is dropped
        assert_eq!(kept_cleared(&cleared, None), ["a", "b", "gone", "q"]);
    }

    #[test]
    fn asked_runs_from_an_older_jaynes_wait_apart_in_grey_until_i_move_them_to_finished_22_11() {
        let listing = Listing {
            older: vec![row("o1", Mark::Older, "Jaynes 0.2.3"), row("o2", Mark::Older, "Jaynes 0.2.3")],
            needs: vec![row("q", Mark::Needs, "Keep this fix?")],
            projects: vec![("p".into(), vec![row("m", Mark::Moved, "started on Jaynes 0.2.3, no longer on this Mac"), row("d", Mark::Delivered, "delivered")])],
        };
        let o = order(&listing, &[], &Default::default());
        // apart: not in "Needs you", nor in the Inbox's number
        assert_eq!(o.older.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["o1", "o2"]);
        assert_eq!(o.needs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["q"]);
        assert_eq!(inbox_count(&o), 1);
        // grey, never amber, in the design-lead's words
        assert_eq!(older_line(&listing.older[0]), "can't go on · Jaynes 0.2.3");
        assert_eq!(crate::look::colour(Mark::Older), crate::look::grey());
        assert_ne!(crate::look::colour(Mark::Older), crate::look::amber());
        // moved: in Finished with its project, the neutral grey dot, the line that says who moved it
        assert_eq!(o.finished, vec![("p".to_string(), vec![listing.projects[0].1[0].clone(), listing.projects[0].1[1].clone()])]);
        assert_eq!(older_line(&listing.projects[0].1[0]), "moved by you · started on Jaynes 0.2.3, no longer on this Mac");
        assert_eq!(crate::look::colour(Mark::Moved), crate::look::grey());
        assert_eq!(older_line(&listing.projects[0].1[1]), "delivered");
        // the window keeps the marks by their word
        for m in [Mark::Older, Mark::Moved] {
            assert_eq!(crate::app_state::mark_of(crate::app_state::mark_word(m)), Some(m));
        }
        // the group's words: Move all only for two or more, and the line it opens
        assert_eq!(jaynes_shell::listing::move_all_asks(2), "Move these 2 runs to Finished for good? What they delivered stays as it is.");
        assert_eq!(jaynes_shell::listing::move_all_button(2), "Move 2");
    }

    #[test]
    fn nothing_waits_nothing_works_the_column_is_its_spaces() {
        let o = order(&Listing::default(), &[], &HashSet::new());
        assert_eq!(o, Order::default());
    }
}
