//! The number of the engine's guarantees, and of those proved in Lean, as the README and the site say it in words
//! ("eighteen sentences", "thirteen are proved in Lean"), is the number docs/guarantees.md and the Lean proofs
//! hold. Counted from them, never written here: a guarantee added to the page, or a proof added to the model, turns
//! this red until every place says the new number (the engineer lead, 23/11, for 0.2.5's G20).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn read(path: &str) -> String {
    std::fs::read_to_string(repo().join(path)).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// The G numbers in a text, as G1, G17: a G followed by digits, alone as a word.
fn g_numbers(text: &str) -> BTreeSet<u32> {
    let b = text.as_bytes();
    let mut out = BTreeSet::new();
    for (i, _) in text.match_indices('G') {
        let before_ok = i == 0 || !b[i - 1].is_ascii_alphanumeric();
        let digits: String = text[i + 1..].chars().take_while(char::is_ascii_digit).collect();
        let after = text[i + 1 + digits.len()..].chars().next();
        if before_ok && !digits.is_empty() && !after.is_some_and(|c| c.is_ascii_alphanumeric()) {
            out.insert(digits.parse().expect("digits"));
        }
    }
    out
}

/// The guarantees: each one a list item of docs/guarantees.md that starts with its number in bold, "- **G1.**".
fn guarantees() -> BTreeSet<u32> {
    read("docs/guarantees.md")
        .lines()
        .filter_map(|l| l.strip_prefix("- **G"))
        .map(|l| l.chars().take_while(char::is_ascii_digit).collect::<String>().parse().expect("a G number"))
        .collect()
}

/// The guarantees proved in Lean: those the proofs name.
fn proved_in_lean() -> BTreeSet<u32> {
    g_numbers(&read("engine/model/JaynesModel/Proofs.lean"))
}

const WORDS: [&str; 31] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve",
    "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen", "twenty", "twenty one",
    "twenty two", "twenty three", "twenty four", "twenty five", "twenty six", "twenty seven", "twenty eight",
    "twenty nine", "thirty",
];

fn in_words(n: usize) -> &'static str {
    WORDS.get(n).unwrap_or_else(|| panic!("{n} has no word here yet: add it to WORDS"))
}

/// The words of a text, lower case, anything that is not a letter a space ("twenty-one" reads "twenty one").
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// A number said in words that ends just before position `at`, as (its number, where it starts).
fn number_before(w: &[String], at: usize) -> Option<usize> {
    (1..=2).rev().filter(|&n| at >= n).find_map(|n| WORDS.iter().position(|x| *x == w[at - n..at].join(" ")))
}

/// Where a file says how many guarantees there are ("eighteen sentences", "its eighteen guarantees") and how many
/// are proved in Lean ("thirteen are proved in Lean", "thirteen proved in Lean"), the numbers it says, in order.
fn said(text: &str) -> (Vec<usize>, Vec<usize>) {
    let w = words(text);
    let (mut all, mut lean) = (Vec::new(), Vec::new());
    for i in 0..w.len() {
        if w[i] == "sentences" || w[i] == "guarantees" {
            all.extend(number_before(&w, i));
        }
        if w[i] == "proved" && w.get(i + 1).map(String::as_str) == Some("in") && w.get(i + 2).map(String::as_str) == Some("lean") {
            let at = if i > 0 && w[i - 1] == "are" { i - 1 } else { i };
            lean.extend(number_before(&w, at));
        }
    }
    (all, lean)
}

/// The files that may say the numbers: the README, every page of the site, every page of docs/.
fn places() -> Vec<String> {
    fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
            let p = e.expect("entry").path();
            if p.is_dir() {
                walk(&p, ext, out);
            } else if p.extension().is_some_and(|x| x == ext) {
                out.push(p);
            }
        }
    }
    let mut found = Vec::new();
    walk(&repo().join("site"), "html", &mut found);
    walk(&repo().join("docs"), "md", &mut found);
    let mut out: Vec<String> =
        found.iter().map(|p| p.strip_prefix(repo()).expect("in the repo").display().to_string()).collect();
    out.push("README.md".to_string());
    out.sort();
    out
}

#[test]
fn the_page_lists_its_guarantees_one_after_another() {
    let g = guarantees();
    assert!(g.len() >= 18, "docs/guarantees.md lists {} guarantees as \"- **G1.**\"", g.len());
}

#[test]
fn the_guarantees_page_names_the_ones_proved_in_lean_as_the_proofs_do() {
    let page = read("docs/guarantees.md");
    let intro = &page[..page.find("proved, in Lean").expect("the page says which are proved, in Lean")];
    let intro = &intro[intro.rfind("\n\n").map_or(0, |i| i + 2)..];
    assert_eq!(g_numbers(intro), proved_in_lean(), "the paragraph of docs/guarantees.md that names the guarantees proved in Lean, against the G numbers of Proofs.lean");
    assert!(proved_in_lean().is_subset(&guarantees()), "Proofs.lean names a guarantee the page does not list");
}

#[test]
fn the_readme_and_the_site_say_how_many_guarantees_there_are_and_how_many_are_proved_in_lean() {
    let (all, lean) = (guarantees().len(), proved_in_lean().len());
    let mut wrong = Vec::new();
    let (mut said_all, mut said_lean) = (0, 0);
    for place in places() {
        let (a, l) = said(&read(&place));
        said_all += a.len();
        said_lean += l.len();
        wrong.extend(a.iter().filter(|&&n| n != all).map(|&n| format!("{place} says \"{}\" sentences or guarantees, and there are {} (\"{}\")", in_words(n), all, in_words(all))));
        wrong.extend(l.iter().filter(|&&n| n != lean).map(|&n| format!("{place} says \"{}\" proved in Lean, and there are {} (\"{}\")", in_words(n), lean, in_words(lean))));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    // Five places say each number today (site/index.html twice, site/docs/index.html, README.md twice): one gone
    // or one more is a place this test should know about.
    assert_eq!((said_all, said_lean), (5, 5), "the places that say how many guarantees, and how many proved in Lean");
}

#[test]
fn asked_the_counting_reads_the_words_the_places_use() {
    assert_eq!(said("It is one of eighteen sentences the engine promises; thirteen are proved in Lean"), (vec![18], vec![13]));
    assert_eq!(said("Core and its twenty-one guarantees; thirteen proved in Lean"), (vec![21], vec![13]));
    assert_eq!(said("<p>Eighteen sentences, such as &quot;x&quot;; thirteen are\nproved in Lean"), (vec![18], vec![13]));
    assert_eq!(said("each sentence is a promise; the guarantees are tested"), (vec![], vec![]));
    assert_eq!(g_numbers("G1, G14 (its order) and G19. EG2 G3a"), BTreeSet::from([1, 14, 19]));
}
