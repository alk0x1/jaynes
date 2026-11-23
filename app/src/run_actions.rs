//! What the person may do to a run from its tab, the page's own controls: Halt one at work, Resume a halted run,
//! Continue one Jaynes stopped, Ask me at more steps (raise the risk), Raise the ceiling near or at it, Abandon one not finished. Each shows only when it can do
//! something, as on the page. They are sent as the person's through `send`, the one place that sends them.

use jaynes_core::question::By;
use jaynes_core::run::{Phase, Run};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Halt,
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
            Act::Halt => "halt",
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
            Act::Halt => "Halt",
            Act::Resume => "Resume",
            Act::Continue => "Continue",
            Act::RaiseRisk => jaynes_shell::listing::ASK_AT_MORE_STEPS,
            Act::RaiseCeiling => "Raise the ceiling by 30 min",
            Act::Abandon => "Abandon",
        }
    }

    /// What its button says on hovering, when its label does not say it all: raising the risk takes the run on the full
    /// path, with more stops and the higher limit (core risk.rs and decide.rs); abandoning a run already delivered
    /// keeps what it delivered (the core's Abandon finishes the run, never undoes it).
    pub fn tip(self, f: &Facts) -> Option<&'static str> {
        match self {
            // the page's words too (jaynes_shell::listing)
            Act::RaiseRisk => Some(jaynes_shell::listing::ASK_AT_MORE_STEPS_TIP),
            Act::Abandon if f.delivered => Some(jaynes_shell::listing::ABANDON_DELIVERED_TIP),
            _ => None,
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
    /// Whether the high risk takes steps the run's risk leaves out: without them, raising it changes only the limit.
    pub risk_adds_steps: bool,
    /// Four fifths of its time or tokens spent.
    pub near_ceiling: bool,
    pub stopped_by_ceiling: bool,
    /// Already delivered while an answer of the person's can still correct it.
    pub delivered: bool,
    /// Started on an older engine no longer on this machine: every control goes to that engine, and is refused.
    pub gone: bool,
    /// Jaynes stopped while it ran: it says it runs, and the process that drove it is gone (`drive::left_running`).
    /// Nobody works on it; the person resumes it, as a halted run.
    pub stopped: bool,
}

impl Facts {
    pub fn of(r: &Run) -> Facts {
        Facts {
            finished: r.finished(),
            halted: r.halted,
            running: matches!(r.phase, Phase::Running),
            blocked: r.blocked(),
            low_risk: r.risk == jaynes_core::risk::Risk::Low,
            risk_adds_steps: jaynes_shell::listing::risk_adds_steps(r),
            near_ceiling: r.spent.seconds * 5 >= r.ceiling.seconds * 4 || r.spent.tokens * 5 >= r.ceiling.tokens * 4,
            stopped_by_ceiling: matches!(r.phase, Phase::Blocked { fault: jaynes_core::event::Fault::Ceiling, .. }),
            delivered: !r.finished() && r.delivered.is_some(),
            gone: jaynes_shell::stop::gone(&jaynes_shell::home(), r).is_some(),
            stopped: jaynes_shell::drive::left_running(&jaynes_shell::home(), r),
        }
    }
}

/// The controls a run shows, in the page's order: Halt first while its agent works, as Command-period does on its tab.
pub fn acts(f: &Facts) -> Vec<Act> {
    if f.finished || f.gone {
        return Vec::new();
    }
    let mut out = Vec::new();
    // a run Jaynes stopped while it ran says it runs, and nothing does: resumed, never halted
    if f.running && !f.halted && !f.stopped {
        out.push(Act::Halt);
    }
    if f.halted || f.stopped {
        out.push(Act::Resume);
    }
    if f.blocked {
        out.push(Act::Continue);
    }
    // asking at more steps only where it would ask at more steps
    if f.low_risk && f.risk_adds_steps {
        out.push(Act::RaiseRisk);
    }
    if f.near_ceiling || f.stopped_by_ceiling {
        out.push(Act::RaiseCeiling);
    }
    out.push(Act::Abandon);
    out
}

/// How long a control pressed once stays armed for the press that sends it: a press later is a first press again.
pub const ARMED_FOR: Duration = Duration::from_secs(4);

/// A control pressed once that asks to be pressed again (Abandon): on which run, and when.
#[derive(Clone, Debug, PartialEq)]
pub struct Armed {
    pub run: String,
    pub act: Act,
    pub at: Instant,
}

impl Armed {
    /// Whether a press of `act` at `now`, with `open`'s tab showing, is the second press: the same control, on the
    /// run whose tab shows, within [`ARMED_FOR`]. Another tab, Claude Code's or a lapse of time disarms it.
    pub fn holds(&self, open: Option<&str>, act: Act, now: Instant) -> bool {
        open == Some(self.run.as_str()) && act == self.act && now.saturating_duration_since(self.at) < ARMED_FOR
    }
}

/// The run as the record has it now.
pub fn read(run: &str) -> Option<Run> {
    jaynes_shell::store::Store::open(&jaynes_shell::home().join("record.sqlite3")).ok()?.state(run).ok().flatten()
}

/// Sends `act` on `run` as the person's, through the engine's own act_in, the one the page's /api/act calls: the same
/// body, so the app and the page can never send two different things. The one place that sends them, so a signed
/// path (G18) replaces this body alone. The core refuses what is not the person's; a refusal is said on the button's
/// row, in the engine's words.
pub fn send(run: &str, act: Act) -> Result<(), String> {
    let body = serde_json::json!({"run": run, "action": act.word()});
    jaynes_shell::page::act_in(&jaynes_shell::home(), &crate::answer::engine_program(), &body, By::Person).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{ARMED_FOR, Act, Armed, Facts, acts};
    use std::time::{Duration, Instant};

    fn facts() -> Facts {
        Facts { finished: false, halted: false, running: true, blocked: false, low_risk: false, risk_adds_steps: true, near_ceiling: false, stopped_by_ceiling: false, delivered: false, gone: false, stopped: false }
    }

    #[test]
    fn each_control_shows_only_when_it_can_do_something_as_on_the_page() {
        // at work: halted from its tab, or abandoned
        assert_eq!(acts(&facts()), [Act::Halt, Act::Abandon]);
        assert_eq!(acts(&Facts { halted: true, running: false, ..facts() }), [Act::Resume, Act::Abandon]);
        assert_eq!(acts(&Facts { blocked: true, running: false, ..facts() }), [Act::Continue, Act::Abandon]);
        assert_eq!(acts(&Facts { low_risk: true, ..facts() }), [Act::Halt, Act::RaiseRisk, Act::Abandon]);
        assert_eq!(acts(&Facts { near_ceiling: true, ..facts() }), [Act::Halt, Act::RaiseCeiling, Act::Abandon]);
        // halted while its phase is still at work: resumed, never halted twice
        assert_eq!(acts(&Facts { halted: true, ..facts() }), [Act::Resume, Act::Abandon]);
        assert_eq!(acts(&Facts { stopped_by_ceiling: true, blocked: true, running: false, ..facts() }), [Act::Continue, Act::RaiseCeiling, Act::Abandon]);
        // a finished run has none
        assert!(acts(&Facts { finished: true, ..facts() }).is_empty());
        // the design-lead, 22/11 (row 9): a run whose engine left the Mac refuses Abandon too, so nothing shows
        assert!(acts(&Facts { gone: true, halted: true, ..facts() }).is_empty());
    }

    // the engineer lead, 22/11, from the real-user's identity run: a pkill outside Jaynes ended its start without a
    // word, and the run said "working, running" with nobody driving it; the engine now says "Jaynes stopped while it
    // ran" and the person resumes it, so the app offers Resume, never a Halt of what nothing runs
    #[test]
    fn asked_a_run_jaynes_stopped_while_it_ran_is_resumed_from_its_tab() {
        assert_eq!(acts(&Facts { stopped: true, ..facts() }), [Act::Resume, Act::Abandon]);
        assert_eq!(acts(&Facts { stopped: true, low_risk: true, ..facts() }), [Act::Resume, Act::RaiseRisk, Act::Abandon]);
        // the card over its controls says what happened, in the engine's words, where it stopped
        assert_eq!(crate::tabs::halted_said(jaynes_shell::drive::JAYNES_STOPPED_SAID, Some("change")), "Jaynes stopped while it ran at “change”");
    }

    #[test]
    fn each_is_the_pages_own_action_word() {
        let words: Vec<&str> = [Act::Halt, Act::Resume, Act::Continue, Act::RaiseRisk, Act::RaiseCeiling, Act::Abandon].iter().map(|a| a.word()).collect();
        assert_eq!(words, ["halt", "resume", "continue", "raise-risk", "raise-ceiling", "abandon"]);
        // what cannot be taken back asks twice; a halt is resumed
        assert!(Act::Abandon.asks_twice() && !Act::Resume.asks_twice() && !Act::Halt.asks_twice());
        assert_eq!(Act::Halt.label(), "Halt");
        // raising the risk asks the person at more steps; its hint says what else it changes (core risk.rs, decide.rs)
        assert_eq!(Act::RaiseRisk.label(), "Ask me at more steps");
        assert_eq!(Act::RaiseRisk.tip(&facts()), Some("Takes this run on the full path: more steps and stops, and the time and token limit of a high-risk run."));
        assert_eq!(Act::Abandon.tip(&facts()), None);
    }

    #[test]
    fn asking_at_more_steps_shows_only_when_the_higher_risk_adds_steps_and_abandon_says_what_stays() {
        // a role with no step for the high risk only: raising it changes nothing but the limit, so it is not offered
        assert_eq!(acts(&Facts { low_risk: true, risk_adds_steps: false, ..facts() }), [Act::Halt, Act::Abandon]);
        assert_eq!(acts(&Facts { low_risk: true, risk_adds_steps: true, ..facts() }), [Act::Halt, Act::RaiseRisk, Act::Abandon]);
        // a run already delivered whose answers still correct it: Abandon keeps what it delivered
        let delivered = Facts { delivered: true, running: false, ..facts() };
        assert_eq!(Act::Abandon.tip(&delivered), Some("Ends the run here. What it delivered stays as it is, and you can no longer correct it at its stops."));
        assert_eq!(Act::Abandon.tip(&facts()), None);
    }

    #[test]
    fn a_control_pressed_once_stays_armed_a_few_seconds_and_only_on_its_run_s_tab() {
        let at = Instant::now();
        let armed = Armed { run: "r1".into(), act: Act::Abandon, at };
        assert!(armed.holds(Some("r1"), Act::Abandon, at + Duration::from_secs(1)));
        // it lapses: a press days later is a first press again
        assert!(!armed.holds(Some("r1"), Act::Abandon, at + ARMED_FOR));
        assert!(!armed.holds(Some("r1"), Act::Abandon, at + Duration::from_secs(86_400)));
        // another tab, or Claude Code's, holds nothing armed; nor another control
        assert!(!armed.holds(Some("r2"), Act::Abandon, at));
        assert!(!armed.holds(None, Act::Abandon, at));
        assert!(!armed.holds(Some("r1"), Act::Halt, at));
        assert!(ARMED_FOR <= Duration::from_secs(5) && ARMED_FOR >= Duration::from_secs(3), "a few seconds: {ARMED_FOR:?}");
    }
}
