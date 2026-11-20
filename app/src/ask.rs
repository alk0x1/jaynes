//! What a blocked agent asks, read from its screen: the choices Claude Code numbers under its question ("1. Yes",
//! "2. Yes, and always allow…", "4. No"), so the bar over the terminal can offer them as buttons with their keys.

/// The choices of the last list on the screen numbered from 1, in order; none when the screen shows no such list.
pub fn choices(screen: &str) -> Vec<String> {
    let mut last: Vec<String> = Vec::new();
    let mut run: Vec<String> = Vec::new();
    for line in screen.lines() {
        let item = line.trim_start().trim_start_matches('❯').trim_start();
        let numbered = item.split_once(". ").and_then(|(n, text)| n.parse::<usize>().ok().map(|n| (n, text.trim())));
        match numbered {
            Some((n, text)) if n == run.len() + 1 => run.push(short(text)),
            Some((1, text)) => run = vec![short(text)],
            // a choice's text may wrap onto lines of its own; anything else ends the list
            None if !run.is_empty() && line.starts_with("      ") && !item.is_empty() => {}
            _ => {
                if !run.is_empty() {
                    last = std::mem::take(&mut run);
                }
            }
        }
    }
    if run.is_empty() { last } else { run }
}

/// A choice without the words after its " · ", which explain it.
fn short(text: &str) -> String {
    text.split(" · ").next().unwrap_or(text).trim().to_owned()
}

/// Whether the bar offers Deny (Esc) for what an agent asks: a permission or anything else, yes; a question, no, since
/// Esc only interrupts it: it is answered with its choices, or in the terminal under the bar.
pub fn offers_deny(kind: Option<&str>, _choices: &[String]) -> bool {
    kind != Some("question")
}

/// What the bar over an agent that failed says after its name: "failed: the tests failed", or "failed" without words,
/// as the column says "failed · open it".
pub fn failed(msg: &str) -> String {
    let msg = msg.trim();
    if msg.is_empty() { "failed".into() } else { format!("failed: {msg}") }
}

/// What it asks, as the bar says it after the agent's name: "asks you to approve Bash: touch x". An agent that says
/// what kind of thing it asks but not its words still gets a whole sentence.
pub fn what(kind: Option<&str>, msg: &str) -> String {
    let msg = msg.trim();
    match kind {
        Some("permission") if msg.is_empty() => "wants your permission".into(),
        Some("question") if msg.is_empty() => "asks you something".into(),
        Some("auth") if msg.is_empty() => "needs you to sign in".into(),
        Some("permission") => format!("asks you to {msg}"),
        Some("question") => format!("asks: {msg}"),
        Some("auth") => format!("needs you to sign in: {msg}"),
        _ if msg.is_empty() => "waits for you".into(),
        _ => format!("waits: {msg}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{choices, failed, offers_deny, what};

    #[test]
    fn a_question_is_answered_with_its_choices_and_a_permission_can_be_denied() {
        let some = vec!["Yes".to_string(), "No".to_string()];
        assert!(offers_deny(Some("permission"), &some));
        assert!(offers_deny(Some("permission"), &[]));
        assert!(offers_deny(None, &[]));
        // a question is not denied: Esc would only interrupt it; it is answered with its choices, or in the terminal
        assert!(!offers_deny(Some("question"), &some));
        assert!(!offers_deny(Some("question"), &[]));
    }

    #[test]
    fn an_agent_that_failed_says_so_over_its_terminal_with_its_words() {
        assert_eq!(failed("the tests failed"), "failed: the tests failed");
        assert_eq!(failed("  the tests failed \n"), "failed: the tests failed");
        assert_eq!(failed(" "), "failed");
    }

    #[test]
    fn the_choices_are_the_last_numbered_list_on_the_screen() {
        let screen = include_str!("../tests/fixtures/permission-screen.txt");
        let got = choices(screen);
        assert_eq!(got.len(), 4);
        assert_eq!(got[0], "Yes");
        assert!(got[1].starts_with("Yes, and always allow access to"));
        // the words after a · explain the choice; the button keeps the choice
        assert_eq!(got[2], "Yes, and switch to auto mode");
        assert_eq!(got[3], "No");
    }

    #[test]
    fn a_numbered_list_the_agent_wrote_earlier_is_not_taken_for_choices() {
        let screen = "⏺ Plan:\n  1. Read the file\n  2. Fix it\n\n Do you want to proceed?\n ❯ 1. Yes\n   2. No\n Esc to cancel\n";
        assert_eq!(choices(screen), ["Yes", "No"]);
        // and with no question, the agent's own list is all there is: the bar asks only for a blocked agent
        assert_eq!(choices("  1. Read the file\n  2. Fix it\n"), ["Read the file", "Fix it"]);
        assert!(choices("nothing numbered here\n").is_empty());
    }

    #[test]
    fn the_bar_says_what_it_asks() {
        assert_eq!(what(Some("permission"), "approve Bash: touch x"), "asks you to approve Bash: touch x");
        assert_eq!(what(Some("question"), "which one?"), "asks: which one?");
        assert_eq!(what(None, ""), "waits for you");
        // the kind without its words: a whole sentence, never "wants to" with nothing after it
        assert_eq!(what(Some("permission"), ""), "wants your permission");
        assert_eq!(what(Some("question"), " "), "asks you something");
        assert_eq!(what(Some("auth"), ""), "needs you to sign in");
    }
}
