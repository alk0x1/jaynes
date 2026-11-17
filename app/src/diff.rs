//! A file's changes as the Changes tab shows them, after difftastic: each piece git finds, its lines Before and After,
//! with only the words that changed lit. The words are compared without the spaces between them, so a reindent, a
//! line broken in two or code moved into a block is not a change; what is lit is what was really added or taken out.
//! git gives the pieces (`git diff -U0`); the words are matched here, longest common sequence first.

use crate::spawn;
use std::process::Stdio;

/// A line of a piece: its number in the file, and its text in parts, each lit or not.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub n: u32,
    pub parts: Vec<(String, bool)>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Piece {
    /// What git says the piece is in: the function or block heading it, as "impl Render for Root"; may be empty.
    pub at: String,
    pub before: Vec<Line>,
    pub after: Vec<Line>,
    /// Already staged for the next commit (git's index), or still only in the file.
    pub staged: bool,
    /// The piece as a patch git can apply on its own: the file's header and this one piece; empty for a new file.
    pub patch: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct FileDiff {
    pub path: String,
    pub pieces: Vec<Piece>,
    /// A file git does not follow yet: all of it is After.
    pub new: bool,
    /// git does not show a binary file's lines.
    pub binary: bool,
    /// Lines left out of a long new file.
    pub cut: usize,
}

impl FileDiff {
    /// Where the file changed, as the line under its name says it: the places git names, then how many pieces.
    pub fn places(&self) -> String {
        let mut named: Vec<&str> = vec![];
        for p in &self.pieces {
            if !p.at.is_empty() && !named.contains(&p.at.as_str()) {
                named.push(&p.at);
            }
        }
        let n = self.pieces.len();
        let count = format!("{n} place{}", if n == 1 { "" } else { "s" });
        if named.is_empty() {
            return count;
        }
        let shown: Vec<&str> = named.iter().take(3).copied().collect();
        format!("changed in {} · {count}", shown.join(" · "))
    }
}

/// The words of a line: names and numbers whole, each other mark alone, spaces as their own runs.
fn tokens(line: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let kind = |c: char| if c.is_alphanumeric() || c == '_' { 0 } else if c.is_whitespace() { 1 } else { 2 };
    for c in line.chars() {
        match out.last_mut() {
            Some(last) if kind(c) != 2 && last.chars().next().map(kind) == Some(kind(c)) => last.push(c),
            _ => out.push(c.to_string()),
        }
    }
    out
}

/// Whether each word that is not a space is kept (false) or changed (true), on both sides: the words of the longest
/// sequence the two sides share are kept. A piece too big to match word by word is changed whole.
fn matched(a: &[&str], b: &[&str], later: bool) -> (Vec<bool>, Vec<bool>) {
    let (n, m) = (a.len(), b.len());
    if n == 0 || m == 0 || n * m > 4_000_000 {
        return (vec![true; n], vec![true; m]);
    }
    // the length of the longest shared sequence of a[i..] and b[j..]
    let mut len = vec![0u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            len[i * (m + 1) + j] = if a[i] == b[j] { len[(i + 1) * (m + 1) + j + 1] + 1 } else { len[(i + 1) * (m + 1) + j].max(len[i * (m + 1) + j + 1]) };
        }
    }
    let (mut ca, mut cb) = (vec![true; n], vec![true; m]);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        // of two equal matches, the later one in b when asked: what was added then comes before what was kept
        if later && a[i] == b[j] && len[i * (m + 1) + j + 1] == len[i * (m + 1) + j] {
            j += 1;
        } else if a[i] == b[j] {
            ca[i] = false;
            cb[j] = false;
            i += 1;
            j += 1;
        } else if len[(i + 1) * (m + 1) + j] >= len[i * (m + 1) + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    (ca, cb)
}

/// The lines of one side split into parts, lit where `changed` says, spaces never lit.
fn lit(lines: &[(u32, String)], changed: &[bool]) -> Vec<Line> {
    let mut at = 0;
    lines
        .iter()
        .map(|(n, text)| {
            let mut parts: Vec<(String, bool)> = vec![];
            for t in tokens(text) {
                let on = if t.trim().is_empty() {
                    false
                } else {
                    let on = changed.get(at).copied().unwrap_or(true);
                    at += 1;
                    on
                };
                match parts.last_mut() {
                    Some((s, was)) if *was == on => s.push_str(&t),
                    _ => parts.push((t, on)),
                }
            }
            Line { n: *n, parts }
        })
        .collect()
}

/// A piece from its lines on each side, its words matched. Two words alike can often be matched either way; of the
/// earliest and the latest matching, the one that lights fewer separate bits reads best: "Halt the picked run" to
/// "Stop the open agent, or halt the picked run" lights "Stop the open agent, or halt" as one, and a line added after
/// another lights that line, not the other's last marks.
fn piece(at: String, before: Vec<(u32, String)>, after: Vec<(u32, String)>) -> Piece {
    let words = |lines: &[(u32, String)]| lines.iter().flat_map(|(_, t)| tokens(t)).filter(|t| !t.trim().is_empty()).collect::<Vec<_>>();
    let (wa, wb) = (words(&before), words(&after));
    let (wa, wb): (Vec<&str>, Vec<&str>) = (wa.iter().map(String::as_str).collect(), wb.iter().map(String::as_str).collect());
    // the lit bits: runs of lit words in a line, the spaces between them not breaking a run
    let bits = |lines: &[Line]| {
        lines
            .iter()
            .map(|l| {
                let words = l.parts.iter().filter(|(t, _)| !t.trim().is_empty());
                let mut was = false;
                words.filter(|(_, on)| !std::mem::replace(&mut was, *on) && *on).count()
            })
            .sum::<usize>()
    };
    [false, true]
        .into_iter()
        .map(|later| {
            let (ca, cb) = matched(&wa, &wb, later);
            Piece { at: at.clone(), before: lit(&before, &ca), after: lit(&after, &cb), ..Default::default() }
        })
        .min_by_key(|p| bits(&p.before) + bits(&p.after))
        .unwrap_or_default()
}

/// The heading git gives a piece, short: "impl Render for Root {" is "impl Render for Root", "fn agent(t: &Value)"
/// is "fn agent", "const SHORTCUTS: &[…]" is "const SHORTCUTS".
fn short(at: &str) -> String {
    let at = at.trim();
    // git takes any line that starts with a letter; in prose or a plain list that names nothing: only a line that
    // looks like code (a brace, a parenthesis, a colon or an equals) says where
    if !at.contains(['{', '(', ':', '=']) {
        return String::new();
    }
    at.find(['{', '(', ':', '=', '<']).map_or(at, |i| &at[..i]).trim().to_owned()
}

/// The first number of "-216" or "+217,8".
fn start(spec: &str) -> u32 {
    spec[1..].split(',').next().and_then(|n| n.parse().ok()).unwrap_or(0)
}

/// The pieces of one file's `git diff -U0`; `staged` when it compared the last commit with the index.
pub fn parse(out: &str, staged: bool) -> Vec<Piece> {
    let mut pieces = vec![];
    // the file's header, before its first piece, which every piece's patch starts with
    let mut header = String::new();
    let mut cur: Option<(String, Vec<(u32, String)>, Vec<(u32, String)>, u32, u32, String)> = None;
    let done = |cur: Option<(String, Vec<(u32, String)>, Vec<(u32, String)>, u32, u32, String)>, header: &str, pieces: &mut Vec<Piece>| {
        if let Some((at, b, a, _, _, raw)) = cur {
            pieces.push(Piece { staged, patch: format!("{header}{raw}"), ..piece(at, b, a) });
        }
    };
    for line in out.lines() {
        if let Some(head) = line.strip_prefix("@@ ") {
            done(cur.take(), &header, &mut pieces);
            let mut f = head.splitn(3, ' ');
            let (old, new) = (f.next().unwrap_or("-0"), f.next().unwrap_or("+0"));
            let at = short(f.next().unwrap_or("").trim_start_matches("@@"));
            cur = Some((at, vec![], vec![], start(old), start(new), format!("{line}\n")));
            continue;
        }
        let Some((_, before, after, nb, na, raw)) = cur.as_mut() else {
            header.push_str(line);
            header.push('\n');
            continue;
        };
        raw.push_str(line);
        raw.push('\n');
        if let Some(t) = line.strip_prefix('-') {
            before.push((*nb, t.to_owned()));
            *nb += 1;
        } else if let Some(t) = line.strip_prefix('+') {
            after.push((*na, t.to_owned()));
            *na += 1;
        }
    }
    done(cur.take(), &header, &mut pieces);
    pieces
}

/// The longest new file the tab shows whole.
const NEW_LINES: usize = 400;

/// A file's changes against the last commit, staged pieces first; a new file as all After.
pub fn read(root: &str, path: &str, new: bool) -> FileDiff {
    let mut d = FileDiff { path: path.to_owned(), new, ..Default::default() };
    if new {
        match std::fs::read(std::path::Path::new(root).join(path)) {
            Ok(bytes) if bytes.contains(&0) => d.binary = true,
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                let lines: Vec<(u32, String)> = text.lines().enumerate().map(|(i, l)| (i as u32 + 1, l.to_owned())).collect();
                d.cut = lines.len().saturating_sub(NEW_LINES);
                let after = lines.into_iter().take(NEW_LINES).map(|(n, t)| Line { n, parts: vec![(t, true)] }).collect();
                d.pieces = vec![Piece { before: vec![], after, ..Default::default() }];
            }
            // a new folder: its files are listed in the panel once one is added
            Err(_) => {}
        }
        return d;
    }
    let diff = |cached: bool| {
        spawn::command("git")
            .arg("-C")
            .arg(root)
            .args(["diff", "-U0", "--no-color", "--no-ext-diff"])
            .args(cached.then_some("--cached"))
            .arg("--")
            .arg(path)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    // what is staged, then what is only in the file, as magit lists them
    let (staged, work) = (diff(true), diff(false));
    d.binary = staged.lines().chain(work.lines()).any(|l| l.starts_with("Binary files "));
    d.pieces = parse(&staged, true).into_iter().chain(parse(&work, false)).collect();
    d
}

#[cfg(test)]
mod tests {
    use super::{parse, read, short, tokens, FileDiff, Line, Piece};

    fn parse_w(out: &str) -> Vec<Piece> {
        parse(out, false)
    }

    #[test]
    fn a_piece_is_staged_and_unstaged_alone() {
        let dir = std::env::temp_dir().join(format!("jaynes-diff-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap().status.success(), "git {args:?}");
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "T"]);
        run(&["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("f.txt"), "a\nb\nc\nd\ne\nf\ng\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "first"]);
        std::fs::write(dir.join("f.txt"), "A\nb\nc\nd\ne\nf\nG\n").unwrap();
        let root = dir.to_string_lossy().to_string();
        let d = read(&root, "f.txt", false);
        assert_eq!(d.pieces.iter().map(|p| p.staged).collect::<Vec<_>>(), [false, false]);
        // the first piece alone goes into the next commit
        crate::git::apply(&root, &d.pieces[0].patch, false).unwrap();
        let d = read(&root, "f.txt", false);
        assert_eq!(d.pieces.iter().map(|p| (p.staged, p.before[0].n)).collect::<Vec<_>>(), [(true, 1), (false, 7)]);
        // and comes back out
        crate::git::apply(&root, &d.pieces[0].patch, true).unwrap();
        assert_eq!(read(&root, "f.txt", false).pieces.iter().map(|p| p.staged).collect::<Vec<_>>(), [false, false]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_heading_is_cut_to_its_name() {
        assert_eq!(short(" const SHORTCUTS: &[(&str, &[(&str, &str)])] = &["), "const SHORTCUTS");
        assert_eq!(short("fn agent(t: &Value) -> Agent {"), "fn agent");
        assert_eq!(short("pub struct Agent {"), "pub struct Agent");
        assert_eq!(short(""), "");
        assert_eq!(short("f"), "");
        assert_eq!(short("The tabs keep their place"), "");
    }

    /// The parts of a side that are lit, joined.
    fn lit(lines: &[Line]) -> Vec<String> {
        lines.iter().flat_map(|l| l.parts.iter().filter(|(_, on)| *on).map(|(s, _)| s.clone())).collect()
    }

    #[test]
    fn words_are_names_marks_and_spaces() {
        assert_eq!(tokens("    this.halt_picked(cx);"), ["    ", "this", ".", "halt_picked", "(", "cx", ")", ";"]);
    }

    #[test]
    fn code_moved_into_a_block_is_not_lit() {
        // the real change of 6219917 in main.rs: halt_picked went inside an if, nothing was taken out
        let out = "diff --git a/app/src/main.rs b/app/src/main.rs\n--- a/app/src/main.rs\n+++ b/app/src/main.rs\n@@ -216 +217,8 @@ impl Render for Root {\n-            .on_action(cx.listener(|this, _: &HaltRun, _, cx| this.halt_picked(cx)))\n+            .on_action(cx.listener(|this, _: &HaltRun, _, cx| {\n+                // over an agent, it stops the agent; else the picked run\n+                if this.agent_view.is_some() {\n+                    this.stop_agent(cx);\n+                } else {\n+                    this.halt_picked(cx);\n+                }\n+            }))\n";
        let pieces = parse(out, false);
        assert_eq!(pieces.len(), 1);
        let p = &pieces[0];
        assert_eq!(p.at, "impl Render for Root");
        assert_eq!(p.before[0].n, 216);
        assert_eq!(p.after.iter().map(|l| l.n).collect::<Vec<_>>(), (217..=224).collect::<Vec<_>>());
        assert!(lit(&p.before).is_empty(), "nothing was taken out: {:?}", lit(&p.before));
        // the line that only moved keeps its words unlit; only the ; after it is new
        let moved: String = p.after[5].parts.iter().filter(|(_, on)| !on).map(|(s, _)| s.as_str()).collect();
        assert!(moved.contains("halt_picked(cx)"), "{:?}", p.after[5].parts);
        // the indentation is never lit
        assert!(!p.after[1].parts[0].1);
    }

    #[test]
    fn a_changed_word_is_lit_on_both_sides() {
        let out = "@@ -57 +57 @@ const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[\n-    (\"⌘.\", \"Halt the picked run\"),\n+    (\"⌘.\", \"Stop the open agent, or halt the picked run\"),\n";
        let p = &parse(out, false)[0];
        assert_eq!(lit(&p.before), ["Halt"]);
        assert_eq!(lit(&p.after).join(""), "Stoptheopenagent,orhalt");
    }

    #[test]
    fn a_line_added_after_another_lights_only_itself() {
        let p = &parse_w("@@ -2 +2,2 @@ fn main() {\n-    println!(\"hi\");\n+    println!(\"hello\");\n+    run();\n")[0];
        assert_eq!(lit(&p.after), ["hello", "run();"]);
    }

    #[test]
    fn a_reindent_is_not_a_change() {
        let p = &parse_w("@@ -3,2 +3,2 @@\n-if a {\n-  b();\n+if a {\n+      b();\n")[0];
        assert!(lit(&p.before).is_empty() && lit(&p.after).is_empty());
    }

    #[test]
    fn places_name_where_the_file_changed() {
        let piece = |at: &str| Piece { at: at.into(), ..Default::default() };
        let d = FileDiff { pieces: vec![piece("const SHORTCUTS"), piece("impl Render for Root {"), piece("impl Render for Root {")], ..Default::default() };
        assert_eq!(d.places(), "changed in const SHORTCUTS · impl Render for Root { · 3 places");
        assert_eq!(FileDiff { pieces: vec![piece("")], ..Default::default() }.places(), "1 place");
    }
}
