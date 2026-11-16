//! The steps panel, at the window's right: the run picked in the list, what its agent does now, and each of its
//! steps with how it stands, moving as the run goes. Command-E folds it away and back.

use crate::look;
use crate::model::{Detail, Runs};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use jaynes_shell::listing::{Mark, Stage};

pub const WIDTH: f32 = 300.;

pub struct Rail {
    runs: Entity<Runs>,
}

impl Rail {
    pub fn new(runs: Entity<Runs>, cx: &mut Context<Self>) -> Self {
        cx.observe(&runs, |_, _, cx| cx.notify()).detach();
        Rail { runs }
    }
}

/// How a step's circle looks: its fill, its ring and what it holds.
fn circle(class: &str, name: &str) -> Div {
    let base = div().size(px(18.)).flex_none().rounded_full().border_1().flex().items_center().justify_center().text_size(px(10.)).mt(px(1.));
    match class {
        "checked" | "accepted" | "sampled" | "jaynes" => base.bg(rgb(look::GREEN)).border_color(rgb(look::GREEN)).text_color(rgb(look::ON_FILL)).child("✓"),
        "running" => base.border_color(rgb(look::BLUE)).child(look::breathing(SharedString::from(format!("breathe-{name}")), 7., look::BLUE)),
        "waiting" => base.bg(rgb(look::AMBER)).border_color(rgb(look::AMBER)).text_color(rgb(look::ON_FILL)).child("!"),
        "blocked" => base.border_color(rgb(look::RED)).text_color(rgb(look::RED)).child("×"),
        _ => base.border_color(rgb(look::LINE_2)),
    }
}

fn step(s: &Stage) -> impl IntoElement {
    let todo = s.class == "todo";
    div()
        .flex()
        .gap(px(10.))
        .items_start()
        .py(px(5.))
        .child(circle(s.class, &s.name))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .flex_1()
                .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(rgb(if todo { look::FAINT } else { look::TEXT })).child(s.name.clone()))
                .when_some((!s.about.is_empty()).then(|| s.about.clone()), |d, about| d.child(div().text_size(px(12.)).text_color(rgb(look::DIM)).child(about)))
                .when_some((!s.detail.is_empty()).then(|| s.detail.clone()), |d, detail| d.child(div().text_size(px(11.5)).text_color(rgb(look::FAINT)).child(detail))),
        )
}

fn now(text: &str) -> impl IntoElement {
    div()
        .flex()
        .gap(px(10.))
        .items_center()
        .px(px(12.))
        .py(px(10.))
        .rounded(px(10.))
        .bg(rgb(look::PANEL_2))
        .border_1()
        .border_color(rgb(look::LINE))
        .child(look::spinner("now-spin", 11., look::BLUE))
        .child(div().min_w_0().flex_1().truncate().font_family(look::MONO_FONT).text_size(px(12.)).text_color(rgb(look::DIM)).child(text.to_string()))
}

fn panel(d: &Detail) -> impl IntoElement {
    let head = div()
        .px(px(16.))
        .pt(px(14.))
        .pb(px(12.))
        .border_b_1()
        .border_color(rgb(look::LINE))
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .flex()
                .gap(px(8.))
                .items_center()
                .child(look::chip(d.mark))
                .child(div().text_size(px(12.)).text_color(rgb(look::FAINT)).child(format!("{} · {} min", d.role, d.minutes))),
        )
        .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::TEXT)).child(d.title.clone()))
        .child(div().text_size(px(12.5)).text_color(rgb(look::DIM)).child(d.project.clone()));
    let mut body = div().id("steps").flex().flex_col().flex_1().overflow_y_scroll().px(px(16.)).py(px(14.)).gap(px(16.));
    if d.mark == Mark::Working {
        body = body.child(div().flex().flex_col().gap(px(6.)).child(look::label("Now", look::FAINT)).child(now(d.now.as_deref().unwrap_or("starting"))));
    } else if d.mark == Mark::Needs {
        body = body.child(div().flex().flex_col().gap(px(6.)).child(look::label("Waits for you", look::AMBER)).child(div().text_size(px(13.)).text_color(rgb(look::TEXT)).child(d.line.clone())));
    }
    let mut steps = div().flex().flex_col();
    for s in &d.stages {
        steps = steps.child(step(s));
    }
    body = body.child(div().flex().flex_col().gap(px(6.)).child(look::label("Steps", look::FAINT)).child(steps));
    div().flex().flex_col().size_full().child(head).child(body)
}

impl Render for Rail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chosen = self.runs.read(cx).chosen().cloned();
        let frame = div()
            .w(px(WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .font_family(look::UI_FONT)
            .bg(rgb(look::PANEL))
            .border_l_1()
            .border_color(rgb(look::LINE));
        match chosen {
            Some(d) => frame.child(panel(&d)),
            None => frame.child(div().p(px(16.)).text_size(px(12.)).text_color(rgb(look::FAINT)).child("Pick a run to see its steps.")),
        }
    }
}
