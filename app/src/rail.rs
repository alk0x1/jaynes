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

/// Whether the panel shows the run picked: only while a run's tab is in the middle; with the Inbox, Claude Code, an
/// agent or Changes there, it asks to pick one, rather than show the last run picked out of sight.
pub fn shows_run(middle: &crate::tabs::Middle) -> bool {
    matches!(middle, crate::tabs::Middle::Run(_))
}

/// What the panel says over the question of a run that waits: one already delivered says so, and that the answer
/// still corrects it (the core: "delivered; your answers can still correct it").
pub fn waits_words(delivered: bool) -> (&'static str, Option<&'static str>) {
    if delivered { ("Delivered", Some(" · your answer can still correct it")) } else { ("Waits for you", None) }
}

/// A card of the panel: its head, "Now" while the run works, "Waits for you" while it waits, and each step (by its
/// place in the run's steps).
#[derive(Clone, Debug, PartialEq)]
pub enum Card {
    Head,
    Now,
    Waits,
    Step(usize),
}

/// The panel's cards in their order, each with where a click takes the run's story and whether the click also raises
/// the question that waits: the head opens the story at its top, "Now" at its end, "Waits for you" at the step that
/// waits (the story's end when none says it waits) with its question, the stop risen or the compare mode for one that
/// chooses among designs, and a step at what the agent told there. Every card the panel draws is here, so none is a
/// card that does nothing.
pub fn cards(d: &Detail) -> Vec<(Card, Place, bool)> {
    let mut cards = vec![(Card::Head, Place::Top, false)];
    match d.mark {
        Mark::Working => cards.push((Card::Now, Place::End, false)),
        Mark::Needs => {
            let waits = d.stages.iter().find(|s| s.class == "waiting").map(|s| Place::Step(s.name.clone())).unwrap_or(Place::End);
            cards.push((Card::Waits, waits, true));
        }
        _ => {}
    }
    cards.extend(d.stages.iter().enumerate().map(|(i, s)| (Card::Step(i), Place::Step(s.name.clone()), false)));
    cards
}

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
fn step(run: &str, s: &Stage, lit: bool, place: Place, tabs: &Entity<Tabs>) -> impl IntoElement {
    let todo = s.class == "todo";
    opens(format!("step-{}", s.name), run, place, tabs)
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

fn now(run: &str, text: &str, place: Place, tabs: &Entity<Tabs>) -> impl IntoElement {
    opens("now".into(), run, place, tabs)
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

/// The head: no card to the eye, it opens the story at its top, its look unchanged.
fn head(d: &Detail, place: Place, tabs: &Entity<Tabs>) -> impl IntoElement {
    opens("head".into(), &d.id, place, tabs)
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
        .child(div().text_size(px(12.5)).text_color(rgb(look::dim())).child(d.project.clone()))
}

/// What waits: where the agent last spoke, and, when `raises`, the question itself.
fn waits(d: &Detail, place: Place, raises: bool, tabs: &Entity<Tabs>, island: &Entity<crate::island::Island>) -> impl IntoElement {
    let island = island.clone();
    opens("waits".into(), &d.id, place, tabs)
        .when(raises, |e| e.on_click(move |_, window, cx| island.update(cx, |i, cx| i.open_stop(window, cx))))
        .hover(|d| d.bg(rgb(look::selected())))
                .mx(px(-8.))
                .px(px(8.))
                .py(px(5.))
                .rounded(px(7.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child({
                    // "Delivered" in green, what the answer still does in amber
                    let (first, rest) = waits_words(d.delivered);
                    div().flex().child(look::label(first, if rest.is_some() { look::green() } else { look::amber() })).children(rest.map(|r| look::label(r, look::amber())))
                })
                .child(div().text_size(px(13.)).text_color(rgb(look::text())).child(d.line.clone()))
}

/// The panel, drawn from its cards (cards), each opening the place a click on it takes the story.
fn panel(d: &Detail, lit: Option<&str>, tabs: &Entity<Tabs>, island: &Entity<crate::island::Island>) -> impl IntoElement {
    let mut top = div().flex().flex_col().size_full();
    let mut body = div().id("steps").flex().flex_col().flex_1().overflow_y_scroll().px(px(16.)).py(px(14.)).gap(px(16.));
    let mut steps = div().flex().flex_col();
    for (card, place, raises) in cards(d) {
        match card {
            Card::Head => top = top.child(head(d, place, tabs)),
            Card::Now => body = body.child(div().flex().flex_col().gap(px(6.)).child(look::label("Now", look::faint())).child(now(&d.id, d.now.as_deref().unwrap_or("starting"), place, tabs))),
            Card::Waits => body = body.child(waits(d, place, raises, tabs, island)),
            Card::Step(i) => {
                let s = &d.stages[i];
                steps = steps.child(step(&d.id, s, lit == Some(s.name.as_str()), place, tabs));
            }
        }
    }
    body = body.child(div().flex().flex_col().gap(px(6.)).child(look::label("Steps", look::faint())).child(steps));
    top.child(body)
}

impl Render for Rail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.tabs.read(cx);
        let chosen = self.runs.read(cx).chosen().cloned().filter(|_| shows_run(&tabs.middle()));
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

#[cfg(test)]
mod tests {
    use super::{cards, shows_run, waits_words, Card};
    use crate::model::Detail;
    use crate::tabs::{Middle, Place};
    use jaynes_shell::listing::{Mark, Stage};

    fn stage(index: usize, name: &str, class: &'static str) -> Stage {
        Stage { index, name: name.into(), about: String::new(), class, detail: String::new() }
    }

    fn run(mark: Mark, stages: Vec<Stage>) -> Detail {
        Detail { id: "r1".into(), title: "Tighten the copy".into(), role: "review".into(), project: "app".into(), mark, line: "Which goes in?".into(), minutes: 3, stages, now: Some("editing".into()), delivered: false }
    }

    // Alan, 17/11 (option 1 of the steps panel, as the session that built it in ab48af1 wrote what he chose): every
    // card in the steps panel clickable; a step lights its part of the story, "Waits for you" opens the stop (the
    // choice, when it chooses), "Now" goes to the end, the head opens the top and does not look like a card
    #[test]
    fn asked_every_card_in_the_steps_panel_clickable() {
        let steps = || vec![stage(1, "copy", "checked"), stage(2, "write", "waiting"), stage(3, "send", "todo")];
        let step = |i: usize, name: &str| (Card::Step(i), Place::Step(name.into()), false);
        // waiting: the head, what waits, at its step and with its question, and each step at its own place
        assert_eq!(
            cards(&run(Mark::Needs, steps())),
            vec![(Card::Head, Place::Top, false), (Card::Waits, Place::Step("write".into()), true), step(0, "copy"), step(1, "write"), step(2, "send")]
        );
        // waiting with no step that says so: what waits opens the story's end, and still its question
        assert_eq!(cards(&run(Mark::Needs, vec![stage(1, "copy", "checked")]))[1], (Card::Waits, Place::End, true));
        // at work: "Now" goes to the end
        assert_eq!(cards(&run(Mark::Working, steps()))[..2], [(Card::Head, Place::Top, false), (Card::Now, Place::End, false)]);
        // finished: the head and the steps, each still opening its place
        assert_eq!(cards(&run(Mark::Delivered, steps())), vec![(Card::Head, Place::Top, false), step(0, "copy"), step(1, "write"), step(2, "send")]);
    }

    #[test]
    fn the_steps_panel_shows_the_run_only_while_a_run_s_tab_is_in_the_middle() {
        assert!(shows_run(&Middle::Run("r1".into())));
        // the Inbox or Claude Code in the middle: "Pick a run to see its steps.", not the last run picked
        for m in [Middle::Inbox, Middle::Terminal, Middle::Agent, Middle::Changes] {
            assert!(!shows_run(&m), "{m:?}");
        }
    }

    #[test]
    fn a_run_delivered_while_its_stop_is_open_says_the_answer_still_corrects_it() {
        assert_eq!(waits_words(false), ("Waits for you", None));
        assert_eq!(waits_words(true), ("Delivered", Some(" · your answer can still correct it")));
    }
}
