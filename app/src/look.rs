//! Sketch D's colours and the small pieces the list and the steps panel share.

use gpui::*;
use jaynes_shell::listing::Mark;

pub const PANEL: u32 = 0x11151c;
pub const PANEL_2: u32 = 0x161b24;
pub const SELECTED: u32 = 0x1c2330;
pub const LINE: u32 = 0x222a36;
pub const LINE_2: u32 = 0x2d3747;
pub const TEXT: u32 = 0xeef2f7;
pub const DIM: u32 = 0x9aa6b8;
pub const FAINT: u32 = 0x5f6b7e;
pub const AMBER: u32 = 0xffb35c;
pub const BLUE: u32 = 0x6fb4ff;
pub const GREEN: u32 = 0x69dba0;
pub const RED: u32 = 0xff7a7a;
pub const GREY: u32 = 0x7d8899;
/// Text on an amber or green fill.
pub const ON_FILL: u32 = 0x0b0e13;

pub const UI_FONT: &str = "Geist";
pub const MONO_FONT: &str = "JetBrains Mono";

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
