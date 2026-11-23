//! The stop, risen over the terminal as an island: the question the picked run waits at, a verdict per item, the
//! person's words where they send something back, what they dropped earlier and may bring back, what the answer will
//! do in the engine's own sentences (G11), and Send. The person may also answer in their own words: Jaynes reads them
//! into the answer they mean and shows what it does; nothing goes until they send it. Esc · later folds it to a pill,
//! and over Claude Code's terminal only the pill shows, which opens the run's tab. A stop that chooses among designs
//! opens the compare mode instead, over the whole window. Every button of a stop is in the Tab order.

use crate::answer::{self, Reckoned, Understood};
use crate::compare::Compare;
use crate::key::Key;
use crate::lightbox::{Lightbox, Shown};
use crate::look;
use crate::model::Runs;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_base::input::{Input, InputEditorStyle, InputEvent, InputState, Position};
use jaynes_core::question::{QuestionKind, VerdictKind};
use jaynes_shell::stop::{self, Stop};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    /// Whether the engine's words on a run whose engine left show, under Details.
    gone_open: bool,
    /// What came of the last Move to Finished, when it did not go.
    move_said: Option<String>,
    reading: bool,
    understood: Option<Result<Understood, String>>,
    /// At the stop before the merge, what its two ways out do, in the engine's words: the question, then Merge's and
    /// Branch only's sentences. Read once a question.
    merge_said: Option<(String, Vec<String>, Vec<String>)>,
    shrunk: HashSet<String>,
    /// The boxes of what the stop offers that the person set themselves (`Stop.offers`).
    ticks: crate::offers::Ticks,
    sending: bool,
    said: Option<String>,
    /// The answer just sent, and to which question: the foot of its run's tab says so for a few seconds.
    sent: Option<(Sent, String)>,
    /// The card's middle, which scrolls between its fixed head and foot.
    scroll: ScrollHandle,
    /// Whether the words field had the keys when the card was last drawn.
    words_had_keys: bool,
    /// The card itself, which takes the keys when the person leaves the words field (Esc): a second Esc folds it.
    focus: FocusHandle,
    /// The card's first control ("See the diff"): the keys go there when the pill opens the card, so Tab goes round it.
    first: FocusHandle,
    /// The tabs over the middle, so a test can open one (`tab <run>`, `tab main`).
    pub tabs: Option<Entity<crate::tabs::Tabs>>,
    /// The Changes panel, for a test that clicks a file in it.
    pub changes: Option<Entity<crate::changes::Changes>>,
}

/// Where the words field is among the card's middle's children (items, what was dropped, the words, the reading).
const WORDS_ROW: usize = 2;
/// Where what Jaynes read of the words comes.
const READ_ROW: usize = 3;

/// A verdict that sends the item back takes the person's words.
fn takes_words(v: VerdictKind) -> bool {
    asks(v).is_some()
}

/// What the field under an item sent back asks: how it should change, or what is missing in it.
fn asks(v: VerdictKind) -> Option<&'static str> {
    match v {
        VerdictKind::Change => Some("how should it change?"),
        VerdictKind::Redo => Some("what is missing?"),
        VerdictKind::Keep | VerdictKind::Drop | VerdictKind::Skip => None,
    }
}

/// The hint in the person's words field, in every stop's card.
pub const WORDS_HINT: &str = "Your words: say what you think, in any language (or answer with the buttons)";

/// The line over the foot when something lies ahead: a step that cannot be undone, whose answer asks for the
/// person's touch, or the merge, which asks them itself before it lands. None at a leave and at the merge itself.
pub fn ahead_line(stop: &Stop) -> Option<String> {
    let then = if crate::answer::touch_of(stop).is_some() { "Sending asks for your touch." } else { "You look at it before it lands." };
    stop.ahead.as_deref().map(|a| format!("Ahead: {a}. {then}"))
}

/// The Send button's words: with " · Touch ID" only where the answer asks for the touch.
pub fn send_label(stop: &Stop, sending: bool) -> String {
    match (crate::answer::touch_of(stop), sending) {
        (_, true) => "Sending…".to_string(),
        (Some(_), _) => "Send · Touch ID".to_string(),
        (None, _) => "Send".to_string(),
    }
}

/// The amber mark of a file in what goes in that runs later outside any box, the heading of the part that says how
/// each runs, the line under it, and the line under the merge's two ways out (design 0.2.5 permissao.md).
pub const RUNS_LATER: &str = "runs later outside any box";
pub const LATER_HEAD: &str = "Runs later outside any box";
pub const READ_THESE: &str = "Read these before you merge: they run on your machine, outside any box.";
pub const ONE_TOUCH: &str = "Merging asks for your touch, once.";
/// The button of each stop the merge's touch keeps: it opens that stop on the page, to answer it differently first.
pub const RIDE_OPEN: &str = "Open";

/// The stops still open that the merge's touch keeps, as its card names them before the touch (design 0.2.5
/// merge.md): the touch counts only for what it showed.
#[derive(Debug, PartialEq)]
pub struct RidesPart {
    pub head: &'static str,
    /// Each stop: its step in quotes, what it keeps as a sentence ("Built on Go, as your request says."), and its
    /// question, which Open opens.
    pub rows: Vec<(String, String, String)>,
    pub under: &'static str,
}

/// The part of the merge's card that names the stops its touch keeps; none when it keeps none, and never at another
/// stop.
pub fn rides_part(stop: &Stop) -> Option<RidesPart> {
    if stop.kind != QuestionKind::Merge || stop.rides.is_empty() {
        return None;
    }
    let sentence = |said: &str| {
        let said = said.trim().trim_end_matches('.');
        let mut letters = said.chars();
        letters.next().map(|first| format!("{}{}.", first.to_uppercase(), letters.as_str())).unwrap_or_default()
    };
    Some(RidesPart {
        head: "Your touch also keeps these",
        rows: stop.rides.iter().map(|r| (format!("“{}”", r.step), sentence(&r.said), r.question.clone())).collect(),
        under: "Merging keeps each as it is. To answer one differently, open it before you merge.",
    })
}

/// The line under the merge's two ways out: the touch once, and the answers it also keeps when it keeps any.
pub fn one_touch(stop: &Stop) -> String {
    match stop.rides.len() {
        _ if stop.kind != QuestionKind::Merge => ONE_TOUCH.to_string(),
        0 => ONE_TOUCH.to_string(),
        1 => format!("{ONE_TOUCH} It also keeps the answer above."),
        n => format!("{ONE_TOUCH} It also keeps the {n} answers above."),
    }
}

/// The most files the merge's stop lists; the rest are counted.
const LANDS_SHOWN: usize = 8;

/// What the merge lands, as its stop says it.
#[derive(Debug, Default, PartialEq)]
pub struct Landing {
    /// "What goes in · 7 files · +182 −40 lines".
    pub goes_in: String,
    /// Each file shown: its path, its lines ("+64 −12", "+88"), and whether it runs later outside any box.
    pub rows: Vec<(String, String, bool)>,
    /// "… and 4 more", past the eighth.
    pub more: Option<String>,
    /// Each file that runs later outside any box, and how, in plain words, as the card draws them: the amber part,
    /// empty when none does.
    pub later: Vec<(String, String)>,
    /// The amber sentence over what lands when a file of it changes how the tests run ("It changes how your tests run:
    /// pyproject.toml (its pytest settings change)."), and the dim one beneath it (`stop::TESTS_RUN_WHY`).
    pub tests_run: Option<(String, String)>,
    /// The files shown that change how the tests run, each labelled so in its own row.
    pub marked: Vec<String>,
}

/// The label at the end of a row whose file changes how the project's tests run.
pub const TESTS_RUN_MARK: &str = "changes how your tests run";

/// A file's lines, as the merge's stop says them: what it adds, what it removes, and only what is there.
fn lines_said(added: u64, removed: u64) -> String {
    match (added, removed) {
        (a, 0) => format!("+{a}"),
        (0, r) => format!("−{r}"),
        (a, r) => format!("+{a} −{r}"),
    }
}

/// What the merge's stop says lands on the person's branch, from what the engine gives (`Stop.lands`).
pub fn landing(stop: &Stop) -> Landing {
    let (added, removed) = stop.lands.iter().fold((0, 0), |(a, r), l| (a + l.added, r + l.removed));
    let n = stop.lands.len();
    let files = if n == 1 { "1 file".to_string() } else { format!("{n} files") };
    Landing {
        goes_in: format!("What goes in · {files} · +{added} −{removed} lines"),
        rows: stop.lands.iter().take(LANDS_SHOWN).map(|l| (l.path.clone(), lines_said(l.added, l.removed), l.runs.is_some())).collect(),
        more: (n > LANDS_SHOWN).then(|| format!("… and {} more", n - LANDS_SHOWN)),
        later: stop.lands.iter().filter_map(|l| l.runs.map(|r| (l.path.clone(), format!("{r}.")))).collect(),
        tests_run: stop.tests_run.clone().map(|said| (said, jaynes_shell::stop::TESTS_RUN_WHY.to_string())),
        marked: stop.lands.iter().take(LANDS_SHOWN).filter(|l| l.tests.is_some()).map(|l| l.path.clone()).collect(),
    }
}

/// The caret of every field: blue, where the kit's own near-black hid it on the dark.
pub fn caret() -> u32 {
    look::blue()
}

/// A text field's border and whether the soft blue ring shows round it: blue with the ring while it has the keys.
pub fn field_look(focused: bool) -> (u32, bool) {
    if focused { (look::blue(), true) } else { (look::line(), false) }
}

pub fn word(v: VerdictKind) -> &'static str {
    match v {
        VerdictKind::Keep => "keep",
        VerdictKind::Change => "change",
        VerdictKind::Redo => "redo",
        VerdictKind::Drop => "drop",
        VerdictKind::Skip => "skip",
    }
}

/// The soft blue ring round what has the keys: a field, or a control Tab reached.
fn focus_ring() -> Vec<BoxShadow> {
    let ring = rgba((look::blue() << 8) | if look::frame() == look::LINEN { 0x38 } else { 0x47 });
    vec![BoxShadow { color: ring.into(), offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(3.), inset: false }]
}

/// A control in a stop's Tab order: Tab and Shift-Tab reach it, Enter or Space press it, and the blue ring says it has
/// the keys. A press of the pointer leaves the keys where they are, so an Enter after a click is still the screen's.
/// The ring is a shadow, painted under the control: one without a fill of its own takes `fill` while it has the keys.
pub fn tabbed_with(d: Stateful<Div>, fill: Option<u32>) -> Stateful<Div> {
    d.tab_index(0).on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default()).focus_visible(move |s| {
        let s = s.border_color(rgb(look::blue())).shadow(focus_ring());
        match fill {
            Some(f) => s.bg(rgb(f)),
            None => s,
        }
    })
}

/// A control with a fill of its own in a stop's Tab order (`tabbed_with`).
pub fn tabbed(d: Stateful<Div>) -> Stateful<Div> {
    tabbed_with(d, None)
}

/// A text field that shows when it has the keys, in every field of the app: the border blue with a soft blue ring
/// round it and a blue caret (the kit's own caret took the kit's light theme's near-black, which hid it on the dark),
/// the I-beam over the whole box, and a press anywhere in the box gives it the keys with the caret after the text; a
/// press on the text itself puts the caret there, as any field does.
pub fn focus_field(id: impl Into<ElementId>, input: &Entity<InputState>, window: &Window, cx: &mut App) -> Stateful<Div> {
    input.update(cx, |s, _| s.set_editor_style(InputEditorStyle { caret: rgb(caret()).into(), ..InputEditorStyle::default() }));
    let (border, ring) = field_look(input.read(cx).focus_handle(cx).is_focused(window));
    let take = input.clone();
    div()
        .id(id)
        .h(px(34.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(px(8.))
        .bg(rgb(look::panel()))
        .border_1()
        .border_color(rgb(border))
        .when(ring, |d| d.shadow(focus_ring()))
        .cursor(CursorStyle::IBeam)
        .text_size(px(13.))
        .text_color(rgb(look::text()))
        // before the screen behind it takes the keys, as a press on what holds no field does
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.prevent_default();
            take.update(cx, |s, cx| {
                if !s.focus_handle(cx).is_focused(window) {
                    s.set_cursor_position(Position::new(0, u32::MAX), window, cx);
                    s.focus(window, cx);
                }
            });
        })
        .child(Input::new(input))
}

/// The card's first button for a leave: what the agent wants to write, in place of the step's changes, which are not
/// there yet.
pub const WANTS_BUTTON: &str = "See what it wants to write";

/// What the card's first button opens, by its words: the step's changes, or for a leave what the agent wants to
/// write; a leave with nothing to write (read, reach, run) has none.
pub fn diff_button(stop: &Stop) -> Option<&'static str> {
    if stop.permission {
        stop.wants.as_ref().map(|_| WANTS_BUTTON)
    } else {
        Some("See the diff")
    }
}

/// A leave's answers, each with what it does beside it, in the engine's sentence ("Allow", "It writes the file and
/// goes on.").
pub fn leave_rows(stop: &Stop) -> Vec<(&str, &str)> {
    stop.offered.iter().map(|v| (stop.label(*v), stop.says(*v))).collect()
}

/// What the agent wants to write, as the diff view draws it: its title, and the engine's lines (`stop::wants_lines`)
/// marked as a diff's, a gap as "@@ ⋯".
pub fn wants_view(w: &stop::Wants) -> (String, String) {
    let lines: Vec<String> = jaynes_shell::stop::wants_lines(w.before.as_deref(), &w.after)
        .into_iter()
        .map(|(k, l)| if k == '⋯' { "@@ ⋯".to_string() } else { format!("{k}{l}") })
        .collect();
    (format!("What the agent wants to write in {} · not written yet", w.file), lines.join("\n"))
}

pub fn button(id: String, text: &str, on: bool, color: u32) -> Stateful<Div> {
    // one that is on has its colour's fill; one that is off is clear, and takes the window's while it has the keys
    tabbed_with(div().id(SharedString::from(id)), (!on).then(look::panel))
        .px(px(10.))
        .py(px(4.))
        .rounded(px(7.))
        .border_1()
        .text_size(px(12.5))
        .cursor_pointer()
        .flex_none()
        .when(on, |d| d.bg(rgb(color)).border_color(rgb(color)).text_color(rgb(look::on_fill())))
        .when(!on, |d| d.border_color(rgb(look::line_2())).text_color(rgb(look::dim())).hover(|d| d.border_color(rgb(color)).text_color(rgb(look::text()))))
        .child(text.to_string())
}

pub fn primary(id: &str, text: String, can: bool) -> Stateful<Div> {
    let d = div().id(SharedString::from(id.to_string()));
    // only a button that can go is in the Tab order
    if can { tabbed(d) } else { d }
        .px(px(14.))
        .py(px(7.))
        .rounded(px(8.))
        .text_size(px(13.))
        .flex_none()
        .when(can, |d| d.bg(rgb(look::blue())).text_color(rgb(look::on_fill())).cursor_pointer())
        .when(!can, |d| d.bg(rgb(look::line())).text_color(rgb(look::faint())))
        .child(text)
}

fn colour_of(v: VerdictKind) -> u32 {
    match v {
        VerdictKind::Keep => look::green(),
        VerdictKind::Drop => look::red(),
        VerdictKind::Change | VerdictKind::Redo => look::amber(),
        VerdictKind::Skip => look::grey(),
    }
}

pub fn sentences(list: &[String]) -> Div {
    let mut d = div().flex().flex_col().gap(px(3.));
    for s in list {
        d = d.child(div().text_size(px(12.5)).text_color(rgb(look::dim())).child(s.clone()));
    }
    d
}

/// How the stop shows over what the middle shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shows {
    Card,
    Pill,
}

/// The stop on a run's tab: its card, or its pill when the person folded it.
///
/// Over Claude Code's terminal, only the pill: the card never covers where the person types.
pub fn shows(on_run_tab: bool, folded: bool) -> Shows {
    if on_run_tab && !folded { Shows::Card } else { Shows::Pill }
}

/// Whether Esc folds the stop: the card open takes it before the terminal under it ever sees it, unless something
/// over it (the diff, Command-K, the shortcuts) is there to close first; a pill lets it go on.
pub fn takes_esc(shows: Shows, over: bool) -> bool {
    shows == Shows::Card && !over
}

/// The card's first line, the run's name, and the one after it: where it waits.
/// A question's title as the stop card and the choice screen show it: its project first, dim, with " · ", then the
/// question; the question alone without a project (design 0.2.5 quatro-itens.md, item 2).
pub fn question_title(project: &str, title: &str) -> (Option<String>, String) {
    ((!project.is_empty()).then(|| format!("{project} · ")), title.to_string())
}

/// The title drawn: the project in dim before the question, the question's names in mono.
pub fn title_view(project: &str, title: &str) -> Div {
    let (first, question) = question_title(project, title);
    div().flex().flex_wrap().items_baseline().when_some(first, |d, p| d.child(div().flex_none().whitespace_nowrap().text_color(rgb(look::dim())).child(p))).child(look::ticked(&question))
}

pub fn card_head(run: &str, project: &str, step: &str, round: u32) -> (String, String) {
    let at = if round > 0 { format!("at “{step}” · round {round}") } else { format!("at “{step}”") };
    if run.is_empty() {
        return (project.to_string(), at);
    }
    let under = if project.is_empty() { at } else { format!("{project} · {at}") };
    (run.to_string(), under)
}

/// A stop of a run whose older engine left the Mac: what the card says in place of its question, in the design-lead's
/// words (22/11); under it, dim, [`GONE_HOW`]; the engine's own sentence is in Details, closed.
pub fn gone_head(gone: &stop::Gone) -> String {
    format!("This run can't go on: it started on Jaynes {}, which is no longer on this Mac.", gone.version)
}

/// What the person does about a run whose engine left: no other way out exists without that engine.
pub const GONE_HOW: &str = "No answer reaches it, Halt included. To do this work, start it again: a new run starts on this Jaynes.";

/// The question's place on a card of a run whose engine left: what can't go on, what to do, the engine's words folded
/// in Details, which `toggle` opens; where what it made is, the place in mono with a Copy beside it; then, not moved
/// yet, Move to Finished (neutral, `mover`) with what it does under it, and once moved, when (motor-velho.md, 22/11).
/// `said` is what came of the last press, when it did not go.
pub fn gone_block(gone: &stop::Gone, open: bool, toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static, mover: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static, said: Option<&str>) -> Div {
    let dim = |t: String| div().text_size(px(12.5)).text_color(rgb(look::dim())).child(t);
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(div().text_size(px(16.)).text_color(rgb(look::text())).child(gone_head(gone)))
        .child(dim(GONE_HOW.into()))
        .when_some(gone.place.as_ref(), |d, place| {
            let (before, at, after) = place.parts();
            let copied = at.clone();
            d.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .text_size(px(12.5))
                    .text_color(rgb(look::dim()))
                    // the sentence's parts touch, as one line: only the Copy stands apart
                    .child(before)
                    .child(div().font_family(look::MONO_FONT).text_size(px(12.)).text_color(rgb(look::text())).child(at))
                    .when(!after.is_empty(), |d| d.child(after))
                    .child(div().w(px(8.)))
                    .child(button("gone-copy".into(), "Copy", false, look::grey()).on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copied.clone())))),
            )
        })
        .when(gone.moved.is_none(), |d| {
            d.child(div().pt(px(6.)).flex().child(button("move-to-finished".into(), jaynes_shell::listing::MOVE_TO_FINISHED, false, look::grey()).on_click(mover)))
                .child(div().text_size(px(12.)).text_color(rgb(look::dim())).child(jaynes_shell::listing::MOVE_SAYS))
                .when_some(said, |d, s| d.child(div().text_size(px(12.)).text_color(rgb(look::red())).child(s.to_string())))
        })
        .when_some(gone.moved, |d, at| d.child(dim(jaynes_shell::moved::moved_on(at))))
        .child(div().id("gone-details").cursor_pointer().text_size(px(12.)).text_color(rgb(look::faint())).child(if open { "▾ Details" } else { "▸ Details" }).on_click(toggle))
        .when(open, |d| d.child(div().text_size(px(12.)).text_color(rgb(look::faint())).child(gone.said.clone())))
}

/// The line of a choice's invite when its run's engine left: no amber, what can't go on (the design-lead, 22/11).
pub fn gone_invite(gone: &stop::Gone) -> String {
    format!("◆ This run can't go on: it started on Jaynes {}.", gone.version)
}

/// Whether the card takes answers: not on a run whose engine left the Mac, where it keeps its head, the two lines,
/// Details, Esc and See the diff, and nothing that answers (the design-lead, 22/11).
pub fn takes_answers(stop: &Stop) -> bool {
    stop.gone.is_none()
}

/// Whether an answer may go: every item judged (or something brought back), nothing refused, not already on its way,
/// and its run's engine still here to take it.
pub fn may_send(stop: &Stop, judged: usize, brought: bool, sending: bool, refused: bool) -> bool {
    (judged == stop.items.len() || brought) && !sending && !refused && takes_answers(stop)
}

/// The stop folded to one line: which run asks, and what.
pub fn pill(run: &str, question: &str) -> String {
    if run.is_empty() { format!("◆ {question} · open") } else { format!("◆ {run}: {question} · open") }
}

/// Whether Tab goes round the stop: its card, or its pill folded on its run's own tab (Enter or Space open it); over
/// Claude Code's terminal the pill leaves Tab to the terminal.
pub fn tab_goes_round(shows: Option<Shows>, on_tab: bool) -> bool {
    match shows {
        Some(Shows::Card) => true,
        Some(Shows::Pill) => on_tab,
        None => false,
    }
}

/// Whether a Tab goes into the open card at its first control: the keys are outside it (the run's tab, the column),
/// where gpui would send them to the window's first stop, off the card. Shift-Tab goes back from where it is.
pub fn enters_card(shows: Option<Shows>, inside: bool, shift: bool) -> bool {
    shows == Some(Shows::Card) && !inside && !shift
}

/// The chip at the card's head: with the words field in focus, Esc leaves the field first, as in the compare mode.
pub fn esc_chip(field: bool) -> &'static str {
    if field { "Esc · leave the field" } else { "Esc · later" }
}

/// The line for the items not judged: kept as they are when Send would take them so, else not judged yet.
pub fn not_judged(n: usize, kept: bool) -> String {
    if kept { format!("{n} not judged: kept as they are") } else { format!("{n} not judged yet") }
}

/// The foot's line beside Send: what was said of the last send, else the count of judged items, and, once something
/// is marked, why the engine would refuse the answer as marked, since that is what keeps Send off. True when it
/// reads as that refusal (in red).
pub fn foot_line(said: Option<&str>, judged: usize, total: usize, refused: Option<&str>) -> (String, bool) {
    if let Some(s) = said {
        return (s.to_string(), false);
    }
    let count = format!("{judged} of {total} judged");
    match refused.filter(|_| judged > 0) {
        Some(r) => (format!("{count} · Jaynes would refuse this: {r}"), true),
        None => (count, false),
    }
}

/// "What this does" at the card: nothing before the first mark (or something brought back that Send would take);
/// then the sentence of each marked item, one a line, what else the answer does, and one line for the rest. The
/// engine says one sentence an item, in their order, before the rest; and counts an item without a mark as kept, so
/// what is not judged yet is counted here, by the marks.
pub fn said_lines(said: &[String], marked: &[bool], sends_unmarked: bool) -> Vec<String> {
    if !marked.contains(&true) && !sends_unmarked {
        return Vec::new();
    }
    let n = marked.len();
    let mut out: Vec<String> = if said.len() >= n {
        said.iter().zip(marked).filter(|(_, m)| **m).map(|(s, _)| s.clone()).chain(said[n..].iter().cloned()).collect()
    } else {
        said.to_vec()
    };
    let rest = marked.iter().filter(|m| !**m).count();
    if rest > 0 {
        out.push(not_judged(rest, sends_unmarked));
    }
    out
}

/// Under the card's title, for a run already delivered.
pub const DELIVERED: &str = "Already delivered. Your answer can still correct it.";

/// The title over the change a step made.
pub fn diff_title(step: &str) -> String {
    format!("Changes made at “{step}”")
}

/// The heading over what a merge lands, as the page says it: "What lands on “main” · 3 files".
pub fn lands_title(into: &str, files: usize) -> String {
    let on = if into.is_empty() { "your branch".to_string() } else { format!("“{into}”") };
    format!("What lands on {on} · {files} file{}", if files == 1 { "" } else { "s" })
}

/// What See the changes shows, with its heading: at the merge, everything that lands (`lands`, the delivery's diff,
/// every file as the last step that changed it left it); at any other stop, the step's own change (`step`).
pub fn diff_shown(stop: &Stop, step: Option<String>, lands: Option<String>) -> (String, String) {
    if stop.kind == QuestionKind::Merge {
        let text = lands.filter(|t| !t.trim().is_empty());
        let files = text.as_deref().map_or(0, |t| t.lines().filter(|l| l.starts_with("diff --git ")).count());
        return (lands_title(&stop.into, files), text.unwrap_or_else(|| "Nothing lands: the run's copy is gone or changed nothing.".to_string()));
    }
    let text = step.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| "This step changed nothing in the project.".to_string());
    (diff_title(&stop.step), text)
}

/// What the foot of the card says once the answer went.
pub const SENT: &str = "Sent as yours. The run goes on.";
/// How long the foot of the run's tab says it after the stop closed.
pub const SENT_FOR: Duration = Duration::from_secs(4);

/// The answer just sent to a run's stop, and when.
#[derive(Clone, Debug)]
pub struct Sent {
    pub run: String,
    pub at: Instant,
    /// Where the answer went and what comes next, as the pill says it ([`sent_said`]).
    pub said: String,
}

/// What the pill says after a Send, as the page's line does (design 0.2.5 quatro-itens.md, item 3): the project the
/// answer went to and the next question with its project, the projects only when they differ; that nothing else
/// waits, when nothing does. `next` is a project ("" without one) and the question it asks.
pub fn sent_said(from: Option<&str>, next: Option<(&str, &str)>) -> String {
    let Some((to, asks)) = next else { return "Sent. Nothing else waits for you.".to_string() };
    match (from.filter(|f| !f.is_empty()), Some(to).filter(|t| !t.is_empty())) {
        (Some(from), Some(to)) if from != to => format!("Sent to {from}. Next: {to} · {asks}"),
        (None, Some(to)) => format!("Sent. Next: {to} · {asks}"),
        _ => format!("Sent. Next: {asks}"),
    }
}

/// What comes after a Send to `sent`: the first other run that needs the person, as the column lists them, with its
/// project ("" for a run without one) and what it asks.
pub fn next_after<'a>(needs: &'a [jaynes_shell::listing::Row], sent: &str) -> Option<(&'a str, &'a str)> {
    needs.iter().find(|r| r.id != sent).map(|r| (if r.project == "no project" { "" } else { r.project.as_str() }, r.line.as_str()))
}

impl Sent {
    /// Whether the foot of the tab showing `run` says it at `now`.
    pub fn shows(&self, run: Option<&str>, now: Instant) -> bool {
        run == Some(self.run.as_str()) && now.saturating_duration_since(self.at) < SENT_FOR
    }
}

/// Whether what a field said reads the person's words.
pub fn reads_on(e: &InputEvent) -> bool {
    matches!(e, InputEvent::PressEnter { secondary: false, .. })
}

/// How tall the card may grow in a window this tall: up to most of it, so a stop with many items or long words shows
/// them without scrolling, while the run's story keeps a few lines over it; never less than the card had before.
pub fn card_cap(window_height: Pixels) -> Pixels {
    (window_height * 0.7).max(px(460.))
}

/// How tall the fade over the bottom of the card's middle is, while more is below.
const FADE: f32 = 32.;
/// The padding under the middle's last row.
const MIDDLE_PAD: f32 = 12.;

/// Where the middle scrolls to when the words field takes the keys: as little as brings its row whole into view, above
/// the fade when more is below it, its top first when it is taller than that room. `view` is the middle, `row` the
/// field's row as laid out (before scrolling), `max` how far the middle scrolls, `now` its offset (zero or less).
/// None before the card was laid out once.
pub fn words_offset(view: Bounds<Pixels>, row: Option<Bounds<Pixels>>, max: Pixels, now: Pixels) -> Option<Pixels> {
    let row = row?;
    let h = f32::from(view.size.height);
    if h <= 0. {
        return None;
    }
    let (top, bottom) = (f32::from(row.top() - view.top()), f32::from(row.bottom() - view.top()));
    let mut down = -f32::from(now);
    if bottom + FADE > down + h {
        down = bottom + FADE - h;
    }
    if top < down {
        down = top;
    }
    Some(px(-down.clamp(0., f32::from(max).max(0.))))
}

/// Whether the fade shows at the bottom of the middle: only while content is below the view, not only its padding.
pub fn fades_below(max: Pixels, offset: Pixels) -> bool {
    let (max, down) = (f32::from(max), -f32::from(offset));
    max > 1. && down < max - MIDDLE_PAD - 1.
}

/// Whether the words field comes whole into view now: when it has just taken the keys, never while it keeps them,
/// so the person may scroll away from it to read.
pub fn comes_into_view(had_keys: bool, has_keys: bool) -> bool {
    has_keys && !had_keys
}

impl Island {
    pub fn new(runs: Entity<Runs>, key: Option<Arc<Key>>, lightbox: Entity<Lightbox>, compare: Entity<Compare>, cx: &mut Context<Self>) -> Self {
        cx.observe(&runs, |this, _, cx| {
            this.follow(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&compare, |_, _, cx| cx.notify()).detach();
        // an answer the compare mode sent: its run's tab says so after the mode closes
        cx.subscribe(&compare, |this, _, e: &crate::compare::SentFrom, cx| {
            let question = this.stop(cx).filter(|s| s.run == e.0).map(|s| s.question).unwrap_or_default();
            this.said_sent(e.0.clone(), question, cx);
        })
        .detach();
        // only in a build for tests: the driver acts as the person (testing.rs)
        #[cfg(feature = "test-driver")]
        if let Some(file) = crate::testing::var("JAYNES_APP_TEST_DO") {
            drive_from(PathBuf::from(file), cx);
        }
        Island {
            ticks: Default::default(),
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
            gone_open: false,
            move_said: None,
            reading: false,
            understood: None,
            merge_said: None,
            shrunk: HashSet::new(),
            sending: false,
            said: None,
            sent: None,
            scroll: ScrollHandle::new(),
            words_had_keys: false,
            focus: cx.focus_handle(),
            first: cx.focus_handle(),
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
            self.compare_open(stop, window, cx);
        }
        cx.notify();
    }

    /// The compare mode on `stop`, with its run's name.
    fn compare_open(&self, stop: Stop, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.run_name(&stop.run, cx);
        let project = self.runs.read(cx).snap.details.get(&stop.run).map(|d| d.project.clone()).unwrap_or_default();
        self.compare.update(cx, |c, cx| c.open(stop, name, project, window, cx));
    }

    /// A run's name, as the column says it; empty before the record is read.
    fn run_name(&self, run: &str, cx: &App) -> String {
        self.runs.read(cx).snap.details.get(run).map(|d| d.title.clone()).unwrap_or_default()
    }

    /// The run whose tab shows in the middle, if a run's does.
    fn tab_run(&self, cx: &App) -> Option<String> {
        self.tabs.as_ref().and_then(|t| t.read(cx).middle().run().map(str::to_string))
    }

    /// How the picked run's stop shows now: the card on its run's tab, else its pill; none while the compare mode
    /// shows it, or with no stop.
    pub fn showing(&self, cx: &App) -> Option<Shows> {
        let stop = self.stop(cx)?;
        if self.compare.read(cx).is_open() {
            return None;
        }
        Some(shows(self.tab_run(cx).as_deref() == Some(stop.run.as_str()), self.shrunk.contains(&stop.question)))
    }

    /// Esc with the card open and nothing over it: the card folds to its pill, and the terminal never sees the key.
    pub fn takes_esc_now(&self, over: bool, cx: &App) -> bool {
        self.showing(cx).is_some_and(|s| takes_esc(s, over))
    }

    pub fn fold(&mut self, cx: &mut Context<Self>) {
        if let Some(stop) = self.stop(cx) {
            self.shrunk.insert(stop.question);
            cx.notify();
        }
    }

    /// Whether Tab goes round a stop's buttons now: the card's, or the compare mode's while no picture shows large.
    pub fn takes_tab(&self, cx: &App) -> bool {
        let compare = self.compare.read(cx);
        let on_tab = self.stop(cx).is_some_and(|s| self.tab_run(cx).as_deref() == Some(s.run.as_str()));
        (compare.is_open() && !compare.viewing()) || tab_goes_round(self.showing(cx), on_tab)
    }

    /// A Tab with the keys outside the open card: they go to its first control ("See the diff"), and true; else false,
    /// and Tab goes round as the window's order says.
    pub fn tab_into(&mut self, shift: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let inside = self.focus.contains_focused(window, cx);
        if enters_card(self.showing(cx), inside, shift) {
            window.focus(&self.first, cx);
            return true;
        }
        false
    }

    /// Whether the person's words field has the keys: Esc then leaves the field before it folds the card.
    pub fn words_focused(&self, window: &Window, cx: &App) -> bool {
        self.words.as_ref().is_some_and(|w| w.read(cx).focus_handle(cx).is_focused(window))
    }

    /// Esc with the words field in focus: the card takes the keys, and a second Esc folds it.
    pub fn leave_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// The answer just went to `run`'s `question`: the foot of its tab says so, for a few seconds.
    fn said_sent(&mut self, run: String, question: String, cx: &mut Context<Self>) {
        let snap = &self.runs.read(cx).snap;
        let from = snap.details.get(&run).map(|d| d.project.as_str()).filter(|p| *p != "no project");
        let said = sent_said(from, next_after(&snap.listing.needs, &run));
        self.sent = Some((Sent { run, at: Instant::now(), said }, question));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SENT_FOR + Duration::from_millis(50)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
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
        cx.subscribe(&input, |this, field, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                this.reckon(cx);
            }
            // Enter in the person's words reads them, as "Read my words" does
            if reads_on(e) && this.words.as_ref() == Some(&field) {
                this.read_words(cx);
            }
            // the field took or let go the keys: its ring follows
            cx.notify();
        })
        .detach();
        input
    }

    fn mark(&mut self, item: String, v: VerdictKind, window: &mut Window, cx: &mut Context<Self>) {
        self.marks.insert(item.clone(), v);
        if let Some(hint) = asks(v)
            && !self.notes.contains_key(&item)
        {
            let input = self.input(hint, window, cx);
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
        // down to where the reading comes, so it never arrives out of view
        self.scroll.scroll_to_item(READ_ROW);
        cx.notify();
        let read = cx.background_spawn(async move { crate::signals::open(|| answer::understand(&stop, &words, None)) });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            let _ = this.update(cx, |this, cx| {
                this.reading = false;
                this.understood = Some(read);
                this.scroll.scroll_to_item(READ_ROW);
                cx.notify();
            });
        })
        .detach();
    }

    /// What came of the last Move to Finished, when it did not go; and whether the engine's words show under Details.
    pub fn gone_said(&self) -> (Option<String>, bool) {
        (self.move_said.clone(), self.gone_open)
    }

    pub fn flip_gone(&mut self, cx: &mut Context<Self>) {
        self.gone_open = !self.gone_open;
        cx.notify();
    }

    /// Move to Finished on the card of a run whose engine left: signed with the person's key, as the column's Move all.
    pub fn move_to_finished(&mut self, run: String, cx: &mut Context<Self>) {
        let moved = answer::move_to_finished(self.key.clone(), vec![run]);
        cx.spawn(async move |this, cx| {
            let moved = moved.await;
            let _ = this.update(cx, |this, cx| {
                this.move_said = moved.err().map(|e| format!("Not moved: {e}"));
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
        let grants = self.ticks.grants(&stop, None);
        let sent = answer::send(self.key.clone(), answer::touch_of(&stop).map(str::to_string), grants, body);
        cx.spawn(async move |this, cx| {
            let sent = sent.await;
            let _ = this.update(cx, |this, cx| {
                this.sending = false;
                match sent {
                    Ok(()) => {
                        this.said = Some(SENT.into());
                        this.said_sent(stop.run.clone(), stop.question.clone(), cx);
                    }
                    Err(e) => this.said = Some(format!("Not sent: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Whether the answer as marked may go: every item judged (or something brought back), nothing refused, not
    /// already on its way.
    fn can_send(&self, stop: &Stop) -> bool {
        let judged = stop.items.iter().filter(|i| self.marks.contains_key(&i.id)).count();
        may_send(stop, judged, !self.bring.is_empty(), self.sending, self.reckoned.refused.is_some())
    }

    /// The stop the island shows open as its card (not folded away, not only a pill): its run and question.
    pub fn stop_shown(&self, cx: &App) -> Option<(String, String)> {
        self.stop(cx).filter(|_| self.showing(cx) == Some(Shows::Card)).map(|s| (s.run, s.question))
    }

    /// Command-Return: sends the answer, when the question shows open and the answer may go.
    pub fn send_if_ready(&mut self, cx: &mut Context<Self>) {
        let Some(stop) = self.stop(cx) else { return };
        if stop.choose || self.showing(cx) != Some(Shows::Card) || !self.can_send(&stop) {
            return;
        }
        self.send(cx);
    }

    /// The merge's answer, one for every file: Merge (keep) or Branch only (drop), with the touch once.
    fn send_all(&mut self, v: VerdictKind, cx: &mut Context<Self>) {
        let Some(stop) = self.stop(cx) else { return };
        if self.sending {
            return;
        }
        self.send_body(answer::all(&stop, v), cx);
    }

    /// What the merge's two ways out do, in the engine's words, read once for its question.
    fn merge_said(&mut self, stop: &Stop) -> (Vec<String>, Vec<String>) {
        if self.merge_said.as_ref().is_none_or(|(q, _, _)| *q != stop.question) {
            let said = |v| answer::reckon(&answer::all(stop, v)).sentences;
            self.merge_said = Some((stop.question.clone(), said(VerdictKind::Keep), said(VerdictKind::Drop)));
        }
        self.merge_said.clone().map(|(_, k, d)| (k, d)).unwrap_or_default()
    }

    /// The stop before the merge: what goes in, each file with its lines, and the files that run later outside any
    /// box, each with how, in amber.
    fn landing_view(&self, stop: &Stop, cx: &mut Context<Self>) -> Div {
        let l = landing(stop);
        let diff_stop = stop.clone();
        let mut rows = div().flex().flex_col().gap(px(3.)).font_family(look::MONO_FONT).text_size(px(12.));
        for (path, lines, later) in &l.rows {
            let marked = l.marked.contains(path);
            rows = rows.child(
                div()
                    .flex()
                    .gap(px(12.))
                    .child(div().flex_1().min_w_0().truncate().text_color(rgb(look::text())).child(path.clone()))
                    .child(div().flex_none().text_color(rgb(look::dim())).child(lines.clone()))
                    .when(marked, |d| d.child(div().flex_none().font_family(look::UI_FONT).text_color(rgb(look::amber())).child(TESTS_RUN_MARK)))
                    .when(*later, |d| d.child(div().flex_none().font_family(look::UI_FONT).text_color(rgb(look::amber())).child(RUNS_LATER))),
            );
        }
        let mut view = div().flex().flex_col().gap(px(10.));
        if let Some(part) = rides_part(stop) {
            // the stops the touch keeps, before what lands: each with Open, which opens it on the page to answer first
            let mut rides = div().flex().flex_col().gap(px(4.)).child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::text())).child(part.head));
            for (i, (step, said, question)) in part.rows.into_iter().enumerate() {
                let run = stop.run.clone();
                rides = rides.child(
                    div()
                        .flex()
                        .gap(px(12.))
                        .items_center()
                        .py(px(4.))
                        .border_t_1()
                        .border_color(rgb(look::line()))
                        .text_size(px(12.5))
                        .child(div().flex_none().text_color(rgb(look::dim())).child(step))
                        .child(div().flex_1().min_w_0().text_color(rgb(look::text())).child(said))
                        .child(button(format!("ride-{i}"), RIDE_OPEN, false, look::text()).on_click(move |_, _, cx| {
                            let (run, question) = (run.clone(), question.clone());
                            cx.background_executor()
                                .spawn(async move {
                                    if let Err(why) = crate::open_page::stop(&run, &question) {
                                        tracing::warn!("the stop did not open on the page: {why}");
                                    }
                                })
                                .detach();
                        })),
                );
            }
            view = view.child(rides.child(div().pt(px(2.)).text_size(px(12.)).text_color(rgb(look::dim())).child(part.under)));
        }
        let mut view = view
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .items_center()
                    .child(div().flex_1().min_w_0().text_size(px(12.5)).text_color(rgb(look::dim())).child(l.goes_in.clone()))
                    .child(button("changes".into(), "See the changes", false, look::blue()).on_click(cx.listener(move |this, _, window, cx| this.diff(&diff_stop, window, cx)))),
            )
            .when_some(l.tests_run.clone(), |d, (said, why)| {
                d.child(div().flex().flex_col().gap(px(2.)).child(div().text_size(px(12.5)).text_color(rgb(look::amber())).child(said)).child(div().text_size(px(12.)).text_color(rgb(look::dim())).child(why)))
            })
            .child(rows.when_some(l.more.clone(), |d, m| d.child(div().font_family(look::UI_FONT).text_color(rgb(look::faint())).child(m))));
        if !l.later.is_empty() {
            let mut later = div().flex().flex_col().gap(px(4.)).px(px(12.)).py(px(10.)).rounded(px(8.)).border_1().border_color(rgba((look::amber() << 8) | 0x73)).child(
                div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::amber())).child(LATER_HEAD),
            );
            for (path, how) in &l.later {
                later = later.child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .text_size(px(12.5))
                        .child(div().flex_none().font_family(look::MONO_FONT).text_size(px(12.)).text_color(rgb(look::text())).child(path.clone()))
                        .child(div().text_color(rgb(look::dim())).child(how.clone())),
                );
            }
            view = view.child(later.child(div().pt(px(2.)).text_size(px(12.)).text_color(rgb(look::amber())).child(READ_THESE)));
        }
        view
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
        if let Some(w) = stop.wants.as_ref().filter(|_| stop.permission) {
            let (title, text) = wants_view(w);
            self.lightbox.update(cx, |l, cx| l.show(Shown::Diff { title, text }, window, cx));
            return;
        }
        let home = jaynes_shell::home();
        let (title, text) = if stop.kind == QuestionKind::Merge {
            let lands = crate::run_actions::read(&stop.run).and_then(|run| jaynes_shell::drive::delivery_diff(&home, &run));
            diff_shown(stop, None, lands)
        } else {
            diff_shown(stop, jaynes_shell::drive::step_diff(&home, &stop.run, &stop.step), None)
        };
        self.lightbox.update(cx, |l, cx| l.show(Shown::Diff { title, text }, window, cx));
    }

    /// A leave's answers, Allow and Refuse, each with what it does beside it ([`leave_rows`]); its item says no more
    /// than the title does.
    fn leave_view(&self, it: &stop::Item, stop: &Stop, cx: &mut Context<Self>) -> Div {
        let mut rows = div().flex().flex_col().gap(px(8.));
        for (v, (label, says)) in stop.offered.iter().zip(leave_rows(stop)) {
            let (item, v) = (it.id.clone(), *v);
            let on = self.marks.get(&it.id) == Some(&v);
            rows = rows.child(
                div()
                    .flex()
                    .gap(px(12.))
                    .items_center()
                    .child(button(format!("v-{}-{v:?}", it.id), label, on, colour_of(v)).min_w(px(76.)).flex().justify_center().on_click(cx.listener(move |this, _, window, cx| this.mark(item.clone(), v, window, cx))))
                    .child(div().min_w_0().text_size(px(12.)).text_color(rgb(look::faint())).child(says.to_string())),
            );
        }
        rows
    }

    fn item_view(&self, it: &stop::Item, stop: &Stop, back: bool, window: &Window, cx: &mut Context<Self>) -> Div {
        let mut body = div().flex().flex_col().gap(px(4.)).min_w_0().flex_1().child(div().text_size(px(14.)).text_color(rgb(look::text())).child(it.label.clone()));
        if !it.detail.is_empty() {
            body = body.child(div().text_size(px(12.5)).text_color(rgb(look::dim())).child(it.detail.clone()));
        }
        for (label, text) in &it.aspects {
            body = body.child(div().flex().gap(px(6.)).text_size(px(12.)).child(div().flex_none().text_color(rgb(look::faint())).child(label.clone())).child(div().text_color(rgb(look::dim())).child(text.clone())));
        }
        if !it.pictures.is_empty() {
            let mut pics = div().flex().gap(px(8.));
            for (n, p) in it.pictures.iter().take(4).enumerate() {
                let (path, title) = (p.clone(), it.label.clone());
                pics = pics.child(
                    tabbed(div().id(SharedString::from(format!("pic-{}-{n}", it.id))))
                        .cursor_pointer()
                        .rounded(px(6.))
                        .overflow_hidden()
                        .border_1()
                        .border_color(rgb(look::line()))
                        .on_click(cx.listener(move |this, _, window, cx| this.picture(title.clone(), path.clone(), window, cx)))
                        .child(img(p.clone()).h(px(110.))),
                );
            }
            body = body.child(pics);
        }
        let mut controls = div().flex().gap(px(6.)).flex_none();
        if back {
            let id = it.id.clone();
            controls = controls.child(button(format!("bring-{}", it.id), "Bring back", self.bring.contains(&it.id), look::amber()).on_click(cx.listener(move |this, _, window, cx| this.toggle_bring(id.clone(), window, cx))));
        } else {
            for v in &stop.offered {
                let (item, v) = (it.id.clone(), *v);
                let on = self.marks.get(&it.id) == Some(&v);
                controls = controls.child(button(format!("v-{}-{v:?}", it.id), stop::label_for(v, stop.kind), on, colour_of(v)).on_click(cx.listener(move |this, _, window, cx| this.mark(item.clone(), v, window, cx))));
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
            .bg(rgb(look::panel()))
            .border_1()
            .border_color(rgb(look::line()))
            .child(div().flex().gap(px(12.)).items_start().child(body).child(controls))
            .when_some(words, |d, w| d.child(focus_field(SharedString::from(format!("{}-{}", if back { "bring-words" } else { "note" }, it.id)), &w, window, cx)))
    }

    /// A stop that chooses among designs: an invitation to compare them over the whole window, with its run's name.
    fn invite(&self, stop: &Stop, cx: &mut Context<Self>) -> Div {
        let open = {
            let stop = stop.clone();
            cx.listener(move |this, _, window, cx| this.compare_open(stop.clone(), window, cx))
        };
        let q = stop.question.clone();
        let name = self.run_name(&stop.run, cx);
        // one option is opened, not compared
        let (line, open_word) = crate::compare::invite(stop.items.len(), &stop.title);
        // a run whose engine left: grey, what can't go on, and Open secondary (it still opens the choice, to look)
        let gone = stop.gone.as_ref();
        let line = gone.map(gone_invite).unwrap_or(line);
        let mark = if gone.is_some() { look::grey() } else { look::amber() };
        div()
            .flex()
            .gap(px(12.))
            .items_center()
            .px(px(16.))
            .py(px(12.))
            .rounded(px(12.))
            .bg(rgb(look::panel_2()))
            .border_1()
            .border_color(rgba((mark << 8) | 0x73))
            .child(div().size(px(8.)).flex_none().rounded_full().bg(rgb(mark)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().truncate().text_size(px(13.)).text_color(rgb(look::text())).child(line))
                    .when(!name.is_empty(), |d| d.child(div().truncate().text_size(px(12.)).text_color(rgb(if gone.is_some() { look::dim() } else { look::faint() })).child(name))),
            )
            .child(if gone.is_none() { primary("compare", open_word.into(), true) } else { button("compare".into(), open_word, false, look::grey()) }.on_click(open))
            .child(button("later".into(), "Esc · later", false, look::grey()).on_click(cx.listener(move |this, _, _, cx| {
                this.shrunk.insert(q.clone());
                cx.notify();
            })))
    }
}

#[cfg(feature = "test-driver")]
/// With JAYNES_APP_TEST_DO=<file>, a test says what the person does, a line at a time: `mark <item> <verdict>`,
/// `words <text>`, `focus-words`, `read`, `bring <item>`, `tab <run>`, `agent <address>`, `notice <agent|run>`, `pref <name> <value>`, `system light|dark`, `settings`, `settings-section <name>`, `settings-fonts`, `settings-close`, `copy-summary`, `presence?`, `act <control>`, `palette <type|down|up|go>`, `bar <bar> <x>|own`, `column <click>`, `close-window`, `reopen`, `shown?`, `signals?`, `panic`, `settings-pick <name> <value>`, `at <run> top|end|<step>`, `waits`, `do <action>`, `key <keystroke>`, `front`, `zoom`, `move-tab <run|main> <place>`, `diff`, `picture`, `send`, `move-to-finished`, `send-read`, `compare`, and in the compare mode the lines it reads.
/// Each goes through what a click does, so a test answers without the person's mouse.
fn drive_from(file: PathBuf, cx: &mut Context<Island>) {
    cx.spawn(async move |this, cx| loop {
        cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
        // taken whole by a rename, so a line written while it is read is not lost: it lands in a new file
        let taking = file.with_extension("taking");
        if std::fs::rename(&file, &taking).is_err() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&taking) else { continue };
        let _emptied = std::fs::write(&file, "");
        if text.trim().is_empty() {
            continue;
        }
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

#[cfg(feature = "test-driver")]
impl Island {
    fn driven(&mut self, line: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
        // the pointer, as macOS sends it to the window, at a point of it: `pointer move|down|up|click x y`,
        // `pointer drag x y x2 y2`, `pointer pinch x y <delta>`, `pointer zoom-scroll x y <dy>` (Command and the wheel),
        // `pointer scroll x y <dy>` (the wheel alone)
        if verb == "pointer" {
            let w: Vec<&str> = rest.split_whitespace().collect();
            let n = |i: usize| w.get(i).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.);
            let at = point(px(n(1)), px(n(2)));
            let to = point(px(n(3)), px(n(4)));
            let left = MouseButton::Left;
            let events: Vec<PlatformInput> = match w.first().copied().unwrap_or("") {
                "move" => vec![PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() })],
                "down" => vec![PlatformInput::MouseDown(MouseDownEvent { button: left, position: at, modifiers: Modifiers::default(), click_count: 1, first_mouse: false })],
                "up" => vec![PlatformInput::MouseUp(MouseUpEvent { button: left, position: at, modifiers: Modifiers::default(), click_count: 1 })],
                "click" => vec![
                    PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }),
                    PlatformInput::MouseDown(MouseDownEvent { button: left, position: at, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }),
                    PlatformInput::MouseUp(MouseUpEvent { button: left, position: at, modifiers: Modifiers::default(), click_count: 1 }),
                ],
                "drag" => {
                    let mut e = vec![PlatformInput::MouseDown(MouseDownEvent { button: left, position: at, modifiers: Modifiers::default(), click_count: 1, first_mouse: false })];
                    for k in 1..=8 {
                        let t = k as f32 / 8.;
                        let p = point(at.x + (to.x - at.x) * t, at.y + (to.y - at.y) * t);
                        e.push(PlatformInput::MouseMove(MouseMoveEvent { position: p, pressed_button: Some(left), modifiers: Modifiers::default() }));
                    }
                    e.push(PlatformInput::MouseUp(MouseUpEvent { button: left, position: to, modifiers: Modifiers::default(), click_count: 1 }));
                    e
                }
                "pinch" => vec![PlatformInput::Pinch(PinchEvent { position: at, delta: n(3), modifiers: Modifiers::default(), phase: TouchPhase::Moved })],
                "zoom-scroll" => vec![PlatformInput::ScrollWheel(ScrollWheelEvent { position: at, delta: ScrollDelta::Pixels(point(px(0.), px(n(3)))), modifiers: Modifiers { platform: true, ..Modifiers::default() }, touch_phase: TouchPhase::Moved })],
                "scroll" => vec![PlatformInput::ScrollWheel(ScrollWheelEvent { position: at, delta: ScrollDelta::Pixels(point(px(0.), px(n(3)))), modifiers: Modifiers::default(), touch_phase: TouchPhase::Moved })],
                other => {
                    tracing::warn!("no pointer {other}");
                    vec![]
                }
            };
            window.defer(cx, move |window, cx| {
                for e in events {
                    window.dispatch_event(e, cx);
                }
            });
            return;
        }
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
            // the words under a Change or a Redo of an item, as typed there
            "note" => {
                let (item, text) = rest.split_once(' ').unwrap_or((rest, ""));
                if let Some(n) = self.notes.get(item).cloned() {
                    let text = text.to_string();
                    n.update(cx, |s, cx| s.set_value(text, window, cx));
                }
                self.reckon(cx);
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
            // a setting changed, as the Settings screen changes it: `pref <name> <value>`
            "pref" => {
                let (name, value) = rest.split_once(' ').unwrap_or((rest, ""));
                if let Err(e) = crate::prefs::change(name, value, cx) {
                    tracing::warn!("{e}");
                }
            }
            // the Settings window, as Command-comma opens it; a section of it, as a click in its toolbar; a control in
            // it, as a click on it: `settings-pick <name> <value>`
            "settings" => window.dispatch_action(Box::new(crate::OpenSettings), cx),
            "settings-section" => {
                if let Some(s) = crate::settings_window::Section::named(rest) {
                    crate::settings_window::with_open(cx, |v, _, cx| v.show(s, cx));
                }
            }
            // the window's close button, as a click on it closes the window; and a panic, to see the app's log keep it
            // as the close button does; and the Dock's click, which opens it again
            "close-window" => {
                crate::close_asked(window, cx);
            }
            // after this line, as the Dock's click comes: from outside the window's own update
            "reopen" => cx.defer(crate::show_main),
            // how the app holds the ending signals now, and whether a program it starts ends on TERM
            #[cfg(unix)]
            "signals?" => {
                tracing::info!("signals now: {}", crate::signals::said());
                let ended = std::process::Command::new("/bin/sh").args(["-c", "kill -TERM $$; sleep 1; exit 7"]).status();
                tracing::info!("a program the app starts, sent TERM: {ended:?}");
                // and from a background thread, which blocks TERM, through what the app's work that starts programs
                // goes through (signals.rs)
                cx.background_spawn(async {
                    let blocked = crate::signals::blocked_here(libc::SIGTERM);
                    let ended = crate::signals::open(|| std::process::Command::new("/bin/sh").args(["-c", "kill -TERM $$; sleep 1; exit 7"]).status());
                    tracing::info!("from a background thread (TERM blocked there: {blocked}), sent TERM: {ended:?}");
                })
                .detach();
            }
            "shown?" => {
                let shown = crate::shown(window);
                tracing::info!("the main window is {}", if shown { "on the screen" } else { "hidden" });
            }
            "panic" => panic!("a test asked the app to panic"),
            // what the next presence tick would tell, told in the log and sent nowhere
            "presence?" => cx.defer(crate::presence_probe),
            // Later on the line over an agent that says a run waits
            "later" => cx.defer(crate::later_on_line),
            // Command-K: `palette open`, `palette type <letters>`, `palette down`, `palette go`; `palette open` needs no keys
            "palette" => {
                let what = rest.to_string();
                cx.defer(move |cx| crate::palette_does(what, cx));
            }
            // a click in the runs column: `column finished|cleared|clear <run>|clear-finished|back <run>`
            "column" => {
                let what = rest.to_string();
                cx.defer(move |cx| crate::column_does(what, cx));
            }
            // a bar's edge dragged with the pointer at x, or double clicked: `bar column|steps|changes <x>|own`
            "bar" => {
                let what = rest.to_string();
                cx.defer(move |cx| crate::drag_bar(what, cx));
            }
            // a press on the open run's control: `act resume|continue|raise-risk|raise-ceiling|abandon`
            "act" => {
                let word = rest.to_string();
                cx.defer(move |cx| crate::press_act(word, cx));
            }
            // the Copy as Markdown button under a finished run's story
            "copy-summary" => window.dispatch_action(Box::new(crate::CopySummary), cx),
            "settings-close" => {
                crate::settings_window::with_open(cx, |_, window, _| window.remove_window());
            }
            "settings-fonts" => {
                crate::settings_window::with_open(cx, |v, _, cx| v.toggle_fonts(cx));
            }
            "settings-pick" => {
                let (name, value) = rest.split_once(' ').unwrap_or((rest, ""));
                let (name, value) = (name.to_owned(), value.to_owned());
                crate::settings_window::with_open(cx, |v, _, cx| v.pick(&name, &value, cx));
            }
            // macOS going light or dark, as the system tells the window: `system light|dark`
            "system" => crate::prefs::system_changed(rest == "dark", cx),
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
                // down, then up: gpui presses a control with the keys on the key's release, as a real keyboard sends it
                Ok(k) => window.defer(cx, move |window, cx| {
                    window.dispatch_keystroke(k.clone(), cx);
                    window.dispatch_event(PlatformInput::KeyUp(KeyUpEvent { keystroke: k }), cx);
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
            // a press on a box of what the stop offers: `offer test_servers`
            "offer" => {
                if let Some(stop) = self.stop(cx) {
                    self.ticks.at(&stop.question);
                    for o in stop.offers.iter().filter(|o| crate::offers::id_of(o.offer) == rest) {
                        self.ticks.flip(o, None);
                    }
                    cx.notify();
                }
            }
            // the card's Move to Finished, on a run whose engine left
            "move-to-finished" => {
                // the card's, or the block at the top of the run's tab when no card of it is open
                let run = self.stop(cx).map(|s| s.run).or_else(|| self.tabs.as_ref().and_then(|t| t.read(cx).open.clone()));
                if let Some(run) = run {
                    self.move_to_finished(run, cx);
                }
            }
            "send-read" => self.send_understood(cx),
            "compare" => {
                if let Some(stop) = self.stop(cx) {
                    self.compare_open(stop, window, cx);
                }
            }
            // a press on the card's words box, as the pointer gives it the keys
            "focus-words" => {
                if let Some(w) = self.words.clone() {
                    w.update(cx, |s, cx| {
                        s.set_cursor_position(Position::new(0, u32::MAX), window, cx);
                        s.focus(window, cx);
                    });
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
        // docked under the terminal, which shrinks to make room: the stop never covers the line where the person types
        let wrap = div().w_full().flex_none().px(px(14.)).pb(px(12.)).pt(px(4.)).flex().justify_center().font_family(look::UI_FONT);
        let tab_run = self.tab_run(cx);
        let now = Instant::now();
        // the answer just sent, at the foot of its run's tab, once its question is gone or while the record catches up
        let sent_foot = |q: Option<&str>, this: &Self| {
            this.sent.as_ref().filter(|(s, question)| s.shows(tab_run.as_deref(), now) && q.is_none_or(|q| q == question)).map(|(s, _)| {
                div().w_full().px(px(16.)).py(px(10.)).rounded(px(12.)).bg(rgb(look::panel_2())).border_1().border_color(rgb(look::line())).text_size(px(12.)).text_color(rgb(look::faint())).child(s.said.clone())
            })
        };
        let Some(stop) = self.stop(cx) else {
            return match sent_foot(None, self) {
                Some(foot) => wrap.child(foot).into_any_element(),
                None => div().into_any_element(),
            };
        };
        if self.compare.read(cx).is_open() {
            return div().into_any_element();
        }
        let on_tab = tab_run.as_deref() == Some(stop.run.as_str());
        if stop.choose && on_tab {
            // the compare mode sent it and closed: the tab says so until the record moves on
            if let Some(foot) = sent_foot(Some(&stop.question), self) {
                return wrap.child(foot).into_any_element();
            }
        }
        let name = self.run_name(&stop.run, cx);
        if shows(on_tab, self.shrunk.contains(&stop.question)) == Shows::Pill {
            let (q, run, step) = (stop.question.clone(), stop.run.clone(), stop.step.clone());
            return wrap
                .child(
                    tabbed(div().id("pill"))
                        .px(px(14.))
                        .py(px(8.))
                        .rounded(px(999.))
                        .bg(rgb(look::panel_2()))
                        .border_1()
                        .border_color(rgb(look::amber()))
                        .text_size(px(13.))
                        .text_color(rgb(look::text()))
                        .cursor_pointer()
                        .truncate()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.shrunk.remove(&q);
                            // the card opens with the keys on its first control: the pill it was is gone, and a Tab from
                            // what is gone would go to the window's first stop, off the card
                            if on_tab {
                                window.focus(&this.first, cx);
                            }
                            // over Claude Code's terminal: the run's tab, where the card opens at its step
                            if !on_tab && let Some(tabs) = this.tabs.clone() {
                                let (run, step) = (run.clone(), step.clone());
                                tabs.update(cx, |t, cx| t.show_at(run, crate::tabs::Place::Step(step), cx));
                            }
                            cx.notify();
                        }))
                        .child(pill(&name, &stop.title)),
                )
                .into_any_element();
        }
        if stop.choose {
            return wrap.child(div().w_full().child(self.invite(&stop, cx))).into_any_element();
        }
        let delivered = self.runs.read(cx).snap.details.get(&stop.run).is_some_and(|d| d.delivered);
        let project = self.runs.read(cx).snap.details.get(&stop.run).map(|d| d.project.clone()).unwrap_or_default();
        if self.words.is_none() {
            self.words = Some(self.input(WORDS_HINT, window, cx));
        }
        let words = self.words.clone().expect("made above");
        let judged = stop.items.iter().filter(|i| self.marks.contains_key(&i.id)).count();
        let can = self.can_send(&stop);
        let merge = stop.kind == QuestionKind::Merge;
        let mut items = div().flex().flex_col().gap(px(10.));
        if merge {
            items = items.child(self.landing_view(&stop, cx));
        } else {
            for it in &stop.items {
                items = items.child(if stop.permission { self.leave_view(it, &stop, cx) } else { self.item_view(it, &stop, false, window, cx) });
            }
            // what the stop offers beside the answer, under the items: in items, so WORDS_ROW still counts right
            self.ticks.at(&stop.question);
            for o in &stop.offers {
                let flip = o.clone();
                items = items.child(crate::offers::row(o, self.ticks.ticked(o, None)).on_click(cx.listener(move |this, _, _, cx| {
                    this.ticks.flip(&flip, None);
                    cx.notify();
                })));
            }
        }
        let mut back = div().flex().flex_col().gap(px(8.));
        if !stop.back.is_empty() {
            let open = self.show_back;
            back = back.child(
                tabbed_with(div().id("back"), Some(look::panel_2()))
                    .self_start()
                    .px(px(4.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .text_color(rgb(look::dim()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_back = !this.show_back;
                        cx.notify();
                    }))
                    .child(format!("{} You dropped {} earlier: only you bring it back", if open { "▾" } else { "▸" }, stop.back.len())),
            );
            if open {
                for it in &stop.back {
                    back = back.child(self.item_view(it, &stop, true, window, cx));
                }
            }
        }
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
                        .p(px(10.))
                        .rounded(px(10.))
                        .bg(rgb(look::panel()))
                        .border_1()
                        .border_color(rgb(look::blue()))
                        .child(look::label("What Jaynes read · this is what it does when you send", look::blue()))
                        .child(sentences(&u.sentences))
                        .when(!u.unsure.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(rgb(look::amber())).child(format!("Not sure: {}", u.unsure))))
                        .when(!u.repeats.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(rgb(look::amber())).child(format!("You asked this in an earlier round: {}", u.repeats.join("; ")))))
                        .when_some(u.refused.clone(), |d, r| d.child(div().text_size(px(12.)).text_color(rgb(look::red())).child(r)))
                        .child(
                            div()
                                .flex()
                                .gap(px(8.))
                                .child(primary("send-read", "Send this".into(), ok).when(ok, |d| d.on_click(cx.listener(|this, _, _, cx| this.send_understood(cx)))))
                                .child(button("by-hand".into(), "Answer with the buttons", false, look::grey()).on_click(cx.listener(|this, _, _, cx| {
                                    this.understood = None;
                                    cx.notify();
                                }))),
                        ),
                )
            }
            _ => None,
        };
        let send_label = send_label(&stop, self.sending);
        let q = stop.question.clone();
        let diff_stop = stop.clone();
        let field = self.words_focused(window, cx);
        // the words field taking the keys comes whole into view, never half under the foot. Read from the focus as
        // the card is drawn, not from the field's Focus event: gpui sends that only to the window in front, so a
        // field focused in a window behind (and in every hidden test photo) never moved the view
        if comes_into_view(self.words_had_keys, field) {
            match words_offset(self.scroll.bounds(), self.scroll.bounds_for_item(WORDS_ROW), self.scroll.max_offset().y, self.scroll.offset().y) {
                Some(y) => self.scroll.set_offset(point(px(0.), y)),
                None => self.scroll.scroll_to_item(WORDS_ROW),
            }
        }
        self.words_had_keys = field;
        let (first, second) = card_head(&name, &project, &stop.step, stop.round);
        let head = div()
            .flex()
            .gap(px(10.))
            .items_center()
            .child(div().size(px(8.)).flex_none().rounded_full().bg(rgb(look::amber())))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(div().truncate().text_size(px(13.)).text_color(rgb(look::text())).child(first))
                    .child(div().truncate().text_size(px(12.)).text_color(rgb(look::faint())).child(second)),
            )
            // at the merge, "See the changes" is beside what goes in
            .when_some(diff_button(&stop).filter(|_| !merge), |d, words| d.child(button("diff".into(), words, false, look::blue()).track_focus(&self.first).on_click(cx.listener(move |this, _, window, cx| this.diff(&diff_stop, window, cx)))))
            // as wide as its longer words, as the compare mode's; a press on it keeps the keys where they are, so it
            // does what it says
            .child(
                button("shrink".into(), esc_chip(field), false, look::grey())
                    .min_w(px(170.))
                    .flex()
                    .justify_center()
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.words_focused(window, cx) {
                            this.leave_field(window, cx);
                        } else {
                            this.shrunk.insert(q.clone());
                            cx.notify();
                        }
                    })),
            );
        let marked: Vec<bool> = stop.items.iter().map(|i| self.marks.contains_key(&i.id)).collect();
        let said = said_lines(&self.reckoned.sentences, &marked, can && judged < stop.items.len());
        // the head stays: the run, where, the question; only the middle scrolls
        let top = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(16.))
            .pt(px(16.))
            .pb(px(10.))
            .child(head)
            .when_some(stop.gone.as_ref(), |d, gone| {
                let run = stop.run.clone();
                d.child(gone_block(gone, self.gone_open, cx.listener(|this, _, _, cx| { this.gone_open = !this.gone_open; cx.notify(); }), cx.listener(move |this, _, _, cx| this.move_to_finished(run.clone(), cx)), self.move_said.as_deref()))
            })
            .when(stop.gone.is_none(), |d| d.child(div().flex().flex_col().gap(px(4.)).child(title_view(&project, &stop.title).text_size(px(16.)).text_color(rgb(look::text()))).when_some(stop.why.clone(), |d, why| d.child(div().text_size(px(12.5)).text_color(rgb(look::dim())).child(why))).when(merge && !stop.asks.is_empty(), |d| d.child(div().text_size(px(12.5)).text_color(rgb(look::dim())).child(stop.asks.clone()))).when(delivered, |d| d.child(div().text_size(px(12.)).text_color(rgb(look::amber())).child(DELIVERED)))));
        // the middle scrolls between the fixed head and foot; while there is more below, its bottom edge fades
        let max = self.scroll.max_offset().y;
        let more_below = fades_below(max, self.scroll.offset().y);
        let middle = div()
            .id("island-scroll")
            .track_scroll(&self.scroll)
            .min_h_0()
            .flex_shrink(1.)
            .overflow_y_scroll()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .px(px(16.))
            .pt(px(2.))
            .pb(px(MIDDLE_PAD))
            .flex()
            .flex_col()
            .gap(px(12.))
            // WORDS_ROW and READ_ROW count these children: items, back, the words, the reading
            .child(items)
            .child(back)
            // the merge is answered with its two ways out, for every file at once: no words, no item to judge
            .when(!merge, |d| d.child(div().flex().gap(px(8.)).items_center().child(div().flex_1().child(focus_field("words", &words, window, cx))).child(button("read".into(), "Read my words", false, look::blue()).on_click(cx.listener(|this, _, _, cx| this.read_words(cx))))))
            .children(understood)
            .when(!said.is_empty(), |d| d.child(div().flex().flex_col().gap(px(4.)).child(look::label("What this does", look::faint())).child(sentences(&said))))
            .when_some(ahead_line(&stop), |d, line| d.child(div().text_size(px(12.)).text_color(rgb(look::amber())).child(line)));
        let transparent = rgba(look::panel_2() << 8);
        // 32 px, over content: the foot's own line is already there, and 16 vanished over the gap between two rows
        let fade = div().absolute().left_0().right_0().bottom_0().h(px(FADE)).bg(linear_gradient(180., linear_color_stop(transparent, 0.), linear_color_stop(rgb(look::panel_2()), 1.)));
        let foot = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(16.))
            .py(px(10.))
            .border_t_1()
            .border_color(rgb(look::line()))
            .child({
                // why Send stays off, where Send is: never under the middle's fold
                let (line, refusing) = foot_line(self.said.as_deref(), judged, stop.items.len(), self.reckoned.refused.as_deref());
                div().flex_1().min_w_0().text_size(px(12.)).text_color(rgb(if refusing { look::red() } else { look::faint() })).child(line)
            })
            .when(!merge, |d| d.child(primary("send", send_label, can).when(can, |d| d.on_click(cx.listener(|this, _, _, cx| this.send(cx))))));
        // the merge's foot: its two ways out, each with what it does in the engine's words, and the one touch
        let foot = if merge {
            let (keep, drop) = self.merge_said(&stop);
            let going = !self.sending;
            let way = |b: Stateful<Div>, said: Vec<String>| div().flex().gap(px(12.)).items_center().child(b.flex_none().min_w(px(150.)).flex().justify_center()).child(div().flex_1().min_w_0().text_size(px(12.5)).text_color(rgb(look::dim())).child(said.join(" ")));
            let merge_label = if self.sending { "Merging…".to_string() } else { format!("{} · Touch ID", stop::label_for(VerdictKind::Keep, stop.kind)) };
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(8.))
                .px(px(16.))
                .py(px(12.))
                .border_t_1()
                .border_color(rgb(look::line()))
                .child(way(primary("merge", merge_label, going).when(going, |d| d.on_click(cx.listener(|this, _, _, cx| this.send_all(VerdictKind::Keep, cx)))), keep))
                .child(way(button("branch-only".into(), stop::label_for(VerdictKind::Drop, stop.kind), false, look::text()).when(going, |d| d.on_click(cx.listener(|this, _, _, cx| this.send_all(VerdictKind::Drop, cx)))), drop))
                .child(div().text_size(px(12.)).text_color(rgb(look::faint())).child(self.said.clone().unwrap_or_else(|| one_touch(&stop))))
        } else {
            foot
        };
        let card = div()
            .id("island")
            .track_focus(&self.focus)
            .w_full()
            .max_h(card_cap(window.viewport_size().height))
            .overflow_hidden()
            .rounded(px(12.))
            .bg(rgb(look::panel_2()))
            .border_1()
            .border_color(rgba((look::amber() << 8) | 0x73))
            .flex()
            .flex_col()
            .child(top)
            .when(takes_answers(&stop), |d| d.child(div().relative().min_h_0().flex_shrink(1.).flex().flex_col().child(middle).when(more_below, |d| d.child(fade))).child(foot));
        // the card takes the room it needs, up to its cap; in a window too small for it, it gives way, and its middle
        // scrolls
        wrap.min_h_0().flex_shrink(1.).child(card).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // named, not `super::*`: that brings gpui's own `test` attribute in, over the standard one
    use super::{TESTS_RUN_MARK, diff_button, leave_rows, question_title, wants_view, DELIVERED, LATER_HEAD, ONE_TOUCH, READ_THESE, RUNS_LATER, SENT, Sent, Shows, WORDS_HINT, ahead_line, landing, send_label, asks, card_cap, caret, card_head, comes_into_view, diff_title, fades_below, field_look, words_offset, enters_card, esc_chip, foot_line, gone_head, gone_invite, may_send, takes_answers, not_judged, pill, GONE_HOW, reads_on, said_lines, shows, tab_goes_round, takes_esc, next_after, sent_said, diff_shown, one_touch, rides_part, RIDE_OPEN};
    use jaynes_core::question::{QuestionKind, VerdictKind};
    use jaynes_shell::stop::{Item, Stop, label_for};
    use std::collections::BTreeMap;

    /// A verdict stop with `ids` for items, one dropped in an earlier round, invented.
    fn judging(ids: &[&str]) -> Stop {
        let item = |id: &str| Item { id: id.into(), label: format!("The {id}"), detail: String::new(), aspects: vec![], pictures: vec![], page: None, options: vec![] };
        Stop {
            run: "r".into(),
            question: "q".into(),
            shown: "s".into(),
            title: "Which of these copy changes go in?".into(),
            asks: "Which of these copy changes go in?".into(),
            step: "copy".into(),
            permission: false,
            kind: QuestionKind::Decision,
            choose: false,
            offered: vec![VerdictKind::Keep, VerdictKind::Change, VerdictKind::Redo, VerdictKind::Drop],
            items: ids.iter().map(|i| item(i)).collect(),
            back: vec![item("old")],
            round: 1,
            about: String::new(),
            ahead: None,
            touch: None,
            into: String::new(),
            lands: vec![],
            tests_run: None,
            passed: None,
            answers: Default::default(),
            note: String::new(),
            why: None,
            consequences: Default::default(),
            wants: None,
            gone: None,
            offers: Vec::new(),
            rides: vec![],
        }
    }

    // the design-lead's quatro-itens.md, item 2 (the real-user, 22/11): two kickoffs waiting read alike ("Is this the
    // first slice?"), so the stop card's title and the choice screen's name the project first, dim
    #[test]
    fn asked_the_question_s_title_names_its_project_first() {
        assert_eq!(question_title("horta", "Is this the first slice?"), (Some("horta · ".to_string()), "Is this the first slice?".to_string()));
        assert_eq!(question_title("", "Is this the first slice?"), (None, "Is this the first slice?".to_string()), "without a project, the question alone");
    }

    // the design-lead's quatro-itens.md, item 3 (the real-user, 22/11): after a Send the next question opened without a
    // word of where the answer went; the app's pill says it for its 4 seconds, as the page's line does
    #[test]
    fn asked_after_send_the_app_says_where_it_went() {
        let next = Some(("horta", "Is this the first slice?"));
        assert_eq!(sent_said(Some("scaffold-vazio"), next), "Sent to scaffold-vazio. Next: horta · Is this the first slice?");
        assert_eq!(sent_said(Some("horta"), next), "Sent. Next: Is this the first slice?", "within one project");
        assert_eq!(sent_said(None, next), "Sent. Next: horta · Is this the first slice?", "from a run without a project");
        assert_eq!(sent_said(Some("horta"), None), "Sent. Nothing else waits for you.");
        // what comes next: the first other run that needs the person, as the column lists them
        let row = |id: &str, project: &str, line: &str| jaynes_shell::listing::Row { id: id.into(), project: project.into(), title: String::new(), apart: None, line: line.into(), mark: jaynes_shell::listing::Mark::Needs };
        let needs = [row("sent", "scaffold-vazio", "Is this the first slice?"), row("b", "horta", "Is this the first slice?"), row("c", "site", "Go with “Harbor”?")];
        assert_eq!(next_after(&needs, "sent"), Some(("horta", "Is this the first slice?")));
        assert_eq!(next_after(&needs[..1], "sent"), None);
        let lone = [row("x", "no project", "Merge into “main”?")];
        assert_eq!(next_after(&lone, "sent"), Some(("", "Merge into “main”?")), "no project names none");
    }

    // the engineer lead, 22/11 (row 32, merge.md): the merge's touch keeps the stops the run went on without, so the
    // app's card shows each before the touch, or the record says the person kept what they never saw in the app
    #[test]
    fn asked_the_merge_card_names_what_the_touch_keeps() {
        use jaynes_core::question::Ride;
        let mut merge = judging(&["copy/hero.txt"]);
        merge.kind = QuestionKind::Merge;
        assert_eq!((rides_part(&merge), one_touch(&merge)), (None, ONE_TOUCH.to_string()), "nothing open, no part and the touch alone");
        merge.rides = vec![
            Ride { question: "q-stack".into(), step: "stack".into(), said: "built on Go, standard library, as your request says".into() },
            Ride { question: "q-bug".into(), step: "bug".into(), said: "The bug above, as your request says.".into() },
        ];
        let part = rides_part(&merge).expect("the stops the touch keeps");
        assert_eq!(part.head, "Your touch also keeps these");
        assert_eq!(
            part.rows,
            [
                ("“stack”".to_string(), "Built on Go, standard library, as your request says.".to_string(), "q-stack".to_string()),
                ("“bug”".to_string(), "The bug above, as your request says.".to_string(), "q-bug".to_string()),
            ],
            "the step in quotes, what it keeps as a sentence, and the stop Open opens"
        );
        assert_eq!(part.under, "Merging keeps each as it is. To answer one differently, open it before you merge.");
        assert_eq!(RIDE_OPEN, "Open");
        assert_eq!(one_touch(&merge), "Merging asks for your touch, once. It also keeps the 2 answers above.");
        merge.rides.truncate(1);
        assert_eq!(one_touch(&merge), "Merging asks for your touch, once. It also keeps the answer above.");
        let mut decision = judging(&["a"]);
        decision.rides = merge.rides.clone();
        assert_eq!(rides_part(&decision), None, "only the merge's card");
    }

    // the engineer lead, 22/11 (row 9), in the design-lead's words: at a stop of a run whose older engine left the Mac,
    // the app heard the refusal only after Send; the card says before it that the run can't go on, and what to do
    #[test]
    fn asked_a_stop_of_a_run_whose_engine_left_says_so_before_send() {
        let gone = jaynes_shell::stop::Gone { version: "0.2.3".into(), place: None, moved: None, said: "run r started on engine 0.2.3+abc, which is no longer on this machine: it cannot be carried on by this one (0.2.5+def)".into() };
        assert_eq!(gone_head(&gone), "This run can't go on: it started on Jaynes 0.2.3, which is no longer on this Mac.");
        assert_eq!(GONE_HOW, "No answer reaches it, Halt included. To do this work, start it again: a new run starts on this Jaynes.");
        let mut stop = judging(&["a"]);
        assert!(may_send(&stop, 1, false, false, false), "a run of this engine sends");
        stop.gone = Some(gone);
        assert!(!may_send(&stop, 1, false, false, false), "Send off: no answer reaches it");
        // the design-lead, 22/11: a control that reaches nothing invites a try; the card keeps its head, the two lines,
        // Details, Esc and See the diff (which reads the run's copy), and nothing that answers
        assert!(!takes_answers(&stop), "no items, words, foot nor Send on it");
        assert!(takes_answers(&judging(&["a"])), "a run of this engine is answered");
    }

    // the design-lead, 22/11 (row 28, (a) and (b)): the invite of a choice whose run's engine left says it can't go on,
    // grey, and the compare mode shows no round, since no other comes
    #[test]
    fn asked_a_choice_whose_engine_left_invites_in_grey_and_shows_no_round_22_11() {
        let gone = jaynes_shell::stop::Gone { version: "0.2.3".into(), place: None, moved: None, said: String::new() };
        assert_eq!(gone_invite(&gone), "◆ This run can't go on: it started on Jaynes 0.2.3.");
        let mut stop = judging(&["a"]);
        stop.round = 1;
        assert_eq!(crate::compare::round_word(&stop).as_deref(), Some("round 1"));
        stop.gone = Some(gone);
        assert_eq!(crate::compare::round_word(&stop), None);
        // where the work is, in three parts, the place shown in mono with its Copy
        let place = jaynes_shell::listing::Place::Branch { branch: "jaynes/r1".into(), project: "tidepool".into() };
        assert_eq!(place.parts(), ("What it delivered is on the branch “".into(), "jaynes/r1".into(), "” of tidepool.".into()));
        assert_eq!(jaynes_shell::moved::moved_on(1_763_800_000).len(), "You moved it to Finished on 22/11.".len());
    }

    // Alan, 16/11 (direction D of the sketches, as the app's first handoff wrote what he chose): "the stop rises as a
    // big island over the terminal (Esc shrinks it to a pill)"
    #[test]
    fn asked_the_stop_rises_as_a_big_island_over_the_terminal_and_esc_shrinks_it_to_a_pill() {
        use gpui::px;
        // on its run's tab the stop rises as its card, over the run's terminal
        assert_eq!(shows(true, false), Shows::Card);
        // a big island: up to 70% of the window
        assert_eq!(card_cap(px(1000.)), px(700.));
        // Esc is the card's while it is open, before the terminal under it sees it
        assert!(takes_esc(Shows::Card, false));
        // and folded, it is a pill of one line that says which run asks, and opens again
        assert_eq!(shows(true, true), Shows::Pill);
        assert_eq!(pill("Tighten the site's copy", "Which of these go in?"), "◆ Tighten the site's copy: Which of these go in? · open");
    }

    // Alan, 16/11 (as the app's first handoff wrote what he chose): "A verdict stop says what Approve and Refuse do,
    // has a "what is missing?" line per refused item, enlarges proof pictures and shows the diff."
    #[test]
    fn asked_a_verdict_stop_says_what_its_buttons_do_and_asks_what_is_missing_per_refused_item() {
        // the buttons are named as the question's kind names them: Allow and Refuse for a permission
        assert_eq!((label_for(VerdictKind::Keep, QuestionKind::Permission), label_for(VerdictKind::Drop, QuestionKind::Permission)), ("Allow", "Refuse"));
        // what each mark does, in the engine's sentences, one a line under "What this does", and a line for the rest
        let said: Vec<String> = ["“The hero” is kept.", "“The footer” is done again.", "“The menu” is kept."].map(String::from).to_vec();
        assert_eq!(said_lines(&said, &[true, true, false], false), vec!["“The hero” is kept.", "“The footer” is done again.", "1 not judged yet"]);
        // an item sent back gets its own line of the person's words: what is missing, or how it should change
        assert_eq!(asks(VerdictKind::Redo), Some("what is missing?"));
        assert_eq!(asks(VerdictKind::Change), Some("how should it change?"));
        assert_eq!((asks(VerdictKind::Keep), asks(VerdictKind::Drop), asks(VerdictKind::Skip)), (None, None, None));
        // and those words go with that item in the answer
        let stop = judging(&["hero", "footer"]);
        let verdicts: BTreeMap<String, String> = [("hero".into(), "keep".into()), ("footer".into(), "redo".into())].into();
        let notes: BTreeMap<String, String> = [("footer".into(), "the links to the docs".into())].into();
        let body = crate::answer::body(&stop, &verdicts, &notes, &BTreeMap::new(), "");
        assert_eq!(body["notes"]["footer"], "the links to the docs");
        // the change the step made opens as its diff, named by its step
        assert_eq!(diff_title("copy"), "Changes made at “copy”");
    }

    // Alan, 16/11 (as the app's first handoff wrote what he chose): "the lock binds the agent, never the person. Every
    // stop has "Your words" (read into verdicts, effects written by Jaynes), bring back what was dropped, and the parts
    // of an option shown apart."
    #[test]
    fn asked_every_stop_has_your_words_and_brings_back_what_was_dropped() {
        assert!(WORDS_HINT.starts_with("Your words"));
        assert!(crate::compare::WORDS_HINT.starts_with("Your words"));
        // the person's words go with the answer, whatever they marked; what they bring back goes with theirs on how
        let stop = judging(&["hero"]);
        let back: BTreeMap<String, String> = [("old".into(), "with the new colours".into())].into();
        let body = crate::answer::body(&stop, &BTreeMap::new(), &BTreeMap::new(), &back, "  keep it short  ");
        assert_eq!(body["context"], "keep it short");
        assert_eq!(body["back"]["old"], "with the new colours");
        // what was dropped is the stop's to show, and only the person brings it back
        assert_eq!(stop.back.len(), 1);
    }

    // Alan, 16/11, asked "quer que o Mac peça Touch ID? 1. Nunca 2. Só no que não se desfaz, como o merge 3. Em toda
    // resposta": "2"
    #[test]
    fn asked_touch_id_only_before_what_cannot_be_undone() {
        // nothing that cannot be undone ahead: no touch asked, and the card says none
        let nothing = judging(&["a"]);
        assert_eq!((crate::answer::touch_of(&nothing), ahead_line(&nothing)), (None, None));
        // ahead of “send”: macOS asks for the touch saying why, and the card says so before the person sends
        let mut sending = judging(&["a"]);
        sending.ahead = Some("“send” cannot be undone".into());
        sending.touch = Some("send your answer to Jaynes: “send” cannot be undone".into());
        assert_eq!(ahead_line(&sending).as_deref(), Some("Ahead: “send” cannot be undone. Sending asks for your touch."));
        assert_eq!(crate::answer::touch_of(&sending), Some("send your answer to Jaynes: “send” cannot be undone"));
    }

    // Alan, 21/11: "pq to sendo chamado aqui? a role?" A leave says why it comes to the person, what each answer
    // does beside its button, and what the agent wants to write, in place of a step's changes that are not there yet
    // (design 0.2.5 permissao.md, on the roles-engineer's 6cbec6e)
    #[test]
    fn asked_permission_card_says_why_you() {
        use jaynes_shell::stop::Wants;
        let mut leave = judging(&["leave"]);
        (leave.kind, leave.permission, leave.offered) = (QuestionKind::Permission, true, vec![VerdictKind::Keep, VerdictKind::Drop]);
        leave.title = "May the agent change `tests/conftest.py`?".into();
        leave.why = Some("Why you: the agent asked for something its step's settings do not allow.".into());
        leave.consequences = [
            (VerdictKind::Keep, "It writes the file and goes on. Before it lands in your branch, the merge asks you again.".to_string()),
            (VerdictKind::Drop, "It goes on without writing it.".to_string()),
        ]
        .into_iter()
        .collect();
        leave.wants = Some(Wants { file: "tests/conftest.py".into(), before: None, after: "import pytest\n\n@pytest.fixture\n".into() });
        // the name in mono, never with its backquotes
        assert_eq!(crate::look::ticks(&leave.title), [("May the agent change ".to_string(), false), ("tests/conftest.py".to_string(), true), ("?".to_string(), false)]);
        assert_eq!(crate::look::ticks("no name here"), [("no name here".to_string(), false)]);
        // Allow and Refuse, each with what it does beside it
        assert_eq!(leave_rows(&leave), [
            ("Allow", "It writes the file and goes on. Before it lands in your branch, the merge asks you again."),
            ("Refuse", "It goes on without writing it."),
        ]);
        // what it wants to write, never "This step changed nothing in the project"
        assert_eq!(diff_button(&leave), Some("See what it wants to write"));
        let (title, text) = wants_view(leave.wants.as_ref().expect("wants"));
        assert_eq!(title, "What the agent wants to write in tests/conftest.py · not written yet");
        assert_eq!(text, "+import pytest\n+\n+@pytest.fixture");
        // an edit: what goes and comes, among what stays
        let edit = Wants { file: "src/a.py".into(), before: Some("a\nb\nc\n".into()), after: "a\nB\nc\n".into() };
        assert_eq!(wants_view(&edit).1, " a\n-b\n+B\n c");
        // a leave with nothing to write (read, reach, run) has no such button; any other stop keeps its diff
        let mut read = leave.clone();
        (read.wants, read.title) = (None, "May the agent read `/etc/hosts`?".into());
        assert_eq!(diff_button(&read), None);
        assert_eq!(diff_button(&judging(&["a"])), Some("See the diff"));
        // and no line of what lies ahead: the merge asks by itself
        assert_eq!(ahead_line(&leave), None);
    }

    // Alan, 21/11: "tive que dar uma porrada de allow com o meu touchid"
    #[test]
    fn asked_touch_only_once_before_the_merge() {
        use jaynes_shell::stop::Lands;
        // a leave answered asks no touch, and says nothing of what lies ahead
        let mut leave = judging(&["tests/conftest.py"]);
        (leave.kind, leave.permission) = (QuestionKind::Permission, true);
        assert_eq!((crate::answer::touch_of(&leave), ahead_line(&leave), send_label(&leave, false)), (None, None, "Send".to_string()));
        // a decision before the merge: the merge is ahead and asks itself, so this answer asks no touch
        let mut before = judging(&["a"]);
        before.ahead = Some("the run merges into your branch".into());
        assert_eq!(crate::answer::touch_of(&before), None);
        assert_eq!(ahead_line(&before).as_deref(), Some("Ahead: the run merges into your branch. You look at it before it lands."));
        assert_eq!(send_label(&before, false), "Send");
        // the stop before the merge: what goes in, every file with its lines, each that runs later with its why
        let mut merge = judging(&["src/auth/login.go", "src/auth/login_test.go", "tests/conftest.py", "Makefile"]);
        (merge.kind, merge.offered, merge.back) = (QuestionKind::Merge, vec![VerdictKind::Keep, VerdictKind::Drop], vec![]);
        merge.title = "Merge into “main”?".into();
        merge.touch = Some("merge “fix-login” into main".into());
        merge.into = "main".into();
        let lands = |path: &str, added, removed, runs| Lands { path: path.into(), added, removed, runs, tests: None };
        merge.lands = vec![
            lands("src/auth/login.go", 64, 12, None),
            lands("src/auth/login_test.go", 88, 0, None),
            lands("tests/conftest.py", 14, 0, Some("runs on its own whenever the tests run")),
            lands("Makefile", 3, 1, Some("runs when you run make")),
        ];
        let l = landing(&merge);
        assert_eq!(l.goes_in, "What goes in · 4 files · +169 −13 lines");
        assert_eq!(l.rows, [
            ("src/auth/login.go".to_string(), "+64 −12".to_string(), false),
            ("src/auth/login_test.go".to_string(), "+88".to_string(), false),
            ("tests/conftest.py".to_string(), "+14".to_string(), true),
            ("Makefile".to_string(), "+3 −1".to_string(), true),
        ]);
        assert_eq!(l.more, None);
        assert_eq!(l.later, [
            ("tests/conftest.py".to_string(), "runs on its own whenever the tests run.".to_string()),
            ("Makefile".to_string(), "runs when you run make.".to_string()),
        ]);
        // the names are drawn in mono, never with raw backticks around them ("`Makefile` runs when you run make.", the
        // engineer lead at the floor-merge picture, 22/11)
        let drawn = l.later.iter().flat_map(|(n, h)| [n, h]).chain(l.rows.iter().flat_map(|(n, r, _)| [n, r])).chain([&l.goes_in]);
        for said in drawn {
            assert!(!said.contains('`'), "a visible backtick in {said:?}");
        }
        assert_eq!((RUNS_LATER, LATER_HEAD, READ_THESE, ONE_TOUCH), (
            "runs later outside any box",
            "Runs later outside any box",
            "Read these before you merge: they run on your machine, outside any box.",
            "Merging asks for your touch, once.",
        ));
        // no "Ahead" at the merge itself; its two ways out, the engine's words
        assert_eq!(ahead_line(&merge), None);
        assert_eq!([label_for(VerdictKind::Keep, merge.kind), label_for(VerdictKind::Drop, merge.kind)], ["Merge", "Branch only"]);
        // two files that run later, one answer for every file, and one touch for it
        for v in [VerdictKind::Keep, VerdictKind::Drop] {
            let body = crate::answer::all(&merge, v);
            let verdicts = body["verdicts"].as_object().expect("verdicts");
            assert_eq!(verdicts.len(), 4);
            assert!(verdicts.values().all(|w| w == if v == VerdictKind::Keep { "keep" } else { "drop" }), "{body}");
        }
        assert_eq!(crate::answer::touch_of(&merge), Some("merge “fix-login” into main"));
        // more than eight files: eight shown, the rest counted; with nothing that runs later, no amber part
        let mut many = merge.clone();
        many.lands = (1..=11).map(|i| lands(&format!("src/f{i}.go"), 2, 0, None)).collect();
        let l = landing(&many);
        assert_eq!((l.rows.len(), l.more.as_deref(), l.later.len()), (8, Some("… and 3 more"), 0));
        assert_eq!(l.goes_in, "What goes in · 11 files · +22 −0 lines");
    }

    // The real-user, 23/11 (the tech-lead's trava): the merge card said "1 FILE" while three landed, one of them
    // pyproject.toml, which changes how the tests run. The card lists every file that lands and says, in amber above
    // them, that one changes how the tests run, the file marked in its own row, the why dim beneath.
    #[test]
    fn asked_the_merge_card_lists_every_file_that_lands_and_marks_the_one_that_changes_how_the_tests_run() {
        use jaynes_shell::stop::Lands;
        let mut merge = judging(&["a"]);
        merge.kind = QuestionKind::Merge;
        let file = |path: &str, tests: Option<&str>| Lands { path: path.into(), added: 3, removed: 1, runs: None, tests: tests.map(str::to_string) };
        merge.lands = vec![file("app/main.py", None), file("tests/test_main.py", None), file("pyproject.toml", Some("its pytest settings change"))];
        merge.tests_run = jaynes_shell::stop::tests_run_said(&merge.lands);
        let l = landing(&merge);
        assert_eq!(l.goes_in, "What goes in · 3 files · +9 −3 lines");
        assert_eq!(l.rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["app/main.py", "tests/test_main.py", "pyproject.toml"]);
        assert_eq!(l.tests_run, Some(("It changes how your tests run: pyproject.toml (its pytest settings change).".to_string(), jaynes_shell::stop::TESTS_RUN_WHY.to_string())));
        assert_eq!(l.marked, ["pyproject.toml"]);
        assert_eq!(TESTS_RUN_MARK, "changes how your tests run");
        // nothing that changes how the tests run: nothing said, nothing marked
        merge.lands = vec![file("app/main.py", None)];
        merge.tests_run = None;
        let l = landing(&merge);
        assert_eq!((l.tests_run, l.marked.len()), (None, 0));
    }

    // Alan, 20/11: "o cursor no your words nao aparece quando quero escrever ai nao sei se o input ta com foco"
    #[test]
    fn asked_the_cursor_in_your_words_shows_so_i_know_the_field_has_focus() {
        // the caret is blue, never the field's own colour
        assert_eq!(caret(), crate::look::blue());
        assert_ne!(caret(), crate::look::panel());
        // with the keys: the border blue and the soft blue ring round it; without them, neither
        assert_eq!(field_look(true), (crate::look::blue(), true));
        assert_eq!(field_look(false), (crate::look::line(), false));
    }

    #[test]
    fn beside_a_send_that_will_not_go_the_foot_says_why() {
        // all judged, the engine would take it: the count, plain
        assert_eq!(foot_line(None, 3, 3, None), ("3 of 3 judged".to_string(), false));
        // all judged, a Change with no words: the engine's reason beside the Send it keeps off, not under the
        // middle's fold (the design-lead's a48d8b7 round, s3c/marked-all.png)
        assert_eq!(foot_line(None, 3, 3, Some("say what to change")), ("3 of 3 judged · Jaynes would refuse this: say what to change".to_string(), true));
        // nothing marked yet: no reason to give, the empty answer is not the person's
        assert_eq!(foot_line(None, 0, 3, Some("say what to change")), ("0 of 3 judged".to_string(), false));
        // sent, or not sent: that line, whatever the marks
        assert_eq!(foot_line(Some("Not sent: no key"), 3, 3, Some("say what to change")), ("Not sent: no key".to_string(), false));
    }

    #[test]
    fn the_card_grows_with_the_window_up_to_most_of_it_and_never_below_what_it_had() {
        use gpui::px;
        // the tab's space, less the room for the run's story to keep a few lines over it
        assert_eq!(card_cap(px(900.)), px(630.));
        assert_eq!(card_cap(px(1080.)), px(756.));
        // a small window: as tall as the card was before it grew
        assert_eq!(card_cap(px(640.)), px(460.));
    }

    #[test]
    fn the_words_field_taking_the_keys_comes_whole_above_the_fade() {
        use gpui::{Bounds, point, px, size};
        // as the hidden app laid it out at 1024x640: the middle from 270, 273 tall; the field's row 291.5 under its top
        let view = Bounds::new(point(px(307.), px(270.)), size(px(702.), px(273.)));
        let row = Bounds::new(point(px(323.), px(561.5)), size(px(670.), px(34.)));
        // "What this does" under it: the row's bottom comes up to the fade's top, not to the middle's edge under it
        assert_eq!(words_offset(view, Some(row), px(234.), px(0.)), Some(px(-84.5)));
        // nothing under it but the padding: as far as the middle goes, where no fade shows
        assert_eq!(words_offset(view, Some(row), px(24.5), px(0.)), Some(px(-24.5)));
        // already whole and clear of the fade: nothing moves
        assert_eq!(words_offset(view, Some(row), px(234.), px(-100.)), Some(px(-100.)));
        // scrolled past it, above: back to its top
        assert_eq!(words_offset(view, Some(row), px(600.), px(-320.)), Some(px(-291.5)));
        // a field taller than the room above the fade: its top first
        let tall = Bounds::new(point(px(323.), px(561.5)), size(px(670.), px(260.)));
        assert_eq!(words_offset(view, Some(tall), px(600.), px(0.)), Some(px(-291.5)));
        // the card not laid out yet: no answer, and the plain scroll to the row does
        assert_eq!(words_offset(view, None, px(234.), px(0.)), None);
    }

    #[test]
    fn the_fade_says_more_is_below_only_for_content_not_for_the_padding() {
        use gpui::px;
        assert!(fades_below(px(234.), px(-84.5)));
        // at the end, or with only the 12 of the middle's padding left under the view
        assert!(!fades_below(px(234.), px(-234.)));
        assert!(!fades_below(px(24.5), px(-12.5)));
        // nothing to scroll
        assert!(!fades_below(px(0.), px(0.)));
    }

    #[test]
    fn the_words_field_comes_into_view_when_it_takes_the_keys_and_only_then() {
        // the field takes the keys (a click, Tab, the driver): the middle scrolls it whole into view
        assert!(comes_into_view(false, true));
        // it keeps them: the person may scroll away to read, and the card leaves the view where they put it
        assert!(!comes_into_view(true, true));
        // it lets them go, or never had them: nothing moves
        assert!(!comes_into_view(true, false));
        assert!(!comes_into_view(false, false));
    }

    #[test]
    fn the_first_tab_from_outside_the_open_card_lands_on_its_first_control() {
        // the keys outside the card (the run's tab, the column): Tab goes to "See the diff", not to the window's first stop
        assert!(enters_card(Some(Shows::Card), false, false));
        // already inside: Tab goes round as before; Shift-Tab goes back from where it is
        assert!(!enters_card(Some(Shows::Card), true, false));
        assert!(!enters_card(Some(Shows::Card), false, true));
        // a pill has no card to enter
        assert!(!enters_card(Some(Shows::Pill), false, false));
    }
    use gpui_base::input::InputEvent;
    use std::time::{Duration, Instant};

    #[test]
    fn over_claude_codes_terminal_the_stop_is_only_its_pill_and_esc_goes_on_to_it() {
        // a run's tab: the card, open until the person folds it
        assert_eq!(shows(true, false), Shows::Card);
        assert_eq!(shows(true, true), Shows::Pill);
        // Claude Code's tab: the pill only, never over where the person types
        assert_eq!(shows(false, false), Shows::Pill);
        // Esc folds the card open, before the terminal under it ever sees it
        assert!(takes_esc(Shows::Card, false));
        // not with something over it (the diff, Command-K, the shortcuts), which close first; nor a pill
        assert!(!takes_esc(Shows::Card, true));
        assert!(!takes_esc(Shows::Pill, false));
    }

    #[test]
    fn the_card_the_pill_and_the_invitation_say_which_run_waits() {
        // a step's name always in curly quotes
        assert_eq!(card_head("Tighten the site's copy", "site", "copy", 1), ("Tighten the site's copy".to_string(), "site · at “copy” · round 1".to_string()));
        assert_eq!(card_head("Tighten the site's copy", "site", "copy", 0).1, "site · at “copy”");
        // no name read yet: the project leads, as before
        assert_eq!(card_head("", "site", "copy", 1), ("site".to_string(), "at “copy” · round 1".to_string()));
        assert_eq!(pill("Tighten the site's copy", "Which of these copy changes go in?"), "◆ Tighten the site's copy: Which of these copy changes go in? · open");
        assert_eq!(pill("", "Which of these copy changes go in?"), "◆ Which of these copy changes go in? · open");
        assert_eq!(DELIVERED, "Already delivered. Your answer can still correct it.");
        assert_eq!(diff_title("copy"), "Changes made at “copy”");
    }

    #[test]
    fn tab_reaches_the_pill_on_its_runs_own_tab_but_never_over_claude_codes_terminal() {
        assert!(tab_goes_round(Some(Shows::Card), true));
        // folded on its own tab: the pill is in the Tab order, and Enter or Space open it
        assert!(tab_goes_round(Some(Shows::Pill), true));
        // over Claude Code's terminal, Tab is the terminal's
        assert!(!tab_goes_round(Some(Shows::Pill), false));
        assert!(!tab_goes_round(None, true));
    }

    #[test]
    fn with_the_words_field_in_focus_esc_leaves_the_field_first() {
        assert_eq!(esc_chip(false), "Esc · later");
        assert_eq!(esc_chip(true), "Esc · leave the field");
    }

    #[test]
    fn what_this_does_shows_only_what_was_marked_and_one_line_for_the_rest() {
        let said: Vec<String> = ["“Hero” is kept.", "“Button” is dropped.", "“Footer” is kept.", "“Old” comes back."].map(String::from).to_vec();
        // nothing marked: nothing said
        assert!(said_lines(&said, &[false, false, false], false).is_empty());
        // the marked, one a line, then what comes back, then one line for the rest
        assert_eq!(said_lines(&said, &[false, true, false], false), vec!["“Button” is dropped.", "“Old” comes back.", "2 not judged yet"]);
        // Send would take them as they are (one brought back): it says so
        assert_eq!(said_lines(&said, &[false, true, false], true), vec!["“Button” is dropped.", "“Old” comes back.", "2 not judged: kept as they are"]);
        // all marked: no line for the rest
        assert_eq!(said_lines(&said, &[true, true, true], true).len(), 4);
        assert_eq!(not_judged(1, false), "1 not judged yet");
    }

    #[test]
    fn the_answer_sent_is_said_at_its_runs_foot_for_four_seconds() {
        let at = Instant::now();
        let sent = Sent { run: "r".into(), at, said: String::new() };
        assert_eq!(SENT, "Sent as yours. The run goes on.");
        assert!(sent.shows(Some("r"), at + Duration::from_millis(3900)));
        assert!(!sent.shows(Some("r"), at + Duration::from_millis(4100)));
        // on its run's tab only: not another run's, nor Claude Code's
        assert!(!sent.shows(Some("other"), at + Duration::from_secs(1)));
        assert!(!sent.shows(None, at + Duration::from_secs(1)));
    }

    #[test]
    fn enter_in_your_words_reads_them() {
        assert!(reads_on(&InputEvent::PressEnter { secondary: false, shift: false }));
        assert!(!reads_on(&InputEvent::Change));
    }

    /// The engineer lead (22/11): the merge card's See the changes showed the last step's diff alone, so a file an
    /// earlier step changed went in unseen. At the merge it shows what lands, the whole delivery, under the page's
    /// words ("What lands on “main” · 2 files"); at any other stop, the step's own change.
    #[test]
    fn asked_the_merge_shows_everything_that_lands() {
        let one = "diff --git a/b.py b/b.py\n+b\n".to_string();
        let step = Some(one.clone());
        let both = "diff --git a/a.py b/a.py\n+a\ndiff --git a/b.py b/b.py\n+b\n".to_string();
        let lands = Some(both.clone());
        let mut merge = judging(&["a.py", "b.py"]);
        (merge.kind, merge.step, merge.into) = (QuestionKind::Merge, "merge".into(), "main".into());
        assert_eq!(diff_shown(&merge, step.clone(), lands.clone()), ("What lands on “main” · 2 files".to_string(), both));
        merge.into = String::new();
        assert_eq!(diff_shown(&merge, step.clone(), Some("diff --git a/a.py b/a.py\n+a\n".into())).0, "What lands on your branch · 1 file");
        assert_eq!(diff_shown(&merge, step.clone(), None).1, "Nothing lands: the run's copy is gone or changed nothing.");
        // any other stop: the step's own change, as before
        let mut other = judging(&["a.py"]);
        other.step = "change".into();
        assert_eq!(diff_shown(&other, step.clone(), lands), ("Changes made at “change”".to_string(), one));
        assert_eq!(diff_shown(&other, None, None).1, "This step changed nothing in the project.");
    }
}
