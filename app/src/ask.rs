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

/// What it asks, as the bar says it after the agent's name: "wants to approve Bash: touch x".
pub fn what(kind: Option<&str>, msg: &str) -> String {
    match kind {
        Some("permission") => format!("wants to {msg}"),
        Some("question") => format!("asks: {msg}"),
        Some("auth") => format!("needs you to sign in: {msg}"),
        _ if msg.is_empty() => "waits for you".into(),
        _ => format!("waits: {msg}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{choices, what};

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
        assert_eq!(what(Some("permission"), "approve Bash: touch x"), "wants to approve Bash: touch x");
        assert_eq!(what(Some("question"), "which one?"), "asks: which one?");
        assert_eq!(what(None, ""), "waits for you");
    }
}
