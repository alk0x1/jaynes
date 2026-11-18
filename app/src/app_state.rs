//! What the window showed, kept for the next time the app opens (app-state.json in the engine's home,
//! ~/.jaynes/engine, beside the settings and apart from them):
//! what the middle showed (Claude Code, an agent, a run or the Inbox), whether the column, the steps panel and Changes
//! showed, the run tabs the person closed, the finished runs they cleared from the column, and where the window stood. Only what is seen: an agent by its address, a
//! run by its id, a closed tab by its mark and a fingerprint of its line, never a stop's words, a key or a folder.
//! Reading it only shows things: an agent or a run no longer there, or a file this app cannot read, opens on Claude
//! Code, and the log says why.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What the middle showed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Open {
    /// Claude Code's own terminal.
    #[default]
    Main,
    Agent {
        address: String,
    },
    Run {
        id: String,
    },
    Inbox,
}

/// A run's tab the person closed: closed until the run moves on, from how it stood then.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Closed {
    pub run: String,
    pub mark: String,
    /// The run's line then (its question, or where it was), as a fingerprint: it tells a change, not the words.
    pub line: u64,
}

/// Where the window stood, in points from the top left of its screen, and which screen (macOS's number for it).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub screen: Option<u64>,
}

/// A screen there is now: its number and size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    pub id: u64,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub open: Open,
    pub column: bool,
    pub steps: bool,
    pub changes: bool,
    pub closed: Vec<Closed>,
    pub frame: Option<Frame>,
    /// The bars' widths as the person dragged them (bars.rs); none is the bar's own.
    pub widths: Widths,
    /// The finished runs the person cleared from the column, by id: still in the record and in `jaynes runs`.
    pub cleared: Vec<String>,
}

/// How wide the person left each bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Widths {
    pub column: Option<f32>,
    pub steps: Option<f32>,
    pub changes: Option<f32>,
}

impl Default for State {
    fn default() -> Self {
        State { open: Open::Main, column: true, steps: true, changes: false, closed: Vec::new(), frame: None, widths: Widths::default(), cleared: Vec::new() }
    }
}

/// The smallest window kept: one smaller opens at the app's own size.
const SMALLEST: (f32, f32) = (640., 400.);
/// How much of a kept window must lie on a screen there is now.
const ON_SCREEN: f32 = 120.;

pub fn path() -> PathBuf {
    crate::testing::var("JAYNES_APP_STATE").map(PathBuf::from).unwrap_or_else(|| jaynes_shell::home().join("app-state.json"))
}

/// The state kept in `path`, and why it is not, when it is there but cannot be read (none is not a reason: the
/// first opening).
pub fn load(path: &Path) -> (State, Option<String>) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (State::default(), None),
        Err(e) => return (State::default(), Some(format!("{} cannot be read ({e}): the app opens on Claude Code", path.display()))),
    };
    let read = serde_json::from_str::<serde_json::Value>(&text).map_err(|e| e.to_string()).and_then(|v| match v {
        serde_json::Value::Object(_) => serde_json::from_value::<State>(v).map_err(|e| e.to_string()),
        _ => Err("not a JSON object".into()),
    });
    match read {
        Ok(s) => (s, None),
        Err(e) => (State::default(), Some(format!("{} is not what this app keeps ({e}): the app opens on Claude Code", path.display()))),
    }
}

pub fn save(path: &Path, s: &State) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(s).expect("the state is JSON") + "\n")?;
    std::fs::rename(&tmp, path)
}

/// What the middle opens on: what it showed, if it is still there, else Claude Code and why.
pub fn open_on(open: &Open, agent_there: impl Fn(&str) -> bool, run_there: impl Fn(&str) -> bool) -> (Open, Option<String>) {
    match open {
        Open::Agent { address } if !agent_there(address) => (Open::Main, Some(format!("the agent {address} open last is gone: the app opens on Claude Code"))),
        Open::Run { id } if !run_there(id) => (Open::Main, Some(format!("the run {id} open last is not in the record: the app opens on Claude Code"))),
        other => (other.clone(), None),
    }
}

/// Where the window opens: where it stood, if that is big enough and on its screen, or on the main one when its
/// screen is gone (`screen` is then none); else the app's own place (None), as when no screen is known (all asleep):
/// where it stood cannot be told to be on one then.
pub fn frame_on(frame: Option<Frame>, screens: &[Screen], main: Option<u64>) -> Option<Frame> {
    let f = frame?;
    let sane = [f.x, f.y, f.width, f.height].iter().all(|v| v.is_finite()) && f.width >= SMALLEST.0 && f.height >= SMALLEST.1;
    if !sane {
        return None;
    }
    let on = |s: &Screen| (f.x + f.width).min(s.width) - f.x.max(0.) >= ON_SCREEN && (f.y + f.height).min(s.height) - f.y.max(0.) >= ON_SCREEN;
    let its = f.screen.and_then(|id| screens.iter().find(|s| s.id == id));
    match its {
        Some(s) => on(s).then_some(f),
        None => screens.iter().find(|s| Some(s.id) == main).or(screens.first()).filter(|s| on(s)).map(|_| Frame { screen: None, ..f }),
    }
}

/// A window's bounds on its screen, as kept.
pub fn frame_of(b: gpui::Bounds<gpui::Pixels>, screen: Option<u64>) -> Frame {
    Frame { x: b.origin.x.into(), y: b.origin.y.into(), width: b.size.width.into(), height: b.size.height.into(), screen }
}

/// A line's fingerprint (FNV-1a, the same in every build): a closed tab tells its run moved on without keeping its
/// words.
pub fn fingerprint(line: &str) -> u64 {
    line.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// A mark as the file keeps it.
pub fn mark_word(m: jaynes_shell::listing::Mark) -> &'static str {
    use jaynes_shell::listing::Mark::*;
    match m {
        Needs => "needs",
        Working => "working",
        Halted => "halted",
        Blocked => "blocked",
        Delivered => "delivered",
        Merged => "merged",
        Undone => "undone",
        Abandoned => "abandoned",
    }
}

pub fn mark_of(word: &str) -> Option<jaynes_shell::listing::Mark> {
    use jaynes_shell::listing::Mark::*;
    [Needs, Working, Halted, Blocked, Delivered, Merged, Undone, Abandoned].into_iter().find(|m| mark_word(*m) == word)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("jaynes-app-state-{}-{}", std::process::id(), fingerprint(&format!("{:?}", std::thread::current().id()))));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn what_was_kept_comes_back_as_it_was() {
        let p = dir().join("app-state.json");
        let s = State {
            open: Open::Agent { address: "w1:t2".into() },
            column: false,
            steps: false,
            changes: true,
            closed: vec![Closed { run: "r1".into(), mark: "needs".into(), line: fingerprint("Pick one") }],
            frame: Some(Frame { x: 10., y: 20., width: 1200., height: 800., screen: Some(2) }),
            widths: Widths { column: Some(52.), steps: Some(410.), changes: None },
            cleared: vec!["r7".into(), "r8".into()],
        };
        save(&p, &s).unwrap();
        assert_eq!(load(&p), (s, None));
    }

    #[test]
    fn none_kept_opens_as_the_first_time_and_a_broken_file_opens_on_claude_code_saying_why() {
        let d = dir();
        assert_eq!(load(&d.join("none.json")), (State::default(), None));
        for broken in ["{not json", r#"{"open":{"kind":"spaceship"}}"#, r#"{"column":"yes"}"#, "[]"] {
            let p = d.join("broken.json");
            std::fs::write(&p, broken).unwrap();
            let (s, why) = load(&p);
            assert_eq!(s, State::default(), "{broken}");
            assert!(why.is_some_and(|w| w.contains("Claude Code")), "{broken}");
        }
        // a field it does not know (a later version's) or one missing is no fault
        let p = d.join("partial.json");
        std::fs::write(&p, r#"{"steps":false,"later":1}"#).unwrap();
        assert_eq!(load(&p), (State { steps: false, ..State::default() }, None));
    }

    #[test]
    fn an_agent_or_a_run_no_longer_there_opens_on_claude_code_saying_why() {
        let there = |a: &str| a == "w1:t2";
        let run = |r: &str| r == "r1";
        let agent = Open::Agent { address: "w1:t2".into() };
        assert_eq!(open_on(&agent, there, run), (agent.clone(), None));
        let (o, why) = open_on(&Open::Agent { address: "w9:t9".into() }, there, run);
        assert_eq!(o, Open::Main);
        assert!(why.unwrap().contains("w9:t9"));
        let (o, why) = open_on(&Open::Run { id: "gone".into() }, there, run);
        assert_eq!(o, Open::Main);
        assert!(why.unwrap().contains("gone"));
        assert_eq!(open_on(&Open::Inbox, there, run), (Open::Inbox, None));
    }

    #[test]
    fn the_window_opens_where_it_stood_only_if_that_is_on_a_screen_now_and_big_enough() {
        let laptop = Screen { id: 1, width: 1512., height: 982. };
        let wide = Screen { id: 2, width: 2560., height: 1440. };
        let f = Frame { x: 40., y: 30., width: 1200., height: 800., screen: Some(1) };
        assert_eq!(frame_on(Some(f), &[laptop, wide], Some(1)), Some(f));
        // on the wide screen, far right: still on it
        let right = Frame { x: 2000., screen: Some(2), ..f };
        assert_eq!(frame_on(Some(right), &[laptop, wide], Some(1)), Some(right));
        // the wide screen unplugged since: on the main one, if it fits there, else the app's own place
        assert_eq!(frame_on(Some(Frame { screen: Some(2), ..f }), &[laptop], Some(1)), Some(Frame { screen: None, ..f }));
        assert_eq!(frame_on(Some(right), &[laptop], Some(1)), None);
        // barely on the edge of its screen
        assert_eq!(frame_on(Some(Frame { x: 1450., ..f }), &[laptop], Some(1)), None);
        assert_eq!(frame_on(Some(Frame { y: -790., ..f }), &[laptop], Some(1)), None);
        // too small, or not a number
        assert_eq!(frame_on(Some(Frame { width: 200., ..f }), &[laptop], Some(1)), None);
        assert_eq!(frame_on(Some(Frame { x: f32::NAN, ..f }), &[laptop], Some(1)), None);
        assert_eq!(frame_on(None, &[laptop], Some(1)), None);
        // no screen known (all asleep): the app's own place, not somewhere off every screen
        assert_eq!(frame_on(Some(f), &[], None), None);
    }

    #[test]
    fn the_file_keeps_no_words_of_a_stop_nor_a_folder() {
        let line = "Choose the colours and the fonts for /Users/someone/secret";
        let s = State { open: Open::Run { id: "r1".into() }, closed: vec![Closed { run: "r1".into(), mark: "needs".into(), line: fingerprint(line) }], ..State::default() };
        let text = serde_json::to_string(&s).unwrap();
        assert!(!text.contains("colours") && !text.contains("/Users"), "{text}");
        // the fingerprint tells a change of line
        assert_ne!(fingerprint(line), fingerprint("Choose the colours"));
        assert_eq!(fingerprint(""), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn every_mark_has_a_word_and_back() {
        use jaynes_shell::listing::Mark::*;
        for m in [Needs, Working, Halted, Blocked, Delivered, Merged, Undone, Abandoned] {
            assert_eq!(mark_of(mark_word(m)), Some(m));
        }
        assert_eq!(mark_of("spaceship"), None);
    }
}
