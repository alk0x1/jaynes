//! What a stop offers the person to grant beside their answer (`Stop.offers`), such as a new project's test servers:
//! a box with the core's words and, under it, what granting it does. The box starts marked as the step's output says;
//! at a stop that chooses among items it follows the item the person picks, until they set the box themselves. When
//! the answer goes, each marked box goes first as the page's act "grant", the person's alone, and then the answer (the
//! engineer lead's T5, 23/11).

use crate::look;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use jaynes_core::manifest::Offer;
use jaynes_shell::stop::{Offered, Stop};
use std::collections::BTreeMap;

/// The boxes the person set themselves at one question: a new question starts with none set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ticks {
    question: Option<String>,
    set: BTreeMap<Offer, bool>,
}

impl Ticks {
    /// Starts over for question `q` when it is another one than the boxes were set at.
    pub fn at(&mut self, q: &str) {
        if self.question.as_deref() != Some(q) {
            self.question = Some(q.to_string());
            self.set.clear();
        }
    }

    /// Whether `o`'s box is marked, with `picked` the item the person picks (at a stop that chooses): as they set it,
    /// else as the picked item says, else as the step said.
    pub fn ticked(&self, o: &Offered, picked: Option<&str>) -> bool {
        self.set.get(&o.offer).copied().unwrap_or_else(|| picked.and_then(|p| o.marked_for.get(p).copied()).unwrap_or(o.marked))
    }

    /// The person presses `o`'s box: it is theirs from now on, whatever item they pick.
    pub fn flip(&mut self, o: &Offered, picked: Option<&str>) {
        let now = self.ticked(o, picked);
        self.set.insert(o.offer, !now);
    }

    /// What goes as granted before the answer: every box marked.
    pub fn grants(&self, stop: &Stop, picked: Option<&str>) -> Vec<Offer> {
        stop.offers.iter().filter(|o| self.ticked(o, picked)).map(|o| o.offer).collect()
    }
}

/// One offer's box as the card draws it: the box, the core's words beside it, and under them, dim, what granting it
/// does. In the stop's Tab order; the caller gives it its press.
pub fn row(o: &Offered, ticked: bool) -> Stateful<Div> {
    let square = div()
        .size(px(16.))
        .mt(px(2.))
        .flex_none()
        .rounded(px(4.))
        .border_1()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.))
        .map(|d| if ticked { d.bg(rgb(look::blue())).border_color(rgb(look::blue())).text_color(rgb(look::on_fill())).child("✓") } else { d.border_color(rgb(look::dim())) });
    crate::island::tabbed_with(div().id(SharedString::from(format!("offer-{}", id_of(o.offer)))), Some(look::panel_2()))
        .flex()
        .gap(px(10.))
        .items_start()
        .px(px(4.))
        .py(px(4.))
        .rounded(px(6.))
        .cursor_pointer()
        .child(square)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(div().text_size(px(13.)).text_color(rgb(look::text())).child(o.says.clone()))
                .child(div().text_size(px(12.)).text_color(rgb(look::dim())).child(o.then.clone())),
        )
}

/// The id of each offer's box, for a test that presses it (`offer test_servers`).
pub fn id_of(o: Offer) -> &'static str {
    match o {
        Offer::TestServers => "test_servers",
    }
}

#[cfg(test)]
mod tests {
    use super::{id_of, Ticks};
    use jaynes_core::manifest::Offer;
    use jaynes_core::question::{QuestionKind, VerdictKind};
    use jaynes_shell::stop::{Item, Offered, Stop, TEST_SERVERS_SAYS, TEST_SERVERS_THEN};
    use std::collections::BTreeMap;

    fn offered(marked: bool, marked_for: &[(&str, bool)]) -> Offered {
        Offered {
            offer: Offer::TestServers,
            says: TEST_SERVERS_SAYS.into(),
            then: TEST_SERVERS_THEN.into(),
            marked,
            marked_for: marked_for.iter().map(|(i, m)| (i.to_string(), *m)).collect::<BTreeMap<_, _>>(),
        }
    }

    fn stack(offers: Vec<Offered>) -> Stop {
        let item = |id: &str| Item { id: id.into(), label: format!("The {id}"), detail: String::new(), aspects: vec![], pictures: vec![], page: None };
        Stop {
            run: "r".into(),
            question: "q1".into(),
            shown: "s".into(),
            title: "Which stack do we build on?".into(),
            asks: "Which stack do we build on?".into(),
            step: "stacks".into(),
            permission: false,
            kind: QuestionKind::Decision,
            choose: true,
            offered: vec![VerdictKind::Keep, VerdictKind::Change, VerdictKind::Drop],
            items: ["web", "cli"].iter().map(|i| item(i)).collect(),
            back: vec![],
            round: 1,
            about: String::new(),
            ahead: None,
            touch: None,
            into: String::new(),
            lands: vec![],
            passed: None,
            answers: Default::default(),
            note: String::new(),
            why: None,
            consequences: Default::default(),
            wants: None,
            gone: None,
            offers,
            rides: vec![],
        }
    }

    // the engineer lead's T5 (23/11): at the stack's stop the box "Let this project's tests start their own test
    // servers" starts as the step said, follows the stack the person picks (one serves HTTP, one does not), and once
    // the person presses it, it is theirs whatever they pick; only a marked box goes as granted
    #[test]
    fn asked_the_test_servers_box_starts_as_the_step_said_and_follows_the_picked_stack_until_the_person_sets_it() {
        let o = offered(true, &[("web", true), ("cli", false)]);
        let stop = stack(vec![o.clone()]);
        assert_eq!((o.says.as_str(), id_of(o.offer)), ("Let this project's tests start their own test servers", "test_servers"));
        let mut t = Ticks::default();
        t.at("q1");
        assert_eq!((t.ticked(&o, None), t.ticked(&o, Some("web")), t.ticked(&o, Some("cli"))), (true, true, false));
        assert_eq!((t.grants(&stop, Some("web")), t.grants(&stop, Some("cli"))), (vec![Offer::TestServers], vec![]));
        // the person marks it at the stack that does not serve: it stays marked at the other too
        t.flip(&o, Some("cli"));
        assert_eq!((t.ticked(&o, Some("cli")), t.ticked(&o, Some("web"))), (true, true));
        t.flip(&o, Some("web"));
        assert_eq!((t.ticked(&o, Some("web")), t.grants(&stop, Some("web"))), (false, vec![]));
        // the same question keeps it; a new question starts as its step said
        t.at("q1");
        assert!(!t.ticked(&o, Some("web")));
        t.at("q2");
        assert!(t.ticked(&o, Some("web")));
        // a box of the whole output, unmarked by the step, starts unmarked whatever is picked
        let plain = offered(false, &[]);
        assert_eq!((t.ticked(&plain, None), t.ticked(&plain, Some("web"))), (false, false));
    }
}
