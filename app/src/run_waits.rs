//! The line over an open agent's terminal when a run waits for the person: "◆ A run waits · <run> · <question>",
//! Open (Command-O) goes to it, Later hides it until the next question. It tells, it does not interrupt: the agent's
//! screen stays whole, and it does not show while the open agent asks something itself (its own ask bar does).

use std::collections::HashSet;

/// A run that waits at a question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waiting {
    pub run: String,
    /// The run's request, short.
    pub title: String,
    pub question: String,
    /// What the question asks, as the role says it.
    pub about: String,
    /// Something that cannot be undone lies ahead of it.
    pub ahead: bool,
}

/// The run to tell of: one that waits before something that cannot be undone first, else the first; none put off
/// for later (its run and question, "run/question"), and none while the open agent asks something itself.
pub fn line(waits: &[Waiting], later: &HashSet<String>, agent_asks: bool) -> Option<Waiting> {
    if agent_asks {
        return None;
    }
    let open = |w: &&Waiting| !later.contains(&key(w));
    waits.iter().filter(open).find(|w| w.ahead).or_else(|| waits.iter().find(open)).cloned()
}

/// The line's words: its head, what waits (said when something that cannot be undone lies ahead), and its buttons.
pub const HEAD: &str = "◆ A run waits";
pub const OPEN: &str = "Open";
pub const OPEN_KEY: &str = "⌘O";
pub const LATER: &str = "Later";

/// What the line says of the run that waits.
pub fn what(w: &Waiting) -> String {
    if w.ahead { format!("{} · {} · before something that cannot be undone", w.title, w.about) } else { format!("{} · {}", w.title, w.about) }
}

/// How Later remembers a question.
pub fn key(w: &Waiting) -> String {
    format!("{}/{}", w.run, w.question)
}

/// The runs that wait, from the record's stops, in the column's order.
pub fn waiting(snap: &crate::model::Snapshot) -> Vec<Waiting> {
    snap.listing
        .needs
        .iter()
        .filter_map(|r| snap.stops.get(&r.id).map(|s| Waiting { run: r.id.clone(), title: r.title.clone(), question: s.question.clone(), about: s.title.clone(), ahead: s.ahead.is_some() && s.touch.is_some() }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{HEAD, LATER, OPEN, OPEN_KEY, Waiting, key, line, what};
    use std::collections::HashSet;

    fn w(run: &str, q: &str, ahead: bool) -> Waiting {
        Waiting { run: run.into(), title: format!("Run {run}"), question: q.into(), about: format!("about {q}"), ahead }
    }

    #[test]
    fn it_tells_the_first_run_that_waits_and_not_one_put_off_for_later() {
        let none = HashSet::new();
        let waits = [w("r1", "q1", false), w("r2", "q2", false)];
        assert_eq!(line(&waits, &none, false).map(|l| l.run), Some("r1".into()));
        // Later on r1's question: the next one, then none
        let later: HashSet<String> = ["r1/q1".to_owned()].into();
        assert_eq!(line(&waits, &later, false).map(|l| l.run), Some("r2".into()));
        let both: HashSet<String> = ["r1/q1".to_owned(), "r2/q2".to_owned()].into();
        assert!(line(&waits, &both, false).is_none());
        // a new question of a run put off comes back
        assert_eq!(line(&[w("r1", "q3", false)], &later, false).map(|l| l.question), Some("q3".into()));
    }

    #[test]
    fn it_stays_away_while_the_open_agent_asks_and_stands_out_before_what_cannot_be_undone() {
        let none = HashSet::new();
        assert!(line(&[w("r1", "q1", false)], &none, true).is_none());
        // what lies ahead that cannot be undone comes first, and is said
        let l = line(&[w("r1", "q1", false), w("r2", "q2", true)], &none, false).unwrap();
        assert_eq!((l.run.as_str(), l.ahead), ("r2", true));
    }

    // Alan, 18/11, on a run's question while an agent is open (the board's "Decidido pelo Alan em 18/11"): "a opção 1 do
    // esboço. Uma linha fina acima do terminal do agente, com Open ⌘O e Later; a tela do agente fica inteira"
    #[test]
    fn asked_a_thin_line_over_the_agents_terminal_with_open_and_later() {
        let none = HashSet::new();
        let waits = [w("r1", "q1", false)];
        // a line, not a stop over the agent: what waits in a few words, Open with its key, and Later
        let l = line(&waits, &none, false).expect("the line");
        assert_eq!((HEAD, OPEN, OPEN_KEY, LATER), ("◆ A run waits", "Open", "⌘O", "Later"));
        assert_eq!(what(&l), "Run r1 · about q1");
        // Later puts it off until that run asks something else
        let later: HashSet<String> = [key(&l)].into();
        assert!(line(&waits, &later, false).is_none());
        assert!(line(&[w("r1", "q2", false)], &later, false).is_some());
        // what lies ahead that cannot be undone is said on the line
        assert_eq!(what(&w("r2", "q2", true)), "Run r2 · about q2 · before something that cannot be undone");
    }
}
