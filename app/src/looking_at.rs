//! The person's presence at a stop, as the page tells it (jaynes_shell::presence::look, what the people's minutes of
//! a run's summary are measured from): a signal every presence::EVERY seconds, only while the stop is on the screen,
//! the window in front with the keyboard, and the person pressed a key, moved the pointer or scrolled in the last
//! minute. A window closed, behind another app or idle sends nothing.

use std::sync::atomic::{AtomicU64, Ordering};

/// How long after a key, a move of the pointer or a scroll the person still counts as there.
const STILL_THERE_MS: u64 = 60_000;

/// When the person last pressed a key, moved the pointer or scrolled in a window of the app (ms since the epoch).
static LAST_INPUT_MS: AtomicU64 = AtomicU64::new(0);

/// A key, the pointer or a scroll, now.
pub fn touched() {
    LAST_INPUT_MS.store(jaynes_shell::store::now(), Ordering::Relaxed);
}

/// What the window shows and how the person stands to it, at a tick.
pub struct Seen {
    /// The window is in front with the keyboard.
    pub front: bool,
    /// When the person last touched it.
    pub last_input_ms: Option<u64>,
    /// The stop on the screen: its run and question.
    pub stop: Option<(String, String)>,
}

/// The stop the person is looking at now, if the signal counts.
pub fn counts(seen: &Seen, now_ms: u64) -> Option<(String, String)> {
    let lately = seen.last_input_ms.is_some_and(|t| now_ms.saturating_sub(t) <= STILL_THERE_MS);
    (seen.front && lately).then(|| seen.stop.clone()).flatten()
}

/// The last touch, as `Seen` wants it.
pub fn last_input() -> Option<u64> {
    Some(LAST_INPUT_MS.load(Ordering::Relaxed)).filter(|t| *t > 0)
}

/// Tells the engine the person is at `stop`, off the window's thread: a stop no longer open is not an error.
pub fn tell(stop: (String, String)) {
    std::thread::spawn(move || {
        let home = jaynes_shell::home();
        if let Err(e) = jaynes_shell::presence::look(&home, &stop.0, &stop.1, &jaynes_core::question::By::Person, jaynes_shell::store::now()) {
            tracing::warn!("the person's presence at {} was not kept: {e}", stop.0);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{Seen, counts};

    #[test]
    fn a_signal_counts_only_with_the_stop_shown_the_window_in_front_and_a_hand_on_it_lately() {
        let now = 1_000_000;
        let stop = Some(("r1".to_owned(), "q1".to_owned()));
        let seen = |front: bool, last: Option<u64>, stop: Option<(String, String)>| Seen { front, last_input_ms: last, stop };
        assert_eq!(counts(&seen(true, Some(now - 5_000), stop.clone()), now), stop.clone());
        // a minute without a key, the pointer or a scroll: nobody is looking
        assert_eq!(counts(&seen(true, Some(now - 61_000), stop.clone()), now), None);
        assert_eq!(counts(&seen(true, None, stop.clone()), now), None);
        // behind another app, or closed
        assert_eq!(counts(&seen(false, Some(now - 1_000), stop.clone()), now), None);
        // no stop on the screen (an agent, the Inbox, Changes over it)
        assert_eq!(counts(&seen(true, Some(now - 1_000), None), now), None);
    }
}
