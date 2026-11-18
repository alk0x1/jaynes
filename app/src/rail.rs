//! The steps panel, at the window's right: the run picked in the list, what its agent does now, and each of its
//! steps with how it stands, moving as the run goes. A click opens the run's tab: the head at the top of its story,
//! "Now" at its end, a step at what the agent told there, lit there and here; "Waits for you" also opens the question
//! that waits. Command-E folds it away and back.

use crate::look;
use crate::model::{Detail, Runs};
use crate::tabs::{Place, Tabs};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use jaynes_shell::listing::{Mark, Stage};

pub struct Rail {
    runs: Entity<Runs>,
    tabs: Entity<Tabs>,
    island: Entity<crate::island::Island>,
}

impl Rail {
    pub fn new(runs: Entity<Runs>, tabs: Entity<Tabs>, island: Entity<crate::island::Island>, cx: &mut Context<Self>) -> Self {
        cx.observe(&runs, |_, _, cx| cx.notify()).detach();
        cx.observe(&tabs, |_, _, cx| cx.notify()).detach();
        Rail { runs, tabs, island }
    }
}

/// How a step's circle looks: its fill, its ring and what it holds.
fn circle(class: &str, name: &str) -> Div {
    let base = div().size(px(18.)).flex_none().rounded_full().border_1().flex().items_center().justify_center().text_size(px(10.)).mt(px(1.));
    match class {
        "checked" | "accepted" | "sampled" | "jaynes" => base.bg(rgb(look::green())).border_color(rgb(look::green())).text_color(rgb(look::on_fill())).child("✓"),
        "running" => base.border_color(rgb(look::blue())).child(look::breathing(SharedString::from(format!("breathe-{name}")), 7., look::blue())),
        // halted where it ran: still, in the grey of a halted run, until it is resumed
        "halted" => base.border_color(rgb(look::grey())).text_color(rgb(look::grey())).child("‖"),
        "waiting" => base.bg(rgb(look::amber())).border_color(rgb(look::amber())).text_color(rgb(look::on_fill())).child("!"),
        "blocked" => base.border_color(rgb(look::red())).text_color(rgb(look::red())).child("×"),
        _ => base.border_color(rgb(look::line_2())),
    }
}

/// Something that opens the run's tab at a place in its story when clicked.
fn opens(id: String, run: &str, place: Place, tabs: &Entity<Tabs>) -> Stateful<Div> {
    let (run, tabs) = (run.to_string(), tabs.clone());
    div()
        .id(SharedString::from(id))
        .cursor_pointer()
        .on_click(move |_, _, cx| tabs.update(cx, |t, cx| t.show_at(run.clone(), place.clone(), cx)))
}

/// A step's row: lit under the mouse, and while it is the step lit in the story.
fn step(run: &str, s: &Stage, lit: bool, tabs: &Entity<Tabs>) -> impl IntoElement {
    let todo = s.class == "todo";
    opens(format!("step-{}", s.name), run, Place::Step(s.name.clone()), tabs)
        .mx(px(-8.))
        .px(px(8.))
        .rounded(px(7.))
        .hover(|d| d.bg(rgb(look::selected())))
        .when(lit, |d| d.bg(rgb(look::selected())))
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
                .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(rgb(if todo { look::faint() } else { look::text() })).child(s.name.clone()))
                .when_some((!s.about.is_empty()).then(|| s.about.clone()), |d, about| d.child(div().text_size(px(12.)).text_color(rgb(look::dim())).child(about)))
                .when_some((!s.detail.is_empty()).then(|| s.detail.clone()), |d, detail| d.child(div().text_size(px(11.5)).text_color(rgb(look::faint())).child(detail))),
        )
}

fn now(run: &str, text: &str, tabs: &Entity<Tabs>) -> impl IntoElement {
    opens("now".into(), run, Place::End, tabs)
        .hover(|d| d.border_color(rgb(look::line_2())))
        .flex()
        .gap(px(10.))
        .items_center()
        .px(px(12.))
        .py(px(10.))
        .rounded(px(10.))
        .bg(rgb(look::panel_2()))
        .border_1()
        .border_color(rgb(look::line()))
        .child(look::spinner("now-spin", 11., look::blue()))
        .child(div().min_w_0().flex_1().truncate().font_family(look::MONO_FONT).text_size(px(12.)).text_color(rgb(look::dim())).child(text.to_string()))
}

fn panel(d: &Detail, lit: Option<&str>, tabs: &Entity<Tabs>, island: &Entity<crate::island::Island>) -> impl IntoElement {
    // the head is no card: it opens the story at its top, its look unchanged
    let head = opens("head".into(), &d.id, Place::Top, tabs)
        .px(px(16.))
        .pt(px(14.))
        .pb(px(12.))
        .border_b_1()
        .border_color(rgb(look::line()))
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .flex()
                .gap(px(8.))
                .items_center()
                .child(look::chip(d.mark))
                .child(div().text_size(px(12.)).text_color(rgb(look::faint())).child(format!("{} · {} min", d.role, d.minutes))),
        )
        .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::text())).child(d.title.clone()))
        .child(div().text_size(px(12.5)).text_color(rgb(look::dim())).child(d.project.clone()));
    let mut body = div().id("steps").flex().flex_col().flex_1().overflow_y_scroll().px(px(16.)).py(px(14.)).gap(px(16.));
    if d.mark == Mark::Working {
        body = body.child(div().flex().flex_col().gap(px(6.)).child(look::label("Now", look::faint())).child(now(&d.id, d.now.as_deref().unwrap_or("starting"), tabs)));
    } else if d.mark == Mark::Needs {
        // the step that waits, where the agent last spoke; the story's end when none says it waits
        let waits = d.stages.iter().find(|s| s.class == "waiting").map(|s| Place::Step(s.name.clone())).unwrap_or(Place::End);
        let island = island.clone();
        body = body.child(
            opens("waits".into(), &d.id, waits, tabs)
                // and the question itself: the stop risen, or the compare mode for one that chooses among designs
                .on_click(move |_, window, cx| island.update(cx, |i, cx| i.open_stop(window, cx)))
                .hover(|d| d.bg(rgb(look::selected())))
                .mx(px(-8.))
                .px(px(8.))
                .py(px(5.))
                .rounded(px(7.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(look::label("Waits for you", look::amber()))
                .child(div().text_size(px(13.)).text_color(rgb(look::text())).child(d.line.clone())),
        );
    }
    let mut steps = div().flex().flex_col();
    for s in &d.stages {
        steps = steps.child(step(&d.id, s, lit == Some(s.name.as_str()), tabs));
    }
    body = body.child(div().flex().flex_col().gap(px(6.)).child(look::label("Steps", look::faint())).child(steps));
    div().flex().flex_col().size_full().child(head).child(body)
}

impl Render for Rail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chosen = self.runs.read(cx).chosen().cloned();
        let tabs = self.tabs.read(cx);
        // lit only while the story shown is this run's
        let lit = chosen.as_ref().filter(|d| tabs.open.as_deref() == Some(d.id.as_str())).and(tabs.lit.clone());
        let frame = div()
            .w_full()
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .font_family(look::UI_FONT)
            .bg(rgb(look::panel()))
            .border_l_1()
            .border_color(rgb(look::line()));
        match chosen {
            Some(d) => frame.child(panel(&d, lit.as_deref(), &self.tabs, &self.island)),
            None => frame.child(div().p(px(16.)).text_size(px(12.)).text_color(rgb(look::faint())).child("Pick a run to see its steps.")),
        }
    }
}
