//! The Jaynes app: a native window where the person works with Claude Code.
//!
//! The window: the runs at the left (from the engine's record), one terminal running `claude` in the middle, and the
//! picked run's steps at the right. The terminal lives in herdr-core (`hc`), not in the app, so closing the window
//! leaves the conversation running, and opening the app again shows it where it was.
//! The app finds its terminal by a label it puts on it (`jaynes.app=main`); a terminal whose program ended is replaced.

// The terminal copied from herdr-app keeps abilities this first window does not call yet (search, scrolling commands,
// theme switching); the later steps use them.
#![allow(dead_code)]

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

actions!(jaynes, [Copy, Paste, Quit, ToggleSteps, NewRun]);

/// The top bar's height; the macOS window buttons sit inside it.
const TOP: f32 = 40.;

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
}

impl Root {
    /// The window's own top bar: the macOS buttons, the mark and the project, then the steps panel's switch and
    /// "Halt all". Dragging it moves the window, a double click zooms it, as a title bar does.
    fn top(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let runs = self.runs.read(cx);
        let project = runs.chosen().map(|d| d.project.clone()).unwrap_or_else(|| {
            std::path::Path::new(&folder()).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        });
        let working: Vec<String> = runs
            .snap
            .listing
            .projects
            .iter()
            .flat_map(|(_, rows)| rows.iter())
            .filter(|r| r.mark == jaynes_shell::listing::Mark::Working)
            .map(|r| r.id.clone())
            .collect();
        let steps = self.steps;
        div()
            .id("top")
            .h(px(TOP))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            // room for the macOS buttons
            .pl(px(84.))
            .pr(px(12.))
            .bg(rgb(look::PANEL))
            .border_b_1()
            .border_color(rgb(look::LINE))
            .font_family(look::UI_FONT)
            .on_mouse_down(MouseButton::Left, |e, window, _| {
                if e.click_count == 2 {
                    window.titlebar_double_click();
                } else {
                    window.start_window_move();
                }
            })
            .child(look::mark_icon(18.))
            .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::TEXT)).child("Jaynes"))
            .child(div().text_size(px(12.5)).text_color(rgb(look::FAINT)).child(format!("· {project}")))
            .child(div().flex_1())
            .child(
                look::key_button("steps-switch", if steps { "◨ Steps" } else { "◧ Steps" }, "⌘E")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.steps = !this.steps;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("halt-all")
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(7.))
                    .border_1()
                    .border_color(rgba((look::RED << 8) | 0x59))
                    .text_size(px(12.5))
                    .text_color(rgb(if working.is_empty() { look::FAINT } else { look::RED }))
                    .when(!working.is_empty(), |d| d.cursor_pointer().hover(|d| d.bg(rgba((look::RED << 8) | 0x1f))))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, _, _| halt(&working))
                    .child("■ Halt all"),
            )
    }
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
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(look::PANEL))
            .key_context("Jaynes")
            .on_action(copy_into(&self.terminal))
            .on_action(paste_into(&self.terminal))
            .on_action(cx.listener(|this, _: &ToggleSteps, _, cx| {
                this.steps = !this.steps;
                cx.notify();
            }))
            .on_action(move |_: &NewRun, window, cx| new_run(&terminal, window, cx))
            .relative()
            .child(self.top(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.side.clone())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.tabs.clone())
                            .child(div().flex_1().min_h_0().child(match self.tabs.read(cx).body(cx) {
                                Some(story) => story,
                                None => self.terminal.clone().into_any_element(),
                            }))
                            .child(self.island.clone()),
                    )
                    .children(self.steps.then(|| self.rail.clone())),
            )
            // over everything: the compare mode, then a picture or a diff at full size
            .child(self.compare.clone())
            .child(self.lightbox.clone())
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
            KeyBinding::new("cmd-q", Quit, None),
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
        titlebar: Some(TitlebarOptions { title: Some("Jaynes".into()), appears_transparent: true, traffic_light_position: Some(point(px(14.), px(13.))) }),
        app_id: Some("jaynes-app".into()),
        focus: std::env::var_os("JAYNES_APP_SHOT").is_none(),
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
        let new_run_in = {
            let view = view.clone();
            std::rc::Rc::new(move |window: &mut Window, cx: &mut App| new_run(&view, window, cx)) as std::rc::Rc<dyn Fn(&mut Window, &mut App)>
        };
        let tabs = cx.new(|cx| tabs::Tabs::new(runs.clone(), cx));
        let side = cx.new(|cx| side::Side::new(runs.clone(), tabs.clone(), new_run_in, cx));
        let rail = cx.new(|cx| rail::Rail::new(runs.clone(), cx));
        let key = the_key();
        let lightbox = cx.new(lightbox::Lightbox::new);
        let compare = cx.new(|cx| compare::Compare::new(key.clone(), cx));
        let island = cx.new(|cx| island::Island::new(runs.clone(), key, lightbox.clone(), compare.clone(), cx));
        island.update(cx, |i, _| i.tabs = Some(tabs.clone()));
        cx.new(|cx| {
            cx.observe(&runs, |_, _, cx| cx.notify()).detach();
            cx.observe(&tabs, |_, _, cx| cx.notify()).detach();
            Root { runs, tabs, side, terminal: view, rail, island, compare, lightbox, steps: true }
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
