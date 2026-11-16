//! The window's colours and the small pieces the list and the steps panel share: one dark for the whole window, the
//! terminal's included (the theme `jaynes` in settings.rs has the same background), as direction A of the polish drew.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use jaynes_shell::listing::Mark;

pub const PANEL: u32 = 0x101217;
pub const PANEL_2: u32 = 0x171a21;
pub const SELECTED: u32 = 0x1d212a;
pub const LINE: u32 = 0x20242d;
pub const LINE_2: u32 = 0x2b303b;
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

/// A small uppercase heading.
pub fn label(text: &str, color: u32) -> Div {
    div().text_size(px(11.)).text_color(rgb(color)).child(text.to_uppercase())
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

/// The app's mark, small: the J on its dark tile with the amber dot of what waits (icon 1 of the polish).
pub fn mark_icon(size: f32) -> Div {
    div()
        .size(px(size))
        .flex_none()
        .relative()
        .rounded(px(size * 0.24))
        .bg(rgb(PANEL_2))
        .border_1()
        .border_color(rgb(LINE_2))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(size * 0.62))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(TEXT))
        .child("J")
        .child(div().absolute().top(px(size * 0.14)).right(px(size * 0.14)).size(px(size * 0.2)).rounded_full().bg(rgb(AMBER)))
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
