//! The window's colours and the small pieces the list and the steps panel share: one dark for the whole window, the
//! terminal's included (the theme `jaynes` in settings.rs has the same background): the near-black of direction C of
//! the top bar, after T3 Code's quiet.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use jaynes_shell::listing::Mark;

/// The window's colours: one set for all of it, the frame round the terminal included. The dark one is the default;
/// a theme puts another in force (`set`), and the next frame draws with it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub panel: u32,
    pub panel_2: u32,
    pub selected: u32,
    pub line: u32,
    pub line_2: u32,
    pub text: u32,
    pub dim: u32,
    pub faint: u32,
    pub amber: u32,
    pub blue: u32,
    pub green: u32,
    pub red: u32,
    pub grey: u32,
    /// An agent that finished a turn the person has not looked at.
    pub violet: u32,
    /// Text on an amber or green fill.
    pub on_fill: u32,
    /// The column at the left and the status bar.
    pub side: u32,
    /// Under the pointer, and fainter (RGBA).
    pub hover: u32,
    pub hover_faint: u32,
    /// Behind a picture shown large, and behind the list of shortcuts (RGBA).
    pub scrim: u32,
    pub scrim_2: u32,
    /// A run's story, as its agent told it.
    pub story: u32,
    /// The ring of an agent that is idle.
    pub idle_ring: u32,
}

/// The dark the window has always had.
pub const DARK: Frame = Frame {
    panel: 0x0b0c0f,
    panel_2: 0x131519,
    selected: 0x181a20,
    line: 0x1b1d23,
    line_2: 0x262931,
    text: 0xe9ecf1,
    dim: 0x9298a5,
    faint: 0x5d6370,
    amber: 0xffb35c,
    blue: 0x6fb4ff,
    green: 0x69dba0,
    red: 0xff7a7a,
    grey: 0x7d8899,
    violet: 0xc49bff,
    on_fill: 0x0b0e13,
    side: 0x0b0c0f,
    hover: 0xffffff14,
    hover_faint: 0xffffff05,
    scrim: 0x05070acc,
    scrim_2: 0x0000_0099,
    story: 0xc9ced8,
    idle_ring: 0x4a505c,
};

/// The light: Linen, a warm off-white.
pub const LINEN: Frame = Frame {
    panel: 0xfbfaf7,
    panel_2: 0xf3f1ec,
    selected: 0xe8e4dc,
    line: 0xe6e2d9,
    line_2: 0xd6d0c4,
    text: 0x24221e,
    dim: 0x5e5a52,
    faint: 0x8f897d,
    amber: 0x975f00,
    blue: 0x2f63c8,
    green: 0x2c7a3f,
    red: 0xb8412f,
    grey: 0x7d786e,
    violet: 0x7a4fc2,
    on_fill: 0xffffff,
    side: 0xf3f1ec,
    hover: 0x24221e12,
    hover_faint: 0x24221e08,
    scrim: 0x24221e66,
    scrim_2: 0x24221e4d,
    story: 0x3a3730,
    idle_ring: 0xb9b2a5,
};

/// The window's colours for the light or the dark.
pub fn for_light(light: bool) -> Frame {
    if light { LINEN } else { DARK }
}

thread_local! {
    static FRAME: std::cell::Cell<Frame> = const { std::cell::Cell::new(DARK) };
}

/// The colours in force.
pub fn frame() -> Frame {
    FRAME.with(|f| f.get())
}

/// Puts other colours in force; the windows show them when next drawn.
pub fn set(f: Frame) {
    FRAME.with(|c| c.set(f));
}

pub fn panel() -> u32 { frame().panel }
pub fn panel_2() -> u32 { frame().panel_2 }
pub fn selected() -> u32 { frame().selected }
pub fn line() -> u32 { frame().line }
pub fn line_2() -> u32 { frame().line_2 }
pub fn text() -> u32 { frame().text }
pub fn dim() -> u32 { frame().dim }
pub fn faint() -> u32 { frame().faint }
pub fn amber() -> u32 { frame().amber }
pub fn blue() -> u32 { frame().blue }
pub fn green() -> u32 { frame().green }
pub fn red() -> u32 { frame().red }
pub fn grey() -> u32 { frame().grey }
pub fn violet() -> u32 { frame().violet }
pub fn on_fill() -> u32 { frame().on_fill }
pub fn side() -> u32 { frame().side }
pub fn hover() -> u32 { frame().hover }
pub fn hover_faint() -> u32 { frame().hover_faint }
pub fn scrim() -> u32 { frame().scrim }
pub fn scrim_2() -> u32 { frame().scrim_2 }
pub fn story() -> u32 { frame().story }
pub fn idle_ring() -> u32 { frame().idle_ring }

pub const UI_FONT: &str = "Instrument Sans";
pub const MONO_FONT: &str = "IBM Plex Mono";

pub fn colour(mark: Mark) -> u32 {
    match mark {
        Mark::Needs => amber(),
        Mark::Working => blue(),
        Mark::Halted => grey(),
        Mark::Blocked | Mark::Undone => red(),
        Mark::Delivered | Mark::Merged => green(),
        Mark::Abandoned => faint(),
    }
}

pub fn word(mark: Mark) -> &'static str {
    match mark {
        Mark::Needs => "needs you",
        Mark::Working => "working",
        Mark::Halted => "halted",
        Mark::Blocked => "blocked",
        Mark::Delivered => "delivered",
        Mark::Merged => "merged",
        Mark::Undone => "undone",
        Mark::Abandoned => "abandoned",
    }
}

/// A small heading, as written: quiet, after T3 Code, not shouted in capitals.
/// A name shown over what the pointer rests on: the rail's things, which show no name of their own.
pub struct Tip(SharedString);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(rgb(panel()))
            .border_1()
            .border_color(rgb(line()))
            .font_family(UI_FONT)
            .text_size(px(12.))
            .text_color(rgb(text()))
            .child(self.0.clone())
    }
}

/// The tooltip that says `text`.
pub fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| cx.new(|_| Tip(text.clone())).into()
}

pub fn label(text: &str, color: u32) -> Div {
    div().text_size(px(11.5)).font_weight(FontWeight::MEDIUM).text_color(rgb(color)).child(text.to_string())
}

/// The mark as a chip: its word on a soft fill of its colour.
pub fn chip(mark: Mark) -> Div {
    let c = colour(mark);
    div()
        .px(px(8.))
        .py(px(2.))
        .rounded(px(999.))
        .text_size(px(11.5))
        .text_color(rgb(c))
        .bg(rgba((c << 8) | 0x22))
        .child(word(mark))
}

/// A ring with a turning arc: something works. Turns once in 0.9 s; still when the person asked for less motion.
pub fn spinner(id: impl Into<ElementId>, size: f32, color: u32) -> impl IntoElement {
    let id = id.into();
    ring(size, color, 0.).with_animation(id, Animation::new(std::time::Duration::from_millis(900)).repeat(), move |_, t| ring(size, color, t))
}

/// The spinner's picture at a turn `t` (0..1): a faint full circle and a quarter of it bright.
fn ring(size: f32, color: u32, t: f32) -> Canvas<()> {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let r = size / 2. - 1.;
            let c = point(b.origin.x + px(size / 2.), b.origin.y + px(size / 2.));
            let at = |turn: f32| {
                let a = turn * std::f32::consts::TAU;
                point(c.x + px(r * a.cos()), c.y + px(r * a.sin()))
            };
            let mut faint = PathBuilder::stroke(px(1.6));
            faint.move_to(at(0.));
            faint.arc_to(point(px(r), px(r)), px(0.), true, true, at(0.5));
            faint.arc_to(point(px(r), px(r)), px(0.), true, true, at(0.));
            if let Ok(p) = faint.build() {
                window.paint_path(p, rgba((color << 8) | 0x40));
            }
            let mut arc = PathBuilder::stroke(px(1.6));
            arc.move_to(at(t));
            arc.arc_to(point(px(r), px(r)), px(0.), false, true, at(t + 0.28));
            if let Ok(p) = arc.build() {
                window.paint_path(p, rgb(color));
            }
        },
    )
    .size(px(size))
    .flex_none()
}

/// A dot that breathes: the step at work now.
pub fn breathing(id: impl Into<ElementId>, size: f32, color: u32) -> impl IntoElement {
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .bg(rgb(color))
        .with_animation(id, Animation::new(std::time::Duration::from_millis(1600)).repeat().with_easing(ease_in_out), |d, t| {
            d.opacity(1. - 0.7 * (t * std::f32::consts::PI).sin())
        })
}

/// The app's mark in the top bar: the favicon's giraffe without its tile, one file a look, its ink the look's text and
/// the cut of its eye the column's colour (assets/mark-bar-dark.svg and mark-bar-light.svg, the design-lead's, sized at
/// four times the 27 points of their box so the picture GPUI draws from them stays sharp at this height).
pub fn mark_icon(height: f32) -> Img {
    use std::sync::{Arc, OnceLock};
    static DARK: OnceLock<Arc<Image>> = OnceLock::new();
    static LIGHT: OnceLock<Arc<Image>> = OnceLock::new();
    let mark = if frame() == LINEN {
        LIGHT.get_or_init(|| Arc::new(Image::from_bytes(ImageFormat::Svg, include_bytes!("../assets/mark-bar-light.svg").to_vec())))
    } else {
        DARK.get_or_init(|| Arc::new(Image::from_bytes(ImageFormat::Svg, include_bytes!("../assets/mark-bar-dark.svg").to_vec())))
    };
    img(mark.clone()).h(px(height)).w(px(height * 21.6 / 27.)).flex_none()
}

/// A quiet button with its key, as the top bar shows them.
pub fn key_button(id: &'static str, label: &str, key: &str) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(9.))
        .py(px(4.))
        .rounded(px(7.))
        .border_1()
        .border_color(rgb(line_2()))
        .text_size(px(12.5))
        .text_color(rgb(dim()))
        .cursor_pointer()
        .hover(|d| d.bg(rgb(selected())).text_color(rgb(text())))
        .child(label.to_string())
        .when(!key.is_empty(), |d| d.child(kbd(key)))
}

/// A key, drawn small in its box.
pub fn kbd(key: &str) -> Div {
    div().px(px(5.)).rounded(px(5.)).border_1().border_color(rgb(line_2())).font_family(MONO_FONT).text_size(px(10.5)).text_color(rgb(faint())).child(key.to_string())
}

/// The window's small line icons, drawn in the colour of the text around them.
#[derive(Clone, Copy)]
pub enum Icon {
    /// The runs list's switch: a window with its left column.
    Sidebar,
    /// The steps panel's switch: a window with its right column.
    Panel,
    Plus,
    Folder,
    Branch,
    Stop,
    Inbox,
    /// Takes a file out of the next commit.
    Minus,
    /// Opens a file elsewhere: an arrow out of a box.
    Open,
    Close,
    /// A terminal: a window with a prompt. Settings' Terminal section, and the tab of the person's own Claude Code.
    Terminal,
    /// Settings, at the left of the status bar: a gear of eight teeth.
    Gear,
    /// A picture shown large: two arrows out to opposite corners.
    Enlarge,
}

impl Icon {
    pub fn svg(self) -> &'static str {
        match self {
            Icon::Sidebar => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="1.7" y="2.7" width="12.6" height="10.6" rx="2"/><path d="M6 2.7v10.6"/></svg>"##,
            Icon::Panel => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="1.7" y="2.7" width="12.6" height="10.6" rx="2"/><path d="M10 2.7v10.6"/></svg>"##,
            Icon::Plus => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.5" stroke-linecap="round"><path d="M8 3v10M3 8h10"/></svg>"##,
            Icon::Folder => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.4" stroke-linejoin="round"><path d="M1.7 4.5a1 1 0 0 1 1-1h3.4l1.5 1.5h5.7a1 1 0 0 1 1 1v6.3a1 1 0 0 1-1 1H2.7a1 1 0 0 1-1-1z"/></svg>"##,
            Icon::Branch => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.4" stroke-linecap="round"><circle cx="4.5" cy="3.5" r="1.6"/><circle cx="4.5" cy="12.5" r="1.6"/><circle cx="11.5" cy="5" r="1.6"/><path d="M4.5 5.1v5.8M11.5 6.6c0 3-7 2-7 4.3"/></svg>"##,
            Icon::Inbox => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.4" stroke-linejoin="round"><path d="M2 9.5h3.2l1 1.8h3.6l1-1.8H14"/><path d="M3.6 3h8.8L14 9.5V13H2V9.5z"/></svg>"##,
            Icon::Minus => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.2" stroke-linecap="round"><path d="M3 8h10"/></svg>"##,
            Icon::Open => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 3h4v4M13 3L7.5 8.5M11 9.5V13H3V5h3.5"/></svg>"##,
            Icon::Close => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.2" stroke-linecap="round"><path d="M4 4l8 8M12 4l-8 8"/></svg>"##,
            Icon::Terminal => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M7 9l3 3-3 3M12 15h5"/></svg>"##,
            Icon::Gear => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.3" stroke-linejoin="round"><path d="M12.69 6.57L14.48 6.74L14.48 9.26L12.69 9.43L12.33 10.3L13.47 11.69L11.69 13.47L10.3 12.33L9.43 12.69L9.26 14.48L6.74 14.48L6.57 12.69L5.7 12.33L4.31 13.47L2.53 11.69L3.67 10.3L3.31 9.43L1.52 9.26L1.52 6.74L3.31 6.57L3.67 5.7L2.53 4.31L4.31 2.53L5.7 3.67L6.57 3.31L6.74 1.52L9.26 1.52L9.43 3.31L10.3 3.67L11.69 2.53L13.47 4.31L12.33 5.7Z"/><circle cx="8" cy="8" r="2.1"/></svg>"##,
            Icon::Enlarge => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M9.5 2.5h4v4M13.5 2.5L9 7M6.5 13.5h-4v-4M2.5 13.5L7 9"/></svg>"##,
            Icon::Stop => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.5" stroke-linejoin="round"><rect x="4" y="4" width="8" height="8" rx="1.6"/></svg>"##,
        }
    }
}

/// An icon at a size, in a colour.
pub fn icon(which: Icon, size: f32, color: u32) -> Svg {
    svg().data(which.svg().as_bytes()).size(px(size)).flex_none().text_color(rgb(color))
}

/// An icon that is a button: a quiet square that lights up under the pointer, bright when what it switches is on.
pub fn icon_button(id: &'static str, which: Icon, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(26.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|d| d.bg(rgb(selected())))
        .child(icon(which, 16., if on { text() } else { dim() }))
}

/// The tab of the person's own Claude Code: a terminal, bright on the tab that is open.
pub fn main_tab_icon(on: bool) -> Svg {
    icon(Icon::Terminal, 14., if on { text() } else { dim() })
}
