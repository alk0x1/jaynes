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
mod presence;
mod rail;
mod settings;
mod side;
mod spawn;
mod sprites;
mod terminal;
mod vt;
mod win32_keys;

use gpui::*;
use serde_json::{json, Value};
use std::sync::Arc;
use terminal::TerminalView;

actions!(jaynes, [Copy, Paste, Quit, ToggleSteps]);

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
    std::env::args()
        .nth(1)
        .or_else(|| std::env::current_dir().ok().map(|d| d.to_string_lossy().into_owned()))
        .unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/".into()))
}

struct Root {
    side: Entity<side::Side>,
    terminal: Entity<TerminalView>,
    rail: Entity<rail::Rail>,
    island: Entity<island::Island>,
    compare: Entity<compare::Compare>,
    lightbox: Entity<lightbox::Lightbox>,
    /// Whether the steps panel shows (Command-E).
    steps: bool,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        terminal::next_frame();
        div()
            .size_full()
            .flex()
            .key_context("Jaynes")
            .on_action(copy_into(&self.terminal))
            .on_action(paste_into(&self.terminal))
            .on_action(cx.listener(|this, _: &ToggleSteps, _, cx| {
                this.steps = !this.steps;
                cx.notify();
            }))
            .relative()
            .child(self.side.clone())
            .child(div().relative().flex_1().min_w_0().h_full().child(self.terminal.clone()).child(self.island.clone()))
            .children(self.steps.then(|| self.rail.clone()))
            // over everything: the compare mode, then a picture or a diff at full size
            .child(self.compare.clone())
            .child(self.lightbox.clone())
    }
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
        titlebar: Some(TitlebarOptions { title: Some("Jaynes".into()), ..Default::default() }),
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
        let side = cx.new(|cx| side::Side::new(runs.clone(), cx));
        let rail = cx.new(|cx| rail::Rail::new(runs.clone(), cx));
        let key = the_key();
        let lightbox = cx.new(lightbox::Lightbox::new);
        let compare = cx.new(|cx| compare::Compare::new(key.clone(), cx));
        let island = cx.new(|cx| island::Island::new(runs, key, lightbox.clone(), compare.clone(), cx));
        cx.new(|_| Root { side, terminal: view, rail, island, compare, lightbox, steps: true })
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
