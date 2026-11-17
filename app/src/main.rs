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
        NextRun, PreviousRun, OpenWaiting, SendAnswer, HaltRun, CloseTab, Shortcuts
    ]
);

/// Every shortcut, as the list over the window (Command-/) shows it.
const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    ("Tabs", &[("⌘1 … ⌘8", "The tab at that place"), ("⌘9", "The last tab"), ("⇧⌘]  ⇧⌘[", "Next tab, previous tab"), ("⌘W", "Close a run's tab")]),
    ("Runs", &[("⌥⌘↓  ⌥⌘↑", "Next run, previous run"), ("⌘O", "Open what waits for you"), ("⌘↩", "Send your answer"), ("⌘.", "Halt the picked run"), ("⌘N", "New run")]),
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
                            .child(div().flex_1().min_h_0().child(match self.tabs.read(cx).body(cx) {
                                Some(story) => story,
                                None => self.terminal.clone().into_any_element(),
                            }))
                            .child(self.island.clone()),
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
            .on_action(copy_into(&self.terminal))
            .on_action(paste_into(&self.terminal))
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
            .on_action(cx.listener(|this, _: &OpenWaiting, window, cx| this.open_waiting(window, cx)))
            .on_action(cx.listener(|this, _: &SendAnswer, _, cx| this.island.update(cx, |i, cx| i.send_if_ready(cx))))
            .on_action(cx.listener(|this, _: &HaltRun, _, cx| this.halt_picked(cx)))
            .on_action(cx.listener(|this, _: &CloseTab, _, cx| this.tabs.update(cx, |t, cx| t.close(cx))))
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
    /// The runs column's header, after herdr: the macOS buttons, the mark and the name, then the column's switch.
    fn side_head(&self, cx: &mut Context<Self>) -> impl IntoElement {
        title_bar("side-head")
            // room for the macOS buttons
            .pl(px(82.))
            .pr(px(8.))
            .gap(px(8.))
            .child(look::mark_icon(22.))
            .child(div().text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::TEXT)).child("Jaynes"))
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

    /// Command-O: the first run that waits, its tab at the step that waits and its question open.
    fn open_waiting(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let runs = self.runs.read(cx);
        let Some(id) = runs.snap.listing.needs.first().map(|r| r.id.clone()) else { return };
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

    /// The status bar, after herdr: where Claude Code works, how many runs work and wait, and "Halt all".
    fn status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let runs = self.runs.read(cx);
        let working: Vec<String> = runs
            .snap
            .listing
            .projects
            .iter()
            .flat_map(|(_, rows)| rows.iter())
            .filter(|r| r.mark == jaynes_shell::listing::Mark::Working)
            .map(|r| r.id.clone())
            .collect();
        let waits = runs.snap.listing.needs.len();
        let item = || div().flex().items_center().gap(px(6.));
        let home = std::env::var("HOME").unwrap_or_default();
        let place = match folder() {
            f if !home.is_empty() && f.starts_with(&home) => format!("~{}", &f[home.len()..]),
            f => f,
        };
        let halt_colour = if working.is_empty() { look::FAINT } else { look::RED };
        div()
            .h(px(STATUS))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(16.))
            .px(px(12.))
            .border_t_1()
            .border_color(rgb(look::LINE))
            .text_size(px(11.5))
            .text_color(rgb(look::DIM))
            .child(item().child(look::icon(look::Icon::Folder, 13., look::DIM)).child(place))
            .when(!self.branch.is_empty(), |d| d.child(item().child(look::icon(look::Icon::Branch, 13., look::DIM)).child(self.branch.clone())))
            .when(!working.is_empty(), |d| d.child(item().child(look::spinner("status-spin", 10., look::BLUE)).child(format!("{} working", working.len()))))
            .when(waits > 0, |d| {
                d.child(item().text_color(rgb(look::AMBER)).child(div().size(px(7.)).rounded_full().bg(rgb(look::AMBER))).child(format!("{waits} waiting")))
            })
            .child(div().flex_1())
            .child(
                item()
                    .id("halt-all")
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(5.))
                    .text_color(rgb(halt_colour))
                    .when(!working.is_empty(), |d| d.cursor_pointer().hover(|d| d.bg(rgba((look::RED << 8) | 0x1f))))
                    .on_click(move |_, _, _| halt(&working))
                    .child(look::icon(look::Icon::Stop, 13., halt_colour))
                    .child("Halt all"),
            )
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
        let tabs = cx.new(|cx| tabs::Tabs::new(runs.clone(), cx));
        let agents = cx.new(|cx| agents::Agents::new(core.clone(), cx));
        let side = cx.new(|cx| side::Side::new(runs.clone(), agents, tabs.clone(), cx));
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
                let open = tabs.read(cx).open.clone();
                if open != this.was_open {
                    this.was_open = open.clone();
                    window.focus(if open.is_some() { &this.focus } else { &terminal_focus }, cx);
                }
                cx.notify();
            })
            .detach();
            // the branch, read again every few seconds: the person may switch it in the terminal
            let place = folder();
            let branch = cx.spawn(async move |this, cx| loop {
                let at = place.clone();
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
                cx.background_executor().timer(std::time::Duration::from_secs(5)).await;
            });
            // JAYNES_APP_NOSIDE: a test's picture of the window without its runs column
            Root { runs, tabs, side, terminal: view, rail, island, compare, lightbox, steps: true, sidebar: std::env::var_os("JAYNES_APP_NOSIDE").is_none(), branch: String::new(), help: false, focus: cx.focus_handle(), was_open: None, _branch: branch }
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
