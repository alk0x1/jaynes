//! The Changes panel, at the window's right in the familiar look: what git says about the folder the middle works in.
//! A commit box, then the files staged for the next commit (− takes one back), the files changed (+ stages one), the
//! new files git does not follow yet (+ adds one), and the commits not pushed. As VS Code does it: with nothing
//! staged, Commit takes every changed file git follows, never a new one. Shift-Command-G shows and hides it.
//! The folder is read again every two seconds (main.rs), and at once after the panel changes something.

use crate::diff::{self, FileDiff};
use crate::git::{self, Entry, Status};
use crate::tabs::Tabs;
use std::collections::HashMap;
use crate::look;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_base::input::{InputEvent, InputState};

pub struct Changes {
    /// The folder the middle works in; the repository is the one holding it.
    pub folder: String,
    /// None outside git, or before the first read.
    pub status: Option<Status>,
    message: Entity<InputState>,
    /// What git said when it refused the last change.
    error: Option<String>,
    /// What the last change did, when it says something worth seeing (Pushed.).
    note: Option<String>,
    tabs: Entity<Tabs>,
    /// The files stacked in the Changes tab, in the order the person clicked them.
    pub shown: Vec<String>,
    /// Their changes, read again with the folder.
    pub diffs: HashMap<String, FileDiff>,
    /// While a run's tab is in the middle: what its steps changed, shown in place of the folder, read only.
    pub run: Option<RunChange>,
}

impl Changes {
    pub fn new(tabs: Entity<Tabs>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // the Changes tab closed: the stack starts empty next time
        cx.observe(&tabs, |this: &mut Changes, tabs, cx| {
            if tabs.read(cx).changes.is_none() && !this.shown.is_empty() {
                this.shown.clear();
                cx.notify();
            }
        })
        .detach();
        let message = cx.new(|cx| InputState::new(window, cx).placeholder("Commit message"));
        cx.subscribe_in(&message, window, |this, _, e: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = e {
                // only what was staged goes with Enter: every changed file waits for a click on the button
                match &this.status {
                    Some(s) if !enter_commits(s) && !s.changed.is_empty() => {
                        this.note = Some(format!("Nothing is staged: {} takes every changed file, so it waits for a click.", commit_label(s)));
                        cx.notify();
                    }
                    _ => this.commit(window, cx),
                }
            }
        })
        .detach();
        Changes { folder: String::new(), status: None, message, error: None, note: None, tabs, shown: vec![], diffs: HashMap::new(), run: None }
    }

    /// The folder's status, read on the main loop's turn.
    pub fn set(&mut self, folder: String, status: Option<Status>, cx: &mut Context<Self>) {
        if self.folder != folder || self.status != status {
            if self.folder != folder {
                self.error = None;
            }
            self.folder = folder;
            self.status = status;
            // a file committed or put back leaves the stack
            let present: Vec<String> = self.status.iter().flat_map(|s| s.staged.iter().chain(s.changed.iter()).map(|e| e.path.clone()).chain(s.new.iter().cloned())).collect();
            let before = self.shown.len();
            self.shown.retain(|p| present.contains(p));
            if self.shown.len() != before {
                let n = self.shown.len();
                self.tabs.update(cx, |t, cx| t.changes_count(n, cx));
            }
            self.read_diffs(self.shown.clone(), cx);
            cx.notify();
        }
    }

    /// What the steps of the run in the middle changed, or None when no run's tab is there.
    pub fn set_run(&mut self, run: Option<RunChange>, cx: &mut Context<Self>) {
        if self.run != run {
            self.run = run;
            cx.notify();
        }
    }

    /// Reads the changes of these files off the window's thread.
    fn read_diffs(&self, paths: Vec<String>, cx: &mut Context<Self>) {
        let Some(status) = self.status.clone() else { return };
        if paths.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let read = cx.background_executor().spawn(async move { paths.iter().map(|p| (p.clone(), diff::read(&status.root, p, status.new.contains(p)))).collect::<Vec<_>>() });
            let read = read.await;
            let _ = this.update(cx, |c, cx| {
                c.diffs.extend(read);
                cx.notify();
            });
        })
        .detach();
    }

    /// Puts back the files stacked before the person left for their editor, and shows the tab again.
    pub fn restore(&mut self, shown: Vec<String>, cx: &mut Context<Self>) {
        if shown.is_empty() {
            return;
        }
        self.shown = shown;
        self.read_diffs(self.shown.clone(), cx);
        let n = self.shown.len();
        self.tabs.update(cx, |t, cx| t.show_changes(n, cx));
    }

    /// A click on a file: its changes go to the bottom of the Changes tab, which shows.
    pub fn open_file(&mut self, path: String, cx: &mut Context<Self>) {
        if stack(&mut self.shown, &path) {
            self.read_diffs(vec![path], cx);
        }
        let n = self.shown.len();
        self.tabs.update(cx, |t, cx| t.show_changes(n, cx));
        cx.notify();
    }

    /// × on a file in the tab: it leaves the stack; the tab goes with the last one.
    pub fn close_file(&mut self, path: &str, cx: &mut Context<Self>) {
        self.shown.retain(|p| p != path);
        let n = self.shown.len();
        self.tabs.update(cx, |t, cx| t.changes_count(n, cx));
        cx.notify();
    }

    /// The lines a file gained and lost, staged and not.
    pub fn entry(&self, path: &str) -> Option<Entry> {
        let s = self.status.as_ref()?;
        let mut found = s.staged.iter().chain(s.changed.iter()).filter(|e| e.path == path);
        let first = found.next()?.clone();
        Some(found.fold(first, |a, e| Entry { path: a.path, add: a.add.zip(e.add).map(|(x, y)| x + y), del: a.del.zip(e.del).map(|(x, y)| x + y) }))
    }

    /// Reads the folder again now, after the panel changed it.
    fn reload(&self, cx: &mut Context<Self>) {
        let folder = self.folder.clone();
        cx.spawn(async move |this, cx| {
            let status = cx.background_executor().spawn({
                let folder = folder.clone();
                async move { git::read(&folder) }
            });
            let status = status.await;
            let _ = this.update(cx, |c, cx| c.set(folder, status, cx));
        })
        .detach();
    }

    /// Runs a change to the repository off the window's thread, then reads it again; git's words stay when it refuses.
    fn act(&mut self, job: impl FnOnce(&str) -> Result<(), String> + Send + 'static, cx: &mut Context<Self>) {
        self.act_saying(job, None, cx);
    }

    /// As `act`, with a note to show when it went well.
    fn act_saying(&mut self, job: impl FnOnce(&str) -> Result<(), String> + Send + 'static, said: Option<String>, cx: &mut Context<Self>) {
        let Some(root) = self.status.as_ref().map(|s| s.root.clone()) else { return };
        self.note = None;
        cx.spawn(async move |this, cx| {
            let done = cx.background_executor().spawn(async move { job(&root) }).await;
            let _ = this.update(cx, |c, cx| {
                c.note = done.is_ok().then_some(said).flatten();
                c.error = done.err();
                c.reload(cx);
                // a piece moved between staged and not: the stack reads its files again
                c.read_diffs(c.shown.clone(), cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The + or − of a piece in the Changes tab: stages it, or takes it back when it is staged. A new file is staged
    /// whole.
    pub fn piece_act(&mut self, path: &str, i: usize, cx: &mut Context<Self>) {
        let Some(d) = self.diffs.get(path) else { return };
        let Some(p) = d.pieces.get(i) else { return };
        if d.new || p.patch.is_empty() {
            let path = path.to_owned();
            self.act(move |root| git::stage(root, &path), cx);
        } else {
            let (patch, back) = (p.patch.clone(), p.staged);
            self.act(move |root| git::apply(root, &patch, back), cx);
        }
    }

    /// P in the Changes tab: the commits not pushed go to the upstream.
    pub fn push(&mut self, cx: &mut Context<Self>) {
        let Some(status) = &self.status else { return };
        let Some(up) = status.upstream.clone() else {
            self.error = Some("This branch has no upstream to push to.".into());
            cx.notify();
            return;
        };
        self.act_saying(git::push, Some(format!("Pushed to {up}.")), cx);
    }

    /// c in the Changes tab: the cursor goes to the commit message.
    pub fn focus_message(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.message.update(cx, |m, cx| m.focus(window, cx));
    }

    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.message.read(cx).value().trim().to_string();
        let Some(status) = &self.status else { return };
        if text.is_empty() {
            self.error = Some("Write a commit message first.".into());
            cx.notify();
            return;
        }
        if status.staged.is_empty() && status.changed.is_empty() {
            self.error = Some("Nothing to commit: stage a new file with + first.".into());
            cx.notify();
            return;
        }
        let staged = !status.staged.is_empty();
        self.message.update(cx, |m, cx| m.set_value("", window, cx));
        self.act(move |root| git::commit(root, &text, staged), cx);
    }

    /// Whether the commit message has the keys.
    pub fn message_focused(&self, window: &Window, cx: &App) -> bool {
        self.message.read(cx).focus_handle(cx).is_focused(window)
    }

    /// How many files differ, for the switch in the tabs bar.
    /// How many files the switch says differ: the run's, while a run's tab is in the middle, else the folder's.
    pub fn count(&self, cx: &App) -> usize {
        match self.shows(cx) {
            Shows::Run(run) => run.map_or(0, RunChange::files),
            Shows::Folder => self.status.as_ref().map_or(0, Status::count),
        }
    }

    /// What the panel shows, from the tab in the middle at once: a run's change only while that run's tab is there.
    fn shows(&self, cx: &App) -> Shows<'_> {
        match self.tabs.read(cx).middle() {
            crate::tabs::Middle::Run(id) => Shows::Run(self.run.as_ref().filter(|r| r.id == id)),
            _ => Shows::Folder,
        }
    }
}

/// What the panel shows: the run's change (None while it is read), or the folder's git.
enum Shows<'a> {
    Run(Option<&'a RunChange>),
    Folder,
}

/// What the steps of a run changed, as its tab's panel shows it: from the run's own copy of the project, read only,
/// as the page shows it (drive::step_diff), never the folder's git.
#[derive(Clone, Debug, PartialEq)]
pub struct RunChange {
    pub id: String,
    /// The steps that changed something, in the run's order.
    pub steps: Vec<StepChange>,
}

impl RunChange {
    /// How many files its steps changed, each once.
    pub fn files(&self) -> usize {
        self.steps.iter().flat_map(|s| s.files.iter().map(|e| e.path.as_str())).collect::<std::collections::BTreeSet<_>>().len()
    }
}

/// What one step changed: its files with their lines gained and lost, and the lines to show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StepChange {
    pub step: String,
    pub files: Vec<Entry>,
    pub lines: Vec<DiffLine>,
}

/// A line of a step's change as the panel draws it.
#[derive(Clone, Debug, PartialEq)]
pub enum DiffLine {
    /// A file's head: the index of its entry in `files`.
    File(usize),
    Piece(String),
    Added(String),
    Removed(String),
    Kept(String),
}

/// The title over what a step changed, in the page's words (page.rs changes_title).
pub fn step_title(step: &str, n: usize) -> String {
    format!("Changes made at “{step}” · {n} file{}", if n == 1 { "" } else { "s" })
}

/// A step's change from its unified diff; None when it changed nothing.
pub fn step_change(step: &str, text: &str) -> Option<StepChange> {
    let mut c = StepChange { step: step.to_owned(), ..Default::default() };
    for l in text.lines() {
        if let Some(rest) = l.strip_prefix("diff --git ") {
            let path = rest.split(" b/").last().unwrap_or(rest).to_owned();
            c.files.push(Entry { path, add: Some(0), del: Some(0) });
            c.lines.push(DiffLine::File(c.files.len() - 1));
            continue;
        }
        let Some(file) = c.files.last_mut() else { continue };
        if l.starts_with("Binary files ") {
            (file.add, file.del) = (None, None);
        } else if l.starts_with("+++ ") || l.starts_with("--- ") || l.starts_with("index ") || l.starts_with("new file mode") || l.starts_with("deleted file mode") || l.starts_with("similarity ") || l.starts_with("rename ") {
        } else if l.starts_with("@@") {
            c.lines.push(DiffLine::Piece(l.to_owned()));
        } else if let Some(t) = l.strip_prefix('+') {
            file.add = file.add.map(|n| n + 1);
            c.lines.push(DiffLine::Added(t.to_owned()));
        } else if let Some(t) = l.strip_prefix('-') {
            file.del = file.del.map(|n| n + 1);
            c.lines.push(DiffLine::Removed(t.to_owned()));
        } else {
            c.lines.push(DiffLine::Kept(l.strip_prefix(' ').unwrap_or(l).to_owned()));
        }
    }
    (!c.files.is_empty()).then_some(c)
}

/// What each of these steps of a run changed, off the window's thread: the steps that changed nothing are left out.
pub fn read_run(home: &std::path::Path, id: &str, steps: &[String]) -> RunChange {
    RunChange { id: id.to_owned(), steps: steps.iter().filter_map(|s| step_change(s, &jaynes_shell::drive::step_diff(home, id, s)?)).collect() }
}

/// How many lines of a step's change the panel draws; the page and Open live version show the rest.
const RUN_LINES: usize = 400;

/// The lines a file gained and lost, as "+118 −34"; "binary" when git does not count it in lines.
fn counts(e: &Entry) -> Div {
    let mono = div().flex_none().flex().gap(px(5.)).font_family(look::MONO_FONT).text_size(z(11.5));
    match (e.add, e.del) {
        (Some(a), Some(d)) => mono.child(div().text_color(rgb(look::green())).child(format!("+{a}"))).child(div().text_color(rgb(look::red())).child(format!("−{d}"))),
        _ => mono.text_color(rgb(look::faint())).child("binary"),
    }
}

/// A path split as the panel shows it: the file's name, then its folder, quieter.
fn split(path: &str) -> (String, String) {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some((dir, name)) => (format!("{name}{}", if path.ends_with('/') { "/" } else { "" }), dir.to_owned()),
        None => (path.to_owned(), String::new()),
    }
}

/// The + or − at a row's end: a plain line icon, lit under the pointer, as VS Code draws them.
fn act_icon(id: SharedString, icon: look::Icon) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(22.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .cursor_pointer()
        .text_color(rgb(look::dim()))
        .hover(|d| d.bg(rgba(look::hover())))
        .child(look::icon(icon, 16., look::dim()))
}

fn heading(text: &str, right: String) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .px(px(16.))
        .pt(px(4.))
        .pb(px(8.))
        .child(div().flex_none().text_size(z(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::text())).child(text.to_string()))
        .child(div().min_w_0().truncate().text_size(z(12.)).text_color(rgb(look::faint())).child(right))
}

fn section(text: &str) -> Div {
    div().px(px(16.)).pt(px(10.)).pb(px(4.)).text_size(z(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::faint())).child(text.to_string())
}

/// A file's row: a click stacks its changes in the Changes tab; a blue dot while they show there.
fn row(path: &str, shown: bool, cx: &mut Context<Changes>) -> Stateful<Div> {
    let (name, dir) = split(path);
    let open = path.to_owned();
    div()
        .id(SharedString::from(format!("file-{path}")))
        .relative()
        .cursor_pointer()
        .hover(|d| d.bg(rgba(look::hover_faint())))
        .when(shown, |d| d.bg(rgb(look::selected())).child(div().absolute().left(px(6.)).top(px(11.)).size(px(6.)).rounded_full().bg(rgb(look::blue()))))
        .on_click(cx.listener(move |this, _, _, cx| this.open_file(open.clone(), cx)))
        .flex()
        .items_center()
        .gap(px(8.))
        .pl(px(16.))
        .pr(px(10.))
        .py(px(3.))
        .text_size(z(12.5))
        .child(div().flex_none().max_w(px(200.)).truncate().text_color(rgb(look::text())).child(name))
        .child(div().min_w_0().flex_1().truncate().text_size(z(11.5)).text_color(rgb(look::faint())).child(dir))
}

impl Changes {
    /// The panel while a run's tab is in the middle: what each step changed, read only, with no commit box: the run's
    /// change is delivered by Jaynes, never committed here.
    fn run_panel(&self, frame: Stateful<Div>, run: &RunChange) -> Stateful<Div> {
        let mut panel = frame.child(heading("Changes", "this run · read only".into()));
        if run.steps.is_empty() {
            return panel.child(div().px(px(16.)).text_size(z(12.5)).text_color(rgb(look::faint())).child("No step of this run has changed a file yet."));
        }
        for s in &run.steps {
            panel = panel.child(section(&step_title(&s.step, s.files.len())));
            let mono = |t: &str, color: u32| {
                div().px(px(16.)).font_family(look::MONO_FONT).text_size(z(11.5)).line_height(z(17.)).text_color(rgb(color)).truncate().child(if t.is_empty() { " ".to_owned() } else { t.to_owned() })
            };
            for l in s.lines.iter().take(RUN_LINES) {
                panel = panel.child(match l {
                    DiffLine::File(i) => {
                        let e = &s.files[*i];
                        let (name, dir) = split(&e.path);
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(16.))
                            .pt(px(8.))
                            .pb(px(2.))
                            .text_size(z(12.5))
                            .child(div().flex_none().max_w(px(200.)).truncate().text_color(rgb(look::text())).child(name))
                            .child(div().min_w_0().flex_1().truncate().text_size(z(11.5)).text_color(rgb(look::faint())).child(dir))
                            .child(counts(e))
                    }
                    DiffLine::Piece(t) => mono(t, look::faint()),
                    DiffLine::Added(t) => mono(&format!("+{t}"), look::green()),
                    DiffLine::Removed(t) => mono(&format!("-{t}"), look::red()),
                    DiffLine::Kept(t) => mono(&format!(" {t}"), look::dim()),
                });
            }
            if s.lines.len() > RUN_LINES {
                panel = panel.child(div().px(px(16.)).py(px(6.)).text_size(z(12.)).text_color(rgb(look::faint())).child(format!("… {} more lines: the page shows them all", s.lines.len() - RUN_LINES)));
            }
        }
        panel
    }
}

impl Render for Changes {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame = div()
            .id("changes")
            // its zoom keys (main.rs): with the commit message in hand too
            .key_context("Changes")
            .w_full()
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .py(px(12.))
            .font_family(look::UI_FONT)
            .bg(rgb(look::panel()))
            .border_l_1()
            .border_color(rgb(look::line()));
        let home = std::env::var("HOME").unwrap_or_default();
        if let Shows::Run(run) = self.shows(cx) {
            return match run {
                Some(run) => self.run_panel(frame, run),
                None => frame.child(heading("Changes", "this run · read only".into())).child(div().px(px(16.)).text_size(z(12.5)).text_color(rgb(look::faint())).child("Reading…")),
            };
        }
        let place = crate::home_short(&self.folder, &home);
        let Some(status) = self.status.clone() else {
            return frame.child(heading("Changes", place)).child(div().px(px(16.)).text_size(z(12.5)).text_color(rgb(look::faint())).child("Not a git folder."));
        };
        let (add, del) = status.staged.iter().chain(status.changed.iter()).fold((0, 0), |(a, d), e| (a + e.add.unwrap_or(0), d + e.del.unwrap_or(0)));
        let n = status.count();
        let summary = if n == 0 { format!("{place} · nothing changed") } else { format!("{place} · {n} file{} · +{add} −{del}", if n == 1 { "" } else { "s" }) };
        let mut panel = frame.child(heading("Changes", summary)).child(
            div()
                .flex()
                .gap(px(8.))
                .px(px(16.))
                .pb(px(6.))
                .child(div().flex_1().min_w_0().child(crate::island::focus_field("commit-message", &self.message, window, cx)))
                .child(
                    div()
                        .id("commit")
                        .flex_none()
                        .flex()
                        .items_center()
                        .px(px(12.))
                        .rounded(px(7.))
                        .border_1()
                        .border_color(rgb(look::line_2()))
                        .bg(rgb(look::panel_2()))
                        .text_size(z(12.5))
                        .text_color(rgb(look::text()))
                        .cursor_pointer()
                        .hover(|d| d.bg(rgb(look::selected())))
                        .on_click(cx.listener(|this, _, window, cx| this.commit(window, cx)))
                        .child(self.status.as_ref().map(commit_label).unwrap_or_else(|| "Commit".into())),
                ),
        );
        if let Some(e) = &self.error {
            panel = panel.child(div().px(px(16.)).pb(px(6.)).text_size(z(12.)).text_color(rgb(look::red())).child(e.clone()));
        }
        if let Some(n) = &self.note {
            panel = panel.child(div().px(px(16.)).pb(px(6.)).text_size(z(12.)).text_color(rgb(look::green())).child(n.clone()));
        }
        if !status.staged.is_empty() {
            panel = panel.child(section("Staged"));
            for e in &status.staged {
                let path = e.path.clone();
                panel = panel.child(row(&e.path, self.shown.contains(&e.path), cx).child(counts(e)).child(
                    act_icon(SharedString::from(format!("unstage-{}", e.path)), look::Icon::Minus).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        let path = path.clone();
                        this.act(move |root| git::unstage(root, &path), cx);
                    })),
                ));
            }
        }
        if !status.changed.is_empty() {
            panel = panel.child(section("Changes"));
            for e in &status.changed {
                let path = e.path.clone();
                panel = panel.child(row(&e.path, self.shown.contains(&e.path), cx).child(counts(e)).child(
                    act_icon(SharedString::from(format!("stage-{}", e.path)), look::Icon::Plus).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        let path = path.clone();
                        this.act(move |root| git::stage(root, &path), cx);
                    })),
                ));
            }
        }
        if !status.new.is_empty() {
            panel = panel.child(section("New files, not in git yet"));
            for p in &status.new {
                let path = p.clone();
                panel = panel.child(row(p, self.shown.contains(p), cx).child(div().flex_none().text_size(z(11.5)).text_color(rgb(look::amber())).child("new")).child(
                    act_icon(SharedString::from(format!("add-{p}")), look::Icon::Plus).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        let path = path.clone();
                        this.act(move |root| git::stage(root, &path), cx);
                    })),
                ));
            }
        }
        let pushed = match &status.upstream {
            None => "no upstream".to_string(),
            Some(u) if status.unpushed.is_empty() => format!("{u} · none"),
            Some(u) => format!("{u} · {}", status.unpushed.len()),
        };
        panel = panel.child(div().h(px(10.))).child(heading("Not pushed", pushed));
        for (hash, subject) in &status.unpushed {
            panel = panel.child(
                div()
                    .flex()
                    .gap(px(10.))
                    .px(px(16.))
                    .py(px(3.))
                    .text_size(z(12.))
                    .child(div().flex_none().font_family(look::MONO_FONT).text_size(z(11.5)).text_color(rgb(look::faint())).child(hash.clone()))
                    .child(div().min_w_0().truncate().text_color(rgb(look::dim())).child(subject.clone())),
            );
        }
        panel
    }
}

/// The Changes tab in the middle: the files clicked in the panel, one under the other, each piece with Before and
/// After side by side and only what changed lit, staged pieces first. Each piece has its + (stage it) or − (take it
/// back); a blue border marks the piece the keys act on, as magit's cursor: n and p move it, s stages, u takes back,
/// c goes to the commit message, P pushes, Esc closes the tab. The keys show at the bottom.
pub struct Stack {
    changes: Entity<Changes>,
    pub focus: FocusHandle,
    /// The piece the keys act on, counted across the files.
    cursor: usize,
    /// The file and line the person asked to edit (e, or a file's open icon), for the window to open.
    pub edit: Option<(String, u32)>,
}

impl Stack {
    pub fn new(changes: Entity<Changes>, cx: &mut Context<Self>) -> Self {
        cx.observe(&changes, |_, _, cx| cx.notify()).detach();
        Stack { changes, focus: cx.focus_handle(), cursor: 0, edit: None }
    }

    /// Every piece in the stack, in order: its file and its place in it.
    fn pieces(&self, cx: &App) -> Vec<(String, usize)> {
        let c = self.changes.read(cx);
        c.shown.iter().flat_map(|p| (0..c.diffs.get(p).map_or(0, |d| if d.binary { 0 } else { d.pieces.len() })).map(move |i| (p.clone(), i))).collect()
    }

    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = &e.keystroke.modifiers;
        if m.platform || m.control || m.alt {
            return;
        }
        let Some(act) = key_act(e.keystroke.key.as_str(), m.shift) else { return };
        let all = self.pieces(cx);
        let at = all.get(self.cursor.min(all.len().saturating_sub(1))).cloned();
        match act {
            Key::Next => self.cursor = (self.cursor + 1).min(all.len().saturating_sub(1)),
            Key::Previous => self.cursor = self.cursor.saturating_sub(1),
            Key::Stage | Key::Unstage => {
                let staged = act == Key::Unstage;
                if let Some((path, i)) = at {
                    // s stages only what is not staged, u takes back only what is
                    let fits = self.changes.read(cx).diffs.get(&path).and_then(|d| d.pieces.get(i)).is_some_and(|p| p.staged == staged);
                    if fits {
                        self.changes.update(cx, |c, cx| c.piece_act(&path, i, cx));
                    }
                }
            }
            Key::Commit => window.dispatch_action(Box::new(crate::CommitBox), cx),
            Key::Edit => {
                if let Some((path, i)) = at {
                    let line = self.changes.read(cx).diffs.get(&path).and_then(|d| d.pieces.get(i)).map_or(1, |p| p.after.first().or(p.before.first()).map_or(1, |l| l.n));
                    self.edit = Some((path, line));
                    window.dispatch_action(Box::new(crate::OpenEditor), cx);
                }
            }
            Key::Push => self.changes.update(cx, |c, cx| c.push(cx)),
            Key::Close => window.dispatch_action(Box::new(crate::CloseTab), cx),
        }
        cx.stop_propagation();
        cx.notify();
    }
}

/// A line's text with its changed words lit in a colour on a soft fill of it.
fn line_text(parts: &[(String, bool)], colour: u32) -> StyledText {
    let mut text = String::new();
    let mut lit = vec![];
    for (s, on) in parts {
        let at = text.len();
        text.push_str(s);
        if *on {
            lit.push((at..text.len(), HighlightStyle { color: Some(rgb(colour).into()), background_color: Some(rgba((colour << 8) | 0x26).into()), ..Default::default() }));
        }
    }
    StyledText::new(text).with_highlights(lit)
}

/// One side of a piece: its label, then its lines with their numbers; "(nothing here)" for a side with none.
fn side(label: &'static str, lines: &[diff::Line], colour: u32) -> Div {
    let mut col = div().flex_1().min_w_0().py(px(4.)).flex().flex_col().child(div().px(px(10.)).pb(px(2.)).font_family(look::UI_FONT).text_size(z(10.5)).text_color(rgb(look::faint())).child(label));
    if lines.is_empty() {
        col = col.child(div().pl(px(44.)).text_color(rgb(look::faint())).child("(nothing here)"));
    }
    for l in lines {
        col = col.child(
            div()
                .flex()
                .px(px(10.))
                .child(div().w(px(34.)).flex_none().text_color(rgb(look::faint())).child(l.n.to_string()))
                .child(div().flex_1().min_w_0().text_color(rgb(look::dim())).child(line_text(&l.parts, colour))),
        );
    }
    col
}

/// A file clicked in the panel goes to the bottom of the stack the Changes tab shows, once: whether it is new there.
pub fn stack(shown: &mut Vec<String>, path: &str) -> bool {
    if shown.iter().any(|p| p == path) {
        return false;
    }
    shown.push(path.to_owned());
    true
}

/// What a key does in the Changes tab, as magit's keys: n and p (and the arrows) move between the pieces, s stages,
/// u takes back, c goes to the commit message, P pushes, e opens the person's editor, Esc closes the tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Next,
    Previous,
    Stage,
    Unstage,
    Commit,
    Push,
    Edit,
    Close,
}

/// The key typed (as gpui names it) and Shift; None for a key the tab leaves to the window.
pub fn key_act(key: &str, shift: bool) -> Option<Key> {
    Some(match (key, shift) {
        ("n" | "down", false) => Key::Next,
        ("p" | "up", false) => Key::Previous,
        ("s", false) => Key::Stage,
        ("u", false) => Key::Unstage,
        ("c", false) => Key::Commit,
        ("p", true) => Key::Push,
        ("e", false) => Key::Edit,
        ("escape", _) => Key::Close,
        _ => return None,
    })
}

/// The keys the line at the tab's bottom shows, as typed, and what each does.
pub const KEYS: &[(&str, &str)] =
    &[("s", "stage"), ("u", "unstage"), ("n p", "next, previous piece"), ("c", "commit"), ("P", "push"), ("e", "edit in your editor"), ("Esc", "close")];

/// The keys the Changes tab answers, at its bottom, as magit's menu shows what works.
fn keys_line() -> Div {
    let key = |k: &str, what: &str| div().flex().gap(px(4.)).child(div().font_family(look::MONO_FONT).text_color(rgb(look::text())).child(k.to_string())).child(what.to_string());
    div()
        .flex_none()
        .flex()
        .flex_wrap()
        .gap_x(px(14.))
        .gap_y(px(4.))
        .px(px(18.))
        .py(px(8.))
        .border_t_1()
        .border_color(rgb(look::line()))
        .text_size(z(11.5))
        .text_color(rgb(look::faint()))
        .children(KEYS.iter().map(|(k, what)| key(k, what)))
}

impl Render for Stack {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let all = self.pieces(cx);
        self.cursor = self.cursor.min(all.len().saturating_sub(1));
        let cursor = all.get(self.cursor).cloned();
        let changes = self.changes.read(cx);
        let mut page = div().id("changes-stack").flex_1().min_h_0().overflow_y_scroll().px(px(18.)).py(px(8.)).flex().flex_col();
        for (k, path) in changes.shown.iter().enumerate() {
            let (name, dir) = split(path);
            let d = changes.diffs.get(path);
            let place = d.map(|d| if d.new { "a new file".to_string() } else { d.places() }).unwrap_or_default();
            let close = {
                let (changes, path) = (self.changes.clone(), path.clone());
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| changes.update(cx, |c, cx| c.close_file(&path, cx))
            };
            if k > 0 {
                page = page.child(div().h(px(1.)).mt(px(14.)).bg(rgb(look::line())));
            }
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pt(px(12.))
                    .pb(px(4.))
                    .text_size(z(12.5))
                    .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::text())).child(name))
                    .child(div().flex_none().text_size(z(11.5)).text_color(rgb(look::faint())).child(dir))
                    .child(div().ml(px(6.)).min_w_0().flex_1().truncate().text_size(z(12.)).text_color(rgb(look::dim())).child(place))
                    .children(changes.entry(path).map(|e| counts(&e)))
                    .child(act_icon(SharedString::from(format!("edit-{path}")), look::Icon::Open).on_click(cx.listener({
                        let path = path.clone();
                        move |this, _, window, cx| {
                            this.edit = Some((path.clone(), 1));
                            window.dispatch_action(Box::new(crate::OpenEditor), cx);
                        }
                    })))
                    .child(act_icon(SharedString::from(format!("close-{path}")), look::Icon::Close).on_click(close)),
            );
            let Some(d) = d else {
                page = page.child(div().py(px(6.)).text_size(z(12.)).text_color(rgb(look::faint())).child("Reading…"));
                continue;
            };
            if d.binary {
                page = page.child(div().py(px(6.)).text_size(z(12.)).text_color(rgb(look::faint())).child("A binary file: git does not show its lines."));
                continue;
            }
            if d.pieces.is_empty() {
                page = page.child(div().py(px(6.)).text_size(z(12.)).text_color(rgb(look::faint())).child(if d.new { "A new folder: add it with + to see its files." } else { "No changes left." }));
                continue;
            }
            for (i, p) in d.pieces.iter().enumerate() {
                let here = cursor.as_ref().is_some_and(|(cp, ci)| cp == path && *ci == i);
                let at = all.iter().position(|(ap, ai)| ap == path && *ai == i).unwrap_or(0);
                let act = {
                    let (changes, path) = (self.changes.clone(), path.clone());
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        cx.stop_propagation();
                        changes.update(cx, |c, cx| c.piece_act(&path, i, cx));
                    }
                };
                page = page.child(
                    div()
                        .id(SharedString::from(format!("piece-{path}-{i}")))
                        .relative()
                        .mt(px(6.))
                        .flex()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(rgb(if here { look::blue() } else { look::line_2() }))
                        .font_family(look::MONO_FONT)
                        .text_size(z(12.))
                        .line_height(z(19.5))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.cursor = at;
                            window.focus(&this.focus, cx);
                            cx.notify();
                        }))
                        .child(side(if p.staged { "Before · staged" } else { "Before" }, &p.before, look::red()))
                        .child(div().w(px(1.)).flex_none().bg(rgb(look::line_2())))
                        .child(side("After", &p.after, look::green()))
                        // its + or −, at the top right corner
                        .child(div().absolute().top(px(3.)).right(px(4.)).child(act_icon(SharedString::from(format!("piece-act-{path}-{i}")), if p.staged { look::Icon::Minus } else { look::Icon::Plus }).on_click(act))),
                );
            }
            if d.cut > 0 {
                page = page.child(div().py(px(6.)).text_size(z(12.)).text_color(rgb(look::faint())).child(format!("… {} more lines", d.cut)));
            }
        }
        div()
            .id("changes-tab")
            .track_focus(&self.focus)
            .key_context("Changes")
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| this.key(e, window, cx)))
            .size_full()
            .flex()
            .flex_col()
            .font_family(look::UI_FONT)
            .child(page.child(div().h(px(24.))))
            .child(keys_line())
    }
}

/// A size of Changes' text at the zoom the person left it (prefs changes_zoom, Command-plus and Command-minus there).
fn z(v: f32) -> Pixels {
    px(v * crate::prefs::get().changes_zoom)
}

/// What the commit button says: how many files the commit takes, the staged ones or, with none staged, every changed
/// file git follows (as `git commit -a`).
pub fn commit_label(s: &crate::git::Status) -> String {
    let n = if s.staged.is_empty() { s.changed.len() } else { s.staged.len() };
    match n {
        0 => "Commit".into(),
        1 => "Commit 1 file".into(),
        n => format!("Commit {n} files"),
    }
}

/// Whether Enter in the commit message commits: only with files staged; taking every changed file needs a click.
pub fn enter_commits(s: &crate::git::Status) -> bool {
    !s.staged.is_empty()
}

#[cfg(test)]
mod tests {
    use super::{commit_label, enter_commits, key_act, read_run, split, stack, step_change, step_title, DiffLine, Key, KEYS};
    use crate::git::{Entry, Status};

    #[test]
    fn a_run_s_tab_shows_what_its_steps_changed_read_only_never_not_a_git_folder() {
        // the page's words over a step's change
        assert_eq!(step_title("directions", 3), "Changes made at “directions” · 3 files");
        assert_eq!(step_title("copy", 1), "Changes made at “copy” · 1 file");
        let text = "diff --git a/site/index.html b/site/index.html\nindex 1..2 100644\n--- a/site/index.html\n+++ b/site/index.html\n@@ -1,2 +1,2 @@ <body>\n <h1>Notes</h1>\n-<p>Start free</p>\n+<p>Try it free</p>\ndiff --git a/logo.png b/logo.png\nnew file mode 100644\nBinary files /dev/null and b/logo.png differ\n";
        let c = step_change("copy", text).expect("a change");
        assert_eq!(c.files.iter().map(|e| (e.path.as_str(), e.add, e.del)).collect::<Vec<_>>(), [("site/index.html", Some(1), Some(1)), ("logo.png", None, None)]);
        assert_eq!(
            c.lines,
            [DiffLine::File(0), DiffLine::Piece("@@ -1,2 +1,2 @@ <body>".into()), DiffLine::Kept("<h1>Notes</h1>".into()), DiffLine::Removed("<p>Start free</p>".into()), DiffLine::Added("<p>Try it free</p>".into()), DiffLine::File(1)]
        );
        assert_eq!(step_change("plan", ""), None);
        // from the run's own record, as the page reads it: a step that changed nothing is left out, and a run
        // without a copy shows no step, never the folder's git
        let home = std::env::temp_dir().join(format!("jaynes-run-changes-{}", std::process::id()));
        let saved = home.join("runs/r1/changes");
        std::fs::create_dir_all(&saved).unwrap();
        std::fs::write(saved.join("copy.diff"), text).unwrap();
        std::fs::write(saved.join("plan.diff"), "").unwrap();
        let run = read_run(&home, "r1", &["plan".into(), "copy".into(), "write".into()]);
        assert_eq!((run.id.as_str(), run.steps.iter().map(|s| s.step.as_str()).collect::<Vec<_>>()), ("r1", vec!["copy"]));
        // the switch over the panel counts the run's files, each once, not the folder's
        std::fs::write(saved.join("write.diff"), "diff --git a/site/index.html b/site/index.html\n@@ -1 +1 @@\n-a\n+b\n").unwrap();
        assert_eq!(read_run(&home, "r1", &["copy".into(), "write".into()]).files(), 2);
        assert!(read_run(&home, "no-such-run", &["copy".into()]).steps.is_empty());
        std::fs::remove_dir_all(&home).unwrap();
    }

    fn status(staged: &[&str], changed: &[&str]) -> Status {
        let e = |p: &&str| Entry { path: p.to_string(), add: None, del: None };
        Status { staged: staged.iter().map(e).collect(), changed: changed.iter().map(e).collect(), ..Status::default() }
    }

    #[test]
    fn the_commit_button_says_how_many_files_it_takes() {
        // staged: those
        assert_eq!(commit_label(&status(&["a", "b"], &["c"])), "Commit 2 files");
        assert_eq!(commit_label(&status(&["a"], &[])), "Commit 1 file");
        // nothing staged: every changed file git follows, as git commit -a
        assert_eq!(commit_label(&status(&[], &["c", "d", "e"])), "Commit 3 files");
        assert_eq!(commit_label(&status(&[], &[])), "Commit");
    }

    #[test]
    fn enter_commits_only_what_was_staged_and_taking_every_changed_file_needs_a_click() {
        assert!(enter_commits(&status(&["a"], &["c"])));
        assert!(!enter_commits(&status(&[], &["c", "d"])), "a commit of everything changed is never one Enter away");
        assert!(!enter_commits(&status(&[], &[])));
    }

    // Alan, 17/11, the Changes as in app-changes.html: "painel simples como o VS Code" (his words were not kept: this
    // is how the app's memory of 17/11 tells it, perhaps a paraphrase; the goal, "magit's power with VS Code's
    // simplicity"). A file shows by its name, its folder after it, faint, as VS Code lists them.
    #[test]
    fn asked_a_simple_panel_like_vs_code_a_file_by_its_name_then_its_folder() {
        assert_eq!(split("app/src/main.rs"), ("main.rs".into(), "app/src".into()));
        assert_eq!(split("research.org"), ("research.org".into(), String::new()));
        assert_eq!(split("docs/audits/"), ("audits/".into(), "docs".into()));
    }

    // Alan, 17/11, the Changes as in app-changes.html: "clicar empilha os diffs numa aba" (from the app's memory of
    // 17/11, perhaps a paraphrase)
    #[test]
    fn asked_a_click_stacks_the_diffs_in_one_tab() {
        let mut shown = vec![];
        assert!(stack(&mut shown, "app/src/main.rs"));
        assert!(stack(&mut shown, "app/src/rail.rs"));
        // the next file under the last, and a file clicked again stays where it was, once
        assert!(!stack(&mut shown, "app/src/main.rs"));
        assert_eq!(shown, ["app/src/main.rs", "app/src/rail.rs"]);
    }

    // Alan, 17/11, the Changes as in app-changes.html: "teclas do magit" (from the app's memory of 17/11, perhaps a
    // paraphrase; the goal, "magit's power with VS Code's simplicity")
    #[test]
    fn asked_magit_keys_in_the_changes_tab() {
        assert_eq!(key_act("n", false), Some(Key::Next));
        assert_eq!(key_act("p", false), Some(Key::Previous));
        assert_eq!(key_act("s", false), Some(Key::Stage));
        assert_eq!(key_act("u", false), Some(Key::Unstage));
        assert_eq!(key_act("c", false), Some(Key::Commit));
        assert_eq!(key_act("p", true), Some(Key::Push));
        assert_eq!(key_act("e", false), Some(Key::Edit));
        assert_eq!(key_act("escape", false), Some(Key::Close));
        assert_eq!(key_act("x", false), None);
        // every key the line at the bottom shows does something
        for (typed, what) in KEYS {
            for k in typed.split(' ') {
                let (k, shift) = match k {
                    "Esc" => ("escape", false),
                    "P" => ("p", true),
                    k => (k, false),
                };
                assert!(key_act(k, shift).is_some(), "{typed} ({what}) does nothing");
            }
        }
    }
}
