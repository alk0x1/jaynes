//! The person's settings: what the Settings screen shows, kept in a file of theirs (~/.jaynes/app-settings.json, or
//! JAYNES_APP_SETTINGS) and read at start. Each is one the app applies; a change takes effect at once.

use crate::settings::{OptionAsAlt, Settings};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The terminal colors the Settings screen offers: the dark ones, as the window around the terminal is dark.
pub const COLORS: &[&str] = &["jaynes", "graphite", "ink", "moss"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
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
}

impl Default for Prefs {
    fn default() -> Self {
        let s = Settings::default();
        Prefs {
            terminal_colors: crate::settings::DEFAULT_DARK.into(),
            terminal_font: s.font_family,
            terminal_font_size: s.font_size,
            option_key: s.option_as_alt,
            copy_on_select: s.copy_on_select,
            scroll_speed: s.terminal_scroll_speed,
            notifications: true,
            new_agent: "claude".into(),
            editor: String::new(),
        }
    }
}

const FONT_SIZE: (f32, f32) = (6.0, 72.0);
const SCROLL_SPEED: (f32, f32) = (0.1, 5.0);

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
        "notifications" => p.notifications = switch()?,
        "new_agent" if !value.is_empty() => p.new_agent = value.into(),
        "editor" => p.editor = value.into(),
        _ => return Err(format!("no setting {name} takes {value:?}")),
    }
    Ok(())
}

/// The terminal's own settings with the person's laid over them.
pub fn onto(p: &Prefs, mut s: Settings) -> Settings {
    s.theme = p.terminal_colors.clone();
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

/// The settings in force.
pub fn get() -> Prefs {
    CURRENT.lock().unwrap().clone().unwrap_or_default()
}

/// Reads the person's file and puts its settings in force; at start.
pub fn start() {
    let p = load(&path());
    crate::settings::set(onto(&p, (*crate::settings::get()).clone()));
    *CURRENT.lock().unwrap() = Some(p);
}

/// Changes one setting: in force at once, every window drawn again, and kept in the person's file.
pub fn change(name: &str, value: &str, cx: &mut gpui::App) -> Result<(), String> {
    let mut p = get();
    set(&mut p, name, value)?;
    crate::settings::set(onto(&p, (*crate::settings::get()).clone()));
    save(&path(), &p).map_err(|e| format!("cannot keep the settings in {}: {e}", path().display()))?;
    *CURRENT.lock().unwrap() = Some(p);
    cx.refresh_windows();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Prefs, load, new_agent_argv, onto, save, set};
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
        let mut p = Prefs::default();
        p.terminal_font = "JetBrains Mono".into();
        p.option_key = OptionAsAlt::Left;
        p.notifications = false;
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
        let mut p = Prefs::default();
        p.terminal_colors = "ink".into();
        p.terminal_font_size = 15.0;
        p.copy_on_select = false;
        p.scroll_speed = 2.0;
        p.option_key = OptionAsAlt::Off;
        let s = onto(&p, Settings::default());
        assert_eq!((s.theme.as_str(), s.font_size, s.copy_on_select, s.terminal_scroll_speed, s.option_as_alt), ("ink", 15.0, false, 2.0, OptionAsAlt::Off));
        // the rest stays as it was
        assert_eq!(s.terminal_padding, Settings::default().terminal_padding);
    }

    #[test]
    fn a_new_agent_is_its_program_or_a_command_line_for_the_login_shell() {
        assert_eq!(new_agent_argv("claude"), ["claude"]);
        let argv = new_agent_argv("claude --model opus");
        assert_eq!(argv[1..], ["-lic".to_owned(), "exec claude --model opus".to_owned()]);
    }
}
