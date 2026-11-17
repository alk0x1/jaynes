//! The tabs over the middle of the window: the person's own Claude Code first, then a tab for each run at work or
//! waiting, as sketch D draws them. A run's tab shows its story (`jaynes_shell::listing::story`): each step its agent
//! worked, with what it did there, moving as the agent speaks. It reads, never types: a run's agent answers to
//! Jaynes, and the person answers it at its stops.

use crate::look;
use crate::model::Runs;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use jaynes_shell::listing::Mark;
use std::time::Duration;

/// Where a click in the steps panel takes a run's story.
#[derive(Clone, PartialEq)]
pub enum Place {
    Top,
    /// Where the agent is now: the story's end.
    End,
    /// The latest time the agent worked at that step.
    Step(String),
}

pub struct Tabs {
    runs: Entity<Runs>,
    /// The run whose tab shows; None for the person's own terminal.
    pub open: Option<String>,
    story: Vec<(String, Vec<String>)>,
    /// Where the story is scrolled; each step it tells is one of its children, so a step can be scrolled to.
    scroll: ScrollHandle,
    /// Where a click in the steps panel asked the story to go, until the story is read and the tab scrolls there.
    seek: Option<Place>,
    /// The step lit in the story and in the steps panel: the one the person last clicked there.
    pub lit: Option<String>,
    _poll: Task<()>,
}

impl Tabs {
    pub fn new(runs: Entity<Runs>, cx: &mut Context<Self>) -> Self {
        cx.observe(&runs, |_, _, cx| cx.notify()).detach();
        // the open run's story, read again each second while it is open
        let poll = cx.spawn(async move |this, cx| loop {
            let Ok(open) = this.update(cx, |t, _| t.open.clone()) else { break };
            if let Some(id) = open
                && Self::tell(this.clone(), id, cx).await.is_err()
            {
                break;
            }
            cx.background_executor().timer(Duration::from_secs(1)).await;
        });
        Tabs { runs, open: None, story: Vec::new(), scroll: ScrollHandle::new(), seek: None, lit: None, _poll: poll }
    }

    /// Reads a run's story and shows it, if its tab is still the one open when the reading ends.
    async fn tell(this: WeakEntity<Self>, id: String, cx: &mut AsyncApp) -> Result<()> {
        let at = id.clone();
        let told = cx.background_executor().spawn(async move { jaynes_shell::listing::story(&jaynes_shell::home(), &at) }).await;
        this.update(cx, |t, cx| {
            if t.open.as_deref() == Some(id.as_str()) && t.story != told {
                t.story = told;
                t.go_to_seek();
                cx.notify();
            }
        })
    }

    /// Shows a run's tab scrolled to a place in its story.
    pub fn show_at(&mut self, run: String, place: Place, cx: &mut Context<Self>) {
        self.show(Some(run), cx);
        self.lit = match &place {
            Place::Step(step) => Some(step.clone()),
            _ => None,
        };
        self.seek = Some(place);
        self.go_to_seek();
        cx.notify();
    }

    /// Scrolls where the click asked once the story is read; a step the agent has not spoken in goes to the top.
    fn go_to_seek(&mut self) {
        if self.story.is_empty() && self.seek != Some(Place::Top) {
            return;
        }
        let top = || point(px(0.), px(0.));
        match self.seek.take() {
            None => {}
            Some(Place::Top) => self.scroll.set_offset(top()),
            Some(Place::End) => self.scroll.scroll_to_bottom(),
            Some(Place::Step(step)) => match self.story.iter().rposition(|(s, _)| *s == step) {
                Some(i) => self.scroll.scroll_to_top_of_item(i),
                None => self.scroll.set_offset(top()),
            },
        }
    }

    /// Shows a run's tab (and picks the run), or the person's terminal with None.
    pub fn show(&mut self, run: Option<String>, cx: &mut Context<Self>) {
        if run != self.open {
            self.story.clear();
            self.seek = None;
            self.lit = None;
            self.scroll.set_offset(point(px(0.), px(0.)));
            // read at once, not at the poll's next turn: the tab would show nothing for up to a second
            if let Some(id) = run.clone() {
                cx.spawn(async move |this, cx| Self::tell(this, id, cx).await).detach();
            }
        }
        if let Some(id) = &run {
            let id = id.clone();
            self.runs.update(cx, |runs, cx| runs.select(id, cx));
        }
        self.open = run;
        cx.notify();
    }

    /// The runs that get a tab: what waits, then what works, then the one open if it is neither.
    fn tabbed(&self, cx: &App) -> Vec<(String, String, Mark)> {
        let listing = &self.runs.read(cx).snap.listing;
        let mut out: Vec<(String, String, Mark)> = listing.needs.iter().map(|r| (r.id.clone(), r.title.clone(), r.mark)).collect();
        for (_, rows) in &listing.projects {
            out.extend(rows.iter().filter(|r| r.mark == Mark::Working).map(|r| (r.id.clone(), r.title.clone(), r.mark)));
        }
        if let Some(open) = &self.open
            && !out.iter().any(|(id, _, _)| id == open)
            && let Some(r) = listing.projects.iter().flat_map(|(_, rows)| rows.iter()).find(|r| &r.id == open)
        {
            out.push((r.id.clone(), r.title.clone(), r.mark));
        }
        out
    }

    /// The open run's story, in place of the terminal.
    pub fn body(&self, cx: &App) -> Option<AnyElement> {
        let open = self.open.as_ref()?;
        let runs = self.runs.read(cx);
        let detail = runs.snap.details.get(open);
        let working = detail.is_some_and(|d| d.mark == Mark::Working);
        let order: Vec<String> = detail.map(|d| d.stages.iter().map(|s| s.name.clone()).collect()).unwrap_or_default();
        // one child per step told, so the steps panel can scroll the story to one of them
        let mut lines = div()
            .id("story")
            .track_scroll(&self.scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(20.))
            .py(px(10.))
            .font_family(look::MONO_FONT)
            .text_size(px(13.))
            .line_height(px(21.))
            .text_color(rgb(0xc9ced8))
            .flex()
            .flex_col();
        if self.story.is_empty() {
            lines = lines.child(div().text_color(rgb(look::FAINT)).child(if working { "The agent is starting…" } else { "No agent has spoken in this run yet." }));
        }
        for (step, said) in &self.story {
            let at = order.iter().position(|s| s == step).map(|i| format!("Step {} of {} · {step}", i + 1, order.len())).unwrap_or_else(|| step.clone());
            // the step clicked in the steps panel stands out: a bar at its left and a ground under it
            let lit = self.lit.as_deref() == Some(step.as_str());
            let mut told = div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .mx(px(-10.))
                .pl(px(8.))
                .pr(px(10.))
                .pb(px(6.))
                .rounded(px(6.))
                .border_l_2()
                .border_color(if lit { rgb(look::BLUE) } else { rgba(0) })
                .when(lit, |d| d.bg(rgb(look::PANEL_2)))
                .child(div().pt(px(10.)).text_color(rgb(look::BLUE)).child(format!("── {at}")));
            for s in said {
                told = told.child(div().flex().gap(px(8.)).child(div().flex_none().text_color(rgb(look::FAINT)).child("●")).child(div().min_w_0().child(s.clone())));
            }
            lines = lines.child(told);
        }
        if working {
            lines = lines.child(div().pt(px(6.)).flex().gap(px(8.)).items_center().child(look::spinner("story-spin", 11., look::BLUE)).child(div().text_color(rgb(look::FAINT)).child("working")));
        }
        Some(
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(lines)
                .child(
                    div()
                        .mx(px(14.))
                        .mb(px(12.))
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(9.))
                        .border_1()
                        .border_color(rgb(look::LINE))
                        .font_family(look::UI_FONT)
                        .text_size(px(12.5))
                        .text_color(rgb(look::FAINT))
                        .child("This run's agent works on its own and answers to Jaynes; you answer it at its stops."),
                )
                .into_any_element(),
        )
    }
}

/// A tab in the top bar, underlined when it shows: amber for a run that waits, the text's colour otherwise. A press
/// on it is the tab's, not the bar's, so it does not move the window.
fn tab(id: SharedString, on: bool, line: u32) -> Stateful<Div> {
    div()
        .id(id)
        .h_full()
        .flex()
        .items_center()
        .gap(px(7.))
        .px(px(12.))
        .max_w(px(240.))
        .border_b_2()
        .border_color(if on { rgb(line) } else { rgba(0) })
        .text_size(px(12.5))
        .text_color(rgb(if on { look::TEXT } else { look::DIM }))
        .hover(|d| d.text_color(rgb(look::TEXT)))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

impl Render for Tabs {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // the tabs are the top bar's: its height, its line under them
        let mut row = div()
            .id("tabs")
            .h_full()
            .min_w_0()
            .flex()
            .items_stretch()
            .overflow_x_scroll()
            .font_family(look::UI_FONT)
            .child(
                tab("tab-main".into(), self.open.is_none(), look::TEXT)
                    .on_click(cx.listener(|this, _, _, cx| this.show(None, cx)))
                    .child(look::claude_icon())
                    .child("Claude Code"),
            );
        for (id, title, mark) in self.tabbed(cx) {
            let on = self.open.as_deref() == Some(id.as_str());
            let pick = id.clone();
            row = row.child(
                tab(SharedString::from(format!("tab-{id}")), on, if mark == Mark::Needs { look::AMBER } else { look::TEXT })
                    .on_click(cx.listener(move |this, _, _, cx| this.show(Some(pick.clone()), cx)))
                    .child(if mark == Mark::Working {
                        look::spinner(SharedString::from(format!("tab-spin-{id}")), 10., look::BLUE).into_any_element()
                    } else {
                        div().size(px(7.)).flex_none().rounded_full().bg(rgb(look::colour(mark))).into_any_element()
                    })
                    .child(div().truncate().child(title)),
            );
        }
        row
    }
}
