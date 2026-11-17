//! The stop, risen over the terminal as an island: the question the picked run waits at, a verdict per item, the
//! person's words where they send something back, what they dropped earlier and may bring back, what the answer will
//! do in the engine's own sentences (G11), and Send. The person may also answer in their own words: Jaynes reads them
//! into the answer they mean and shows what it does; nothing goes until they send it. Esc · later shrinks it to a pill.
//! A stop that chooses among designs opens the compare mode instead, over the whole window.

use crate::answer::{self, Reckoned, Understood};
use crate::compare::Compare;
use crate::key::Key;
use crate::lightbox::{Lightbox, Shown};
use crate::look;
use crate::model::Runs;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_base::input::{Input, InputEvent, InputState};
use jaynes_core::question::VerdictKind;
use jaynes_shell::stop::{self, Stop};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

pub struct Island {
    runs: Entity<Runs>,
    key: Option<Arc<Key>>,
    lightbox: Entity<Lightbox>,
    compare: Entity<Compare>,
    /// The question the marks below are for: new marks for a new question.
    question: Option<String>,
    marks: BTreeMap<String, VerdictKind>,
    notes: HashMap<String, Entity<InputState>>,
    words: Option<Entity<InputState>>,
    /// What the person brings back of what they dropped, with their words on how.
    bring: HashSet<String>,
    bring_words: HashMap<String, Entity<InputState>>,
    show_back: bool,
    reckoned: Reckoned,
    reading: bool,
    understood: Option<Result<Understood, String>>,
    shrunk: HashSet<String>,
    sending: bool,
    said: Option<String>,
    /// The tabs over the middle, so a test can open one (`tab <run>`, `tab main`).
    pub tabs: Option<Entity<crate::tabs::Tabs>>,
    /// The Changes panel, for a test that clicks a file in it.
    pub changes: Option<Entity<crate::changes::Changes>>,
}

/// A verdict that sends the item back takes the person's words.
fn takes_words(v: VerdictKind) -> bool {
    matches!(v, VerdictKind::Change | VerdictKind::Redo)
}

fn word(v: VerdictKind) -> &'static str {
    match v {
        VerdictKind::Keep => "keep",
        VerdictKind::Change => "change",
        VerdictKind::Redo => "redo",
        VerdictKind::Drop => "drop",
        VerdictKind::Skip => "skip",
    }
}

/// A text field in a box of its own height: the field fills whatever holds it.
pub fn field(input: &Entity<InputState>) -> Div {
    div().h(px(34.)).px(px(10.)).flex().items_center().rounded(px(8.)).bg(rgb(look::PANEL)).border_1().border_color(rgb(look::LINE)).text_size(px(13.)).text_color(rgb(look::TEXT)).child(Input::new(input))
}

pub fn button(id: String, text: &str, on: bool, color: u32) -> Stateful<Div> {
    div()
        .id(SharedString::from(id))
        .px(px(10.))
        .py(px(4.))
        .rounded(px(7.))
        .border_1()
        .text_size(px(12.5))
        .cursor_pointer()
        .flex_none()
        .when(on, |d| d.bg(rgb(color)).border_color(rgb(color)).text_color(rgb(look::ON_FILL)))
        .when(!on, |d| d.border_color(rgb(look::LINE_2)).text_color(rgb(look::DIM)).hover(|d| d.border_color(rgb(color)).text_color(rgb(look::TEXT))))
        .child(text.to_string())
}

pub fn primary(id: &str, text: String, can: bool) -> Stateful<Div> {
    div()
        .id(SharedString::from(id.to_string()))
        .px(px(14.))
        .py(px(7.))
        .rounded(px(8.))
        .text_size(px(13.))
        .flex_none()
        .when(can, |d| d.bg(rgb(look::BLUE)).text_color(rgb(look::ON_FILL)).cursor_pointer())
        .when(!can, |d| d.bg(rgb(look::LINE)).text_color(rgb(look::FAINT)))
        .child(text)
}

fn colour_of(v: VerdictKind) -> u32 {
    match v {
        VerdictKind::Keep => look::GREEN,
        VerdictKind::Drop => look::RED,
        VerdictKind::Change | VerdictKind::Redo => look::AMBER,
        VerdictKind::Skip => look::GREY,
    }
}

pub fn sentences(list: &[String]) -> Div {
    let mut d = div().flex().flex_col().gap(px(3.));
    for s in list {
        d = d.child(div().text_size(px(12.5)).text_color(rgb(look::DIM)).child(s.clone()));
    }
    d
}

impl Island {
    pub fn new(runs: Entity<Runs>, key: Option<Arc<Key>>, lightbox: Entity<Lightbox>, compare: Entity<Compare>, cx: &mut Context<Self>) -> Self {
        cx.observe(&runs, |this, _, cx| {
            this.follow(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&compare, |_, _, cx| cx.notify()).detach();
        if let Some(file) = std::env::var_os("JAYNES_APP_TEST_DO") {
            drive_from(PathBuf::from(file), cx);
        }
        Island {
            runs,
            key,
            lightbox,
            compare,
            question: None,
            marks: BTreeMap::new(),
            notes: HashMap::new(),
            words: None,
            bring: HashSet::new(),
            bring_words: HashMap::new(),
            show_back: false,
            reckoned: Reckoned::default(),
            reading: false,
            understood: None,
            shrunk: HashSet::new(),
            sending: false,
            said: None,
            tabs: None,
            changes: None,
        }
    }

    /// New marks when the picked run waits at another question.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let now = self.runs.read(cx).stop().map(|s| s.question.clone());
        if now != self.question {
            self.question = now;
            self.marks.clear();
            self.notes.clear();
            self.words = None;
            self.bring.clear();
            self.bring_words.clear();
            self.show_back = false;
            self.reckoned = Reckoned::default();
            self.understood = None;
            self.said = None;
        }
    }

    /// Brings the picked run's question up: out of its pill, and into the compare mode when it chooses among designs.
    pub fn open_stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(stop) = self.stop(cx) else { return };
        self.shrunk.remove(&stop.question);
        if stop.choose {
            self.compare.update(cx, |c, cx| c.open(stop, window, cx));
        }
        cx.notify();
    }

    fn stop(&self, cx: &App) -> Option<Stop> {
        self.runs.read(cx).stop().cloned()
    }

    fn text(input: Option<&Entity<InputState>>, cx: &App) -> String {
        input.map(|i| i.read(cx).value().to_string()).unwrap_or_default()
    }

    /// The answer as marked, as the body the page would send.
    fn body(&self, stop: &Stop, cx: &App) -> serde_json::Value {
        let verdicts = self.marks.iter().map(|(id, v)| (id.clone(), word(*v).to_string())).collect();
        let notes = self.marks.iter().filter(|(_, v)| takes_words(**v)).map(|(id, _)| (id.clone(), Self::text(self.notes.get(id), cx))).collect();
        let back = self.bring.iter().map(|id| (id.clone(), Self::text(self.bring_words.get(id), cx))).collect();
        answer::body(stop, &verdicts, &notes, &back, &Self::text(self.words.as_ref(), cx))
    }

    fn reckon(&mut self, cx: &mut Context<Self>) {
        let Some(stop) = self.stop(cx) else { return };
        self.reckoned = answer::reckon(&self.body(&stop, cx));
        cx.notify();
    }

    fn input(&self, placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        cx.subscribe(&input, |this, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                this.reckon(cx);
            }
        })
        .detach();
        input
    }

    fn mark(&mut self, item: String, v: VerdictKind, window: &mut Window, cx: &mut Context<Self>) {
        self.marks.insert(item.clone(), v);
        if takes_words(v) && !self.notes.contains_key(&item) {
            let input = self.input(if v == VerdictKind::Change { "how should it change?" } else { "what is missing?" }, window, cx);
            self.notes.insert(item, input);
        }
        self.reckon(cx);
    }

    fn toggle_bring(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.bring.remove(&id) {
            self.bring.insert(id.clone());
            if !self.bring_words.contains_key(&id) {
                let input = self.input("as it was, or how it should come back", window, cx);
                self.bring_words.insert(id, input);
            }
        }
        self.reckon(cx);
    }

    /// Reads the person's words into the answer they mean, on a thread of its own: a model reads them.
    fn read_words(&mut self, cx: &mut Context<Self>) {
        let Some(stop) = self.stop(cx) else { return };
        let words = Self::text(self.words.as_ref(), cx);
        if words.trim().is_empty() {
            return;
        }
        self.reading = true;
        self.understood = None;
        cx.notify();
        let read = cx.background_spawn(async move { answer::understand(&stop, &words, None) });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            let _ = this.update(cx, |this, cx| {
                this.reading = false;
                this.understood = Some(read);
                cx.notify();
            });
        })
        .detach();
    }

    fn send_body(&mut self, body: serde_json::Value, cx: &mut Context<Self>) {
        let Some(stop) = self.stop(cx) else { return };
        self.sending = true;
        self.said = None;
        cx.notify();
        let sent = answer::send(self.key.clone(), stop.ahead.clone(), body);
        cx.spawn(async move |this, cx| {
            let sent = sent.await;
            let _ = this.update(cx, |this, cx| {
                this.sending = false;
                this.said = Some(match sent {
                    Ok(()) => "Sent as yours. The run goes on.".into(),
                    Err(e) => format!("Not sent: {e}"),
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Whether the answer as marked may go: every item judged (or something brought back), nothing refused, not
    /// already on its way.
    fn can_send(&self, stop: &Stop) -> bool {
        let judged = stop.items.iter().filter(|i| self.marks.contains_key(&i.id)).count();
        (judged == stop.items.len() || !self.bring.is_empty()) && !self.sending && self.reckoned.refused.is_none()
    }

    /// Command-Return: sends the answer, when the question shows open and the answer may go.
    pub fn send_if_ready(&mut self, cx: &mut Context<Self>) {
        let Some(stop) = self.stop(cx) else { return };
        if stop.choose || self.compare.read(cx).is_open() || self.shrunk.contains(&stop.question) || !self.can_send(&stop) {
            return;
        }
        self.send(cx);
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        let Some(stop) = self.stop(cx) else { return };
        let body = self.body(&stop, cx);
        self.send_body(body, cx);
    }

    fn send_understood(&mut self, cx: &mut Context<Self>) {
        let Some(Ok(u)) = self.understood.clone() else { return };
        let Some(stop) = self.stop(cx) else { return };
        let mut body = u.body;
        for (k, v) in [("run", stop.run.clone()), ("question", stop.question.clone()), ("shown", stop.shown.clone())] {
            body[k] = serde_json::json!(v);
        }
        self.send_body(body, cx);
    }

    fn picture(&self, title: String, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.lightbox.update(cx, |l, cx| l.show(Shown::Picture { title, path }, window, cx));
    }

    fn diff(&self, stop: &Stop, window: &mut Window, cx: &mut Context<Self>) {
        let text = jaynes_shell::drive::step_diff(&jaynes_shell::home(), &stop.run, &stop.step).unwrap_or_default();
        let text = if text.trim().is_empty() { "This step changed nothing in the project.".to_string() } else { text };
        let title = format!("What {} changed", stop.step);
        self.lightbox.update(cx, |l, cx| l.show(Shown::Diff { title, text }, window, cx));
    }

    fn item_view(&self, it: &stop::Item, stop: &Stop, back: bool, cx: &mut Context<Self>) -> Div {
        let mut body = div().flex().flex_col().gap(px(4.)).min_w_0().flex_1().child(div().text_size(px(14.)).text_color(rgb(look::TEXT)).child(it.label.clone()));
        if !it.detail.is_empty() {
            body = body.child(div().text_size(px(12.5)).text_color(rgb(look::DIM)).child(it.detail.clone()));
        }
        for (label, text) in &it.aspects {
            body = body.child(div().flex().gap(px(6.)).text_size(px(12.)).child(div().flex_none().text_color(rgb(look::FAINT)).child(label.clone())).child(div().text_color(rgb(look::DIM)).child(text.clone())));
        }
        if !it.pictures.is_empty() {
            let mut pics = div().flex().gap(px(8.));
            for (n, p) in it.pictures.iter().take(4).enumerate() {
                let (path, title) = (p.clone(), it.label.clone());
                pics = pics.child(
                    div()
                        .id(SharedString::from(format!("pic-{}-{n}", it.id)))
                        .cursor_pointer()
                        .rounded(px(6.))
                        .overflow_hidden()
                        .border_1()
                        .border_color(rgb(look::LINE))
                        .on_click(cx.listener(move |this, _, window, cx| this.picture(title.clone(), path.clone(), window, cx)))
                        .child(img(p.clone()).h(px(110.))),
                );
            }
            body = body.child(pics);
        }
        let mut controls = div().flex().gap(px(6.)).flex_none();
        if back {
            let id = it.id.clone();
            controls = controls.child(button(format!("bring-{}", it.id), "Bring back", self.bring.contains(&it.id), look::AMBER).on_click(cx.listener(move |this, _, window, cx| this.toggle_bring(id.clone(), window, cx))));
        } else {
            for v in &stop.offered {
                let (item, v) = (it.id.clone(), *v);
                let on = self.marks.get(&it.id) == Some(&v);
                controls = controls.child(button(format!("v-{}-{v:?}", it.id), stop::label(v, stop.permission), on, colour_of(v)).on_click(cx.listener(move |this, _, window, cx| this.mark(item.clone(), v, window, cx))));
            }
        }
        let words = if back {
            self.bring.contains(&it.id).then(|| self.bring_words.get(&it.id).cloned()).flatten()
        } else {
            self.marks.get(&it.id).filter(|v| takes_words(**v)).and_then(|_| self.notes.get(&it.id).cloned())
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(10.))
            .rounded(px(10.))
            .bg(rgb(look::PANEL))
            .border_1()
            .border_color(rgb(look::LINE))
            .child(div().flex().gap(px(12.)).items_start().child(body).child(controls))
            .when_some(words, |d, w| d.child(field(&w)))
    }

    /// A stop that chooses among designs: an invitation to compare them over the whole window.
    fn invite(&self, stop: &Stop, cx: &mut Context<Self>) -> Div {
        let open = {
            let (compare, stop) = (self.compare.clone(), stop.clone());
            cx.listener(move |_, _, window, cx| compare.update(cx, |c, cx| c.open(stop.clone(), window, cx)))
        };
        let q = stop.question.clone();
        div()
            .flex()
            .gap(px(12.))
            .items_center()
            .px(px(16.))
            .py(px(12.))
            .rounded(px(12.))
            .bg(rgb(look::PANEL_2))
            .border_1()
            .border_color(rgba((look::AMBER << 8) | 0x73))
            .child(div().text_size(px(14.)).text_color(rgb(look::AMBER)).flex_none().child(format!("◆ {} options", stop.items.len())))
            .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).text_color(rgb(look::TEXT)).child(stop.title.clone()))
            .child(primary("compare", "Compare".into(), true).on_click(open))
            .child(button("later".into(), "Esc · later", false, look::GREY).on_click(cx.listener(move |this, _, _, cx| {
                this.shrunk.insert(q.clone());
                cx.notify();
            })))
    }
}

/// With JAYNES_APP_TEST_DO=<file>, a test says what the person does, a line at a time: `mark <item> <verdict>`,
/// `words <text>`, `read`, `bring <item>`, `tab <run>`, `agent <address>`, `notice <agent|run>`, `at <run> top|end|<step>`, `waits`, `do <action>`, `key <keystroke>`, `front`, `zoom`, `move-tab <run|main> <place>`, `diff`, `picture`, `send`, `send-read`, `compare`, and in the compare mode the lines it reads.
/// Each goes through what a click does, so a test answers without the person's mouse.
fn drive_from(file: PathBuf, cx: &mut Context<Island>) {
    cx.spawn(async move |this, cx| loop {
        cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        if text.trim().is_empty() {
            continue;
        }
        let _emptied = std::fs::write(&file, "");
        for line in text.lines() {
            let line = line.to_string();
            let Some(window) = cx.update(|cx| cx.windows().first().copied()) else { continue };
            let done = window.update(cx, |_, window, cx| this.update(cx, |this, cx| this.driven(&line, window, cx)));
            if !matches!(done, Ok(Ok(()))) {
                return;
            }
        }
    })
    .detach();
}

impl Island {
    fn driven(&mut self, line: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
        if self.compare.read(cx).is_open() {
            let line = line.to_string();
            self.compare.update(cx, |c, cx| c.driven(&line, window, cx));
            return;
        }
        match verb {
            "mark" => {
                let (item, v) = rest.split_once(' ').unwrap_or((rest, "keep"));
                let v = match v {
                    "keep" => VerdictKind::Keep,
                    "drop" => VerdictKind::Drop,
                    "redo" => VerdictKind::Redo,
                    "change" => VerdictKind::Change,
                    _ => VerdictKind::Skip,
                };
                self.mark(item.to_string(), v, window, cx);
            }
            "bring" => {
                self.show_back = true;
                self.toggle_bring(rest.to_string(), window, cx);
            }
            "words" => {
                if let Some(w) = self.words.clone() {
                    let text = rest.to_string();
                    w.update(cx, |s, cx| s.set_value(text, window, cx));
                }
            }
            "read" => self.read_words(cx),
            "close-box" => self.lightbox.update(cx, |l, cx| l.close(cx)),
            "menubar" => crate::menubar::click(),
            "select" => {
                let id = rest.to_string();
                self.runs.update(cx, |r, cx| r.select(id, cx));
            }
            "tab" => {
                let run = (rest != "main").then(|| rest.to_string());
                if let Some(tabs) = self.tabs.clone() {
                    tabs.update(cx, |t, cx| t.show(run, cx));
                }
            }
            // an agent from the column, as a click on it opens it
            "agent" => {
                if let Some(tabs) = self.tabs.clone() {
                    tabs.update(cx, |t, cx| t.show_agent(rest.to_string(), rest.to_string(), cx));
                }
            }
            // a click on the system's notice about an agent or a run
            "notice" => crate::notify::click(rest.to_string()),
            // a file in the Changes panel, as a click on it stacks it in the Changes tab
            "file" => {
                if let Some(changes) = self.changes.clone() {
                    changes.update(cx, |c, cx| c.open_file(rest.to_string(), cx));
                }
            }
            "waits" => self.open_stop(window, cx),
            // an action the window takes, by its name: what a shortcut or a menu does
            "do" => match cx.build_action(&format!("jaynes::{rest}"), None) {
                Ok(action) => window.dispatch_action(action, cx),
                Err(e) => tracing::warn!("no action {rest}: {e}"),
            },
            // a key pressed, through the window's keys as a press on the keyboard goes: `key cmd-w`
            "key" => match Keystroke::parse(rest) {
                // after this line is done, as a key comes from the keyboard: not inside the island's own update
                Ok(k) => window.defer(cx, move |window, cx| {
                    window.dispatch_keystroke(k, cx);
                }),
                Err(e) => tracing::warn!("no key {rest}: {e}"),
            },
            // the window in front and active, so real events from Jaynes Input (app/tools/input) reach it
            "front" => {
                cx.activate(true);
                window.activate_window();
            }
            // the window filling the screen, where a test knows its place
            "zoom" => window.zoom_window(),
            "move-tab" => {
                let (key, to) = rest.split_once(' ').unwrap_or((rest, "0"));
                let key = (key != "main").then(|| key.to_string());
                if let (Some(tabs), Ok(to)) = (self.tabs.clone(), to.parse()) {
                    tabs.update(cx, |t, cx| t.move_tab(&key, to, cx));
                }
            }
            "at" => {
                let (run, at) = rest.split_once(' ').unwrap_or((rest, "top"));
                let place = match at {
                    "top" => crate::tabs::Place::Top,
                    "end" => crate::tabs::Place::End,
                    step => crate::tabs::Place::Step(step.to_string()),
                };
                let run = run.to_string();
                if let Some(tabs) = self.tabs.clone() {
                    tabs.update(cx, |t, cx| t.show_at(run, place, cx));
                }
            }
            "diff" => {
                if let Some(stop) = self.stop(cx) {
                    self.diff(&stop, window, cx);
                }
            }
            "picture" => {
                if let Some(it) = self.stop(cx).and_then(|s| s.items.into_iter().find(|i| !i.pictures.is_empty())) {
                    self.picture(it.label.clone(), it.pictures[0].clone(), window, cx);
                }
            }
            "send" => self.send(cx),
            "send-read" => self.send_understood(cx),
            "compare" => {
                if let Some(stop) = self.stop(cx) {
                    self.compare.update(cx, |c, cx| c.open(stop, window, cx));
                }
            }
            _ => {
                let line = line.to_string();
                self.compare.update(cx, |c, cx| c.driven(&line, window, cx));
            }
        }
    }
}

impl Render for Island {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(stop) = self.stop(cx) else { return div().into_any_element() };
        if self.compare.read(cx).is_open() {
            return div().into_any_element();
        }
        let project = self.runs.read(cx).chosen().map(|d| d.project.clone()).unwrap_or_default();
        // docked under the terminal, which shrinks to make room: the stop never covers the line where the person types
        let wrap = div().w_full().flex_none().px(px(14.)).pb(px(12.)).pt(px(4.)).flex().justify_center().font_family(look::UI_FONT);
        if self.shrunk.contains(&stop.question) {
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
        if stop.choose {
            return wrap.child(div().w_full().child(self.invite(&stop, cx))).into_any_element();
        }
        if self.words.is_none() {
            self.words = Some(self.input("Your words: say what you think, in any language (or answer with the buttons)", window, cx));
        }
        let words = self.words.clone().expect("made above");
        let judged = stop.items.iter().filter(|i| self.marks.contains_key(&i.id)).count();
        let can = self.can_send(&stop);
        let mut items = div().flex().flex_col().gap(px(10.));
        for it in &stop.items {
            items = items.child(self.item_view(it, &stop, false, cx));
        }
        let mut back = div().flex().flex_col().gap(px(8.));
        if !stop.back.is_empty() {
            let open = self.show_back;
            back = back.child(
                div()
                    .id("back")
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .text_color(rgb(look::DIM))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_back = !this.show_back;
                        cx.notify();
                    }))
                    .child(format!("{} You dropped {} earlier: only you bring it back", if open { "▾" } else { "▸" }, stop.back.len())),
            );
            if open {
                for it in &stop.back {
                    back = back.child(self.item_view(it, &stop, true, cx));
                }
            }
        }
        let understood = match (&self.understood, self.reading) {
            (_, true) => Some(div().text_size(px(12.5)).text_color(rgb(look::FAINT)).child("Reading your words…")),
            (Some(Err(e)), _) => Some(div().text_size(px(12.5)).text_color(rgb(look::RED)).child(e.clone())),
            (Some(Ok(u)), _) => {
                let ok = u.refused.is_none() && !self.sending;
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .p(px(10.))
                        .rounded(px(10.))
                        .bg(rgb(look::PANEL))
                        .border_1()
                        .border_color(rgb(look::BLUE))
                        .child(look::label("What Jaynes read · this is what it does when you send", look::BLUE))
                        .child(sentences(&u.sentences))
                        .when(!u.unsure.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(rgb(look::AMBER)).child(format!("Not sure: {}", u.unsure))))
                        .when(!u.repeats.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(rgb(look::AMBER)).child(format!("You asked this in an earlier round: {}", u.repeats.join("; ")))))
                        .when_some(u.refused.clone(), |d, r| d.child(div().text_size(px(12.)).text_color(rgb(look::RED)).child(r)))
                        .child(
                            div()
                                .flex()
                                .gap(px(8.))
                                .child(primary("send-read", "Send this".into(), ok).when(ok, |d| d.on_click(cx.listener(|this, _, _, cx| this.send_understood(cx)))))
                                .child(button("by-hand".into(), "Answer with the buttons", false, look::GREY).on_click(cx.listener(|this, _, _, cx| {
                                    this.understood = None;
                                    cx.notify();
                                }))),
                        ),
                )
            }
            _ => None,
        };
        let send_label = match (&stop.ahead, self.sending) {
            (_, true) => "Sending…".to_string(),
            (Some(_), _) => "Send · Touch ID".to_string(),
            (None, _) => "Send".to_string(),
        };
        let q = stop.question.clone();
        let diff_stop = stop.clone();
        let card = div()
            .id("island")
            .w_full()
            .max_h(px(460.))
            .overflow_y_scroll()
            .p(px(16.))
            .rounded(px(12.))
            .bg(rgb(look::PANEL_2))
            .border_1()
            .border_color(rgba((look::AMBER << 8) | 0x73))
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
                    .child(div().flex_1().truncate().text_size(px(12.)).text_color(rgb(look::FAINT)).child(if stop.round > 0 { format!("at {} · round {}", stop.step, stop.round) } else { format!("at {}", stop.step) }))
                    .child(button("diff".into(), "See the diff", false, look::BLUE).on_click(cx.listener(move |this, _, window, cx| this.diff(&diff_stop, window, cx))))
                    .child(button("shrink".into(), "Esc · later", false, look::GREY).on_click(cx.listener(move |this, _, _, cx| {
                        this.shrunk.insert(q.clone());
                        cx.notify();
                    }))),
            )
            .child(div().text_size(px(16.)).text_color(rgb(look::TEXT)).child(stop.title.clone()))
            .child(items)
            .child(back)
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .child(div().flex_1().child(field(&words)))
                    .child(button("read".into(), "Read my words", false, look::BLUE).on_click(cx.listener(|this, _, _, cx| this.read_words(cx)))),
            )
            .children(understood)
            .when(!self.reckoned.sentences.is_empty(), |d| d.child(div().flex().flex_col().gap(px(4.)).child(look::label("What this does", look::FAINT)).child(sentences(&self.reckoned.sentences))))
            .when_some(stop.ahead.clone(), |d, ahead| d.child(div().text_size(px(12.)).text_color(rgb(look::AMBER)).child(format!("Ahead: {ahead}. Sending asks for your touch."))))
            .when_some(self.reckoned.refused.clone().filter(|_| judged > 0), |d, r| d.child(div().text_size(px(12.)).text_color(rgb(look::RED)).child(format!("Jaynes would refuse this: {r}"))))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(div().flex_1().text_size(px(12.)).text_color(rgb(look::FAINT)).child(match &self.said {
                        Some(s) => s.clone(),
                        None => format!("{judged} of {} judged", stop.items.len()),
                    }))
                    .child(primary("send", send_label, can).when(can, |d| d.on_click(cx.listener(|this, _, _, cx| this.send(cx))))),
            );
        wrap.child(card).into_any_element()
    }
}
