//! A finished run's summary drawn, not shown raw. The engine writes it in a small part of Markdown (`jaynes summary`):
//! "## " its title, "### " a heading, "- " and "1. " items, "**bold**" and "`code`" inside a line, plain lines and
//! empty ones. Those are drawn as what they are; anything else is drawn as it is written. Copy as Markdown still
//! copies the text itself.

use crate::look;
use gpui::*;

/// A piece of a line.
#[derive(Clone, Debug, PartialEq)]
pub enum Span {
    Plain(String),
    Bold(String),
    Code(String),
}

/// A line of the summary, as drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Title(Vec<Span>),
    Heading(Vec<Span>),
    /// A list's item: its mark ("•", or "3." in a numbered list) and its text.
    Item(String, Vec<Span>),
    Line(Vec<Span>),
    /// Room between parts, for one or more empty lines.
    Gap,
}

pub fn blocks(md: &str) -> Vec<Block> {
    let mut out = Vec::new();
    for line in md.lines() {
        let t = line.trim_end();
        let block = if t.trim().is_empty() {
            Block::Gap
        } else if let Some(r) = t.strip_prefix("## ") {
            Block::Title(spans(r))
        } else if let Some(r) = t.strip_prefix("### ") {
            Block::Heading(spans(r))
        } else if let Some(r) = t.strip_prefix("- ") {
            Block::Item("•".into(), spans(r))
        } else if let Some((n, r)) = numbered(t) {
            Block::Item(format!("{n}."), spans(r))
        } else {
            Block::Line(spans(t))
        };
        // one gap between parts, none first
        if block == Block::Gap && matches!(out.last(), None | Some(Block::Gap)) {
            continue;
        }
        out.push(block);
    }
    while out.last() == Some(&Block::Gap) {
        out.pop();
    }
    out
}

/// "12. the rest": its number and the rest.
fn numbered(t: &str) -> Option<(&str, &str)> {
    let (n, rest) = t.split_once(". ")?;
    (!n.is_empty() && n.len() <= 3 && n.chars().all(|c| c.is_ascii_digit())).then_some((n, rest))
}

/// A line's pieces: "`code`" and "**bold**" where they open and close on the line; a mark that does not close is
/// text.
pub fn spans(line: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut rest = line;
    let flush = |plain: &mut String, out: &mut Vec<Span>| {
        if !plain.is_empty() {
            out.push(Span::Plain(std::mem::take(plain)));
        }
    };
    while let Some(c) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix('`')
            && let Some(end) = after.find('`')
        {
            flush(&mut plain, &mut out);
            out.push(Span::Code(after[..end].to_owned()));
            rest = &after[end + 1..];
            continue;
        }
        if let Some(after) = rest.strip_prefix("**")
            && let Some(end) = after.find("**").filter(|&e| e > 0)
        {
            flush(&mut plain, &mut out);
            out.push(Span::Bold(after[..end].to_owned()));
            rest = &after[end + 2..];
            continue;
        }
        plain.push(c);
        rest = &rest[c.len_utf8()..];
    }
    flush(&mut plain, &mut out);
    out
}

/// A line's pieces as text in the app's fonts: bold in the interface's, code in the mono one on a faint ground.
fn styled(spans: &[Span], colour: u32, weight: FontWeight) -> StyledText {
    let mut text = String::new();
    let mut runs = Vec::new();
    for span in spans {
        let (s, font) = match span {
            Span::Plain(s) => (s, Font { weight, ..font(look::UI_FONT) }),
            Span::Bold(s) => (s, Font { weight: FontWeight::SEMIBOLD, ..font(look::UI_FONT) }),
            Span::Code(s) => (s, font(look::MONO_FONT)),
        };
        text.push_str(s);
        let ground = matches!(span, Span::Code(_)).then(|| rgb(look::line()).into());
        runs.push(TextRun { len: s.len(), font, color: rgb(colour).into(), background_color: ground, underline: None, strikethrough: None });
    }
    StyledText::new(text).with_runs(runs)
}

/// The summary as the app draws it.
pub fn draw(md: &str) -> Div {
    let mut out = div().flex().flex_col().text_size(px(13.)).line_height(px(20.));
    for block in blocks(md) {
        out = out.child(match block {
            Block::Title(s) => div().text_size(px(15.)).line_height(px(22.)).child(styled(&s, look::text(), FontWeight::SEMIBOLD)),
            Block::Heading(s) => div().pt(px(4.)).child(styled(&s, look::text(), FontWeight::SEMIBOLD)),
            Block::Item(mark, s) => div()
                .flex()
                .gap(px(6.))
                .child(div().flex_none().min_w(px(16.)).text_color(rgb(look::faint())).child(mark))
                .child(div().flex_1().min_w_0().child(styled(&s, look::dim(), FontWeight::NORMAL))),
            Block::Line(s) => div().child(styled(&s, look::dim(), FontWeight::NORMAL)),
            Block::Gap => div().h(px(8.)),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    // named, not `super::*`: that brings gpui's own `test` attribute in, over the standard one
    use super::{Block, Span, blocks, spans};

    fn plain(s: &str) -> Span {
        Span::Plain(s.into())
    }

    #[test]
    fn the_summary_is_read_as_its_parts() {
        let md = "## Fix the date picker\n\n### Delivered\n\nA branch, in my-app. The files Jaynes measured:\n\n- `src/date.rs`: 12 lines changed\n- and 2 more files\n\n\n### Steps\n\n1. **plan** (agent): checked, 1 attempt.\n10. **merge** (person): accepted.\n";
        assert_eq!(
            blocks(md),
            [
                Block::Title(vec![plain("Fix the date picker")]),
                Block::Gap,
                Block::Heading(vec![plain("Delivered")]),
                Block::Gap,
                Block::Line(vec![plain("A branch, in my-app. The files Jaynes measured:")]),
                Block::Gap,
                Block::Item("•".into(), vec![Span::Code("src/date.rs".into()), plain(": 12 lines changed")]),
                Block::Item("•".into(), vec![plain("and 2 more files")]),
                Block::Gap,
                Block::Heading(vec![plain("Steps")]),
                Block::Gap,
                Block::Item("1.".into(), vec![Span::Bold("plan".into()), plain(" (agent): checked, 1 attempt.")]),
                Block::Item("10.".into(), vec![Span::Bold("merge".into()), plain(" (person): accepted.")]),
            ]
        );
    }

    #[test]
    fn a_mark_that_does_not_close_is_text() {
        assert_eq!(spans("a `b and **c"), [plain("a `b and **c")]);
        assert_eq!(spans("****"), [plain("****")]);
        assert_eq!(spans("Run `r1`: role fix-bug"), [plain("Run "), Span::Code("r1".into()), plain(": role fix-bug")]);
        // not a numbered item: a sentence that ends with a number
        assert_eq!(blocks("Version 2. Then more")[0], Block::Line(vec![plain("Version 2. Then more")]));
        assert_eq!(blocks("2026. A year")[0], Block::Line(vec![plain("2026. A year")]));
        // nothing but empty lines draws nothing
        assert!(blocks("\n\n  \n").is_empty());
    }
}
