//! What the person may do to a run from its tab, the page's own controls: Resume a halted run, Continue one Jaynes
//! stopped, Raise the risk, Raise the ceiling near or at it, Abandon one not finished. Each shows only when it can do
//! something, as on the page. They are sent as the person's through `send`, the one place that sends them.

use jaynes_core::Input;
use jaynes_core::question::By;
use jaynes_core::run::{Phase, Run};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Resume,
    Continue,
    RaiseRisk,
    RaiseCeiling,
    Abandon,
}

impl Act {
    /// The word the page sends for it.
    pub fn word(self) -> &'static str {
        match self {
            Act::Resume => "resume",
            Act::Continue => "continue",
            Act::RaiseRisk => "raise-risk",
            Act::RaiseCeiling => "raise-ceiling",
            Act::Abandon => "abandon",
        }
    }

    /// Its button, as the page says it.
    pub fn label(self) -> &'static str {
        match self {
            Act::Resume => "Resume",
            Act::Continue => "Continue",
            Act::RaiseRisk => "Raise the risk",
            Act::RaiseCeiling => "Raise the ceiling by 30 min",
            Act::Abandon => "Abandon",
        }
    }

    /// What cannot be taken back is pressed twice, as on the page.
    pub fn asks_twice(self) -> bool {
        self == Act::Abandon
    }
}

/// What the controls are decided from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Facts {
    pub finished: bool,
    pub halted: bool,
    pub running: bool,
    pub blocked: bool,
    pub low_risk: bool,
    /// Four fifths of its time or tokens spent.
    pub near_ceiling: bool,
    pub stopped_by_ceiling: bool,
}

impl Facts {
    pub fn of(r: &Run) -> Facts {
        Facts {
            finished: r.finished(),
            halted: r.halted,
            running: matches!(r.phase, Phase::Running),
            blocked: r.blocked(),
            low_risk: r.risk == jaynes_core::risk::Risk::Low,
            near_ceiling: r.spent.seconds * 5 >= r.ceiling.seconds * 4 || r.spent.tokens * 5 >= r.ceiling.tokens * 4,
            stopped_by_ceiling: matches!(r.phase, Phase::Blocked { fault: jaynes_core::event::Fault::Ceiling, .. }),
        }
    }
}

/// The controls a run shows, in the page's order (halting is Command-period, so it is not here).
pub fn acts(f: &Facts) -> Vec<Act> {
    if f.finished {
        return Vec::new();
    }
    let mut out = Vec::new();
    if f.halted {
        out.push(Act::Resume);
    }
    if f.blocked {
        out.push(Act::Continue);
    }
    if f.low_risk {
        out.push(Act::RaiseRisk);
    }
    if f.near_ceiling || f.stopped_by_ceiling {
        out.push(Act::RaiseCeiling);
    }
    out.push(Act::Abandon);
    out
}

/// The run as the record has it now.
pub fn read(run: &str) -> Option<Run> {
    jaynes_shell::store::Store::open(&jaynes_shell::home().join("record.sqlite3")).ok()?.state(run).ok().flatten()
}

/// Sends `act` on `run` as the person's, the way the page's /api/act does: the one place that sends them, so a
/// signed path (G18) or the engine's own act_in replaces this body alone. The core refuses what is not the person's.
pub fn send(run: &str, act: Act) -> Result<(), String> {
    let r = read(run).ok_or("no such run")?;
    let by = By::Person;
    let input = match act {
        Act::Resume => Input::Resume { by },
        Act::Continue => Input::Continue { by },
        Act::Abandon => Input::Abandon { by },
        Act::RaiseRisk => Input::RaiseRisk { to: jaynes_core::risk::Risk::High, by },
        // as the page raises it: half an hour and two million tokens more
        Act::RaiseCeiling => Input::RaiseCeiling { ceiling: jaynes_core::risk::Ceiling { seconds: r.ceiling.seconds + 1800, tokens: r.ceiling.tokens + 2_000_000 }, by },
    };
    jaynes_shell::drive::submit(&jaynes_shell::home(), &crate::answer::engine_program(), &r, input).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{Act, Facts, acts};

    fn facts() -> Facts {
        Facts { finished: false, halted: false, running: true, blocked: false, low_risk: false, near_ceiling: false, stopped_by_ceiling: false }
    }

    #[test]
    fn each_control_shows_only_when_it_can_do_something_as_on_the_page() {
        // at work: it may only be abandoned (halting is Command-period)
        assert_eq!(acts(&facts()), [Act::Abandon]);
        assert_eq!(acts(&Facts { halted: true, running: false, ..facts() }), [Act::Resume, Act::Abandon]);
        assert_eq!(acts(&Facts { blocked: true, running: false, ..facts() }), [Act::Continue, Act::Abandon]);
        assert_eq!(acts(&Facts { low_risk: true, ..facts() }), [Act::RaiseRisk, Act::Abandon]);
        assert_eq!(acts(&Facts { near_ceiling: true, ..facts() }), [Act::RaiseCeiling, Act::Abandon]);
        assert_eq!(acts(&Facts { stopped_by_ceiling: true, blocked: true, running: false, ..facts() }), [Act::Continue, Act::RaiseCeiling, Act::Abandon]);
        // a finished run has none
        assert!(acts(&Facts { finished: true, ..facts() }).is_empty());
    }

    #[test]
    fn each_is_the_pages_own_action_word() {
        let words: Vec<&str> = [Act::Resume, Act::Continue, Act::RaiseRisk, Act::RaiseCeiling, Act::Abandon].iter().map(|a| a.word()).collect();
        assert_eq!(words, ["resume", "continue", "raise-risk", "raise-ceiling", "abandon"]);
        // what cannot be taken back asks twice
        assert!(Act::Abandon.asks_twice() && !Act::Resume.asks_twice());
    }
}
