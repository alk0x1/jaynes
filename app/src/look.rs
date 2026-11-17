//! The window's colours and the small pieces the list and the steps panel share: one dark for the whole window, the
//! terminal's included (the theme `jaynes` in settings.rs has the same background): the near-black of direction C of
//! the top bar (docs/prototypes/app-topbar.html), after T3 Code's quiet.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use jaynes_shell::listing::Mark;

pub const PANEL: u32 = 0x0b0c0f;
pub const PANEL_2: u32 = 0x131519;
pub const SELECTED: u32 = 0x181a20;
pub const LINE: u32 = 0x1b1d23;
pub const LINE_2: u32 = 0x262931;
pub const TEXT: u32 = 0xe9ecf1;
pub const DIM: u32 = 0x9298a5;
pub const FAINT: u32 = 0x5d6370;
pub const AMBER: u32 = 0xffb35c;
pub const BLUE: u32 = 0x6fb4ff;
pub const GREEN: u32 = 0x69dba0;
pub const RED: u32 = 0xff7a7a;
pub const GREY: u32 = 0x7d8899;
/// Text on an amber or green fill.
pub const ON_FILL: u32 = 0x0b0e13;

pub const UI_FONT: &str = "Instrument Sans";
pub const MONO_FONT: &str = "IBM Plex Mono";

pub fn colour(mark: Mark) -> u32 {
    match mark {
        Mark::Needs => AMBER,
        Mark::Working => BLUE,
        Mark::Halted => GREY,
        Mark::Blocked | Mark::Undone => RED,
        Mark::Delivered | Mark::Merged => GREEN,
        Mark::Abandoned => FAINT,
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

/// The app's mark in the top bar: the giraffe of its icon, whose neck is the J (assets/mark.svg, drawn into mark.png
/// by assets/make-icon.sh at four times this size).
pub fn mark_icon(height: f32) -> Img {
    static MARK: std::sync::OnceLock<std::sync::Arc<Image>> = std::sync::OnceLock::new();
    let mark = MARK.get_or_init(|| std::sync::Arc::new(Image::from_bytes(ImageFormat::Png, include_bytes!("../assets/mark.png").to_vec())));
    img(mark.clone()).h(px(height)).w(px(height * 67. / 88.)).flex_none()
}

/// A quiet button with its key, as the top bar shows them.
pub fn key_button(id: &'static str, text: &str, key: &str) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(9.))
        .py(px(4.))
        .rounded(px(7.))
        .border_1()
        .border_color(rgb(LINE_2))
        .text_size(px(12.5))
        .text_color(rgb(DIM))
        .cursor_pointer()
        .hover(|d| d.bg(rgb(SELECTED)).text_color(rgb(TEXT)))
        .child(text.to_string())
        .when(!key.is_empty(), |d| d.child(kbd(key)))
}

/// A key, drawn small in its box.
pub fn kbd(key: &str) -> Div {
    div().px(px(5.)).rounded(px(5.)).border_1().border_color(rgb(LINE_2)).font_family(MONO_FONT).text_size(px(10.5)).text_color(rgb(FAINT)).child(key.to_string())
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
}

impl Icon {
    fn svg(self) -> &'static str {
        match self {
            Icon::Sidebar => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="1.7" y="2.7" width="12.6" height="10.6" rx="2"/><path d="M6 2.7v10.6"/></svg>"##,
            Icon::Panel => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="1.7" y="2.7" width="12.6" height="10.6" rx="2"/><path d="M10 2.7v10.6"/></svg>"##,
            Icon::Plus => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.5" stroke-linecap="round"><path d="M8 3v10M3 8h10"/></svg>"##,
            Icon::Folder => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.4" stroke-linejoin="round"><path d="M1.7 4.5a1 1 0 0 1 1-1h3.4l1.5 1.5h5.7a1 1 0 0 1 1 1v6.3a1 1 0 0 1-1 1H2.7a1 1 0 0 1-1-1z"/></svg>"##,
            Icon::Branch => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.4" stroke-linecap="round"><circle cx="4.5" cy="3.5" r="1.6"/><circle cx="4.5" cy="12.5" r="1.6"/><circle cx="11.5" cy="5" r="1.6"/><path d="M4.5 5.1v5.8M11.5 6.6c0 3-7 2-7 4.3"/></svg>"##,
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
        .hover(|d| d.bg(rgb(SELECTED)))
        .child(icon(which, 16., if on { TEXT } else { DIM }))
}

/// Claude Code's critter, small, for the tab of the person's own Claude Code.
pub fn claude_icon() -> Svg {
    const CRITTER: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 12"><path fill="#000" fill-rule="evenodd" d="M3 0h10v3h3v3h-3v6h-1V9h-1v3h-1V9H6v3H5V9H4v3H3V6H0V3h3zM5 3v2h1V3zm5 0v2h1V3z"/></svg>"##;
    svg().data(CRITTER.as_bytes()).w(px(14.)).h(px(10.5)).flex_none().text_color(rgb(0xe8875f))
}
