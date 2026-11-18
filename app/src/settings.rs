//! JAYNES: copied from herdr-app's src/settings.rs (Apache-2.0) and changed: only the live settings and the theme
//! the terminal draws with are kept (the built-in palettes, fonts, zoom); settings.json, theme files and layouts are gone.
//!
//! A theme is a palette plus optional color overrides. Chrome colors are derived from the palette (dark or light by
//! its background), so every theme is consistent and overrides only need what they change.

use crate::vt::Rgb;
use gpui::{Hsla, Pixels, Rgba, px};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Mutex;

/// A terminal palette, as herdr-app shipped them.
#[derive(Clone)]
struct Palette {
    background: u32,
    foreground: u32,
    selection: u32,
    cursor: Option<u32>,
    ansi: [u32; 16],
    /// working, blocked, done: the presets pick their own; theme files take
    /// them from ansi (yellow, red, blue). Idle is grey: it needs nobody.
    status: Option<[u32; 3]>,
}

const DARK_STATUS: Option<[u32; 3]> = Some([0xe5c07b, 0xf0705f, 0x76a8ff]);

const GRAPHITE: Palette = Palette {
    background: 0x17191d,
    foreground: 0xd2d4d9,
    selection: 0x394454,
    cursor: None,
    status: DARK_STATUS,
    ansi: [
        0x45484f, 0xdc8c95, 0xa6bd91, 0xd8be87, 0x96b2d7, 0xb5a2ce, 0x8cbdc3, 0xd2d4d9, 0x818793, 0xf0a5ad, 0xbad5a2, 0xead2a1, 0xb0cbef,
        0xcbb7e6, 0xa4d5db, 0xf0f0f2,
    ],
};
/// JAYNES: the Jaynes window's own dark: graphite's colours on the background the panels have (look.rs), so the
/// terminal and the panels are one surface.
const JAYNES: Palette = Palette {
    background: 0x0b0c0f,
    foreground: 0xd6d9e0,
    selection: 0x2b3444,
    cursor: None,
    status: DARK_STATUS,
    ansi: GRAPHITE.ansi,
};
const INK: Palette = Palette {
    background: 0x111720,
    foreground: 0xcdd6e4,
    selection: 0x344861,
    cursor: None,
    status: DARK_STATUS,
    ansi: [
        0x414b5a, 0xe58c9a, 0x89c5aa, 0xd9be82, 0x91b6ee, 0xb9a0e2, 0x85c4d0, 0xcdd6e4, 0x7f91a7, 0xf5a5b2, 0xa5dec3, 0xedd39c, 0xaccbfa,
        0xd0b7f4, 0xa0dce5, 0xf0f5fc,
    ],
};
const MOSS: Palette = Palette {
    background: 0x151c19,
    foreground: 0xcdd7ce,
    selection: 0x354a40,
    cursor: None,
    status: DARK_STATUS,
    ansi: [
        0x435248, 0xdf978d, 0xa0c4a2, 0xd4bc8b, 0x99b9cd, 0xbba4c8, 0x8bbdb5, 0xcdd7ce, 0x82968a, 0xf1afa3, 0xb8dab9, 0xe9d2a5, 0xb1d0e2,
        0xd1bddc, 0xa4d6ca, 0xeff5ee,
    ],
};

/// Light: white paper, dark ink. Every ANSI color stays readable on white
/// (bright white is ink, not white: programs write it as "strong" text).
const PAPER: Palette = Palette {
    background: 0xffffff,
    foreground: 0x25272c,
    selection: 0xcfdcf5,
    cursor: Some(0x25272c),
    status: Some([0x9a6700, 0xc93c2c, 0x2f6fe4]),
    ansi: [
        0x25272c, 0xc62a2a, 0x1a7f37, 0x8f5d00, 0x1f62d0, 0x7a45c8, 0x0f6f74, 0x5f6068, 0x7b7e87, 0xd1242f, 0x1f883d, 0x9a6700, 0x0969da,
        0x8250df, 0x1b7c83, 0x111214,
    ],
};

pub const PRESETS: &[&str] = &["jaynes", "graphite", "ink", "moss", "paper"];

/// The theme a layout without one shows, by appearance.
pub const DEFAULT_DARK: &str = "jaynes";
pub const DEFAULT_LIGHT: &str = "paper";

fn preset(name: &str) -> Option<&'static Palette> {
    match name {
        "jaynes" => Some(&JAYNES),
        "graphite" => Some(&GRAPHITE),
        "ink" => Some(&INK),
        "moss" => Some(&MOSS),
        "paper" => Some(&PAPER),
        _ => None,
    }
}


fn is_light(c: u32) -> bool {
    let ch = |s: u32| ((c >> s) & 255) as f32 / 255.0;
    0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0) > 0.5
}

/// Every theme that can be shown, presets first, each with its colors
/// (the settings screen draws them as swatches).

fn hex(c: u32) -> String {
    format!("#{c:06x}")
}

fn mix(a: u32, b: u32, t: f32) -> u32 {
    [16, 8, 0].iter().fold(0, |acc, s| {
        let (x, y) = (((a >> s) & 255) as f32, ((b >> s) & 255) as f32);
        acc | (((x * (1.0 - t) + y * t).round() as u32) << s)
    })
}

/// Every color the app and mods use, resolved. Values are `#rrggbb` or `#rrggbbaa`.
pub type Theme = BTreeMap<String, String>;

/// The panel colors around a palette. `chrome` (sidebars, bars) and
/// `canvas` (behind cards) step away from the terminal's background:
/// darker on a dark theme, toward warm grey on a light one.
fn resolve(p: &Palette, overrides: &BTreeMap<String, String>) -> Theme {
    let bg = p.background;
    let light = is_light(bg);
    let fg = if light { 0x1b1b1e } else { 0xeeeeef };
    let [working, blocked, done] = p.status.unwrap_or([p.ansi[3], p.ansi[1], p.ansi[4]]);
    let (popover, raised, hover, selected, border, input, canvas, chrome) = if light {
        let warm = 0xe8e8e2;
        (hex(bg), hex(mix(bg, 0, 0.035)), "#0000000c".into(), "#00000012".into(), "#00000017".into(), "#00000026".into(), mix(bg, warm, 0.2), mix(bg, warm, 0.6))
    } else {
        (
            hex(mix(bg, 0xffffff, 0.06)),
            hex(mix(bg, 0xffffff, 0.09)),
            "#ffffff0e".into(),
            "#ffffff17".into(),
            "#ffffff18".into(),
            "#ffffff24".into(),
            mix(bg, 0, 0.16),
            mix(bg, 0, 0.32),
        )
    };
    let mut t: Theme = [
        ("background", hex(bg)),
        ("terminal_foreground", hex(p.foreground)),
        ("foreground", hex(fg)),
        ("surface", hex(bg)),
        ("canvas", hex(canvas)),
        ("chrome", hex(chrome)),
        ("popover", popover),
        ("raised", raised),
        ("hover", hover),
        ("selected", selected),
        ("border", border),
        ("input", input),
        ("muted", hex(mix(bg, fg, if light { 0.66 } else { 0.6 }))),
        ("dim", hex(mix(bg, fg, if light { 0.46 } else { 0.38 }))),
        ("accent", hex(p.ansi[4])),
        ("accent_foreground", hex(bg)),
        ("solid", hex(fg)),
        ("solid_foreground", hex(if light { 0xffffff } else { mix(bg, 0, 0.28) })),
        ("ring", hex(mix(bg, fg, 0.7))),
        ("selection", hex(p.selection)),
        ("cursor", hex(p.cursor.unwrap_or(p.foreground))),
        ("working", hex(working)),
        ("blocked", hex(blocked)),
        ("done", hex(done)),
        ("idle", hex(mix(bg, fg, 0.38))),
        ("unknown", hex(mix(bg, fg, 0.38))),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    for (i, c) in p.ansi.iter().enumerate() {
        t.insert(format!("ansi{i}"), hex(*c));
    }
    for (k, v) in overrides {
        t.insert(k.clone(), v.clone());
    }
    t
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OptionAsAlt {
    #[default]
    Auto,
    Off,
    On,
    Left,
    Right,
}

impl OptionAsAlt {
    fn is_auto(&self) -> bool {
        *self == Self::Auto
    }
}


#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub font_family: String,
    pub font_size: f32,
    /// The terminals' text weight (bold text stays at 700).
    pub font_weight: u16,
    pub font_ligatures: bool,
    /// Cell height multiplier, 0.8..2.
    pub terminal_line_height: f32,
    /// Cell width multiplier, 0.8..2.
    pub terminal_cell_width: f32,
    /// Wheel and trackpad scroll multiplier, 0.1..5.
    pub terminal_scroll_speed: f32,
    pub ui_font_family: String,
    pub ui_font_size: f32,
    /// How big the whole app is drawn (1 = as designed). The zoom keys change it.
    pub zoom: f32,
    /// The theme showing: a preset or a file in themes/.
    pub theme: String,
    /// The layout's theme for each appearance (the same one twice when it names one).
    pub theme_light: String,
    pub theme_dark: String,
    /// Whether the window is dark now (the person's `appearance`, else the system's).
    pub dark: bool,
    /// Any theme color by name (see `theme()` in herdr.d.ts), `#rrggbb` or `#rrggbbaa`.
    pub colors: BTreeMap<String, String>,
    /// A draggable scroll position thumb at a terminal pane's right edge.
    pub pane_scrollbars: bool,
    /// Space between a terminal's edge and its text, in px: [x, y].
    pub terminal_padding: [f32; 2],
    /// 0..1: below 1, what's behind a terminal shows through.
    pub terminal_opacity: f32,
    pub clipboard_write: bool,
    pub copy_on_select: bool,
    pub minimum_contrast: f32,
    pub option_as_alt: OptionAsAlt,
    pub sounds: Sounds,
}

/// Attention sounds: when an agent needs you, and when one finished while you
/// weren't looking. `done` / `blocked`: an MP3 file instead of the bundled one
/// (absolute, or relative to ~/.config/herdr-app).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sounds {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
}

impl Default for Sounds {
    fn default() -> Self {
        Sounds { enabled: true, done: None, blocked: None }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            font_family: DEFAULT_FONT.into(),
            font_size: 14.0,
            font_weight: 500,
            font_ligatures: false,
            terminal_line_height: 1.0,
            terminal_cell_width: 1.0,
            terminal_scroll_speed: 1.0,
            ui_font_family: "Instrument Sans".into(),
            ui_font_size: 13.0,
            zoom: 1.0,
            theme: DEFAULT_DARK.into(),
            theme_light: DEFAULT_LIGHT.into(),
            theme_dark: DEFAULT_DARK.into(),
            dark: true,
            colors: BTreeMap::new(),
            pane_scrollbars: true,
            terminal_padding: [10.0, 6.0],
            terminal_opacity: 1.0,
            clipboard_write: true,
            copy_on_select: true,
            minimum_contrast: 1.0,
            option_as_alt: OptionAsAlt::Auto,
            sounds: Sounds::default(),
        }
    }
}


pub fn parse_hex(s: &str) -> Option<Hsla> {
    let h = s.trim().trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).ok()?;
    let (rgb, a) = match h.len() {
        6 => (v, 255),
        8 => (v >> 8, v & 255),
        _ => return None,
    };
    Some(
        Rgba {
            r: ((rgb >> 16) & 255) as f32 / 255.0,
            g: ((rgb >> 8) & 255) as f32 / 255.0,
            b: (rgb & 255) as f32 / 255.0,
            a: a as f32 / 255.0,
        }
        .into(),
    )
}

pub fn rgb(s: &str) -> Rgb {
    let h = s.trim().trim_start_matches('#');
    let v = u32::from_str_radix(&h[..h.len().min(6)], 16).unwrap_or(0xff00ff);
    Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

thread_local! {
    static CURRENT: RefCell<(Rc<Settings>, Rc<Theme>)> =
        RefCell::new((Rc::new(Settings::default()), Rc::new(resolve(&JAYNES, &BTreeMap::new()))));
}

const ZOOM_MIN: f32 = 0.5;
const ZOOM_MAX: f32 = 3.0;

pub fn get() -> Rc<Settings> {
    CURRENT.with(|c| c.borrow().0.clone())
}

/// The resolved theme: every color by name.
pub fn theme() -> Rc<Theme> {
    CURRENT.with(|c| c.borrow().1.clone())
}

/// A theme color as GPUI paint (magenta if missing, so mistakes show).
pub fn color(name: &str) -> Hsla {
    theme().get(name).and_then(|v| parse_hex(v)).unwrap_or(gpui::hsla(0.83, 1.0, 0.5, 1.0))
}

/// A theme color name, or a literal `#rrggbb[aa]`.
pub fn color_or_literal(v: &str) -> Hsla {
    parse_hex(v).unwrap_or_else(|| color(v))
}

/// Reads settings.json (writing the defaults if it's missing), or says why it can't be used.

/// Installs new settings; returns whether anything changed.
pub fn set(s: Settings) -> bool {
    #[cfg(target_os = "macos")]
    gpui_macos::set_option_as_alt(match s.option_as_alt {
        OptionAsAlt::Auto => None,
        OptionAsAlt::Off => Some(gpui_macos::OptionAsAlt::None),
        OptionAsAlt::On => Some(gpui_macos::OptionAsAlt::Both),
        OptionAsAlt::Left => Some(gpui_macos::OptionAsAlt::Left),
        OptionAsAlt::Right => Some(gpui_macos::OptionAsAlt::Right),
    });
    CURRENT.with(|c| {
        // Read the theme even when the settings are the same: its file may
        // have new colors under the same name.
        let p = preset(&s.theme).cloned().unwrap_or_else(|| JAYNES.clone());
        let theme = resolve(&p, &s.colors);
        if *c.borrow().0 == s && *c.borrow().1 == theme {
            return false;
        }
        *c.borrow_mut() = (Rc::new(s), Rc::new(theme));
        true
    })
}

pub fn next_zoom(steps: i32) -> f32 {
    if steps == 0 {
        return 1.0;
    }
    ((zoom() * 10.0).round() + steps as f32).clamp(ZOOM_MIN * 10.0, ZOOM_MAX * 10.0) / 10.0
}

pub fn zoom() -> f32 {
    get().zoom
}

/// A designed size in px, at the current zoom.
pub fn z(v: f32) -> Pixels {
    px(v * zoom())
}

pub fn font_size() -> f32 {
    get().font_size * zoom()
}

pub fn ui_font_size() -> f32 {
    get().ui_font_size * zoom()
}

/// Font families on this machine (the bundled ones and the home's included).
static FONTS: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);

/// Whether this machine has a font family (as the text system listed them at start); true before the list comes.
pub fn has_font(family: &str) -> bool {
    FONTS.lock().unwrap().as_ref().is_none_or(|have| have.contains(&family.to_lowercase()))
}

/// Three colors that stand for a preset in a swatch: its background, its blue and its green.
pub fn swatch(name: &str) -> Option<[u32; 3]> {
    preset(name).map(|p| [p.background, p.ansi[4], p.ansi[2]])
}

pub fn note_fonts(names: Vec<String>) {
    *FONTS.lock().unwrap() = Some(names.into_iter().map(|n| n.to_lowercase()).collect());
}


static FONTS_ADDED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many times fonts were added while running: text shaped before may now
/// fall back to other glyphs.
pub fn fonts_added() -> u64 {
    FONTS_ADDED.load(std::sync::atomic::Ordering::Relaxed)
}

/// The terminal font, or the bundled default when this machine lacks it:
/// the platform's own fallback is proportional and breaks the grid.
pub fn terminal_font() -> String {
    let family = get().font_family.clone();
    match FONTS.lock().unwrap().as_ref() {
        Some(have) if !have.contains(&family.to_lowercase()) => DEFAULT_FONT.into(),
        _ => family,
    }
}

/// Fonts Herdr ships, so the defaults look the same everywhere.
pub const BUNDLED_FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/InstrumentSans-Variable.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-SemiBold.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Bold.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Italic.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Variable.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Italic-Variable.ttf"),
    include_bytes!("../assets/fonts/Geist.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Italic-Variable.ttf"),
    SYMBOLS_FONT_FILE,
];

/// The symbols font itself: terminals read icons' glyphs from it directly.
pub const SYMBOLS_FONT_FILE: &[u8] = include_bytes!("../assets/fonts/SymbolsNerdFont-Regular.ttf");

/// Nerd Font icons for terminals, under any font (MIT, nerd-fonts).
pub const SYMBOLS_FONT: &str = "Symbols Nerd Font";

/// Terminals' font unless a layout or settings.json picks another.
pub const DEFAULT_FONT: &str = "IBM Plex Mono";

