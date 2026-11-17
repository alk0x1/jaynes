//! The person's agents as Herdr arranges them: its spaces (the `herdr-app` document in herdr-core, which both apps
//! share) and the live terminals in each, in the order of the space's tabs. Each agent gets the state the column
//! shows beside its dot: waiting for the person, working, finished and not yet looked at, or idle.

use crate::hc;
use gpui::*;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

/// The spaces as the column shows them, read from herdr-core by a thread of their own once a second.
pub struct Agents {
    pub spaces: Vec<Space>,
    /// Why herdr-core could not be read, shown instead of the spaces.
    pub trouble: Option<String>,
    /// The agent open in the middle: the person looks at it, so what it finished is seen.
    pub open: Option<String>,
    core: Arc<hc::Core>,
    _watch: Task<()>,
}

impl Agents {
    pub fn new(core: Arc<hc::Core>, cx: &mut Context<Self>) -> Self {
        let reader = core.clone();
        let (tx, rx) = async_channel::bounded::<Result<Vec<Space>, String>>(1);
        std::thread::Builder::new()
            .name("agents".into())
            .spawn(move || watch(&reader, tx))
            .expect("cannot start the thread that reads the agents");
        let watch = cx.spawn(async move |this, cx| {
            while let Ok(read) = rx.recv().await {
                let gone = this
                    .update(cx, |this, cx| {
                        match read {
                            Ok(spaces) => (this.spaces, this.trouble) = (spaces, None),
                            Err(e) => this.trouble = Some(e),
                        }
                        this.mark_seen();
                        cx.notify();
                    })
                    .is_err();
                if gone {
                    break;
                }
            }
        });
        Agents { spaces: Vec::new(), trouble: None, open: None, core, _watch: watch }
    }

    /// Opens an agent in the middle (None: none), marking at once what it finished as seen.
    pub fn open(&mut self, address: Option<String>, cx: &mut Context<Self>) {
        self.open = address;
        self.mark_seen();
        cx.notify();
    }

    /// Writes `herdr.seen` on the open agent when it finished a turn the person had not looked at, as Herdr does
    /// when the person looks at it: both apps read the same label, so it stops waiting for review in both. The
    /// column shows it seen at once, before herdr-core is read again.
    fn mark_seen(&mut self) {
        let Some((address, turn)) = seen_now(&self.spaces, self.open.as_deref()) else { return };
        for a in self.spaces.iter_mut().flat_map(|s| s.agents.iter_mut()).filter(|a| a.address == address) {
            a.state = State::Idle;
        }
        let core = self.core.clone();
        std::thread::spawn(move || {
            if let Err(e) = core.call("terminal.label", json!({"terminal": address, "labels": {"herdr.seen": turn.to_string()}})) {
                tracing::warn!("cannot mark {address} seen: {e:#}");
            }
        });
    }

    /// How many agents wait for the person, and how many finished unseen.
    pub fn counts(&self) -> (usize, usize) {
        let all = || self.spaces.iter().flat_map(|s| &s.agents);
        (all().filter(|a| matches!(a.state, State::Waiting | State::Error)).count(), all().filter(|a| a.state == State::ToReview).count())
    }
}

/// Sends the spaces at once, then whenever they change. A core without Herdr's document has no spaces yet.
fn watch(core: &hc::Core, tx: async_channel::Sender<Result<Vec<Space>, String>>) {
    let mut last: Option<Result<Vec<Space>, String>> = None;
    loop {
        let read = core.call("terminal.list", json!({})).map_err(|e| format!("{e:#}")).map(|list| {
            let doc = core.call("doc.get", json!({"name": "herdr-app"})).unwrap_or(Value::Null);
            arrange(&doc, &list)
        });
        if last.as_ref() != Some(&read) {
            if tx.send_blocking(read.clone()).is_err() {
                return;
            }
            last = Some(read);
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// The open agent and its turn, when it finished a turn the person had not looked at.
pub fn seen_now(spaces: &[Space], open: Option<&str>) -> Option<(String, u64)> {
    let open = open?;
    let a = spaces.iter().flat_map(|s| &s.agents).find(|a| a.address == open)?;
    (a.state == State::ToReview).then(|| (a.address.clone(), a.turn))
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// It asks the person something: a permission, a question, a login.
    Waiting,
    Error,
    Working,
    /// It finished a turn the person has not looked at (its turn is past `herdr.seen`).
    ToReview,
    Idle,
}

impl State {
    pub fn word(self) -> &'static str {
        match self {
            State::Waiting => "waiting",
            State::Error => "error",
            State::Working => "working",
            State::ToReview => "to review",
            State::Idle => "idle",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Agent {
    pub address: String,
    pub title: String,
    pub state: State,
    /// When its state last changed (ms since the epoch).
    pub since_ms: u64,
    /// A plain shell, not a coding agent.
    pub shell: bool,
    /// How many turns it finished; `herdr.seen` holds the last one the person looked at.
    pub turn: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Space {
    pub id: String,
    pub name: String,
    pub folder: String,
    pub agents: Vec<Agent>,
}

/// The spaces in Herdr's order, each with its live terminals: those its tabs hold first, in the tabs' order, then the
/// ones labelled with the space but in no tab. `doc` is `doc.get herdr-app`'s reply, `list` is `terminal.list`'s.
pub fn arrange(doc: &Value, list: &Value) -> Vec<Space> {
    let live: Vec<&Value> = list["terminals"].as_array().map(|ts| ts.iter().filter(|t| t["state"] == "live").collect()).unwrap_or_default();
    let spaces = doc["value"]["spaces"].as_array().cloned().unwrap_or_default();
    spaces
        .iter()
        .map(|space| {
            let id = space["id"].as_str().unwrap_or_default().to_owned();
            let mut order = Vec::new();
            for tab in space["tabs"].as_array().into_iter().flatten() {
                blocks(&tab["root"], &mut order);
            }
            let mine: Vec<&Value> = live.iter().copied().filter(|t| t["labels"]["herdr.space"].as_str() == Some(id.as_str())).collect();
            let mut agents: Vec<Agent> = order.iter().filter_map(|a| live.iter().find(|t| t["address"].as_str() == Some(a.as_str()))).map(|t| agent(t)).collect();
            for t in &mine {
                if !agents.iter().any(|a| Some(a.address.as_str()) == t["address"].as_str()) {
                    agents.push(agent(t));
                }
            }
            let folder = space["folder"].as_str().unwrap_or_default().to_owned();
            let name = Some(space["name"].as_str().unwrap_or_default().trim().to_owned())
                .filter(|n| !n.is_empty())
                // Herdr names an unnamed space where it shows its terminals ("helm / 1")
                .or_else(|| mine.iter().find_map(|t| t["labels"]["where"].as_str().and_then(|w| w.rsplit_once(" / ")).map(|(n, _)| n.to_owned())))
                .unwrap_or_else(|| folder.rsplit('/').find(|p| !p.is_empty()).unwrap_or("space").to_owned());
            Space { id, name, folder, agents }
        })
        .collect()
}

/// The terminals a tab's layout holds, in order (rows and columns nest).
fn blocks(node: &Value, out: &mut Vec<String>) {
    if let Some(t) = node["content"]["terminal"].as_str() {
        out.push(t.to_owned());
    }
    for child in node["children"].as_array().into_iter().flatten() {
        blocks(child, out);
    }
}

fn agent(t: &Value) -> Agent {
    let turn = t["turn"].as_u64().unwrap_or(0);
    let seen = t["labels"]["herdr.seen"].as_str().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    let state = match t["status"]["state"].as_str() {
        Some("blocked") => State::Waiting,
        Some("error") => State::Error,
        Some("working") => State::Working,
        Some("done") if turn > seen => State::ToReview,
        _ => State::Idle,
    };
    // terminal.list names the program in the status (an agent that reports) or only in what runs in front; `agent` is
    // `hc ls`'s summary, kept for older cores
    let base = |v: &Value| v.as_str().map(|s| s.rsplit('/').next().unwrap_or(s).trim_start_matches('-').to_owned());
    let program = [t["status"]["app"].as_str().map(str::to_owned), t["agent"].as_str().map(str::to_owned), base(&t["foreground"]["argv"][0]), base(&t["argv"][0])]
        .into_iter()
        .flatten()
        .find(|p| !p.is_empty())
        .unwrap_or_default();
    let shell = program.is_empty() || ["zsh", "bash", "fish", "sh", "nu"].contains(&program.as_str());
    let program = program.as_str();
    let address = t["address"].as_str().unwrap_or_default().to_owned();
    let title = [t["title"].as_str(), (!program.is_empty()).then_some(program)]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| address.clone());
    Agent { address, title, state, since_ms: t["status_ms"].as_u64().or_else(|| t["created_ms"].as_u64()).unwrap_or(0), shell, turn }
}

/// How long ago, shortly: now, 5m, 7h, 2d.
pub fn age(since_ms: u64, now_ms: u64) -> String {
    let minutes = now_ms.saturating_sub(since_ms) / 60_000;
    match minutes {
        0 => "now".into(),
        m if m < 60 => format!("{m}m"),
        m if m < 48 * 60 => format!("{}h", m / 60),
        m => format!("{}d", m / (24 * 60)),
    }
}

#[cfg(test)]
mod tests {
    // not super::*: gpui's glob brings its own `test` attribute, which would shadow the standard one
    use super::{State, Value, age, arrange, seen_now};
    use serde_json::json;

    fn term(address: &str, space: &str, state: Option<&str>, turn: u64, seen: u64) -> Value {
        json!({
            "address": address, "state": "live", "agent": "claude-code", "title": format!("title {address}"),
            "turn": turn, "status_ms": 1000,
            "status": state.map(|s| json!({"app": "claude-code", "state": s})),
            "labels": {"herdr.space": space, "herdr.seen": seen.to_string(), "where": "named / 1"}
        })
    }

    fn doc() -> Value {
        json!({"value": {"spaces": [
            {"id": "s1", "name": "jaynes/tenure", "folder": "/p/jaynes", "tabs": [
                {"id": "tb1", "name": "", "root": {"type": "row", "children": [
                    {"type": "block", "id": "b1", "content": {"terminal": "t2"}},
                    {"type": "row", "children": [{"type": "block", "id": "b2", "content": {"terminal": "t1"}}]}
                ]}}
            ]},
            {"id": "s2", "name": "", "folder": "/p/helm", "tabs": []}
        ]}, "version": 3})
    }

    #[test]
    fn spaces_keep_herdrs_order_and_agents_their_tabs_order() {
        let list = json!({"terminals": [term("t1", "s1", Some("working"), 3, 3), term("t2", "s1", Some("blocked"), 5, 5)]});
        let spaces = arrange(&doc(), &list);
        assert_eq!(spaces.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s1", "s2"]);
        assert_eq!(spaces[0].agents.iter().map(|a| a.address.as_str()).collect::<Vec<_>>(), ["t2", "t1"]);
        assert_eq!(spaces[0].name, "jaynes/tenure");
    }

    #[test]
    fn the_state_says_what_the_person_should_do() {
        let list = json!({"terminals": [
            term("t1", "s1", Some("blocked"), 1, 1),
            term("t2", "s1", Some("working"), 1, 1),
            term("t3", "s1", Some("done"), 7, 6),
            term("t4", "s1", Some("done"), 7, 7),
            term("t5", "s1", Some("idle"), 2, 2),
            term("t6", "s1", Some("error"), 2, 2),
        ]});
        let space = &arrange(&doc(), &list)[0];
        let state = |t: &str| space.agents.iter().find(|a| a.address == t).unwrap().state;
        let states: Vec<State> = ["t1", "t2", "t3", "t4", "t5", "t6"].into_iter().map(state).collect();
        assert_eq!(states, [State::Waiting, State::Working, State::ToReview, State::Idle, State::Idle, State::Error]);
    }

    #[test]
    fn a_terminal_in_no_tab_still_shows_in_its_space_after_the_tabs() {
        let list = json!({"terminals": [term("t9", "s1", Some("done"), 1, 1), term("t1", "s1", None, 0, 0)]});
        let spaces = arrange(&doc(), &list);
        assert_eq!(spaces[0].agents.iter().map(|a| a.address.as_str()).collect::<Vec<_>>(), ["t1", "t9"]);
    }

    #[test]
    fn finished_terminals_and_terminals_outside_the_spaces_do_not_show() {
        let mut gone = term("t1", "s1", Some("done"), 1, 1);
        gone["state"] = json!("finished");
        let mut app = term("t5", "s1", None, 0, 0);
        app["labels"] = json!({"jaynes.app": "main"});
        let spaces = arrange(&doc(), &json!({"terminals": [gone, app]}));
        assert!(spaces.iter().all(|s| s.agents.is_empty()));
    }

    #[test]
    fn a_space_without_a_name_takes_the_one_herdr_shows() {
        let mut t = term("t7", "s2", Some("done"), 1, 1);
        t["labels"]["where"] = json!("helm / 1");
        let spaces = arrange(&doc(), &json!({"terminals": [t]}));
        assert_eq!(spaces[1].name, "helm");
        // and with no terminal to tell, its folder's name
        assert_eq!(arrange(&doc(), &json!({"terminals": []}))[1].name, "helm");
    }

    #[test]
    fn a_shell_is_told_apart_and_a_title_falls_back() {
        let mut shell = term("t3", "s1", None, 0, 0);
        shell["agent"] = json!("zsh");
        shell["title"] = Value::Null;
        let a = &arrange(&doc(), &json!({"terminals": [shell]}))[0].agents[0];
        assert!(a.shell);
        assert_eq!(a.title, "zsh");
        assert_eq!(a.state, State::Idle);
    }

    #[test]
    fn the_full_record_names_its_program_in_the_status_and_foreground_not_agent() {
        // terminal.list's records carry no `agent`: the program shows in `status.app`, or only in `foreground`
        let mut claude = term("t1", "s1", Some("working"), 1, 1);
        claude.as_object_mut().unwrap().remove("agent");
        let mut quiet = term("t2", "s1", None, 0, 0);
        quiet.as_object_mut().unwrap().remove("agent");
        quiet["foreground"] = json!({"argv": ["claude"], "name": "2.1.325"});
        let mut shell = term("t3", "s1", None, 0, 0);
        shell.as_object_mut().unwrap().remove("agent");
        shell["argv"] = json!(["/bin/zsh", "-l"]);
        let spaces = arrange(&doc(), &json!({"terminals": [claude, quiet, shell]}));
        let shell_of = |t: &str| spaces[0].agents.iter().find(|a| a.address == t).unwrap().shell;
        assert_eq!([shell_of("t1"), shell_of("t2"), shell_of("t3")], [false, false, true]);
    }

    #[test]
    fn the_open_agent_is_seen_when_it_finished_a_turn_the_person_had_not_looked_at() {
        let list = json!({"terminals": [
            term("t1", "s1", Some("done"), 7, 6),
            term("t2", "s1", Some("done"), 7, 7),
            term("t3", "s1", Some("working"), 8, 7),
        ]});
        let spaces = arrange(&doc(), &list);
        // open and finished unseen: its turn becomes herdr.seen, as Herdr does when the person looks
        assert_eq!(seen_now(&spaces, Some("t1")), Some(("t1".to_string(), 7)));
        // already seen, still working, closed, or not there: nothing to write
        assert_eq!(seen_now(&spaces, Some("t2")), None);
        assert_eq!(seen_now(&spaces, Some("t3")), None);
        assert_eq!(seen_now(&spaces, None), None);
        assert_eq!(seen_now(&spaces, Some("t9")), None);
    }

    #[test]
    fn a_terminal_that_never_reported_a_state_is_as_old_as_its_start() {
        let mut shell = term("t3", "s1", None, 0, 0);
        shell.as_object_mut().unwrap().remove("status_ms");
        shell["created_ms"] = json!(5000);
        assert_eq!(arrange(&doc(), &json!({"terminals": [shell]}))[0].agents[0].since_ms, 5000);
    }

    #[test]
    fn age_is_short() {
        let m = 60_000;
        assert_eq!(age(0, 20_000), "now");
        assert_eq!(age(0, 5 * m), "5m");
        assert_eq!(age(0, 7 * 60 * m), "7h");
        assert_eq!(age(0, 50 * 60 * m), "2d");
        assert_eq!(age(10, 5), "now");
    }
}
