//! The column at the left, in the app's familiar look: the Inbox with how many things need the person, then the
//! person's spaces with their agents (read from herdr-core, as Herdr arranges them), each with its dot and a short word,
//! then the Jaynes runs. The runs' list is the engine's own (`jaynes_shell::listing`), read from the record, so it says
//! what `jaynes runs` and the page say.

use crate::agents::{self, Agent, Agents, State};
use crate::look;
use crate::model::Runs;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use jaynes_shell::listing::{Mark, Row};
use std::collections::HashSet;

pub const WIDTH: f32 = 292.;

pub struct Side {
    runs: Entity<Runs>,
    agents: Entity<Agents>,
    tabs: Entity<crate::tabs::Tabs>,
    /// The spaces the person folded.
    folded: HashSet<String>,
}

/// An agent's state as the column colours it.
fn colour(state: State) -> u32 {
    match state {
        State::Waiting => look::amber(),
        State::Error => look::red(),
        State::Working => look::blue(),
        State::ToReview => look::violet(),
        State::Idle => look::faint(),
    }
}

fn dot(color: u32) -> Div {
    div().size(px(8.)).rounded_full().flex_none().bg(rgb(color))
}

impl Side {
    pub fn new(runs: Entity<Runs>, agents: Entity<Agents>, tabs: Entity<crate::tabs::Tabs>, cx: &mut Context<Self>) -> Self {
        cx.observe(&runs, |_, _, cx| cx.notify()).detach();
        cx.observe(&agents, |_, _, cx| cx.notify()).detach();
        cx.observe(&tabs, |_, _, cx| cx.notify()).detach();
        Side { runs, agents, tabs, folded: HashSet::new() }
    }

    fn inbox(&self, waiting: usize, review: usize, on: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let need = waiting + review;
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
                        .bg(rgb(if waiting > 0 { look::amber() } else { look::violet() }))
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(look::on_fill()))
                        .child(need.to_string()),
                )
            })
    }

    fn space_head(&self, id: &str, name: &str, folded: bool, waits: bool, cx: &mut Context<Self>) -> impl IntoElement {
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
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.folded.remove(&key) {
                    this.folded.insert(key.clone());
                }
                cx.notify();
            }))
            .child(div().w(px(10.)).text_color(rgb(look::faint())).child(if folded { "▸" } else { "▾" }))
            .child(div().flex_1().min_w_0().truncate().child(name.to_owned()))
            // a folded space still says that something in it waits
            .when(folded && waits, |d| d.child(dot(look::amber())))
    }

    fn agent(&self, a: &Agent, on: bool, now: u64, cx: &mut Context<Self>) -> impl IntoElement {
        let icon = agent_icon(a, "agent");
        let right = if a.shell || a.state == State::Idle {
            div().text_color(rgb(look::faint())).child(agents::age(a.since_ms, now))
        } else {
            div().text_color(rgb(colour(a.state))).child(a.state.word())
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

    fn row(&self, row: &Row, on: bool, cx: &mut Context<Self>) -> impl IntoElement {
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
            .child(div().mt(px(3.)).size(px(12.)).flex_none().flex().items_center().justify_center().child(if row.mark == Mark::Working {
                look::spinner(SharedString::from(format!("spin-{}", row.id)), 11., look::blue()).into_any_element()
            } else {
                dot(look::colour(row.mark)).into_any_element()
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .flex_1()
                    .child(div().truncate().text_size(px(13.)).text_color(rgb(look::text())).child(row.title.clone()))
                    .child(div().truncate().text_size(px(11.5)).text_color(rgb(if row.mark == Mark::Needs { look::amber() } else { look::dim() })).child(row.line.clone())),
            )
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
            div().size(px(8.)).rounded_full().flex_none().border(px(1.5)).border_color(rgb(0x4a505c)).into_any_element()
        } else {
            dot(colour(a.state)).into_any_element()
        }
}

fn section(name: &str, count: Option<usize>, color: u32) -> impl IntoElement {
    div()
        .flex()
        .justify_between()
        .px(px(18.))
        .pt(px(16.))
        .pb(px(4.))
        .child(look::label(name, color))
        .children(count.map(|n| div().text_size(px(11.)).text_color(rgb(look::faint())).child(n.to_string())))
}

impl Render for Side {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let runs = self.runs.read(cx);
        let (listing, selected, trouble) = (runs.snap.listing.clone(), runs.selected.clone(), runs.trouble.clone());
        let agents = self.agents.read(cx);
        let (spaces, core_trouble) = (agents.spaces.clone(), agents.trouble.clone());
        let (waiting, review) = agents.counts();
        let now = agents::now_ms();
        let open = self.tabs.read(cx).agent.as_ref().map(|(a, _)| a.clone());
        let mut list = div().id("side-list").flex().flex_col().flex_1().overflow_y_scroll().pb(px(12.));
        let inbox_on = self.tabs.read(cx).inbox;
        list = list.child(self.inbox(waiting + listing.needs.len(), review, inbox_on, cx));

        list = list.child(section("Spaces", None, look::faint()));
        if let Some(trouble) = core_trouble {
            list = list.child(div().px(px(18.)).py(px(6.)).text_size(px(12.)).text_color(rgb(look::red())).child(format!("herdr-core cannot be read: {trouble}")));
        } else if spaces.is_empty() {
            list = list.child(div().px(px(18.)).py(px(6.)).text_size(px(12.)).text_color(rgb(look::faint())).child("No spaces yet."));
        }
        for space in &spaces {
            let folded = self.folded.contains(&space.id);
            let waits = space.agents.iter().any(|a| matches!(a.state, State::Waiting | State::Error));
            list = list.child(self.space_head(&space.id, &space.name, folded, waits, cx));
            if !folded {
                for a in &space.agents {
                    list = list.child(self.agent(a, open.as_deref() == Some(a.address.as_str()), now, cx));
                }
            }
        }

        list = list.child(section("Runs", None, look::faint()));
        if let Some(trouble) = trouble {
            list = list.child(div().px(px(18.)).py(px(6.)).text_size(px(12.)).text_color(rgb(look::red())).child(format!("The record cannot be read: {trouble}")));
        } else if listing.needs.is_empty() && listing.projects.is_empty() {
            list = list.child(div().px(px(18.)).py(px(6.)).text_size(px(12.)).text_color(rgb(look::faint())).child("No runs yet."));
        }
        if !listing.needs.is_empty() {
            list = list.child(section("Waits for you", Some(listing.needs.len()), look::amber()));
            for row in &listing.needs {
                list = list.child(self.row(row, selected.as_deref() == Some(row.id.as_str()), cx));
            }
        }
        for (project, rows) in &listing.projects {
            list = list.child(section(project, None, look::faint()));
            for row in rows {
                list = list.child(self.row(row, selected.as_deref() == Some(row.id.as_str()), cx));
            }
        }
        // the column's width, its header and its line at the right are the window's (main.rs)
        div().size_full().flex().flex_col().min_h_0().font_family(look::UI_FONT).child(list)
    }
}
