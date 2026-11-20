//! Command-K: find any agent, run or command by typing, the familiar look's palette. Its list is the Inbox, every
//! agent and shell of every space, every run, then the commands the app has, each with its key; typing keeps those
//! whose name (and space) hold its letters in order.

use crate::agents::{Agent, Space};
use crate::look;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_base::input::{Input, InputEvent, InputState};
use jaynes_shell::listing::{Listing, Mark};

/// Where an entry goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Go {
    Inbox,
    /// An agent or a shell, by its address.
    Agent(String),
    Run(String),
    NewAgent,
    OpenWaiting,
    /// The page in the browser, with its key (open_page.rs).
    OpenPage,
    ToggleChanges,
    Shortcuts,
}

impl Go {
    /// Whether it is one of the app's commands, drawn with › whether it has a key or not; the Inbox, an agent and a
    /// run are places.
    pub fn is_command(&self) -> bool {
        !matches!(self, Go::Inbox | Go::Agent(_) | Go::Run(_))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub label: String,
    /// The space it is in, shown at the right.
    pub space: String,
    pub go: Go,
    /// A command's key, shown at the right instead.
    pub key: Option<&'static str>,
    /// The agent it opens, for its mark as the column draws it; a run's mark.
    pub agent: Option<Agent>,
    pub run: Option<Mark>,
}

/// Whether `q`'s letters stand in `s` in order, case aside.
pub fn fuzzy(q: &str, s: &str) -> bool {
    let mut want = q.chars().flat_map(char::to_lowercase).peekable();
    for c in s.chars().flat_map(char::to_lowercase) {
        if want.peek() == Some(&c) {
            want.next();
        }
    }
    want.peek().is_none()
}

/// The entries `q` finds, by name or space, in the list's order.
pub fn matching<'a>(all: &'a [Entry], q: &str) -> Vec<&'a Entry> {
    all.iter().filter(|e| fuzzy(q, &format!("{} {}", e.label, e.space))).collect()
}

/// Everything Command-K lists: the Inbox, every agent and shell of every space, every run, then the commands, "Open
/// the page" among them.
pub fn entries(spaces: &[Space], runs: &Listing, here: Option<&str>) -> Vec<Entry> {
    let mut out = vec![Entry { label: "Inbox".into(), space: String::new(), go: Go::Inbox, key: None, agent: None, run: None }];
    for s in spaces {
        for a in &s.agents {
            out.push(Entry { label: a.title.clone(), space: s.name.clone(), go: Go::Agent(a.address.clone()), key: None, agent: Some(a.clone()), run: None });
        }
    }
    for r in runs.needs.iter().chain(runs.projects.iter().flat_map(|(_, rows)| rows.iter())) {
        if !out.iter().any(|e| e.go == Go::Run(r.id.clone())) {
            out.push(Entry { label: format!("run {}", r.title), space: String::new(), go: Go::Run(r.id.clone()), key: None, agent: None, run: Some(r.mark) });
        }
    }
    let new_in = here.map(|s| format!("New agent in {s}")).unwrap_or_else(|| "New agent".into());
    out.extend([
        Entry { label: new_in, space: String::new(), go: Go::NewAgent, key: Some("⌘T"), agent: None, run: None },
        Entry { label: "Open the next thing that needs you".into(), space: String::new(), go: Go::OpenWaiting, key: Some("⌘O"), agent: None, run: None },
        Entry { label: crate::open_page::LABEL.into(), space: String::new(), go: Go::OpenPage, key: None, agent: None, run: None },
        Entry { label: "Show or hide Changes".into(), space: String::new(), go: Go::ToggleChanges, key: Some("⇧⌘G"), agent: None, run: None },
        Entry { label: "Keyboard shortcuts".into(), space: String::new(), go: Go::Shortcuts, key: Some("⌘/"), agent: None, run: None },
    ]);
    out
}

/// How many entries show at once.
const SHOWN: usize = 10;

/// What the palette tells the window: go there, or close.
pub enum Said {
    Go(Go),
    Close,
}

impl EventEmitter<Said> for PaletteView {}

pub struct PaletteView {
    all: Vec<Entry>,
    input: Entity<InputState>,
    /// The entry the keys are on, among those shown.
    at: usize,
    focus: FocusHandle,
}

impl PaletteView {
    pub fn new(all: Vec<Entry>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search agents, spaces and commands"));
        cx.subscribe_in(&input, window, |this, _, e: &InputEvent, _, cx| match e {
            InputEvent::Change => {
                this.at = 0;
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.go(cx),
            _ => {}
        })
        .detach();
        input.update(cx, |i, cx| i.focus(window, cx));
        PaletteView { all, input, at: 0, focus: cx.focus_handle() }
    }

    fn query(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    fn shown(&self, cx: &App) -> Vec<Entry> {
        matching(&self.all, &self.query(cx)).into_iter().take(SHOWN).cloned().collect()
    }

    /// The letters typed, as typing them does: for a test.
    pub fn typed(&mut self, q: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |i, cx| i.set_value(q.to_string(), window, cx));
        self.at = 0;
        cx.notify();
    }

    /// Down (1) or up (-1) the entries shown.
    pub fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let n = self.shown(cx).len();
        if n > 0 {
            self.at = (self.at as isize + by).clamp(0, n as isize - 1) as usize;
            cx.notify();
        }
    }

    /// Return: the entry the keys are on.
    pub fn go(&mut self, cx: &mut Context<Self>) {
        match self.shown(cx).get(self.at) {
            Some(e) => cx.emit(Said::Go(e.go.clone())),
            None => cx.emit(Said::Close),
        }
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(Said::Close);
    }
}

/// A run's mark in the palette: a dot of its colour.
fn run_dot(m: Mark) -> AnyElement {
    div().size(px(7.)).rounded_full().bg(rgb(look::colour(m))).into_any_element()
}

impl Render for PaletteView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = self.shown(cx);
        let mut list = div().flex().flex_col().py(px(4.));
        for (k, e) in shown.iter().enumerate() {
            let on = k == self.at;
            let mark: AnyElement = if e.go.is_command() {
                div().w(px(12.)).text_color(rgb(look::faint())).child("›").into_any_element()
            } else if let Some(a) = &e.agent {
                crate::side::agent_icon(a, "palette")
            } else if let Some(m) = e.run {
                run_dot(m)
            } else {
                look::icon(look::Icon::Inbox, 13., look::faint()).into_any_element()
            };
            let right = match e.key {
                Some(key) if e.go.is_command() => look::kbd(key).into_any_element(),
                _ => div().text_color(rgb(look::faint())).child(e.space.clone()).into_any_element(),
            };
            let go = e.go.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("palette-{k}")))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(14.))
                    .py(px(7.))
                    .cursor_pointer()
                    .text_color(rgb(if on { look::text() } else { look::dim() }))
                    .when(on, |d| d.bg(rgb(look::selected())))
                    .child(div().w(px(14.)).flex_none().flex().items_center().justify_center().child(mark))
                    .child(div().flex_1().min_w_0().truncate().child(e.label.clone()))
                    .child(div().flex_none().text_size(px(12.)).child(right))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(Said::Go(go.clone())))),
            );
        }
        if shown.is_empty() {
            list = list.child(div().px(px(14.)).py(px(8.)).text_color(rgb(look::faint())).child("Nothing by that name"));
        }
        // the scrim closes it on a click outside the box, as Esc does
        div()
            .id("palette")
            .absolute()
            .inset_0()
            .bg(rgba(look::scrim_2()))
            .flex()
            .justify_center()
            .items_start()
            .pt(px(110.))
            .track_focus(&self.focus)
            .on_click(cx.listener(|this, _, _, cx| this.close(cx)))
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| match e.keystroke.key.as_str() {
                "down" => {
                    this.step(1, cx);
                    cx.stop_propagation();
                }
                "up" => {
                    this.step(-1, cx);
                    cx.stop_propagation();
                }
                "escape" => {
                    this.close(cx);
                    cx.stop_propagation();
                }
                _ => {}
            }))
            .child(
                div()
                    .id("palette-box")
                    .w(px(560.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(rgb(look::line_2()))
                    .bg(rgb(look::panel_2()))
                    .overflow_hidden()
                    .font_family(look::UI_FONT)
                    .text_size(px(13.))
                    .shadow_lg()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(14.))
                            .py(px(12.))
                            .border_b_1()
                            .border_color(rgb(look::line_2()))
                            .text_color(rgb(look::faint()))
                            .child("⌕")
                            .child(div().flex_1().text_color(rgb(look::text())).child(Input::new(&self.input))),
                    )
                    .child(list),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{Entry, Go, fuzzy, matching};

    fn entry(label: &str, space: &str, go: Go) -> Entry {
        Entry { label: label.into(), space: space.into(), go, key: None, agent: None, run: None }
    }

    #[test]
    fn it_lists_the_inbox_every_agent_every_run_once_then_the_commands() {
        use crate::agents::{Agent, Space, State};
        use jaynes_shell::listing::{Listing, Mark, Row};
        let agent = |a: &str| Agent { address: a.into(), title: a.to_uppercase(), state: State::Idle, since_ms: 0, shell: false, turn: 0, kind: None, msg: String::new(), cwd: String::new() };
        let spaces = [Space { id: "s".into(), name: "helm".into(), folder: String::new(), agents: vec![agent("t1"), agent("t2")], tabs: vec![] }];
        let row = |id: &str, mark| Row { id: id.into(), title: format!("Run {id}"), line: String::new(), mark };
        // a run that waits shows in the list of those that wait and in its project's: once here
        let runs = Listing { needs: vec![row("r1", Mark::Needs)], projects: vec![("p".into(), vec![row("r1", Mark::Needs), row("r2", Mark::Merged)])] };
        let all = super::entries(&spaces, &runs, Some("helm"));
        let labels: Vec<&str> = all.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["Inbox", "T1", "T2", "run Run r1", "run Run r2", "New agent in helm", "Open the next thing that needs you", "Open the page", "Show or hide Changes", "Keyboard shortcuts"]);
        assert_eq!(all[7].go, Go::OpenPage);
        // every command is drawn as one, with › on its left, a key or not; the places are not commands
        let commands: Vec<&str> = all.iter().filter(|e| e.go.is_command()).map(|e| e.label.as_str()).collect();
        assert_eq!(commands, ["New agent in helm", "Open the next thing that needs you", "Open the page", "Show or hide Changes", "Keyboard shortcuts"]);
        assert_eq!(all[1].space, "helm");
        assert_eq!(all[5].key, Some("⌘T"));
        assert_eq!(super::entries(&spaces, &runs, None)[5].label, "New agent");
    }

    #[test]
    fn its_letters_in_order_find_a_name() {
        assert!(fuzzy("hdj", "handoff de jaynes"));
        assert!(fuzzy("", "anything"));
        assert!(!fuzzy("jh", "handoff de jaynes"));
        assert!(fuzzy("BAL", "Identidade do Ballast"), "case does not count");
    }

    #[test]
    fn typing_keeps_what_matches_by_name_or_space_in_the_lists_order() {
        let all = vec![
            entry("Inbox", "", Go::Inbox),
            entry("Handoff de jaynes", "jaynes/tenure", Go::Agent("t1".into())),
            entry("zsh", "helm", Go::Agent("t2".into())),
            entry("run Identidade do Ballast", "", Go::Run("r1".into())),
        ];
        let names = |q: &str| matching(&all, q).iter().map(|e| e.label.clone()).collect::<Vec<_>>();
        assert_eq!(names("").len(), 4);
        assert_eq!(names("helm"), ["zsh"], "a space's name finds its agents");
        assert_eq!(names("ball"), ["run Identidade do Ballast"]);
        assert!(names("qqq").is_empty());
    }
}
