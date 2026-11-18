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
    /// What the open agent asks, while it waits for the person.
    pub ask: Option<Ask>,
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
                            Ok(spaces) => {
                                // the agent open in the front window is the one the person looks at
                                let looking = this.open.clone().filter(|_| crate::notify::looking());
                                crate::notify::post(&news(&this.spaces, &spaces, looking.as_deref()), false);
                                (this.spaces, this.trouble) = (spaces, None);
                            }
                            Err(e) => this.trouble = Some(e),
                        }
                        this.mark_seen();
                        this.read_ask(cx);
                        cx.notify();
                    })
                    .is_err();
                if gone {
                    break;
                }
            }
        });
        Agents { spaces: Vec::new(), trouble: None, open: None, ask: None, core, _watch: watch }
    }

    /// Opens an agent in the middle (None: none), marking at once what it finished as seen.
    /// Spaces read just now, by whoever changed them (a split, a new agent): shown at once, not at the next reading.
    pub fn read_now(&mut self, spaces: Vec<Space>, cx: &mut Context<Self>) {
        self.spaces = spaces;
        self.mark_seen();
        self.read_ask(cx);
        cx.notify();
    }

    pub fn open(&mut self, address: Option<String>, cx: &mut Context<Self>) {
        self.open = address;
        self.mark_seen();
        self.read_ask(cx);
        cx.notify();
    }

    /// Reads what the open agent asks from its screen when it waits; nothing when it does not.
    fn read_ask(&mut self, cx: &mut Context<Self>) {
        let open = self.open.as_deref().and_then(|o| self.spaces.iter().flat_map(|s| &s.agents).find(|a| a.address == o)).cloned();
        let Some(a) = open.filter(|a| a.state == State::Waiting) else {
            self.ask = None;
            return;
        };
        let core = self.core.clone();
        let at = a.address.clone();
        cx.spawn(async move |this, cx| {
            let screen = cx.background_executor().spawn(async move { core.call("terminal.read", json!({"terminal": at})) }).await;
            let choices = screen.ok().and_then(|s| s["text"].as_str().map(crate::ask::choices)).unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                // still open and still waiting when the screen came back
                let waits = this.spaces.iter().flat_map(|s| &s.agents).any(|x| x.address == a.address && x.state == State::Waiting);
                if this.open.as_deref() == Some(a.address.as_str()) && waits {
                    let ask = Some(Ask { address: a.address.clone(), title: a.title.clone(), what: crate::ask::what(a.kind.as_deref(), &a.msg), choices });
                    if this.ask != ask {
                        this.ask = ask;
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Writes `herdr.seen` on the open agent when it finished a turn the person had not looked at, as Herdr does
    /// when the person looks at it: both apps read the same label, so it stops waiting for review in both. The
    /// column shows it seen at once, before herdr-core is read again.
    fn mark_seen(&mut self) {
        let Some((address, turn)) = seen_now(&self.spaces, self.open.as_deref()) else { return };
        self.write_seen(address, turn);
    }

    /// "Mark reviewed" in the Inbox: what the agent finished is seen without opening it.
    pub fn mark_reviewed(&mut self, address: &str, cx: &mut Context<Self>) {
        if let Some((address, turn)) = seen_now(&self.spaces, Some(address)) {
            self.write_seen(address, turn);
            cx.notify();
        }
    }

    fn write_seen(&mut self, address: String, turn: u64) {
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
/// The spaces and their agents, read now from the core and Herdr's document.
pub fn read_spaces(core: &hc::Core) -> Result<Vec<Space>, String> {
    core.call("terminal.list", json!({})).map_err(|e| format!("{e:#}")).map(|list| {
        let doc = core.call("doc.get", json!({"name": "herdr-app"})).unwrap_or(Value::Null);
        arrange(&doc, &list)
    })
}

fn watch(core: &hc::Core, tx: async_channel::Sender<Result<Vec<Space>, String>>) {
    let mut last: Option<Result<Vec<Space>, String>> = None;
    loop {
        let read = read_spaces(core);
        if last.as_ref() != Some(&read) {
            if tx.send_blocking(read.clone()).is_err() {
                return;
            }
            last = Some(read);
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// A fresh id as Herdr makes its own: a prefix, then 32 hex digits.
fn new_id(prefix: &str) -> String {
    use std::io::Read as _;
    let mut bytes = [0u8; 16];
    let random = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).is_ok();
    if !random {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        bytes = (now ^ ((std::process::id() as u128) << 64)).to_le_bytes();
    }
    format!("{prefix}{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// Herdr's document with a new tab at the end of a space, holding one terminal, drawn as Herdr draws a tab: a row
/// whose id is the tab's, with one block. Everything else stays as it was. None when the space is gone.
pub fn with_tab(value: &Value, space: &str, tab: &str, block: &str, terminal: &str) -> Option<Value> {
    let mut out = value.clone();
    let found = out["spaces"].as_array_mut()?.iter_mut().find(|s| s["id"].as_str() == Some(space))?;
    if !found["tabs"].is_array() {
        found["tabs"] = json!([]);
    }
    found["tabs"].as_array_mut()?.push(json!({
        "id": tab, "name": "",
        "root": {"type": "row", "id": tab, "children": [{"type": "block", "id": block, "content": {"terminal": terminal}}]}
    }));
    Some(out)
}

/// Where a new agent beside `open` starts: the folder `open` works in, else the space's. Herdr's folder for a space
/// can be another project's (a space made in one folder and used for another).
pub fn new_agent_folder(space: &Space, open: &str) -> Option<String> {
    let cwd = space.agents.iter().find(|a| a.address == open).map(|a| a.cwd.clone());
    cwd.into_iter().chain([space.folder.clone()]).find(|f| !f.is_empty() && std::path::Path::new(f).is_dir())
}

/// Starts the new agent (Claude Code, unless Settings name another) in a folder, in a tab of its own at the end of the space, so Herdr shows it too; its address.
/// The document is written only if no one wrote it since it was read, else read again: Herdr writes it too. Without
/// the tab, the agent still shows in the space here, by its labels.
pub fn start_in(core: &hc::Core, space: &Space, folder: Option<String>, display: Value) -> Result<String, String> {
    run_in(core, space, crate::prefs::new_agent_argv(&crate::prefs::get().new_agent), folder, display).map_err(|e| format!("cannot start the new agent: {e}"))
}

/// Runs a program in a new tab of a space, in a folder: labelled with the space and the tab, and the tab added to
/// Herdr's herdr-app document, as `start_in` does for Claude Code.
pub fn run_in(core: &hc::Core, space: &Space, argv: Vec<String>, folder: Option<String>, display: Value) -> Result<String, String> {
    let (tab, block) = (new_id("tb"), new_id("p"));
    let labels = json!({"herdr.space": space.id, "herdr.tab": tab});
    let address = core.run(argv, folder, labels, None, None, display).map_err(|e| format!("{e:#}"))?;
    if let Err(e) = change_doc(core, |v| with_tab(v, &space.id, &tab, &block, &address)) {
        tracing::warn!("{address} has no tab in Herdr: {e}");
    }
    Ok(address)
}

/// Runs the person's shell split beside a pane of a tab (to its right or under it), in the pane's folder: labelled
/// with the space and the tab, and the split written to Herdr's document, so Herdr shows it too.
/// `size`: the pane's, so the shell starts at the size it shows at.
#[allow(clippy::too_many_arguments)]
pub fn split_beside(core: &hc::Core, space: &str, tab: &str, target: &str, folder: Option<String>, dir: crate::layout::Dir, size: Option<(u16, u16)>, display: Value) -> Result<String, String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let labels = json!({"herdr.space": space, "herdr.tab": tab});
    let address = core.run(vec![shell, "-l".into()], folder, labels, None, size, display).map_err(|e| format!("{e:#}"))?;
    let (block, node) = (new_id("p"), new_id("n"));
    if let Err(e) = change_doc(core, |v| crate::layout::split(v, tab, target, &address, &block, &node, dir)) {
        tracing::warn!("{address} is not split in Herdr: {e}");
    }
    Ok(address)
}

/// Changes Herdr's herdr-app document: read it, change it, write it only if its version still matches, again when
/// it does not (up to five times). `change` says None when what it changes is gone.
fn change_doc(core: &hc::Core, change: impl Fn(&Value) -> Option<Value>) -> Result<(), String> {
    for _ in 0..5 {
        let read = core.call("doc.get", json!({"name": "herdr-app"})).map_err(|e| format!("{e:#}"));
        let written = read.and_then(|doc| {
            let value = change(&doc["value"]).ok_or("what it changes is gone")?;
            core.call("doc.set", json!({"name": "herdr-app", "value": value, "version": doc["version"]})).map_err(|e| format!("{e:#}"))
        });
        match written {
            Ok(_) => return Ok(()),
            Err(e) if e.contains("conflict") => continue,
            Err(e) => return Err(e),
        }
    }
    Err("the document kept changing".into())
}

/// What is worth telling the person about their agents between two readings: one that starts to need them, one that
/// finished a turn they have not seen. Not the one they look at (`looking`), and nothing at the first reading.
pub fn news(before: &[Space], now: &[Space], looking: Option<&str>) -> Vec<crate::news::News> {
    if before.is_empty() {
        return Vec::new();
    }
    let was = |address: &str| before.iter().flat_map(|s| &s.agents).find(|a| a.address == address).map(|a| a.state);
    let mut out = Vec::new();
    for s in now {
        for a in &s.agents {
            if Some(a.address.as_str()) == looking || was(&a.address) == Some(a.state) {
                continue;
            }
            let (title, body) = match a.state {
                State::Waiting => (format!("{} needs you", a.title), format!("{} · {}", s.name, crate::ask::what(a.kind.as_deref(), &a.msg))),
                State::Error => (format!("{} stopped", a.title), format!("{} · {}", s.name, if a.msg.is_empty() { "an error" } else { &a.msg })),
                State::ToReview => (format!("{} finished", a.title), s.name.clone()),
                _ => continue,
            };
            out.push(crate::news::News { run: a.address.clone(), title, body });
        }
    }
    out
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

/// What an agent waiting for the person asks, for the bar over its terminal.
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    pub address: String,
    pub title: String,
    /// "wants to approve Bash: touch x"
    pub what: String,
    /// Its numbered choices, as its screen shows them; the first is the one Command-Return takes.
    pub choices: Vec<String>,
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
    /// While it waits: what kind of thing it asks (permission, question, auth) and its words.
    pub kind: Option<String>,
    pub msg: String,
    /// The folder it works in.
    pub cwd: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Space {
    pub id: String,
    pub name: String,
    pub folder: String,
    pub agents: Vec<Agent>,
    /// Its tabs as Herdr splits them: each tab's id and how its live terminals share the room.
    pub tabs: Vec<(String, crate::layout::Layout)>,
}

/// The tab that holds a terminal, in any space: its id and its layout.
pub fn tab_of<'a>(spaces: &'a [Space], address: &str) -> Option<&'a (String, crate::layout::Layout)> {
    spaces.iter().flat_map(|s| &s.tabs).find(|(_, l)| l.panes().iter().any(|p| p == address))
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
            let is_live = |a: &str| live.iter().any(|t| t["address"].as_str() == Some(a));
            let tabs = space["tabs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| crate::layout::read(&t["root"], &is_live).map(|l| (t["id"].as_str().unwrap_or_default().to_owned(), l)))
                .collect();
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
            Space { id, name, folder, agents, tabs }
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
    Agent { address, title, state, since_ms: t["status_ms"].as_u64().or_else(|| t["created_ms"].as_u64()).unwrap_or(0), shell, turn, kind: t["status"]["kind"].as_str().map(str::to_owned), msg: t["status"]["msg"].as_str().unwrap_or_default().to_owned(), cwd: t["cwd"].as_str().unwrap_or_default().to_owned() }
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
    use super::{State, Value, age, arrange, new_agent_folder, news, seen_now, with_tab};
    use serde_json::json;

    #[test]
    fn a_new_agent_starts_where_the_open_one_works() {
        let mut spaces = arrange(&doc(), &json!({"terminals": [term("t1", "s1", Some("done"), 1, 1)]}));
        let s = &mut spaces[0];
        s.folder = "/usr".into();
        s.agents[0].cwd = "/tmp".into();
        // Herdr's folder for a space can be another project's: the open agent's own folder comes first
        assert_eq!(new_agent_folder(s, "t1"), Some("/tmp".to_owned()));
        s.agents[0].cwd = "/no/such/folder".into();
        assert_eq!(new_agent_folder(s, "t1"), Some("/usr".to_owned()));
        s.folder = "/no/such/either".into();
        assert_eq!(new_agent_folder(s, "t1"), None);
    }

    fn term(address: &str, space: &str, state: Option<&str>, turn: u64, seen: u64) -> Value {
        json!({
            "address": address, "state": "live", "agent": "claude-code", "title": format!("title {address}"),
            "turn": turn, "status_ms": 1000,
            "status": state.map(|s| json!({"app": "claude-code", "state": s, "kind": "permission", "msg": "approve Bash: ls"})),
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
    fn a_new_agent_gets_a_tab_at_the_end_of_its_space_as_herdr_draws_one() {
        let mut value = doc()["value"].clone();
        // fields this app does not know stay as they were
        value["theme"] = json!("dark");
        value["spaces"][0]["pinned"] = json!(true);
        let out = with_tab(&value, "s1", "tbnew", "pnew", "t8").unwrap();
        assert_eq!(out["theme"], "dark");
        assert_eq!(out["spaces"][0]["pinned"], true);
        assert_eq!(out["spaces"][1], value["spaces"][1]);
        let tabs = out["spaces"][0]["tabs"].as_array().unwrap();
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0], value["spaces"][0]["tabs"][0]);
        assert_eq!(
            tabs[1],
            json!({"id": "tbnew", "name": "", "root": {"type": "row", "id": "tbnew", "children": [
                {"type": "block", "id": "pnew", "content": {"terminal": "t8"}}
            ]}})
        );
        // and the column then shows it in that space, last
        let list = json!({"terminals": [term("t1", "s1", None, 0, 0), term("t2", "s1", None, 0, 0), term("t8", "s1", None, 0, 0)]});
        let spaces = arrange(&json!({"value": out}), &list);
        assert_eq!(spaces[0].agents.last().unwrap().address, "t8");
        // a space that is gone takes no tab
        assert!(with_tab(&value, "s9", "tbnew", "pnew", "t8").is_none());
    }

    #[test]
    fn news_is_an_agent_that_starts_to_need_the_person_or_finishes() {
        let read = |terms: Vec<Value>| arrange(&doc(), &json!({"terminals": terms}));
        let before = read(vec![term("t1", "s1", Some("working"), 3, 3), term("t2", "s1", Some("working"), 5, 5), term("t3", "s1", Some("blocked"), 1, 1)]);
        let now = read(vec![term("t1", "s1", Some("blocked"), 3, 3), term("t2", "s1", Some("done"), 6, 5), term("t3", "s1", Some("blocked"), 1, 1)]);
        let told = news(&before, &now, None);
        // in the order the column shows them (the space's tabs: t2 first)
        assert_eq!(told.iter().map(|n| n.run.as_str()).collect::<Vec<_>>(), ["t2", "t1"]);
        assert_eq!(told[1].title, "title t1 needs you");
        assert_eq!(told[1].body, "jaynes/tenure · wants to approve Bash: ls");
        assert_eq!(told[0].title, "title t2 finished");
        // the agent the person looks at is not news; what stood at the first reading is not news either
        assert_eq!(news(&before, &now, Some("t1")).iter().map(|n| n.run.as_str()).collect::<Vec<_>>(), ["t2"]);
        assert!(news(&[], &now, None).is_empty());
        // an agent that comes waiting is news
        let more = read(vec![term("t1", "s1", Some("blocked"), 3, 3), term("t2", "s1", Some("done"), 6, 5), term("t3", "s1", Some("blocked"), 1, 1), term("t9", "s1", Some("blocked"), 0, 0)]);
        assert_eq!(news(&now, &more, None).iter().map(|n| n.run.as_str()).collect::<Vec<_>>(), ["t9"]);
    }

    #[test]
    fn an_agent_knows_the_folder_it_works_in() {
        let mut t = term("t1", "s1", Some("working"), 1, 1);
        t["cwd"] = json!("/p/jaynes/app");
        assert_eq!(arrange(&doc(), &json!({"terminals": [t]}))[0].agents[0].cwd, "/p/jaynes/app");
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
