//! The card at the top of the tab of a run that delivered on a branch of its own: where its change is, the branch
//! with Copy, and Merge when the engine takes it; once merged, that it was, with Undo the merge (the design-lead's
//! quatro-itens.md, item 5; the tech-lead's T7, 23/11: "Toda run entregue numa branch e não mergeada mostra a linha
//! ... com Copy, e o botão Merge de um toque por cima quando o merge é possível ... o app é teu, com as mesmas
//! palavras"). The words are the core's (jaynes_shell::listing::Unmerged), as on the page.

use crate::island;
use crate::look;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use jaynes_core::run::Run;
use jaynes_shell::listing::{self, Unmerged};
use std::path::Path;

/// Where a delivered run's change is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Branch {
    /// On its branch, not merged into the person's.
    Unmerged(Unmerged),
    /// Merged: "Merged into “main” in horta. Nothing was pushed."
    Merged(String),
}

/// `Branch` of run `r` of the record in `home`: None for a run that delivered nothing on a branch, or whose merge was
/// undone.
pub fn of(home: &Path, r: &Run) -> Option<Branch> {
    if let Some(u) = listing::unmerged(home, r) {
        return Some(Branch::Unmerged(u));
    }
    (r.merged && !r.undone && r.delivered.is_some()).then(|| {
        let into = listing::into_of(home, &r.id);
        Branch::Merged(listing::merged_said(if into.is_empty() { "your branch" } else { &into }, &project_of(home, &r.id)))
    })
}

/// The project's folder name, from the run's setup, as `listing::unmerged` says it.
fn project_of(home: &Path, run: &str) -> String {
    let setup = jaynes_shell::store::Store::open(&home.join("record.sqlite3")).and_then(|s| s.setup(run)).unwrap_or_default();
    setup["project"].as_str().and_then(|p| Path::new(p).file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "your project".into())
}

/// What the card shows, each part only when it says something.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    /// The first line, the branch in the middle, in mono, with Copy.
    pub line: Option<(String, String, String)>,
    /// "Merge into “main”", primary, and the dim sentence beside it.
    pub button: Option<(String, String)>,
    /// Amber, when the last merge did not land.
    pub not_landed: Option<String>,
    /// git's own words, under "▸ What git said", closed; never in the sentence.
    pub git: Option<String>,
    /// Once merged, in place of the branch's line, with Undo the merge.
    pub merged: Option<String>,
}

pub fn shown(b: &Branch) -> Shown {
    match b {
        Branch::Unmerged(u) => {
            let (amber, git) = u.not_landed().map_or((None, None), |(a, g)| (Some(a), Some(g)));
            Shown { line: Some(u.line()), button: u.button().map(|b| (b, u.says())), not_landed: amber, git: git.filter(|g| !g.trim().is_empty()), merged: None }
        }
        Branch::Merged(said) => Shown { merged: Some(said.clone()), ..Shown::default() },
    }
}

/// What a press on one of the card's controls does.
pub type Press = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// What the card's controls do: copy the branch, merge, undo the merge, open or close git's words.
pub struct On {
    pub copy: Press,
    pub merge: Press,
    pub undo: Press,
    pub git: Press,
}

/// How the card stands: Copy just pressed, git's words open, Undo armed for its second press, and what the engine
/// said when it refused the last press.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub copied: bool,
    pub git_open: bool,
    pub undo_armed: bool,
    pub refused: Option<String>,
}

/// The card, as the design-lead drew it: the line in `text`, the branch in mono, Copy at the right; Merge, the
/// page's primary, with its sentence in `dim`; no amber unless a merge did not land.
pub fn card(b: &Branch, s: &State, on: On) -> Div {
    let shown = shown(b);
    let text = |t: String| div().text_size(px(13.)).text_color(rgb(look::text())).child(t);
    let On { copy, merge, undo, git } = on;
    div()
        .mx(px(14.))
        .mt(px(10.))
        .mb(px(10.))
        .px(px(16.))
        .py(px(14.))
        .rounded(px(10.))
        .border_1()
        .border_color(rgb(look::line()))
        .bg(rgb(look::panel_2()))
        .flex()
        .flex_col()
        .gap(px(10.))
        .when_some(shown.line, |d, (before, branch, after)| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        // the sentence's parts touch, as one line; it wraps before the Copy, never under it
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .child(text(before))
                            .child(div().font_family(look::MONO_FONT).text_size(px(12.5)).text_color(rgb(look::text())).child(branch))
                            .child(text(after)),
                    )
                    .child(island::button("branch-copy".into(), if s.copied { "Copied" } else { "Copy" }, false, look::grey()).on_click(copy)),
            )
        })
        .when_some(shown.button, |d, (label, says)| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(12.))
                    .child(island::primary("branch-merge", label, true).on_click(merge))
                    .child(div().flex_1().min_w(px(160.)).text_size(px(12.5)).text_color(rgb(look::dim())).child(says)),
            )
        })
        .when_some(shown.not_landed, |d, amber| d.child(div().text_size(px(12.5)).text_color(rgb(look::amber())).child(amber)))
        .when_some(shown.git, |d, words| {
            let open = s.git_open;
            d.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .id("branch-git")
                            .cursor_pointer()
                            .text_size(px(11.5))
                            .text_color(rgb(look::faint()))
                            .hover(|d| d.text_color(rgb(look::dim())))
                            .child(format!("{} {}", if open { "▾" } else { "▸" }, listing::WHAT_GIT_SAID))
                            .on_click(git),
                    )
                    .when(open, |d| d.child(div().pl(px(12.)).font_family(look::MONO_FONT).text_size(px(12.)).line_height(px(18.)).text_color(rgb(look::faint())).child(words))),
            )
        })
        .when_some(shown.merged, |d, said| {
            let label = if s.undo_armed { "Undo the merge: press again" } else { "Undo the merge" };
            d.child(div().flex().items_center().gap(px(12.)).child(div().flex_1().min_w_0().child(text(said))).child(island::button("branch-undo".into(), label, false, look::red()).on_click(undo)))
        })
        .when_some(s.refused.clone(), |d, said| d.child(div().text_size(px(12.)).text_color(rgb(look::red())).child(said)))
}

#[cfg(test)]
mod tests {
    use super::{Branch, Shown, shown};
    use jaynes_shell::listing::{NotLanded, Unmerged};

    fn unmerged() -> Unmerged {
        Unmerged { branch: "jaynes/same-2".into(), into: "main".into(), project: "horta".into(), refused: None, failed: None }
    }

    /// The tech-lead, T7, 23/11: "Toda run entregue numa branch e não mergeada mostra a linha \"The change is on the
    /// branch “…”, not in your folder yet\", com Copy, e o botão Merge de um toque por cima quando o merge é possível.
    /// ... o app é teu, com as mesmas palavras." The design-lead's quatro-itens.md, item 5: after the merge, "Merged
    /// into “main” in horta. Nothing was pushed." with Undo the merge; a checkout with changes keeps the button, a
    /// conflict takes it away; "O texto do git nunca vai na frase: fica num ▸ What git said, fechado".
    #[test]
    fn asked_a_run_delivered_on_a_branch_says_where_its_change_is_with_copy_and_merge_in_the_pages_words() {
        let s = shown(&Branch::Unmerged(unmerged()));
        assert_eq!(s.line, Some(("The change is on the branch “".into(), "jaynes/same-2".into(), "”, not in your folder yet.".into())));
        assert_eq!(s.button, Some(("Merge into “main”".into(), "It lands on “main” in horta. Nothing is pushed, and Undo takes it back.".into())));
        assert_eq!((s.not_landed, s.git, s.merged), (None, None, None));
        // the engine refuses Merge now: the line and its Copy, no button
        let refused = shown(&Branch::Unmerged(Unmerged { refused: Some("a stop is open".into()), ..unmerged() }));
        assert!(refused.line.is_some() && refused.button.is_none());
        // a checkout with changes: the button stays, to merge again; Jaynes's own check has no words of git's
        let dirty = shown(&Branch::Unmerged(Unmerged { failed: Some(NotLanded::Dirty { git: String::new() }), ..unmerged() }));
        assert!(dirty.button.is_some());
        assert_eq!(dirty.not_landed.as_deref(), Some("Not merged: your checkout of “main” has changes you have not committed. Commit or stash them, then merge again."));
        assert_eq!(dirty.git, None);
        // a conflict: no button, the branch to merge by hand, git's words apart
        let conflict = shown(&Branch::Unmerged(Unmerged { failed: Some(NotLanded::Conflict { git: "CONFLICT (content): Merge conflict in README.md".into() }), ..unmerged() }));
        assert!(conflict.button.is_none() && conflict.line.is_some());
        assert_eq!(conflict.not_landed.as_deref(), Some("Not merged: the change conflicts with what “main” has now. The branch “jaynes/same-2” still has it, so you can merge it by hand."));
        assert_eq!(conflict.git.as_deref(), Some("CONFLICT (content): Merge conflict in README.md"));
        // merged: that line alone, with Undo the merge
        let merged = shown(&Branch::Merged(jaynes_shell::listing::merged_said("main", "horta")));
        assert_eq!(merged, Shown { merged: Some("Merged into “main” in horta. Nothing was pushed.".into()), ..Shown::default() });
    }
}
