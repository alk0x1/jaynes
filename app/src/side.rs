//! The runs beside the terminal: what waits for the person first, then each project's runs, newest first. The list
//! is the engine's own (`jaynes_shell::listing`), read from the record, so it says what `jaynes runs` and the page say.

use crate::look;
use crate::model::Runs;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use jaynes_shell::listing::{Mark, Row};

pub const WIDTH: f32 = 250.;

pub struct Side {
    runs: Entity<Runs>,
    tabs: Entity<crate::tabs::Tabs>,
    /// What "+ New run" does: asks Claude Code in the terminal for a run.
    new_run: std::rc::Rc<dyn Fn(&mut Window, &mut App)>,
}

impl Side {
    pub fn new(runs: Entity<Runs>, tabs: Entity<crate::tabs::Tabs>, new_run: std::rc::Rc<dyn Fn(&mut Window, &mut App)>, cx: &mut Context<Self>) -> Self {
        cx.observe(&runs, |_, _, cx| cx.notify()).detach();
        Side { runs, tabs, new_run }
    }

    fn row(&self, row: &Row, on: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let id = row.id.clone();
        let tabs = self.tabs.clone();
        div()
            .id(SharedString::from(format!("run-{}", row.id)))
            .mx(px(6.))
            .px(px(8.))
            .py(px(7.))
            .rounded(px(7.))
            .flex()
            .gap(px(10.))
            .items_start()
            .when(on, |d| d.bg(rgb(look::SELECTED)))
            .hover(|d| d.bg(rgb(look::SELECTED)))
            .cursor_pointer()
            .on_click(cx.listener(move |_, _, _, cx| tabs.update(cx, |tabs, cx| tabs.show(Some(id.clone()), cx))))
            .child(div().mt(px(3.)).size(px(12.)).flex_none().flex().items_center().justify_center().child(if row.mark == Mark::Working {
                look::spinner(SharedString::from(format!("spin-{}", row.id)), 11., look::BLUE).into_any_element()
            } else {
                div().size(px(8.)).rounded_full().bg(rgb(look::colour(row.mark))).into_any_element()
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .flex_1()
                    .child(div().truncate().text_size(px(13.)).text_color(rgb(look::TEXT)).child(row.title.clone()))
                    .child(div().truncate().text_size(px(11.5)).text_color(rgb(if row.mark == Mark::Needs { look::AMBER } else { look::DIM })).child(row.line.clone())),
            )
    }
}

fn section(name: &str, count: Option<usize>, color: u32) -> impl IntoElement {
    div()
        .flex()
        .justify_between()
        .px(px(12.))
        .pt(px(14.))
        .pb(px(6.))
        .child(look::label(name, color))
        .children(count.map(|n| div().text_size(px(11.)).text_color(rgb(look::FAINT)).child(n.to_string())))
}

impl Render for Side {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let runs = self.runs.read(cx);
        let (listing, selected, trouble) = (runs.snap.listing.clone(), runs.selected.clone(), runs.trouble.clone());
        let mut list = div().id("runs").flex().flex_col().flex_1().overflow_y_scroll().pb(px(12.));
        if let Some(trouble) = trouble {
            list = list.child(div().p(px(12.)).text_size(px(12.)).text_color(rgb(look::RED)).child(format!("The record cannot be read: {trouble}")));
        } else if listing.needs.is_empty() && listing.projects.is_empty() {
            list = list.child(div().p(px(12.)).text_size(px(12.)).text_color(rgb(look::FAINT)).child("No runs yet."));
        }
        if !listing.needs.is_empty() {
            list = list.child(section("Needs you", Some(listing.needs.len()), look::AMBER));
            for row in &listing.needs {
                list = list.child(self.row(row, selected.as_deref() == Some(row.id.as_str()), cx));
            }
        }
        for (project, rows) in &listing.projects {
            list = list.child(section(project, None, look::FAINT));
            for row in rows {
                list = list.child(self.row(row, selected.as_deref() == Some(row.id.as_str()), cx));
            }
        }
        div()
            .w(px(WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .font_family(look::UI_FONT)
            .bg(rgb(look::PANEL))
            .border_r_1()
            .border_color(rgb(look::LINE))
            .child(list)
            .child(
                div()
                    .id("new-run")
                    .m(px(10.))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(8.))
                    .border_1()
                    .border_dashed()
                    .border_color(rgb(look::LINE_2))
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(12.5))
                    .text_color(rgb(look::DIM))
                    .cursor_pointer()
                    .hover(|d| d.text_color(rgb(look::TEXT)).bg(rgb(look::SELECTED)))
                    .on_click({
                        let new_run = self.new_run.clone();
                        move |_, window, cx| new_run(window, cx)
                    })
                    .child("+ New run")
                    .child(look::kbd("⌘N")),
            )
    }
}
