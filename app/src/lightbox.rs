//! A picture of proof at full size, or the change a step made, over the whole window. A click or Esc closes it.

use crate::look;
use gpui::*;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub enum Shown {
    Picture { title: String, path: PathBuf },
    Diff { title: String, text: String },
}

pub struct Lightbox {
    pub shown: Option<Shown>,
    focus: FocusHandle,
}

/// The longest diff drawn; what is not drawn is said, never cut silently.
const MOST: usize = 40_000;

impl Lightbox {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Lightbox { shown: None, focus: cx.focus_handle() }
    }

    pub fn show(&mut self, shown: Shown, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = Some(shown);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.shown = None;
        cx.notify();
    }
}

fn diff_lines(text: &str) -> Div {
    let mut text = text.to_string();
    let mut more = 0;
    if text.len() > MOST {
        let cut = (0..=MOST).rev().find(|&n| text.is_char_boundary(n)).unwrap_or(0);
        more = text.len() - cut;
        text.truncate(cut);
    }
    let mut lines = div().flex().flex_col().font_family(look::MONO_FONT).text_size(px(12.));
    for l in text.lines() {
        let color = if l.starts_with("diff --git") || l.starts_with("+++") || l.starts_with("---") {
            look::TEXT
        } else if l.starts_with('+') {
            look::GREEN
        } else if l.starts_with('-') {
            look::RED
        } else if l.starts_with("@@") {
            look::BLUE
        } else {
            look::DIM
        };
        lines = lines.child(div().whitespace_nowrap().text_color(rgb(color)).child(if l.is_empty() { " ".to_string() } else { l.to_string() }));
    }
    if more > 0 {
        lines = lines.child(div().pt(px(8.)).text_color(rgb(look::AMBER)).child(format!("{more} more bytes not shown here: read the branch before it lands.")));
    }
    lines
}

impl Render for Lightbox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(shown) = self.shown.clone() else { return div().into_any_element() };
        let (title, body): (String, AnyElement) = match shown {
            Shown::Picture { title, path } => (title, img(path).size_full().object_fit(ObjectFit::Contain).into_any_element()),
            Shown::Diff { title, text } => (title, div().id("diff").size_full().overflow_scroll().p(px(14.)).child(diff_lines(&text)).into_any_element()),
        };
        div()
            .id("lightbox")
            .track_focus(&self.focus)
            .absolute()
            .inset_0()
            .bg(rgba(0x05070acc))
            .p(px(28.))
            .flex()
            .font_family(look::UI_FONT)
            .on_click(cx.listener(|this, _, _, cx| this.close(cx)))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    this.close(cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .rounded(px(12.))
                    .bg(rgb(look::PANEL))
                    .border_1()
                    .border_color(rgb(look::LINE_2))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .px(px(14.))
                            .py(px(10.))
                            .border_b_1()
                            .border_color(rgb(look::LINE))
                            .text_size(px(13.))
                            .text_color(rgb(look::TEXT))
                            .child(title)
                            .child(div().text_color(rgb(look::FAINT)).child("Esc · close")),
                    )
                    .child(div().flex_1().min_h_0().p(px(10.)).child(body)),
            )
            .into_any_element()
    }
}
