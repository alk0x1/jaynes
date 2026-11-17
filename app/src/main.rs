//! The Jaynes app: a native window where the person works with Claude Code.
//!
//! The window: the runs at the left (from the engine's record), one terminal running `claude` in the middle, and the
//! picked run's steps at the right. The terminal lives in herdr-core (`hc`), not in the app, so closing the window
//! leaves the conversation running, and opening the app again shows it where it was.
//! The app finds its terminal by a label it puts on it (`jaynes.app=main`); a terminal whose program ended is replaced.

// The terminal copied from herdr-app keeps abilities this first window does not call yet (search, scrolling commands,
// theme switching); the later steps use them.
#![allow(dead_code)]

mod agents;
mod ask;
mod inbox;
mod answer;
mod bench;
mod compare;
mod core_bin;
mod hc;
mod island;
mod key;
mod lightbox;
mod links;
mod look;
mod model;
mod menubar;
mod news;
mod notify;
mod presence;
mod rail;
mod settings;
mod side;
mod spawn;
mod tabs;
mod sprites;
mod terminal;
mod vt;
mod win32_keys;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use serde_json::{json, Value};
use std::sync::Arc;
use terminal::TerminalView;

actions!(
    jaynes,
    [
        Copy, Paste, Quit, ToggleSteps, NewRun, ToggleRuns, Tab1, Tab2, Tab3, Tab4, Tab5, Tab6, Tab7, Tab8, Tab9, NextTab, PreviousTab,
        NextRun, PreviousRun, OpenWaiting, SendAnswer, HaltRun, CloseTab, Shortcuts, NewAgent, Deny, Pick1, Pick2, Pick3, Pick4, Pick5, Pick6, Pick7, Pick8, Pick9, ShowInbox
    ]
);

/// Every shortcut, as the list over the window (Command-/) shows it.
const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    ("Tabs", &[("⌘1 … ⌘8", "The tab at that place"), ("⌘9", "The last tab"), ("⇧⌘]  ⇧⌘[", "Next tab, previous tab"), ("⌘W", "Close a run's tab"), ("⌘T", "New agent in this space")]),
    ("Runs", &[("⌥⌘↓  ⌥⌘↑", "Next run, previous run"),  ("⌘↩", "Send your answer"), ("⌘.", "Stop the open agent, or halt the picked run"), ("⌘N", "New run")]),
    ("Inbox", &[("⌘I", "The Inbox: what needs you, in every space"), ("⌘O", "The next thing that needs you")]),
    ("Agents", &[("⌘↩", "Allow, the first choice"), ("⌘⌫", "Deny"), ("⌥1 … ⌥9", "Pick that choice")]),
    ("Window", &[("⌘0", "Runs column"), ("⌘E", "Steps panel"), ("⌘/", "This list")]),
];

/// The top bar's height; the macOS window buttons sit inside it, in the runs column's header.
const TOP: f32 = 42.;
/// The status bar's height, at the bottom of the window.
const STATUS: f32 = 28.;

/// The label the app puts on its terminal, so it finds it again after the window closes.
const LABEL: (&str, &str) = ("jaynes.app", "main");

/// The live terminal the app made earlier, if its program still runs.
fn existing(core: &hc::Core) -> Option<String> {
    let listed = core.call("terminal.list", json!({"labels": {LABEL.0: LABEL.1}})).ok()?;
    let terminals = listed.get("terminals").and_then(Value::as_array).cloned().unwrap_or_default();
    terminals
        .iter()
        .find(|t| t.get("state").and_then(Value::as_str) == Some("live"))
        .and_then(|t| t.get("address").and_then(Value::as_str).map(str::to_owned))
}

/// The folder Claude Code starts in: the one named on the command line, else the one the app was started from.
fn folder() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    // opened from Finder or the Dock, the app starts in "/": the person's home is where they work
    let here = std::env::current_dir().ok().map(|d| d.to_string_lossy().into_owned()).filter(|d| d != "/");
    std::env::args().nth(1).filter(|a| !a.starts_with("-psn")).or(here).unwrap_or(home)
}

struct Root {
    runs: Entity<model::Runs>,
    tabs: Entity<tabs::Tabs>,
    side: Entity<side::Side>,
    terminal: Entity<TerminalView>,
    core: Arc<hc::Core>,
    agents: Entity<agents::Agents>,
    /// The agent from the column open in the middle, attached while it shows: attached, the app sets its size while
    /// the person uses it here, and Herdr sets it again once they type there (herdr-core sizes for the viewer used
    /// last).
    agent_view: Option<(String, Entity<TerminalView>)>,
    rail: Entity<rail::Rail>,
    island: Entity<island::Island>,
    compare: Entity<compare::Compare>,
    lightbox: Entity<lightbox::Lightbox>,
    /// Whether the steps panel shows (Command-E).
    steps: bool,
    /// Whether the runs column shows.
    sidebar: bool,
    /// The branch of the folder Claude Code works in, read every few seconds; empty outside git.
    branch: String,
    /// Whether the list of shortcuts shows over the window (Command-/).
    help: bool,
    /// The window's own focus, where the keys go while a run's story shows instead of the terminal.
    focus: FocusHandle,
    /// The tab that showed when the focus last followed it.
    was_open: Option<String>,
    /// Whether the Inbox showed then.
    was_inbox: bool,
    _branch: Task<()>,
}

/// Halts each run at work: the engine stops its agent where it is, and the run can be resumed.
fn halt(runs: &[String]) {
    for id in runs {
        let spawned = spawn::command(answer::engine_program()).args(["halt", id]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
        if let Err(e) = spawned {
            tracing::warn!("cannot halt {id}: {e}");
        }
    }
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        terminal::next_frame();
        let terminal = self.terminal.clone();
        let shown = self.shown_terminal();
        let middle = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.tabs_bar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .children(self.ask_bar(cx))
                            .child(div().flex_1().min_h_0().child(match (&self.agent_view, self.tabs.read(cx).body(cx)) {
                                _ if self.tabs.read(cx).inbox => self.inbox_view(cx),
                                (Some((_, agent)), _) => agent.clone().into_any_element(),
                                (None, Some(story)) => story,
                                (None, None) => self.terminal.clone().into_any_element(),
                            }))
                            // a run's question, over Claude Code's terminal and the runs' tabs, not over an agent or the Inbox
                            .when(self.agent_view.is_none() && !self.tabs.read(cx).inbox, |d| d.child(self.island.clone())),
                    )
                    .children(self.steps.then(|| self.rail.clone())),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(look::PANEL))
            .font_family(look::UI_FONT)
            .track_focus(&self.focus)
            .key_context("Jaynes")
            .on_action(copy_into(&shown))
            .on_action(paste_into(&shown))
            .on_action(cx.listener(|this, _: &ToggleSteps, _, cx| {
                this.steps = !this.steps;
                cx.notify();
            }))
            .on_action(move |_: &NewRun, window, cx| new_run(&terminal, window, cx))
            .on_action(cx.listener(|this, _: &ToggleRuns, _, cx| {
                this.sidebar = !this.sidebar;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Tab1, _, cx| this.tabs.update(cx, |t, cx| t.go(0, cx))))
            .on_action(cx.listener(|this, _: &Tab2, _, cx| this.tabs.update(cx, |t, cx| t.go(1, cx))))
            .on_action(cx.listener(|this, _: &Tab3, _, cx| this.tabs.update(cx, |t, cx| t.go(2, cx))))
            .on_action(cx.listener(|this, _: &Tab4, _, cx| this.tabs.update(cx, |t, cx| t.go(3, cx))))
            .on_action(cx.listener(|this, _: &Tab5, _, cx| this.tabs.update(cx, |t, cx| t.go(4, cx))))
            .on_action(cx.listener(|this, _: &Tab6, _, cx| this.tabs.update(cx, |t, cx| t.go(5, cx))))
            .on_action(cx.listener(|this, _: &Tab7, _, cx| this.tabs.update(cx, |t, cx| t.go(6, cx))))
            .on_action(cx.listener(|this, _: &Tab8, _, cx| this.tabs.update(cx, |t, cx| t.go(7, cx))))
            .on_action(cx.listener(|this, _: &Tab9, _, cx| this.tabs.update(cx, |t, cx| t.go(8, cx))))
            .on_action(cx.listener(|this, _: &NextTab, _, cx| this.tabs.update(cx, |t, cx| t.step_by(1, cx))))
            .on_action(cx.listener(|this, _: &PreviousTab, _, cx| this.tabs.update(cx, |t, cx| t.step_by(-1, cx))))
            .on_action(cx.listener(|this, _: &NextRun, _, cx| this.step_run(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousRun, _, cx| this.step_run(-1, cx)))
            .on_action(cx.listener(|this, _: &OpenWaiting, window, cx| this.open_next(window, cx)))
            .on_action(cx.listener(|this, _: &ShowInbox, _, cx| this.tabs.update(cx, |t, cx| t.show_inbox(cx))))
            .on_action(cx.listener(|this, _: &SendAnswer, _, cx| {
                // the open agent's first choice when it asks, else the run's answer
                if !this.answer(Answer::Pick(1), cx) {
                    this.island.update(cx, |i, cx| i.send_if_ready(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &Deny, _, cx| this.answer_or_pass(Answer::Deny, cx)))
            .on_action(cx.listener(|this, _: &Pick1, _, cx| this.answer_or_pass(Answer::Pick(1), cx)))
            .on_action(cx.listener(|this, _: &Pick2, _, cx| this.answer_or_pass(Answer::Pick(2), cx)))
            .on_action(cx.listener(|this, _: &Pick3, _, cx| this.answer_or_pass(Answer::Pick(3), cx)))
            .on_action(cx.listener(|this, _: &Pick4, _, cx| this.answer_or_pass(Answer::Pick(4), cx)))
            .on_action(cx.listener(|this, _: &Pick5, _, cx| this.answer_or_pass(Answer::Pick(5), cx)))
            .on_action(cx.listener(|this, _: &Pick6, _, cx| this.answer_or_pass(Answer::Pick(6), cx)))
            .on_action(cx.listener(|this, _: &Pick7, _, cx| this.answer_or_pass(Answer::Pick(7), cx)))
            .on_action(cx.listener(|this, _: &Pick8, _, cx| this.answer_or_pass(Answer::Pick(8), cx)))
            .on_action(cx.listener(|this, _: &Pick9, _, cx| this.answer_or_pass(Answer::Pick(9), cx)))
            .on_action(cx.listener(|this, _: &HaltRun, _, cx| {
                // over an agent, it stops the agent; else the picked run
                if this.agent_view.is_some() {
                    this.stop_agent(cx);
                } else {
                    this.halt_picked(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CloseTab, _, cx| this.tabs.update(cx, |t, cx| t.close(cx))))
            .on_action(cx.listener(|this, _: &NewAgent, window, cx| this.new_agent(window, cx)))
            .on_action(cx.listener(|this, _: &Shortcuts, _, cx| {
                this.help = !this.help;
                cx.notify();
            }))
            // Esc puts the list of shortcuts away before the terminal sees it
            .when(self.help, |d| {
                d.capture_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                    if e.keystroke.key == "escape" {
                        this.help = false;
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
            })
            .relative()
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .when(self.sidebar, |d| {
                        d.child(
                            div()
                                .w(px(side::WIDTH))
                                .h_full()
                                .flex_none()
                                .flex()
                                .flex_col()
                                .border_r_1()
                                .border_color(rgb(look::LINE))
                                .child(self.side_head(cx))
                                .child(div().flex_1().min_h_0().child(self.side.clone())),
                        )
                    })
                    .child(middle),
            )
            .child(self.status_bar(cx))
            // over everything: the compare mode, then a picture or a diff at full size
            .child(self.compare.clone())
            .child(self.lightbox.clone())
            .when(self.help, |d| d.child(self.shortcuts(cx)))
    }
}

/// A piece of the top bar: dragging it moves the window, a double click zooms it, as a title bar does.
fn title_bar(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(TOP))
        .flex_none()
        .flex()
        .items_center()
        .border_b_1()
        .border_color(rgb(look::LINE))
        .on_mouse_down(MouseButton::Left, |e, window, _| {
            if e.click_count == 2 {
                window.titlebar_double_click();
            } else {
                window.start_window_move();
            }
        })
}

/// A press on a button in the top bar is the button's, not the bar's.
fn own_press<E: InteractiveElement>(e: E) -> E {
    e.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

impl Root {
    /// The terminal in the middle: the agent open from the column, else the person's own Claude Code.
    fn shown_terminal(&self) -> Entity<TerminalView> {
        self.agent_view.as_ref().map(|(_, v)| v.clone()).unwrap_or_else(|| self.terminal.clone())
    }

    /// Attaches the agent the tabs show, letting go of the one shown before; the person looks at it from now on.
    fn follow_agent(&mut self, address: Option<String>, cx: &mut Context<Self>) {
        if self.agent_view.as_ref().map(|(a, _)| a) == address.as_ref() {
            return;
        }
        if let Some((_, old)) = self.agent_view.take() {
            old.read(cx).detach();
        }
        let core = self.core.clone();
        self.agent_view = address.clone().map(|a| (a.clone(), cx.new(|cx| TerminalView::attach(core, a, (120, 40), cx))));
        self.agents.update(cx, |agents, cx| agents.open(address, cx));
    }

    /// What the open agent asks, when it shows and waits.
    fn open_ask(&self, cx: &App) -> Option<agents::Ask> {
        let (open, _) = self.agent_view.as_ref()?;
        self.agents.read(cx).ask.clone().filter(|a| a.address == *open)
    }

    /// Answers the open agent as its keys would: a choice's number, or Esc to deny. False when it asks nothing, or
    /// has no such choice.
    fn answer(&mut self, answer: Answer, cx: &mut Context<Self>) -> bool {
        let Some(ask) = self.open_ask(cx) else { return false };
        let key = match answer {
            Answer::Pick(n) if n <= ask.choices.len() => vec![b'0' + n as u8],
            Answer::Pick(_) => return false,
            Answer::Deny => vec![0x1b],
        };
        if let Some((_, view)) = &self.agent_view {
            view.read(cx).press(&key);
        }
        // answered: the bar goes until the agent asks again
        self.agents.update(cx, |a, cx| {
            a.ask = None;
            cx.notify();
        });
        true
    }

    /// Option-1 to 9 and Command-Backspace answer the open agent when it asks; otherwise the keys go on as they were.
    fn answer_or_pass(&mut self, answer: Answer, cx: &mut Context<Self>) {
        if !self.answer(answer, cx) {
            cx.propagate();
        }
    }

    /// The bar over a waiting agent's terminal, in the familiar look: what it asks, then its choices as buttons
    /// with their keys, the first in amber (Command-Return), and Deny (Command-Backspace).
    fn ask_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ask = self.open_ask(cx)?;
        let button = |id: SharedString, text: String, key: String, main: bool| {
            div()
                .id(id)
                .flex_none()
                .max_w(px(260.))
                .flex()
                .items_center()
                .gap(px(7.))
                .px(px(12.))
                .py(px(6.))
                .rounded(px(7.))
                .border_1()
                .border_color(rgb(if main { look::AMBER } else { look::LINE_2 }))
                .bg(rgb(if main { look::AMBER } else { look::PANEL_2 }))
                .text_size(px(12.5))
                .text_color(rgb(if main { look::ON_FILL } else { look::TEXT }))
                .when(main, |d| d.font_weight(FontWeight::SEMIBOLD))
                .when(!main, |d| d.hover(|d| d.bg(rgb(look::SELECTED))))
                .cursor_pointer()
                .child(div().min_w_0().truncate().child(text))
                .child(div().flex_none().text_size(px(11.)).opacity(0.6).child(key))
        };
        let mut acts = div().flex().flex_wrap().items_center().gap(px(8.));
        acts = acts.child(
            button("ask-deny".into(), "Deny".into(), "⌘⌫".into(), false)
                .border_color(rgba(0))
                .bg(rgba(0))
                .text_color(rgb(look::DIM))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.answer(Answer::Deny, cx);
                })),
        );
        for (i, choice) in ask.choices.iter().enumerate().take(9) {
            let n = i + 1;
            let key = if n == 1 { "⌘↩".to_string() } else { format!("⌥{n}") };
            acts = acts.child(button(SharedString::from(format!("ask-{n}")), choice.clone(), key, n == 1).on_click(cx.listener(move |this, _, _, cx| {
                this.answer(Answer::Pick(n), cx);
            })));
        }
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .gap(px(10.))
                .px(px(18.))
                .py(px(12.))
                .relative()
                .border_b_1()
                .border_color(rgb(look::LINE))
                .bg(rgba((look::AMBER << 8) | 0x0c))
                // the amber edge at the left, as the familiar look marks what waits
                .child(div().absolute().left_0().top_0().bottom_0().w(px(3.)).bg(rgb(look::AMBER)))
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .gap(px(6.))
                        .text_size(px(13.))
                        .line_height(px(19.5))
                        .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::TEXT)).child(ask.title.clone()))
                        .child(div().min_w_0().truncate().text_color(rgb(look::DIM)).child(ask.what.clone())),
                )
                .child(acts)
                .into_any_element(),
        )
    }

    /// Command-T or the + after a space's tabs: Claude Code in a new tab of the open agent's space, opened at once.
    fn new_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((open, _)) = self.tabs.read(cx).agent.clone() else { return };
        let Some(space) = self.agents.read(cx).spaces.iter().find(|s| s.agents.iter().any(|a| a.address == open)).cloned() else { return };
        let (core, tabs, display) = (self.core.clone(), self.tabs.clone(), terminal::initial_display(window));
        cx.spawn(async move |_, cx| {
            match cx.background_executor().spawn(async move { agents::start_in(&core, &space, display) }).await {
                Ok(address) => {
                    let _ = tabs.update(cx, |t, cx| t.show_agent(address, "claude".into(), cx));
                }
                Err(e) => tracing::warn!("{e}"),
            }
        })
        .detach();
    }

    /// The runs column's header, after herdr: the macOS buttons, the mark and the name, then the column's switch.
    fn side_head(&self, cx: &mut Context<Self>) -> impl IntoElement {
        title_bar("side-head")
            // room for the macOS buttons, as the familiar look leaves it
            .pl(px(82.))
            .pr(px(8.))
            .child(div().flex_1())
            .child(self.sidebar_switch(cx))
    }

    fn sidebar_switch(&self, cx: &mut Context<Self>) -> impl IntoElement {
        own_press(look::icon_button("sidebar-switch", look::Icon::Sidebar, false)).on_click(cx.listener(|this, _, _, cx| {
            this.sidebar = !this.sidebar;
            cx.notify();
        }))
    }

    /// The middle's header: the tabs are the bar, then a new run and the steps panel's switch, as icons.
    fn tabs_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        title_bar("tabs-bar")
            .items_stretch()
            .pr(px(10.))
            .gap(px(4.))
            // without the runs column, the macOS buttons and its switch move here
            .when(!self.sidebar, |d| d.pl(px(82.)).child(div().flex().items_center().child(self.sidebar_switch(cx))))
            .when(self.sidebar, |d| d.pl(px(4.)))
            .child(self.tabs.clone())
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(own_press(look::icon_button("new-run", look::Icon::Plus, false)).on_click(cx.listener(|this, _, window, cx| {
                        new_run(&this.terminal, window, cx);
                    })))
                    .child(own_press(look::icon_button("steps-switch", look::Icon::Panel, self.steps)).on_click(cx.listener(|this, _, _, cx| {
                        this.steps = !this.steps;
                        cx.notify();
                    }))),
            )
    }

    /// The runs as the column lists them, top to bottom.
    fn run_ids(&self, cx: &App) -> Vec<String> {
        let listing = &self.runs.read(cx).snap.listing;
        listing.needs.iter().chain(listing.projects.iter().flat_map(|(_, rows)| rows.iter())).map(|r| r.id.clone()).collect()
    }

    /// The next run in the column (1) or the one before (-1), opened as a click on it opens it.
    fn step_run(&mut self, by: isize, cx: &mut Context<Self>) {
        let ids = self.run_ids(cx);
        if ids.is_empty() {
            return;
        }
        let here = self.runs.read(cx).selected.as_ref().and_then(|s| ids.iter().position(|i| i == s));
        let n = ids.len() as isize;
        let at = match here {
            Some(h) => (h as isize + by).clamp(0, n - 1) as usize,
            None => 0,
        };
        let id = ids[at].clone();
        self.tabs.update(cx, |t, cx| t.show(Some(id), cx));
    }

    /// What the Inbox lists: what needs the person, then what finished unseen.
    fn inbox_items(&self, cx: &App) -> (Vec<inbox::Item>, Vec<inbox::Item>) {
        inbox::gather(&self.agents.read(cx).spaces, &self.runs.read(cx).snap.listing)
    }

    /// Command-O: the next thing in the Inbox after the one open, round the end.
    fn open_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (needs, review) = self.inbox_items(cx);
        let order: Vec<inbox::Key> = needs.iter().chain(review.iter()).map(|i| i.key.clone()).collect();
        let tabs = self.tabs.read(cx);
        let open = match (&tabs.agent, &tabs.open) {
            (Some((a, _)), _) => Some(inbox::Key::Agent(a.clone())),
            (None, Some(r)) => Some(inbox::Key::Run(r.clone())),
            _ => None,
        };
        if let Some(key) = inbox::next(&order, open.as_ref()) {
            let title = needs.iter().chain(review.iter()).find(|i| i.key == key).map(|i| i.title.clone()).unwrap_or_default();
            self.open_item(key, title, window, cx);
        }
    }

    fn open_item(&mut self, key: inbox::Key, title: String, window: &mut Window, cx: &mut Context<Self>) {
        match key {
            inbox::Key::Agent(address) => self.tabs.update(cx, |t, cx| t.show_agent(address, title, cx)),
            inbox::Key::Run(id) => self.open_run_waiting(id, window, cx),
        }
    }

    /// The Inbox in the middle, in the familiar look: what needs the person in every space, then what finished
    /// unseen, each with what it asks or did, where, and what to do; a click opens it.
    fn inbox_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let (needs, review) = self.inbox_items(cx);
        let spaces = self.agents.read(cx).spaces.len();
        let now = agents::now_ms();
        let mut page = div()
            .id("inbox")
            .size_full()
            .overflow_y_scroll()
            .px(px(34.))
            .py(px(26.))
            .font_family(look::UI_FONT)
            .flex()
            .flex_col()
            .child(div().text_size(px(19.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::TEXT)).child("Inbox"))
            .child(
                div()
                    .mt(px(4.))
                    .text_size(px(12.5))
                    .text_color(rgb(look::FAINT))
                    .child(format!("What needs you, across all {spaces} spaces. Agents land here when they ask something or finish.")),
            );
        let button = |id: SharedString, text: &'static str| {
            div()
                .id(id)
                .flex_none()
                .px(px(12.))
                .py(px(6.))
                .rounded(px(7.))
                .border_1()
                .border_color(rgb(look::LINE_2))
                .bg(rgb(look::PANEL_2))
                .text_size(px(12.5))
                .text_color(rgb(look::TEXT))
                .hover(|d| d.bg(rgb(look::SELECTED)))
                .cursor_pointer()
                .child(text)
        };
        for (name, items) in [("Needs you", &needs), ("Finished, to review", &review)] {
            if items.is_empty() {
                continue;
            }
            page = page.child(
                div()
                    .mt(px(22.))
                    .mb(px(6.))
                    .flex()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(look::DIM))
                    .child(name)
                    .child(div().font_weight(FontWeight::NORMAL).text_color(rgb(look::FAINT)).child(items.len().to_string())),
            );
            for item in items.iter() {
                let id = match &item.key {
                    inbox::Key::Agent(a) => format!("inbox-agent-{a}"),
                    inbox::Key::Run(r) => format!("inbox-run-{r}"),
                };
                let (key, title) = (item.key.clone(), item.title.clone());
                let act = match &item.key {
                    inbox::Key::Agent(address) if item.state == agents::State::ToReview => {
                        let (agents, address) = (self.agents.clone(), address.clone());
                        button(SharedString::from(format!("{id}-seen")), "Mark reviewed").on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            agents.update(cx, |a, cx| a.mark_reviewed(&address, cx));
                        })
                    }
                    _ => {
                        let (key, title) = (item.key.clone(), item.title.clone());
                        button(SharedString::from(format!("{id}-open")), "Open").on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.open_item(key.clone(), title.clone(), window, cx);
                        }))
                    }
                };
                let colour = match item.state {
                    agents::State::Waiting => look::AMBER,
                    agents::State::Error => look::RED,
                    _ => look::VIOLET,
                };
                page = page.child(
                    div()
                        .id(SharedString::from(id))
                        .flex()
                        .items_center()
                        .gap(px(14.))
                        .px(px(14.))
                        .py(px(11.))
                        .rounded(px(9.))
                        .hover(|d| d.bg(rgb(look::PANEL_2)))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| this.open_item(key.clone(), title.clone(), window, cx)))
                        .child(div().size(px(8.)).flex_none().rounded_full().bg(rgb(colour)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(6.))
                                        .text_size(px(13.))
                                        .child(div().flex_none().max_w(px(320.)).truncate().font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::TEXT)).child(item.title.clone()))
                                        .child(div().min_w_0().truncate().text_color(rgb(look::DIM)).child(item.what.clone())),
                                )
                                .child(div().text_size(px(12.)).text_color(rgb(look::FAINT)).child(item.place.clone())),
                        )
                        .child(act)
                        .child(
                            div()
                                .w(px(34.))
                                .flex_none()
                                .text_right()
                                .text_size(px(12.))
                                .text_color(rgb(look::FAINT))
                                .child(if item.since_ms > 0 { agents::age(item.since_ms, now) } else { String::new() }),
                        ),
                );
            }
        }
        if needs.is_empty() && review.is_empty() {
            page = page.child(div().mt(px(40.)).text_size(px(15.)).font_weight(FontWeight::MEDIUM).text_color(rgb(look::GREEN)).child("✓ All clear. Nothing needs you."));
        }
        let all = || self.agents.read(cx).spaces.iter().flat_map(|s| s.agents.iter()).cloned().collect::<Vec<_>>();
        let (working, idle) = (all().iter().filter(|a| a.state == agents::State::Working).count(), all().iter().filter(|a| a.state == agents::State::Idle && !a.shell).count());
        if working + idle > 0 {
            let words = [(working, "working"), (idle, "idle")].iter().filter(|(n, _)| *n > 0).map(|(n, w)| format!("{n} {w}")).collect::<Vec<_>>().join(" · ");
            page = page.child(div().mt(px(26.)).text_size(px(12.5)).text_color(rgb(look::FAINT)).child(format!("{words}. They show up here when they need you.")));
        }
        page.into_any_element()
    }

    /// A run that waits, its tab at the step that waits and its question open.
    fn open_run_waiting(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let runs = self.runs.read(cx);
        let place = runs
            .snap
            .details
            .get(&id)
            .and_then(|d| d.stages.iter().find(|s| s.class == "waiting"))
            .map(|s| tabs::Place::Step(s.name.clone()))
            .unwrap_or(tabs::Place::End);
        self.tabs.update(cx, |t, cx| t.show_at(id, place, cx));
        self.island.update(cx, |i, cx| i.open_stop(window, cx));
    }

    /// Command-.: halts the picked run when it works; a run that waits or ended has nothing to halt.
    /// Stop, or Command-. over an agent: herdr-core presses its stop key (Esc for an agent that reports, else
    /// Ctrl-C), never more.
    fn stop_agent(&self, cx: &App) {
        let Some((address, _)) = self.agent_view.clone() else { return };
        let core = self.core.clone();
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = core.call("terminal.interrupt", json!({"terminal": address})) {
                    tracing::warn!("cannot stop {address}: {e:#}");
                }
            })
            .detach();
    }

    fn halt_picked(&self, cx: &App) {
        if let Some(d) = self.runs.read(cx).chosen().filter(|d| d.mark == jaynes_shell::listing::Mark::Working) {
            halt(std::slice::from_ref(&d.id));
        }
    }

    /// The list of shortcuts over the window; a click or Esc puts it away.
    fn shortcuts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut panel = div()
            .id("shortcuts-panel")
            .w(px(440.))
            .p(px(20.))
            .rounded(px(12.))
            .bg(rgb(look::PANEL_2))
            .border_1()
            .border_color(rgb(look::LINE_2))
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::TEXT)).child("Shortcuts"));
        for (group, keys) in SHORTCUTS {
            let mut list = div().flex().flex_col().gap(px(6.)).child(look::label(group, look::FAINT));
            for (key, what) in keys.iter() {
                list = list.child(
                    div()
                        .flex()
                        .gap(px(12.))
                        .child(div().w(px(110.)).flex_none().font_family(look::MONO_FONT).text_size(px(12.5)).text_color(rgb(look::TEXT)).child(*key))
                        .child(div().text_size(px(13.)).text_color(rgb(look::DIM)).child(*what)),
                );
            }
            panel = panel.child(list);
        }
        div()
            .id("shortcuts")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x0000_0099))
            .on_click(cx.listener(|this, _, _, cx| {
                this.help = false;
                cx.notify();
            }))
            .child(panel)
    }

    /// Where the middle works: the open agent's space and folder, else the folder Claude Code works in.
    fn place(&self, cx: &App) -> (Option<String>, String) {
        if let Some((open, _)) = &self.agent_view {
            let spaces = &self.agents.read(cx).spaces;
            if let Some((s, a)) = spaces.iter().flat_map(|s| s.agents.iter().map(move |a| (s, a))).find(|(_, a)| a.address == *open) {
                return (Some(s.name.clone()), a.cwd.clone());
            }
        }
        (None, folder())
    }

    /// The status bar, in the familiar look: where the middle works (the space, its branch, its folder, and Stop
    /// while the agent works), then how many things wait, wait for review and work, each opening the Inbox, and the
    /// list of shortcuts; "Halt all" while Jaynes runs work.
    fn status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let runs = self.runs.read(cx);
        let working_runs: Vec<String> = runs
            .snap
            .listing
            .projects
            .iter()
            .flat_map(|(_, rows)| rows.iter())
            .filter(|r| r.mark == jaynes_shell::listing::Mark::Working)
            .map(|r| r.id.clone())
            .collect();
        let waiting_runs = runs.snap.listing.needs.len();
        let agents = self.agents.read(cx);
        let (waiting, review) = agents.counts();
        let working = agents.spaces.iter().flat_map(|s| &s.agents).filter(|a| a.state == agents::State::Working).count() + working_runs.len();
        let waiting = waiting + waiting_runs;
        let inbox = self.tabs.read(cx).inbox;
        let open_works = self.agent_view.as_ref().is_some_and(|(open, _)| agents.spaces.iter().flat_map(|s| &s.agents).any(|a| a.address == *open && a.state == agents::State::Working));
        let (space, place) = self.place(cx);
        let place = home_short(&place, &std::env::var("HOME").unwrap_or_default());
        let item = || div().flex().items_center().gap(px(6.));
        let count = |id: &'static str| {
            item().id(id).cursor_pointer().hover(|d| d.text_color(rgb(look::TEXT))).on_click(cx.listener(|this, _, _, cx| this.tabs.update(cx, |t, cx| t.show_inbox(cx))))
        };
        let dot = |c: u32| div().size(px(7.)).rounded_full().bg(rgb(c));
        let mut bar = div()
            .h(px(STATUS))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(16.))
            .px(px(12.))
            .border_t_1()
            .border_color(rgb(look::LINE))
            .text_size(px(11.5))
            .text_color(rgb(look::DIM));
        if inbox {
            bar = bar.child(div().font_weight(FontWeight::MEDIUM).text_color(rgb(look::TEXT)).child("Inbox"));
        } else {
            bar = bar
                .when_some(space, |d, s| d.child(div().font_weight(FontWeight::MEDIUM).text_color(rgb(look::TEXT)).child(s)))
                .when(!self.branch.is_empty(), |d| d.child(div().font_family(look::MONO_FONT).text_size(px(11.)).text_color(rgb(look::BLUE)).child(format!("⎇ {}", self.branch))))
                .child(div().min_w_0().truncate().child(place))
                .when(open_works, |d| {
                    d.child(
                        item()
                            .id("stop-agent")
                            .cursor_pointer()
                            .hover(|d| d.text_color(rgb(look::TEXT)))
                            .on_click(cx.listener(|this, _, _, cx| this.stop_agent(cx)))
                            .child("Stop")
                            .child(div().opacity(0.6).child("⌘.")),
                    )
                });
        }
        bar.child(div().flex_1())
            .when(waiting > 0, |d| d.child(count("count-waiting").child(dot(look::AMBER)).child(format!("{waiting} waiting"))))
            .when(review > 0, |d| d.child(count("count-review").child(dot(look::VIOLET)).child(format!("{review} to review"))))
            .when(working > 0, |d| d.child(count("count-working").child(look::spinner("status-spin", 10., look::BLUE)).child(format!("{working} working"))))
            .when(!working_runs.is_empty(), |d| {
                d.child(
                    item()
                        .id("halt-all")
                        .px(px(6.))
                        .py(px(2.))
                        .rounded(px(5.))
                        .text_color(rgb(look::RED))
                        .cursor_pointer()
                        .hover(|d| d.bg(rgba((look::RED << 8) | 0x1f)))
                        .on_click(move |_, _, _| halt(&working_runs))
                        .child(look::icon(look::Icon::Stop, 13., look::RED))
                        .child("Halt all runs"),
                )
            })
            .child(
                item()
                    .id("shortcuts-hint")
                    .cursor_pointer()
                    .hover(|d| d.text_color(rgb(look::TEXT)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.help = true;
                        cx.notify();
                    }))
                    .child("⌘/"),
            )
    }
}

/// How the person answers an agent that asks: one of its numbered choices, or no.
#[derive(Clone, Copy)]
enum Answer {
    Pick(usize),
    Deny,
}

/// A folder under the person's home, from the tilde.
fn home_short(folder: &str, home: &str) -> String {
    match folder.strip_prefix(home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => format!("~{rest}"),
        _ => folder.to_owned(),
    }
}

/// The branch checked out where Claude Code works; empty when it is not a git folder.
fn branch_of(folder: &str) -> String {
    spawn::command("git")
        .args(["-C", folder, "branch", "--show-current"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// "+ New run": asks Claude Code in the terminal for a run through Jaynes, the person finishing the sentence.
fn new_run(terminal: &Entity<TerminalView>, window: &mut Window, cx: &mut App) {
    terminal.update(cx, |t, cx| {
        t.type_text("/jaynes ");
        let focus = t.focus_handle().clone();
        window.focus(&focus, cx);
    });
}

fn copy_into(t: &Entity<TerminalView>) -> impl Fn(&Copy, &mut Window, &mut App) + 'static {
    let t = t.clone();
    move |_, _, cx| {
        t.update(cx, |t, cx| {
            t.copy(cx);
        });
    }
}

fn paste_into(t: &Entity<TerminalView>) -> impl Fn(&Paste, &mut Window, &mut App) + 'static {
    let t = t.clone();
    move |_, _, cx| t.update(cx, |t, cx| t.paste(cx))
}

fn main() {
    #[cfg(target_os = "macos")]
    core_bin::fill_path();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("JAYNES_APP_LOG").unwrap_or_else(|_| "warn,jaynes_app=info".into()),
        )
        .init();
    if let Some(bin) = core_bin::find(None) {
        core_bin::use_core(bin);
    }
    tracing::info!("bundle: {}", if std::env::current_exe().is_ok_and(|e| e.to_string_lossy().contains(".app/Contents/MacOS/")) { "Jaynes.app" } else { "none, a bare build" });
    let core = match hc::Core::connect() {
        Ok(core) => core,
        Err(e) => {
            eprintln!("jaynes-app: cannot reach herdr-core (hc): {e:#}");
            std::process::exit(1);
        }
    };
    if let Err(e) = hc::check(&core.hello) {
        eprintln!("jaynes-app: this herdr-core cannot serve the app: {e:?}");
        std::process::exit(1);
    }

    gpui_platform::application().run(move |cx| {
        let fonts = settings::BUNDLED_FONTS.iter().map(|f| std::borrow::Cow::Borrowed(*f)).collect();
        if let Err(e) = cx.text_system().add_fonts(fonts) {
            tracing::warn!("bundled fonts not loaded: {e:#}");
        }
        settings::note_fonts(cx.text_system().all_font_names());
        gpui_base::init(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-c", Copy, Some("Jaynes")),
            KeyBinding::new("cmd-v", Paste, Some("Jaynes")),
            KeyBinding::new("cmd-e", ToggleSteps, Some("Jaynes")),
            KeyBinding::new("cmd-n", NewRun, Some("Jaynes")),
            KeyBinding::new("cmd-0", ToggleRuns, Some("Jaynes")),
            KeyBinding::new("cmd-1", Tab1, Some("Jaynes")),
            KeyBinding::new("cmd-2", Tab2, Some("Jaynes")),
            KeyBinding::new("cmd-3", Tab3, Some("Jaynes")),
            KeyBinding::new("cmd-4", Tab4, Some("Jaynes")),
            KeyBinding::new("cmd-5", Tab5, Some("Jaynes")),
            KeyBinding::new("cmd-6", Tab6, Some("Jaynes")),
            KeyBinding::new("cmd-7", Tab7, Some("Jaynes")),
            KeyBinding::new("cmd-8", Tab8, Some("Jaynes")),
            KeyBinding::new("cmd-9", Tab9, Some("Jaynes")),
            KeyBinding::new("cmd-}", NextTab, Some("Jaynes")),
            KeyBinding::new("cmd-{", PreviousTab, Some("Jaynes")),
            KeyBinding::new("cmd-alt-down", NextRun, Some("Jaynes")),
            KeyBinding::new("cmd-alt-up", PreviousRun, Some("Jaynes")),
            KeyBinding::new("cmd-o", OpenWaiting, Some("Jaynes")),
            KeyBinding::new("cmd-enter", SendAnswer, Some("Jaynes")),
            KeyBinding::new("cmd-.", HaltRun, Some("Jaynes")),
            KeyBinding::new("cmd-w", CloseTab, Some("Jaynes")),
            KeyBinding::new("cmd-t", NewAgent, Some("Jaynes")),
            KeyBinding::new("cmd-i", ShowInbox, Some("Jaynes")),
            KeyBinding::new("cmd-backspace", Deny, Some("Jaynes")),
            KeyBinding::new("alt-1", Pick1, Some("Jaynes")),
            KeyBinding::new("alt-2", Pick2, Some("Jaynes")),
            KeyBinding::new("alt-3", Pick3, Some("Jaynes")),
            KeyBinding::new("alt-4", Pick4, Some("Jaynes")),
            KeyBinding::new("alt-5", Pick5, Some("Jaynes")),
            KeyBinding::new("alt-6", Pick6, Some("Jaynes")),
            KeyBinding::new("alt-7", Pick7, Some("Jaynes")),
            KeyBinding::new("alt-8", Pick8, Some("Jaynes")),
            KeyBinding::new("alt-9", Pick9, Some("Jaynes")),
            KeyBinding::new("cmd-/", Shortcuts, Some("Jaynes")),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        // the menu bar's menus, where a Mac app shows what it does and its keys
        cx.set_menus(vec![
            Menu { name: "Jaynes".into(), disabled: false, items: vec![MenuItem::action("Shortcuts", Shortcuts), MenuItem::separator(), MenuItem::action("Quit Jaynes", Quit)] },
            Menu { name: "Edit".into(), disabled: false, items: vec![MenuItem::action("Copy", Copy), MenuItem::action("Paste", Paste)] },
            Menu {
                name: "View".into(),
                disabled: false,
                items: vec![
                    MenuItem::action("Runs Column", ToggleRuns),
                    MenuItem::action("Steps Panel", ToggleSteps),
                    MenuItem::separator(),
                    MenuItem::action("Next Tab", NextTab),
                    MenuItem::action("Previous Tab", PreviousTab),
                    MenuItem::action("Close Tab", CloseTab),
                    MenuItem::action("New Agent", NewAgent),
                ],
            },
            Menu {
                name: "Run".into(),
                disabled: false,
                items: vec![
                    MenuItem::action("New Run", NewRun),
                    MenuItem::action("Open What Waits", OpenWaiting),
                    MenuItem::action("Send Answer", SendAnswer),
                    MenuItem::action("Halt", HaltRun),
                    MenuItem::separator(),
                    MenuItem::action("Next Run", NextRun),
                    MenuItem::action("Previous Run", PreviousRun),
                ],
            },
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        // The terminal stays in herdr-core when the window closes; the app has nothing else to show.
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        open(core.clone(), cx);
        // A test that draws the window into a picture leaves the person's keyboard where it is.
        if std::env::var_os("JAYNES_APP_SHOT").is_none() {
            cx.activate(true);
        }
    });
}

fn open(core: Arc<hc::Core>, cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1440.), px(880.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // the window draws its own top bar, the macOS buttons inside it
        titlebar: Some(TitlebarOptions { title: Some("Jaynes".into()), appears_transparent: true, traffic_light_position: Some(point(px(14.), px(14.))) }),
        app_id: Some("jaynes-app".into()),
        focus: std::env::var_os("JAYNES_APP_SHOT").is_none(),
        // the top bar is the app's own: a tab dragged there moves the tab, not the window, which the bar's empty
        // parts move themselves (start_window_move); otherwise AppKit drags the window from the whole titlebar strip
        app_owns_titlebar_drag: true,
        ..Default::default()
    };
    cx.open_window(options, move |window, cx| {
        let address = existing(&core).unwrap_or_else(|| {
            let labels = json!({LABEL.0: LABEL.1});
            core.run(vec!["claude".into()], Some(folder()), labels, None, None, terminal::initial_display(window))
                .unwrap_or_else(|e| {
                    eprintln!("jaynes-app: cannot start claude in a terminal: {e:#}");
                    std::process::exit(1);
                })
        });
        let view = cx.new(|cx| TerminalView::attach(core.clone(), address, (120, 40), cx));
        let focus = view.read(cx).focus_handle().clone();
        window.focus(&focus, cx);
        let runs = cx.new(model::Runs::new);
        let agents = cx.new(|cx| agents::Agents::new(core.clone(), cx));
        let tabs = cx.new(|cx| tabs::Tabs::new(runs.clone(), agents.clone(), cx));
        let side = cx.new(|cx| side::Side::new(runs.clone(), agents.clone(), tabs.clone(), cx));
        let key = the_key();
        let lightbox = cx.new(lightbox::Lightbox::new);
        let compare = cx.new(|cx| compare::Compare::new(key.clone(), cx));
        let island = cx.new(|cx| island::Island::new(runs.clone(), key, lightbox.clone(), compare.clone(), cx));
        island.update(cx, |i, _| i.tabs = Some(tabs.clone()));
        let rail = cx.new(|cx| rail::Rail::new(runs.clone(), tabs.clone(), island.clone(), cx));
        let terminal_focus = focus.clone();
        cx.new(|cx| {
            cx.observe(&runs, |_, _, cx| cx.notify()).detach();
            // the keys go where the person looks: the terminal on Claude Code's tab, else the window itself, or a
            // shortcut pressed over a run's story would reach nothing, the terminal being off the screen
            cx.observe_in(&tabs, window, move |this: &mut Root, tabs, window, cx| {
                let (open, agent, inbox) = (tabs.read(cx).open.clone(), tabs.read(cx).agent.as_ref().map(|(a, _)| a.clone()), tabs.read(cx).inbox);
                let was_agent = this.agent_view.as_ref().map(|(a, _)| a.clone());
                if open != this.was_open || agent != was_agent || inbox != this.was_inbox {
                    this.was_open = open.clone();
                    this.was_inbox = inbox;
                    this.follow_agent(agent, cx);
                    let to = match (&this.agent_view, &open) {
                        _ if inbox => this.focus.clone(),
                        (Some((_, v)), _) => v.read(cx).focus_handle().clone(),
                        (None, Some(_)) => this.focus.clone(),
                        (None, None) => terminal_focus.clone(),
                    };
                    window.focus(&to, cx);
                }
                cx.notify();
            })
            .detach();
            // the branch, read again every few seconds: the person may switch it in the terminal
            let branch = cx.spawn(async move |this, cx| loop {
                let Ok(at) = this.update(cx, |root: &mut Root, cx| root.place(cx).1) else { break };
                let now = cx.background_executor().spawn(async move { branch_of(&at) }).await;
                let gone = this
                    .update(cx, |root: &mut Root, cx| {
                        if root.branch != now {
                            root.branch = now;
                            cx.notify();
                        }
                    })
                    .is_err();
                if gone {
                    break;
                }
                cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
            });
            // JAYNES_APP_NOSIDE: a test's picture of the window without its runs column
            Root { runs, tabs, side, terminal: view, core: core.clone(), agents, agent_view: None, rail, island, compare, lightbox, steps: true, sidebar: std::env::var_os("JAYNES_APP_NOSIDE").is_none(), branch: String::new(), help: false, focus: cx.focus_handle(), was_open: None, was_inbox: false, _branch: branch }
        })
    })
    .map(|handle| {
        if let Some(path) = std::env::var_os("JAYNES_APP_SHOT") {
            shots(handle.into(), path.into(), cx);
        }
    })
    .expect("cannot open the Jaynes window");
}

/// The person's own key, paired with the engine; None when either fails, and the app then answers nothing.
fn the_key() -> Option<Arc<key::Key>> {
    // a test's key is paired only with a test's engine, never with the person's own
    if std::env::var_os("JAYNES_APP_TEST_KEY").is_some() && std::env::var_os("JAYNES_ENGINE_HOME").is_none() {
        tracing::warn!("a test key pairs only with an engine named in JAYNES_ENGINE_HOME");
        return None;
    }
    let key = key::Key::load().map_err(|e| tracing::warn!("no key of the person's: {e}")).ok()?;
    key.pair(&jaynes_shell::home()).map_err(|e| tracing::warn!("the key is not paired: {e}")).ok()?;
    Some(Arc::new(key))
}

/// With JAYNES_APP_SHOT=<file>, the window draws itself into that PNG every second, even when it is covered: how a
/// test (or an agent) sees what the person would see without taking their screen.
fn shots(window: AnyWindowHandle, path: std::path::PathBuf, cx: &mut App) {
    cx.spawn(async move |cx| {
        // The app quits with its window, so the loop ends with it.
        loop {
            cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
            cx.update(terminal::flush_output_clock);
            let shot = window.render_current_to_image(cx).and_then(|image| {
                image.save_with_format(&path, image::ImageFormat::Png)?;
                Ok(())
            });
            if let Err(e) = shot {
                tracing::warn!("cannot save the window's picture: {e:#}");
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::home_short;

    #[test]
    fn a_folder_under_home_is_shown_from_the_tilde() {
        assert_eq!(home_short("/Users/a/Projects/jaynes", "/Users/a"), "~/Projects/jaynes");
        assert_eq!(home_short("/Users/a", "/Users/a"), "~");
        assert_eq!(home_short("/Users/ab/x", "/Users/a"), "/Users/ab/x");
        assert_eq!(home_short("/tmp/x", ""), "/tmp/x");
    }
}
