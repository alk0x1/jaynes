//! The compare mode, over the whole window, for a stop that chooses among designs. Each direction at full size, flipped
//! in place with the arrow keys; its parts beside, each with a pin that says "this stays"; up to two directions starred
//! to keep as finalists; none, with what was missing; the person's own words; and one button that says what it does:
//! choose one, mix the parts pinned in two, develop the two starred, or redo them all. What the answer will do is the
//! engine's sentences, and it goes signed with the person's key like every answer from the app. A click on the picture
//! opens it large with zoom (viewer.rs); a stop with one option opens that option, with nothing to compare. Esc closes
//! one layer at a time: the picture shown large, the list of what the answer does, the field with the keys, the mode.

use crate::answer::{self, Reckoned, Understood};
use crate::island::{button, focus_field, primary, sentences, tabbed};
use crate::key::Key;
use crate::look;
use crate::viewer::{self, Does, Sizes, View};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_base::input::{InputEvent, InputState};
use jaynes_shell::stop::Stop;
use serde_json::Value;
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The note a shortlisted direction carries to the agent, with what of it stays, from the parts' labels as the stop
/// has them: "… Its palette stays.", "… Its palette and type stay."
pub fn shortlist_note(stays: &[String]) -> String {
    let base = jaynes_core::decide::SHORTLISTED;
    match stays {
        [] => base.to_string(),
        [_] => format!("{base} Its {} stays.", parts_phrase(stays)),
        _ => format!("{base} Its {} stay.", parts_phrase(stays)),
    }
}

/// A part of a direction as a sentence names it: its label with a small first letter ("Palette" is "palette"), unless
/// the label is written in capitals ("CTA").
pub fn part_word(label: &str) -> String {
    let mut letters = label.chars();
    match (letters.next(), letters.next()) {
        (Some(first), second) if !second.is_some_and(char::is_uppercase) => first.to_lowercase().chain(label.chars().skip(1)).collect(),
        _ => label.to_string(),
    }
}

/// Parts named in a sentence: "palette", "palette and type", "palette, type and layout".
pub fn parts_phrase(labels: &[String]) -> String {
    let words: Vec<String> = labels.iter().map(|l| part_word(l)).collect();
    match words.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The note of a mix, in the shortlist's words for its parts: the parts of the base direction that stay, and each part
/// another direction lends it, as (part, its direction, what it is).
pub fn mix_note(base: &str, kept: &[String], lent: &[(String, String, String)]) -> String {
    let mut note = format!("Mix: keep the {} of {base}.", parts_phrase(kept));
    for (label, name, text) in lent {
        note.push_str(&format!(" Take the {} of {name}: {text}", part_word(label)));
    }
    note
}

/// How long a zoom made by a button or a click takes; a pinch or a wheel follows the fingers.
const ZOOM_TIME: Duration = Duration::from_millis(180);

/// The viewer's top and bottom lines, in points, so the box the picture gets is known before it is laid out.
const VIEW_HEAD: f32 = 46.;
const VIEW_FOOT: f32 = 34.;
/// Its margin in the window.
const VIEW_MARGIN: f32 = 28.;

/// What the person marked at the stop: the direction shown and its screen, the parts that stay (by direction and the
/// part's label), the directions starred to keep as finalists, none of them, and what they bring back of what they
/// dropped in an earlier round.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marks {
    pub dir: usize,
    pub pic: usize,
    pub pins: BTreeSet<(usize, String)>,
    pub keep: Vec<usize>,
    pub none: bool,
    pub bring: Option<String>,
    /// The person chose something here (another option, a shortlist, a part kept, none, what comes back): only then
    /// does Enter send.
    pub touched: bool,
}

/// What a press on a direction's star did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Starred {
    On,
    Off,
    /// Two are shortlisted already: the third is refused, never one taken off silently.
    Full,
}

/// What Enter does in the mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enter {
    /// The main button.
    Go,
    /// Nothing chosen yet: the main button pulses once and nothing goes.
    Pulse,
}

/// The title over a direction's parts.
pub const PARTS: &str = "Its parts · keep one to carry it on; parts from two make a mix";
/// The hint beside the strip of directions.
pub const HINT: &str = "← → flip · shortlist two to develop both";
/// The hint, for a while, after a third shortlist was refused.
pub const FULL: &str = "Two at most: unshortlist one first";
/// The hint in the person's words field.
pub const WORDS_HINT: &str = "Your words: what stays, what changes, in any language";
/// The hint in the words field when there is one option only: words there ask or change before anything is chosen.
pub const ONE_HINT: &str = "Not sure yet? Ask the agent, or say what to change";
/// With one option: the main button while the field is empty, and the button beside "Not yet" once it has words.
pub const GO_ONE: &str = "Go with this one";
/// With one option and the person's words: nothing is chosen, and the words go to the agent as a question or a change.
pub const NOT_YET: &str = "Not yet: send my words";
/// How long that hint shows.
pub const FULL_FOR: Duration = Duration::from_secs(3);

/// With words that ask: the main button, and the line under the field.
pub const ASK_ONE: &str = "Ask the agent ↵";
pub const ASK_FOOT: &str = "The agent answers here. Nothing is redone and nothing is chosen.";
/// In the conversation while the agent answers a question.
pub const ANSWERING: &str = "The agent is answering. Nothing is redone.";
/// The two ways the person's words go, in the field once it has words: a question, or a change.
pub const QUESTION: &str = "Question";
pub const CHANGE: &str = "Change";

/// The line under the field while the agent answers.
pub fn asked_foot(step: &str) -> String {
    format!("Your question went to the agent at “{step}”. It answers here; nothing is redone and nothing is chosen.")
}

/// The line under the field when the words ask for a change of the one option: it is not chosen.
pub fn change_foot(name: &str, step: &str) -> String {
    format!("“{name}” is not chosen yet: your words go to the agent at “{step}”, and it asks you again.")
}

/// Whether words read as a question, guessed without a model: they end in "?" or open as one does ("e se", "what
/// if", "why", "como"…). A click on Question or Change overrides it.
pub fn looks_like_question(words: &str) -> bool {
    let w = words.trim().to_lowercase();
    if w.is_empty() {
        return false;
    }
    const OPENS: [&str; 16] =
        ["e se ", "what if ", "por que", "porque ", "why ", "how ", "como ", "what about ", "and if ", "o que ", "qual ", "quais ", "which ", "what ", "is it ", "does "];
    w.ends_with('?') || OPENS.iter().any(|o| w.starts_with(o))
}

/// One entry of a stop's conversation as the column beside the option shows it: what it was, its words, and whether
/// the agent said it.
#[derive(Clone, Debug, PartialEq)]
pub struct Said {
    pub round: u32,
    pub head: String,
    pub text: String,
    pub agent: bool,
}

/// The conversation of a stop's step (`StepState.thread`) about one option: the entries about it and those about the
/// whole, in order, each with what it was ("You · round 2 · a question", "Agent · answered · nothing redone").
pub fn conversation(thread: &[jaynes_core::run::Entry], item: Option<&str>) -> Vec<Said> {
    use jaynes_core::run::EntryKind;
    thread
        .iter()
        .filter(|e| e.item.is_none() || item.is_none() || e.item.as_deref() == item)
        .map(|e| {
            // the stop's round, as its head says it ("round 2"): the engine counts the step's from 0
            let round = e.round + 1;
            let (head, agent) = match e.kind {
                EntryKind::Change => (format!("You · round {round} · a change"), false),
                EntryKind::Ask => (format!("You · round {round} · a question"), false),
                EntryKind::Answer if e.late => ("Agent · answered late · it changed nothing".to_string(), true),
                EntryKind::Answer => ("Agent · answered · nothing redone".to_string(), true),
                EntryKind::AskFailed => ("No answer".to_string(), true),
                EntryKind::Reply => (format!("Agent · round {round} · changed the option"), true),
            };
            Said { round, head, text: e.text.clone(), agent }
        })
        .collect()
}

/// Whether a stop chooses with one option only: there is nothing to choose among, so it asks yes or not yet, by the
/// option's name ("Go with “Python + FastAPI”?"), and the role's question goes on the line under it.
pub fn one_option(stop: &Stop) -> bool {
    stop.choose && stop.items.len() == 1
}

/// The hint in the person's words field.
pub fn words_hint(one: bool) -> &'static str {
    if one { ONE_HINT } else { WORDS_HINT }
}

/// What the line under the question says after the step: the role's question with one option, whose title asks whether
/// to go with it; else what the step does.
pub fn under_about(stop: &Stop) -> &str {
    if one_option(stop) && !stop.asks.trim().is_empty() { &stop.asks } else { &stop.about }
}

/// A part's button: keep it, or kept.
pub fn keep_word(kept: bool) -> &'static str {
    if kept { "Kept ✓" } else { "Keep this part" }
}

/// A direction's star with its word.
pub fn star_word(on: bool) -> &'static str {
    if on { "★ Shortlisted" } else { "☆ Shortlist" }
}

/// What a direction's card says under its picture: its number, its name and how many of its parts are kept.
pub fn card_label(i: usize, name: &str, kept: usize) -> String {
    if kept > 0 { format!("{} · {name} · {kept} kept", i + 1) } else { format!("{} · {name}", i + 1) }
}

/// The line under the question: which run asks, at which step, and what the step does.
pub fn under_title(run: &str, step: &str, about: &str) -> Option<String> {
    match (run.is_empty(), about.is_empty()) {
        (false, false) => Some(format!("{run} · at “{step}”: {about}")),
        (false, true) => Some(format!("{run} · at “{step}”")),
        (true, false) => Some(format!("At “{step}”: {about}")),
        (true, true) => None,
    }
}

/// What one Esc closes: only the layer on top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Esc {
    /// The picture shown large.
    Viewer,
    /// The list of everything the answer does.
    More,
    /// The field with the keys: it lets them go and keeps its words.
    Field,
    /// The compare mode, back to the terminal, keeping what was marked and written for the same question.
    Screen,
}

/// What one Esc closes, from the top: the viewer, the list of what the answer does, the field, the mode.
pub fn esc(viewer: bool, more: bool, field: bool) -> Esc {
    if viewer {
        Esc::Viewer
    } else if more {
        Esc::More
    } else if field {
        Esc::Field
    } else {
        Esc::Screen
    }
}

/// The card that invites to the mode: its line, and its button. With one option the title already names it ("Go with
/// “Harbor”?"), so no count repeats it.
pub fn invite(n: usize, title: &str) -> (String, &'static str) {
    if n == 1 { (format!("◆ {title}"), "Open") } else { (format!("◆ {n} options · {title}"), "Compare") }
}

/// Whether the mode shows the strip of directions to flip among: not for a stop with one option.
pub fn strip(n: usize) -> bool {
    n > 1
}

/// The button that rejects every option, before it is pressed.
pub fn none_word(n: usize) -> &'static str {
    if n == 1 { "Not this one" } else { "None of these" }
}

/// The button at the top: what Esc does from where the keys are.
pub fn chip(field: bool) -> &'static str {
    if field { "Esc · leave the field" } else { "Esc · back" }
}

/// The line over the main button: the first sentence of what the answer does and, when there are more, the switch
/// that shows them all, in the same place whether they show or not.
pub fn said_row(said: &[String], all: bool) -> Option<(String, Option<String>)> {
    let first = said.first()?.clone();
    let toggle = match (said.len(), all) {
        (1, _) => None,
        (_, true) => Some("▾ Hide what this does".to_string()),
        (n, false) => Some(format!("▸ {} more of what this does", n - 1)),
    };
    Some((first, toggle))
}

/// Everything the answer does, over the bar, while the person asks to see it.
pub fn said_panel(said: &[String], all: bool) -> Option<Vec<String>> {
    (all && said.len() > 1).then(|| said.to_vec())
}

/// Whether `b` is the stop `a` was: what the person marked and wrote there stays only for it. A question's id is its
/// step's in every run of a role, so two runs that wait at the same step ask questions of the same id; and a new round
/// of the same question shows other options, with a fingerprint of its own (`shown`).
pub fn same_stop(a: &Stop, b: &Stop) -> bool {
    a.run == b.run && a.question == b.question && a.shown == b.shown
}

/// A direction's short name: its label before the colon.
fn name(label: &str) -> String {
    label.split_once(':').map(|(n, _)| n).unwrap_or(label).trim().to_string()
}

/// Its idea: the label after the colon.
fn idea(label: &str) -> String {
    label.split_once(':').map(|(_, i)| i.trim().to_string()).unwrap_or_default()
}

impl Marks {
    /// The next or previous direction, round the strip of `n`; with one option there is nowhere to flip.
    pub fn flip(&mut self, by: isize, n: usize) {
        if n < 2 {
            return;
        }
        self.dir = (self.dir as isize + by).rem_euclid(n as isize) as usize;
        self.pic = 0;
        self.touched = true;
    }

    /// A click on a direction's card: it shows, and it is what the person chose to look at.
    pub fn pick(&mut self, d: usize) {
        self.dir = d;
        self.pic = 0;
        self.touched = true;
    }

    /// Shortlists `d`, or takes it off the shortlist; two at most, so a third is refused and nothing is taken off.
    pub fn star(&mut self, d: usize) -> Starred {
        if let Some(i) = self.keep.iter().position(|k| *k == d) {
            self.keep.remove(i);
            self.touched = true;
            return Starred::Off;
        }
        if self.keep.len() == 2 {
            return Starred::Full;
        }
        self.keep.push(d);
        self.touched = true;
        Starred::On
    }

    /// Keeps a part of direction `d`, or lets it go.
    pub fn pin(&mut self, d: usize, label: String) {
        if !self.pins.remove(&(d, label.clone())) {
            self.pins.insert((d, label));
        }
        self.touched = true;
    }

    /// What Enter does: the main button once the person chose something here; before, it only pulses, so an Enter
    /// as the mode opens never chooses the first option for them.
    pub fn enter(&self) -> Enter {
        if self.touched { Enter::Go } else { Enter::Pulse }
    }

    /// The marks within a stop of `n` options: nothing that points past its options, from marks kept before.
    pub fn fit(&mut self, n: usize) {
        if self.dir >= n {
            self.dir = 0;
            self.pic = 0;
        }
        self.pins.retain(|(d, _)| *d < n);
        self.keep.retain(|d| *d < n);
    }

    /// The direction the main button chooses when it chooses one alone: the one with parts pinned, else the one shown.
    /// None when it does something else (a mix, two finalists, none, or what comes back).
    pub fn chosen(&self, stop: &Stop) -> Option<usize> {
        let dirs: BTreeSet<usize> = self.pins.iter().map(|(d, _)| *d).collect();
        if self.bring.is_some() || self.none || self.keep.len() == 2 || dirs.len() >= 2 || stop.items.is_empty() {
            return None;
        }
        Some(dirs.iter().next().copied().unwrap_or(self.dir).min(stop.items.len() - 1))
    }

    /// What the main button does, in words, and the answer it sends, with the person's words and, for none, what they
    /// said was missing.
    pub fn proposal(&self, stop: &Stop, words: &str, why: &str) -> Option<(String, Value)> {
        let names: Vec<String> = stop.items.iter().map(|i| name(&i.label)).collect();
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
            let context = [why.trim(), words.trim()].iter().filter(|t| !t.is_empty()).copied().collect::<Vec<_>>().join(" ");
            let label = if stop.items.len() == 1 { "Not this one: make a new one".to_string() } else { format!("None of these: make {} new ones", stop.items.len()) };
            return Some((label, answer::body(stop, &verdicts, &notes, &BTreeMap::new(), &context)));
        }
        if self.keep.len() == 2 {
            for &k in &self.keep {
                let item = &stop.items[k];
                verdicts.insert(item.id.clone(), "change".into());
                let stays = pinned(k);
                notes.insert(item.id.clone(), shortlist_note(&stays));
            }
            let label = format!("Develop {} and {}", names[self.keep[0]], names[self.keep[1]]);
            return Some((label, answer::body(stop, &verdicts, &notes, &BTreeMap::new(), words)));
        }
        let dirs: BTreeSet<usize> = self.pins.iter().map(|(d, _)| *d).collect();
        if dirs.len() >= 2 {
            // the direction with the most pins is the base; the others lend it the parts pinned in them
            let base = *dirs.iter().max_by_key(|d| (pinned(**d).len(), **d == self.dir)).unwrap_or(&self.dir);
            let item = &stop.items[base];
            let mut parts = vec![];
            let mut lent = vec![];
            for &d in dirs.iter().filter(|d| **d != base) {
                for label in pinned(d) {
                    let text = stop.items[d].aspects.iter().find(|(l, _)| *l == label).map(|(_, t)| t.clone()).unwrap_or_default();
                    parts.push(format!("{label} of {}", names[d]));
                    lent.push((label, names[d].clone(), text));
                }
            }
            let note = mix_note(&names[base], &pinned(base), &lent);
            verdicts.insert(item.id.clone(), "change".into());
            notes.insert(item.id.clone(), note);
            let label = format!("Mix: {} + {}", names[base], parts.join(" + "));
            return Some((label, answer::body(stop, &verdicts, &notes, &BTreeMap::new(), words)));
        }
        let target = self.chosen(stop)?;
        let one = one_option(stop);
        if one && !words.trim().is_empty() {
            // one option and words: "not yet", a change with the words on it, which chooses nothing (the engine says
            // "is not chosen yet"); going with it, the words beside, is the button next to this one (also)
            verdicts.insert(stop.items[target].id.clone(), "change".into());
            notes.insert(stop.items[target].id.clone(), words.trim().to_string());
            return Some((NOT_YET.to_string(), answer::body(stop, &verdicts, &notes, &BTreeMap::new(), "")));
        }
        verdicts.insert(stop.items[target].id.clone(), "keep".into());
        // the option's name is on its card, ringed while the pointer is on the button: a name in the button read as
        // something else ("Go on with Go"); with one option there is nothing to choose among
        let label = if one {
            GO_ONE
        } else if target == self.dir {
            "Choose this one"
        } else {
            "Choose the one with kept parts"
        };
        Some((label.to_string(), answer::body(stop, &verdicts, &notes, &BTreeMap::new(), words)))
    }

    /// The button beside the main one, when there is one: with one option and the person's words, going with it
    /// anyway, the words sent beside as they are today.
    pub fn also(&self, stop: &Stop, words: &str) -> Option<(String, Value)> {
        if !one_option(stop) || words.trim().is_empty() || self.none || self.bring.is_some() {
            return None;
        }
        let verdicts: BTreeMap<String, String> = [(stop.items[0].id.clone(), "keep".to_string())].into();
        Some((GO_ONE.to_string(), answer::body(stop, &verdicts, &BTreeMap::new(), &BTreeMap::new(), words)))
    }
}

/// The picture of a direction shown large: how it is shown, the press the person holds on it, a zoom a button or a
/// click is making, and the box it is shown in as the last frame laid it out.
struct Viewer {
    view: View,
    /// Untouched since it opened at Fit: a box that changes size fits it again.
    fresh: bool,
    /// The sizes the view was made for.
    made_for: Option<Sizes>,
    /// A press on the picture: where it began, the view then, and whether it has moved, so a release is a click.
    drag: Option<((f32, f32), View, bool)>,
    anim: Option<(View, View, Instant)>,
    body: Rc<Cell<Option<Bounds<Pixels>>>>,
}

pub struct Compare {
    /// The record as the app reads it: the stop's conversation comes from it, and is drawn again as it changes.
    runs: Entity<crate::model::Runs>,
    /// The person's click on Question or Change in the field; None goes by the guess (`looks_like_question`).
    ask_kind: Option<bool>,
    /// A question went: the field empties as it is drawn next.
    clear_words: bool,
    /// Whether every sentence of what the answer does shows, over the bar, or only the first.
    all_said: bool,
    key: Option<Arc<Key>>,
    stop: Option<Stop>,
    open: bool,
    marks: Marks,
    why: Option<Entity<InputState>>,
    words: Option<Entity<InputState>>,
    reckoned: Reckoned,
    reading: bool,
    understood: Option<Result<Understood, String>>,
    sending: bool,
    said: Option<String>,
    focus: FocusHandle,
    viewer: Option<Viewer>,
    /// The pictures' own sizes, read once from their files.
    sizes: HashMap<PathBuf, (f32, f32)>,
    /// Why the live version did not open, beside its button; and whether it is opening.
    live_said: Option<String>,
    live_opening: bool,
    /// The pointer on the main button: the card it chooses shows a ring.
    go_hover: bool,
    /// What the person marked and wrote at the other stops they opened, by run, question and what it showed.
    kept: HashMap<(String, String, String), Kept>,
    /// The name of the run whose stop shows, as the column says it.
    name: String,
    /// How many times Enter pulsed the main button with nothing chosen yet: each a pulse of its own.
    pulse: usize,
    /// When a third shortlist was refused: the hint says why for a while.
    full_at: Option<Instant>,
}

/// The answer the mode sent, for the run's tab to say so after the mode closes.
pub struct SentFrom(pub String);

impl EventEmitter<SentFrom> for Compare {}

fn kept_key(s: &Stop) -> (String, String, String) {
    (s.run.clone(), s.question.clone(), s.shown.clone())
}

/// A stop's marks and words while another stop shows.
#[derive(Default)]
struct Kept {
    marks: Marks,
    words: String,
    why: Option<String>,
}

impl Compare {
    pub fn new(key: Option<Arc<Key>>, runs: Entity<crate::model::Runs>, cx: &mut Context<Self>) -> Self {
        cx.observe(&runs, |_, _, cx| cx.notify()).detach();
        Compare {
            runs,
            ask_kind: None,
            clear_words: false,
            all_said: false,
            key,
            stop: None,
            open: false,
            marks: Marks::default(),
            why: None,
            words: None,
            reckoned: Reckoned::default(),
            reading: false,
            understood: None,
            sending: false,
            said: None,
            focus: cx.focus_handle(),
            viewer: None,
            sizes: HashMap::new(),
            live_said: None,
            live_opening: false,
            go_hover: false,
            kept: HashMap::new(),
            name: String::new(),
            pulse: 0,
            full_at: None,
        }
    }

    /// The stop the compare mode shows, while it is open: its run and question.
    pub fn stop_shown(&self) -> Option<(String, String)> {
        self.stop.as_ref().filter(|_| self.is_open()).map(|s| (s.run.clone(), s.question.clone()))
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens on a stop of the run named `name`: what the person marked and wrote there comes back as they left it,
    /// and what they marked at the stop shown before stays with that one, for when they come back to it.
    pub fn open(&mut self, stop: Stop, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.stop.as_ref().is_some_and(|s| same_stop(s, &stop)) {
            if let Some(was) = self.stop.as_ref() {
                let left = Kept { marks: std::mem::take(&mut self.marks), words: Self::text(self.words.as_ref(), cx), why: self.why.as_ref().map(|w| w.read(cx).value().to_string()) };
                self.kept.insert(kept_key(was), left);
            }
            let back = self.kept.remove(&kept_key(&stop)).unwrap_or_default();
            self.marks = back.marks;
            self.marks.fit(stop.items.len());
            self.full_at = None;
            self.words = None;
            self.why = back.why.map(|text| {
                let why = self.input("what was missing in all of them?", window, cx);
                why.update(cx, |s, cx| s.set_value(text, window, cx));
                why
            });
            if !back.words.is_empty() {
                let words = self.input(words_hint(one_option(&stop)), window, cx);
                words.update(cx, |s, cx| s.set_value(back.words, window, cx));
                self.words = Some(words);
            }
            self.understood = None;
            self.said = None;
            self.live_said = None;
            self.all_said = false;
        }
        self.stop = Some(stop);
        self.name = name;
        self.open = true;
        self.viewer = None;
        if self.words.is_none() {
            let hint = words_hint(self.stop.as_ref().is_some_and(one_option));
            self.words = Some(self.input(hint, window, cx));
        }
        window.focus(&self.focus, cx);
        self.reckon(cx);
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.viewer = None;
        self.go_hover = false;
        cx.notify();
    }

    fn input(&self, placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        cx.subscribe(&input, move |this, field, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                this.reckon(cx);
            }
            // Enter in the person's words reads them, as "Read my words" does
            if crate::island::reads_on(e) && this.words.as_ref() == Some(&field) {
                this.read_words(cx);
            }
            // the field took or let go the keys: the ring and the chip at the top follow
            cx.notify();
        })
        .detach();
        input
    }

    fn text(input: Option<&Entity<InputState>>, cx: &App) -> String {
        input.map(|i| i.read(cx).value().to_string()).unwrap_or_default()
    }

    fn many(&self) -> bool {
        self.stop.as_ref().is_some_and(|s| s.items.len() > 1)
    }

    fn flip(&mut self, by: isize, cx: &mut Context<Self>) {
        let n = self.stop.as_ref().map(|s| s.items.len()).unwrap_or(0);
        self.marks.flip(by, n);
        self.reckon(cx);
    }

    /// A press on a direction's star; a third is refused, and the hint says why for a while.
    fn star(&mut self, d: usize, cx: &mut Context<Self>) {
        if self.marks.star(d) == Starred::Full {
            self.full_at = Some(Instant::now());
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(FULL_FOR).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
        self.reckon(cx);
    }

    fn pin(&mut self, d: usize, label: String, cx: &mut Context<Self>) {
        self.marks.pin(d, label);
        self.reckon(cx);
    }

    /// Enter on the mode: the main button once the person chose something here, else one pulse of it.
    fn enter(&mut self, cx: &mut Context<Self>) {
        match self.marks.enter() {
            Enter::Go => self.go(cx),
            Enter::Pulse => {
                self.pulse += 1;
                cx.notify();
            }
        }
    }

    /// Whether the picture shows large, over the mode.
    pub fn viewing(&self) -> bool {
        self.viewer.is_some()
    }

    /// What the main button does, in words, and the answer it sends.
    fn proposal(&self, cx: &App) -> Option<(String, Value)> {
        let stop = self.stop.as_ref()?;
        self.marks.proposal(stop, &Self::text(self.words.as_ref(), cx), &Self::text(self.why.as_ref(), cx))
    }

    fn reckon(&mut self, cx: &mut Context<Self>) {
        self.reckoned = self.proposal(cx).map(|(_, body)| answer::reckon(&body)).unwrap_or_default();
        if self.reckoned.sentences.len() <= 1 {
            self.all_said = false;
        }
        cx.notify();
    }

    fn send_body(&mut self, body: Value, cx: &mut Context<Self>) {
        let Some(stop) = self.stop.clone() else { return };
        self.sending = true;
        self.said = None;
        cx.notify();
        let sent = answer::send(self.key.clone(), answer::touch_of(&stop).map(str::to_string), body);
        cx.spawn(async move |this, cx| {
            let sent = sent.await;
            let _ = this.update(cx, |this, cx| {
                this.sending = false;
                match sent {
                    Ok(()) => {
                        this.said = Some(crate::island::SENT.into());
                        this.open = false;
                        this.viewer = None;
                        // the run's tab says it for a while, where the person is back
                        cx.emit(SentFrom(stop.run.clone()));
                    }
                    Err(e) => this.said = Some(format!("Not sent: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The stop's conversation as the record has it now; empty while the record shows another question.
    fn thread(&self, cx: &App) -> crate::model::Thread {
        let Some(stop) = self.stop.as_ref() else { return Default::default() };
        let snap = &self.runs.read(cx).snap;
        let same = snap.stops.get(&stop.run).is_some_and(|s| s.question == stop.question);
        snap.threads.get(&stop.run).filter(|_| same).cloned().unwrap_or_default()
    }

    /// Whether the person's words go as a question to the agent: one option, words, and Question picked or guessed.
    fn asks(&self, stop: &Stop, cx: &App) -> bool {
        let words = Self::text(self.words.as_ref(), cx);
        one_option(stop) && !words.trim().is_empty() && self.ask_kind.unwrap_or_else(|| looks_like_question(&words))
    }

    /// Sends the words as a question about the option shown: the agent answers in the conversation beside it.
    fn ask(&mut self, cx: &mut Context<Self>) {
        let Some(stop) = self.stop.clone() else { return };
        let words = Self::text(self.words.as_ref(), cx);
        if self.sending || words.trim().is_empty() || self.thread(cx).asking {
            return;
        }
        let item = stop.items.get(self.marks.dir).map(|i| i.id.clone());
        self.sending = true;
        self.said = None;
        cx.notify();
        let asked = answer::ask(self.key.clone(), stop.run.clone(), stop.question.clone(), item, words.trim().to_string());
        cx.spawn(async move |this, cx| {
            let asked = asked.await;
            let _ = this.update(cx, |this, cx| {
                this.sending = false;
                match asked {
                    // the words are in the conversation now: the field is free for the next, emptied as it is drawn
                    Ok(()) => {
                        this.clear_words = true;
                        this.ask_kind = None;
                    }
                    Err(e) => this.said = Some(format!("Not asked: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn go(&mut self, cx: &mut Context<Self>) {
        if self.stop.as_ref().is_some_and(|s| self.asks(s, cx)) {
            return self.ask(cx);
        }
        if self.sending || self.reckoned.refused.is_some() {
            return;
        }
        if let Some((_, body)) = self.proposal(cx) {
            self.send_body(body, cx);
        }
    }

    /// The button beside the main one: with one option and words, going with it anyway.
    fn go_also(&mut self, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        let Some(stop) = self.stop.as_ref() else { return };
        if let Some((_, body)) = self.marks.also(stop, &Self::text(self.words.as_ref(), cx)) {
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
        let read = cx.background_spawn(async move { crate::signals::open(|| answer::understand(&stop, &words, pressed.as_deref())) });
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

    /// The live version of a direction: a page a role kept, opened only through the page, with its key, which the
    /// engine starts when it does not run (open_page.rs); why not, beside the button.
    fn open_live(&mut self, page: String, cx: &mut Context<Self>) {
        if self.live_opening {
            return;
        }
        self.live_opening = true;
        self.live_said = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let said = cx.background_executor().spawn(async move { crate::signals::open(|| crate::open_page::live(&page)) }).await;
            let _ = this.update(cx, |this, cx| {
                this.live_opening = false;
                this.live_said = said.err();
                cx.notify();
            });
        })
        .detach();
    }

    fn some_field_focused(&self, window: &Window, cx: &App) -> bool {
        [&self.words, &self.why].iter().filter_map(|f| f.as_ref()).any(|f| f.read(cx).focus_handle(cx).is_focused(window))
    }

    /// The field lets the keys go, and keeps its words: the mode has them again.
    fn leave_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn picture(&self) -> Option<PathBuf> {
        let item = self.stop.as_ref()?.items.get(self.marks.dir)?;
        item.pictures.get(self.marks.pic).or(item.pictures.first()).cloned()
    }

    /// A picture's own size, read from its file once.
    fn size_of(&mut self, path: &PathBuf) -> (f32, f32) {
        *self.sizes.entry(path.clone()).or_insert_with(|| image::image_dimensions(path).map(|(w, h)| (w as f32, h as f32)).unwrap_or((1600., 1000.)))
    }

    /// The box the viewer shows the picture in: as the last frame laid it out, else as the window will.
    fn view_sizes(&mut self, window: &Window) -> Option<Sizes> {
        let pic = self.picture()?;
        let pic = self.size_of(&pic);
        let body = match self.viewer.as_ref().and_then(|v| v.body.get()) {
            Some(b) => (f32::from(b.size.width), f32::from(b.size.height)),
            None => {
                let w = window.viewport_size();
                (f32::from(w.width) - 2. * VIEW_MARGIN - 2., f32::from(w.height) - 2. * VIEW_MARGIN - 2. - VIEW_HEAD - VIEW_FOOT)
            }
        };
        Some(Sizes { pic, body: (body.0.max(1.), body.1.max(1.)) })
    }

    fn open_viewer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picture().is_none() {
            return;
        }
        self.viewer = Some(Viewer { view: View { z: 1., x: 0., y: 0. }, fresh: true, made_for: None, drag: None, anim: None, body: Rc::new(Cell::new(None)) });
        // the keys go to the viewer: the field keeps its words and lets them go
        window.focus(&self.focus, cx);
        self.fit_viewer(window);
        cx.notify();
    }

    fn fit_viewer(&mut self, window: &Window) {
        let Some(s) = self.view_sizes(window) else { return };
        if let Some(v) = self.viewer.as_mut() {
            v.view = View::fitted(s);
            v.made_for = Some(s);
            v.fresh = true;
            v.anim = None;
        }
    }

    fn close_viewer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.viewer = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// The view moved to `to`, at once or over a short while.
    fn view_to(&mut self, to: View, animate: bool, cx: &mut Context<Self>) {
        if let Some(v) = self.viewer.as_mut() {
            let from = v.anim.map(|(a, b, t0)| View::between(a, b, ease(t0.elapsed().as_secs_f32() / ZOOM_TIME.as_secs_f32()))).unwrap_or(v.view);
            v.anim = animate.then(|| (from, to, Instant::now()));
            v.view = to;
            v.fresh = false;
        }
        cx.notify();
    }

    fn viewer_does(&mut self, does: Does, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.view_sizes(window) else { return };
        let Some(view) = self.viewer.as_ref().map(|v| v.view) else { return };
        match does {
            Does::Close => self.close_viewer(window, cx),
            Does::In => self.view_to(view.zoomed(view.z * viewer::STEP, s.center(), s), true, cx),
            Does::Out => self.view_to(view.zoomed(view.z / viewer::STEP, s.center(), s), true, cx),
            Does::Fit => self.view_to(View::fitted(s), true, cx),
            Does::Actual => self.view_to(view.zoomed(1., s.center(), s), true, cx),
            Does::Screen(by) => {
                let n = self.stop.as_ref().and_then(|st| st.items.get(self.marks.dir)).map(|i| i.pictures.len()).unwrap_or(0);
                if n > 1 {
                    self.marks.pic = (self.marks.pic as isize + by).rem_euclid(n as isize) as usize;
                    self.keep_view(window, cx);
                }
            }
            Does::Direction(by) => {
                if self.many() {
                    let pic = self.marks.pic;
                    self.flip(by, cx);
                    // the same screen of the next direction, if it has one, at the same zoom and place
                    let n = self.stop.as_ref().and_then(|st| st.items.get(self.marks.dir)).map(|i| i.pictures.len()).unwrap_or(0);
                    self.marks.pic = pic.min(n.saturating_sub(1));
                    self.keep_view(window, cx);
                }
            }
        }
    }

    /// Another picture in the viewer: the same zoom and place, or Fit if it was never touched.
    fn keep_view(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(s) = self.view_sizes(window) else { return };
        if let Some(v) = self.viewer.as_mut() {
            v.view = if v.fresh { View::fitted(s) } else { v.view.kept(s) };
            v.made_for = Some(s);
            v.anim = None;
            v.drag = None;
        }
        cx.notify();
    }

    /// A point of the window in the viewer's box.
    fn in_body(&self, at: Point<Pixels>) -> Option<(f32, f32)> {
        let b = self.viewer.as_ref()?.body.get()?;
        Some((f32::from(at.x - b.origin.x), f32::from(at.y - b.origin.y)))
    }

    fn press(&mut self, at: Point<Pixels>) {
        let Some(p) = self.in_body(at) else { return };
        if let Some(v) = self.viewer.as_mut() {
            v.drag = Some((p, v.view, false));
        }
    }

    fn drag_to(&mut self, at: Point<Pixels>, held: bool, window: &Window, cx: &mut Context<Self>) {
        let Some(p) = self.in_body(at) else { return };
        let Some(s) = self.view_sizes(window) else { return };
        let Some(v) = self.viewer.as_mut() else { return };
        let Some((from, start, moved)) = v.drag.as_mut() else { return };
        if !held {
            v.drag = None;
            return;
        }
        let by = (p.0 - from.0, p.1 - from.1);
        if *moved || by.0.abs() + by.1.abs() > 3. {
            *moved = true;
            v.view = start.moved(by, s);
            v.anim = None;
            v.fresh = false;
            cx.notify();
        }
    }

    fn release(&mut self, at: Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        let Some(p) = self.in_body(at) else { return };
        let Some(s) = self.view_sizes(window) else { return };
        let Some((_, _, moved)) = self.viewer.as_mut().and_then(|v| v.drag.take()) else { return };
        if !moved {
            // a click, not a drag: between Fit and actual size at the point clicked, as Quick Look does
            let view = self.viewer.as_ref().map(|v| v.view).expect("open");
            self.view_to(view.clicked(p, s), true, cx);
        }
    }

    fn wheel(&mut self, e: &ScrollWheelEvent, window: &Window, cx: &mut Context<Self>) {
        let Some(p) = self.in_body(e.position) else { return };
        let Some(s) = self.view_sizes(window) else { return };
        let Some(view) = self.viewer.as_ref().map(|v| v.view) else { return };
        let d = e.delta.pixel_delta(px(20.));
        let (dx, dy) = (f32::from(d.x), f32::from(d.y));
        if e.modifiers.platform || e.modifiers.control {
            self.view_to(view.zoomed(view.z * (dy / 200.).exp(), p, s), false, cx);
        } else {
            self.view_to(view.moved((dx, dy), s), false, cx);
        }
    }

    fn pinch(&mut self, e: &PinchEvent, window: &Window, cx: &mut Context<Self>) {
        let Some(p) = self.in_body(e.position) else { return };
        let Some(s) = self.view_sizes(window) else { return };
        let Some(view) = self.viewer.as_ref().map(|v| v.view) else { return };
        self.view_to(view.zoomed(view.z * (1. + e.delta), p, s), false, cx);
    }

    fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = e.keystroke.key.as_str();
        let cmd = e.keystroke.modifiers.platform;
        let field = self.some_field_focused(window, cx);
        if key == "escape" {
            match esc(self.viewer.is_some(), self.all_said, field) {
                Esc::Viewer => self.close_viewer(window, cx),
                Esc::More => {
                    self.all_said = false;
                    cx.notify();
                }
                Esc::Field => self.leave_field(window, cx),
                Esc::Screen => self.close(cx),
            }
            cx.stop_propagation();
            return;
        }
        if self.viewer.is_some() {
            if let Some(does) = viewer::key(key, cmd) {
                self.viewer_does(does, window, cx);
                cx.stop_propagation();
            }
            return;
        }
        // with the keys in a field, the arrows and Enter are the field's
        if field || cmd {
            return;
        }
        // a control Tab gave the keys to: Enter and Space are its own press
        let on_control = !self.focus.is_focused(window);
        match key {
            "left" => self.flip(-1, cx),
            "right" => self.flip(1, cx),
            "space" if !on_control => self.open_viewer(window, cx),
            "enter" if !on_control => self.enter(cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// A test's line: `next`, `prev`, `star`, `pin <part>`, `none <why>`, `bring <item>`, `go`, `enter`, `close`, `view`,
    /// `viewer in|out|fit|actual|screen <n>|dir <n>|close`, `more`, `focus`, `type <text>`, `hover-go`, `live`, and
    /// `key <keystroke>` through the window's keys.
    pub fn driven(&mut self, line: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
        match verb {
            "next" => self.flip(1, cx),
            "prev" => self.flip(-1, cx),
            "star" => self.star(self.marks.dir, cx),
            "pin" => self.pin(self.marks.dir, rest.to_string(), cx),
            "none" => {
                self.marks.none = true;
                self.marks.touched = true;
                let why = self.input("what was missing in all of them?", window, cx);
                let text = rest.to_string();
                why.update(cx, |s, cx| s.set_value(text, window, cx));
                self.why = Some(why);
                self.reckon(cx);
            }
            "bring" => {
                self.marks.bring = Some(rest.to_string());
                self.marks.touched = true;
                self.reckon(cx);
            }
            "go" => self.go(cx),
            "also" => self.go_also(cx),
            // a click on Question or Change in the field
            "ask-kind" => {
                self.ask_kind = Some(rest == "question");
                cx.notify();
            }
            // Enter on the mode, as the keyboard's reaches it with the keys on the mode
            "enter" => self.enter(cx),
            "close" => self.close(cx),
            "view" => self.open_viewer(window, cx),
            "viewer" => {
                let (what, n) = rest.split_once(' ').unwrap_or((rest, "1"));
                let n: isize = n.parse().unwrap_or(1);
                let does = match what {
                    "in" => Does::In,
                    "out" => Does::Out,
                    "fit" => Does::Fit,
                    "actual" => Does::Actual,
                    "screen" => Does::Screen(n),
                    "dir" => Does::Direction(n),
                    _ => Does::Close,
                };
                self.viewer_does(does, window, cx);
            }
            "more" => {
                self.all_said = !self.all_said;
                cx.notify();
            }
            // a press on the words' box, as the pointer gives it the keys
            "focus" => {
                if let Some(w) = self.words.clone() {
                    w.update(cx, |s, cx| {
                        s.set_cursor_position(gpui_base::input::Position::new(0, u32::MAX), window, cx);
                        s.focus(window, cx);
                    });
                }
            }
            // the person's words, as typing leaves them: set_value sends no Change, which each keystroke does, so what
            // the answer does is reckoned here as a keystroke has it reckoned
            "type" => {
                if let Some(w) = self.words.clone() {
                    let text = rest.to_string();
                    w.update(cx, |s, cx| s.set_value(text, window, cx));
                    self.reckon(cx);
                }
            }
            "hover-go" => {
                self.go_hover = !self.go_hover;
                cx.notify();
            }
            "live" => {
                if let Some(page) = self.stop.as_ref().and_then(|s| s.items.get(self.marks.dir)).and_then(|i| i.page.clone()) {
                    self.open_live(page, cx);
                }
            }
            "key" => match Keystroke::parse(rest) {
                Ok(k) => window.defer(cx, move |window, cx| {
                    window.dispatch_keystroke(k, cx);
                }),
                Err(e) => tracing::warn!("no key {rest}: {e}"),
            },
            _ => {}
        }
    }
}

/// A zoom's ease: quick at first, slow at the end.
fn ease(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    1. - (1. - t).powi(3)
}

/// A quiet button that says what it does in the text's colour, with its icon: the two over the stage.
fn strong(id: &'static str, icon: look::Icon, text: &str) -> Stateful<Div> {
    tabbed(div().id(id))
        .flex()
        .items_center()
        .gap(px(6.))
        .flex_none()
        .px(px(10.))
        .py(px(4.))
        .rounded(px(7.))
        .border_1()
        .border_color(rgb(look::line_2()))
        .text_size(px(12.5))
        .text_color(rgb(look::text()))
        .cursor_pointer()
        .bg(rgb(look::panel()))
        .hover(|d| d.bg(rgb(look::selected())))
        .child(look::icon(icon, 13., look::text()))
        .child(text.to_string())
}

/// A ring round a card, as wide as `w`, in the blue that says what the main button chooses.
fn ring(w: f32) -> Vec<BoxShadow> {
    vec![BoxShadow { color: rgb(look::blue()).into(), offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(w), inset: false }]
}

impl Compare {
    /// The line over the main button: the first sentence of what the answer does, and its switch beside it, which
    /// stays where it is when the list opens over the bar.
    fn consequences(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some((first, toggle)) = said_row(&self.reckoned.sentences, self.all_said) else { return div().into_any_element() };
        div()
            .flex()
            .gap(px(10.))
            .items_center()
            .child(div().min_w_0().truncate().text_size(px(12.5)).text_color(rgb(look::dim())).child(first))
            .when_some(toggle, |d, t| {
                d.child(
                    tabbed(div().id("more-said"))
                        .flex_none()
                        .px(px(4.))
                        .rounded(px(5.))
                        .bg(rgb(look::panel()))
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(rgb(if self.all_said { look::text() } else { look::faint() }))
                        .hover(|d| d.text_color(rgb(look::text())))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.all_said = !this.all_said;
                            cx.notify();
                        }))
                        .child(t),
                )
            })
            .into_any_element()
    }

    /// The picture large, with its zoom, over the mode: the box of lightbox.rs with its own top and bottom lines.
    fn viewer_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let stop = self.stop.clone()?;
        let item = stop.items.get(self.marks.dir)?.clone();
        let path = self.picture()?;
        let s = self.view_sizes(window)?;
        let v = self.viewer.as_mut()?;
        // a box that changed size: Fit again if untouched, else the same zoom kept within it
        if v.made_for != Some(s) {
            v.view = if v.fresh { View::fitted(s) } else { v.view.kept(s) };
            v.made_for = Some(s);
        }
        let mut shown = v.view;
        if let Some((a, b, t0)) = v.anim {
            let t = t0.elapsed().as_secs_f32() / ZOOM_TIME.as_secs_f32();
            if t >= 1. {
                v.anim = None;
            } else {
                shown = View::between(a, b, ease(t));
                window.request_animation_frame();
            }
        }
        let zoomed = v.view.zoomed_in(s);
        let dragging = v.drag.is_some_and(|(_, _, moved)| moved);
        let body_cell = v.body.clone();
        let many = stop.items.len() > 1;
        let mut tabs = div().flex().gap(px(6.)).items_center();
        if item.pictures.len() > 1 {
            for (n, _) in item.pictures.iter().enumerate() {
                tabs = tabs.child(button(format!("vtab-{n}"), &format!("Screen {}", n + 1), n == self.marks.pic, look::text()).on_click(cx.listener(move |this, _, window, cx| {
                    this.marks.pic = n;
                    this.keep_view(window, cx);
                })));
            }
        }
        let zoom_button = |id: &'static str, text: &str, does: Does, cx: &mut Context<Self>| button(id.into(), text, false, look::text()).on_click(cx.listener(move |this, _, window, cx| this.viewer_does(does, window, cx)));
        let head = div()
            .h(px(VIEW_HEAD))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(14.))
            .border_b_1()
            .border_color(rgb(look::line()))
            .text_size(px(13.))
            .text_color(rgb(look::text()))
            .child(div().flex_none().child(format!("{} · {}", self.marks.dir + 1, name(&item.label))))
            .child(tabs)
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(zoom_button("zoom-out", "−", Does::Out, cx))
                    .child(div().w(px(48.)).flex().justify_center().text_size(px(12.5)).text_color(rgb(look::dim())).child(format!("{}%", shown.percent())))
                    .child(zoom_button("zoom-in", "+", Does::In, cx))
                    .child(zoom_button("zoom-fit", "Fit", Does::Fit, cx))
                    .child(zoom_button("zoom-actual", "Actual size", Does::Actual, cx)),
            )
            .when_some(item.page.clone(), |d, page| d.child(strong("view-live", look::Icon::Open, "Open live version").on_click(cx.listener(move |this, _, _, cx| this.open_live(page.clone(), cx)))))
            .child(button("view-close".into(), "Esc · close", false, look::grey()).on_click(cx.listener(|this, _, window, cx| this.close_viewer(window, cx))));
        let mut hint = String::from("Pinch or ⌘ scroll to zoom · drag to move · click for actual size and back");
        if item.pictures.len() > 1 {
            hint.push_str(" · ↑ ↓ its screens");
        }
        if many {
            hint.push_str(" · ← → the other directions, same place");
        }
        hint.push_str(" · Esc close");
        let (w, h) = (s.pic.0 * shown.z, s.pic.1 * shown.z);
        let this = cx.entity().downgrade();
        let body = div()
            .id("view-body")
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .cursor(if dragging { CursorStyle::ClosedHand } else if zoomed { CursorStyle::OpenHand } else { CursorStyle::PointingHand })
            .child(img(path).absolute().left(px(shown.x)).top(px(shown.y)).w(px(w)).h(px(h)))
            // the box as laid out, for the pointer's place in it; and the pointer's moves and release anywhere while a
            // press drags the picture
            .child(
                canvas(
                    move |b, _, _| {
                        let changed = body_cell.get() != Some(b);
                        body_cell.set(Some(b));
                        changed
                    },
                    move |_, changed, window, _| {
                        if changed {
                            window.refresh();
                        }
                        let moving = this.clone();
                        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = moving.update(cx, |c, cx| c.drag_to(e.position, e.pressed_button == Some(MouseButton::Left), window, cx));
                            }
                        });
                        let releasing = this.clone();
                        window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                            if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
                                let _ = releasing.update(cx, |c, cx| c.release(e.position, window, cx));
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.press(e.position);
            }))
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, window, cx| this.wheel(e, window, cx)))
            .on_pinch(cx.listener(|this, e: &PinchEvent, window, cx| this.pinch(e, window, cx)));
        let foot = div()
            .h(px(VIEW_FOOT))
            .flex_none()
            .flex()
            .items_center()
            .px(px(14.))
            .border_t_1()
            .border_color(rgb(look::line()))
            .text_size(px(12.))
            .text_color(rgb(look::faint()))
            .child(div().truncate().child(hint));
        Some(
            div()
                .id("viewer")
                .absolute()
                .inset_0()
                .bg(rgba(look::scrim()))
                .p(px(VIEW_MARGIN))
                .flex()
                // a press outside the box closes it; one on the picture never does
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_viewer(window, cx)))
                .child(
                    div()
                        .id("viewer-box")
                        .flex()
                        .flex_col()
                        .size_full()
                        .rounded(px(12.))
                        .bg(rgb(look::panel()))
                        .border_1()
                        .border_color(rgb(look::line_2()))
                        .overflow_hidden()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(head)
                        .child(body)
                        .child(foot),
                )
                .into_any_element(),
        )
    }
}

impl Render for Compare {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(stop) = self.stop.clone().filter(|_| self.open) else { return div().into_any_element() };
        let Some(item) = stop.items.get(self.marks.dir).cloned() else { return div().into_any_element() };
        if std::mem::take(&mut self.clear_words)
            && let Some(w) = self.words.clone()
        {
            w.update(cx, |s, cx| s.set_value("", window, cx));
        }
        let thread = self.thread(cx);
        let asks = self.asks(&stop, cx);
        let n = stop.items.len();
        let names: Vec<String> = stop.items.iter().map(|i| name(&i.label)).collect();
        let proposal = self.proposal(cx);
        let chosen = self.marks.chosen(&stop);
        let mut label = proposal.as_ref().map(|(l, _)| l.clone()).unwrap_or_default();
        let mut also = self.marks.also(&stop, &Self::text(self.words.as_ref(), cx)).map(|(l, _)| l);
        if asks {
            // a question asks no touch: it chooses nothing
            label = ASK_ONE.trim_end_matches(" ↵").to_string();
        } else if answer::touch_of(&stop).is_some() {
            label.push_str(" · Touch ID");
            if let Some(a) = also.as_mut() {
                a.push_str(" · Touch ID");
            }
        }
        let can = if asks { !self.sending && !thread.asking } else { !self.sending && self.reckoned.refused.is_none() && proposal.is_some() };
        let field = self.some_field_focused(window, cx);
        // the line over the stage: the screens, the direction's name and idea, and the two ways to see it larger
        let mut head = div().flex().gap(px(6.)).items_center();
        if item.pictures.len() > 1 {
            for (k, _) in item.pictures.iter().enumerate() {
                head = head.child(button(format!("tab-{k}"), &format!("Screen {}", k + 1), k == self.marks.pic, look::text()).on_click(cx.listener(move |this, _, _, cx| {
                    this.marks.pic = k;
                    cx.notify();
                })));
            }
        }
        head = head
            .child(div().flex_1())
            .child(div().flex_none().text_size(px(14.)).text_color(rgb(look::text())).child(format!("{} · {}", self.marks.dir + 1, names[self.marks.dir])))
            .child(div().min_w_0().text_size(px(12.5)).text_color(rgb(look::faint())).max_w(px(420.)).truncate().child(idea(&item.label)))
            .child(div().w(px(6.)).flex_none())
            .when_some(self.live_said.clone(), |d, why| d.child(div().min_w_0().max_w(px(360.)).truncate().text_size(px(12.)).text_color(rgb(look::amber())).child(why)))
            .when(!item.pictures.is_empty(), |d| d.child(strong("enlarge", look::Icon::Enlarge, "Enlarge").on_click(cx.listener(|this, _, window, cx| this.open_viewer(window, cx)))))
            .when_some(item.page.clone(), |d, page| {
                d.child(strong("live", look::Icon::Open, if self.live_opening { "Opening…" } else { "Open live version" }).on_click(cx.listener(move |this, _, _, cx| this.open_live(page.clone(), cx))))
            });
        // the stage: this direction's picture, at the size the window gives it; a click shows it large
        let stage = match item.pictures.get(self.marks.pic).or(item.pictures.first()) {
            Some(p) => div()
                .id("stage")
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .cursor(CursorStyle::PointingHand)
                .on_click(cx.listener(|this, _, window, cx| this.open_viewer(window, cx)))
                .child(img(p.clone()).size_full().object_fit(ObjectFit::Contain).rounded(px(10.))),
            None => div().id("stage").flex_1().flex().items_center().justify_center().text_color(rgb(look::faint())).child("This direction has no picture."),
        };
        // the directions as a strip of large pictures under the stage, each with its star; none for one option
        // the hint after the directions, or on a line of its own under them when the window leaves it no room
        let mut strip_row = div().flex().flex_wrap().gap(px(12.)).items_end().flex_none();
        for (i, it) in stop.items.iter().enumerate() {
            let on = i == self.marks.dir;
            let starred = self.marks.keep.contains(&i);
            let pins = self.marks.pins.iter().filter(|(d, _)| *d == i).count();
            let ringed = self.go_hover && chosen == Some(i);
            strip_row = strip_row.child(
                tabbed(div().id(SharedString::from(format!("dir-{i}"))))
                    .w(px(208.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(6.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(rgb(if on { look::text() } else { look::line_2() }))
                    // a fill of its own, so the ring drawn under it shows only round it
                    .bg(rgb(look::panel()))
                    .when(ringed, |d| d.shadow(ring(2.)))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgb(look::panel_2())))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.marks.pick(i);
                        this.reckon(cx);
                    }))
                    .child(match it.pictures.first() {
                        Some(p) => img(p.clone()).w_full().h(px(112.)).object_fit(ObjectFit::Cover).rounded(px(6.)).into_any_element(),
                        None => div().w_full().h(px(112.)).rounded(px(6.)).bg(rgb(look::panel_2())).into_any_element(),
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(12.5)).text_color(rgb(if on { look::text() } else { look::dim() })).child(card_label(i, &names[i], pins)))
                            .child(
                                tabbed(div().id(SharedString::from(format!("star-{i}"))))
                                    .flex_none()
                                    .px(px(4.))
                                    .rounded(px(5.))
                                    .bg(rgb(look::panel()))
                                    .text_size(px(12.))
                                    .text_color(rgb(if starred { look::amber() } else { look::faint() }))
                                    .hover(|d| d.text_color(rgb(look::text())))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.star(i, cx);
                                    }))
                                    .child(star_word(starred)),
                            ),
                    ),
            );
        }
        // after a third shortlist, why it did not mark, in the hint's place for a while
        let full = self.full_at.is_some_and(|t| t.elapsed() < FULL_FOR);
        strip_row = strip_row.child(div().pb(px(10.)).pl(px(4.)).text_size(px(12.)).text_color(rgb(if full { look::amber() } else { look::faint() })).child(if full { FULL } else { HINT }));
        let left = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(18.))
            .pt(px(14.))
            .pb(px(10.))
            .child(head)
            .child(stage)
            .when(strip(n), |d| d.child(strip_row));
        // the side: this direction's parts with their pins, and what was said of it
        let mut parts = div().flex().flex_col().gap(px(8.));
        for (label, text) in &item.aspects {
            let on = self.marks.pins.contains(&(self.marks.dir, label.clone()));
            let (d, l) = (self.marks.dir, label.clone());
            parts = parts.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .p(px(8.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(if on { look::amber() } else { look::line() }))
                    .child(div().flex().items_center().child(div().flex_1().text_size(px(12.5)).text_color(rgb(look::text())).child(label.clone())).child(button(format!("pin-{label}"), keep_word(on), on, look::amber()).on_click(cx.listener(move |this, _, _, cx| this.pin(d, l.clone(), cx)))))
                    .child(div().text_size(px(12.)).text_color(rgb(look::dim())).child(text.clone())),
            );
        }
        let mut back = div().flex().flex_col().gap(px(6.));
        for b in &stop.back {
            let on = self.marks.bring.as_deref() == Some(b.id.as_str());
            let id = b.id.clone();
            back = back.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .children(b.pictures.first().map(|p| img(p.clone()).w(px(48.)).h(px(30.)).object_fit(ObjectFit::Cover).rounded(px(4.))))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.5)).text_color(rgb(look::dim())).child(name(&b.label)))
                    .child(button(format!("bring-{}", b.id), if on { "Comes back" } else { "Bring back" }, on, look::amber()).on_click(cx.listener(move |this, _, _, cx| {
                        this.marks.bring = if this.marks.bring.as_deref() == Some(id.as_str()) { None } else { Some(id.clone()) };
                        this.marks.touched = true;
                        this.reckon(cx);
                    }))),
            );
        }
        // the conversation at the stop about this option, round by round: the newest whole, lit in amber while it is the
        // agent's and the person has not answered; the earlier on a line each
        let said = conversation(&thread.entries, Some(item.id.as_str()));
        let talk = (!said.is_empty() || thread.asking).then(|| {
            let mut list = div().flex().flex_col().gap(px(8.)).child(look::label(&format!("Conversation at “{}”", stop.step), look::faint()));
            let (last, mut round) = (said.len().saturating_sub(1), 0);
            for (k, s) in said.iter().enumerate() {
                if s.round != round {
                    round = s.round;
                    list = list.child(div().text_size(px(11.5)).text_color(rgb(look::faint())).child(format!("Round {round}")));
                }
                let newest = k == last && !thread.asking;
                let lit = newest && s.agent;
                let (who, rest) = s.head.split_once(" · ").map(|(a, b)| (a.to_string(), format!(" · {b}"))).unwrap_or((s.head.clone(), String::new()));
                let text = div().text_size(px(12.5)).text_color(rgb(look::text())).child(s.text.clone());
                list = list.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .px(px(10.))
                        .py(px(8.))
                        .rounded(px(8.))
                        .bg(rgb(look::panel_2()))
                        .border_1()
                        .border_color(rgb(if lit { look::amber() } else { look::line() }))
                        .child(div().flex().text_size(px(11.5)).child(div().text_color(rgb(if lit { look::amber() } else { look::faint() })).child(who)).child(div().text_color(rgb(look::faint())).child(rest)))
                        .child(if newest { text } else { text.truncate() }),
                );
            }
            if thread.asking {
                list = list.child(
                    div().px(px(10.)).py(px(8.)).rounded(px(8.)).bg(rgb(look::panel_2())).border_1().border_color(rgb(look::amber())).text_size(px(12.5)).text_color(rgb(look::dim())).child(ANSWERING),
                );
            }
            list
        });
        let side = div()
            .id("compare-side")
            // wider with a conversation in it, as the design's column A (fio.md)
            .w(px(if talk.is_some() { 380. } else { 300. }))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(rgb(look::line()))
            .bg(rgb(look::panel()))
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .when(!item.aspects.is_empty(), |d| d.child(div().flex().flex_col().gap(px(6.)).child(look::label(PARTS, look::faint())).child(parts)))
            .when(!stop.back.is_empty(), |d| d.child(div().flex().flex_col().gap(px(6.)).child(look::label("You dropped earlier · only you bring it back", look::faint())).child(back)))
            .when(!item.detail.is_empty(), |d| d.child(div().flex().flex_col().gap(px(6.)).child(look::label("What was said of it", look::faint())).child(div().text_size(px(12.5)).text_color(rgb(look::dim())).child(item.detail.clone()))))
            .children(talk);
        // the bar: none, the person's words, what the answer does, and the button that sends it
        let words = self.words.clone();
        let understood = match (&self.understood, self.reading) {
            (_, true) => Some(div().text_size(px(12.5)).text_color(rgb(look::faint())).child("Reading your words…")),
            (Some(Err(e)), _) => Some(div().text_size(px(12.5)).text_color(rgb(look::red())).child(e.clone())),
            (Some(Ok(u)), _) => {
                let ok = u.refused.is_none() && !self.sending;
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(look::label("What Jaynes read · this is what it does when you send", look::blue()))
                        .child(sentences(&u.sentences))
                        .when(!u.unsure.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(rgb(look::amber())).child(format!("Not sure: {}", u.unsure))))
                        .when_some(u.refused.clone(), |d, r| d.child(div().text_size(px(12.)).text_color(rgb(look::red())).child(r)))
                        .child(div().flex().gap(px(8.)).child(primary("send-read", "Send this".into(), ok).when(ok, |d| d.on_click(cx.listener(|this, _, _, cx| this.send_understood(cx))))).child(button("by-hand".into(), "Answer with the buttons", false, look::grey()).on_click(cx.listener(|this, _, _, cx| {
                            this.understood = None;
                            cx.notify();
                        })))),
                )
            }
            _ => None,
        };
        let none_part = if self.marks.none {
            let why = self.why.clone();
            div()
                .flex()
                .gap(px(8.))
                .items_center()
                .children(why.map(|w| div().w(px(340.)).child(focus_field("why-field", &w, window, cx))))
                .child(button("none-cancel".into(), "cancel", false, look::grey()).on_click(cx.listener(|this, _, _, cx| {
                    this.marks.none = false;
                    this.reckon(cx);
                })))
        } else {
            div().child(button("none".into(), none_word(n), false, look::red()).on_click(cx.listener(|this, _, window, cx| {
                this.marks.none = true;
                this.marks.touched = true;
                if this.why.is_none() {
                    this.why = Some(this.input("what was missing in all of them?", window, cx));
                }
                this.reckon(cx);
            })))
        };
        let words_field = words.map(|w| focus_field("words-field", &w, window, cx));
        // with one option and words: whether they go as a question or a change, guessed and changed by a click
        let has_words = !Self::text(self.words.as_ref(), cx).trim().is_empty();
        let kind = (one_option(&stop) && has_words).then(|| {
            let pick = |id: &'static str, word: &'static str, on: bool, q: bool| {
                tabbed(div().id(id))
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .text_size(px(12.5))
                    .cursor_pointer()
                    .text_color(rgb(if on { look::text() } else { look::faint() }))
                    .when(on, |d| d.bg(rgb(look::selected())))
                    .child(word)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.ask_kind = Some(q);
                        cx.notify();
                    }))
            };
            div().flex().flex_none().p(px(2.)).rounded(px(8.)).border_1().border_color(rgb(look::line_2())).child(pick("kind-question", QUESTION, asks, true)).child(pick("kind-change", CHANGE, !asks, false))
        });
        let panel = said_panel(&self.reckoned.sentences, self.all_said).map(|all| {
            let mut list = div().flex().flex_col().gap(px(4.)).child(look::label("What this does", look::faint()));
            for s in all {
                list = list.child(div().text_size(px(12.5)).text_color(rgb(look::dim())).child(s));
            }
            div()
                .id("more-panel")
                .absolute()
                .bottom_full()
                .left(px(18.))
                .max_w(px(640.))
                .p(px(12.))
                .rounded(px(10.))
                .bg(rgb(look::panel_2()))
                .border_1()
                .border_color(rgb(look::line_2()))
                .shadow(vec![BoxShadow { color: hsla(0., 0., 0., 0.28), offset: point(px(0.), px(6.)), blur_radius: px(18.), spread_radius: px(0.), inset: false }])
                .child(list)
                .with_animation("more-in", Animation::new(Duration::from_millis(140)), |d, t| d.opacity(t).mb(px(8. - 4. * (1. - t))))
        });
        let go = primary("go", if self.sending { "Sending…".into() } else { format!("{label} ↵") }, can)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.go_hover = *hovered;
                cx.notify();
            }))
            .when(can, |d| d.on_click(cx.listener(|this, _, _, cx| this.go(cx))));
        // Enter with nothing chosen yet: one pulse of the button, and nothing goes
        let go = if self.pulse > 0 {
            go.with_animation(("go-pulse", self.pulse), Animation::new(Duration::from_millis(480)), |d, t| {
                let alpha = ((1. - t) * 0.6 * 255.) as u32;
                d.shadow(vec![BoxShadow { color: rgba((look::blue() << 8) | alpha).into(), offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(8. * t), inset: false }])
            })
            .into_any_element()
        } else {
            go.into_any_element()
        };
        let bar = div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(18.))
            .py(px(12.))
            .border_t_1()
            .border_color(rgb(look::line()))
            .bg(rgb(look::panel()))
            .child(div().flex().gap(px(8.)).items_center().child(none_part).children(kind).child(div().flex_1().children(words_field)).child(button("read".into(), "Read my words", false, look::blue()).on_click(cx.listener(|this, _, _, cx| this.read_words(cx)))))
            .children(understood)
            .child(
                div()
                    .flex()
                    .gap(px(12.))
                    .items_end()
                    .child(div().flex_1().min_w_0().child(if thread.asking {
                        div().text_size(px(12.5)).text_color(rgb(look::dim())).child(asked_foot(&stop.step)).into_any_element()
                    } else if asks {
                        div().text_size(px(12.5)).text_color(rgb(look::dim())).child(ASK_FOOT).into_any_element()
                    } else {
                        self.consequences(cx)
                    }))
                    .when_some(self.reckoned.refused.clone(), |d, r| d.child(div().max_w(px(360.)).text_size(px(12.)).text_color(rgb(look::red())).child(r)))
                    .when_some(self.said.clone(), |d, s| d.child(div().text_size(px(12.)).text_color(rgb(look::faint())).child(s)))
                    .when_some(also.filter(|_| !self.sending), |d, a| d.child(button("also".into(), &a, false, look::grey()).on_click(cx.listener(|this, _, _, cx| this.go_also(cx)))))
                    .child(go),
            )
            .children(panel);
        let top = div()
            .flex()
            .gap(px(14.))
            .items_center()
            .px(px(18.))
            .py(px(12.))
            .border_b_1()
            .border_color(rgb(look::line()))
            .bg(rgb(look::panel()))
            .pl(px(84.))
            .child(div().size(px(8.)).rounded_full().bg(rgb(look::amber())))
            .child(div().flex_1().min_w_0().flex().flex_col().child(div().truncate().text_size(px(16.)).text_color(rgb(look::text())).child(stop.title.clone())).when_some(under_title(&self.name, &stop.step, under_about(&stop)), |d, under| d.child(div().truncate().text_size(px(12.5)).text_color(rgb(look::dim())).child(under))))
            .when(stop.round > 0, |d| d.child(div().text_size(px(12.)).text_color(rgb(look::faint())).child(format!("round {}", stop.round))))
            // as wide as its longer words, so "round" does not move when they change; a press on it keeps the keys
            // where they are, so it does what it says
            .child(
                button("close".into(), chip(field), false, look::grey())
                    .min_w(px(190.))
                    .flex()
                    .justify_center()
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.some_field_focused(window, cx) {
                            this.leave_field(window, cx);
                        } else {
                            this.close(cx);
                        }
                    })),
            );
        let viewer = if self.viewer.is_some() { self.viewer_view(window, cx) } else { None };
        let viewing = viewer.is_some();
        div()
            .id("compare")
            .track_focus(&self.focus)
            .absolute()
            .inset_0()
            // over the terminal, which would otherwise take the press under it: the keys, and the release, so no
            // click here ever reached its button and the field lost the keys as it got them
            .occlude()
            .bg(rgb(look::panel()))
            .flex()
            .flex_col()
            .font_family(look::UI_FONT)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| this.key_down(e, window, cx)))
            // Command-0 and Command-9 are the runs column's and the ninth tab's: with the picture large, the viewer's
            .when(viewing, |d| {
                d.on_action(cx.listener(|this, _: &crate::ToggleRuns, window, cx| this.viewer_does(Does::Actual, window, cx)))
                    .on_action(cx.listener(|this, _: &crate::Tab9, window, cx| this.viewer_does(Does::Fit, window, cx)))
            })
            .child(top)
            .child(div().flex_1().min_h_0().flex().child(left).child(side))
            .child(bar)
            .children(viewer)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{ANSWERING, ASK_FOOT, ASK_ONE, Said, asked_foot, change_foot, conversation, looks_like_question, Enter, Esc, FULL, HINT, Marks, PARTS, Starred, WORDS_HINT, card_label, chip, esc, invite, keep_word, mix_note, none_word, one_option, part_word, parts_phrase, said_panel, said_row, same_stop, shortlist_note, star_word, strip, under_about, under_title, words_hint};

    #[test]
    fn the_notes_name_the_parts_as_a_sentence_does_from_the_labels_the_stop_has() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let shortlisted = jaynes_core::decide::SHORTLISTED;
        // in lower case inside a sentence, unless the label is written in capitals
        assert_eq!(part_word("Palette"), "palette");
        assert_eq!(part_word("CTA"), "CTA");
        assert_eq!(parts_phrase(&s(&["Palette"])), "palette");
        assert_eq!(parts_phrase(&s(&["Palette", "Type"])), "palette and type");
        assert_eq!(parts_phrase(&s(&["Palette", "Type", "CTA"])), "palette, type and CTA");
        // the shortlist's note, from the labels as the stop has them
        assert_eq!(shortlist_note(&s(&["Palette"])), format!("{shortlisted} Its palette stays."));
        assert_eq!(shortlist_note(&s(&["Palette", "Fonts"])), format!("{shortlisted} Its palette and fonts stay."));
        // the mix's, in the same words
        assert_eq!(mix_note("Harbor", &s(&["Palette", "Fonts"]), &[("Palette".into(), "Night".into(), "navy, white".into())]), "Mix: keep the palette and fonts of Harbor. Take the palette of Night: navy, white");
        // and through the button that sends them: two shortlisted, the first with its palette kept
        let stop = choosing(&["Harbor", "Night", "Ledger"]);
        let two = Marks { keep: vec![0, 1], pins: [(0, "Palette".to_string())].into(), ..Marks::default() };
        let (_, body) = two.proposal(&stop, "", "").expect("a proposal");
        assert_eq!(body["notes"]["harbor"], format!("{shortlisted} Its palette stays."));
        let mix = Marks { pins: [(0, "Palette".to_string()), (0, "Fonts".to_string()), (1, "Palette".to_string())].into(), ..Marks::default() };
        let (_, body) = mix.proposal(&stop, "", "").expect("a proposal");
        assert_eq!(body["notes"]["harbor"], "Mix: keep the fonts and palette of Harbor. Take the palette of Night: cream, ink");
    }

    #[test]
    fn a_shortlisted_direction_carries_what_of_it_stays_in_one_sentence() {
        assert_eq!(shortlist_note(&[]), jaynes_core::decide::SHORTLISTED);
        assert_eq!(shortlist_note(&["palette".to_string()]), format!("{} Its palette stays.", jaynes_core::decide::SHORTLISTED));
        assert_eq!(shortlist_note(&["palette".to_string(), "type".to_string()]), format!("{} Its palette and type stay.", jaynes_core::decide::SHORTLISTED));
        assert_eq!(shortlist_note(&["palette".into(), "type".into(), "layout".into()]), format!("{} Its palette, type and layout stay.", jaynes_core::decide::SHORTLISTED));
    }
    use jaynes_core::question::QuestionKind;
    use jaynes_shell::stop::{Item, Stop};

    fn item(id: &str, label: &str) -> Item {
        Item { id: id.into(), label: label.into(), detail: String::new(), aspects: vec![("Palette".into(), "cream, ink".into()), ("Fonts".into(), "a serif".into())], pictures: vec![], page: None }
    }

    /// A stop that chooses among `names`, invented.
    fn choosing(names: &[&str]) -> Stop {
        Stop {
            run: "r".into(),
            question: "q".into(),
            shown: "s".into(),
            title: "Which direction goes on?".into(),
            asks: "Which direction goes on?".into(),
            step: "directions".into(),
            permission: false,
            kind: QuestionKind::Decision,
            choose: true,
            offered: vec![],
            items: names.iter().map(|n| item(&n.to_lowercase(), &format!("{n}: an idea"))).collect(),
            back: vec![],
            round: 1,
            about: String::new(),
            ahead: None,
            touch: None,
            into: String::new(),
            lands: vec![],
            passed: None,
            answers: Default::default(),
            why: None,
            consequences: Default::default(),
            wants: None,
        }
    }

    fn label(m: &Marks, stop: &Stop) -> String {
        m.proposal(stop, "", "").expect("a proposal").0
    }

    #[test]
    fn enter_sends_only_after_the_person_chose_something_here() {
        // the mode just opened on the first option: Enter pulses the button and sends nothing
        assert_eq!(Marks::default().enter(), Enter::Pulse);
        let mut flipped = Marks::default();
        flipped.flip(1, 3);
        assert_eq!((flipped.dir, flipped.enter()), (1, Enter::Go));
        // one option: nowhere to flip, so nothing chosen yet
        let mut one = Marks::default();
        one.flip(1, 1);
        assert_eq!((one.dir, one.enter()), (0, Enter::Pulse));
        let mut picked = Marks::default();
        picked.pick(0);
        assert_eq!(picked.enter(), Enter::Go);
        let mut starred = Marks::default();
        starred.star(2);
        assert_eq!(starred.enter(), Enter::Go);
        let mut kept = Marks::default();
        kept.pin(0, "Palette".into());
        assert_eq!(kept.enter(), Enter::Go);
    }

    #[test]
    fn a_third_shortlist_is_refused_and_says_so_without_taking_one_off() {
        let mut m = Marks::default();
        assert_eq!(m.star(0), Starred::On);
        assert_eq!(m.star(2), Starred::On);
        assert_eq!(m.star(1), Starred::Full);
        assert_eq!(m.keep, vec![0, 2]);
        assert_eq!(FULL, "Two at most: unshortlist one first");
        assert_eq!(m.star(0), Starred::Off);
        assert_eq!(m.star(1), Starred::On);
        assert_eq!(m.keep, vec![2, 1]);
    }

    // Alan, 20/11, in the video of the choice screen: "o que são esses pins?" (the design-lead's checkup, item 14): the
    // parts' pins became "Keep this part" and the star "☆ Shortlist", each saying what it does
    #[test]
    fn asked_what_are_these_pins() {
        assert_eq!(keep_word(false), "Keep this part");
        assert_eq!(keep_word(true), "Kept ✓");
        assert_eq!(PARTS, "Its parts · keep one to carry it on; parts from two make a mix");
        assert_eq!(star_word(false), "☆ Shortlist");
        assert_eq!(star_word(true), "★ Shortlisted");
        assert_eq!(HINT, "← → flip · shortlist two to develop both");
        assert_eq!(card_label(1, "Night", 1), "2 · Night · 1 kept");
        assert_eq!(card_label(0, "Harbor", 0), "1 · Harbor");
    }

    #[test]
    fn the_line_under_the_question_says_which_run_asks() {
        assert_eq!(under_title("Three directions for the look", "directions", "the look of the site").as_deref(), Some("Three directions for the look · at “directions”: the look of the site"));
        assert_eq!(under_title("Three directions for the look", "directions", "").as_deref(), Some("Three directions for the look · at “directions”"));
        // a run with no name read yet: the step and what it does, as before
        assert_eq!(under_title("", "directions", "the look of the site").as_deref(), Some("At “directions”: the look of the site"));
        assert_eq!(under_title("", "directions", ""), None);
    }

    #[test]
    fn marks_kept_from_another_round_never_point_past_its_options() {
        let look = choosing(&["Harbor", "Night", "Ledger"]);
        // a new round of the same question shows something else: what was marked for the last one does not come back
        assert!(!same_stop(&look, &Stop { shown: "s2".into(), ..look.clone() }));
        // and marks that come back fit the options there are
        let mut m = Marks { dir: 2, pic: 1, pins: [(2, "Palette".to_string()), (0, "Fonts".to_string())].into(), keep: vec![0, 2], ..Marks::default() };
        m.fit(2);
        assert_eq!((m.dir, m.pic), (0, 0));
        assert_eq!(m.pins, [(0, "Fonts".to_string())].into());
        assert_eq!(m.keep, vec![0]);
    }

    #[test]
    fn marks_stay_only_for_the_same_question_of_the_same_run() {
        let look = choosing(&["Harbor", "Night", "Ledger"]);
        let icon = Stop { run: "another run".into(), items: look.items[..1].to_vec(), ..look.clone() };
        assert!(same_stop(&look, &look.clone()));
        // the same step's question in another run: what was marked for three options is not carried to one
        assert!(!same_stop(&look, &icon));
        assert!(!same_stop(&look, &Stop { question: "q2".into(), ..look.clone() }));
    }

    // Alan, 20/11: "o esc nao funciona se o foco tiver no input, nao sei se e a melhor forma"
    #[test]
    fn asked_esc_works_with_the_focus_in_the_field() {
        assert_eq!(esc(true, true, true), Esc::Viewer);
        assert_eq!(esc(false, true, true), Esc::More);
        assert_eq!(esc(false, false, true), Esc::Field);
        assert_eq!(esc(false, false, false), Esc::Screen);
        // the field with the keys: a first Esc leaves it, and the chip at the top says so; then the mode closes
        assert_eq!(chip(true), "Esc · leave the field");
        // from a run's tab or the Inbox too, not only from the terminal: back where the person was
        assert_eq!(chip(false), "Esc · back");
    }

    // Alan, 20/11: "aparece "compare" mesmo so com uma opcao"
    #[test]
    fn asked_no_compare_with_only_one_option() {
        assert_eq!(invite(1, "Go with “Harbor”?"), ("◆ Go with “Harbor”?".to_string(), "Open"));
        assert_eq!(invite(3, "Which direction goes on?"), ("◆ 3 options · Which direction goes on?".to_string(), "Compare"));
        assert!(!strip(1) && strip(2));
        assert_eq!(none_word(1), "Not this one");
        assert_eq!(none_word(3), "None of these");
        let one = choosing(&["Harbor"]);
        let none = Marks { none: true, ..Marks::default() };
        assert_eq!(label(&none, &one), "Not this one: make a new one");
        assert_eq!(label(&none, &choosing(&["Harbor", "Night", "Ledger"])), "None of these: make 3 new ones");
    }

    // Alan, 21/11: "entender onde o python entra antes de decidir"
    #[test]
    fn asked_one_option_asks_go_with_it_or_not_yet_and_never_chooses_his_question() {
        let mut one = choosing(&["Python + FastAPI"]);
        one.title = "Go with “Python + FastAPI”?".into();
        one.asks = "Which stack goes on?".into();
        one.about = "Picks the stack.".into();
        let shown = Marks::default();
        let words = "entender onde o python entra antes de decidir";
        // the field says words can ask before choosing; with two or more it says what it said
        assert_eq!(words_hint(one_option(&one)), "Not sure yet? Ask the agent, or say what to change");
        assert_eq!(words_hint(one_option(&choosing(&["Harbor", "Night"]))), WORDS_HINT);
        // the field empty: going with it is the choice
        let (go, body) = shown.proposal(&one, "", "").expect("a proposal");
        assert_eq!((go.as_str(), body["verdicts"]["python + fastapi"].as_str()), ("Go with this one", Some("keep")));
        assert_eq!(shown.also(&one, ""), None);
        // his words: not yet, a change carrying them, never a keep; nothing is chosen
        let (go, body) = shown.proposal(&one, words, "").expect("a proposal");
        assert_eq!(go, "Not yet: send my words");
        assert_eq!(body["verdicts"]["python + fastapi"].as_str(), Some("change"));
        assert_eq!(body["notes"]["python + fastapi"].as_str(), Some(words));
        assert_eq!(body["context"].as_str(), Some(""));
        // and beside it, going with it anyway, his words sent beside it as before
        let (also, body) = shown.also(&one, words).expect("the button beside");
        assert_eq!((also.as_str(), body["verdicts"]["python + fastapi"].as_str(), body["context"].as_str()), ("Go with this one", Some("keep"), Some(words)));
        // "Not this one" stays
        assert_eq!(label(&Marks { none: true, ..Marks::default() }, &one), "Not this one: make a new one");
        assert_eq!(Marks { none: true, ..Marks::default() }.also(&one, words), None);
        // the role's question goes on the line under the title, which asks whether to go with it
        assert_eq!(under_title("Pick a stack", &one.step, under_about(&one)).as_deref(), Some("Pick a stack · at “directions”: Which stack goes on?"));
        assert_eq!(invite(1, &one.title).0, "◆ Go with “Python + FastAPI”?");
        // with two or more nothing changes: the words go beside the choice
        let two = choosing(&["Harbor", "Night"]);
        assert_eq!(label(&shown, &two), "Choose this one");
        assert_eq!(shown.proposal(&two, words, "").expect("a proposal").1["context"].as_str(), Some(words));
        assert_eq!(shown.also(&two, words), None);
        assert_eq!(under_about(&Stop { about: "Picks the look.".into(), ..two }), "Picks the look.");
    }

    // Alan, 20/11: "o "Go on with.." fez eu achar que a tecnologia ia ser "Go" de golang, vamos deixar mais claro oq o
    // botao fala"
    #[test]
    fn asked_the_main_button_says_clearly_what_it_does_not_go_on_with_read_as_go() {
        let stop = choosing(&["Go", "Night", "Ledger"]);
        let shown = Marks { dir: 0, ..Marks::default() };
        assert_eq!(label(&shown, &stop), "Choose this one");
        assert_eq!(shown.chosen(&stop), Some(0));
        // a part pinned in Night while Ledger shows: the button chooses Night, says so, and rings Night's card
        let pinned = Marks { dir: 2, pins: [(1, "Palette".to_string())].into(), ..Marks::default() };
        assert_eq!(label(&pinned, &stop), "Choose the one with kept parts");
        assert_eq!(pinned.chosen(&stop), Some(1));
        // the other buttons say what they do, as before
        let two = Marks { keep: vec![0, 2], ..Marks::default() };
        assert_eq!(label(&two, &stop), "Develop Go and Ledger");
        assert_eq!(two.chosen(&stop), None);
        let mix = Marks { pins: [(0, "Palette".to_string()), (1, "Fonts".to_string())].into(), ..Marks::default() };
        assert!(label(&mix, &stop).starts_with("Mix: "));
    }

    // Alan, 20/11: "o fewer vai la pra lado esquerdo, ruim" (the switch of the list of what the answer does)
    #[test]
    fn asked_the_fewer_switch_does_not_jump_to_the_left() {
        let said: Vec<String> = ["\"Harbor\" is chosen.", "\"Night\" leaves.", "\"Ledger\" leaves.", "directions keeps only what you chose."].map(String::from).to_vec();
        let closed = said_row(&said, false).expect("a line");
        let open = said_row(&said, true).expect("still the line, with the list open");
        assert_eq!(closed.0, open.0, "the first sentence stays where it was");
        assert_eq!(closed.1.as_deref(), Some("▸ 3 more of what this does"));
        assert_eq!(open.1.as_deref(), Some("▾ Hide what this does"));
        assert_eq!(said_panel(&said, false), None);
        assert_eq!(said_panel(&said, true), Some(said.clone()));
        // one sentence: no switch and no list
        let one = vec!["\"Harbor\" is chosen.".to_string()];
        assert_eq!(said_row(&one, true), Some((one[0].clone(), None)));
        assert_eq!(said_panel(&one, true), None);
    }

    // Alan, 16/11 (as the app's first handoff wrote what he chose): "A design stop is not an island: it opens a
    // full-window compare mode. It says what the stop fixes now and what comes later; shows each direction at real
    // size, flipped in place with the arrow keys; has tabs for main, empty and phone; a live-version link; the
    // reference beside; pins saying "this stays" to mix directions; keep two; none with a reason; the round."
    #[test]
    fn asked_a_design_stop_opens_a_full_window_compare_mode() {
        use crate::viewer::{Does, key};
        let stop = choosing(&["Harbor", "Night", "Ledger"]);
        // flipped in place with the arrow keys, round the strip; the up and down arrows go through a direction's screens
        assert_eq!((key("right", false), key("left", false)), (Some(Does::Direction(1)), Some(Does::Direction(-1))));
        assert_eq!((key("down", false), key("up", false)), (Some(Does::Screen(1)), Some(Does::Screen(-1))));
        let mut m = Marks::default();
        m.flip(-1, 3);
        assert_eq!(m.dir, 2);
        // parts kept from two directions make a mix
        let mix = Marks { pins: [(0, "Palette".to_string()), (1, "Fonts".to_string())].into(), ..Marks::default() };
        assert!(label(&mix, &stop).starts_with("Mix: "));
        // keep two, never a third
        let mut two = Marks::default();
        assert_eq!((two.star(0), two.star(2), two.star(1)), (Starred::On, Starred::On, Starred::Full));
        assert_eq!(label(&two, &stop), "Develop Harbor and Ledger");
        // none, with what was missing in all of them going with the answer
        let none = Marks { none: true, ..Marks::default() };
        let (said, body) = none.proposal(&stop, "", "all too dark").expect("a proposal");
        assert_eq!(said, "None of these: make 3 new ones");
        assert!(body["context"].as_str().is_some_and(|c| c.contains("all too dark")), "{body}");
    }

    // Alan, 16/11 (as the app's first handoff wrote what he chose): "the lock binds the agent, never the person. Every
    // stop has "Your words" (read into verdicts, effects written by Jaynes), bring back what was dropped, and the parts
    // of an option shown apart."
    #[test]
    fn asked_the_choice_has_your_words_and_the_parts_of_an_option_apart() {
        assert!(WORDS_HINT.starts_with("Your words"));
        let stop = choosing(&["Harbor", "Night"]);
        // the person's words go with whatever the button sends
        let (_, body) = Marks::default().proposal(&stop, "warmer, please", "").expect("a proposal");
        assert!(body["context"].as_str().is_some_and(|c| c.contains("warmer, please")), "{body}");
        // each option's parts shown apart, each one kept on its own
        assert_eq!(stop.items[0].aspects.len(), 2);
        assert!(PARTS.starts_with("Its parts"));
        assert_eq!(keep_word(false), "Keep this part");
    }

    // Alan, 21/11: "eu queria uma thread de conversa com a jaynes e nao que minha pergunta sumisse". A question gets
    // its answer beside the option, nothing redone; his words and the agent's stay in view round by round (design
    // 0.2.5 fio.md, A, the column beside the option)
    #[test]
    fn asked_question_is_answered_without_redoing_the_step() {
        use jaynes_core::run::{Entry, EntryKind};
        let e = |round: u32, kind: EntryKind, text: &str, item: Option<&str>| Entry {
            round,
            question: if kind == EntryKind::Reply { String::new() } else { "q-stacks".into() },
            kind,
            text: text.into(),
            item: item.map(str::to_string),
            late: false,
            ask: matches!(kind, EntryKind::Ask | EntryKind::Answer | EntryKind::AskFailed).then(|| "q-stacks-a1".into()),
        };
        let thread = [
            // the engine counts a step's rounds from 0 (StepState.thread's round), the stop from 1 (its pass, "round 2"
            // at the top): the column says the stop's, so a change at round 0 is "round 1"
            e(0, EntryKind::Change, "entender onde o Python entra antes de decidir", Some("python")),
            e(1, EntryKind::Reply, "Python entra em 7 das 9 peças. Mantive Python.", None),
            e(1, EntryKind::Ask, "e se fosse Go só na API?", Some("python")),
            e(1, EntryKind::Answer, "Daria, com o fatiador saindo da API. Não mudei nada.", Some("python")),
            // about another option: not in this one's column
            e(1, EntryKind::Ask, "and Rust?", Some("rust")),
        ];
        let said = conversation(&thread, Some("python"));
        let heads: Vec<(&str, &str, bool)> = said.iter().map(|s| (s.head.as_str(), s.text.as_str(), s.agent)).collect();
        assert_eq!(heads, [
            ("You · round 1 · a change", "entender onde o Python entra antes de decidir", false),
            ("Agent · round 2 · changed the option", "Python entra em 7 das 9 peças. Mantive Python.", true),
            ("You · round 2 · a question", "e se fosse Go só na API?", false),
            ("Agent · answered · nothing redone", "Daria, com o fatiador saindo da API. Não mudei nada.", true),
        ]);
        assert_eq!(said.iter().map(|s| s.round).collect::<Vec<_>>(), [1, 2, 2, 2]);
        // an answer that came after the stop was answered, and a question with no answer
        let late = Entry { late: true, ..thread[3].clone() };
        let failed = e(1, EntryKind::AskFailed, "Jaynes stopped while the agent answered: ask again", Some("python"));
        let said = conversation(&[late, failed], Some("python"));
        assert_eq!(said[0].head, "Agent · answered late · it changed nothing");
        assert_eq!((said[1].head.as_str(), said[1].agent), ("No answer", true));
        let _: Option<&Said> = said.first();

        // the guess without a model: a question ends in "?" or opens as one; a click changes it
        for q in ["e se fosse Go?", "what if it were Go", "Por que Qdrant", "why not Go", "How fast is it", "como fica a API"] {
            assert!(looks_like_question(q), "{q}");
        }
        for c in ["usa Go na API", "make the API async", "  "] {
            assert!(!looks_like_question(c), "{c}");
        }
        // the words under the field, and while the agent answers
        assert_eq!(ASK_ONE, "Ask the agent ↵");
        assert_eq!(ASK_FOOT, "The agent answers here. Nothing is redone and nothing is chosen.");
        assert_eq!(change_foot("Python, Postgres and Qdrant", "stacks"), "“Python, Postgres and Qdrant” is not chosen yet: your words go to the agent at “stacks”, and it asks you again.");
        assert_eq!(ANSWERING, "The agent is answering. Nothing is redone.");
        assert_eq!(asked_foot("stacks"), "Your question went to the agent at “stacks”. It answers here; nothing is redone and nothing is chosen.");
    }
}
