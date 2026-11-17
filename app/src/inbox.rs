//! The Inbox: what needs the person across every space. Agents that ask something and Jaynes runs that wait come
//! first ("Needs you"), then agents that finished a turn the person has not looked at ("Finished, to review"). Command-I
//! shows it in the middle; Command-O goes to the next thing in it.

use crate::agents::{Space, State};
use jaynes_shell::listing::Listing;

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

/// What needs the person, then what finished unseen.
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
    let review = agents().filter(|(_, a)| a.state == State::ToReview).map(|(s, a)| item(s, a, "finished".into())).collect();
    (needs, review)
}

/// Command-O: the thing after the one open, round the end; the first when none of them is open.
pub fn next(order: &[Key], open: Option<&Key>) -> Option<Key> {
    if order.is_empty() {
        return None;
    }
    let at = open.and_then(|o| order.iter().position(|k| k == o)).map(|i| (i + 1) % order.len()).unwrap_or(0);
    Some(order[at].clone())
}

#[cfg(test)]
mod tests {
    use super::{Key, gather, next};
    use crate::agents::{Agent, Space, State};
    use jaynes_shell::listing::{Listing, Mark, Row};

    fn agent(address: &str, state: State) -> Agent {
        Agent { address: address.into(), title: address.to_uppercase(), state, since_ms: 0, shell: false, turn: 0, kind: Some("permission".into()), msg: "approve Bash: ls".into(), cwd: String::new() }
    }

    #[test]
    fn what_asks_comes_first_then_the_waiting_runs_then_what_finished_unseen() {
        let spaces = [
            Space { id: "s1".into(), name: "jaynes".into(), folder: String::new(), agents: vec![agent("t1", State::ToReview), agent("t2", State::Waiting), agent("t3", State::Working)] },
            Space { id: "s2".into(), name: "helm".into(), folder: String::new(), agents: vec![agent("t4", State::Error), agent("t5", State::Idle)] },
        ];
        let runs = Listing { needs: vec![Row { id: "r1".into(), title: "Run".into(), line: "Pick one".into(), mark: Mark::Needs }], projects: vec![] };
        let (needs, review) = gather(&spaces, &runs);
        assert_eq!(needs.iter().map(|i| i.key.clone()).collect::<Vec<_>>(), [Key::Agent("t2".into()), Key::Agent("t4".into()), Key::Run("r1".into())]);
        assert_eq!(needs[0].what, "wants to approve Bash: ls");
        assert_eq!(needs[0].place, "jaynes");
        assert_eq!(needs[1].what, "stopped with an error");
        assert_eq!(review.iter().map(|i| i.key.clone()).collect::<Vec<_>>(), [Key::Agent("t1".into())]);
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
