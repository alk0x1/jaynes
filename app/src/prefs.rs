//! The person's settings: what the Settings screen shows, kept in a file of theirs (app-settings.json in the engine's
//! home, ~/.jaynes/engine, or JAYNES_APP_SETTINGS) and read at start. Each is one the app applies; a change takes
//! effect at once.

use crate::settings::{OptionAsAlt, Settings};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The terminal colors the Settings screen offers for the dark appearance.
pub const COLORS: &[&str] = &["jaynes", "graphite", "ink", "moss"];
/// The terminal's colors in the light appearance: the window's own Linen.
pub const LIGHT_COLORS: &str = "linen";

/// Whether the window is dark or light: the person's pick, or macOS's as it changes during the day.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    Dark,
    Light,
    System,
}

/// Whether the window is light now: Light, or Match the system while macOS is light.
pub fn light_now(a: Appearance, system_dark: bool) -> bool {
    match a {
        Appearance::Dark => false,
        Appearance::Light => true,
        Appearance::System => !system_dark,
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Dark (the default), Light (Linen), or Match the system.
    pub appearance: Appearance,
    /// One of COLORS.
    pub terminal_colors: String,
    pub terminal_font: String,
    pub terminal_font_size: f32,
    pub option_key: OptionAsAlt,
    pub copy_on_select: bool,
    /// Wheel and trackpad scroll multiplier.
    pub scroll_speed: f32,
    /// The system's notices when an agent needs the person or finishes, or a run does.
    pub notifications: bool,
    /// What Command-T starts in a new tab: a program, or a command line for the login shell.
    pub new_agent: String,
    /// What e in Changes opens; empty: $VISUAL, else $EDITOR, else vi.
    pub editor: String,
    /// How big Changes draws its text, the panel and the tab with the diffs (1 = as designed): Command-plus and
    /// Command-minus with the keys there.
    pub changes_zoom: f32,
}

impl Default for Prefs {
    fn default() -> Self {
        let s = Settings::default();
        Prefs {
            appearance: Appearance::Dark,
            terminal_colors: crate::settings::DEFAULT_DARK.into(),
            terminal_font: s.font_family,
            terminal_font_size: s.font_size,
            option_key: s.option_as_alt,
            copy_on_select: s.copy_on_select,
            scroll_speed: s.terminal_scroll_speed,
            notifications: true,
            new_agent: "claude".into(),
            editor: String::new(),
            changes_zoom: 1.0,
        }
    }
}

const FONT_SIZE: (f32, f32) = (6.0, 72.0);
const SCROLL_SPEED: (f32, f32) = (0.1, 5.0);
const CHANGES_ZOOM: (f32, f32) = (0.7, 2.0);

/// A view of the app the zoom keys change, each keeping its own level: Changes, or the terminals' font.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Changes,
    Terminal,
}

/// What the zoom keys zoom: Changes while the stacked diffs or the commit message have the keys, else the terminal.
pub fn zoom_view(diffs_focused: bool, message_focused: bool) -> View {
    if diffs_focused || message_focused { View::Changes } else { View::Terminal }
}

/// The setting the zoom keys change in `view`, and its new value: `steps` up or down (a tenth of Changes' size, a
/// point of the terminal's font), held in reach; 0 brings the view's own size back.
pub fn zoomed(view: View, p: &Prefs, steps: i32) -> (&'static str, String) {
    match view {
        View::Changes => {
            let z = if steps == 0 { 1.0 } else { ((p.changes_zoom * 10.0).round() + steps as f32).clamp(CHANGES_ZOOM.0 * 10.0, CHANGES_ZOOM.1 * 10.0) / 10.0 };
            ("changes_zoom", z.to_string())
        }
        View::Terminal => {
            let size = if steps == 0 { Prefs::default().terminal_font_size } else { (p.terminal_font_size.round() + steps as f32).clamp(FONT_SIZE.0, FONT_SIZE.1) };
            ("terminal_font_size", size.to_string())
        }
    }
}

impl Prefs {
    /// Values out of reach come back inside it; an unknown color or an empty program is the default.
    fn kept_in_reach(mut self) -> Self {
        let d = Prefs::default();
        if !COLORS.contains(&self.terminal_colors.as_str()) {
            self.terminal_colors = d.terminal_colors;
        }
        if self.terminal_font.trim().is_empty() {
            self.terminal_font = d.terminal_font;
        }
        self.terminal_font_size = if self.terminal_font_size.is_finite() { self.terminal_font_size.clamp(FONT_SIZE.0, FONT_SIZE.1) } else { d.terminal_font_size };
        self.scroll_speed = if self.scroll_speed.is_finite() { self.scroll_speed.clamp(SCROLL_SPEED.0, SCROLL_SPEED.1) } else { d.scroll_speed };
        self.changes_zoom = if self.changes_zoom.is_finite() { self.changes_zoom.clamp(CHANGES_ZOOM.0, CHANGES_ZOOM.1) } else { d.changes_zoom };
        if self.new_agent.trim().is_empty() {
            self.new_agent = d.new_agent;
        }
        self.new_agent = self.new_agent.trim().to_owned();
        self.editor = self.editor.trim().to_owned();
        self
    }
}

/// Where the person's settings are kept.
pub fn path() -> PathBuf {
    std::env::var_os("JAYNES_APP_SETTINGS").map(PathBuf::from).unwrap_or_else(|| jaynes_shell::home().join("app-settings.json"))
}

/// The settings in `path`: the defaults when it is missing or not JSON; a field of the wrong kind is its default and
/// a field the app does not know (from a later version) is skipped, so the rest still count.
pub fn load(path: &Path) -> Prefs {
    let Ok(text) = std::fs::read_to_string(path) else { return Prefs::default() };
    let Ok(Value::Object(read)) = serde_json::from_str::<Value>(&text) else {
        tracing::warn!("{} is not a JSON object: the default settings count", path.display());
        return Prefs::default();
    };
    let mut kept = serde_json::to_value(Prefs::default()).expect("settings are JSON");
    for (k, v) in read {
        let Some(slot) = kept.get(&k) else { continue };
        let mut tried = kept.clone();
        tried[&k] = v;
        if serde_json::from_value::<Prefs>(tried.clone()).is_ok() {
            kept = tried;
        } else {
            tracing::warn!("{}: {k} is not one the app reads ({slot} kept)", path.display());
        }
    }
    serde_json::from_value::<Prefs>(kept).unwrap_or_default().kept_in_reach()
}

/// Writes them to `path` whole, through a file beside it, so a crash never leaves half a file.
pub fn save(path: &Path, p: &Prefs) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(p).expect("settings are JSON") + "\n")?;
    std::fs::rename(&tmp, path)
}

/// Changes one setting by its name, from words (as the test driver and the screen give them).
pub fn set(p: &mut Prefs, name: &str, value: &str) -> Result<(), String> {
    let value = value.trim();
    let number = |reach: (f32, f32)| value.trim_end_matches('×').parse::<f32>().map(|n| n.clamp(reach.0, reach.1)).map_err(|_| format!("{name} takes a number, not {value}"));
    let switch = || match value {
        "on" | "true" | "yes" => Ok(true),
        "off" | "false" | "no" => Ok(false),
        _ => Err(format!("{name} is on or off, not {value}")),
    };
    match name {
        "appearance" => {
            p.appearance = match value {
                "dark" => Appearance::Dark,
                "light" => Appearance::Light,
                "system" => Appearance::System,
                _ => return Err(format!("appearance is dark, light or system, not {value}")),
            }
        }
        "terminal_colors" if COLORS.contains(&value) => p.terminal_colors = value.into(),
        "terminal_colors" => return Err(format!("no terminal colors named {value}: {}", COLORS.join(", "))),
        "terminal_font" if !value.is_empty() => p.terminal_font = value.into(),
        "terminal_font_size" => p.terminal_font_size = number(FONT_SIZE)?,
        "option_key" => {
            p.option_key = match value {
                "auto" => OptionAsAlt::Auto,
                "alt" | "on" => OptionAsAlt::On,
                "left" => OptionAsAlt::Left,
                "right" => OptionAsAlt::Right,
                "characters" | "off" => OptionAsAlt::Off,
                _ => return Err(format!("option_key is auto, alt, left, right or characters, not {value}")),
            }
        }
        "copy_on_select" => p.copy_on_select = switch()?,
        "scroll_speed" => p.scroll_speed = number(SCROLL_SPEED)?,
        "changes_zoom" => p.changes_zoom = number(CHANGES_ZOOM)?,
        "notifications" => p.notifications = switch()?,
        "new_agent" if !value.is_empty() => p.new_agent = value.into(),
        "editor" => p.editor = value.into(),
        _ => return Err(format!("no setting {name} takes {value:?}")),
    }
    Ok(())
}

/// The terminal's own settings with the person's laid over them, in the dark.
pub fn onto(p: &Prefs, s: Settings) -> Settings {
    onto_for(p, s, false)
}

/// The terminal's settings with the person's laid over them; in the light, the terminal is Linen, as the window is.
pub fn onto_for(p: &Prefs, mut s: Settings, light: bool) -> Settings {
    s.theme = if light { LIGHT_COLORS.into() } else { p.terminal_colors.clone() };
    s.theme_dark = p.terminal_colors.clone();
    s.font_family = p.terminal_font.clone();
    s.font_size = p.terminal_font_size;
    s.option_as_alt = p.option_key;
    s.copy_on_select = p.copy_on_select;
    s.terminal_scroll_speed = p.scroll_speed;
    s
}

/// What Command-T runs: a lone program as it is, else the line through the person's login shell, as they type it.
pub fn new_agent_argv(command: &str) -> Vec<String> {
    let command = command.trim();
    if !command.is_empty() && !command.contains(char::is_whitespace) {
        return vec![command.to_owned()];
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    vec![shell, "-lic".into(), format!("exec {command}")]
}

static CURRENT: Mutex<Option<Prefs>> = Mutex::new(None);
/// Whether macOS is dark now, as the window last heard.
static SYSTEM_DARK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// What macOS's appearance is now (at start, and each time it changes).
pub fn system_is_dark(dark: bool) {
    SYSTEM_DARK.store(dark, std::sync::atomic::Ordering::Relaxed);
}

/// Puts `p` in force: the window's colours and the terminal's, for the appearance it says now.
fn in_force(p: &Prefs) {
    let light = light_now(p.appearance, SYSTEM_DARK.load(std::sync::atomic::Ordering::Relaxed));
    crate::look::set(crate::look::for_light(light));
    crate::settings::set(onto_for(p, (*crate::settings::get()).clone(), light));
}

/// Puts the settings in force again, as macOS's appearance changed: every window drawn with them.
pub fn system_changed(dark: bool, cx: &mut gpui::App) {
    system_is_dark(dark);
    in_force(&get());
    cx.refresh_windows();
}

/// The settings in force.
pub fn get() -> Prefs {
    CURRENT.lock().unwrap().clone().unwrap_or_default()
}

/// Reads the person's file and puts its settings in force; at start.
pub fn start(cx: &gpui::App) {
    let p = load(&path());
    system_is_dark(matches!(cx.window_appearance(), gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark));
    in_force(&p);
    *CURRENT.lock().unwrap() = Some(p);
}

/// Changes one setting: in force at once, every window drawn again, and kept in the person's file.
pub fn change(name: &str, value: &str, cx: &mut gpui::App) -> Result<(), String> {
    let mut p = get();
    set(&mut p, name, value)?;
    in_force(&p);
    save(&path(), &p).map_err(|e| format!("cannot keep the settings in {}: {e}", path().display()))?;
    *CURRENT.lock().unwrap() = Some(p);
    cx.refresh_windows();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Prefs, View, load, new_agent_argv, onto, save, set, zoom_view, zoomed};

    // Alan, 20/11: "quero zoom e outzoom no app também"; then "o principal zoom é pra ver as mudanças, comparações etc"
    // (the comparisons' pictures zoom in the viewer, the desktop-engineer's)
    #[test]
    fn asked_zoom_in_and_out_in_the_app_mainly_to_see_the_changes() {
        // the changes zoom where they are read: the stacked diffs, and Changes with its message in hand; the rest is
        // the terminal's font
        assert_eq!(zoom_view(true, false), View::Changes);
        assert_eq!(zoom_view(false, true), View::Changes);
        assert_eq!(zoom_view(false, false), View::Terminal);
        let mut p = Prefs::default();
        assert_eq!(p.changes_zoom, 1.0);
        // Changes: a tenth a step, between 0.7 and 2
        assert_eq!(zoomed(View::Changes, &p, 1), ("changes_zoom", "1.1".to_string()));
        set(&mut p, "changes_zoom", "1.9").unwrap();
        assert_eq!(zoomed(View::Changes, &p, 1), ("changes_zoom", "2".to_string()));
        assert_eq!(zoomed(View::Changes, &{ let mut q = p.clone(); q.changes_zoom = 2.0; q }, 1), ("changes_zoom", "2".to_string()), "held at its top");
        assert_eq!(zoomed(View::Changes, &p, 0), ("changes_zoom", "1".to_string()));
        // a number out of reach is held in it, as the others are
        let mut big = p.clone();
        set(&mut big, "changes_zoom", "9").unwrap();
        assert_eq!(big.changes_zoom, 2.0);
        // the terminal: a point a step of its font, the same one Settings shows; Command-0 its own size back
        p.terminal_font_size = 15.0;
        assert_eq!(zoomed(View::Terminal, &p, -1), ("terminal_font_size", "14".to_string()));
        assert_eq!(zoomed(View::Terminal, &p, 0), ("terminal_font_size", Prefs::default().terminal_font_size.to_string()));
        // each view its own: a step of one leaves the other
        assert_eq!(p.changes_zoom, 1.9);
    }
    use crate::settings::{OptionAsAlt, Settings};

    fn dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("jaynes-prefs-{}-{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn rand_suffix() -> u128 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    }

    #[test]
    fn a_missing_or_broken_file_is_the_defaults_and_an_unknown_field_is_skipped() {
        let d = dir();
        assert_eq!(load(&d.join("none.json")), Prefs::default());
        std::fs::write(d.join("broken.json"), "{ not json").unwrap();
        assert_eq!(load(&d.join("broken.json")), Prefs::default());
        std::fs::write(d.join("odd.json"), r#"{"terminal_font_size": 16, "from_a_later_version": true, "copy_on_select": "yes"}"#).unwrap();
        // a field of the wrong kind drops the file to the defaults only for that field
        let p = load(&d.join("odd.json"));
        assert_eq!(p.terminal_font_size, 16.0);
        assert!(p.copy_on_select);
    }

    #[test]
    fn values_out_of_reach_come_back_inside_it() {
        let d = dir();
        std::fs::write(d.join("p.json"), r#"{"terminal_font_size": 400, "scroll_speed": 0, "terminal_colors": "neon", "new_agent": "  "}"#).unwrap();
        let p = load(&d.join("p.json"));
        assert_eq!(p.terminal_font_size, 72.0);
        assert_eq!(p.scroll_speed, 0.1);
        assert_eq!(p.terminal_colors, "jaynes");
        assert_eq!(p.new_agent, "claude");
    }

    #[test]
    fn what_is_saved_is_read_back() {
        let d = dir();
        let path = d.join("deeper/app-settings.json");
        let p = Prefs { terminal_font: "JetBrains Mono".into(), option_key: OptionAsAlt::Left, notifications: false, ..Prefs::default() };
        save(&path, &p).unwrap();
        assert_eq!(load(&path), p);
    }

    #[test]
    fn a_setting_changes_by_its_name() {
        let mut p = Prefs::default();
        set(&mut p, "terminal_font_size", "16").unwrap();
        set(&mut p, "option_key", "left").unwrap();
        set(&mut p, "notifications", "off").unwrap();
        set(&mut p, "terminal_colors", "moss").unwrap();
        set(&mut p, "editor", "hx").unwrap();
        assert_eq!((p.terminal_font_size, p.option_key, p.notifications, p.terminal_colors.as_str(), p.editor.as_str()), (16.0, OptionAsAlt::Left, false, "moss", "hx"));
        assert!(set(&mut p, "terminal_colors", "neon").is_err());
        assert!(set(&mut p, "nonsense", "1").is_err());
        assert!(set(&mut p, "terminal_font_size", "big").is_err());
    }

    #[test]
    fn the_terminal_draws_with_them() {
        let p = Prefs { terminal_colors: "ink".into(), terminal_font_size: 15.0, copy_on_select: false, scroll_speed: 2.0, option_key: OptionAsAlt::Off, ..Prefs::default() };
        let s = onto(&p, Settings::default());
        assert_eq!((s.theme.as_str(), s.font_size, s.copy_on_select, s.terminal_scroll_speed, s.option_as_alt), ("ink", 15.0, false, 2.0, OptionAsAlt::Off));
        // the rest stays as it was
        assert_eq!(s.terminal_padding, Settings::default().terminal_padding);
    }

    // Alan, 18/11 (decided by him, as the board keeps it): the light theme is option 2, Linen, with the line
    // "Appearance" (Dark, Light, Match the system)
    #[test]
    fn asked_appearance_dark_light_or_match_the_system() {
        use super::{Appearance, light_now};
        let mut p = Prefs::default();
        assert_eq!(p.appearance, Appearance::Dark);
        assert!(!light_now(p.appearance, false) && !light_now(p.appearance, true));
        set(&mut p, "appearance", "light").unwrap();
        assert!(light_now(p.appearance, true));
        set(&mut p, "appearance", "system").unwrap();
        assert!(light_now(p.appearance, false) && !light_now(p.appearance, true));
        assert!(set(&mut p, "appearance", "sepia").is_err());
    }

    // Alan, 18/11, the same decision: the light theme is Linen
    #[test]
    fn asked_a_light_theme_linen_and_in_the_dark_the_colors_picked() {
        use super::onto_for;
        let p = Prefs { terminal_colors: "moss".into(), ..Prefs::default() };
        assert_eq!(onto_for(&p, Settings::default(), true).theme, "linen");
        assert_eq!(onto_for(&p, Settings::default(), false).theme, "moss");
        assert!(crate::settings::swatch("linen").is_some());
        // the window's colours go with it, and the dark is the one it always had
        assert_eq!(crate::look::for_light(false), crate::look::DARK);
        assert_eq!(crate::look::for_light(true).panel, 0xfbfaf7);
    }

    #[test]
    fn every_color_offered_has_a_swatch() {
        for c in super::COLORS {
            assert!(crate::settings::swatch(c).is_some(), "{c}");
        }
        assert_eq!(crate::settings::swatch("jaynes").unwrap()[0], 0x0b0c0f);
        assert!(crate::settings::swatch("neon").is_none());
    }

    #[test]
    fn a_new_agent_is_its_program_or_a_command_line_for_the_login_shell() {
        assert_eq!(new_agent_argv("claude"), ["claude"]);
        let argv = new_agent_argv("claude --model opus");
        assert_eq!(argv[1..], ["-lic".to_owned(), "exec claude --model opus".to_owned()]);
    }
}
