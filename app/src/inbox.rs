//! The Inbox: what needs the person across every space. Agents that ask something or failed, Jaynes runs that wait
//! and blocked runs come first ("Needs you"), then agents that finished a turn the person has not looked at
//! ("Finished", neutral: nothing is expected of them there). Command-I shows it in the middle; Command-O goes to the
//! next thing that needs the person.

use crate::agents::{Space, State};
use jaynes_shell::listing::{Listing, Mark};
use std::collections::{HashMap, HashSet};

/// A thing in the Inbox: one of the person's agents, or a Jaynes run.
#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Agent(String),
    Run(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub key: Key,
    pub title: String,
    /// The space it is in, or the run's project.
    pub place: String,
    /// What it asks or did, after its title: "wants to approve Bash: …", "finished".
    pub what: String,
    pub state: State,
    pub since_ms: u64,
}

/// What needs the person, then what finished unseen. A blocked run needs them: to continue it or abandon it.
pub fn gather(spaces: &[Space], runs: &Listing) -> (Vec<Item>, Vec<Item>) {
    let agents = || spaces.iter().flat_map(|s| s.agents.iter().map(move |a| (s, a)));
    let item = |s: &Space, a: &crate::agents::Agent, what: String| Item {
        key: Key::Agent(a.address.clone()),
        title: a.title.clone(),
        place: s.name.clone(),
        what,
        state: a.state,
        since_ms: a.since_ms,
    };
    let mut needs: Vec<Item> = agents()
        .filter(|(_, a)| matches!(a.state, State::Waiting | State::Error))
        .map(|(s, a)| {
            let what = if a.state == State::Error { "stopped with an error".to_string() } else { crate::ask::what(a.kind.as_deref(), &a.msg) };
            item(s, a, what)
        })
        .collect();
    needs.extend(runs.needs.iter().map(|r| Item { key: Key::Run(r.id.clone()), title: r.title.clone(), place: "Jaynes run".into(), what: r.line.clone(), state: State::Waiting, since_ms: 0 }));
    let blocked = runs.projects.iter().flat_map(|(_, rows)| rows).filter(|r| r.mark == Mark::Blocked);
    needs.extend(blocked.map(|r| Item {
        key: Key::Run(r.id.clone()),
        title: r.title.clone(),
        place: format!("Jaynes run · {}", r.line),
        what: "blocked · open it".into(),
        state: State::Error,
        since_ms: 0,
    }));
    let review = agents().filter(|(_, a)| a.state == State::ToReview).map(|(s, a)| item(s, a, "finished".into())).collect();
    (needs, review)
}

/// The runs delivered or merged that the person has not seen (`seen`), for "Finished", each with what it delivered:
/// "delivered · 1 file". None before the app knows what was seen.
pub fn unseen_runs(runs: &Listing, seen: Option<&HashSet<String>>, files: &HashMap<String, usize>) -> Vec<Item> {
    let Some(seen) = seen else { return Vec::new() };
    runs.projects
        .iter()
        .flat_map(|(project, rows)| rows.iter().map(move |r| (project, r)))
        .filter(|(_, r)| matches!(r.mark, Mark::Delivered | Mark::Merged) && !seen.contains(&r.id))
        .map(|(project, r)| {
            let word = if r.mark == Mark::Merged { "merged" } else { "delivered" };
            let what = match files.get(&r.id) {
                Some(n) => format!("{word} · {n} {}", if *n == 1 { "file" } else { "files" }),
                None => word.to_string(),
            };
            Item { key: Key::Run(r.id.clone()), title: r.title.clone(), place: format!("Jaynes run · {project}"), what, state: State::ToReview, since_ms: 0 }
        })
        .collect()
}

/// Up (-1) or down (1) the Inbox's `len` items from `at`: the first down lands on the first, the ends hold, and a
/// list shorter than the cursor brings it to its last. None with nothing to walk.
pub fn moved(len: usize, at: Option<usize>, by: isize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match at {
        None => 0,
        Some(a) => (a.min(len - 1) as isize + by).clamp(0, len as isize - 1) as usize,
    })
}

/// Command-O: the thing after the one open, round the end; the first when none of them is open.
pub fn next(order: &[Key], open: Option<&Key>) -> Option<Key> {
    if order.is_empty() {
        return None;
    }
    let at = open.and_then(|o| order.iter().position(|k| k == o)).map(|i| (i + 1) % order.len()).unwrap_or(0);
    Some(order[at].clone())
}

/// The line under the Inbox's name: "across all N spaces" only where there is more than one to be across.
pub fn subtitle(spaces: usize) -> String {
    let across = if spaces > 1 { format!(", across all {spaces} spaces") } else { String::new() };
    format!("What needs you{across}. Agents land here when they ask something or finish.")
}

#[cfg(test)]
mod tests {
    use super::{Key, gather, moved, next, subtitle, unseen_runs};
    use crate::agents::{Agent, Space, State};
    use jaynes_shell::listing::{Listing, Mark, Row};
    use std::collections::{HashMap, HashSet};

    #[test]
    fn a_run_delivered_and_not_seen_is_in_finished_with_what_it_delivered() {
        let row = |id: &str, title: &str, mark: Mark| Row { id: id.into(), title: title.into(), line: String::new(), mark };
        let runs = Listing {
            needs: vec![],
            projects: vec![("site".into(), vec![
                row("d1", "Say quietly in the hero", Mark::Delivered),
                row("d2", "Rename the footer", Mark::Merged),
                row("d3", "Old one", Mark::Delivered),
                row("a1", "Dropped", Mark::Abandoned),
                row("w1", "Going", Mark::Working),
            ])],
        };
        let files: HashMap<String, usize> = [("d1".to_string(), 1), ("d2".to_string(), 3)].into();
        let seen: HashSet<String> = ["d3".to_string()].into();
        let got = unseen_runs(&runs, Some(&seen), &files);
        assert_eq!(got.iter().map(|i| i.key.clone()).collect::<Vec<_>>(), [Key::Run("d1".into()), Key::Run("d2".into())]);
        assert_eq!((got[0].title.as_str(), got[0].what.as_str(), got[0].state), ("Say quietly in the hero", "delivered · 1 file", State::ToReview));
        assert_eq!(got[1].what, "merged · 3 files");
        // before the app knows what was seen (the record not read yet), nothing is said unseen
        assert!(unseen_runs(&runs, None, &files).is_empty());
    }

    fn agent(address: &str, state: State) -> Agent {
        Agent { address: address.into(), title: address.to_uppercase(), state, since_ms: 0, shell: false, turn: 0, kind: Some("permission".into()), msg: "approve Bash: ls".into(), cwd: String::new() }
    }

    #[test]
    fn the_inbox_says_across_all_spaces_only_when_there_are_two_or_more() {
        assert_eq!(subtitle(0), "What needs you. Agents land here when they ask something or finish.");
        assert_eq!(subtitle(1), "What needs you. Agents land here when they ask something or finish.");
        assert_eq!(subtitle(3), "What needs you, across all 3 spaces. Agents land here when they ask something or finish.");
    }

    #[test]
    fn what_asks_comes_first_then_the_waiting_runs_then_what_finished_unseen() {
        let spaces = [
            Space { id: "s1".into(), name: "jaynes".into(), folder: String::new(), agents: vec![agent("t1", State::ToReview), agent("t2", State::Waiting), agent("t3", State::Working)], tabs: vec![] },
            Space { id: "s2".into(), name: "helm".into(), folder: String::new(), agents: vec![agent("t4", State::Error), agent("t5", State::Idle)], tabs: vec![] },
        ];
        let runs = Listing {
            needs: vec![Row { id: "r1".into(), title: "Run".into(), line: "Pick one".into(), mark: Mark::Needs }],
            projects: vec![(
                "api".into(),
                vec![
                    Row { id: "r2".into(), title: "Stuck".into(), line: "step 2 of 3, blocked: no room".into(), mark: Mark::Blocked },
                    Row { id: "r3".into(), title: "Going".into(), line: "step 1 of 3, running".into(), mark: Mark::Working },
                    Row { id: "r4".into(), title: "Stopped".into(), line: "step 1 of 3, halted".into(), mark: Mark::Halted },
                ],
            )],
        };
        let (needs, review) = gather(&spaces, &runs);
        assert_eq!(
            needs.iter().map(|i| i.key.clone()).collect::<Vec<_>>(),
            [Key::Agent("t2".into()), Key::Agent("t4".into()), Key::Run("r1".into()), Key::Run("r2".into())]
        );
        assert_eq!(needs[3].what, "blocked · open it");
        assert_eq!(needs[3].place, "Jaynes run · step 2 of 3, blocked: no room");
        assert_eq!(needs[0].what, "wants to approve Bash: ls");
        assert_eq!(needs[0].place, "jaynes");
        assert_eq!(needs[1].what, "stopped with an error");
        assert_eq!(review.iter().map(|i| i.key.clone()).collect::<Vec<_>>(), [Key::Agent("t1".into())]);
    }

    #[test]
    fn the_arrows_walk_the_inbox_and_stop_at_its_ends() {
        assert_eq!(moved(5, None, 1), Some(0), "the first down lands on the first");
        assert_eq!(moved(5, None, -1), Some(0));
        assert_eq!(moved(5, Some(0), 1), Some(1));
        assert_eq!(moved(5, Some(4), 1), Some(4), "no wrap at the end");
        assert_eq!(moved(5, Some(0), -1), Some(0));
        // the list got shorter under the cursor: it stays on the last
        assert_eq!(moved(2, Some(4), 1), Some(1));
        assert_eq!(moved(0, Some(1), 1), None, "nothing to walk");
    }

    #[test]
    fn command_o_goes_round_what_needs_the_person() {
        let order = [Key::Agent("t2".into()), Key::Run("r1".into()), Key::Agent("t1".into())];
        assert_eq!(next(&order, None), Some(Key::Agent("t2".into())));
        assert_eq!(next(&order, Some(&Key::Agent("t2".into()))), Some(Key::Run("r1".into())));
        assert_eq!(next(&order, Some(&Key::Agent("t1".into()))), Some(Key::Agent("t2".into())));
        // something open that needs nothing: the first
        assert_eq!(next(&order, Some(&Key::Agent("t9".into()))), Some(Key::Agent("t2".into())));
        assert_eq!(next(&[], None), None);
    }
}
