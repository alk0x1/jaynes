//! The Changes panel, at the window's right in the familiar look: what git says about the folder the middle works in.
//! A commit box, then the files staged for the next commit (− takes one back), the files changed (+ stages one), the
//! new files git does not follow yet (+ adds one), and the commits not pushed. As VS Code does it: with nothing
//! staged, Commit takes every changed file git follows, never a new one. Shift-Command-G shows and hides it.
//! The folder is read again every two seconds (main.rs), and at once after the panel changes something.

use crate::git::{self, Entry, Status};
use crate::look;
use gpui::*;
use gpui_base::input::{InputEvent, InputState};

pub const WIDTH: f32 = 360.;

pub struct Changes {
    /// The folder the middle works in; the repository is the one holding it.
    pub folder: String,
    /// None outside git, or before the first read.
    pub status: Option<Status>,
    message: Entity<InputState>,
    /// What git said when it refused the last change.
    error: Option<String>,
}

impl Changes {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| InputState::new(window, cx).placeholder("Commit message"));
        cx.subscribe_in(&message, window, |this, _, e: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = e {
                this.commit(window, cx);
            }
        })
        .detach();
        Changes { folder: String::new(), status: None, message, error: None }
    }

    /// The folder's status, read on the main loop's turn.
    pub fn set(&mut self, folder: String, status: Option<Status>, cx: &mut Context<Self>) {
        if self.folder != folder || self.status != status {
            if self.folder != folder {
                self.error = None;
            }
            self.folder = folder;
            self.status = status;
            cx.notify();
        }
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
        let Some(root) = self.status.as_ref().map(|s| s.root.clone()) else { return };
        cx.spawn(async move |this, cx| {
            let done = cx.background_executor().spawn(async move { job(&root) }).await;
            let _ = this.update(cx, |c, cx| {
                c.error = done.err();
                c.reload(cx);
                cx.notify();
            });
        })
        .detach();
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

    /// How many files differ, for the switch in the tabs bar.
    pub fn count(&self) -> usize {
        self.status.as_ref().map_or(0, Status::count)
    }
}

/// The lines a file gained and lost, as "+118 −34"; "binary" when git does not count it in lines.
fn counts(e: &Entry) -> Div {
    let mono = div().flex_none().flex().gap(px(5.)).font_family(look::MONO_FONT).text_size(px(11.5));
    match (e.add, e.del) {
        (Some(a), Some(d)) => mono.child(div().text_color(rgb(look::GREEN)).child(format!("+{a}"))).child(div().text_color(rgb(look::RED)).child(format!("−{d}"))),
        _ => mono.text_color(rgb(look::FAINT)).child("binary"),
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
        .text_color(rgb(look::DIM))
        .hover(|d| d.bg(rgba(0xffffff14)))
        .child(look::icon(icon, 16., look::DIM))
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
        .child(div().flex_none().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::TEXT)).child(text.to_string()))
        .child(div().min_w_0().truncate().text_size(px(12.)).text_color(rgb(look::FAINT)).child(right))
}

fn section(text: &str) -> Div {
    div().px(px(16.)).pt(px(10.)).pb(px(4.)).text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(look::FAINT)).child(text.to_string())
}

fn row(name: String, dir: String) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .pl(px(16.))
        .pr(px(10.))
        .py(px(3.))
        .text_size(px(12.5))
        .child(div().flex_none().max_w(px(200.)).truncate().text_color(rgb(look::TEXT)).child(name))
        .child(div().min_w_0().flex_1().truncate().text_size(px(11.5)).text_color(rgb(look::FAINT)).child(dir))
}

impl Render for Changes {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame = div()
            .id("changes")
            .w(px(WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .py(px(12.))
            .font_family(look::UI_FONT)
            .bg(rgb(look::PANEL))
            .border_l_1()
            .border_color(rgb(look::LINE));
        let home = std::env::var("HOME").unwrap_or_default();
        let place = crate::home_short(&self.folder, &home);
        let Some(status) = self.status.clone() else {
            return frame.child(heading("Changes", place)).child(div().px(px(16.)).text_size(px(12.5)).text_color(rgb(look::FAINT)).child("Not a git folder."));
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
                .child(div().flex_1().min_w_0().child(crate::island::field(&self.message)))
                .child(
                    div()
                        .id("commit")
                        .flex_none()
                        .flex()
                        .items_center()
                        .px(px(12.))
                        .rounded(px(7.))
                        .border_1()
                        .border_color(rgb(look::LINE_2))
                        .bg(rgb(look::PANEL_2))
                        .text_size(px(12.5))
                        .text_color(rgb(look::TEXT))
                        .cursor_pointer()
                        .hover(|d| d.bg(rgb(look::SELECTED)))
                        .on_click(cx.listener(|this, _, window, cx| this.commit(window, cx)))
                        .child("Commit"),
                ),
        );
        if let Some(e) = &self.error {
            panel = panel.child(div().px(px(16.)).pb(px(6.)).text_size(px(12.)).text_color(rgb(look::RED)).child(e.clone()));
        }
        if !status.staged.is_empty() {
            panel = panel.child(section("Staged"));
            for e in &status.staged {
                let (name, dir) = split(&e.path);
                let path = e.path.clone();
                panel = panel.child(row(name, dir).child(counts(e)).child(
                    act_icon(SharedString::from(format!("unstage-{}", e.path)), look::Icon::Minus).on_click(cx.listener(move |this, _, _, cx| {
                        let path = path.clone();
                        this.act(move |root| git::unstage(root, &path), cx);
                    })),
                ));
            }
        }
        if !status.changed.is_empty() {
            panel = panel.child(section("Changes"));
            for e in &status.changed {
                let (name, dir) = split(&e.path);
                let path = e.path.clone();
                panel = panel.child(row(name, dir).child(counts(e)).child(
                    act_icon(SharedString::from(format!("stage-{}", e.path)), look::Icon::Plus).on_click(cx.listener(move |this, _, _, cx| {
                        let path = path.clone();
                        this.act(move |root| git::stage(root, &path), cx);
                    })),
                ));
            }
        }
        if !status.new.is_empty() {
            panel = panel.child(section("New files, not in git yet"));
            for p in &status.new {
                let (name, dir) = split(p);
                let path = p.clone();
                panel = panel.child(row(name, dir).child(div().flex_none().text_size(px(11.5)).text_color(rgb(look::AMBER)).child("new")).child(
                    act_icon(SharedString::from(format!("add-{p}")), look::Icon::Plus).on_click(cx.listener(move |this, _, _, cx| {
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
                    .text_size(px(12.))
                    .child(div().flex_none().font_family(look::MONO_FONT).text_size(px(11.5)).text_color(rgb(look::FAINT)).child(hash.clone()))
                    .child(div().min_w_0().truncate().text_color(rgb(look::DIM)).child(subject.clone())),
            );
        }
        panel
    }
}

#[cfg(test)]
mod tests {
    use super::split;

    #[test]
    fn a_path_shows_its_name_then_its_folder() {
        assert_eq!(split("app/src/main.rs"), ("main.rs".into(), "app/src".into()));
        assert_eq!(split("research.org"), ("research.org".into(), String::new()));
        assert_eq!(split("docs/audits/"), ("audits/".into(), "docs".into()));
    }
}
