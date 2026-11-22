//! The tabs over the middle of the window: the person's own Claude Code first, then a tab for each run at work or
//! waiting, as sketch D draws them. A run's tab shows its story (`jaynes_shell::listing::story`): each step its agent
//! worked, with what it did there, moving as the agent speaks. It reads, never types: a run's agent answers to
//! Jaynes, and the person answers it at its stops. The person drags the tabs into the order they like, Claude Code's
//! among them, and Command-1 to 9 follow that order.

use crate::agents::{Agent, Agents, Space};
use crate::look;
use crate::model::Runs;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use jaynes_shell::listing::{Listing, Mark, Row};
use std::collections::HashMap;
use std::time::Duration;

pub use jaynes_shell::listing::{BlockedSaid, halted_said};
use jaynes_shell::listing::{AGENT_SAID_HEADING, DETAILS_CLOSED, DETAILS_OPEN, STILL_OPEN_HEADING, WAYS_HEADING};

/// The note on a run's tab, only while its agent works or the run waits for the person: none once it is finished,
/// halted (nothing works on it until resumed) or blocked (Jaynes stopped it).
pub fn note_for(finished: bool, halted: bool, blocked: bool) -> Option<&'static str> {
    (!finished && !halted && !blocked).then_some("The agent works on its own and reports to Jaynes. You answer at its stops.")
}


/// The story in the role's order, the first step first, whatever times its files carry; a step the role does not
/// name comes last.
pub fn in_role_order(mut story: Vec<(String, Vec<String>)>, order: &[String]) -> Vec<(String, Vec<String>)> {
    story.sort_by_key(|(s, _)| order.iter().position(|o| o == s).unwrap_or(usize::MAX));
    story
}

/// The block at the top of a halted run's tab: who halted it and where, and its controls (Resume, Abandon).
pub fn halted_block(title: &str, controls: Option<Div>) -> Div {
    div()
        .mx(px(14.))
        .mt(px(12.))
        .mb(px(4.))
        .px(px(14.))
        .py(px(12.))
        .rounded(px(9.))
        .border_1()
        .border_color(rgb(look::line_2()))
        .bg(rgb(look::panel_2()))
        .font_family(look::UI_FONT)
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::text())).child(title.to_string()))
        .when_some(controls, |d, c| d.child(div().pt(px(4.)).child(c)))
}

/// A line of a blocked run's card, top to bottom as it is drawn (design 0.2.5, bloqueada.md).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    /// "Blocked at “change”".
    Title(String),
    /// What happened, said first.
    What(String),
    /// Why, in the person's words, and what the run keeps after it.
    Why(String),
    /// "The agent said", "What you can do", "Still open while the tests run:".
    Heading(&'static str),
    /// The agent's own words at the step, never a tool's line.
    Said(String),
    /// A way out, the best first, and under it what follows, its command (with Copy) and what it leaves open.
    Way(String),
    Then(String),
    Command(String),
    StillOpen(String),
    /// Continue and Abandon.
    Controls,
    /// What Continue does, under the buttons.
    ContinueDoes(String),
    /// That the run waits.
    Waits(String),
    /// The switch of the raw reason, closed or open, and its lines when open.
    Details(&'static str),
    Raw(String),
}

/// The parts of a blocked run's card, in order; the raw reason only while Details is open.
pub fn card(said: &BlockedSaid, details_open: bool) -> Vec<Part> {
    let mut parts = vec![Part::Title(said.title.clone()), Part::What(said.what.clone())];
    let why = [said.why.as_str()].into_iter().chain(said.holds.as_deref()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ");
    if !why.is_empty() {
        parts.push(Part::Why(why));
    }
    if !said.agent_said.is_empty() {
        parts.push(Part::Heading(AGENT_SAID_HEADING));
        parts.extend(said.agent_said.iter().cloned().map(Part::Said));
    }
    if !said.ways.is_empty() {
        parts.push(Part::Heading(WAYS_HEADING));
        for w in &said.ways {
            parts.push(Part::Way(w.says.clone()));
            parts.extend(w.then.clone().map(Part::Then));
            parts.extend(w.command.clone().map(Part::Command));
            if !w.still_open.is_empty() {
                parts.push(Part::Heading(STILL_OPEN_HEADING));
                parts.extend(w.still_open.iter().cloned().map(Part::StillOpen));
            }
        }
    }
    parts.push(Part::Controls);
    parts.push(Part::ContinueDoes(said.continue_does.clone()));
    parts.extend(said.waits.clone().map(Part::Waits));
    if !said.details.is_empty() {
        parts.push(Part::Details(if details_open { DETAILS_OPEN } else { DETAILS_CLOSED }));
        if details_open {
            parts.extend(said.details.iter().cloned().map(Part::Raw));
        }
    }
    parts
}

/// The widest the ways out run, so a line never runs across the whole tab.
const WAYS_WIDTH: f32 = 640.;

/// The red block at the top of a blocked run's tab, drawn from its parts: what happened first, why, what the agent
/// said, what the person can do, the run's controls and what Continue does, and the raw reason under Details, which
/// `on_details` opens and closes.
pub fn blocked_block(said: &BlockedSaid, details_open: bool, controls: Option<Div>, on_details: impl Fn(&mut Window, &mut App) + 'static) -> Div {
    let mut b = div()
        .mx(px(14.))
        .mt(px(12.))
        .mb(px(4.))
        .px(px(14.))
        .py(px(12.))
        .rounded(px(9.))
        .border_1()
        .border_color(rgb(look::red()))
        .bg(rgb(look::panel_2()))
        .font_family(look::UI_FONT)
        .flex()
        .flex_col()
        .gap(px(6.));
    let mut controls = controls;
    let mut on_details = Some(on_details);
    let line = |size: f32, color: u32, text: String| div().text_size(px(size)).text_color(rgb(color)).child(text);
    let way = |d: Div| d.max_w(px(WAYS_WIDTH));
    for (i, part) in card(said, details_open).into_iter().enumerate() {
        b = match part {
            Part::Title(t) => b.child(line(13., look::text(), t).font_weight(FontWeight::SEMIBOLD)),
            Part::What(t) => b.child(line(13., look::text(), t).font_weight(FontWeight::SEMIBOLD)),
            Part::Why(t) => b.child(line(12.5, look::dim(), t)),
            Part::Heading(h) => b.child(line(11.5, look::faint(), h.to_string()).pt(px(4.))),
            Part::Said(t) => b.child(line(12.5, look::story(), t)),
            Part::Way(t) => b.child(way(line(12.5, look::text(), t))),
            Part::Then(t) => b.child(way(line(12.5, look::dim(), t).pl(px(12.)))),
            Part::Command(c) => {
                let copy = c.clone();
                b.child(
                    way(div().pl(px(12.)).flex().gap(px(8.)).items_center())
                        .child(div().min_w_0().px(px(8.)).py(px(4.)).rounded(px(6.)).border_1().border_color(rgb(look::line_2())).bg(rgb(look::panel())).font_family(look::MONO_FONT).text_size(px(12.)).text_color(rgb(look::story())).child(c))
                        .child(crate::island::button(format!("copy-{i}"), "Copy", false, look::text()).on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone())))),
                )
            }
            Part::StillOpen(t) => b.child(way(line(12., look::faint(), format!("· {t}")).pl(px(12.)))),
            Part::Controls => b.when_some(controls.take(), |d, c| d.child(div().pt(px(6.)).child(c))),
            Part::ContinueDoes(t) => b.child(line(11.5, look::faint(), t)),
            Part::Waits(t) => b.child(line(11.5, look::faint(), t)),
            Part::Details(t) => match on_details.take() {
                Some(f) => b.child(div().id("blocked-details").cursor_pointer().text_size(px(11.5)).text_color(rgb(look::faint())).child(t).on_click(move |_, window, cx| f(window, cx))),
                None => b,
            },
            Part::Raw(t) => b.child(div().font_family(look::MONO_FONT).text_size(px(12.)).line_height(px(18.)).text_color(rgb(look::faint())).child(t)),
        };
    }
    b
}

/// What shows in the middle of the window, over the rest: what the keys follow, and the one place a run's own keys
/// (Command-period, Command-Return) reach a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Middle {
    /// Claude Code's own terminal.
    Terminal,
    /// A run's tab.
    Run(String),
    Agent,
    Inbox,
    Changes,
}

impl Middle {
    /// The run a run's keys act on: only the one whose tab shows, never one picked before.
    pub fn run(&self) -> Option<&str> {
        match self {
            Middle::Run(r) => Some(r),
            _ => None,
        }
    }
}

/// What shows in the middle: Changes over the agent it was opened from, the Inbox or an agent over the rest, else
/// the open run's tab, else Claude Code's.
pub fn middle_of(open: Option<&str>, agent: bool, inbox: bool, changes: bool) -> Middle {
    match open {
        _ if changes => Middle::Changes,
        _ if inbox => Middle::Inbox,
        _ if agent => Middle::Agent,
        Some(r) => Middle::Run(r.to_string()),
        None => Middle::Terminal,
    }
}

/// Where a click in the steps panel takes a run's story.
#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    Top,
    /// Where the agent is now: the story's end.
    End,
    /// The latest time the agent worked at that step.
    Step(String),
}

pub struct Tabs {
    runs: Entity<Runs>,
    agents: Entity<Agents>,
    /// The run whose tab shows; None for the person's own terminal.
    pub open: Option<String>,
    /// The agent from the column that shows in the middle, over the rest: its address and title.
    pub agent: Option<(String, String)>,
    /// Whether the Inbox shows in the middle (Command-I), over the rest.
    pub inbox: bool,
    /// The Changes tab: how many files it stacks, while it shows in the middle over the agent it was opened from.
    pub changes: Option<usize>,
    story: Vec<(String, Vec<String>)>,
    /// What a finished run did, in Markdown (`jaynes summary`): none while it runs, or when it cannot be read.
    summary: Option<String>,
    /// What the open run's controls are decided from, read with its story.
    pub facts: Option<crate::run_actions::Facts>,
    /// Where and why the open run is blocked, read with its story; None when it is not.
    pub blocked: Option<BlockedSaid>,
    /// The run whose blocked card shows its raw reason (Details open): only that run's.
    details_open: Option<String>,
    /// When the summary was last copied: its button says Copied for a moment.
    copied_at: Option<std::time::Instant>,
    /// Where the story is scrolled; each step it tells is one of its children, so a step can be scrolled to.
    scroll: ScrollHandle,
    /// Where a click in the steps panel asked the story to go, until the story is read and the tab scrolls there.
    seek: Option<Place>,
    /// The step lit in the story and in the steps panel: the one the person last clicked there.
    pub lit: Option<String>,
    /// The tabs in the person's order, Claude Code's as None: new ones come last, gone ones leave.
    order: Vec<Option<String>>,
    /// The runs whose tab the person closed, with how each stood then (its mark and its line's fingerprint): closed
    /// until it moves on.
    closed: HashMap<String, (Mark, u64)>,
    _poll: Task<()>,
}

impl Tabs {
    pub fn new(runs: Entity<Runs>, agents: Entity<Agents>, cx: &mut Context<Self>) -> Self {
        cx.observe(&runs, |_, _, cx| cx.notify()).detach();
        cx.observe(&agents, |_, _, cx| cx.notify()).detach();
        // the open run's story, read again each second while it is open
        let poll = cx.spawn(async move |this, cx| loop {
            let Ok(open) = this.update(cx, |t, _| t.open.clone()) else { break };
            if let Some(id) = open
                && Self::tell(this.clone(), id, cx).await.is_err()
            {
                break;
            }
            cx.background_executor().timer(Duration::from_secs(1)).await;
        });
        Tabs { runs, agents, open: None, agent: None, inbox: false, changes: None, story: Vec::new(), summary: None, facts: None, blocked: None, details_open: None, copied_at: None, scroll: ScrollHandle::new(), seek: None, lit: None, order: Vec::new(), closed: HashMap::new(), _poll: poll }
    }

    /// Reads a run's story and shows it, if its tab is still the one open when the reading ends.
    async fn tell(this: WeakEntity<Self>, id: String, cx: &mut AsyncApp) -> Result<()> {
        let at = id.clone();
        // the summary is read with the story: it never waits for the role's promises to be proved, the reading after
        // the proof brings them
        let told = cx
            .background_executor()
            .spawn(async move {
                let home = jaynes_shell::home();
                let run = crate::run_actions::read(&at);
                let facts = run.as_ref().map(crate::run_actions::Facts::of);
                let blocked = run.as_ref().and_then(|r| match &r.phase {
                    jaynes_core::run::Phase::Blocked { .. } => jaynes_shell::listing::blocked(&home, r),
                    _ => None,
                });
                (jaynes_shell::listing::story(&home, &at), jaynes_shell::listing::summary(&home, &at).ok().map(|s| s.markdown()), facts, blocked)
            })
            .await;
        let (told, summary, facts, blocked) = told;
        this.update(cx, |t, cx| {
            // in the role's order: the files' times do not say it (a copy of a run, a step sent back)
            let order: Vec<String> = t.runs.read(cx).snap.details.get(&id).map(|d| d.stages.iter().map(|s| s.name.clone()).collect()).unwrap_or_default();
            let told = in_role_order(told, &order);
            if t.open.as_deref() == Some(id.as_str()) && (t.story != told || t.summary != summary || t.facts != facts || t.blocked != blocked) {
                t.story = told;
                t.summary = summary;
                t.facts = facts;
                t.blocked = blocked;
                t.go_to_seek();
                cx.notify();
            }
        })
    }

    /// Shows a run's tab scrolled to a place in its story.
    pub fn show_at(&mut self, run: String, place: Place, cx: &mut Context<Self>) {
        self.show(Some(run), cx);
        self.lit = match &place {
            Place::Step(step) => Some(step.clone()),
            _ => None,
        };
        self.seek = Some(place);
        self.go_to_seek();
        cx.notify();
    }

    /// Scrolls where the click asked once the story is read; a step the agent has not spoken in goes to the top.
    fn go_to_seek(&mut self) {
        if self.story.is_empty() && self.seek != Some(Place::Top) {
            return;
        }
        let top = || point(px(0.), px(0.));
        match self.seek.take() {
            None => {}
            Some(Place::Top) => self.scroll.set_offset(top()),
            Some(Place::End) => self.scroll.scroll_to_bottom(),
            Some(Place::Step(step)) => match self.story.iter().rposition(|(s, _)| *s == step) {
                Some(i) => self.scroll.scroll_to_top_of_item(i),
                None => self.scroll.set_offset(top()),
            },
        }
    }

    /// The open run's block when it is halted: who halted it and at which step.
    pub fn halted(&self, cx: &App) -> Option<String> {
        let open = self.open.as_ref()?;
        if !self.facts.is_some_and(|f| f.halted && !f.finished) {
            return None;
        }
        let runs = self.runs.read(cx);
        let who = runs.snap.halted.get(open).cloned().unwrap_or_else(|| "halted".into());
        // the step under way when it was halted, or else the last one its agent spoke in
        let step = runs.snap.details.get(open).and_then(|d| d.stages.iter().find(|s| s.class == "halted").map(|s| s.name.clone())).or_else(|| self.story.last().map(|(s, _)| s.clone()));
        Some(halted_said(&who, step.as_deref()))
    }

    /// The open run's card when it is blocked, and whether its Details are open.
    pub fn blocked(&self) -> Option<(BlockedSaid, bool)> {
        let said = self.blocked.clone()?;
        Some((said, self.details_open.is_some() && self.details_open == self.open))
    }

    /// Opens or closes the raw reason of the open run's card; another run's opens closed.
    pub fn flip_details(&mut self, cx: &mut Context<Self>) {
        self.details_open = if self.details_open.is_some() && self.details_open == self.open { None } else { self.open.clone() };
        cx.notify();
    }

    /// What shows in the middle now.
    pub fn middle(&self) -> Middle {
        middle_of(self.open.as_deref(), self.agent.is_some(), self.inbox, self.changes.is_some())
    }

    /// Shows the Inbox in the middle.
    pub fn show_inbox(&mut self, cx: &mut Context<Self>) {
        self.show(None, cx);
        self.inbox = true;
        cx.notify();
    }

    /// Shows the Changes tab with its files; the agent open stays where it is, under it, and the folder stays its.
    pub fn show_changes(&mut self, files: usize, cx: &mut Context<Self>) {
        self.inbox = false;
        self.changes = Some(files);
        cx.notify();
    }

    /// The Changes tab now stacks this many files; it goes when none is left.
    pub fn changes_count(&mut self, files: usize, cx: &mut Context<Self>) {
        if self.changes.is_some() {
            self.changes = (files > 0).then_some(files);
            cx.notify();
        }
    }

    /// Shows one of the person's agents (from the column) in the middle.
    pub fn show_agent(&mut self, address: String, title: String, cx: &mut Context<Self>) {
        self.show(None, cx);
        self.agent = Some((address, title));
        cx.notify();
    }

    /// Shows a run's tab (and picks the run), or the person's terminal with None.
    pub fn show(&mut self, run: Option<String>, cx: &mut Context<Self>) {
        self.agent = None;
        self.inbox = false;
        self.changes = None;
        if run != self.open {
            self.story.clear();
            // the run before's summary must not show under this one's story, nor be what its button copies
            self.summary = None;
            self.copied_at = None;
            self.facts = None;
            self.blocked = None;
            self.seek = None;
            self.lit = None;
            self.scroll.set_offset(point(px(0.), px(0.)));
            // read at once, not at the poll's next turn: the tab would show nothing for up to a second
            if let Some(id) = run.clone() {
                cx.spawn(async move |this, cx| Self::tell(this, id, cx).await).detach();
            }
        }
        if let Some(id) = &run {
            let id = id.clone();
            self.runs.update(cx, |runs, cx| runs.select(id, cx));
        }
        self.open = run;
        cx.notify();
    }

    /// The closed tabs still closed, as the app keeps them for the next time it opens: one whose run moved on, or is
    /// gone, is not kept.
    pub fn closed_kept(&self, cx: &App) -> Vec<crate::app_state::Closed> {
        let listing = &self.runs.read(cx).snap.listing;
        let mut kept: Vec<crate::app_state::Closed> = listing
            .rows()
            .filter(|r| self.closed.get(&r.id).is_some_and(|(mark, line)| *mark == r.mark && *line == crate::app_state::fingerprint(&r.line)))
            .map(|r| crate::app_state::Closed { run: r.id.clone(), mark: crate::app_state::mark_word(r.mark).into(), line: crate::app_state::fingerprint(&r.line) })
            .collect();
        kept.sort_by(|a, b| a.run.cmp(&b.run));
        kept.dedup_by(|a, b| a.run == b.run);
        kept
    }

    /// The tabs closed when the app last stood, closed again; a mark this app does not know is skipped.
    pub fn close_kept(&mut self, kept: &[crate::app_state::Closed], cx: &mut Context<Self>) {
        for c in kept {
            if let Some(mark) = crate::app_state::mark_of(&c.mark) {
                self.closed.insert(c.run.clone(), (mark, c.line));
            }
        }
        cx.notify();
    }

    fn tabbed(&self, cx: &App) -> Vec<(String, String, Mark)> {
        tabbed(&self.runs.read(cx).snap.listing, &self.closed, self.open.as_deref())
    }

    /// Command-W: closes the run's tab that shows and moves to its neighbour, as a browser does; it stays closed until
    /// the run moves on. Claude Code's tab never closes.
    /// Closes the tab that shows; false when there is none to close (Claude Code's own tab), and the window goes.
    pub fn close(&mut self, cx: &mut Context<Self>) -> bool {
        // the Changes tab first: under it, the agent it was opened from
        if self.changes.take().is_some() || self.agent.take().is_some() || std::mem::take(&mut self.inbox) {
            cx.notify();
            return true;
        }
        let Some(id) = self.open.clone() else { return false };
        let keys: Vec<Option<String>> = self.shown(cx).into_iter().map(|(k, _, _)| k).collect();
        let at = keys.iter().position(|k| k.as_deref() == Some(id.as_str())).unwrap_or(0);
        let next = if at > 0 { keys.get(at - 1) } else { keys.get(at + 1) }.cloned().flatten();
        let listing = &self.runs.read(cx).snap.listing;
        if let Some(r) = listing.rows().find(|r| r.id == id) {
            self.closed.insert(id, (r.mark, crate::app_state::fingerprint(&r.line)));
        }
        self.show(next, cx);
        true
    }

    /// The tabs as they stand in the bar, Claude Code's as None, with each run's title and mark.
    fn shown(&self, cx: &App) -> Vec<(Option<String>, String, Mark)> {
        let tabbed = self.tabbed(cx);
        let present: Vec<Option<String>> = std::iter::once(None).chain(tabbed.iter().map(|(id, _, _)| Some(id.clone()))).collect();
        arrange(&self.order, &present)
            .into_iter()
            .map(|key| match &key {
                None => (None, String::new(), Mark::Working),
                Some(id) => {
                    let (_, title, mark) = tabbed.iter().find(|(t, _, _)| t == id).cloned().expect("arranged from these");
                    (key, title, mark)
                }
            })
            .collect()
    }

    /// The agents of the open agent's space, its tabs while it shows; empty when no agent shows or no space holds it.
    fn space_tabs(&self, cx: &App) -> Vec<Agent> {
        match &self.agent {
            Some((open, _)) => space_tabs(&self.agents.read(cx).spaces, open),
            None => Vec::new(),
        }
    }

    /// Command-1 to 8: the tab at that place; Command-9: the last, as in a browser. With an agent open, the tabs of
    /// its space.
    pub fn go(&mut self, n: usize, cx: &mut Context<Self>) {
        let space = self.space_tabs(cx);
        if !space.is_empty() {
            if let Some(a) = at(n, space.len()).map(|i| &space[i]) {
                self.show_agent(a.address.clone(), a.title.clone(), cx);
            }
            return;
        }
        let keys: Vec<Option<String>> = self.shown(cx).into_iter().map(|(k, _, _)| k).collect();
        if let Some(key) = at(n, keys.len()).map(|i| keys[i].clone()) {
            self.show(key, cx);
        }
    }

    /// The next tab (1) or the one before (-1), round the ends.
    pub fn step_by(&mut self, by: isize, cx: &mut Context<Self>) {
        let space = self.space_tabs(cx);
        if let Some((open, _)) = &self.agent
            && let Some(here) = space.iter().position(|a| a.address == *open)
        {
            let n = space.len() as isize;
            let a = &space[((here as isize + by) % n + n) as usize % space.len()];
            self.show_agent(a.address.clone(), a.title.clone(), cx);
            return;
        }
        let keys: Vec<Option<String>> = self.shown(cx).into_iter().map(|(k, _, _)| k).collect();
        let n = keys.len() as isize;
        // the agent's tab stands after the rest: the next is the first, the one before the last
        let here = if self.agent.is_some() { if by > 0 { -1 } else { n } } else { keys.iter().position(|k| *k == self.open).unwrap_or(0) as isize };
        if n > 0 {
            let key = keys[((here + by) % n + n) as usize % keys.len()].clone();
            self.show(key, cx);
        }
    }

    /// Puts a tab at a place in the bar.
    pub fn move_tab(&mut self, key: &Option<String>, to: usize, cx: &mut Context<Self>) {
        let keys: Vec<Option<String>> = self.shown(cx).into_iter().map(|(k, _, _)| k).collect();
        self.order = moved(&keys, key, to);
        cx.notify();
    }

    /// The open run's story, in place of the terminal.
    pub fn body(&self, cx: &App) -> Option<AnyElement> {
        let open = self.open.as_ref()?;
        let runs = self.runs.read(cx);
        let detail = runs.snap.details.get(open);
        let working = detail.is_some_and(|d| d.mark == Mark::Working);
        let order: Vec<String> = detail.map(|d| d.stages.iter().map(|s| s.name.clone()).collect()).unwrap_or_default();
        // one child per step told, so the steps panel can scroll the story to one of them
        let mut lines = div()
            .id("story")
            .track_scroll(&self.scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(20.))
            .py(px(10.))
            .font_family(look::MONO_FONT)
            .text_size(px(13.))
            .line_height(px(21.))
            .text_color(rgb(look::story()))
            .flex()
            .flex_col();
        if self.story.is_empty() {
            lines = lines.child(div().text_color(rgb(look::faint())).child(if working { "The agent is starting…" } else { "No agent has spoken in this run yet." }));
        }
        for (step, said) in &self.story {
            let at = order.iter().position(|s| s == step).map(|i| format!("Step {} of {} · {step}", i + 1, order.len())).unwrap_or_else(|| step.clone());
            // the step clicked in the steps panel stands out: a bar at its left and a ground under it
            let lit = self.lit.as_deref() == Some(step.as_str());
            let mut told = div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .mx(px(-10.))
                .pl(px(8.))
                .pr(px(10.))
                .pb(px(6.))
                .rounded(px(6.))
                .border_l_2()
                .border_color(if lit { rgb(look::blue()) } else { rgba(0) })
                .when(lit, |d| d.bg(rgb(look::panel_2())))
                .child(div().pt(px(10.)).text_color(rgb(look::blue())).child(format!("── {at}")));
            for s in said {
                told = told.child(div().flex().gap(px(8.)).child(div().flex_none().text_color(rgb(look::faint())).child("●")).child(div().min_w_0().child(s.clone())));
            }
            lines = lines.child(told);
        }
        if working {
            lines = lines.child(div().pt(px(6.)).flex().gap(px(8.)).items_center().child(look::spinner("story-spin", 11., look::blue())).child(div().text_color(rgb(look::faint())).child("working")));
        }
        if let Some(summary) = &self.summary {
            lines = lines.child(self.summary_section(summary));
        }
        Some(
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(lines)
                // plain words, not a box that reads as a field; none once no agent works in the run
                .when_some(self.facts.and_then(|f| note_for(f.finished, f.halted, f.blocked)), |d, note| {
                    d.child(div().mx(px(20.)).mb(px(10.)).font_family(look::UI_FONT).text_size(px(12.)).text_color(rgb(look::faint())).child(note))
                })
                .into_any_element(),
        )
    }
}

/// How long the summary's button says Copied.
const COPIED_FOR: Duration = Duration::from_millis(2500);

impl Tabs {
    /// A finished run's summary under its story: what it did for someone who was not there, drawn from its Markdown,
    /// with a button that copies the Markdown.
    fn summary_section(&self, summary: &str) -> Stateful<Div> {
        let copied = self.copied_at.is_some_and(|t| t.elapsed() < COPIED_FOR);
        let button = div()
            .id("copy-summary")
            .flex_none()
            .px(px(10.))
            .py(px(4.))
            .rounded(px(7.))
            .border_1()
            .border_color(rgb(look::line_2()))
            .bg(rgb(look::panel_2()))
            .font_family(look::UI_FONT)
            .text_size(px(12.))
            .text_color(rgb(if copied { look::green() } else { look::text() }))
            .cursor_pointer()
            .hover(|d| d.border_color(rgb(look::dim())))
            .child(if copied { "Copied" } else { "Copy as Markdown" })
            .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::CopySummary), cx));
        // drawn as its parts (titles, lists, bold, code), not as raw Markdown: the button copies the Markdown
        let text = div().mt(px(8.)).px(px(14.)).py(px(12.)).rounded(px(8.)).border_1().border_color(rgb(look::line())).bg(rgb(look::panel_2())).child(crate::summary_md::draw(summary));
        div()
            .id("summary")
            .mt(px(22.))
            .pt(px(14.))
            .border_t_1()
            .border_color(rgb(look::line()))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .font_family(look::UI_FONT)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::text())).child("Summary"))
                            .child(div().text_size(px(12.)).text_color(rgb(look::faint())).child("What this run did, for someone who was not here: paste it in a merge request or a note.")),
                    )
                    .child(button),
            )
            .child(text)
    }

    /// The summary on the clipboard, as its button copies it; the button says Copied for a moment.
    pub fn copy_summary(&mut self, cx: &mut Context<Self>) {
        let Some(summary) = self.summary.clone() else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(summary));
        self.copied_at = Some(std::time::Instant::now());
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }
}

/// A tab in the top bar, underlined when it shows: amber for a run that waits, the text's colour otherwise. A press
/// on it is the tab's, not the bar's, so it does not move the window.
fn tab(id: SharedString, on: bool, line: u32) -> Stateful<Div> {
    div()
        .id(id)
        .h_full()
        .flex()
        .items_center()
        .gap(px(7.))
        .px(px(12.))
        .max_w(px(240.))
        .border_b_2()
        .border_color(if on { rgb(line) } else { rgba(0) })
        .text_size(px(12.5))
        .text_color(rgb(if on { look::text() } else { look::dim() }))
        .hover(|d| d.text_color(rgb(look::text())))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

/// A tab on its way to another place in the bar: what follows the mouse.
#[derive(Clone)]
struct Dragged {
    key: Option<String>,
    title: String,
}

impl Render for Dragged {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(12.))
            .py(px(6.))
            .rounded(px(7.))
            .bg(rgb(look::panel_2()))
            .border_1()
            .border_color(rgb(look::line_2()))
            .font_family(look::UI_FONT)
            .text_size(px(12.5))
            .text_color(rgb(look::text()))
            .child(self.title.clone())
    }
}

impl Tabs {
    /// The tabs of the open agent's space, as the familiar look draws them: a pill for the one that shows, each with
    /// its mark and its Command key.
    fn render_space(&self, space: Vec<Agent>, cx: &mut Context<Self>) -> Stateful<Div> {
        let open = self.agent.as_ref().map(|(a, _)| a.clone());
        let mut row = div().id("tabs").h_full().min_w_0().flex().items_center().gap(px(2.)).px(px(4.)).overflow_x_scroll().font_family(look::UI_FONT);
        // the way back to the person's own Claude Code, whose tab the space's tabs replace
        row = row.child(
            div()
                .id("tab-main")
                .h(px(28.))
                .flex_none()
                .px(px(8.))
                .rounded(px(7.))
                .flex()
                .items_center()
                .hover(|d| d.bg(rgb(look::selected())))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| this.show(None, cx)))
                .child(look::main_tab_icon(false)),
        );
        row = row.child(div().w(px(1.)).h(px(16.)).mx(px(4.)).flex_none().bg(rgb(look::line_2())));
        for (n, a) in space.iter().enumerate() {
            let on = open.as_deref() == Some(a.address.as_str()) && self.changes.is_none();
            let (address, title) = (a.address.clone(), a.title.clone());
            row = row.child(
                div()
                    .id(SharedString::from(format!("tab-agent-{}", a.address)))
                    .h(px(28.))
                    .flex_none()
                    .max_w(px(210.))
                    .px(px(10.))
                    .rounded(px(7.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.5))
                    .text_color(rgb(if on { look::text() } else { look::faint() }))
                    .when(on, |d| d.bg(rgb(look::selected())))
                    .hover(|d| d.text_color(rgb(look::dim())))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| this.show_agent(address.clone(), title.clone(), cx)))
                    .child(div().w(px(12.)).flex_none().flex().items_center().justify_center().child(crate::side::agent_icon(a, "tab")))
                    .child(div().min_w_0().truncate().child(a.title.clone()))
                    .when(n < 9, |d| d.child(div().flex_none().font_family(look::MONO_FONT).text_size(px(10.5)).text_color(rgb(look::faint())).child(format!("⌘{}", n + 1)))),
            );
        }
        if let Some(n) = self.changes {
            row = row.child(changes_tab(n, cx));
        }
        // a new agent in this space (Command-T)
        row.child(
            div()
                .id("tab-new-agent")
                .h(px(28.))
                .flex_none()
                .px(px(9.))
                .rounded(px(7.))
                .flex()
                .items_center()
                .text_size(px(16.))
                .text_color(rgb(look::faint()))
                .hover(|d| d.text_color(rgb(look::dim())).bg(rgb(look::selected())))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .tooltip(look::tip("New agent ⌘T"))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::NewAgent), cx))
                .child("+"),
        )
    }
}

impl Render for Tabs {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let space = self.space_tabs(cx);
        if !space.is_empty() {
            return self.render_space(space, cx);
        }
        let shown = self.shown(cx);
        // what the bar shows is the order from now on, so a tab that comes keeps its place among the rest
        self.order = shown.iter().map(|(k, _, _)| k.clone()).collect();
        // the tabs are the top bar's: its height, its line under them
        let mut row = div().id("tabs").h_full().min_w_0().flex().items_stretch().overflow_x_scroll().font_family(look::UI_FONT);
        for (at, (key, title, mark)) in shown.into_iter().enumerate() {
            let on = self.open == key && self.agent.is_none() && !self.inbox;
            let (pick, dragged) = (key.clone(), Dragged { key: key.clone(), title: if key.is_none() { "Claude Code".into() } else { title.clone() } });
            let id = SharedString::from(match &key {
                None => "tab-main".to_string(),
                Some(id) => format!("tab-{id}"),
            });
            let line = if mark == Mark::Needs && key.is_some() { look::amber() } else { look::text() };
            let t = tab(id, on, line)
                .on_click(cx.listener(move |this, _, _, cx| this.show(pick.clone(), cx)))
                .on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
                .drag_over::<Dragged>(|style, _, _, _| style.bg(rgb(look::selected())))
                .on_drop(cx.listener(move |this, d: &Dragged, _, cx| this.move_tab(&d.key, at, cx)));
            row = row.child(match &key {
                None => t.child(look::main_tab_icon(on)).child("Claude Code"),
                Some(id) => t
                    .child(if mark == Mark::Working {
                        look::spinner(SharedString::from(format!("tab-spin-{id}")), 10., look::blue()).into_any_element()
                    } else {
                        div().size(px(7.)).flex_none().rounded_full().bg(rgb(look::colour(mark))).into_any_element()
                    })
                    .child(div().truncate().child(title)),
            });
        }
        if let Some(n) = self.changes {
            row = row.child(div().flex().items_center().px(px(4.)).child(changes_tab(n, cx)));
        }
        if self.inbox {
            row = row.child(tab("tab-inbox".into(), true, look::text()).child(look::icon(look::Icon::Inbox, 13., look::dim())).child("Inbox"));
        }
        // the agent opened from the column, after the rest
        if let Some((address, title)) = &self.agent {
            row = row.child(
                tab(SharedString::from(format!("tab-agent-{address}")), true, look::text())
                    .child(div().flex_none().font_family(look::MONO_FONT).text_size(px(10.)).text_color(rgb(look::faint())).child("›_"))
                    .child(div().truncate().child(title.clone())),
            );
        }
        row
    }
}

/// The Changes tab, lit: its name, how many files it stacks, and × to close it.
fn changes_tab(n: usize, cx: &mut Context<Tabs>) -> Stateful<Div> {
    div()
        .id("tab-changes")
        .h(px(28.))
        .flex_none()
        .px(px(10.))
        .rounded(px(7.))
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(12.5))
        .text_color(rgb(look::text()))
        .bg(rgb(look::selected()))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child("Changes")
        .child(div().text_size(px(11.)).text_color(rgb(look::faint())).child(format!("{n} file{}", if n == 1 { "" } else { "s" })))
        .child(
            div()
                .id("tab-changes-close")
                .text_color(rgb(look::faint()))
                .hover(|d| d.text_color(rgb(look::text())))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.changes = None;
                    cx.notify();
                }))
                .child("×"),
        )
}

/// The tabs of the space that holds `open`, in the column's order, each as one of its agents; empty when no space
/// holds it. A tab Herdr splits in panes is one tab, shown as the pane with the keyboard when it is one of them, else
/// as its first agent that is not a shell.
fn space_tabs(spaces: &[Space], open: &str) -> Vec<Agent> {
    let Some(s) = spaces.iter().find(|s| s.agents.iter().any(|a| a.address == open)) else { return Vec::new() };
    let tab = |a: &Agent| s.tabs.iter().position(|(_, l)| l.panes().contains(&a.address));
    let mut out: Vec<(Option<usize>, Agent)> = Vec::new();
    for a in &s.agents {
        let t = tab(a);
        match out.iter_mut().find(|(o, _)| t.is_some() && *o == t) {
            Some((_, shown)) => {
                if shown.address != open && (a.address == open || (shown.shell && !a.shell)) {
                    *shown = a.clone();
                }
            }
            None => out.push((t, a.clone())),
        }
    }
    out.into_iter().map(|(_, a)| a).collect()
}

/// The place Command-1 to 8 (`n` 0 to 7) take among `len` tabs; Command-9 (8) the last.
fn at(n: usize, len: usize) -> Option<usize> {
    match n {
        _ if len == 0 => None,
        n if n >= 8 => Some(len - 1),
        n => (n < len).then_some(n),
    }
}

/// The runs that get a tab: what waits, then what works, unless the person closed it and it stands as it stood
/// then; and the one open, whatever it is.
fn tabbed(listing: &Listing, closed: &HashMap<String, (Mark, u64)>, open: Option<&str>) -> Vec<(String, String, Mark)> {
    let still_closed = |r: &Row| closed.get(&r.id).is_some_and(|(mark, line)| *mark == r.mark && *line == crate::app_state::fingerprint(&r.line));
    let mut out: Vec<(String, String, Mark)> = listing
        .needs
        .iter()
        .chain(listing.projects.iter().flat_map(|(_, rows)| rows.iter()).filter(|r| r.mark == Mark::Working))
        .filter(|r| !still_closed(r))
        .map(|r| (r.id.clone(), r.title.clone(), r.mark))
        .collect();
    if let Some(open) = open
        && !out.iter().any(|(id, _, _)| id == open)
        && let Some(r) = listing.rows().find(|r| r.id == open)
    {
        out.push((r.id.clone(), r.title.clone(), r.mark));
    }
    out
}

/// The tabs in the order the person left them: those still there in that order, then the new ones as they come.
fn arrange(order: &[Option<String>], present: &[Option<String>]) -> Vec<Option<String>> {
    let mut out: Vec<Option<String>> = order.iter().filter(|k| present.contains(k)).cloned().collect();
    out.extend(present.iter().filter(|k| !order.contains(k)).cloned());
    out
}

/// The order with one tab taken out and put at a place; a tab that is not there leaves the order as it was.
fn moved(order: &[Option<String>], key: &Option<String>, to: usize) -> Vec<Option<String>> {
    let Some(from) = order.iter().position(|k| k == key) else { return order.to_vec() };
    let mut out = order.to_vec();
    let k = out.remove(from);
    out.insert(to.min(out.len()), k);
    out
}

#[cfg(test)]
mod tests {
    // Alan, 21/11: "confuso pra caralho"
    #[test]
    fn asked_the_blocked_card_says_what_happened_first_and_the_raw_only_under_details() {
        use super::{Part, card};
        use jaynes_core::event::Fault;
        // the reason his run kept: the box refused the suite's test server, on a port of its choosing
        let panic = "panic: httptest: failed to listen on a port: listen tcp6 [::1]:0: bind: operation not permitted [recovered, repanicked]";
        let reason = format!(
            "the check suite-passes of change could not judge: the box refused what the command tried, so its failure says nothing about the project: {panic}; {panic}; {panic} It tried local port 0, a port of its own choosing. A check's commands bind and reach only the ports in JAYNES_PORTS (65520, 65521, 65522, 65523, 65524, 65525, 65526, 65527) and the ones the check names in its local_ports, never its step's. Two ways out, the first the better: (1) the suite takes its test servers' ports from JAYNES_PORTS, and the box stays closed; (2) the person opens this project with `jaynes project open-ports /Users/x/Projects/app`, and its checks then reach every service listening on this machine, the Jaynes page among them.. No attempt was spent"
        );
        let mut said = blocked_said(Some("change"), Fault::Jaynes, &reason);
        said.agent_said = vec!["Done. The whole suite passes.".into()];
        let closed = card(&said, false);
        // what happened first, under the title, then why with what the run keeps
        assert_eq!(closed[..3], [
            Part::Title("Blocked at “change”".into()),
            Part::What("The change is done. Jaynes could not check it.".into()),
            Part::Why(format!("{} No attempt was used.", said.why)),
        ]);
        // the agent's words under their heading, then the ways out one a line, the best first, then the buttons, what
        // Continue does, that the run waits, and Details closed last
        let at = |p: &Part| closed.iter().position(|q| q == p).unwrap_or_else(|| panic!("no {p:?} in {closed:?}"));
        let heads = [at(&Part::Heading("The agent said")), at(&Part::Said("Done. The whole suite passes.".into())), at(&Part::Heading("What you can do"))];
        assert!(heads.windows(2).all(|w| w[0] < w[1]), "{closed:?}");
        let ways: Vec<usize> = said.ways.iter().map(|w| at(&Part::Way(w.says.clone()))).collect();
        assert!(ways.len() >= 2 && ways.windows(2).all(|w| w[0] < w[1]) && heads[2] < ways[0], "{closed:?}");
        assert_eq!(said.ways.last().map(|w| w.says.as_str()), Some("Or abandon: the run ends and its change is not kept. Your project stays as it was."));
        let tail = &closed[closed.len() - 4..];
        assert_eq!(tail, [
            Part::Controls,
            Part::ContinueDoes(said.continue_does.clone()),
            Part::Waits("The run waits here until you continue or abandon.".into()),
            Part::Details("▸ Details"),
        ]);
        // nothing raw on the card while Details is closed: no panic, no box terms, never a tool's line
        let shown = format!("{closed:?}");
        for word in ["panic", "JAYNES_PORTS", "local_ports", "Last thing the agent said", "Bash"] {
            assert!(!shown.contains(word), "{word} on the closed card: {shown}");
        }
        // Details open: the raw reason under it, the panic once
        let open = card(&said, true);
        let d = open.iter().position(|p| *p == Part::Details("▾ Details")).expect("Details open");
        assert_eq!(open[d + 1..].iter().filter(|p| matches!(p, Part::Raw(r) if r.contains("panic"))).count(), 1);
        assert!(open[d + 1..].iter().all(|p| matches!(p, Part::Raw(_))) && open.len() > d + 1);
        // a cause with no ways out and no agent's words: no heading over nothing
        let r = card(&blocked_said(None, Fault::Role, "nothing to deliver."), false);
        assert!(!r.iter().any(|p| matches!(p, Part::Heading(_))), "{r:?}");
    }

    #[test]
    fn a_blocked_run_says_at_which_step_why_in_words_and_what_continue_does() {
        let reason = "change spent its 3 attempts: the agent exited with code 1: boom; the agent exited with code 1: boom";
        let b = blocked_said(Some("change"), Fault::Agent, reason);
        // the step's name in curly quotes, always: "change" reads as a verb
        assert_eq!(b.title, "Blocked at “change”");
        assert_eq!(b.why, "“change” used its 3 attempts. The agent exited with code 1.");
        // the agent's attempts come back with Continue (core: Fault::Agent sets spent to 0)
        assert_eq!(b.continue_does, "Continue tries “change” again, with its attempts back.");
        // any other cause: the engine's words, as a sentence, and Continue goes on from there
        let j = blocked_said(Some("change"), Fault::Jaynes, "the role program could not answer: no box here");
        assert_eq!(j.why, "The role program could not answer: no box here.");
        assert_eq!(j.continue_does, "Continue goes on from “change”.");
        let r = blocked_said(None, Fault::Role, "nothing to deliver.");
        assert_eq!((r.title.as_str(), r.why.as_str(), r.continue_does.as_str()), ("Blocked", "Nothing to deliver.", "Continue goes on."));
        // a run blocked before the engine said codes in words keeps "Some(1)" in its record: said as a code too
        let old = blocked_said(Some("change"), Fault::Agent, "change spent its 3 attempts: the agent exited with Some(1):   ");
        assert_eq!(old.why, "“change” used its 3 attempts. The agent exited with code 1.");
        assert_eq!(blocked_said(Some("x"), Fault::Agent, "x spent its 3 attempts: Codex exited with None: ").why, "“x” used its 3 attempts. Codex was stopped by a signal, with no exit code.");
        // an attempts reason with no failure said is still a sentence
        assert_eq!(blocked_said(Some("fix"), Fault::Agent, "fix spent its 2 attempts: ").why, "“fix” used its 2 attempts.");
    }

    #[test]
    fn the_note_on_a_run_s_tab_shows_only_while_its_agent_can_still_work() {
        // while its agent works or the run waits for the person
        assert_eq!(note_for(false, false, false), Some("The agent works on its own and reports to Jaynes. You answer at its stops."));
        // not finished, not halted (nothing works on it until resumed), not blocked (Jaynes stopped it)
        assert_eq!(note_for(true, false, false), None, "a finished run has no agent at work");
        assert_eq!(note_for(false, true, false), None, "a halted run's agent does not work");
        assert_eq!(note_for(false, false, true), None, "a blocked run's agent does not work");
    }

    #[test]
    fn a_halted_run_says_who_halted_it_and_at_which_step() {
        assert_eq!(halted_said("halted by an agent", Some("directions")), "Halted by an agent at “directions”");
        assert_eq!(halted_said("halted by you", Some("copy")), "Halted by you at “copy”");
        assert_eq!(halted_said("halted", None), "Halted");
    }

    #[test]
    fn the_story_tells_the_steps_in_the_role_s_order_whatever_their_files_say() {
        let story = vec![("write".to_string(), vec!["w".to_string()]), ("copy".to_string(), vec!["c".to_string()]), ("gone".to_string(), vec![])];
        let order = vec!["copy".to_string(), "write".to_string()];
        let names: Vec<String> = in_role_order(story, &order).into_iter().map(|(s, _)| s).collect();
        // a step the role does not name (its files only) comes last
        assert_eq!(names, ["copy", "write", "gone"]);
    }

    #[test]
    fn the_middle_is_what_shows_over_the_rest_and_a_run_only_on_its_own_tab() {
        // Claude Code's tab: no run, even one picked before
        assert_eq!(middle_of(None, false, false, false), Middle::Terminal);
        assert_eq!(middle_of(Some("r1"), false, false, false), Middle::Run("r1".into()));
        // what shows over a run's tab is not the run
        assert_eq!(middle_of(None, true, false, false), Middle::Agent);
        assert_eq!(middle_of(None, false, true, false), Middle::Inbox);
        assert_eq!(middle_of(None, true, false, true), Middle::Changes);
        assert_eq!(middle_of(Some("r1"), false, false, true), Middle::Changes);
        // the run whose keys (Command-period, Command-Return) act: only the run's own tab has one
        assert_eq!(Middle::Run("r1".into()).run(), Some("r1"));
        for m in [Middle::Terminal, Middle::Agent, Middle::Inbox, Middle::Changes] {
            assert_eq!(m.run(), None, "{m:?}");
        }
    }

    use jaynes_shell::listing::blocked_said;
    use super::{Middle, arrange, at, halted_said, in_role_order, middle_of, moved, note_for, space_tabs, tabbed};
    use jaynes_core::event::Fault;
    use crate::agents::{Agent, Space, State};
    use jaynes_shell::listing::{Listing, Mark, Row};
    use std::collections::HashMap;

    fn row(id: &str, mark: Mark, line: &str) -> Row {
        Row { id: id.into(), project: String::new(), apart: None, title: id.to_uppercase(), line: line.into(), mark }
    }

    #[test]
    fn a_closed_tab_stays_closed_until_its_run_moves_on() {
        let listing = |line: &str| Listing { older: vec![],
            needs: vec![row("w", Mark::Needs, line)],
            projects: vec![("p".into(), vec![row("k", Mark::Working, "step 1 of 2"), row("d", Mark::Delivered, "delivered")])],
        };
        let ids = |l: &Listing, closed: &HashMap<String, (Mark, u64)>, open: Option<&str>| -> Vec<String> {
            tabbed(l, closed, open).into_iter().map(|(id, _, _)| id).collect()
        };
        let none = HashMap::new();
        assert_eq!(ids(&listing("Pick one"), &none, None), ["w", "k"]);
        assert_eq!(ids(&listing("Pick one"), &none, Some("d")), ["w", "k", "d"]);

        let closed = HashMap::from([("w".to_string(), (Mark::Needs, crate::app_state::fingerprint("Pick one")))]);
        assert_eq!(ids(&listing("Pick one"), &closed, None), ["k"]);
        // a new question at that run is news: its tab comes back
        assert_eq!(ids(&listing("Pick another"), &closed, None), ["w", "k"]);
        // opened again from the column, it shows
        assert_eq!(ids(&listing("Pick one"), &closed, Some("w")), ["k", "w"]);
    }

    fn keys(list: &[&str]) -> Vec<Option<String>> {
        list.iter().map(|k| (*k != "main").then(|| k.to_string())).collect()
    }

    #[test]
    fn the_tabs_keep_the_persons_order_new_ones_coming_last_and_gone_ones_leaving() {
        assert_eq!(arrange(&[], &keys(&["main", "a", "b"])), keys(&["main", "a", "b"]));
        assert_eq!(arrange(&keys(&["b", "main", "a"]), &keys(&["main", "a", "b", "c"])), keys(&["b", "main", "a", "c"]));
        assert_eq!(arrange(&keys(&["b", "main", "a"]), &keys(&["main", "b"])), keys(&["b", "main"]));
    }

    fn space(id: &str, agents: &[&str]) -> Space {
        let agent = |a: &&str| Agent { address: a.to_string(), title: a.to_uppercase(), state: State::Idle, since_ms: 0, shell: false, turn: 0, kind: None, msg: String::new(), cwd: String::new() };
        Space { id: id.into(), name: id.into(), folder: String::new(), agents: agents.iter().map(agent).collect(), tabs: vec![] }
    }

    #[test]
    fn an_open_agent_brings_the_tabs_of_its_space() {
        let spaces = [space("s1", &["t1", "t2"]), space("s2", &["t3", "t4", "t5"])];
        let ids = |open| space_tabs(&spaces, open).iter().map(|a| a.address.clone()).collect::<Vec<_>>();
        assert_eq!(ids("t4"), ["t3", "t4", "t5"]);
        assert_eq!(ids("t1"), ["t1", "t2"]);
        // an agent no space holds (gone, or opened by address) stands alone
        assert!(ids("t9").is_empty());
    }

    #[test]
    fn a_tab_split_in_panes_is_one_tab() {
        use crate::layout::{Dir, Layout};
        let mut s = space("s1", &["t1", "t2", "t3", "t4"]);
        s.agents[1].shell = true;
        // t2 (a shell) and t3 share a tab, split; t1 and t4 have one each
        s.tabs = vec![
            ("tb1".into(), Layout::Pane("t1".into())),
            ("tb2".into(), Layout::Split(Dir::Row, vec![(1.0, Layout::Pane("t2".into())), (1.0, Layout::Pane("t3".into()))])),
            ("tb3".into(), Layout::Pane("t4".into())),
        ];
        let spaces = [s];
        let ids = |open| space_tabs(&spaces, open).iter().map(|a| a.address.clone()).collect::<Vec<_>>();
        // the split tab shows as its agent, not its shell
        assert_eq!(ids("t1"), ["t1", "t3", "t4"]);
        // as the pane with the keyboard, when it is one of the tab's
        assert_eq!(ids("t2"), ["t1", "t2", "t4"]);
        assert_eq!(ids("t3"), ["t1", "t3", "t4"]);
    }

    #[test]
    fn command_1_to_8_take_that_place_and_command_9_the_last() {
        assert_eq!(at(0, 3), Some(0));
        assert_eq!(at(2, 3), Some(2));
        assert_eq!(at(5, 3), None);
        assert_eq!(at(8, 3), Some(2));
        assert_eq!(at(8, 12), Some(11));
        assert_eq!(at(8, 0), None);
    }

    #[test]
    fn a_dragged_tab_lands_where_it_is_dropped() {
        let order = keys(&["main", "a", "b", "c"]);
        assert_eq!(moved(&order, &None, 2), keys(&["a", "b", "main", "c"]));
        assert_eq!(moved(&order, &Some("c".into()), 0), keys(&["c", "main", "a", "b"]));
        assert_eq!(moved(&order, &Some("a".into()), 9), keys(&["main", "b", "c", "a"]));
        assert_eq!(moved(&order, &Some("x".into()), 0), order);
    }
}
