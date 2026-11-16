//! The stop, risen over the terminal as an island: the question the picked run waits at, a verdict per item, the
//! person's words where they send something back, what the answer will do in the engine's own sentences (G11), and
//! Send. Esc shrinks it to a pill and the run waits. An answer goes signed with the person's own key; before one that
//! leads to something that cannot be undone, macOS asks for the person's touch.

use crate::key::Key;
use crate::look;
use crate::model::Runs;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_base::input::{Input, InputState};
use jaynes_core::question::{By, VerdictKind};
use jaynes_shell::stop::{self, Stop};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

pub struct Island {
    runs: Entity<Runs>,
    key: Option<Arc<Key>>,
    /// The question the marks below are for: new marks for a new question.
    question: Option<String>,
    marks: BTreeMap<String, VerdictKind>,
    notes: HashMap<String, Entity<InputState>>,
    context: Option<Entity<InputState>>,
    effects: Vec<String>,
    refused: Option<String>,
    shrunk: HashSet<String>,
    sending: bool,
    said: Option<String>,
}

/// The engine program that carries a run on after an answer: JAYNES_BIN, else the installed `jaynes`.
fn engine_program() -> PathBuf {
    std::env::var_os("JAYNES_BIN").map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        home.join(".local/bin/jaynes")
    })
}

/// With JAYNES_APP_TEST_DO=<file>, a test says what the person does, a line at a time: `mark <item> <verdict>` or
/// `send`. Each goes through what a click does, so a test answers a stop without the person's mouse.
fn drive_from(file: PathBuf, cx: &mut Context<Island>) {
    cx.spawn(async move |this, cx| loop {
        cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        if text.trim().is_empty() {
            continue;
        }
        let _emptied = std::fs::write(&file, "");
        for line in text.lines() {
            let words: Vec<&str> = line.split_whitespace().collect();
            let done = this.update(cx, |this, cx| match words.as_slice() {
                ["mark", item, verdict] => {
                    let v = match *verdict {
                        "keep" => VerdictKind::Keep,
                        "drop" => VerdictKind::Drop,
                        "redo" => VerdictKind::Redo,
                        "change" => VerdictKind::Change,
                        _ => VerdictKind::Skip,
                    };
                    this.marks.insert(item.to_string(), v);
                    this.reckon(cx);
                }
                ["send"] => this.send(cx),
                _ => {}
            });
            if done.is_err() {
                return;
            }
        }
    })
    .detach();
}

/// A verdict that sends the item back takes the person's words.
fn takes_words(v: VerdictKind) -> bool {
    matches!(v, VerdictKind::Change | VerdictKind::Redo)
}

impl Island {
    pub fn new(runs: Entity<Runs>, key: Option<Arc<Key>>, cx: &mut Context<Self>) -> Self {
        cx.observe(&runs, |this, _, cx| {
            this.follow(cx);
            cx.notify();
        })
        .detach();
        if let Some(file) = std::env::var_os("JAYNES_APP_TEST_DO") {
            drive_from(PathBuf::from(file), cx);
        }
        Island { runs, key, question: None, marks: BTreeMap::new(), notes: HashMap::new(), context: None, effects: Vec::new(), refused: None, shrunk: HashSet::new(), sending: false, said: None }
    }

    /// New marks when the picked run waits at another question.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let now = self.runs.read(cx).stop().map(|s| s.question.clone());
        if now != self.question {
            self.question = now;
            self.marks.clear();
            self.notes.clear();
            self.context = None;
            self.effects.clear();
            self.refused = None;
            self.said = None;
        }
    }

    fn stop(&self, cx: &App) -> Option<Stop> {
        self.runs.read(cx).stop().cloned()
    }

    fn words(&self, item: &str, cx: &App) -> String {
        self.notes.get(item).map(|n| n.read(cx).value().to_string()).unwrap_or_default()
    }

    fn answer(&self, stop: &Stop, cx: &App) -> jaynes_core::question::Answer {
        let marks = self.marks.iter().map(|(id, v)| (id.clone(), (*v, self.words(id, cx)))).collect();
        let context = self.context.as_ref().map(|c| c.read(cx).value().to_string()).unwrap_or_default();
        stop::answer(stop, &marks, &context)
    }

    /// What the answer as marked will do, written by the engine, and whether the engine would refuse it.
    fn reckon(&mut self, cx: &mut Context<Self>) {
        let Some(stop) = self.stop(cx) else { return };
        let home = jaynes_shell::home();
        let mut answer = self.answer(&stop, cx);
        // what it will do as the person's answer, which it becomes once signed
        answer.by = By::Person;
        let run = jaynes_shell::store::Store::open(&home.join("record.sqlite3")).and_then(|s| s.state(&stop.run)).ok().flatten();
        let Some(run) = run else { return };
        let Some(q) = run.questions.get(&stop.question) else { return };
        match jaynes_shell::drive::effects(&home, &run, &q.question, &answer) {
            Ok(fx) => self.effects = fx.sentences,
            Err(e) => self.effects = vec![format!("Jaynes cannot say what this does: {e}")],
        }
        self.refused = if run.engine == jaynes_core::ENGINE { jaynes_core::decide(Some(&run), &jaynes_core::Input::Answer { answer }).err().map(|r| r.0) } else { None };
        cx.notify();
    }

    fn mark(&mut self, item: String, v: VerdictKind, window: &mut Window, cx: &mut Context<Self>) {
        self.marks.insert(item.clone(), v);
        if takes_words(v) && !self.notes.contains_key(&item) {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(if v == VerdictKind::Change { "how should it change?" } else { "what is missing?" }));
            cx.subscribe(&input, |this, _, _: &gpui_base::input::InputEvent, cx| this.reckon(cx)).detach();
            self.notes.insert(item, input);
        }
        self.reckon(cx);
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        let Some(stop) = self.stop(cx) else { return };
        let Some(key) = self.key.clone() else {
            self.said = Some("The app has no key of yours, so it cannot answer as you.".into());
            cx.notify();
            return;
        };
        let answer = self.answer(&stop, cx);
        self.sending = true;
        self.said = None;
        cx.notify();
        let touch = stop.ahead.as_ref().map(|ahead| crate::presence::ask(&format!("send your answer to Jaynes: {ahead}")));
        cx.spawn(async move |this, cx| {
            if let Some(touch) = touch {
                let given = touch.recv().await.unwrap_or_else(|_| Err("no answer from macOS".into()));
                if let Err(e) = given {
                    let _ = this.update(cx, |this, cx| {
                        this.sending = false;
                        this.said = Some(format!("Not sent: {e}"));
                        cx.notify();
                    });
                    return;
                }
            }
            let home = jaynes_shell::home();
            let signature = key.sign(&jaynes_shell::person::message(&stop.run, &answer));
            let sent = jaynes_shell::store::Store::open(&home.join("record.sqlite3"))
                .and_then(|s| s.state(&stop.run))
                .and_then(|r| r.ok_or_else(|| "the run is gone".to_string()))
                .and_then(|run| jaynes_shell::person::submit(&home, &engine_program(), &run, answer, &key.public(), &signature));
            let _ = this.update(cx, |this, cx| {
                this.sending = false;
                this.said = Some(match sent {
                    Ok(_) => "Sent as yours. The run goes on.".into(),
                    Err(e) => format!("Not sent: {e}"),
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn context_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(c) = &self.context {
            return c.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Your words for the agents that work after this (optional)"));
        self.context = Some(input.clone());
        input
    }
}

fn button(id: String, text: &str, on: bool, color: u32) -> Stateful<Div> {
    div()
        .id(SharedString::from(id))
        .px(px(10.))
        .py(px(4.))
        .rounded(px(7.))
        .border_1()
        .text_size(px(12.5))
        .cursor_pointer()
        .when(on, |d| d.bg(rgb(color)).border_color(rgb(color)).text_color(rgb(look::ON_FILL)))
        .when(!on, |d| d.border_color(rgb(look::LINE_2)).text_color(rgb(look::DIM)).hover(|d| d.border_color(rgb(color)).text_color(rgb(look::TEXT))))
        .child(text.to_string())
}

/// A text field in a box of its own height: the field fills whatever holds it.
fn field(input: &Entity<InputState>) -> Div {
    div().h(px(34.)).px(px(10.)).flex().items_center().rounded(px(8.)).bg(rgb(look::PANEL)).border_1().border_color(rgb(look::LINE)).text_size(px(13.)).child(Input::new(input))
}

fn colour_of(v: VerdictKind) -> u32 {
    match v {
        VerdictKind::Keep => look::GREEN,
        VerdictKind::Drop => look::RED,
        VerdictKind::Change | VerdictKind::Redo => look::AMBER,
        VerdictKind::Skip => look::GREY,
    }
}

impl Render for Island {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(stop) = self.stop(cx) else { return div().into_any_element() };
        let project = self.runs.read(cx).chosen().map(|d| d.project.clone()).unwrap_or_default();
        let shrunk = self.shrunk.contains(&stop.question);
        let wrap = div().absolute().bottom(px(18.)).left(px(18.)).right(px(18.)).flex().justify_center().font_family(look::UI_FONT);
        if shrunk {
            let q = stop.question.clone();
            return wrap
                .child(
                    div()
                        .id("pill")
                        .px(px(14.))
                        .py(px(8.))
                        .rounded(px(999.))
                        .bg(rgb(look::PANEL_2))
                        .border_1()
                        .border_color(rgb(look::AMBER))
                        .text_size(px(13.))
                        .text_color(rgb(look::TEXT))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.shrunk.remove(&q);
                            cx.notify();
                        }))
                        .child(format!("◆ {} · open", stop.title)),
                )
                .into_any_element();
        }
        let judged = stop.items.iter().filter(|i| self.marks.contains_key(&i.id)).count();
        let all = judged == stop.items.len();
        let mut items = div().flex().flex_col().gap(px(10.));
        for it in &stop.items {
            let mut verdicts = div().flex().gap(px(6.)).flex_none();
            for v in &stop.offered {
                let (item, v) = (it.id.clone(), *v);
                let on = self.marks.get(&it.id) == Some(&v);
                verdicts = verdicts.child(
                    button(format!("v-{}-{v:?}", it.id), stop::label(v, stop.permission), on, colour_of(v))
                        .on_click(cx.listener(move |this, _, window, cx| this.mark(item.clone(), v, window, cx))),
                );
            }
            let mut body = div().flex().flex_col().gap(px(4.)).min_w_0().flex_1().child(div().text_size(px(14.)).text_color(rgb(look::TEXT)).child(it.label.clone()));
            if !it.detail.is_empty() {
                body = body.child(div().text_size(px(12.5)).text_color(rgb(look::DIM)).child(it.detail.clone()));
            }
            for (label, text) in &it.aspects {
                body = body.child(div().flex().gap(px(6.)).text_size(px(12.)).child(div().text_color(rgb(look::FAINT)).child(label.clone())).child(div().text_color(rgb(look::DIM)).child(text.clone())));
            }
            if !it.pictures.is_empty() {
                let mut pics = div().flex().gap(px(8.));
                for p in it.pictures.iter().take(3) {
                    pics = pics.child(img(p.clone()).h(px(110.)).rounded(px(6.)));
                }
                body = body.child(pics);
            }
            let words = self.marks.get(&it.id).filter(|v| takes_words(**v)).and_then(|_| self.notes.get(&it.id).cloned());
            let row = div().flex().flex_col().gap(px(6.)).p(px(10.)).rounded(px(10.)).bg(rgb(look::PANEL)).border_1().border_color(rgb(look::LINE))
                .child(div().flex().gap(px(12.)).items_start().child(body).child(verdicts))
                .when_some(words, |d, w| d.child(field(&w)));
            items = items.child(row);
        }
        let context = self.context_input(window, cx);
        let mut effects = div().flex().flex_col().gap(px(3.));
        for e in &self.effects {
            effects = effects.child(div().text_size(px(12.)).text_color(rgb(look::DIM)).child(e.clone()));
        }
        let send_label = match (&stop.ahead, self.sending) {
            (_, true) => "Sending…".to_string(),
            (Some(_), _) => "Send · Touch ID".to_string(),
            (None, _) => "Send".to_string(),
        };
        let can = all && !self.sending && self.refused.is_none();
        let q = stop.question.clone();
        let card = div()
            .id("island")
            .w_full()
            .max_w(px(820.))
            .max_h(px(620.))
            .overflow_y_scroll()
            .p(px(16.))
            .rounded(px(14.))
            .bg(rgb(look::PANEL_2))
            .border_1()
            .border_color(rgb(look::LINE_2))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .items_center()
                    .child(div().size(px(8.)).rounded_full().bg(rgb(look::AMBER)))
                    .child(div().text_size(px(13.)).text_color(rgb(look::TEXT)).child(project))
                    .child(div().flex_1().truncate().text_size(px(12.)).text_color(rgb(look::FAINT)).child(format!("at {}", stop.step)))
                    .child(button("shrink".into(), "Esc · later", false, look::GREY).on_click(cx.listener(move |this, _, _, cx| {
                        this.shrunk.insert(q.clone());
                        cx.notify();
                    }))),
            )
            .child(div().text_size(px(16.)).text_color(rgb(look::TEXT)).child(stop.title.clone()))
            .child(items)
            .child(field(&context))
            .when(!self.effects.is_empty(), |d| d.child(div().flex().flex_col().gap(px(4.)).child(look::label("What this does", look::FAINT)).child(effects)))
            .when_some(stop.ahead.clone(), |d, ahead| d.child(div().text_size(px(12.)).text_color(rgb(look::AMBER)).child(format!("Ahead: {ahead}. Sending asks for your touch."))))
            .when_some(self.refused.clone(), |d, r| d.child(div().text_size(px(12.)).text_color(rgb(look::RED)).child(format!("Jaynes would refuse this: {r}"))))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(div().flex_1().text_size(px(12.)).text_color(rgb(look::FAINT)).child(match &self.said {
                        Some(s) => s.clone(),
                        None => format!("{judged} of {} judged", stop.items.len()),
                    }))
                    .child(
                        div()
                            .id("send")
                            .px(px(14.))
                            .py(px(7.))
                            .rounded(px(8.))
                            .text_size(px(13.))
                            .when(can, |d| d.bg(rgb(look::BLUE)).text_color(rgb(look::ON_FILL)).cursor_pointer().on_click(cx.listener(|this, _, _, cx| this.send(cx))))
                            .when(!can, |d| d.bg(rgb(look::LINE)).text_color(rgb(look::FAINT)))
                            .child(send_label),
                    ),
            );
        wrap.child(card).into_any_element()
    }
}
