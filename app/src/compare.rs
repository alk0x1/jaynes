//! The compare mode, over the whole window, for a stop that chooses among designs. Each direction at full size, flipped
//! in place with the arrow keys; its parts beside, each with a pin that says "this stays"; up to two directions starred
//! to keep as finalists; none, with what was missing; the person's own words; and one button that says what it does:
//! go on with one, mix the parts pinned in two, develop the two starred, or redo them all. What the answer will do is
//! the engine's sentences, and it goes signed with the person's key like every answer from the app.

use crate::answer::{self, Reckoned, Understood};
use crate::island::{button, field, primary, sentences};
use crate::key::Key;
use crate::look;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_base::input::{InputEvent, InputState};
use jaynes_shell::stop::Stop;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const BG: u32 = look::PANEL;
const FINALIST: &str = "A finalist: develop it further, keeping what it is.";

pub struct Compare {
    /// Whether every sentence of what the answer does shows, or only the first.
    all_said: bool,
    key: Option<Arc<Key>>,
    stop: Option<Stop>,
    open: bool,
    dir: usize,
    pic: usize,
    /// The parts that stay, by direction and the part's label.
    pins: BTreeSet<(usize, String)>,
    /// The directions starred to keep as finalists.
    keep: Vec<usize>,
    none: bool,
    /// What the person brings back of what they dropped in an earlier round.
    bring: Option<String>,
    why: Option<Entity<InputState>>,
    words: Option<Entity<InputState>>,
    reckoned: Reckoned,
    reading: bool,
    understood: Option<Result<Understood, String>>,
    sending: bool,
    said: Option<String>,
    focus: FocusHandle,
}

/// A direction's short name: its label before the colon.
fn name(label: &str) -> String {
    label.split_once(':').map(|(n, _)| n).unwrap_or(label).trim().to_string()
}

/// Its idea: the label after the colon.
fn idea(label: &str) -> String {
    label.split_once(':').map(|(_, i)| i.trim().to_string()).unwrap_or_default()
}

impl Compare {
    pub fn new(key: Option<Arc<Key>>, cx: &mut Context<Self>) -> Self {
        Compare {
            all_said: false,
            key,
            stop: None,
            open: false,
            dir: 0,
            pic: 0,
            pins: BTreeSet::new(),
            keep: Vec::new(),
            none: false,
            bring: None,
            why: None,
            words: None,
            reckoned: Reckoned::default(),
            reading: false,
            understood: None,
            sending: false,
            said: None,
            focus: cx.focus_handle(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens on a stop; what the person marked stays while it is the same question.
    pub fn open(&mut self, stop: Stop, window: &mut Window, cx: &mut Context<Self>) {
        if self.stop.as_ref().map(|s| &s.question) != Some(&stop.question) {
            self.dir = 0;
            self.pic = 0;
            self.pins.clear();
            self.keep.clear();
            self.none = false;
            self.bring = None;
            self.why = None;
            self.words = None;
            self.understood = None;
            self.said = None;
        }
        self.stop = Some(stop);
        self.open = true;
        if self.words.is_none() {
            self.words = Some(self.input("Your words: what stays, what changes, in any language", window, cx));
        }
        window.focus(&self.focus, cx);
        self.reckon(cx);
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
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

    fn text(input: Option<&Entity<InputState>>, cx: &App) -> String {
        input.map(|i| i.read(cx).value().to_string()).unwrap_or_default()
    }

    fn flip(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(n) = self.stop.as_ref().map(|s| s.items.len()).filter(|n| *n > 0) else { return };
        self.dir = (self.dir as isize + by).rem_euclid(n as isize) as usize;
        self.pic = 0;
        self.reckon(cx);
    }

    fn star(&mut self, d: usize, cx: &mut Context<Self>) {
        if let Some(i) = self.keep.iter().position(|k| *k == d) {
            self.keep.remove(i);
        } else {
            if self.keep.len() == 2 {
                self.keep.remove(0);
            }
            self.keep.push(d);
        }
        self.reckon(cx);
    }

    fn pin(&mut self, d: usize, label: String, cx: &mut Context<Self>) {
        if !self.pins.remove(&(d, label.clone())) {
            self.pins.insert((d, label));
        }
        self.reckon(cx);
    }

    /// What the main button does, in words, and the answer it sends.
    fn proposal(&self, cx: &App) -> Option<(String, Value)> {
        let stop = self.stop.as_ref()?;
        let names: Vec<String> = stop.items.iter().map(|i| name(&i.label)).collect();
        let words = Self::text(self.words.as_ref(), cx);
        let mut verdicts: BTreeMap<String, String> = stop.items.iter().map(|i| (i.id.clone(), "drop".to_string())).collect();
        let mut notes = BTreeMap::new();
        let pinned = |d: usize| self.pins.iter().filter(|(p, _)| *p == d).map(|(_, l)| l.clone()).collect::<Vec<_>>();
        if let Some(id) = &self.bring {
            // what comes back goes on, made again by the agent from the person's words; the current options leave
            let back: BTreeMap<String, String> = [(id.clone(), words.trim().to_string())].into();
            let label = format!("Bring back {}", stop.back.iter().find(|b| &b.id == id).map(|b| name(&b.label)).unwrap_or_default());
            return Some((label, answer::body(stop, &verdicts, &notes, &back, "")));
        }
        if self.none {
            let why = Self::text(self.why.as_ref(), cx);
            let context = [why.trim(), words.trim()].iter().filter(|t| !t.is_empty()).copied().collect::<Vec<_>>().join(" ");
            let label = format!("None of these: make {} new ones", stop.items.len());
            return Some((label, answer::body(stop, &verdicts, &notes, &BTreeMap::new(), &context)));
        }
        if self.keep.len() == 2 {
            for &k in &self.keep {
                let item = &stop.items[k];
                verdicts.insert(item.id.clone(), "change".into());
                let stays = pinned(k);
                let note = if stays.is_empty() { FINALIST.to_string() } else { format!("{FINALIST} Its {} stay.", stays.join(", ")) };
                notes.insert(item.id.clone(), note);
            }
            let label = format!("Develop {} and {}", names[self.keep[0]], names[self.keep[1]]);
            return Some((label, answer::body(stop, &verdicts, &notes, &BTreeMap::new(), &words)));
        }
        let dirs: BTreeSet<usize> = self.pins.iter().map(|(d, _)| *d).collect();
        if dirs.len() >= 2 {
            // the direction with the most pins is the base; the others lend it the parts pinned in them
            let base = *dirs.iter().max_by_key(|d| (pinned(**d).len(), **d == self.dir)).unwrap_or(&self.dir);
            let item = &stop.items[base];
            let mut note = format!("Mix: keep the {} of {}.", pinned(base).join(", "), names[base]);
            let mut parts = vec![];
            for &d in dirs.iter().filter(|d| **d != base) {
                for label in pinned(d) {
                    let text = stop.items[d].aspects.iter().find(|(l, _)| *l == label).map(|(_, t)| t.clone()).unwrap_or_default();
                    note.push_str(&format!(" Take the {label} of {}: {text}", names[d]));
                    parts.push(format!("{label} of {}", names[d]));
                }
            }
            verdicts.insert(item.id.clone(), "change".into());
            notes.insert(item.id.clone(), note);
            let label = format!("Mix: {} + {}", names[base], parts.join(" + "));
            return Some((label, answer::body(stop, &verdicts, &notes, &BTreeMap::new(), &words)));
        }
        let target = dirs.iter().next().copied().unwrap_or(self.dir);
        verdicts.insert(stop.items[target].id.clone(), "keep".into());
        Some((format!("Go on with {}", names[target]), answer::body(stop, &verdicts, &notes, &BTreeMap::new(), &words)))
    }

    fn reckon(&mut self, cx: &mut Context<Self>) {
        self.reckoned = self.proposal(cx).map(|(_, body)| answer::reckon(&body)).unwrap_or_default();
        cx.notify();
    }

    fn send_body(&mut self, body: Value, cx: &mut Context<Self>) {
        let Some(stop) = self.stop.clone() else { return };
        self.sending = true;
        self.said = None;
        cx.notify();
        let sent = answer::send(self.key.clone(), stop.ahead.clone(), body);
        cx.spawn(async move |this, cx| {
            let sent = sent.await;
            let _ = this.update(cx, |this, cx| {
                this.sending = false;
                match sent {
                    Ok(()) => {
                        this.said = Some("Sent as yours. The run goes on.".into());
                        this.open = false;
                    }
                    Err(e) => this.said = Some(format!("Not sent: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn go(&mut self, cx: &mut Context<Self>) {
        if self.sending || self.reckoned.refused.is_some() {
            return;
        }
        if let Some((_, body)) = self.proposal(cx) {
            self.send_body(body, cx);
        }
    }

    fn read_words(&mut self, cx: &mut Context<Self>) {
        let Some(stop) = self.stop.clone() else { return };
        let words = Self::text(self.words.as_ref(), cx);
        if words.trim().is_empty() {
            return;
        }
        let pressed = self.proposal(cx).map(|(label, _)| label);
        self.reading = true;
        self.understood = None;
        cx.notify();
        let read = cx.background_spawn(async move { answer::understand(&stop, &words, pressed.as_deref()) });
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

    fn send_understood(&mut self, cx: &mut Context<Self>) {
        let (Some(Ok(u)), Some(stop)) = (self.understood.clone(), self.stop.clone()) else { return };
        let mut body = u.body;
        for (k, v) in [("run", stop.run), ("question", stop.question), ("shown", stop.shown)] {
            body[k] = serde_json::json!(v);
        }
        self.send_body(body, cx);
    }

    fn open_live(&mut self, page: String, cx: &mut Context<Self>) {
        // a page an agent wrote: opened only through the page's sandbox, never as a file
        if std::net::TcpStream::connect(("127.0.0.1", jaynes_shell::install::PORT)).is_ok() {
            cx.open_url(&format!("http://127.0.0.1:{}/page/{page}", jaynes_shell::install::PORT));
        } else {
            self.said = Some("The live version opens through the Jaynes page, which is not running.".into());
            cx.notify();
        }
    }

    fn some_field_focused(&self, window: &Window, cx: &App) -> bool {
        [&self.words, &self.why].iter().filter_map(|f| f.as_ref()).any(|f| f.read(cx).focus_handle(cx).is_focused(window))
    }

    /// A test's line: `next`, `prev`, `star`, `pin <part>`, `none <why>`, `bring <item>`, `go`, `close`.
    pub fn driven(&mut self, line: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
        match verb {
            "next" => self.flip(1, cx),
            "prev" => self.flip(-1, cx),
            "star" => self.star(self.dir, cx),
            "pin" => self.pin(self.dir, rest.to_string(), cx),
            "none" => {
                self.none = true;
                let why = self.input("what was missing in all of them?", window, cx);
                let text = rest.to_string();
                why.update(cx, |s, cx| s.set_value(text, window, cx));
                self.why = Some(why);
                self.reckon(cx);
            }
            "bring" => {
                self.bring = Some(rest.to_string());
                self.reckon(cx);
            }
            "go" => self.go(cx),
            "close" => self.close(cx),
            _ => {}
        }
    }
}

impl Compare {
    /// What the answer does: its first sentence, and the others behind a click, so the decision stays in front.
    fn consequences(&self, cx: &mut Context<Self>) -> AnyElement {
        let said = &self.reckoned.sentences;
        if said.len() <= 1 || self.all_said {
            let mut d = sentences(said);
            if said.len() > 1 {
                d = d.child(div().id("fewer").cursor_pointer().text_size(px(12.)).text_color(rgb(look::FAINT)).on_click(cx.listener(|this, _, _, cx| {
                    this.all_said = false;
                    cx.notify();
                })).child("▾ fewer"));
            }
            return d.into_any_element();
        }
        div()
            .flex()
            .gap(px(10.))
            .items_center()
            .child(div().min_w_0().truncate().text_size(px(12.5)).text_color(rgb(look::DIM)).child(said[0].clone()))
            .child(
                div()
                    .id("more-said")
                    .flex_none()
                    .cursor_pointer()
                    .text_size(px(12.))
                    .text_color(rgb(look::FAINT))
                    .hover(|d| d.text_color(rgb(look::TEXT)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.all_said = true;
                        cx.notify();
                    }))
                    .child(format!("▸ {} more of what this does", said.len() - 1)),
            )
            .into_any_element()
    }
}

impl Render for Compare {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(stop) = self.stop.clone().filter(|_| self.open) else { return div().into_any_element() };
        let Some(item) = stop.items.get(self.dir).cloned() else { return div().into_any_element() };
        let names: Vec<String> = stop.items.iter().map(|i| name(&i.label)).collect();
        let proposal = self.proposal(cx);
        let mut label = proposal.as_ref().map(|(l, _)| l.clone()).unwrap_or_default();
        if stop.ahead.is_some() {
            label.push_str(" · Touch ID");
        }
        let can = !self.sending && self.reckoned.refused.is_none() && proposal.is_some();
        // the stage: this direction's picture, at the size the window gives it
        let mut tabs = div().flex().gap(px(6.)).items_center();
        if item.pictures.len() > 1 {
            for (n, _) in item.pictures.iter().enumerate() {
                tabs = tabs.child(button(format!("tab-{n}"), &format!("Screen {}", n + 1), n == self.pic, look::TEXT).on_click(cx.listener(move |this, _, _, cx| {
                    this.pic = n;
                    cx.notify();
                })));
            }
        }
        tabs = tabs.child(div().flex_1()).child(div().text_size(px(14.)).text_color(rgb(look::TEXT)).child(format!("{} · {}", self.dir + 1, names[self.dir]))).child(div().text_size(px(12.5)).text_color(rgb(look::FAINT)).max_w(px(520.)).truncate().child(idea(&item.label)));
        let stage = match item.pictures.get(self.pic).or(item.pictures.first()) {
            Some(p) => div().flex_1().min_h_0().flex().items_center().justify_center().child(img(p.clone()).size_full().object_fit(ObjectFit::Contain).rounded(px(10.))),
            None => div().flex_1().flex().items_center().justify_center().text_color(rgb(look::FAINT)).child("This direction has no picture."),
        };
        // the directions as a strip of large pictures under the stage, each with its star
        let mut strip = div().flex().gap(px(12.)).items_end().flex_none();
        for (i, it) in stop.items.iter().enumerate() {
            let on = i == self.dir;
            let starred = self.keep.contains(&i);
            let pins = self.pins.iter().filter(|(d, _)| *d == i).count();
            strip = strip.child(
                div()
                    .id(SharedString::from(format!("dir-{i}")))
                    .w(px(208.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(6.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(rgb(if on { look::TEXT } else { look::LINE_2 }))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgb(look::PANEL_2)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.dir = i;
                        this.pic = 0;
                        this.reckon(cx);
                    }))
                    .child(match it.pictures.first() {
                        Some(p) => img(p.clone()).w_full().h(px(112.)).object_fit(ObjectFit::Cover).rounded(px(6.)).into_any_element(),
                        None => div().w_full().h(px(112.)).rounded(px(6.)).bg(rgb(look::PANEL_2)).into_any_element(),
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(12.5)).text_color(rgb(if on { look::TEXT } else { look::DIM })).child(if pins > 0 { format!("{} · {} · {pins} pinned", i + 1, names[i]) } else { format!("{} · {}", i + 1, names[i]) }))
                            .child(
                                div()
                                    .id(SharedString::from(format!("star-{i}")))
                                    .px(px(2.))
                                    .text_size(px(15.))
                                    .text_color(rgb(if starred { look::AMBER } else { look::FAINT }))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.star(i, cx);
                                    }))
                                    .child(if starred { "★" } else { "☆" }),
                            ),
                    ),
            );
        }
        strip = strip.child(div().pb(px(10.)).pl(px(4.)).text_size(px(12.)).text_color(rgb(look::FAINT)).child("← → flip · ☆ keep two to develop both"));
        let left = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(18.))
            .pt(px(14.))
            .pb(px(10.))
            .child(tabs)
            .child(stage)
            .child(strip);
        // the side: this direction's parts with their pins, and what was said of it
        let mut parts = div().flex().flex_col().gap(px(8.));
        for (label, text) in &item.aspects {
            let on = self.pins.contains(&(self.dir, label.clone()));
            let (d, l) = (self.dir, label.clone());
            parts = parts.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .p(px(8.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(if on { look::AMBER } else { look::LINE }))
                    .child(div().flex().items_center().child(div().flex_1().text_size(px(12.5)).text_color(rgb(look::TEXT)).child(label.clone())).child(button(format!("pin-{label}"), if on { "Stays" } else { "Pin" }, on, look::AMBER).on_click(cx.listener(move |this, _, _, cx| this.pin(d, l.clone(), cx)))))
                    .child(div().text_size(px(12.)).text_color(rgb(look::DIM)).child(text.clone())),
            );
        }
        let mut back = div().flex().flex_col().gap(px(6.));
        for b in &stop.back {
            let on = self.bring.as_deref() == Some(b.id.as_str());
            let id = b.id.clone();
            back = back.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .children(b.pictures.first().map(|p| img(p.clone()).w(px(48.)).h(px(30.)).object_fit(ObjectFit::Cover).rounded(px(4.))))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.5)).text_color(rgb(look::DIM)).child(name(&b.label)))
                    .child(button(format!("bring-{}", b.id), if on { "Comes back" } else { "Bring back" }, on, look::AMBER).on_click(cx.listener(move |this, _, _, cx| {
                        this.bring = if this.bring.as_deref() == Some(id.as_str()) { None } else { Some(id.clone()) };
                        this.reckon(cx);
                    }))),
            );
        }
        let page = item.page.clone();
        let side = div()
            .id("compare-side")
            .w(px(300.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(rgb(look::LINE))
            .bg(rgb(look::PANEL))
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .when(!item.aspects.is_empty(), |d| d.child(div().flex().flex_col().gap(px(6.)).child(look::label("Its parts", look::FAINT)).child(parts)))
            .when(!stop.back.is_empty(), |d| d.child(div().flex().flex_col().gap(px(6.)).child(look::label("You dropped earlier · only you bring it back", look::FAINT)).child(back)))
            .when(!item.detail.is_empty(), |d| d.child(div().flex().flex_col().gap(px(6.)).child(look::label("What was said of it", look::FAINT)).child(div().text_size(px(12.5)).text_color(rgb(look::DIM)).child(item.detail.clone()))))
            .when_some(page, |d, page| d.child(button("live".into(), "Open the live version ↗", false, look::BLUE).on_click(cx.listener(move |this, _, _, cx| this.open_live(page.clone(), cx)))));
        // the bar: none, the person's words, what the answer does, and the button that sends it
        let words = self.words.clone();
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
                        .child(look::label("What Jaynes read · this is what it does when you send", look::BLUE))
                        .child(sentences(&u.sentences))
                        .when(!u.unsure.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(rgb(look::AMBER)).child(format!("Not sure: {}", u.unsure))))
                        .when_some(u.refused.clone(), |d, r| d.child(div().text_size(px(12.)).text_color(rgb(look::RED)).child(r)))
                        .child(div().flex().gap(px(8.)).child(primary("send-read", "Send this".into(), ok).when(ok, |d| d.on_click(cx.listener(|this, _, _, cx| this.send_understood(cx))))).child(button("by-hand".into(), "Answer with the buttons", false, look::GREY).on_click(cx.listener(|this, _, _, cx| {
                            this.understood = None;
                            cx.notify();
                        })))),
                )
            }
            _ => None,
        };
        let none_part = if self.none {
            let why = self.why.clone();
            div()
                .flex()
                .gap(px(8.))
                .items_center()
                .children(why.map(|w| div().w(px(340.)).child(field(&w))))
                .child(button("none-cancel".into(), "cancel", false, look::GREY).on_click(cx.listener(|this, _, _, cx| {
                    this.none = false;
                    this.reckon(cx);
                })))
        } else {
            div().child(button("none".into(), "None of these", false, look::RED).on_click(cx.listener(|this, _, window, cx| {
                this.none = true;
                if this.why.is_none() {
                    this.why = Some(this.input("what was missing in all of them?", window, cx));
                }
                this.reckon(cx);
            })))
        };
        let bar = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(18.))
            .py(px(12.))
            .border_t_1()
            .border_color(rgb(look::LINE))
            .bg(rgb(look::PANEL))
            .child(div().flex().gap(px(8.)).items_center().child(none_part).child(div().flex_1().children(words.map(|w| field(&w)))).child(button("read".into(), "Read my words", false, look::BLUE).on_click(cx.listener(|this, _, _, cx| this.read_words(cx)))))
            .children(understood)
            .child(
                div()
                    .flex()
                    .gap(px(12.))
                    .items_end()
                    .child(div().flex_1().min_w_0().child(self.consequences(cx)))
                    .when_some(self.reckoned.refused.clone(), |d, r| d.child(div().max_w(px(360.)).text_size(px(12.)).text_color(rgb(look::RED)).child(r)))
                    .when_some(self.said.clone(), |d, s| d.child(div().text_size(px(12.)).text_color(rgb(look::FAINT)).child(s)))
                    .child(primary("go", if self.sending { "Sending…".into() } else { format!("{label} ↵") }, can).when(can, |d| d.on_click(cx.listener(|this, _, _, cx| this.go(cx))))),
            );
        let top = div()
            .flex()
            .gap(px(14.))
            .items_center()
            .px(px(18.))
            .py(px(12.))
            .border_b_1()
            .border_color(rgb(look::LINE))
            .bg(rgb(look::PANEL))
            .pl(px(84.))
            .child(div().size(px(8.)).rounded_full().bg(rgb(look::AMBER)))
            .child(div().flex_1().min_w_0().flex().flex_col().child(div().truncate().text_size(px(16.)).text_color(rgb(look::TEXT)).child(stop.title.clone())).when(!stop.about.is_empty(), |d| d.child(div().truncate().text_size(px(12.5)).text_color(rgb(look::DIM)).child(format!("At {}: {}", stop.step, stop.about)))))
            .when(stop.round > 0, |d| d.child(div().text_size(px(12.)).text_color(rgb(look::FAINT)).child(format!("round {}", stop.round))))
            .child(button("close".into(), "Esc · back to the terminal", false, look::GREY).on_click(cx.listener(|this, _, _, cx| this.close(cx))));
        let _ = window;
        div()
            .id("compare")
            .track_focus(&self.focus)
            .absolute()
            .inset_0()
            .bg(rgb(BG))
            .flex()
            .flex_col()
            .font_family(look::UI_FONT)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                if this.some_field_focused(window, cx) {
                    return;
                }
                match e.keystroke.key.as_str() {
                    "left" => this.flip(-1, cx),
                    "right" => this.flip(1, cx),
                    "escape" => this.close(cx),
                    "enter" => this.go(cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(top)
            .child(div().flex_1().min_h_0().flex().child(left).child(side))
            .child(bar)
            .into_any_element()
    }
}
