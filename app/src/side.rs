//! The runs beside the terminal: what waits for the person first, then each project's runs, newest first. The list
//! is the engine's own (`jaynes_shell::listing`), read from the record, so it says what `jaynes runs` and the page say.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use jaynes_shell::listing::{Listing, Mark, Row};
use std::time::Duration;

/// Sketch D's colours.
mod ink {
    pub const PANEL: u32 = 0x11151c;
    pub const SELECTED: u32 = 0x1c2330;
    pub const LINE: u32 = 0x222a36;
    pub const TEXT: u32 = 0xeef2f7;
    pub const DIM: u32 = 0x9aa6b8;
    pub const FAINT: u32 = 0x5f6b7e;
    pub const AMBER: u32 = 0xffb35c;
    pub const BLUE: u32 = 0x6fb4ff;
    pub const GREEN: u32 = 0x69dba0;
    pub const RED: u32 = 0xff7a7a;
    pub const GREY: u32 = 0x7d8899;
}

pub const WIDTH: f32 = 250.;
const UI_FONT: &str = "Geist";

pub struct Side {
    listing: Listing,
    selected: Option<String>,
    /// Why the record could not be read, shown instead of the list.
    trouble: Option<String>,
    _watch: Task<()>,
}

impl Side {
    /// Reads the record now, and again whenever it gains an entry.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (tx, rx) = async_channel::bounded::<Result<Listing, String>>(1);
        std::thread::Builder::new()
            .name("record".into())
            .spawn(move || watch(tx))
            .expect("cannot start the thread that reads the record");
        let watch = cx.spawn(async move |this, cx| {
            while let Ok(read) = rx.recv().await {
                let gone = this
                    .update(cx, |this, cx| {
                        match read {
                            Ok(listing) => {
                                this.listing = listing;
                                this.trouble = None;
                            }
                            Err(e) => this.trouble = Some(e),
                        }
                        cx.notify();
                    })
                    .is_err();
                if gone {
                    break;
                }
            }
        });
        Side { listing: Listing::default(), selected: None, trouble: None, _watch: watch }
    }
}

/// Sends the list at once and after every change to the record; checks once a second.
fn watch(tx: async_channel::Sender<Result<Listing, String>>) {
    let home = jaynes_shell::home();
    let mut seen = None;
    loop {
        let seq = jaynes_shell::store::Store::open(&home.join("record.sqlite3")).and_then(|s| s.last_seq());
        let changed = match (&seq, seen) {
            (Ok(n), Some(m)) => *n != m,
            _ => true,
        };
        if changed {
            seen = seq.as_ref().ok().copied();
            let read = seq.and_then(|_| jaynes_shell::listing::read(&home));
            if tx.send_blocking(read).is_err() {
                return;
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn colour(mark: Mark) -> u32 {
    match mark {
        Mark::Needs => ink::AMBER,
        Mark::Working => ink::BLUE,
        Mark::Halted => ink::GREY,
        Mark::Blocked | Mark::Undone => ink::RED,
        Mark::Delivered | Mark::Merged => ink::GREEN,
        Mark::Abandoned => ink::FAINT,
    }
}

fn section(name: String, count: Option<usize>, color: u32) -> impl IntoElement {
    div()
        .flex()
        .justify_between()
        .px(px(12.))
        .pt(px(14.))
        .pb(px(6.))
        .text_size(px(11.))
        .text_color(rgb(color))
        .child(name.to_uppercase())
        .children(count.map(|n| div().text_color(rgb(ink::FAINT)).child(n.to_string())))
}

impl Side {
    fn row(&self, row: &Row, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.selected.as_deref() == Some(row.id.as_str());
        let id = row.id.clone();
        div()
            .id(SharedString::from(format!("run-{}", row.id)))
            .mx(px(6.))
            .px(px(8.))
            .py(px(7.))
            .rounded(px(7.))
            .flex()
            .gap(px(10.))
            .items_start()
            .when(on, |d| d.bg(rgb(ink::SELECTED)))
            .hover(|d| d.bg(rgb(ink::SELECTED)))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = Some(id.clone());
                cx.notify();
            }))
            .child(div().mt(px(5.)).size(px(8.)).flex_none().rounded_full().bg(rgb(colour(row.mark))))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .flex_1()
                    .child(div().truncate().text_size(px(13.)).text_color(rgb(ink::TEXT)).child(row.title.clone()))
                    .child(div().truncate().text_size(px(11.5)).text_color(rgb(if row.mark == Mark::Needs { ink::AMBER } else { ink::DIM })).child(row.line.clone())),
            )
    }
}

impl Render for Side {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().id("runs").flex().flex_col().flex_1().overflow_y_scroll().pb(px(12.));
        if let Some(trouble) = &self.trouble {
            list = list.child(div().p(px(12.)).text_size(px(12.)).text_color(rgb(ink::RED)).child(format!("The record cannot be read: {trouble}")));
        } else if self.listing.needs.is_empty() && self.listing.projects.is_empty() {
            list = list.child(div().p(px(12.)).text_size(px(12.)).text_color(rgb(ink::FAINT)).child("No runs yet."));
        }
        if !self.listing.needs.is_empty() {
            list = list.child(section("Needs you".into(), Some(self.listing.needs.len()), ink::AMBER));
            for row in self.listing.needs.clone() {
                list = list.child(self.row(&row, cx));
            }
        }
        for (project, rows) in self.listing.projects.clone() {
            list = list.child(section(project, None, ink::FAINT));
            for row in &rows {
                list = list.child(self.row(row, cx));
            }
        }
        div()
            .w(px(WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .font_family(UI_FONT)
            .bg(rgb(ink::PANEL))
            .border_r_1()
            .border_color(rgb(ink::LINE))
            .child(list)
    }
}
