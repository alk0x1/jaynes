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
mod app_state;
mod bars;
mod ask;
mod inbox;
mod answer;
mod bench;
mod changes;
mod compare;
mod core_bin;
mod diff;
mod git;
mod hc;
mod island;
mod key;
mod layout;
mod lightbox;
mod links;
mod look;
mod model;
mod menubar;
mod news;
mod notify;
mod prefs;
mod settings_window;
mod testing;
mod looking_at;
mod run_actions;
mod palette;
mod run_waits;
mod trail;
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
        NextRun, PreviousRun, OpenWaiting, SendAnswer, HaltRun, CloseTab, Shortcuts, NewAgent, Deny, Pick1, Pick2, Pick3, Pick4, Pick5, Pick6, Pick7, Pick8, Pick9, ShowInbox, ToggleChanges, CommitBox, OpenEditor, SplitRight, SplitDown, NextPane, PreviousPane, OpenSettings, CopySummary, OpenPalette
    ]
);

/// Every shortcut, as the list over the window (Command-/) shows it.
const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    ("Tabs", &[("⌘1 … ⌘8", "The tab at that place"), ("⌘9", "The last tab"), ("⇧⌘]  ⇧⌘[", "Next tab, previous tab"), ("⌘W", "Close a run's tab"), ("⌘T", "New agent in this space")]),
    ("Runs", &[("⌥⌘↓  ⌥⌘↑", "Next run, previous run"),  ("⌘↩", "Send your answer"), ("⌘.", "Stop the open agent, or halt the picked run"), ("⌘N", "New run")]),
    ("Inbox", &[("⌘I", "The Inbox: what needs you, in every space"), ("⌘O", "The next thing that needs you")]),
    ("Agents", &[("⌘↩", "Allow, the first choice"), ("⌘⌫", "Deny"), ("⌥1 … ⌥9", "Pick that choice")]),
    ("Panes", &[("⌘D", "Split right: your shell beside the agent"), ("⇧⌘D", "Split down"), ("⌘]  ⌘[", "Next pane, previous pane")]),
    ("Window", &[("⌘0", "Runs column"), ("⌘E", "Steps panel"), ("⇧⌘G", "Changes panel"), ("⌘K", "Go to any agent, run or command"), ("⌘/", "This list")]),
];

/// The top bar's height; the macOS window buttons sit inside it, in the runs column's header.
const TOP: f32 = 42.;
/// The status bar's height, at the bottom of the window.
const STATUS: f32 = 28.;

/// The label the app puts on its terminal, so it finds it again after the window closes.
const LABEL: (&str, &str) = ("jaynes.app", "main");

/// The live terminal the app made earlier, if its program still runs.
fn existing(core: &hc::Core) -> Option<String> {
    existing_or_why(core).ok().flatten()
}

/// The app's own terminal, if it lives; why herdr-core could not say.
fn existing_or_why(core: &hc::Core) -> anyhow::Result<Option<String>> {
    let listed = core.call("terminal.list", json!({"labels": {LABEL.0: LABEL.1}}))?;
    let terminals = listed.get("terminals").and_then(Value::as_array).cloned().unwrap_or_default();
    let terminals = terminals
        .iter()
        .find(|t| t.get("state").and_then(Value::as_str) == Some("live"))
        .and_then(|t| t.get("address").and_then(Value::as_str).map(str::to_owned));
    Ok(terminals)
}

/// The folder Claude Code starts in: the one named on the command line, else the one the app was started from.
fn folder() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    // opened from Finder or the Dock, the app starts in "/": the person's home is where they work
    let here = std::env::current_dir().ok().map(|d| d.to_string_lossy().into_owned()).filter(|d| d != "/");
    std::env::args().nth(1).filter(|a| !a.starts_with("-psn")).or(here).unwrap_or(home)
}

pub(crate) struct Root {
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
    /// Every pane of the open agent's tab, attached while it shows; the open agent is the one with the keyboard.
    panes: Vec<(String, Entity<TerminalView>)>,
    /// The terminal shown before a switch, kept over the agent just opened until the core paints its screen, so the
    /// switch never flashes an empty terminal.
    cover: Option<Cover>,
    rail: Entity<rail::Rail>,
    /// What git says about the folder the middle works in, at the right (Shift-Command-G).
    changes: Entity<changes::Changes>,
    changes_on: bool,
    /// The Changes tab's stacked files, in the middle.
    stack: Entity<changes::Stack>,
    /// Whether the Changes tab showed when the focus last followed the tabs.
    was_changes: bool,
    /// The person's editor open from the Changes tab: its terminal, and where to go back when it ends (the agent that
    /// was open, the files stacked).
    editing: Option<(String, Option<(String, String)>, Vec<String>)>,
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
    /// The window is hidden and its terminals let go, to be taken up again when it shows.
    let_go: bool,
    /// A control of the open run pressed once that asks to be pressed again (Abandon), and what the last one said.
    armed: Option<(String, run_actions::Act)>,
    act_said: Option<String>,
    /// Command-K, while it shows.
    palette: Option<(Entity<palette::PaletteView>, Subscription)>,
    /// The runs' questions put off with Later on the line over an agent ("run/question").
    later: std::collections::HashSet<String>,
    /// What the window showed when the app last stood, to show again once the agents and the runs are read.
    restore: Option<app_state::State>,
    /// What was kept last (app_state.rs); none until what stood is shown again, so the first frames keep nothing.
    kept: Option<app_state::State>,
    _bounds: Option<Subscription>,
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        terminal::next_frame();
        self.remember(window, cx);
        let terminal = self.terminal.clone();
        let shown = self.shown_terminal();
        let middle = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.tabs_bar(cx))
            .children(self.core_cannot_save(cx))
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
                            .when(self.tabs.read(cx).changes.is_none(), |d| d.children(self.ask_bar(cx)))
                            // a run that waits, said over the agent open: it tells, it does not cover the agent
                            .when(self.agent_view.is_some() && self.tabs.read(cx).changes.is_none() && !self.tabs.read(cx).inbox, |d| d.children(self.run_waits_line(cx)))
                            .child(div().flex_1().min_h_0().child(match (&self.agent_view, self.tabs.read(cx).body(cx)) {
                                _ if self.tabs.read(cx).changes.is_some() => self.stack.clone().into_any_element(),
                                _ if self.tabs.read(cx).inbox => self.inbox_view(cx),
                                (Some(_), _) => {
                                    let body = self.agent_body(cx);
                                    self.covered(body, cx)
                                }
                                (None, Some(story)) => story,
                                (None, None) => self.terminal.clone().into_any_element(),
                            }))
                            // a run's own controls, under its story: Resume, Continue, Raise, Abandon
                            .when(self.agent_view.is_none() && !self.tabs.read(cx).inbox && self.tabs.read(cx).changes.is_none(), |d| d.children(self.run_controls(cx)))
                            // a run's question, over Claude Code's terminal and the runs' tabs, not over an agent or the Inbox
                            .when(self.agent_view.is_none() && !self.tabs.read(cx).inbox && self.tabs.read(cx).changes.is_none(), |d| d.child(self.island.clone())),
                    )
                    // at the right, Changes when it is on, else the run's steps
                    .when(self.changes_on, |d| d.child(self.changes.clone()))
                    .when(!self.changes_on && self.steps, |d| d.child(self.rail.clone())),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(look::panel()))
            .font_family(look::UI_FONT)
            .track_focus(&self.focus)
            .key_context("Jaynes")
            // a hand on the window, for the person's presence at a stop: before any pane takes it
            .capture_key_down(|_, _, _| looking_at::touched())
            .capture_any_mouse_down(|_, _, _| looking_at::touched())
            .child(canvas(|_, _, _| {}, |_, _, window, _| {
                window.on_mouse_event(|_: &MouseMoveEvent, phase, _, _| {
                    if phase == DispatchPhase::Capture {
                        looking_at::touched();
                    }
                });
                window.on_mouse_event(|_: &ScrollWheelEvent, phase, _, _| {
                    if phase == DispatchPhase::Capture {
                        looking_at::touched();
                    }
                });
            }).absolute().size_0())
            .on_action(copy_into(&shown))
            .on_action(paste_into(&shown))
            .on_action(cx.listener(|this, _: &ToggleSteps, _, cx| {
                this.steps = !this.steps;
                cx.notify();
            }))
            .on_action(move |_: &NewRun, window, cx| new_run(&terminal, window, cx))
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| this.open_palette(window, cx)))
            // the button under a finished run's story
            .on_action(cx.listener(|this, _: &CopySummary, _, cx| this.tabs.update(cx, |t, cx| t.copy_summary(cx))))
            .on_action(cx.listener(|this, _: &ToggleChanges, _, cx| {
                this.changes_on = !this.changes_on;
                cx.notify();
            }))
            // c in the Changes tab: the panel shows, its commit message takes the keys
            .on_action(cx.listener(|this, _: &CommitBox, window, cx| {
                this.changes_on = true;
                this.changes.update(cx, |c, cx| c.focus_message(window, cx));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenEditor, window, cx| this.open_editor(window, cx)))
            .on_action(cx.listener(|this, _: &SplitRight, window, cx| this.split_pane(layout::Dir::Row, window, cx)))
            .on_action(cx.listener(|this, _: &SplitDown, window, cx| this.split_pane(layout::Dir::Column, window, cx)))
            .on_action(cx.listener(|this, _: &NextPane, _, cx| this.step_pane(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousPane, _, cx| this.step_pane(-1, cx)))
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
            .on_action(cx.listener(|this, _: &OpenWaiting, window, cx| {
                // the run the line over the agent tells of, when it shows: what its Open says
                match this.run_waits(cx) {
                    Some(w) if this.agent_view.is_some() => this.tabs.update(cx, |t, cx| t.show(Some(w.run), cx)),
                    _ => this.open_next(window, cx),
                }
            }))
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
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                if !this.tabs.update(cx, |t, cx| t.close(cx)) {
                    this.let_go(cx);
                    hide_main(window);
                }
            }))
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
                                .border_color(rgb(look::line()))
                                .bg(rgb(look::side()))
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
            .when_some(self.palette.as_ref().map(|(p, _)| p.clone()), |d, p| d.child(p))
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
        .border_color(rgb(look::line()))
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

struct Cover {
    was: Shown,
    _painted: Subscription,
    _deadline: Task<()>,
}

/// What the middle showed before a switch to an agent.
#[derive(Clone)]
enum Shown {
    Terminal(Entity<TerminalView>),
    /// An agent's tab split in panes: the pane with the keyboard, the layout and every pane's view.
    Split(String, layout::Layout, Vec<(String, Entity<TerminalView>)>),
    Inbox,
    Changes,
}

/// The longest an agent's old screen stays over the next one: an attach that slow shows the empty terminal after all.
const COVER_AT_MOST: std::time::Duration = std::time::Duration::from_millis(700);

impl Root {
    /// The run the line over an open agent tells of, if one waits (run_waits.rs).
    fn run_waits(&self, cx: &App) -> Option<run_waits::Waiting> {
        let waits = run_waits::waiting(&self.runs.read(cx).snap);
        run_waits::line(&waits, &self.later, self.open_ask(cx).is_some())
    }

    /// "◆ A run waits · <run> · <question>", Open (Command-O) and Later, over the open agent's terminal.
    fn run_waits_line(&self, cx: &mut Context<Self>) -> Option<Div> {
        let w = self.run_waits(cx)?;
        let (run, key) = (w.run.clone(), run_waits::key(&w));
        let what = if w.ahead { format!("{} · {} · before something that cannot be undone", w.title, w.about) } else { format!("{} · {}", w.title, w.about) };
        Some(
            div()
                .flex_none()
                .mx(px(10.))
                .mt(px(8.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(8.))
                .border_1()
                .border_color(if w.ahead { rgb(look::amber()) } else { rgba((look::amber() << 8) | 0x66) })
                .bg(rgb(look::panel_2()))
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(12.5))
                .child(div().flex_none().text_color(rgb(look::amber())).font_weight(FontWeight::SEMIBOLD).child("◆ A run waits"))
                .child(div().flex_1().min_w_0().truncate().text_color(rgb(look::text())).child(what))
                .child(
                    div()
                        .id("run-waits-open")
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(9.))
                        .py(px(3.))
                        .rounded(px(6.))
                        .bg(rgb(look::blue()))
                        .text_color(rgb(look::on_fill()))
                        .cursor_pointer()
                        .child("Open")
                        .child(div().text_size(px(10.5)).child("⌘O"))
                        .on_click(cx.listener(move |this, _, _, cx| this.tabs.update(cx, |t, cx| t.show(Some(run.clone()), cx)))),
                )
                .child(
                    div()
                        .id("run-waits-later")
                        .flex_none()
                        .px(px(9.))
                        .py(px(3.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(rgb(look::line_2()))
                        .text_color(rgb(look::dim()))
                        .cursor_pointer()
                        .child("Later")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.later.insert(key.clone());
                            cx.notify();
                        })),
                ),
        )
    }

    /// Later on the line over the agent, as a click on it: for a test.
    #[cfg(feature = "test-driver")]
    pub(crate) fn put_off(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.run_waits(cx) {
            self.later.insert(run_waits::key(&w));
            cx.notify();
        }
    }

    /// The line under the tabs while herdr-core cannot save its terminals: what to do, said once.
    fn core_cannot_save(&self, cx: &App) -> Option<Div> {
        let trouble = self.agents.read(cx).trouble.clone()?;
        (trouble.contains("registry writer stopped") || trouble.contains("cannot save terminals")).then(|| {
            div()
                .flex_none()
                .px(px(14.))
                .py(px(6.))
                .border_b_1()
                .border_color(rgb(look::line()))
                .bg(rgb(look::panel_2()))
                .text_size(px(12.5))
                .text_color(rgb(look::amber()))
                .child(hc::CANNOT_SAVE)
        })
    }

    /// Command-K: the palette over the window, its list read now.
    pub(crate) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let here = self.agent_view.as_ref().and_then(|(a, _)| self.agents.read(cx).spaces.iter().find(|s| s.agents.iter().any(|x| x.address == *a)).map(|s| s.name.clone()));
        let all = palette::entries(&self.agents.read(cx).spaces, &self.runs.read(cx).snap.listing, here.as_deref());
        let view = cx.new(|cx| palette::PaletteView::new(all, window, cx));
        let said = cx.subscribe_in(&view, window, |this, _, said: &palette::Said, window, cx| {
            this.palette = None;
            if let palette::Said::Go(go) = said {
                this.go_to(go.clone(), window, cx);
            } else {
                // back where the keys were
                let to = this.agent_view.as_ref().map(|(_, v)| v.read(cx).focus_handle().clone()).unwrap_or_else(|| this.terminal.read(cx).focus_handle().clone());
                window.focus(&to, cx);
            }
            cx.notify();
        });
        self.palette = Some((view, said));
        cx.notify();
    }

    /// Where an entry of Command-K goes: as a click in the column, or the command's own key.
    fn go_to(&mut self, go: palette::Go, window: &mut Window, cx: &mut Context<Self>) {
        use palette::Go;
        match go {
            Go::Inbox => self.tabs.update(cx, |t, cx| t.show_inbox(cx)),
            Go::Agent(address) => {
                let title = self.agents.read(cx).spaces.iter().flat_map(|s| &s.agents).find(|a| a.address == address).map(|a| a.title.clone()).unwrap_or_else(|| address.clone());
                self.tabs.update(cx, |t, cx| t.show_agent(address, title, cx));
            }
            Go::Run(id) => self.tabs.update(cx, |t, cx| t.show(Some(id), cx)),
            Go::NewAgent => window.dispatch_action(Box::new(NewAgent), cx),
            Go::OpenWaiting => window.dispatch_action(Box::new(OpenWaiting), cx),
            Go::ToggleChanges => window.dispatch_action(Box::new(ToggleChanges), cx),
            Go::Shortcuts => window.dispatch_action(Box::new(Shortcuts), cx),
        }
    }

    /// Command-K's view, while it shows: for a test.
    #[cfg(feature = "test-driver")]
    pub(crate) fn palette(&self) -> Option<Entity<palette::PaletteView>> {
        self.palette.as_ref().map(|(p, _)| p.clone())
    }

    /// The open run's controls, as the page shows them: each only when it can do something.
    fn run_controls(&self, cx: &mut Context<Self>) -> Option<Div> {
        let run = self.tabs.read(cx).open.clone()?;
        let acts = run_actions::acts(&self.tabs.read(cx).facts?);
        if acts.is_empty() {
            return None;
        }
        let mut bar = div().flex().items_center().gap(px(8.)).mx(px(14.)).mb(px(8.)).flex_wrap();
        for act in acts {
            let armed = self.armed.as_ref() == Some(&(run.clone(), act));
            let label = if armed { format!("{}: press again", act.label()) } else { act.label().to_string() };
            let danger = act.asks_twice();
            let run = run.clone();
            bar = bar.child(
                div()
                    .id(SharedString::from(format!("act-{}", act.word())))
                    .px(px(10.))
                    .py(px(5.))
                    .rounded(px(7.))
                    .border_1()
                    .border_color(rgb(if danger { look::red() } else { look::line_2() }))
                    .bg(rgb(look::panel_2()))
                    .text_size(px(12.5))
                    .text_color(rgb(if danger { look::red() } else { look::text() }))
                    .cursor_pointer()
                    .hover(|d| d.border_color(rgb(look::dim())))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.press(run.clone(), act, cx))),
            );
        }
        Some(bar.when_some(self.act_said.clone(), |d, said| d.child(div().text_size(px(12.)).text_color(rgb(look::red())).child(said))))
    }

    /// A press on a run's control: one that asks twice is armed first; then it is sent as the person's.
    pub(crate) fn press(&mut self, run: String, act: run_actions::Act, cx: &mut Context<Self>) {
        self.act_said = None;
        if act.asks_twice() && self.armed.as_ref() != Some(&(run.clone(), act)) {
            self.armed = Some((run, act));
            cx.notify();
            return;
        }
        self.armed = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let sent = cx.background_executor().spawn(async move { run_actions::send(&run, act) }).await;
            let _ = this.update(cx, |this, cx| {
                if let Err(e) = sent {
                    tracing::warn!("{} was not sent: {e}", act.word());
                    this.act_said = Some(e);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The stop on the screen, for the person's presence: the compare mode's, else the island's while the middle
    /// shows it (an agent, the Inbox or the Changes tab over it hide it).
    fn stop_on_screen(&self, cx: &App) -> Option<(String, String)> {
        if let Some(s) = self.compare.read(cx).stop_shown() {
            return Some(s);
        }
        let tabs = self.tabs.read(cx);
        (self.agent_view.is_none() && !tabs.inbox && tabs.changes.is_none()).then(|| self.island.read(cx).stop_shown(cx)).flatten()
    }

    /// Once the agents and the runs are read, shows again what the window showed when the app last stood: the agent,
    /// the run or the Inbox, if still there (else Claude Code, the log saying why), and the tabs closed then. It only
    /// shows: nothing is started, sent or answered. What the person opened meanwhile stays.
    fn show_what_stood(&mut self, cx: &mut Context<Self>) {
        let runs = self.runs.read(cx);
        if self.restore.is_none() || !self.agents.read(cx).read_once || !(runs.read_once || runs.trouble.is_some()) {
            return;
        }
        let Some(state) = self.restore.take() else { return };
        let listing = &runs.snap.listing;
        let run_there = |id: &str| listing.needs.iter().chain(listing.projects.iter().flat_map(|(_, rows)| rows.iter())).any(|r| r.id == id);
        let spaces = &self.agents.read(cx).spaces;
        let agent_title = |address: &str| spaces.iter().flat_map(|s| &s.agents).find(|a| a.address == address).map(|a| a.title.clone());
        let (open, why) = app_state::open_on(&state.open, |a| agent_title(a).is_some(), run_there);
        if let Some(why) = why {
            tracing::warn!("{why}");
        }
        let title = match &open {
            app_state::Open::Agent { address } => agent_title(address),
            _ => None,
        };
        let t = self.tabs.read(cx);
        let untouched = t.open.is_none() && t.agent.is_none() && !t.inbox && t.changes.is_none();
        self.tabs.update(cx, |t, cx| {
            t.close_kept(&state.closed, cx);
            if untouched {
                match open {
                    app_state::Open::Main => {}
                    app_state::Open::Agent { address } => t.show_agent(address.clone(), title.unwrap_or(address), cx),
                    app_state::Open::Run { id } => t.show(Some(id), cx),
                    app_state::Open::Inbox => t.show_inbox(cx),
                }
            }
        });
        // kept from here on, as the person changes it
        self.kept = Some(state);
    }

    /// What the window shows now, as app_state keeps it.
    fn what_stands(&self, window: &Window, cx: &App) -> app_state::State {
        let t = self.tabs.read(cx);
        let open = match (&t.agent, t.inbox, &t.open) {
            (Some((address, _)), _, _) => app_state::Open::Agent { address: address.clone() },
            (None, true, _) => app_state::Open::Inbox,
            (None, false, Some(id)) => app_state::Open::Run { id: id.clone() },
            (None, false, None) => app_state::Open::Main,
        };
        let frame = Some(app_state::frame_of(window.window_bounds().get_bounds(), window.display(cx).map(|d| d.id().into())));
        // the bars' widths join when their edges can be dragged (bars.rs): until then each is its own
        app_state::State { open, column: self.sidebar, steps: self.steps, changes: self.changes_on, closed: t.closed_kept(cx), frame, widths: app_state::Widths::default() }
    }

    /// Keeps what the window shows when it changed, once what stood was shown again.
    fn remember(&mut self, window: &Window, cx: &App) {
        let Some(kept) = &self.kept else { return };
        let now = self.what_stands(window, cx);
        if now == *kept {
            return;
        }
        if let Err(e) = app_state::save(&app_state::path(), &now) {
            tracing::warn!("what the window shows was not kept: {e}");
        }
        self.kept = Some(now);
    }

    /// Lets go of every terminal the window shows, as it hides: a hidden app holds no terminal's size, and the
    /// person's other viewers (Herdr) size them as they use them.
    pub(crate) fn let_go(&mut self, cx: &App) {
        for (_, v) in &self.panes {
            v.read(cx).detach();
        }
        self.terminal.read(cx).detach();
        self.let_go = true;
    }

    /// Takes the terminals up again, as the window shows.
    fn take_up(&mut self, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.let_go) {
            return;
        }
        let core = self.core.clone();
        for v in self.panes.iter().map(|(_, v)| v.clone()).chain([self.terminal.clone()]) {
            v.update(cx, |v, cx| v.reattach(core.clone(), cx));
        }
        cx.notify();
    }

    /// What the middle shows now, if it can stay over an agent while it opens (a run's story cannot).
    fn middle_shown(&self, cx: &App) -> Option<Shown> {
        if self.was_changes {
            return Some(Shown::Changes);
        }
        if self.was_inbox {
            return Some(Shown::Inbox);
        }
        match &self.agent_view {
            Some((open, v)) => Some(match self.open_layout(cx) {
                Some(l) => Shown::Split(open.clone(), l, self.panes.clone()),
                None => Shown::Terminal(v.clone()),
            }),
            None => self.was_open.is_none().then(|| Shown::Terminal(self.terminal.clone())),
        }
    }

    /// After a switch to an agent whose screen has not come in yet, keeps `before` over it until it does.
    fn cover_until_painted(&mut self, before: Option<Shown>, cx: &mut Context<Self>) {
        self.cover = None;
        let Some((_, new)) = self.agent_view.clone() else { return };
        let Some(was) = before.filter(|was| match was {
            Shown::Terminal(old) => *old != new && !self.panes.iter().any(|(_, v)| v == old),
            // another pane of the same tab: the tab stays as it is
            Shown::Split(_, _, panes) => !panes.iter().any(|(_, v)| *v == new),
            _ => true,
        }) else {
            return;
        };
        if new.read(cx).has_screen() || self.was_inbox || self.was_changes {
            return;
        }
        let painted = cx.observe(&new, |this: &mut Root, new, cx| {
            if new.read(cx).has_screen() && this.cover.is_some() {
                this.cover = None;
                cx.notify();
            }
        });
        let deadline = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COVER_AT_MOST).await;
            let _ = this.update(cx, |this, cx| {
                this.cover = None;
                cx.notify();
            });
        });
        self.cover = Some(Cover { was, _painted: painted, _deadline: deadline });
    }

    /// The agent's body, under the screen shown before while the agent's own has not come in.
    fn covered(&self, body: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let Some(cover) = &self.cover else { return body };
        let was = match cover.was.clone() {
            Shown::Terminal(view) => view.into_any_element(),
            Shown::Split(open, l, panes) => self.layout_element(&l, &open, &panes, true, cx).into_any_element(),
            Shown::Inbox => self.inbox_view(cx),
            Shown::Changes => self.stack.clone().into_any_element(),
        };
        div().relative().size_full().child(body).child(div().absolute().inset_0().bg(rgb(look::panel())).child(was)).into_any_element()
    }

    /// The terminal in the middle: the agent open from the column, else the person's own Claude Code.
    fn shown_terminal(&self) -> Entity<TerminalView> {
        self.agent_view.as_ref().map(|(_, v)| v.clone()).unwrap_or_else(|| self.terminal.clone())
    }

    /// Attaches the agent the tabs show and the other panes of its tab, letting go of those no longer shown; the
    /// person looks at the agent from now on, and it has the keyboard.
    fn follow_agent(&mut self, address: Option<String>, cx: &mut Context<Self>) {
        let same = self.agent_view.as_ref().map(|(a, _)| a) == address.as_ref();
        self.sync_panes(address.as_deref(), cx);
        self.agent_view = address.as_ref().and_then(|a| self.panes.iter().find(|(p, _)| p == a).cloned());
        if !same {
            self.agents.update(cx, |agents, cx| agents.open(address, cx));
        }
    }

    /// The open agent's tab as Herdr splits it; None when it is alone, or in no tab of Herdr's.
    fn open_layout(&self, cx: &App) -> Option<layout::Layout> {
        let (open, _) = self.agent_view.as_ref()?;
        agents::tab_of(&self.agents.read(cx).spaces, open).map(|(_, l)| l.clone()).filter(|l| l.panes().len() > 1)
    }

    /// Keeps a view attached for every pane of the open agent's tab, and only those: herdr-core sizes a terminal for
    /// the viewer used last, so a pane that does not show here is let go.
    fn sync_panes(&mut self, open: Option<&str>, cx: &mut Context<Self>) {
        let wanted: Vec<String> = match open {
            Some(a) => agents::tab_of(&self.agents.read(cx).spaces, a).map(|(_, l)| l.panes()).filter(|p| p.iter().any(|p| p == a)).unwrap_or_else(|| vec![a.to_owned()]),
            None => vec![],
        };
        let (keep, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut self.panes).into_iter().partition(|(a, _)| wanted.contains(a));
        for (_, v) in gone {
            v.read(cx).detach();
        }
        self.panes = keep;
        for a in wanted {
            if !self.panes.iter().any(|(p, _)| *p == a) {
                let core = self.core.clone();
                let view = cx.new(|cx| TerminalView::attach(core, a.clone(), (120, 40), cx));
                self.panes.push((a, view));
            }
        }
    }

    /// The open agent in the middle: its terminal alone, or every pane of its tab as Herdr splits it, the one with the
    /// keyboard ringed in blue; a click on a pane gives it the keyboard.
    fn agent_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some((open, view)) = self.agent_view.clone() else { return div().into_any_element() };
        let Some(l) = self.open_layout(cx) else { return view.into_any_element() };
        self.layout_element(&l, &open, &self.panes, true, cx).into_any_element()
    }

    /// A tab's panes as `l` lays them out, each drawn by its view among `panes`; `top` for the whole tab.
    fn layout_element(&self, l: &layout::Layout, open: &str, panes: &[(String, Entity<TerminalView>)], top: bool, cx: &mut Context<Self>) -> Div {
        match l {
            layout::Layout::Pane(a) => {
                let view = panes.iter().find(|(p, _)| p == a).map(|(_, v)| v.clone());
                let title = self.agents.read(cx).spaces.iter().flat_map(|s| &s.agents).find(|x| x.address == *a).map(|x| x.title.clone()).unwrap_or_else(|| a.clone());
                let (address, on) = (a.clone(), a == open);
                div()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgb(if on { look::blue() } else { look::line_2() }))
                    .overflow_hidden()
                    .capture_any_mouse_down(cx.listener(move |this, _, _, cx| {
                        if this.agent_view.as_ref().map(|(o, _)| o) != Some(&address) {
                            let (address, title) = (address.clone(), title.clone());
                            this.tabs.update(cx, |t, cx| t.show_agent(address, title, cx));
                        }
                    }))
                    .children(view)
            }
            layout::Layout::Split(dir, kids) => {
                let total: f32 = kids.iter().map(|(w, _)| *w).sum::<f32>().max(0.0001);
                let mut d = div().size_full().min_w_0().min_h_0().flex().gap(px(terminal::PANE_GAP));
                if *dir == layout::Dir::Column {
                    d = d.flex_col();
                }
                for (w, k) in kids {
                    d = d.child(div().flex_basis(px(0.)).flex_grow(w / total).flex_shrink(1.).min_w_0().min_h_0().flex().child(self.layout_element(k, open, panes, false, cx)));
                }
                d.p(px(if top { terminal::PANE_GAP } else { 0. }))
            }
        }
    }

    /// Command-D (right) or Shift-Command-D (down): the person's shell split beside the open agent, in its folder, in
    /// its tab in Herdr too; it takes the keyboard.
    fn split_pane(&mut self, dir: layout::Dir, window: &mut Window, cx: &mut Context<Self>) {
        let Some((open, view)) = self.agent_view.clone() else { return };
        let size = view.read(cx).split_size(dir == layout::Dir::Column, self.open_layout(cx).is_none());
        let agents = self.agents.read(cx);
        let Some(space) = agents.spaces.iter().find(|s| s.agents.iter().any(|a| a.address == open)) else { return };
        let Some((tab, _)) = agents::tab_of(&agents.spaces, &open) else { return };
        let (space, tab) = (space.id.clone(), tab.clone());
        let folder = agents.spaces.iter().flat_map(|s| &s.agents).find(|a| a.address == open).map(|a| a.cwd.clone()).filter(|c| !c.is_empty());
        let (core, tabs, display, agents) = (self.core.clone(), self.tabs.clone(), terminal::initial_display(window), self.agents.clone());
        cx.spawn(async move |_, cx| {
            // the tab as split, read before the shell shows: shown alone first, it would fill the room, then shrink
            let split = cx.background_executor().spawn(async move { agents::split_beside(&core, &space, &tab, &open, folder, dir, size, display).map(|a| (a, agents::read_spaces(&core))) }).await;
            match split {
                Ok((address, spaces)) => {
                    if let Ok(spaces) = spaces {
                        let _ = agents.update(cx, |a, cx| a.read_now(spaces, cx));
                    }
                    let _ = tabs.update(cx, |t, cx| t.show_agent(address, "zsh".into(), cx));
                }
                Err(e) => tracing::warn!("cannot split: {e}"),
            }
        })
        .detach();
    }

    /// Command-] and Command-[: the keyboard goes to the next or the previous pane of the tab, round the end.
    fn step_pane(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some((open, _)) = self.agent_view.clone() else { return };
        let Some(l) = self.open_layout(cx) else { return };
        let panes = l.panes();
        let Some(at) = panes.iter().position(|p| *p == open) else { return };
        let next = panes[(at as isize + by).rem_euclid(panes.len() as isize) as usize].clone();
        let title = self.agents.read(cx).spaces.iter().flat_map(|s| &s.agents).find(|a| a.address == next).map(|a| a.title.clone()).unwrap_or_default();
        self.tabs.update(cx, |t, cx| t.show_agent(next, title, cx));
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
                .border_color(rgb(if main { look::amber() } else { look::line_2() }))
                .bg(rgb(if main { look::amber() } else { look::panel_2() }))
                .text_size(px(12.5))
                .text_color(rgb(if main { look::on_fill() } else { look::text() }))
                .when(main, |d| d.font_weight(FontWeight::SEMIBOLD))
                .when(!main, |d| d.hover(|d| d.bg(rgb(look::selected()))))
                .cursor_pointer()
                .child(div().min_w_0().truncate().child(text))
                .child(div().flex_none().text_size(px(11.)).opacity(0.6).child(key))
        };
        let mut acts = div().flex().flex_wrap().items_center().gap(px(8.));
        acts = acts.child(
            button("ask-deny".into(), "Deny".into(), "⌘⌫".into(), false)
                .border_color(rgba(0))
                .bg(rgba(0))
                .text_color(rgb(look::dim()))
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
                .border_color(rgb(look::line()))
                .bg(rgba((look::amber() << 8) | 0x0c))
                // the amber edge at the left, as the familiar look marks what waits
                .child(div().absolute().left_0().top_0().bottom_0().w(px(3.)).bg(rgb(look::amber())))
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .gap(px(6.))
                        .text_size(px(13.))
                        .line_height(px(19.5))
                        .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::text())).child(ask.title.clone()))
                        .child(div().min_w_0().truncate().text_color(rgb(look::dim())).child(ask.what.clone())),
                )
                .child(acts)
                .into_any_element(),
        )
    }

    /// Command-T or the + after a space's tabs: Claude Code in a new tab of the open agent's space, in the open agent's
    /// folder, opened at once.
    fn new_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((open, _)) = self.tabs.read(cx).agent.clone() else { return };
        let Some(space) = self.agents.read(cx).spaces.iter().find(|s| s.agents.iter().any(|a| a.address == open)).cloned() else { return };
        let folder = agents::new_agent_folder(&space, &open);
        let (core, tabs, display, agents) = (self.core.clone(), self.tabs.clone(), terminal::initial_display(window), self.agents.clone());
        cx.spawn(async move |_, cx| {
            // its tab, read before it shows, so the tab bar has it at once
            match cx.background_executor().spawn(async move { agents::start_in(&core, &space, folder, display).map(|a| (a, agents::read_spaces(&core))) }).await {
                Ok((address, spaces)) => {
                    if let Ok(spaces) = spaces {
                        let _ = agents.update(cx, |a, cx| a.read_now(spaces, cx));
                    }
                    let _ = tabs.update(cx, |t, cx| t.show_agent(address, "claude".into(), cx));
                }
                Err(e) => tracing::warn!("{e}"),
            }
        })
        .detach();
    }

    /// e in the Changes tab, or a file's open icon: the person's own editor on that file at that line, in a terminal
    /// tab (in the open agent's space, so Herdr shows it too), started by their login shell so it is the one they set
    /// ($VISUAL, else $EDITOR) with all their configuration. When it ends, the Changes tab comes back as it was.
    fn open_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, line)) = self.stack.update(cx, |s, _| s.edit.take()) else { return };
        let Some(root) = self.changes.read(cx).status.as_ref().map(|s| s.root.clone()) else { return };
        let back = self.tabs.read(cx).agent.clone();
        let space = back.as_ref().and_then(|(open, _)| self.agents.read(cx).spaces.iter().find(|s| s.agents.iter().any(|a| a.address == *open)).cloned());
        let kept = self.changes.read(cx).shown.clone();
        let argv = editor_argv(&path, line, &prefs::get().editor);
        let title = path.rsplit('/').next().unwrap_or(&path).to_owned();
        let (core, display) = (self.core.clone(), terminal::initial_display(window));
        cx.spawn(async move |this, cx| {
            let started = cx
                .background_executor()
                .spawn(async move {
                    match space {
                        Some(space) => agents::run_in(&core, &space, argv, Some(root), display),
                        None => core.run(argv, Some(root), json!({"jaynes.editor": "1"}), None, None, display).map_err(|e| format!("{e:#}")),
                    }
                })
                .await;
            match started {
                Ok(address) => {
                    let _ = this.update(cx, |root: &mut Root, cx| {
                        root.editing = Some((address.clone(), back, kept));
                        root.tabs.update(cx, |t, cx| t.show_agent(address, title, cx));
                        root.watch_editor(cx);
                    });
                }
                Err(e) => tracing::warn!("cannot open the editor: {e}"),
            }
        })
        .detach();
    }

    /// Waits for the editor to end, then goes back to where the person was, the Changes tab again.
    fn watch_editor(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
            let going = this.update(cx, |root: &mut Root, cx| {
                let Some((address, _, _)) = root.editing.clone() else { return false };
                let ended = root.agent_view.as_ref().filter(|(a, _)| *a == address).is_some_and(|(_, v)| v.read(cx).ended.is_some());
                // the person went elsewhere: the editor goes on in its tab, nothing to bring back
                if root.agent_view.as_ref().is_none_or(|(a, _)| *a != address) {
                    root.editing = None;
                    return false;
                }
                if ended {
                    let (address, back, kept) = root.editing.take().unwrap_or_default();
                    // the editor's terminal is done with: it leaves herdr-core's list
                    let core = root.core.clone();
                    cx.background_executor().spawn(async move { core.call("terminal.rm", json!({"terminal": address})) }).detach();
                    root.tabs.update(cx, |t, cx| match back {
                        Some((a, title)) => t.show_agent(a, title, cx),
                        None => t.show(None, cx),
                    });
                    root.changes.update(cx, |c, cx| c.restore(kept, cx));
                    return false;
                }
                true
            });
            if !matches!(going, Ok(true)) {
                break;
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
                    .child(self.changes_switch(cx))
                    .child(own_press(look::icon_button("new-run", look::Icon::Plus, false)).on_click(cx.listener(|this, _, window, cx| {
                        new_run(&this.terminal, window, cx);
                    })))
                    .child(own_press(look::icon_button("steps-switch", look::Icon::Panel, self.steps)).on_click(cx.listener(|this, _, _, cx| {
                        this.steps = !this.steps;
                        cx.notify();
                    }))),
            )
    }

    /// The Changes panel's switch, as the familiar look has it: its name and how many files differ, bright while on.
    fn changes_switch(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let n = self.changes.read(cx).count();
        own_press(
            div()
                .id("changes-switch")
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(10.))
                .py(px(5.))
                .rounded(px(6.))
                .text_size(px(12.5))
                .cursor_pointer()
                .text_color(rgb(if self.changes_on { look::text() } else { look::faint() }))
                .when(self.changes_on, |d| d.bg(rgb(look::selected())))
                .hover(|d| d.text_color(rgb(look::dim())))
                .child("Changes")
                .when(n > 0, |d| d.child(div().text_color(rgb(look::faint())).child(n.to_string()))),
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.changes_on = !this.changes_on;
            cx.notify();
        }))
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
            .child(div().text_size(px(19.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::text())).child("Inbox"))
            .child(
                div()
                    .mt(px(4.))
                    .text_size(px(12.5))
                    .text_color(rgb(look::faint()))
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
                .border_color(rgb(look::line_2()))
                .bg(rgb(look::panel_2()))
                .text_size(px(12.5))
                .text_color(rgb(look::text()))
                .hover(|d| d.bg(rgb(look::selected())))
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
                    .text_color(rgb(look::dim()))
                    .child(name)
                    .child(div().font_weight(FontWeight::NORMAL).text_color(rgb(look::faint())).child(items.len().to_string())),
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
                    agents::State::Waiting => look::amber(),
                    agents::State::Error => look::red(),
                    _ => look::violet(),
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
                        .hover(|d| d.bg(rgb(look::panel_2())))
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
                                        .child(div().flex_none().max_w(px(320.)).truncate().font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::text())).child(item.title.clone()))
                                        .child(div().min_w_0().truncate().text_color(rgb(look::dim())).child(item.what.clone())),
                                )
                                .child(div().text_size(px(12.)).text_color(rgb(look::faint())).child(item.place.clone())),
                        )
                        .child(act)
                        .child(
                            div()
                                .w(px(34.))
                                .flex_none()
                                .text_right()
                                .text_size(px(12.))
                                .text_color(rgb(look::faint()))
                                .child(if item.since_ms > 0 { agents::age(item.since_ms, now) } else { String::new() }),
                        ),
                );
            }
        }
        if needs.is_empty() && review.is_empty() {
            page = page.child(div().mt(px(40.)).text_size(px(15.)).font_weight(FontWeight::MEDIUM).text_color(rgb(look::green())).child("✓ All clear. Nothing needs you."));
        }
        let all = || self.agents.read(cx).spaces.iter().flat_map(|s| s.agents.iter()).cloned().collect::<Vec<_>>();
        let (working, idle) = (all().iter().filter(|a| a.state == agents::State::Working).count(), all().iter().filter(|a| a.state == agents::State::Idle && !a.shell).count());
        if working + idle > 0 {
            let words = [(working, "working"), (idle, "idle")].iter().filter(|(n, _)| *n > 0).map(|(n, w)| format!("{n} {w}")).collect::<Vec<_>>().join(" · ");
            page = page.child(div().mt(px(26.)).text_size(px(12.5)).text_color(rgb(look::faint())).child(format!("{words}. They show up here when they need you.")));
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
            .bg(rgb(look::panel_2()))
            .border_1()
            .border_color(rgb(look::line_2()))
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::text())).child("Shortcuts"));
        for (group, keys) in SHORTCUTS {
            let mut list = div().flex().flex_col().gap(px(6.)).child(look::label(group, look::faint()));
            for (key, what) in keys.iter() {
                list = list.child(
                    div()
                        .flex()
                        .gap(px(12.))
                        .child(div().w(px(110.)).flex_none().font_family(look::MONO_FONT).text_size(px(12.5)).text_color(rgb(look::text())).child(*key))
                        .child(div().text_size(px(13.)).text_color(rgb(look::dim())).child(*what)),
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
            .bg(rgba(look::scrim_2()))
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
            item().id(id).cursor_pointer().hover(|d| d.text_color(rgb(look::text()))).on_click(cx.listener(|this, _, _, cx| this.tabs.update(cx, |t, cx| t.show_inbox(cx))))
        };
        let dot = |c: u32| div().size(px(7.)).rounded_full().bg(rgb(c));
        let mut bar = div()
            .h(px(STATUS))
            .flex_none()
            .bg(rgb(look::side()))
            .flex()
            .items_center()
            .gap(px(16.))
            .px(px(12.))
            .border_t_1()
            .border_color(rgb(look::line()))
            .text_size(px(11.5))
            .text_color(rgb(look::dim()));
        if inbox {
            bar = bar.child(div().font_weight(FontWeight::MEDIUM).text_color(rgb(look::text())).child("Inbox"));
        } else {
            bar = bar
                .when_some(space, |d, s| d.child(div().font_weight(FontWeight::MEDIUM).text_color(rgb(look::text())).child(s)))
                .when(!self.branch.is_empty(), |d| d.child(div().font_family(look::MONO_FONT).text_size(px(11.)).text_color(rgb(look::blue())).child(format!("⎇ {}", self.branch))))
                .child(div().min_w_0().truncate().child(place))
                .when(open_works, |d| {
                    d.child(
                        item()
                            .id("stop-agent")
                            .cursor_pointer()
                            .hover(|d| d.text_color(rgb(look::text())))
                            .on_click(cx.listener(|this, _, _, cx| this.stop_agent(cx)))
                            .child("Stop")
                            .child(div().opacity(0.6).child("⌘.")),
                    )
                });
        }
        bar.child(div().flex_1())
            .when(waiting > 0, |d| d.child(count("count-waiting").child(dot(look::amber())).child(format!("{waiting} waiting"))))
            .when(review > 0, |d| d.child(count("count-review").child(dot(look::violet())).child(format!("{review} to review"))))
            .when(working > 0, |d| d.child(count("count-working").child(look::spinner("status-spin", 10., look::blue())).child(format!("{working} working"))))
            .when(!working_runs.is_empty(), |d| {
                d.child(
                    item()
                        .id("halt-all")
                        .px(px(6.))
                        .py(px(2.))
                        .rounded(px(5.))
                        .text_color(rgb(look::red()))
                        .cursor_pointer()
                        .hover(|d| d.bg(rgba((look::red() << 8) | 0x1f)))
                        .on_click(move |_, _, _| halt(&working_runs))
                        .child(look::icon(look::Icon::Stop, 13., look::red()))
                        .child("Halt all runs"),
                )
            })
            .child(
                item()
                    .id("shortcuts-hint")
                    .cursor_pointer()
                    .hover(|d| d.text_color(rgb(look::text())))
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

/// The command that opens the person's editor on a file at a line: their login shell reads their settings, picks
/// $VISUAL, else $EDITOR, else vi, and the editor replaces it (helix takes the line as file:line, the others as +line).
fn editor_argv(path: &str, line: u32, editor: &str) -> Vec<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let file = quote(path);
    // the one picked in Settings, else the one the person set for their shell
    let e = if editor.trim().is_empty() { "\"${VISUAL:-${EDITOR:-vi}}\"".to_owned() } else { quote(editor.trim()) };
    let script = format!("e={e}; case \"${{e%% *}}\" in *hx|*helix) exec $e {file}:{line} ;; *) exec $e +{line} {file} ;; esac");
    vec![shell, "-lic".into(), script]
}

/// A folder under the person's home, from the tilde.
fn home_short(folder: &str, home: &str) -> String {
    match folder.strip_prefix(home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => format!("~{rest}"),
        _ => folder.to_owned(),
    }
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
    // what the app says goes to its own log too, and it ends saying why (trail.rs)
    trail::start(tracing_subscriber::EnvFilter::try_from_env("JAYNES_APP_LOG").unwrap_or_else(|_| "warn,jaynes_app=info".into()));
    if let Some(bin) = core_bin::find(None) {
        core_bin::use_core(bin);
    }
    tracing::info!("bundle: {}", if std::env::current_exe().is_ok_and(|e| e.to_string_lossy().contains(".app/Contents/MacOS/")) { "Jaynes.app" } else { "none, a bare build" });
    let core = match hc::Core::connect() {
        Ok(core) => core,
        Err(e) => trail::fail(&format!("cannot reach herdr-core (hc): {e:#}")),
    };
    if let Err(e) = hc::check(&core.hello) {
        trail::fail(&format!("this herdr-core cannot serve the app: {e:?}"));
    }

    let app = gpui_platform::application();
    // the Dock icon clicked while the app runs: its window comes back
    app.on_reopen(show_main);
    app.run(move |cx| {
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
            KeyBinding::new("cmd-shift-g", ToggleChanges, Some("Jaynes")),
            KeyBinding::new("cmd-d", SplitRight, Some("Jaynes")),
            KeyBinding::new("cmd-shift-d", SplitDown, Some("Jaynes")),
            KeyBinding::new("cmd-]", NextPane, Some("Jaynes")),
            KeyBinding::new("cmd-[", PreviousPane, Some("Jaynes")),
            KeyBinding::new("cmd-/", Shortcuts, Some("Jaynes")),
            KeyBinding::new("cmd-k", OpenPalette, Some("Jaynes")),
            KeyBinding::new("cmd-,", OpenSettings, None),
            KeyBinding::new("cmd-w", settings_window::CloseSettings, Some("Settings")),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        // the menu bar's menus, where a Mac app shows what it does and its keys
        cx.set_menus(vec![
            Menu { name: "Jaynes".into(), disabled: false, items: vec![MenuItem::action("Settings…", OpenSettings), MenuItem::separator(), MenuItem::action("Shortcuts", Shortcuts), MenuItem::separator(), MenuItem::action("Quit Jaynes", Quit)] },
            Menu { name: "Edit".into(), disabled: false, items: vec![MenuItem::action("Copy", Copy), MenuItem::action("Paste", Paste)] },
            Menu {
                name: "View".into(),
                disabled: false,
                items: vec![
                    MenuItem::action("Go to…", OpenPalette),
                    MenuItem::separator(),
                    MenuItem::action("Runs Column", ToggleRuns),
                    MenuItem::action("Steps Panel", ToggleSteps),
                    MenuItem::action("Changes Panel", ToggleChanges),
                    MenuItem::separator(),
                    MenuItem::action("Next Tab", NextTab),
                    MenuItem::action("Previous Tab", PreviousTab),
                    MenuItem::action("Close Tab", CloseTab),
                    MenuItem::action("New Agent", NewAgent),
                    MenuItem::separator(),
                    MenuItem::action("Split Right", SplitRight),
                    MenuItem::action("Split Down", SplitDown),
                    MenuItem::action("Next Pane", NextPane),
                    MenuItem::action("Previous Pane", PreviousPane),
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
        cx.on_action(|_: &Quit, cx| {
            trail::ending(trail::Ending::Quit);
            cx.quit();
        });
        // the app's last line in its log, however it ends
        cx.on_app_quit(|_| {
            trail::ended();
            async {}
        })
        .detach();
        cx.on_action(|_: &OpenSettings, cx| settings_window::open(cx));
        // The terminal stays in herdr-core when the window closes; the app has nothing else to show.
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                trail::ending(trail::Ending::LastWindow);
                cx.quit();
            }
        })
        .detach();
        notify::listen();
        prefs::start(cx);
        open(core.clone(), cx);
        // A test that draws the window into a picture leaves the person's keyboard where it is.
        if crate::testing::var("JAYNES_APP_SHOT").is_none() {
            cx.activate(true);
        }
    });
}

fn open(core: Arc<hc::Core>, cx: &mut App) {
    let (state, why) = app_state::load(&app_state::path());
    if let Some(why) = why {
        tracing::warn!("{why}");
    }
    let screens: Vec<app_state::Screen> = cx.displays().iter().map(|d| app_state::Screen { id: d.id().into(), width: d.bounds().size.width.into(), height: d.bounds().size.height.into() }).collect();
    let main = cx.primary_display().map(|d| u64::from(d.id()));
    let kept = app_state::frame_on(state.frame, &screens, main);
    let display_id = kept.and_then(|f| f.screen).map(DisplayId::from);
    let bounds = match kept {
        Some(f) => Bounds::new(point(px(f.x), px(f.y)), size(px(f.width), px(f.height))),
        None => {
            if let Some(f) = state.frame {
                tracing::warn!("the window stood at {f:?}, off the screens there are now ({} known) or too small: it opens at its own place", screens.len());
            }
            Bounds::centered(None, size(px(1440.), px(880.)), cx)
        }
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        display_id,
        // the window draws its own top bar, the macOS buttons inside it
        titlebar: Some(TitlebarOptions { title: Some("Jaynes".into()), appears_transparent: true, traffic_light_position: Some(point(px(14.), px(14.))) }),
        app_id: Some("jaynes-app".into()),
        focus: crate::testing::var("JAYNES_APP_SHOT").is_none(),
        // the top bar is the app's own: a tab dragged there moves the tab, not the window, which the bar's empty
        // parts move themselves (start_window_move); otherwise AppKit drags the window from the whole titlebar strip
        app_owns_titlebar_drag: true,
        ..Default::default()
    };
    cx.open_window(options, move |window, cx| {
        // Match the system: macOS going light or dark, the window follows
        window
            .observe_window_appearance(|window, cx| {
                let dark = matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark);
                prefs::system_changed(dark, cx);
            })
            .detach();
        // its close button hides it: the app stays, in the Dock and the menu bar, and notices still come
        window.on_window_should_close(cx, |window, cx| {
            if let Some(Some(root)) = window.root::<Root>() {
                root.update(cx, |r, cx| r.let_go(cx));
            }
            hide_main(window);
            false
        });
        // herdr-core that cannot save its terminals still starts one when asked, unseen: none is asked then
        let listed = existing_or_why(&core);
        let address = if listed.as_ref().is_err_and(hc::cannot_save) {
            tracing::warn!("{}", hc::CANNOT_SAVE);
            "none".to_string()
        } else {
            listed.ok().flatten().unwrap_or_else(|| {
            let labels = json!({LABEL.0: LABEL.1});
            core.run(vec!["claude".into()], Some(folder()), labels, None, None, terminal::initial_display(window))
                .unwrap_or_else(|e| {
                    // herdr-core that cannot save its terminals answers nothing: the window opens all the same, saying
                    // so, with what needs no new terminal (the runs, Settings, a run's story and summary)
                    if hc::cannot_save(&e) {
                        tracing::warn!("{}: {e:#}", hc::CANNOT_SAVE);
                        "none".into()
                    } else {
                        trail::fail(&format!("cannot start claude in a terminal: {e:#}"))
                    }
                })
            })
        };
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
        let changes = cx.new(|cx| changes::Changes::new(tabs.clone(), window, cx));
        let stack = cx.new(|cx| changes::Stack::new(changes.clone(), cx));
        island.update(cx, |i, _| i.changes = Some(changes.clone()));
        let terminal_focus = focus.clone();
        cx.new(|cx| {
            cx.observe(&runs, |this: &mut Root, _, cx| {
                this.show_what_stood(cx);
                cx.notify();
            })
            .detach();
            // a tab split or a pane gone, here or in Herdr: the panes shown follow
            cx.observe(&agents, |this: &mut Root, _, cx| {
                this.show_what_stood(cx);
                let open = this.agent_view.as_ref().map(|(a, _)| a.clone());
                let before: Vec<String> = this.panes.iter().map(|(a, _)| a.clone()).collect();
                this.sync_panes(open.as_deref(), cx);
                if this.panes.iter().map(|(a, _)| a.clone()).collect::<Vec<_>>() != before {
                    cx.notify();
                }
            })
            .detach();
            // the switch in the tabs bar counts the files Changes lists
            cx.observe(&changes, |_, _, cx| cx.notify()).detach();
            // the keys go where the person looks: the terminal on Claude Code's tab, else the window itself, or a
            // shortcut pressed over a run's story would reach nothing, the terminal being off the screen
            cx.observe_in(&tabs, window, move |this: &mut Root, tabs, window, cx| {
                let (open, agent, inbox) = (tabs.read(cx).open.clone(), tabs.read(cx).agent.as_ref().map(|(a, _)| a.clone()), tabs.read(cx).inbox);
                let changes = tabs.read(cx).changes.is_some();
                let was_agent = this.agent_view.as_ref().map(|(a, _)| a.clone());
                if open != this.was_open || agent != was_agent || inbox != this.was_inbox || changes != this.was_changes {
                    let before = this.middle_shown(cx);
                    this.was_open = open.clone();
                    this.was_inbox = inbox;
                    this.was_changes = changes;
                    this.follow_agent(agent, cx);
                    this.cover_until_painted(before, cx);
                    let to = match (&this.agent_view, &open) {
                        _ if changes => this.stack.read(cx).focus.clone(),
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
            // the branch and the changes, read again every few seconds: the person or an agent changes them
            // a click on a notice opens what it was about: the agent, or else the run
            let clicks = notify::clicked();
            cx.spawn(async move |this, cx| {
                while let Ok(about) = clicks.recv().await {
                    let opened = this.update(cx, |root: &mut Root, cx| {
                        show_main(cx);
                        let agent = root.agents.read(cx).spaces.iter().flat_map(|s| &s.agents).find(|a| a.address == about).map(|a| a.title.clone());
                        match agent {
                            Some(title) => root.tabs.update(cx, |t, cx| t.show_agent(about, title, cx)),
                            None => root.tabs.update(cx, |t, cx| t.show(Some(about), cx)),
                        }
                    });
                    if opened.is_err() {
                        break;
                    }
                }
            })
            .detach();
            let branch = cx.spawn(async move |this, cx| loop {
                let Ok(at) = this.update(cx, |root: &mut Root, cx| root.place(cx).1) else { break };
                let status = cx.background_executor().spawn({
                    let at = at.clone();
                    async move { git::read(&at) }
                });
                let status = status.await;
                let now = status.as_ref().map(|s| s.branch.clone()).unwrap_or_default();
                let gone = this
                    .update(cx, |root: &mut Root, cx| {
                        root.changes.update(cx, |c, cx| c.set(at, status, cx));
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
            // the window moved or resized is kept as it stands
            let bounds = cx.observe_window_bounds(window, |this: &mut Root, window, cx| this.remember(window, cx));
            let (changes_on, steps, sidebar) = (state.changes || crate::testing::var("JAYNES_APP_CHANGES").is_some(), state.steps, state.column && crate::testing::var("JAYNES_APP_NOSIDE").is_none());
            Root { runs, tabs, side, terminal: view, core: core.clone(), agents, agent_view: None, panes: Vec::new(), cover: None, rail, changes, changes_on, stack, was_changes: false, editing: None, island, compare, lightbox, steps, sidebar, branch: String::new(), help: false, focus: cx.focus_handle(), was_open: None, was_inbox: false, let_go: false, armed: None, act_said: None, palette: None, later: Default::default(), restore: Some(state), kept: None, _bounds: Some(bounds), _branch: branch }
        })
    })
    .map(|handle| {
        MAIN.with(|m| m.set(Some(handle.into())));
        presence_ticks(handle.into(), cx);
        if let Some(path) = crate::testing::var("JAYNES_APP_SHOT") {
            shots(handle.into(), path.into(), cx);
        }
    })
    .expect("cannot open the Jaynes window");
}

/// How the person stands to the main window now, for their presence at a stop.
pub(crate) fn seen_now(window: &mut Window, cx: &App) -> looking_at::Seen {
    let front = window.is_window_active() && ns_window_visible(window);
    let stop = window.root::<Root>().flatten().and_then(|r| r.read(cx).stop_on_screen(cx));
    looking_at::Seen { front, last_input_ms: looking_at::last_input(), stop }
}

/// Command-K as a test uses it: `type <letters>`, `down`, `up`, `go`.
#[cfg(feature = "test-driver")]
pub(crate) fn palette_does(what: String, cx: &mut App) {
    let Some(main) = MAIN.with(|m| m.get()) else { return tracing::warn!("palette {what}: no main window") };
    let updated = main.update(cx, |_, window, cx| {
        let Some(Some(root)) = window.root::<Root>() else { return tracing::warn!("palette {what}: no root") };
        // opened as the Command-K action opens it, without the keyboard: a test's window has none, and an action
        // dispatched there reaches the window only when something in it has the keys
        if what == "open" {
            return root.update(cx, |r, cx| r.open_palette(window, cx));
        }
        let Some(p) = root.read(cx).palette() else { return tracing::warn!("palette {what}: Command-K is not open") };
        let (verb, rest) = what.split_once(' ').unwrap_or((what.as_str(), ""));
        let rest = rest.to_string();
        p.update(cx, |p, cx| match verb {
            "type" => p.typed(&rest, window, cx),
            "down" => p.step(1, cx),
            "up" => p.step(-1, cx),
            "go" => p.go(cx),
            _ => tracing::warn!("palette: no verb {verb}"),
        });
    });
    if let Err(e) = updated {
        tracing::warn!("palette {what}: the window could not be updated: {e}");
    }
}

/// Later on the line over the agent, as a click on it: for a test.
#[cfg(feature = "test-driver")]
pub(crate) fn later_on_line(cx: &mut App) {
    let Some(main) = MAIN.with(|m| m.get()) else { return };
    let _ = main.update(cx, |_, window, cx| {
        if let Some(Some(root)) = window.root::<Root>() {
            root.update(cx, |r, cx| r.put_off(cx));
        }
    });
}

/// A press on the open run's control named `word`, as a click on it: for a test.
#[cfg(feature = "test-driver")]
pub(crate) fn press_act(word: String, cx: &mut App) {
    use run_actions::Act;
    let Some(act) = [Act::Resume, Act::Continue, Act::RaiseRisk, Act::RaiseCeiling, Act::Abandon].into_iter().find(|a| a.word() == word) else { return };
    let Some(main) = MAIN.with(|m| m.get()) else { return };
    let _ = main.update(cx, |_, window, cx| {
        if let Some(Some(root)) = window.root::<Root>() {
            root.update(cx, |r, cx| {
                let shown = r.tabs.read(cx).facts.map(|f| run_actions::acts(&f)).unwrap_or_default();
                tracing::info!("controls shown: {:?}", shown.iter().map(|a| a.word()).collect::<Vec<_>>());
                if let Some(run) = r.tabs.read(cx).open.clone() {
                    r.press(run, act, cx);
                }
            });
        }
    });
}

/// What the next presence tick would tell, said in the log and sent nowhere: for a test.
#[cfg(feature = "test-driver")]
pub(crate) fn presence_probe(cx: &mut App) {
    let Some(main) = MAIN.with(|m| m.get()) else { return };
    if let Ok(seen) = main.update(cx, |_, window, cx| seen_now(window, cx)) {
        let counts = looking_at::counts(&seen, jaynes_shell::store::now());
        tracing::info!("presence: stop on the screen {:?}, window in front {}, touched {}, would tell {:?}", seen.stop, seen.front, seen.last_input_ms.is_some(), counts);
    }
}

/// Every presence::EVERY seconds, the person's presence at the stop on the screen, when it counts (looking_at.rs):
/// the window in front with the keyboard, a hand on it in the last minute.
fn presence_ticks(main: AnyWindowHandle, cx: &mut App) {
    cx.spawn(async move |cx| loop {
        cx.background_executor().timer(std::time::Duration::from_secs(jaynes_shell::presence::EVERY)).await;
        let seen = main.update(cx, |_, window, cx| seen_now(window, cx));
        let Ok(seen) = seen else { break };
        if let Some(stop) = looking_at::counts(&seen, jaynes_shell::store::now()) {
            looking_at::tell(stop);
        }
    })
    .detach();
}

thread_local! {
    /// The main window: hidden when the person closes it, never closed, so it comes back as it was.
    static MAIN: std::cell::Cell<Option<AnyWindowHandle>> = const { std::cell::Cell::new(None) };
}

/// Hides the main window, as its close button and Command-W on its last tab do: the app stays in the Dock and the
/// menu bar, its notices still come, and its tabs, the run open and the terminals stay as they are.
pub(crate) fn hide_main(window: &mut Window) {
    set_shown(window, false);
    trail::window_closed();
}

/// Brings the main window back as it was, in front: a click on the Dock, on the menu bar item or on a notice.
pub(crate) fn show_main(cx: &mut App) {
    if let Some(main) = MAIN.with(|m| m.get()) {
        let _ = main.update(cx, |_, window, cx| {
            set_shown(window, true);
            window.activate_window();
            if let Some(Some(root)) = window.root::<Root>() {
                root.update(cx, |r, cx| r.take_up(cx));
            }
        });
    }
    // a test that draws the window into a picture leaves the person's keyboard where it is
    if crate::testing::var("JAYNES_APP_SHOT").is_none() {
        cx.activate(true);
    }
}

/// Whether this window is on the screen.
pub(crate) fn shown(window: &mut Window) -> bool {
    ns_window_visible(window)
}

/// Orders the window out of the screen, or back in front, without closing it (AppKit's orderOut: and
/// makeKeyAndOrderFront:; GPUI hides only the whole app).
#[cfg(target_os = "macos")]
fn set_shown(window: &mut Window, shown: bool) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return };
    // SAFETY: the view AppKit gave GPUI for this window, alive while the window is; its window is an NSWindow.
    unsafe {
        let view: &AnyObject = &*h.ns_view.as_ptr().cast::<AnyObject>();
        let ns: *mut AnyObject = msg_send![view, window];
        if let Some(ns) = ns.as_ref() {
            let none: *const AnyObject = std::ptr::null();
            if shown {
                let _: () = msg_send![ns, makeKeyAndOrderFront: none];
            } else {
                let _: () = msg_send![ns, orderOut: none];
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn ns_window_visible(window: &mut Window) -> bool {
    use objc2::msg_send;
    use objc2::runtime::{AnyObject, Bool};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return false };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return false };
    // SAFETY: as in set_shown.
    unsafe {
        let view: &AnyObject = &*h.ns_view.as_ptr().cast::<AnyObject>();
        let ns: *mut AnyObject = msg_send![view, window];
        ns.as_ref().is_some_and(|ns| {
            let v: Bool = msg_send![ns, isVisible];
            v.as_bool()
        })
    }
}

#[cfg(not(target_os = "macos"))]
fn set_shown(window: &mut Window, _shown: bool) {
    window.minimize_window();
}

#[cfg(not(target_os = "macos"))]
fn ns_window_visible(_: &mut Window) -> bool {
    true
}

/// The person's own key, paired with the engine; None when either fails, and the app then answers nothing.
fn the_key() -> Option<Arc<key::Key>> {
    // a test's key is paired only with a test's engine, never with the person's own
    if crate::testing::var("JAYNES_APP_TEST_KEY").is_some() && std::env::var_os("JAYNES_ENGINE_HOME").is_none() {
        tracing::warn!("a test key pairs only with an engine named in JAYNES_ENGINE_HOME");
        return None;
    }
    let key = key::Key::load().map_err(|e| tracing::warn!("no key of the person's: {e}")).ok()?;
    key.pair(&jaynes_shell::home()).map_err(|e| tracing::warn!("the key is not paired: {e}")).ok()?;
    Some(Arc::new(key))
}

/// With JAYNES_APP_SHOT=<file>, the window draws itself into that PNG every second, even when it is covered: how a
/// test (or an agent) sees what the person would see without taking their screen.
/// With JAYNES_APP_SHOT_MS=<ms> it draws that often instead, and a `{n}` in the file's name numbers the pictures, so
/// a test can watch something that lasts less than a second.
pub(crate) fn shots(window: AnyWindowHandle, path: std::path::PathBuf, cx: &mut App) {
    let every = crate::testing::var("JAYNES_APP_SHOT_MS").and_then(|ms| ms.to_str().and_then(|ms| ms.parse().ok())).unwrap_or(1000);
    cx.spawn(async move |cx| {
        // The app quits with its window, so the loop ends with it.
        for n in 0u64.. {
            cx.background_executor().timer(std::time::Duration::from_millis(every)).await;
            cx.update(terminal::flush_output_clock);
            let path = std::path::PathBuf::from(path.to_string_lossy().replace("{n}", &format!("{n:05}")));
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
    use super::{editor_argv, home_short};

    #[test]
    fn the_editor_opens_at_the_line_through_the_login_shell() {
        let argv = editor_argv("app/src/it's.rs", 218, "");
        assert_eq!(argv[1], "-lic");
        let out = std::process::Command::new("sh").arg("-c").arg(argv[2].replace("exec $e", "echo $e")).env("VISUAL", "nvim").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "nvim +218 app/src/it's.rs");
        let out = std::process::Command::new("sh").arg("-c").arg(argv[2].replace("exec $e", "echo $e")).env("VISUAL", "hx").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hx app/src/it's.rs:218");
        // the editor picked in Settings comes before the shell's
        let argv = editor_argv("a.rs", 3, "hx");
        let out = std::process::Command::new("sh").arg("-c").arg(argv[2].replace("exec $e", "echo $e")).env("VISUAL", "nvim").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hx a.rs:3");
    }

    #[test]
    fn a_folder_under_home_is_shown_from_the_tilde() {
        assert_eq!(home_short("/Users/a/Projects/jaynes", "/Users/a"), "~/Projects/jaynes");
        assert_eq!(home_short("/Users/a", "/Users/a"), "~");
        assert_eq!(home_short("/Users/ab/x", "/Users/a"), "/Users/ab/x");
        assert_eq!(home_short("/tmp/x", ""), "/tmp/x");
    }
}
