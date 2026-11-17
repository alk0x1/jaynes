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

/// Where a click in the steps panel takes a run's story.
#[derive(Clone, PartialEq)]
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
    /// Where the story is scrolled; each step it tells is one of its children, so a step can be scrolled to.
    scroll: ScrollHandle,
    /// Where a click in the steps panel asked the story to go, until the story is read and the tab scrolls there.
    seek: Option<Place>,
    /// The step lit in the story and in the steps panel: the one the person last clicked there.
    pub lit: Option<String>,
    /// The tabs in the person's order, Claude Code's as None: new ones come last, gone ones leave.
    order: Vec<Option<String>>,
    /// The runs whose tab the person closed, with how each stood then: closed until it moves on.
    closed: HashMap<String, (Mark, String)>,
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
        Tabs { runs, agents, open: None, agent: None, inbox: false, changes: None, story: Vec::new(), scroll: ScrollHandle::new(), seek: None, lit: None, order: Vec::new(), closed: HashMap::new(), _poll: poll }
    }

    /// Reads a run's story and shows it, if its tab is still the one open when the reading ends.
    async fn tell(this: WeakEntity<Self>, id: String, cx: &mut AsyncApp) -> Result<()> {
        let at = id.clone();
        let told = cx.background_executor().spawn(async move { jaynes_shell::listing::story(&jaynes_shell::home(), &at) }).await;
        this.update(cx, |t, cx| {
            if t.open.as_deref() == Some(id.as_str()) && t.story != told {
                t.story = told;
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

    fn tabbed(&self, cx: &App) -> Vec<(String, String, Mark)> {
        tabbed(&self.runs.read(cx).snap.listing, &self.closed, self.open.as_deref())
    }

    /// Command-W: closes the run's tab that shows and moves to its neighbour, as a browser does; it stays closed until
    /// the run moves on. Claude Code's tab never closes.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        // the Changes tab first: under it, the agent it was opened from
        if self.changes.take().is_some() || self.agent.take().is_some() || std::mem::take(&mut self.inbox) {
            cx.notify();
            return;
        }
        let Some(id) = self.open.clone() else { return };
        let keys: Vec<Option<String>> = self.shown(cx).into_iter().map(|(k, _, _)| k).collect();
        let at = keys.iter().position(|k| k.as_deref() == Some(id.as_str())).unwrap_or(0);
        let next = if at > 0 { keys.get(at - 1) } else { keys.get(at + 1) }.cloned().flatten();
        let listing = &self.runs.read(cx).snap.listing;
        if let Some(r) = listing.needs.iter().chain(listing.projects.iter().flat_map(|(_, rows)| rows.iter())).find(|r| r.id == id) {
            self.closed.insert(id, (r.mark, r.line.clone()));
        }
        self.show(next, cx);
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
            .text_color(rgb(0xc9ced8))
            .flex()
            .flex_col();
        if self.story.is_empty() {
            lines = lines.child(div().text_color(rgb(look::FAINT)).child(if working { "The agent is starting…" } else { "No agent has spoken in this run yet." }));
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
                .border_color(if lit { rgb(look::BLUE) } else { rgba(0) })
                .when(lit, |d| d.bg(rgb(look::PANEL_2)))
                .child(div().pt(px(10.)).text_color(rgb(look::BLUE)).child(format!("── {at}")));
            for s in said {
                told = told.child(div().flex().gap(px(8.)).child(div().flex_none().text_color(rgb(look::FAINT)).child("●")).child(div().min_w_0().child(s.clone())));
            }
            lines = lines.child(told);
        }
        if working {
            lines = lines.child(div().pt(px(6.)).flex().gap(px(8.)).items_center().child(look::spinner("story-spin", 11., look::BLUE)).child(div().text_color(rgb(look::FAINT)).child("working")));
        }
        Some(
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(lines)
                .child(
                    div()
                        .mx(px(14.))
                        .mb(px(12.))
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(9.))
                        .border_1()
                        .border_color(rgb(look::LINE))
                        .font_family(look::UI_FONT)
                        .text_size(px(12.5))
                        .text_color(rgb(look::FAINT))
                        .child("This run's agent works on its own and answers to Jaynes; you answer it at its stops."),
                )
                .into_any_element(),
        )
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
        .text_color(rgb(if on { look::TEXT } else { look::DIM }))
        .hover(|d| d.text_color(rgb(look::TEXT)))
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
            .bg(rgb(look::PANEL_2))
            .border_1()
            .border_color(rgb(look::LINE_2))
            .font_family(look::UI_FONT)
            .text_size(px(12.5))
            .text_color(rgb(look::TEXT))
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
                .hover(|d| d.bg(rgb(look::SELECTED)))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| this.show(None, cx)))
                .child(look::claude_icon()),
        );
        row = row.child(div().w(px(1.)).h(px(16.)).mx(px(4.)).flex_none().bg(rgb(look::LINE_2)));
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
                    .text_color(rgb(if on { look::TEXT } else { look::FAINT }))
                    .when(on, |d| d.bg(rgb(look::SELECTED)))
                    .hover(|d| d.text_color(rgb(look::DIM)))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| this.show_agent(address.clone(), title.clone(), cx)))
                    .child(div().w(px(12.)).flex_none().flex().items_center().justify_center().child(crate::side::agent_icon(a, "tab")))
                    .child(div().min_w_0().truncate().child(a.title.clone()))
                    .when(n < 9, |d| d.child(div().flex_none().font_family(look::MONO_FONT).text_size(px(10.5)).text_color(rgb(look::FAINT)).child(format!("⌘{}", n + 1)))),
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
                .text_color(rgb(look::FAINT))
                .hover(|d| d.text_color(rgb(look::DIM)).bg(rgb(look::SELECTED)))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
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
            let line = if mark == Mark::Needs && key.is_some() { look::AMBER } else { look::TEXT };
            let t = tab(id, on, line)
                .on_click(cx.listener(move |this, _, _, cx| this.show(pick.clone(), cx)))
                .on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
                .drag_over::<Dragged>(|style, _, _, _| style.bg(rgb(look::SELECTED)))
                .on_drop(cx.listener(move |this, d: &Dragged, _, cx| this.move_tab(&d.key, at, cx)));
            row = row.child(match &key {
                None => t.child(look::claude_icon()).child("Claude Code"),
                Some(id) => t
                    .child(if mark == Mark::Working {
                        look::spinner(SharedString::from(format!("tab-spin-{id}")), 10., look::BLUE).into_any_element()
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
            row = row.child(tab("tab-inbox".into(), true, look::TEXT).child(look::icon(look::Icon::Inbox, 13., look::DIM)).child("Inbox"));
        }
        // the agent opened from the column, after the rest
        if let Some((address, title)) = &self.agent {
            row = row.child(
                tab(SharedString::from(format!("tab-agent-{address}")), true, look::TEXT)
                    .child(div().flex_none().font_family(look::MONO_FONT).text_size(px(10.)).text_color(rgb(look::FAINT)).child("›_"))
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
        .text_color(rgb(look::TEXT))
        .bg(rgb(look::SELECTED))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child("Changes")
        .child(div().text_size(px(11.)).text_color(rgb(look::FAINT)).child(format!("{n} file{}", if n == 1 { "" } else { "s" })))
        .child(
            div()
                .id("tab-changes-close")
                .text_color(rgb(look::FAINT))
                .hover(|d| d.text_color(rgb(look::TEXT)))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.changes = None;
                    cx.notify();
                }))
                .child("×"),
        )
}

/// The agents of the space that holds `open`, in the column's order; empty when no space holds it.
fn space_tabs(spaces: &[Space], open: &str) -> Vec<Agent> {
    spaces.iter().find(|s| s.agents.iter().any(|a| a.address == open)).map(|s| s.agents.clone()).unwrap_or_default()
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
fn tabbed(listing: &Listing, closed: &HashMap<String, (Mark, String)>, open: Option<&str>) -> Vec<(String, String, Mark)> {
    let still_closed = |r: &Row| closed.get(&r.id).is_some_and(|(mark, line)| *mark == r.mark && *line == r.line);
    let mut out: Vec<(String, String, Mark)> = listing
        .needs
        .iter()
        .chain(listing.projects.iter().flat_map(|(_, rows)| rows.iter()).filter(|r| r.mark == Mark::Working))
        .filter(|r| !still_closed(r))
        .map(|r| (r.id.clone(), r.title.clone(), r.mark))
        .collect();
    if let Some(open) = open
        && !out.iter().any(|(id, _, _)| id == open)
        && let Some(r) = listing.needs.iter().chain(listing.projects.iter().flat_map(|(_, rows)| rows.iter())).find(|r| r.id == open)
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
    use super::{arrange, at, moved, space_tabs, tabbed};
    use crate::agents::{Agent, Space, State};
    use jaynes_shell::listing::{Listing, Mark, Row};
    use std::collections::HashMap;

    fn row(id: &str, mark: Mark, line: &str) -> Row {
        Row { id: id.into(), title: id.to_uppercase(), line: line.into(), mark }
    }

    #[test]
    fn a_closed_tab_stays_closed_until_its_run_moves_on() {
        let listing = |line: &str| Listing {
            needs: vec![row("w", Mark::Needs, line)],
            projects: vec![("p".into(), vec![row("k", Mark::Working, "step 1 of 2"), row("d", Mark::Delivered, "delivered")])],
        };
        let ids = |l: &Listing, closed: &HashMap<String, (Mark, String)>, open: Option<&str>| -> Vec<String> {
            tabbed(l, closed, open).into_iter().map(|(id, _, _)| id).collect()
        };
        let none = HashMap::new();
        assert_eq!(ids(&listing("Pick one"), &none, None), ["w", "k"]);
        assert_eq!(ids(&listing("Pick one"), &none, Some("d")), ["w", "k", "d"]);

        let closed = HashMap::from([("w".to_string(), (Mark::Needs, "Pick one".to_string()))]);
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
