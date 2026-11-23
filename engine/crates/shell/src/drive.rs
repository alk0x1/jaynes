//! The loop around the core: take an input, let the core decide, record what it decided in one transaction, carry
//! out its commands, and turn what happened into the next inputs. One process drives a run at a time; inputs from
//! people wait in the inbox until that process takes them.

use crate::agent::{self, Agent, Ended};
use crate::project::{Workspace, EXCHANGE};
use crate::protocol::{Body, Call, Given};
use crate::role::RoleProgram;
use crate::store::Store;
use jaynes_core::event::{Event, Read};
use jaynes_core::manifest::ProjectAccess;
use jaynes_core::question::{Answer, Question};
use jaynes_core::{AgentTurn, AskOutcome, AskTurn, AttemptOutcome, CheckResult, CheckVerdict, Command, Digest, Input, Run};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct Setup {
    pub run: String,
    pub dir: PathBuf,
    pub role: RoleProgram,
    /// The run's copy of its target, when it has one.
    pub copy: Option<Box<dyn Workspace>>,
    pub agent: Box<dyn Agent>,
    /// The person opened the project's checks to every local port: as they last chose for this run (a `PortsChosen`
    /// in the record), else as the run's setup says (`ports_open`).
    pub any_port: bool,
    /// The person let the project's tests start their own test servers on ports the system picks (`jaynes project
    /// test-servers`), chosen as `any_port` is.
    pub servers: bool,
    /// With `servers`, the ports the box holds for test servers that the person let the project's tests use (`jaynes
    /// project test-servers DIR PORT`), chosen as `any_port` is.
    pub freed_ports: Vec<u16>,
    /// The engine's home, and the person's project the run works on, when it has one.
    pub home: PathBuf,
    pub project: Option<PathBuf>,
}

impl Setup {
    pub fn load(store: &Store, home: &Path, run: &str) -> Result<Setup, String> {
        let s = store.setup(run)?;
        let dir = home.join("runs").join(run);
        let mut role = RoleProgram::new(&dir.join("role"), &dir.join("library"));
        role.pidfile = Some(dir.join("role.pid"));
        // its name for the runs that wait for a port its checks hold, and where its checks say what they wait for
        let request = store.state(run).ok().flatten().map(|r| r.request).unwrap_or_default();
        let who = crate::listing::short_name(s["project"].as_str().unwrap_or_default(), s["title"].as_str(), &request);
        role.waits = Some((crate::port_wait::folder(&dir), who));
        // a run started before targets names none: it works on a repository
        let target = match s.get("target") {
            Some(t) => serde_json::from_value(t.clone()).map_err(|e| format!("the run's target is not one this engine knows: {e}"))?,
            None => jaynes_core::manifest::Target::Repository,
        };
        let copy = match (s["project"].as_str(), s["branch"].as_str()) {
            (Some(p), Some(b)) => Some(crate::project::open(target, Path::new(p), &dir.join("project"), b)),
            _ => None,
        };
        let agent = agent_in(s["agent"].as_str().unwrap_or("claude"), Some(&role.library));
        let chosen = store.state(run)?.and_then(|r| r.ports);
        let any_port = chosen.as_ref().map(|p| p.any).unwrap_or(ports_open(&s).is_some());
        let servers = chosen.as_ref().map(|p| p.servers).unwrap_or(servers_open(&s).is_some());
        let freed_ports = chosen.map(|p| p.freed_ports).unwrap_or_else(|| freed_open(&s));
        let project = s["project"].as_str().map(PathBuf::from);
        Ok(Setup { run: run.to_string(), dir, role, copy, agent, any_port, servers, freed_ports, home: home.to_path_buf(), project })
    }
}

/// The variables of the environment a run keeps from its start, and every process that carries it on runs with: where
/// the person's programs and toolchains are (a compiler from mise, asdf or Homebrew is found by them), and nothing
/// else, so no key or token the environment holds goes into the record.
pub const RUN_ENV: &[&str] = &[
    "PATH", "GOROOT", "GOPATH", "GOBIN", "CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN", "JAVA_HOME", "NVM_DIR", "NVM_BIN",
    "VOLTA_HOME", "PNPM_HOME", "BUN_INSTALL", "PYENV_ROOT", "PYENV_VERSION", "RBENV_ROOT", "ASDF_DIR", "ASDF_DATA_DIR",
    "MISE_DATA_DIR", "MISE_CONFIG_DIR", "HOMEBREW_PREFIX", "DEVELOPER_DIR", "SDKROOT",
    // where the browser tools keep the browsers they downloaded, which a project's tests start
    "PLAYWRIGHT_BROWSERS_PATH", "PUPPETEER_CACHE_DIR",
];

/// This process's values of [`RUN_ENV`], for the setup of a run it starts.
pub fn started_env() -> serde_json::Map<String, serde_json::Value> {
    RUN_ENV.iter().filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), serde_json::Value::String(v)))).collect()
}

/// The environment the run's setup kept from its start, by variable of [`RUN_ENV`]; none for a run started before it
/// kept one.
pub fn run_env(setup: &serde_json::Value) -> Option<std::collections::BTreeMap<String, String>> {
    let kept = setup.get("env")?.as_object()?;
    Some(kept.iter().filter(|(k, _)| RUN_ENV.contains(&k.as_str())).filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect())
}

/// The file in the engine's home that keeps the environment of the person's terminal: what the last `jaynes` they ran
/// at a terminal, or the install, had of [`RUN_ENV`]. A run started without a terminal (from the page or the app,
/// which may be opened with a short PATH) starts with it.
pub const TERMINAL_ENV: &str = "terminal-env.json";

/// The mark the app gives the `jaynes start` it runs ("terminal"), for it to start with [`terminal_env`].
pub const START_ENV: &str = "JAYNES_START_ENV";

/// Keep this process's [`RUN_ENV`] as the person's terminal's, when it is not already what the file says. What
/// cannot be written is left as it was: no command fails for it.
pub fn keep_terminal_env(home: &Path) {
    let now = started_env();
    if now.is_empty() || terminal_env(home).is_some_and(|kept| env_is(&kept)) {
        return;
    }
    let _kept = std::fs::create_dir_all(home).and_then(|()| std::fs::write(home.join(TERMINAL_ENV), serde_json::Value::Object(now).to_string()));
}

/// The environment of the person's terminal, as [`keep_terminal_env`] kept it, by variable of [`RUN_ENV`].
pub fn terminal_env(home: &Path) -> Option<std::collections::BTreeMap<String, String>> {
    let text = std::fs::read_to_string(home.join(TERMINAL_ENV)).ok()?;
    let kept: serde_json::Value = serde_json::from_str(&text).ok()?;
    run_env(&serde_json::json!({ "env": kept }))
}

/// Whether this process's environment is the one `kept` says, for every variable of [`RUN_ENV`]: those it holds set
/// as it holds them, the others unset.
pub fn env_is(kept: &std::collections::BTreeMap<String, String>) -> bool {
    RUN_ENV.iter().all(|k| std::env::var_os(k).as_deref() == kept.get(*k).map(std::ffi::OsStr::new))
}

/// When the person let this run's project reach any local port, as the run's setup recorded it at its start: the
/// time they opened it, in seconds since 1970.
pub fn ports_open(setup: &serde_json::Value) -> Option<u64> {
    let p = setup.get("ports")?;
    if p.get("any").and_then(serde_json::Value::as_bool) == Some(true) { p.get("opened").and_then(serde_json::Value::as_u64).or(Some(0)) } else { None }
}

/// When the person let this run's project's tests start their own test servers, as the run's setup recorded it at
/// its start: the time they allowed it, in seconds since 1970.
pub fn servers_open(setup: &serde_json::Value) -> Option<u64> {
    let p = setup.get("ports")?;
    if p.get("servers").and_then(serde_json::Value::as_bool) == Some(true) { p.get("opened").and_then(serde_json::Value::as_u64).or(Some(0)) } else { None }
}

/// The ports the box holds for test servers that the person let this run's project's tests use, as the run's setup
/// recorded it at its start; none without the test servers.
pub fn freed_open(setup: &serde_json::Value) -> Vec<u16> {
    if servers_open(setup).is_none() {
        return Vec::new();
    }
    setup["ports"]["freed_ports"].as_array().into_iter().flatten().filter_map(|p| p.as_u64().and_then(|p| u16::try_from(p).ok())).collect()
}

/// Whether `spec` names an agent program Jaynes knows: `claude`, `claude:MODEL`, `codex:MODEL` or `command:CMD`. Anything else is
/// refused before a run starts, and never taken for Claude Code: a name mistyped would spend the person's quota.
pub fn known_agent(spec: &str) -> Result<(), String> {
    if spec == "claude" || spec.starts_with("claude:") || spec.strip_prefix("command:").is_some_and(|c| !c.trim().is_empty())
        || spec.strip_prefix("codex:").is_some_and(|m| !m.trim().is_empty())
    {
        return Ok(());
    }
    Err(format!("{spec} is not an agent Jaynes knows: the agent is claude, claude:MODEL, codex:MODEL or command:CMD"))
}

/// The agent a run's setup names: `command:CMD`, `codex:MODEL`, or `claude:MODEL` with the model fixed when the run
/// started.
pub fn agent_named(spec: &str) -> Box<dyn Agent> {
    agent_in(spec, None)
}

/// `agent_named`, with the role library whose box runs an agent program that has no guards of Jaynes's own.
pub fn agent_in(spec: &str, library: Option<&Path>) -> Box<dyn Agent> {
    if let Some(cmd) = spec.strip_prefix("command:") {
        return Box::new(agent::Shell { command: cmd.to_string() });
    }
    if let Some(model) = spec.strip_prefix("codex:") {
        return Box::new(agent::Codex::new(model, library));
    }
    let mut claude = agent::Claude { library: library.map(Path::to_path_buf), ..Default::default() };
    if let Some(model) = spec.strip_prefix("claude:").filter(|m| !m.is_empty()) {
        claude.model = model.to_string();
    }
    Box::new(claude)
}

pub fn merges(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("merges.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Start a run in a process of its own, which goes on by itself; what it says goes to `start-<run>.log` in the
/// engine's home. The run's id is returned before the run is recorded.
/// `title`: the short name the run is shown by, when one was given (`listing::run_title` says what it is otherwise).
///
/// `terminal`: the run starts with the environment of the person's terminal ([`terminal_env`]), as the page's do; an
/// agent's (through MCP) keeps its own. `starter` is the agent's process that asked for the run
/// (`starter::of_this_process`), None for the person's.
#[allow(clippy::too_many_arguments)]
pub fn launch(home: &Path, exe: &Path, role: &str, project: &str, request: &str, title: Option<&str>, high: bool, terminal: bool, starter: Option<&str>) -> Result<String, String> {
    let dir = crate::install::find_role(home, role)?;
    // a role that does not fit the project says why here, to the page or the agent that asked, before a run exists
    let folder = std::fs::canonicalize(project).map_err(|e| format!("{project}: {e}"))?;
    crate::toml_role::fits_or_says(&dir, &crate::install::library(home), &folder)?;
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?;
    let run = format!("r{}-{:03}", t.as_secs(), t.subsec_millis());
    let mut argv = vec!["start", "--run", &run, "--role", role, "--project", project, "--request", request];
    if high {
        argv.extend(["--risk", "high"]);
    }
    if let Some(t) = title.map(str::trim).filter(|t| !t.is_empty()) {
        argv.extend(["--title", t]);
    }
    argv.extend(["--starter", starter.unwrap_or("none")]);
    std::fs::create_dir_all(home).map_err(|e| e.to_string())?;
    let log = std::fs::File::create(home.join(format!("start-{run}.log"))).map_err(|e| e.to_string())?;
    let err = log.try_clone().map_err(|e| e.to_string())?;
    let mut start = std::process::Command::new(exe);
    start.args(&argv).env("JAYNES_ENGINE_HOME", home).env_remove(START_ENV);
    if let Some(kept) = terminal.then(|| terminal_env(home)).flatten() {
        for k in RUN_ENV {
            match kept.get(*k) {
                Some(v) => start.env(k, v),
                None => start.env_remove(k),
            };
        }
    }
    start.stdin(std::process::Stdio::null()).stdout(log).stderr(err).spawn().map_err(|e| format!("the run could not start: {e}"))?;
    Ok(run)
}

/// Revert every merge of a run and record it: the core is asked first, so nothing is reverted it would refuse.
pub fn undo(store: &mut Store, home: &Path, run: &str, note: String, by: jaynes_core::question::By) -> Result<String, String> {
    let state = store.state(run)?.ok_or("no such run")?;
    if state.engine != jaynes_core::ENGINE {
        let said = act_elsewhere(home, &state, &json!({"undo": {"note": note, "by": by}}))?;
        return Ok(said["recorded"].as_str().unwrap_or("").to_string());
    }
    let input = Input::Undo { note, by };
    jaynes_core::decide(Some(&state), &input).map_err(|r| r.0)?;
    let setup = Setup::load(store, home, run)?;
    let copy = setup.copy.as_ref().ok_or("the run has no project")?;
    let landed = merges(&setup.dir);
    let reference = if landed.is_empty() { "nothing had been merged".to_string() } else { copy.undo(&landed, run)? };
    store.decide(run, &input)?;
    Ok(reference)
}

/// Give a run an input from outside its process (the page, an agent): recorded now and carried on in a process
/// of its own when nothing drives the run, or left in its inbox for the process that does. A leave is recorded at
/// once in any case, since an agent inside a turn waits for it. Says which.
pub fn submit(home: &Path, exe: &Path, run: &Run, input: Input) -> Result<&'static str, String> {
    if run.engine != jaynes_core::ENGINE {
        let said = act_elsewhere(home, run, &json!({"submit": input}))?;
        return Ok(if said["recorded"] == "inbox" { "inbox" } else { "now" });
    }
    // a run whose process is gone says so in the record before the person resumes it
    let settled;
    let run = match &input {
        Input::Resume { .. } if !run.halted => match settle_left_running(home, &run.id)? {
            Some(now) => {
                settled = now;
                &settled
            }
            None => run,
        },
        _ => run,
    };
    // what the person chose for the project's ports, the run's own process reads from their file: nobody else says it
    if matches!(input, Input::PortsChosen { .. }) {
        return Err("only the run's own process records the project's ports, read from what the person chose".into());
    }
    jaynes_core::decide(Some(run), &input).map_err(|r| r.0)?;
    let mut store = Store::open(&home.join("record.sqlite3"))?;
    if let Input::Answer { answer } = &input
        && run.questions.get(&answer.question).is_some_and(|q| q.question.kind == jaynes_core::question::QuestionKind::Permission) {
            store.decide(&run.id, &input)?;
            return Ok("now");
        }
    let dir = home.join("runs").join(&run.id);
    let lock = File::create(dir.join("lock")).map_err(|e| e.to_string())?;
    if lock.try_lock().is_ok() {
        // a leave to change the tests starts nothing: recorded, for the checks the person's answer sets going (T1)
        if matches!(input, Input::LeaveTests { .. }) {
            store.decide(&run.id, &input)?;
            return Ok("now");
        }
        // An input that starts work is left to the process carried on here, which recovers first and then takes it:
        // decided here, the attempt it begins would be taken by that process for one Jaynes stopped. The rest is
        // recorded at once.
        if matches!(input, Input::Answer { .. } | Input::Grant { .. } | Input::Continue { .. } | Input::SendBack { .. } | Input::Resume { .. } | Input::AskAgent { .. }) {
            store.inbox_put(&run.id, &input)?;
        } else {
            store.decide(&run.id, &input)?;
        }
        drop(lock);
        let log = File::create(dir.join("continue.log")).map_err(|e| e.to_string())?;
        let err = log.try_clone().map_err(|e| e.to_string())?;
        std::process::Command::new(exe)
            .args(["continue", &run.id])
            .env("JAYNES_ENGINE_HOME", home)
            .stdin(std::process::Stdio::null())
            .stdout(log)
            .stderr(err)
            .spawn()
            .map_err(|e| format!("the run could not be carried on: {e}"))?;
        Ok("now")
    } else {
        store.inbox_put(&run.id, &input)?;
        stop_agent_for(&dir, &input);
        Ok("inbox")
    }
}

/// What the run's process finds when the agent it waits for was stopped by a halt: the turn it gets back is
/// then interrupted, not an attempt that failed.
const HALTED_AGENT: &str = "agent-halted";

/// What the run's process finds when a halt stopped a project's command in the box that a call to the role was
/// running (a check's suite): the call it gets back is then interrupted, and runs again when the person resumes.
const HALTED_BOX: &str = "box-halted";

/// Where a call to the role has each command it runs in the box note its process group while it runs
/// (`JAYNES_BOX_GROUPS`, a file named by the number): such a command runs in a session of its own, which stopping
/// the role's group never reaches.
pub const BOX_GROUPS: &str = "box-groups";

/// The rule a halt is recorded by when the process driving the run was stopped by a signal (a TERM or an INT from
/// outside Jaynes): the run is halted, as by the person, and only the person resumes it.
pub const JAYNES_STOPPED: &str = "jaynes-was-stopped";

/// What the person reads of a run whose process was stopped while it ran.
pub const JAYNES_STOPPED_SAID: &str = "Jaynes stopped while it ran";

/// The signal that stopped this process while it drove a run, 0 while none came.
static STOPPED: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
/// Set once the halt that signal asked for is in the run's inbox and its agent is stopped.
static STOPPED_SETTLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The run this process drives: where its record and its folder are.
static DRIVING: std::sync::Mutex<Option<(PathBuf, String, PathBuf)>> = std::sync::Mutex::new(None);

/// The signal that stopped this process while it drove a run, if one did: it then ends as that signal says.
pub fn stopped_by() -> Option<i32> {
    match STOPPED.load(std::sync::atomic::Ordering::SeqCst) {
        0 => None,
        sig => Some(sig),
    }
}

/// A process driving a run that is told to stop (a TERM or an INT, from outside Jaynes) does what a halt does: the halt
/// goes in the run's inbox, by Jaynes's own rule, and the agent and the box's commands are stopped, so no agent goes on
/// working, and spending, with nobody listening to it. The process then records the halt and ends; a second signal
/// ends it at once. A hangup is left as it was: a start under `nohup` keeps going when its terminal closes.
fn stop_on_signals(home: &Path, run: &str, dir: &Path) {
    use std::sync::atomic::Ordering;
    if let Ok(mut d) = DRIVING.lock() {
        *d = Some((home.to_path_buf(), run.to_string(), dir.to_path_buf()));
    }
    // the unit tests drive runs in the test process, which keeps the signals it has
    if cfg!(test) {
        return;
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        use signal_hook::consts::{SIGINT, SIGTERM};
        let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGTERM, SIGINT]) else { return };
        std::thread::spawn(move || {
            use std::io::Write;
            for sig in signals.forever() {
                if STOPPED.swap(sig, Ordering::SeqCst) != 0 {
                    std::process::exit(128 + sig);
                }
                let driving = DRIVING.lock().ok().and_then(|d| d.clone());
                let Some((home, run, dir)) = driving else { std::process::exit(128 + sig) };
                let name = signal_hook::low_level::signal_name(sig).unwrap_or("a signal");
                let _said = writeln!(std::io::stderr(), "{name} came from outside Jaynes: run {run} is halted and its agent stopped; resume it to go on");
                let halt = Input::Halt { by: jaynes_core::question::By::Rule { rule: JAYNES_STOPPED.to_string() } };
                match Store::open(&home.join("record.sqlite3")).and_then(|s| s.inbox_put(&run, &halt)) {
                    Ok(()) => stop_agent_for(&dir, &halt),
                    // nothing to record the halt in: the process cannot say it, and ends as the signal asks
                    Err(_) => std::process::exit(128 + sig),
                }
                STOPPED_SETTLED.store(true, Ordering::SeqCst);
            }
        });
    });
}

/// Where the process driving a run notes its pid, once it takes the run.
const DRIVER_PID: &str = "driver.pid";

/// A run that says it runs while the process that last drove it is gone (killed outright, or stopped before it
/// handled anything): nobody works on it, and whoever reads it says "Jaynes stopped while it ran". A run with no note
/// of its driver is not judged; a pid that lives, whatever process it is now, is taken as its driver.
pub fn left_running(home: &Path, r: &Run) -> bool {
    if r.finished() || r.halted || !matches!(r.phase, jaynes_core::run::Phase::Running) {
        return false;
    }
    let Some(pid) = std::fs::read_to_string(home.join("runs").join(&r.id).join(DRIVER_PID)).ok().and_then(|t| t.trim().parse::<u32>().ok()) else {
        return false;
    };
    !std::process::Command::new("kill").args(["-0", &pid.to_string()]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
}

/// Before the person resumes a run left running with nobody driving it, record what happened to it: Jaynes stopped
/// while it ran, a halt by Jaynes's own rule, so their resume goes on from there as from any halt. Only while no
/// process holds the run; what the gone process left running (its agent, the box's commands) is stopped now, so it
/// spends nothing more. The run as it stands after, when it was so left.
pub fn settle_left_running(home: &Path, run: &str) -> Result<Option<Run>, String> {
    let dir = home.join("runs").join(run);
    let Ok(lock) = File::create(dir.join("lock")) else { return Ok(None) };
    if lock.try_lock().is_err() {
        return Ok(None);
    }
    let mut store = Store::open(&home.join("record.sqlite3"))?;
    let Some(state) = store.state(run)? else { return Ok(None) };
    if !left_running(home, &state) {
        return Ok(None);
    }
    let _stopped = agent::stop_left(&dir.join("agent.pid"));
    let _stopped = stop_box_groups(&dir);
    let _stopped = agent::stop_left(&dir.join("role.pid"));
    let _halted = decide_and_record(&mut store, run, &Input::Halt { by: jaynes_core::question::By::Rule { rule: JAYNES_STOPPED.to_string() } })?;
    // what was under way is settled as at any halt: interrupted, spending no attempt, or held if it may have acted
    let (now, _, _) = decide_and_record(&mut store, run, &Input::Recovered)?;
    drop(lock);
    Ok(Some(now))
}

/// Once a signal stopped this process, wait (a few seconds at most) until its halt is in the inbox: true when one
/// came and has not been taken yet, so the drive takes it before it ends.
fn halt_to_take(taken: &mut bool) -> bool {
    if *taken || stopped_by().is_none() {
        return false;
    }
    for _ in 0..50 {
        if STOPPED_SETTLED.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    *taken = true;
    true
}

/// Stop the agent of a run left an abandon or a halt, once that input is in its inbox, so its process starts
/// no other turn: nothing the agent would do from here is wanted, and it costs tokens. A halt or an abandon stops
/// too the commands in the box that a call to the role is running.
pub fn stop_agent_for(dir: &Path, input: &Input) {
    match input {
        Input::Abandon { by, .. } if by.person() => {
            agent::stop_left(&dir.join("agent.pid"));
            stop_box_groups(dir);
        }
        // an agent's abandon sets the run aside, which the person may bring back: what it stops comes back
        // interrupted, as at a halt, never as an attempt that failed
        Input::Halt { .. } | Input::Abandon { .. } => {
            // the mark goes first: the process may get the turn back as soon as the agent is gone. A halt that stops
            // nothing takes away only its own mark, never one an earlier halt left for the agent it stopped
            let before = dir.join(HALTED_AGENT).exists();
            let _marked = std::fs::write(dir.join(HALTED_AGENT), "");
            if !agent::stop_left(&dir.join("agent.pid")) && !before {
                let _gone = std::fs::remove_file(dir.join(HALTED_AGENT));
            }
            // a mark an earlier halt left stays: that halt's stopped call may not have come back yet
            let had = dir.join(HALTED_BOX).exists();
            let _marked = std::fs::write(dir.join(HALTED_BOX), "");
            if !stop_box_groups(dir) && !had {
                let _gone = std::fs::remove_file(dir.join(HALTED_BOX));
            }
        }
        _ => {}
    }
}

/// Stop, gently then for good, each group a call to the role noted in the run's folder, but only one Jaynes
/// started: its leader is alive, leads its group, and descends from the role program the run noted in `role.pid`.
/// A note that is not such a group is left alone, whatever wrote it. True when something was stopped.
pub fn stop_box_groups(dir: &Path) -> bool {
    let Some(role) = std::fs::read_to_string(dir.join("role.pid")).ok().and_then(|t| t.trim().parse::<u32>().ok()) else { return false };
    let Ok(notes) = std::fs::read_dir(dir.join(BOX_GROUPS)) else { return false };
    let ps = |field: &str, pid: u32| -> Option<u32> {
        let out = std::process::Command::new("ps").args(["-o", &format!("{field}="), "-p", &pid.to_string()]).stderr(std::process::Stdio::null()).output().ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    };
    let descends = |pid: u32| {
        let mut at = pid;
        for _ in 0..16 {
            match ps("ppid", at) {
                Some(p) if p == role => return true,
                Some(p) if p > 1 => at = p,
                _ => return false,
            }
        }
        false
    };
    let mut stopped = false;
    for note in notes.flatten() {
        let Some(pgid) = note.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        if pgid <= 1 || ps("pgid", pgid) != Some(pgid) || !descends(pgid) {
            continue;
        }
        let group = format!("-{pgid}");
        let signal = |sig: &str| std::process::Command::new("kill").args([sig, "--", &group]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
        let _term = signal("-TERM");
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(100));
            if !signal("-0") {
                break;
            }
        }
        let _kill = signal("-KILL");
        let _gone = std::fs::remove_file(note.path());
        stopped = true;
    }
    stopped
}

// --- each run on its own engine ----------------------------------------------------------------------------------

/// Where the program of each engine that started a run is kept, by version.
fn engines(home: &Path) -> PathBuf {
    home.join("engines")
}

/// What is said of a run whose older engine is no longer on this machine, when something goes to it.
pub fn gone_said(run: &Run) -> String {
    format!("run {} started on engine {}, which is no longer on this machine: it cannot be carried on by this one ({})", run.id, run.engine, jaynes_core::ENGINE)
}

/// The program of an engine, if it is still kept on this machine.
pub fn engine_exe(home: &Path, engine: &str) -> Option<PathBuf> {
    Some(engines(home).join(engine).join("jaynes")).filter(|p| p.is_file())
}

/// Keep this engine's program, so the runs it starts can be carried on by it after Jaynes is updated.
pub fn keep_engine(home: &Path) -> Result<(), String> {
    let dir = engines(home).join(jaynes_core::ENGINE);
    if dir.join("jaynes").is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let tmp = dir.join(format!("jaynes.{}.tmp", std::process::id()));
    std::fs::copy(&exe, &tmp).map_err(|e| format!("Jaynes could not keep its program for this run: {e}"))?;
    std::fs::rename(&tmp, dir.join("jaynes")).map_err(|e| e.to_string())
}

/// Drop the programs of engines no unfinished run is on, except this one.
pub fn sweep_engines(home: &Path, store: &Store) -> Result<(), String> {
    let mut used = std::collections::BTreeSet::from([jaynes_core::ENGINE.to_string()]);
    for id in store.runs()? {
        if let Ok(Some(r)) = store.state(&id)
            && !r.finished()
        {
            used.insert(r.engine);
        }
    }
    for e in std::fs::read_dir(engines(home)).into_iter().flatten().flatten() {
        if !used.contains(&e.file_name().to_string_lossy().to_string()) {
            let _gone = std::fs::remove_dir_all(e.path());
        }
    }
    Ok(())
}

/// Hand an act on a run to the engine the run started on: `{"submit": input}`, `{"effects": {question, answer}}`
/// or `{"undo": {note, by}}`, answered as JSON.
fn act_elsewhere(home: &Path, run: &Run, act: &Value) -> Result<Value, String> {
    let exe = engine_exe(home, &run.engine).ok_or_else(|| gone_said(run))?;
    let mut child = std::process::Command::new(&exe)
        .args(["act", &run.id])
        .env("JAYNES_ENGINE_HOME", home)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("the engine of run {} did not start: {e}", run.id))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(act.to_string().as_bytes()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let said: Value = String::from_utf8_lossy(&out.stdout).lines().rev().find_map(|l| serde_json::from_str(l).ok()).unwrap_or(Value::Null);
    if said["ok"] == true {
        Ok(said)
    } else {
        Err(said["refused"].as_str().map(str::to_string).unwrap_or_else(|| String::from_utf8_lossy(&out.stderr).trim().to_string()))
    }
}

/// What an answer will do, as the engine the run started on writes it.
pub fn effects(home: &Path, run: &Run, question: &jaynes_core::question::Question, answer: &jaynes_core::question::Answer) -> Result<jaynes_core::event::Effects, String> {
    if run.engine == jaynes_core::ENGINE {
        let mut fx = jaynes_core::effects(run, question, answer);
        // the core says "your branch"; the copy knows its name
        if question.kind == jaynes_core::question::QuestionKind::Merge {
            let into = crate::project::start_branch_of(&home.join("runs").join(&run.id).join("project"));
            if !into.is_empty() {
                for s in &mut fx.sentences {
                    if let Some(rest) = s.strip_prefix("The delivery merges into your branch") {
                        *s = format!("The delivery merges into “{into}”{rest}");
                    }
                }
            }
        }
        return Ok(fx);
    }
    let said = act_elsewhere(home, run, &json!({"effects": {"question": question.id, "answer": answer}}))?;
    serde_json::from_value(said["effects"].clone()).map_err(|e| e.to_string())
}

/// Carry out an act handed over by another engine, on a run of this one.
pub fn act_here(home: &Path, exe: &Path, run_id: &str, act: &Value) -> Value {
    let attempt = || -> Result<Value, String> {
        let store = Store::open(&home.join("record.sqlite3"))?;
        let run = store.state(run_id)?.ok_or("no such run")?;
        drop(store);
        if let Some(input) = act.get("submit") {
            let input: Input = serde_json::from_value(input.clone()).map_err(|e| e.to_string())?;
            return Ok(json!({"ok": true, "recorded": submit(home, exe, &run, input)?}));
        }
        if let Some(fx) = act.get("effects") {
            let qid = fx["question"].as_str().unwrap_or("");
            let q = run.questions.get(qid).ok_or("no such question")?;
            let answer: jaynes_core::question::Answer = serde_json::from_value(fx["answer"].clone()).map_err(|e| e.to_string())?;
            return Ok(json!({"ok": true, "effects": jaynes_core::effects(&run, &q.question, &answer)}));
        }
        if let Some(u) = act.get("undo") {
            let by: jaynes_core::question::By = serde_json::from_value(u["by"].clone()).map_err(|e| e.to_string())?;
            let mut store = Store::open(&home.join("record.sqlite3"))?;
            return Ok(json!({"ok": true, "recorded": undo(&mut store, home, run_id, u["note"].as_str().unwrap_or("").to_string(), by)?}));
        }
        Err("no such act".into())
    };
    attempt().unwrap_or_else(|e| json!({"ok": false, "refused": e}))
}

// --- what a run changed, and clearing its copy ----------------------------------------------------------------

/// The git settings of every agent's turn: no configuration of the system's, and in place of the person's global one
/// [`AGENT_GITCONFIG`], which Jaynes writes in the turn's exchange (`GIT_CONFIG_GLOBAL`).
pub const AGENT_GIT_ENV: &[(&str, &str)] = &[("GIT_CONFIG_NOSYSTEM", "1")];

/// The global git configuration of every agent's turn: an identity of its own and nothing else. As the lowest of git's
/// settings, a repository that says its own identity (a project's tests that make one) commits with it.
pub const AGENT_GITCONFIG: &str = "[user]\n\tname = Jaynes agent\n\temail = agent@jaynes.invalid\n";

/// The person's own lists of what git ignores and how it reads files, for the agent's git to read as theirs does.
#[derive(Default)]
pub struct PersonGitLists {
    /// The files, where they are named and where they lead, for the agent's box to read.
    pub reads: Vec<PathBuf>,
    /// The lines of the agent's git configuration that name them, after [`AGENT_GITCONFIG`].
    pub config: String,
}

/// What git ignores and how it reads files, as the person set it: the files their global configuration names
/// (core.excludesFile, core.attributesFile) or else git's own places for them (`$XDG_CONFIG_HOME/git` or
/// ~/.config/git, ignore and attributes). Each is named in the agent's configuration, which takes the place of the
/// person's: a plain file that is there, and never one of credentials nor in a place secrets live; else /dev/null, so
/// the agent's git looks for none in the home its box hides ("warning: unable to access '~/.config/git/ignore':
/// Operation not permitted" at every git status of the real-user's agents, 23/11). Filters, diff and merge drivers the
/// attributes name run only with the configuration that defines them, which the agent's git never reads.
pub fn person_git_lists(home: &Path, var: impl Fn(&str) -> Option<String>) -> PersonGitLists {
    let own = var("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(".config")).join("git");
    let configured = |key: &str| -> Option<PathBuf> {
        let out = crate::project::git_command().env("HOME", home).env_remove("GIT_CONFIG_GLOBAL").env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["config", "--global", "--type=path", "--get", key]).output().ok().filter(|o| o.status.success())?;
        let named = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        Some(if named.is_absolute() { named } else { home.join(named) })
    };
    let secret = |p: &Path| p.components().any(|c| {
        let c = c.as_os_str().to_string_lossy().to_lowercase();
        c.contains("credential") || [".ssh", ".aws", ".gnupg", ".netrc", ".kube", ".docker"].contains(&c.as_str())
    });
    let mut lists = PersonGitLists { reads: Vec::new(), config: "[core]\n".into() };
    for (key, place) in [("core.excludesFile", "ignore"), ("core.attributesFile", "attributes")] {
        let file = configured(key).unwrap_or_else(|| own.join(place));
        let real = std::fs::canonicalize(&file).ok().filter(|r| r.is_file() && !secret(&file) && !secret(r));
        let named = match real {
            Some(real) => {
                for p in [file.clone(), real] {
                    if !lists.reads.contains(&p) {
                        lists.reads.push(p);
                    }
                }
                file
            }
            None => PathBuf::from("/dev/null"),
        };
        let quoted = named.display().to_string().replace('\\', "\\\\").replace('"', "\\\"");
        lists.config.push_str(&format!("\t{} = \"{quoted}\"\n", key.trim_start_matches("core.")));
    }
    lists
}

/// The most Jaynes reads of a file an agent left in its folder.
const MOST_READ: u64 = 1 << 20;

/// A file an agent left under `root`, read only when it is a plain file there, not a link nor anything reached through
/// one, and of at most a megabyte: Jaynes reads it outside any box, and a link could point at anything this account
/// may read. Err(None) when there is none.
fn read_inside(root: &Path, path: &Path) -> Result<String, Option<String>> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(None),
        Err(e) => return Err(Some(format!("does not read: {e}"))),
    };
    if !meta.is_file() {
        return Err(Some("is not a plain file: a link or a folder is not read".to_string()));
    }
    // a hard link shares a file of this account that may be anywhere, without the agent reading it
    if std::os::unix::fs::MetadataExt::nlink(&meta) > 1 {
        return Err(Some("is a hard link to another file, and is not read".to_string()));
    }
    let (Ok(real), Ok(base)) = (std::fs::canonicalize(path), std::fs::canonicalize(root)) else { return Err(Some("does not read".to_string())) };
    if !real.starts_with(&base) {
        return Err(Some("is reached through a link out of the step's folder, and is not read".to_string()));
    }
    if meta.len() > MOST_READ {
        return Err(Some(format!("is larger than {MOST_READ} bytes")));
    }
    std::fs::read_to_string(&real).map_err(|e| Some(format!("does not read: {e}")))
}

/// `path`, under `root`, when it is a plain file there reached through no link at all and is not a hard link: what Jaynes
/// may read of it, outside any box.
fn plain_inside(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() || std::os::unix::fs::MetadataExt::nlink(&meta) > 1 {
        return Err(format!("{} is a link or not a plain file, and is not read", path.display()));
    }
    let base = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let rel = path.strip_prefix(root).map_err(|_| format!("{} is not inside the step's folder", path.display()))?;
    let real = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    if real != base.join(rel) {
        return Err(format!("{} is reached through a link, and is not read", path.display()));
    }
    Ok(real)
}

/// Write `bytes` at `path` under `root`, a folder an agent wrote in: every folder on the way from `root` is a folder, never
/// a link (made where it is missing), and the file is made anew, never written through what was there. Jaynes writes
/// outside any box, so a link left in the way would carry the write anywhere this account may write.
fn write_inside(root: &Path, path: &Path, bytes: &[u8]) -> Result<(), String> {
    let rel = path.strip_prefix(root).map_err(|_| format!("{} is not inside {}", path.display(), root.display()))?;
    let mut at = root.to_path_buf();
    let parts: Vec<_> = rel.components().collect();
    let Some((last, folders)) = parts.split_last() else { return Err("no file to write".to_string()) };
    for c in folders {
        at.push(c);
        match std::fs::symlink_metadata(&at) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(format!("{} is a link or a file, not a folder: Jaynes does not write through it", at.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&at).map_err(|e| e.to_string())?,
            Err(e) => return Err(e.to_string()),
        }
    }
    at.push(last);
    if std::fs::symlink_metadata(&at).is_ok() {
        std::fs::remove_file(&at).map_err(|e| format!("Jaynes could not replace {}: {e}", at.display()))?;
    }
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&at).map_err(|e| format!("Jaynes could not write {}: {e}", at.display()))?;
    std::io::Write::write_all(&mut f, bytes).map_err(|e| e.to_string())
}

/// How long a question the person was told of may stay unanswered before they are reminded of it, and between two
/// reminders. A first value, to come from use.
pub const REMIND_EVERY: u64 = 24 * 3600;

/// A question the person was told of, still unanswered, whose reminder is due.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reminder {
    pub run: String,
    pub question: String,
    pub step: String,
    pub about: String,
    /// When the person was last told of it, by its notice or by a reminder.
    pub since: u64,
}

/// The reminders due at `now`: every question a run waits on the person for, that they were told of, and that nobody
/// answered since `REMIND_EVERY`. It decides nothing of when to show them: the app and the page do that, at a pause
/// of the person's, and say they did with `mark_reminded`.
pub fn reminders_due(home: &Path, store: &Store, now: u64) -> Result<Vec<Reminder>, String> {
    let mut told_at: std::collections::BTreeMap<(String, String), u64> = Default::default();
    for (at, run, e) in store.history()? {
        if let Event::Waiting { on, .. } = e {
            for q in on {
                told_at.entry((run.clone(), q)).or_insert(at);
            }
        }
    }
    let mut out = Vec::new();
    let runs: std::collections::BTreeSet<String> = told_at.keys().map(|(r, _)| r.clone()).collect();
    for id in runs {
        let Some(run) = store.state(&id)? else { continue };
        // a run whose engine left the Mac takes no answer: nothing to remind of
        if run.finished() || crate::stop::left_behind(home, &run, jaynes_core::ENGINE) {
            continue;
        }
        let reminded = reminded(home, &id);
        for q in run.questions.values() {
            let qid = &q.question.id;
            let theirs = !q.superseded && q.answer.as_ref().is_none_or(|a| !a.by.person());
            let Some(&told) = told_at.get(&(id.clone(), qid.clone())) else { continue };
            if !theirs || !run.told.contains(qid) {
                continue;
            }
            let since = reminded.get(qid).copied().unwrap_or(told).max(told);
            if now.saturating_sub(since) >= REMIND_EVERY {
                out.push(Reminder { run: id.clone(), question: qid.clone(), step: q.question.step.clone(), about: q.question.about.clone(), since });
            }
        }
    }
    Ok(out)
}

fn reminded(home: &Path, run: &str) -> std::collections::BTreeMap<String, u64> {
    std::fs::read_to_string(home.join("runs").join(run).join("reminded.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// The person was reminded of `question` of `run` at `now`: the next reminder is due `REMIND_EVERY` later.
pub fn mark_reminded(home: &Path, run: &str, question: &str, now: u64) -> Result<(), String> {
    let mut r = reminded(home, run);
    r.insert(question.to_string(), now);
    let dir = home.join("runs").join(run);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tmp = dir.join(".reminded.json");
    std::fs::write(&tmp, serde_json::to_string(&r).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dir.join("reminded.json")).map_err(|e| e.to_string())
}

/// What step `step` of a run changed in the project, as git shows it: from the run's copy while it is kept, or from
/// what was saved of it when the copy was cleared. None when the step changed nothing.
pub fn step_diff(home: &Path, run: &str, step: &str) -> Option<String> {
    let dir = home.join("runs").join(run);
    if let Ok(saved) = std::fs::read_to_string(dir.join("changes").join(format!("{step}.diff"))) {
        return Some(saved);
    }
    let read = |name: &str| -> std::collections::BTreeMap<String, String> {
        std::fs::read_to_string(dir.join(name)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    };
    let commit = read("project-steps.json").get(step)?.clone();
    let copy = dir.join("project");
    let git = crate::project::git_dir(&copy);
    let parent = read("project-parents.json")
        .get(step)
        .cloned()
        .unwrap_or_else(|| std::fs::read_to_string(git.join("jaynes-base")).unwrap_or_default().trim().to_string());
    crate::project::diff(&copy, &parent, &commit)
}

/// Everything a run's delivery brings, as one diff against where the run started (its branch as it was then): each
/// file the merge lands, as the last step of the path that changed it left it (Run::delivered_files). What "See the
/// changes" shows at the merge; [`step_diff`] is one step's. None when the run's copy is gone or nothing lands.
pub fn delivery_diff(home: &Path, run: &Run) -> Option<String> {
    let lands: Vec<(String, String)> = run.delivered_files().into_iter().map(|(c, step)| (c.what, step)).collect();
    delivery_diff_of(home, &run.id, &lands)
}

/// [`delivery_diff`] for the files `lands`, each with the step that last changed it.
pub fn delivery_diff_of(home: &Path, run: &str, lands: &[(String, String)]) -> Option<String> {
    let dir = home.join("runs").join(run);
    let copy = dir.join("project");
    let (_, base) = crate::project::start_of(&copy)?;
    let steps: std::collections::BTreeMap<String, String> =
        std::fs::read_to_string(dir.join("project-steps.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    // each file from the commit of the step that last changed it, in the order of the paths
    let mut lands: Vec<&(String, String)> = lands.iter().collect();
    lands.sort();
    let mut out = String::new();
    for (path, step) in lands {
        let commit = steps.get(step)?;
        out.push_str(crate::project::diff_paths(&copy, &base, commit, &[path.as_str()])?.trim_end_matches('\n'));
        out.push('\n');
    }
    out.lines().any(|l| l.starts_with("diff --git ")).then_some(out)
}

/// How long a finished run keeps its copy of the project: a week, or `$JAYNES_KEEP_COPIES_SECONDS`.
fn keep_copies() -> u64 {
    std::env::var("JAYNES_KEEP_COPIES_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(jaynes_core::attention::SETTLE)
}

/// Clear the copies of runs finished longer ago than they are kept, saving first what each step changed, so the
/// page still shows it. Only finished runs: a run under way, or waiting, keeps its copy.
pub fn sweep_copies(home: &Path, store: &Store) -> Result<(), String> {
    let now = crate::store::now();
    let times = store.times()?;
    for id in store.runs()? {
        let copy = home.join("runs").join(&id).join("project");
        if !copy.exists() {
            continue;
        }
        let Ok(Some(run)) = store.state(&id) else { continue };
        if run.finished() {
            clear_caches(&home.join("runs").join(&id));
        }
        let last = times.get(&id).map(|t| t.1).unwrap_or(now);
        if !run.finished() || last.saturating_add(keep_copies()) > now {
            continue;
        }
        let saved = home.join("runs").join(&id).join("changes");
        std::fs::create_dir_all(&saved).map_err(|e| e.to_string())?;
        for d in &run.manifest.steps {
            if let Some(text) = step_diff(home, &id, &d.name) {
                std::fs::write(saved.join(format!("{}.diff", d.name)), text).map_err(|e| e.to_string())?;
            }
        }
        let git = crate::project::git_dir(&copy);
        remove_all(&copy).map_err(|e| format!("Jaynes could not clear the copy of run {id}: {e}"))?;
        if git.exists() {
            remove_all(&git).map_err(|e| format!("Jaynes could not clear the copy of run {id}: {e}"))?;
        }
    }
    Ok(())
}

/// Clear the toolchains' caches a run's turns filled (Go's builds and modules, npm's packages) in each folder an
/// agent worked in: a finished run starts no turn again, and they are hundreds of megabytes a run.
fn clear_caches(dir: &Path) {
    let mut places = vec![dir.join("project")];
    if let Ok(steps) = std::fs::read_dir(dir.join("steps")) {
        for step in steps.flatten() {
            places.extend(std::fs::read_dir(step.path()).into_iter().flatten().flatten().map(|a| a.path()));
        }
    }
    for p in places {
        let tmp = p.join(EXCHANGE).join("tmp");
        if tmp.is_dir() {
            let _gone = remove_all(&tmp);
        }
    }
}

/// Remove a folder and all it holds, the folders Go makes read-only in its module cache included.
fn remove_all(dir: &Path) -> std::io::Result<()> {
    if std::fs::remove_dir_all(dir).is_ok() {
        return Ok(());
    }
    use std::os::unix::fs::PermissionsExt;
    fn writable(p: &Path) {
        let Ok(meta) = std::fs::symlink_metadata(p) else { return };
        if !meta.is_dir() {
            return;
        }
        let _set = std::fs::set_permissions(p, std::fs::Permissions::from_mode(meta.permissions().mode() | 0o700));
        for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
            writable(&e.path());
        }
    }
    writable(dir);
    std::fs::remove_dir_all(dir)
}

/// Where an agent writes why a step cannot be done as asked, in its exchange folder.
const IMPOSSIBLE: &str = "impossible.txt";
/// Where the agent writes its words to the person: beside the work a stop shows, never inside it, and a question's
/// answer.
const REPLY: &str = "reply.md";
/// What a question is about, for an agent that answers it in a session of its own.
const ASKED_ABOUT: &str = "asked-about.json";
/// The most characters of a reply the person is shown.
pub const REPLY_MOST: usize = 4000;

/// A reply as words for the person: line breaks kept, at most one blank line between paragraphs, tabs as spaces,
/// control characters and the invisible marks that turn or hide text taken out, cut at REPLY_MOST with "…". None when
/// nothing is left.
pub fn reply_text(raw: &str) -> Option<String> {
    let invisible = |c: char| matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}');
    let mut out = String::new();
    let mut blank = false;
    for line in raw.replace("\r\n", "\n").split(['\n', '\r']) {
        let line: String = line.chars().map(|c| if c == '\t' { ' ' } else { c }).filter(|&c| !c.is_control() && !invisible(c)).collect();
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank = true;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank { "\n\n" } else { "\n" });
        }
        blank = false;
        out.push_str(line);
    }
    let out = out.trim();
    if out.is_empty() {
        return None;
    }
    if out.chars().count() <= REPLY_MOST {
        return Some(out.to_string());
    }
    let mut cut: String = out.chars().take(REPLY_MOST - 1).collect::<String>().trim_end().to_string();
    cut.push('…');
    Some(cut)
}

/// What the agent wrote to the person in this turn's exchange, as words for them.
fn reply_in(exchange: &Path) -> Option<String> {
    read_inside(exchange.parent().unwrap_or(exchange), &exchange.join(REPLY)).ok().and_then(|t| reply_text(&t))
}

/// What a person sees as the process goes: one line each.
pub fn say(line: &str) {
    eprintln!("{line}");
}

/// Decide on the run's latest state and record the events, in one transaction of the record.
pub fn decide_and_record(store: &mut Store, run: &str, input: &Input) -> Result<(Run, Vec<Event>, Vec<Command>), String> {
    store.decide(run, input)
}

pub struct Driver<'a> {
    pub store: &'a mut Store,
    pub setup: Setup,
    _lock: File,
}

impl<'a> Driver<'a> {
    /// Take the run, or say that another process holds it.
    pub fn take(store: &'a mut Store, setup: Setup) -> Result<Driver<'a>, String> {
        std::fs::create_dir_all(&setup.dir).map_err(|e| e.to_string())?;
        let lock = File::create(setup.dir.join("lock")).map_err(|e| e.to_string())?;
        lock.try_lock().map_err(|_| "another process is driving this run".to_string())?;
        stop_on_signals(&setup.home, &setup.run, &setup.dir);
        std::fs::write(setup.dir.join(DRIVER_PID), std::process::id().to_string()).map_err(|e| format!("Jaynes could not note the process driving the run: {e}"))?;
        // macOS tells no child that its parent died: an agent the last process started may still be writing.
        if agent::stop_left(&setup.dir.join("agent.pid")) {
            say("stopped an agent program that a process which died had left running");
        }
        // the box's commands first, while they still descend from the role program that ran them
        if stop_box_groups(&setup.dir) {
            say("stopped a project's command in the box that a call to the role had left running");
        }
        if agent::stop_left(&setup.dir.join("role.pid")) {
            say("stopped a call to the role that a process which died had left running");
        }
        if let Some((seq, _)) = store.verify()? {
            return Err(format!("the record was changed at entry {seq}: Jaynes drives no run over a record that does not hold"));
        }
        if let Some(run) = store.state(&setup.run)?
            && run.engine != jaynes_core::ENGINE {
                return Err(format!(
                    "run {} started on engine {} and this is {}: a run stays on the engine it started with",
                    run.id, run.engine, jaynes_core::ENGINE
                ));
            }
        Ok(Driver { store, setup, _lock: lock })
    }

    /// Drive the run from `first` until nothing more can happen without someone outside.
    pub fn drive(&mut self, first: Input) -> Result<Run, String> {
        // A recovery settles what was under way when Jaynes stopped before anything from outside acts on the run.
        let mut recovering = matches!(first, Input::Recovered);
        let mut queue: VecDeque<Input> = VecDeque::from([first]);
        let mut halt_taken = false;
        let mut last = self.store.state(&self.setup.run)?;
        if let Some(run) = &last {
            self.catch_up_ledger(run)?;
        }
        loop {
            // What came from outside (an answer, a halt, an abandon) goes before what the last turn left to do.
            let mut came = Vec::new();
            while !recovering
                && let Some(input) = self.store.inbox_take(&self.setup.run)?
            {
                came.push(input);
            }
            recovering = false;
            for input in came.into_iter().rev() {
                queue.push_front(input);
            }
            let Some(mut input) = queue.pop_front() else {
                // a signal that stopped the process left its halt for the drive to record before it ends
                if halt_to_take(&mut halt_taken) {
                    continue;
                }
                break;
            };
            // the person continues as the project's ports stand now: what they chose since the start goes on record first,
            // before a Continue, a redo or any other answer, a correction, a resume (T9, the real user's of 23/11)
            if matches!(input, Input::Continue { .. } | Input::SendBack { .. } | Input::Answer { .. } | Input::Correct { .. } | Input::Resume { .. }) {
                self.ports_now()?;
            }
            // A turn whose agent a halt stopped is interrupted, so it spends no attempt and runs again on resume.
            if matches!(input, Input::AttemptEnded { .. }) && std::fs::remove_file(self.setup.dir.join(HALTED_AGENT)).is_ok() {
                input = Input::Recovered;
            }
            // So is a call to the role whose command in the box a halt stopped: its checks are not failed, and the mark
            // stays until the person resumes, when what was under way runs again.
            if matches!(input, Input::ChecksEnded { .. } | Input::AttemptEnded { .. }) && self.setup.dir.join(HALTED_BOX).exists() {
                input = Input::Recovered;
            }
            let resuming = matches!(input, Input::Resume { .. });
            let (state, events, commands) = match decide_and_record(self.store, &self.setup.run, &input) {
                Ok(x) if resuming && std::fs::remove_file(self.setup.dir.join(HALTED_BOX)).is_ok() => {
                    // only what a halt interrupted runs again: a delivery under way is the resume's own to send again
                    let interrupted = x.0.steps.iter().any(|s| {
                        matches!(s.status, jaynes_core::run::Status::Checking { .. } | jaynes_core::run::Status::Running { .. })
                    });
                    if interrupted {
                        queue.push_front(Input::Recovered);
                    }
                    x
                }
                Ok(x) => x,
                Err(why) => {
                    if !matches!(input, Input::Tick) {
                        say(&format!("refused: {why}"));
                    }
                    continue;
                }
            };
            // For the tests of recovery: die where a kill can fall, once the record holds a step done.
            if let Ok(step) = std::env::var("JAYNES_DIE_AFTER_DONE")
                && events.iter().any(|e| matches!(e, Event::StepDone { step: s, .. } if *s == step))
            {
                std::process::exit(9);
            }
            // and once the record holds a defect taken out, before anything is said of it beside the run
            if std::env::var_os("JAYNES_DIE_AFTER_UNPLANT").is_some() && events.iter().any(|e| matches!(e, Event::DefectRemoved { .. })) {
                std::process::exit(9);
            }
            self.follow_project(&state, &events)?;
            // what the person granted at a stop, recorded first: the project's tests start their own test servers
            // from here, as `jaynes project test-servers` lets them, and the run records it as that does
            if events.iter().any(|e| matches!(e, Event::Granted { offer: jaynes_core::manifest::Offer::TestServers, .. })) {
                self.let_test_servers()?;
            }
            if events.iter().any(|e| matches!(e, Event::RunFinished { .. })) {
                clear_caches(&self.setup.dir);
            }
            for e in &events {
                narrate(e, &state);
            }
            for e in &events {
                if let Event::AttemptStarted { step, fix: 0, .. } = e {
                    let agent = state.manifest.index(step).map(|i| state.manifest.steps[i].executor) == Some(jaynes_core::manifest::Executor::Agent);
                    if agent {
                        say(&format!("  {step} by {}", self.setup.agent.name().chars().take(60).collect::<String>()));
                    }
                }
            }
            let mut state = state;
            for c in commands {
                // What came from outside goes first before an agent's or a role's turn starts: a run abandoned or
                // halted meanwhile starts no turn, and the turn it had begun counts as interrupted.
                if matches!(c, Command::RunAgent(_) | Command::RunRole { .. } | Command::RunChecks { .. }) {
                    // every turn runs as the project's ports stand now: what the person chose while the run waited
                    // counts from its next turn, a redo's or a correction's as a Continue's (T9, the real user's of
                    // 23/11), and goes on record first
                    self.ports_now()?;
                    if let Some(now) = self.store.state(&self.setup.run)? {
                        state = now;
                    }
                    let mut came = Vec::new();
                    while let Some(input) = self.store.inbox_take(&self.setup.run)? {
                        came.push(input);
                    }
                    for input in came {
                        match decide_and_record(self.store, &self.setup.run, &input) {
                            Ok((now, events, more)) => {
                                for e in &events {
                                    narrate(e, &now);
                                }
                                if events.iter().any(|e| matches!(e, Event::Granted { offer: jaynes_core::manifest::Offer::TestServers, .. })) {
                                    self.let_test_servers()?;
                                }
                                state = now;
                                queue.extend(more.into_iter().filter_map(|m| self.carry_out_quietly(&state, m)));
                            }
                            Err(why) => say(&format!("refused: {why}")),
                        }
                    }
                    if state.finished() {
                        continue;
                    }
                    if state.halted {
                        if let Ok((now, events, _)) = decide_and_record(self.store, &self.setup.run, &Input::Recovered) {
                            for e in &events {
                                narrate(e, &now);
                            }
                            state = now;
                        }
                        continue;
                    }
                }
                if let Some(next) = self.carry_out(&state, c) {
                    queue.push_back(next);
                }
            }
            last = Some(state);
        }
        last.ok_or_else(|| "the run did not start".to_string())
    }

    /// Keep the project copy in step with the record: an output that passed becomes a commit; an attempt that
    /// failed leaves nothing behind.
    fn ledger(&self) -> std::collections::BTreeMap<String, String> {
        std::fs::read_to_string(self.setup.dir.join("project-steps.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn keep_ledger(&self, l: &std::collections::BTreeMap<String, String>) -> Result<(), String> {
        std::fs::write(self.setup.dir.join("project-steps.json"), serde_json::to_string(l).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }

    /// Each step's change, by step, for the steps in `names` that changed something, in the order given.
    fn changes(&self, names: &[String]) -> Vec<(String, String)> {
        let l = self.ledger();
        names.iter().filter_map(|n| l.get(n).map(|c| (n.clone(), c.clone()))).collect()
    }

    /// The change of `step` as the record holds it: `change`, or, when it is one with an evaluation's defect in it,
    /// the change as it was before.
    fn unplanted(&self, step: &str, change: &str) -> String {
        match crate::planted::active(&self.setup.dir, step) {
            Some(d) if d.planted == change => d.original,
            _ => change.to_string(),
        }
    }

    /// [`Driver::changes`], each as the record holds it: no step works from a planted defect.
    fn unplanted_changes(&self, names: &[String]) -> Vec<(String, String)> {
        self.changes(names).into_iter().map(|(n, c)| { let c = self.unplanted(&n, &c); (n, c) }).collect()
    }

    /// The steps whose changes the delivery takes, in order.
    fn writers(run: &Run) -> Vec<String> {
        run.path()
            .into_iter()
            .filter(|&j| run.manifest.steps[j].reach.project == ProjectAccess::Write)
            .map(|j| run.manifest.steps[j].name.clone())
            .collect()
    }

    /// Plant an evaluation's `patch` into the change of `step` while its stop waits: the step's change becomes one
    /// with the defect in it, shown at its stop as the step's, and the copy is composed with it. The core says
    /// first whether it may, so nothing of the copy changes on a refusal; the record then holds that a defect is
    /// in, and the core holds the delivery while it is. Who planted it, the note and the patch are kept beside the
    /// run, never in the record. The id of the defect.
    pub fn plant(&mut self, step: &str, patch: &Path, note: &str, planter: &str, by: jaynes_core::question::By) -> Result<String, String> {
        let run = self.store.state(&self.setup.run)?.ok_or("no such run")?;
        let id = crate::planted::next_id(&self.setup.dir);
        let input = Input::Plant { id: id.clone(), step: step.to_string(), by };
        jaynes_core::decide(Some(&run), &input).map_err(|r| format!("refused: {r}"))?;
        let copy = self.setup.copy.as_ref().ok_or("the run has no copy of a project to plant a defect into")?;
        let original = self.ledger().get(step).cloned().ok_or_else(|| format!("{step} changed nothing in the project: a defect is planted into a change"))?;
        let text = std::fs::read_to_string(patch).map_err(|e| format!("{}: {e}", patch.display()))?;
        let round = run.manifest.index(step).map(|i| run.steps[i].round).unwrap_or(0);
        let (planted, files) = copy.plant(&original, patch, &format!("{step}, round {round}"))?;
        let mut defects = crate::planted::read(&self.setup.dir);
        defects.push(crate::planted::Defect {
            id: id.clone(), step: step.to_string(), planter: planter.to_string(), note: note.to_string(), patch: text, files,
            at: crate::store::now(), original: original.clone(), planted: planted.clone(), removed: None,
        });
        crate::planted::write(&self.setup.dir, &defects)?;
        let mut l = self.ledger();
        l.insert(step.to_string(), planted);
        self.keep_ledger(&l)?;
        // the copy as the stop shows it: the changes of the run over the base, the defect in its step's
        copy.compose(&self.changes(&Self::writers(&run)))?;
        self.drive(input)?;
        Ok(id)
    }

    /// Take the planted defect `id` out: its step's change is again exactly the one it was, the copy composed from
    /// it, and the delivery goes on when no other defect is in. Verified when the step's change is the tree its
    /// checks judged, recorded with its output.
    pub fn unplant(&mut self, id: &str, remover: &str, by: jaynes_core::question::By) -> Result<Run, String> {
        let run = self.store.state(&self.setup.run)?.ok_or("no such run")?;
        let input = Input::Unplant { id: id.to_string(), by };
        // taken out in the record by a process that died before it said so beside the run: say so now, and go on
        let finishing = run.unplanted.contains(id) && crate::planted::read(&self.setup.dir).iter().any(|d| d.id == id && d.removed.is_none());
        if !finishing {
            jaynes_core::decide(Some(&run), &input).map_err(|r| format!("refused: {r}"))?;
        }
        let mut defects = crate::planted::read(&self.setup.dir);
        let d = defects.iter_mut().find(|d| d.id == id && d.removed.is_none()).ok_or_else(|| format!("there is no planted defect {id} in this run"))?;
        let mut l = self.ledger();
        // a step sent back since has a change of its own, from the change as it was: the defect is in no change now
        if l.get(&d.step) == Some(&d.planted) {
            l.insert(d.step.clone(), d.original.clone());
            self.keep_ledger(&l)?;
        }
        let verified = match (&self.setup.copy, run.manifest.index(&d.step).and_then(|i| run.steps[i].done.as_ref())) {
            (Some(copy), Some(done)) => {
                let judged = self.store.get(&done.output)?["tree"].as_str().map(str::to_string);
                l.get(&d.step).map(|c| copy.state_of(c)).transpose()? == judged
            }
            _ => false,
        };
        if let Some(copy) = &self.setup.copy {
            copy.compose(&self.changes(&Self::writers(&run)))?;
        }
        // the record first: a process that dies after it leaves the defect out for the core, and a second unplant
        // says so beside the run; one that dies before leaves it in for both, and a second unplant takes it out
        let end = self.drive(if finishing { Input::Recovered } else { input });
        d.removed = Some(crate::planted::Removed { by: remover.to_string(), at: crate::store::now(), verified });
        crate::planted::write(&self.setup.dir, &defects)?;
        end
    }

    /// The project's tests start their own test servers, as the person granted at a stop: the project's choice, as
    /// `jaynes project test-servers DIR` makes it, never the ports of the usual databases and caches, which only that
    /// command lets, naming them; then the run records it. Only macOS's box keeps tests from serving.
    fn let_test_servers(&mut self) -> Result<(), String> {
        let Some(project) = self.setup.project.clone() else { return Ok(()) };
        if !cfg!(target_os = "macos") {
            return Ok(());
        }
        let mut prefs = crate::project_prefs::load(&self.setup.home, &project)?;
        let was = prefs.ports.clone().unwrap_or_default();
        if !was.servers {
            let opened = if was.any { was.opened } else { crate::store::now() };
            prefs.ports = Some(crate::project_prefs::Ports { any: was.any, servers: true, opened, freed_ports: Vec::new(), freed_at: Default::default() });
            crate::project_prefs::save(&self.setup.home, &project, &prefs)?;
        }
        self.ports_now()
    }

    /// Record what the person chose for the project's ports since the run last said, and take it from here: the setup
    /// stays as it was.
    fn ports_now(&mut self) -> Result<(), String> {
        let Some(project) = &self.setup.project else { return Ok(()) };
        let chosen = crate::project_prefs::load(&self.setup.home, project)?.ports.filter(|p| p.any || p.servers);
        let (any, servers) = chosen.as_ref().map(|p| (p.any, p.servers)).unwrap_or_default();
        let freed_ports = chosen.as_ref().filter(|p| p.servers).map(|p| p.freed_ports.clone()).unwrap_or_default();
        if (any, servers, &freed_ports) == (self.setup.any_port, self.setup.servers, &self.setup.freed_ports) {
            return Ok(());
        }
        let input = Input::PortsChosen { any, servers, opened: chosen.map(|p| p.opened).unwrap_or(0), freed_ports: freed_ports.clone() };
        // a run that cannot take it (one finished) answers the continue itself
        let Some(run) = self.store.state(&self.setup.run)? else { return Ok(()) };
        if jaynes_core::decide(Some(&run), &input).is_err() {
            return Ok(());
        }
        let (now, events, _) = decide_and_record(self.store, &self.setup.run, &input)?;
        for e in &events {
            narrate(e, &now);
        }
        self.setup.any_port = any;
        self.setup.servers = servers;
        self.setup.freed_ports = freed_ports;
        Ok(())
    }

    /// Put the copy where step `i` works: the base with the changes of the earlier steps it uses, and, when it
    /// runs again, its own previous change on top, unless the person asked it to start over; after an attempt of this
    /// round that failed, what that attempt left, uncommitted, as it left it. Whether that attempt's files are there.
    fn prepare(&self, run: &Run, i: usize) -> Result<bool, String> {
        let Some(copy) = &self.setup.copy else { return Ok(false) };
        if run.manifest.steps[i].reach.project == ProjectAccess::None {
            return Ok(false);
        }
        let writers: Vec<String> = run
            .sources(i)
            .into_iter()
            .filter(|&j| run.manifest.steps[j].reach.project == ProjectAccess::Write)
            .map(|j| run.manifest.steps[j].name.clone())
            .collect();
        let parent = copy.compose(&self.unplanted_changes(&writers))?;
        let name = run.manifest.steps[i].name.clone();
        let mut parents = self.parents();
        parents.insert(name.clone(), parent);
        std::fs::write(self.setup.dir.join("project-parents.json"), serde_json::to_string(&parents).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let s = &run.steps[i];
        if run.manifest.steps[i].reach.project == ProjectAccess::Write && s.round > 0 && !s.afresh {
            let own = self.unplanted_changes(std::slice::from_ref(&name));
            let mut all = self.unplanted_changes(&writers);
            all.extend(own);
            copy.compose(&all).map(|_| ())?;
        }
        let tried = self.tried().get(&name).filter(|t| s.tried && t.0 == s.round).map(|t| t.1.clone());
        match tried {
            Some(t) if copy.lay_tried(&t)? => Ok(true),
            Some(_) => {
                say(&format!("{name}: the files of its last attempt were kept over another state of the copy: it starts from the copy as it is"));
                Ok(false)
            }
            None => Ok(false),
        }
    }

    /// What the failed attempt of each step left, by step: its round, and the change kept over the state it started from.
    fn tried(&self) -> std::collections::BTreeMap<String, (u32, String)> {
        std::fs::read_to_string(self.setup.dir.join("project-tried.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn keep_tried(&self, step: &str, kept: Option<(u32, String)>) -> Result<(), String> {
        let mut t = self.tried();
        match kept {
            Some(k) => t.insert(step.to_string(), k),
            None => t.remove(step),
        };
        std::fs::write(self.setup.dir.join("project-tried.json"), serde_json::to_string(&t).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }

    /// The merges the run made, oldest first.
    pub fn merges(&self) -> Vec<String> {
        merges(&self.setup.dir)
    }

    fn parents(&self) -> std::collections::BTreeMap<String, String> {
        std::fs::read_to_string(self.setup.dir.join("project-parents.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn follow_project(&self, run: &Run, events: &[Event]) -> Result<(), String> {
        let Some(copy) = &self.setup.copy else { return Ok(()) };
        for e in events {
            let (step, done) = match e {
                Event::StepDone { step, output, round, .. } => (step, Some((output, *round))),
                Event::AttemptFailed { step, .. } | Event::AttemptInterrupted { step, .. } | Event::AttemptImpossible { step, .. } => (step, None),
                _ => continue,
            };
            let Some(i) = run.manifest.index(step) else { continue };
            if run.manifest.steps[i].reach.project != ProjectAccess::Write {
                continue;
            }
            match done {
                Some((output, round)) => {
                    self.keep_tried(step, None)?;
                    self.commit_done(step, output, round)?
                }
                None => {
                    // an agent's next attempt starts from what this one left (the tech-lead, 22/11): kept beside the copy
                    let decl = &run.manifest.steps[i];
                    if decl.executor == jaynes_core::manifest::Executor::Agent {
                        let s = &run.steps[i];
                        let kept = copy.keep_tried(&decl.reach.write, &format!("{step}, round {}, attempt {}, failed", s.round, s.attempt))?;
                        self.keep_tried(step, kept.map(|c| (s.round, c)))?;
                    }
                    copy.discard()?
                }
            }
        }
        Ok(())
    }

    /// Make the commit of a step's output that passed: the tree Jaynes staged and checked, recorded with the output,
    /// over the copy as the steps before it left it. Its change is then in the ledger the delivery is made from.
    fn commit_done(&self, step: &str, output: &Digest, round: u32) -> Result<(), String> {
        let Some(copy) = &self.setup.copy else { return Ok(()) };
        let parent = self.parents().get(step).cloned().unwrap_or(copy.base()?);
        let message = format!("{step}, round {round}");
        let commit = copy.keep(self.store.get(output)?["tree"].as_str(), &parent, &message)?;
        let mut l = self.ledger();
        match commit {
            Some(c) => l.insert(step.to_string(), c),
            None => l.remove(step),
        };
        self.keep_ledger(&l)
    }

    /// A process that died between recording a step done and committing its change left the ledger behind the
    /// record: commit each such step from the tree recorded with its output, before anything touches the copy.
    fn catch_up_ledger(&self, run: &Run) -> Result<(), String> {
        let Some(copy) = &self.setup.copy else { return Ok(()) };
        let ledger = self.ledger();
        for (i, s) in run.steps.iter().enumerate() {
            let decl = &run.manifest.steps[i];
            let Some(done) = s.done.as_ref().filter(|_| s.status == jaynes_core::run::Status::Done && decl.reach.project == ProjectAccess::Write) else { continue };
            let Some(tree) = self.store.get(&done.output)?["tree"].as_str().map(str::to_string) else { continue };
            // a change with an evaluation's defect in it is not the record's: the change as it was is
            let kept = ledger.get(&decl.name).map(|c| self.unplanted(&decl.name, c));
            let have = kept.as_ref().map(|c| copy.state_of(c)).transpose()?;
            let parent_tree = self.parents().get(&decl.name).map(|p| copy.state_of(p)).transpose()?;
            let current = match &have {
                Some(t) => *t == tree,
                None => parent_tree.as_deref() == Some(tree.as_str()),
            };
            if !current {
                say(&format!("{} was done in the record and its change was not yet kept: kept now", decl.name));
                self.commit_done(&decl.name, &done.output, done.round)?;
            }
        }
        Ok(())
    }

    /// Commands that came with an input from outside, other than a turn: shown, asked, delivered. A turn waits for
    /// the loop.
    fn carry_out_quietly(&mut self, run: &Run, c: Command) -> Option<Input> {
        match c {
            Command::RunAgent(_) | Command::RunRole { .. } => None,
            other => self.carry_out(run, other),
        }
    }

    fn carry_out(&mut self, run: &Run, c: Command) -> Option<Input> {
        match c {
            Command::Probe { step, .. } => {
                let i = run.manifest.index(&step)?;
                let cwd = self.cwd(run, i, 0);
                let result = match run.manifest.steps[i].executor {
                    jaynes_core::manifest::Executor::Agent => self.setup.agent.probe(&cwd),
                    _ => self.setup.role.call(Call::Hello { engine: jaynes_core::ENGINE.into(), features: vec![], extensions: Default::default() }).map(|_| ()),
                };
                Some(Input::ProbeEnded { step, ok: result.is_ok(), reason: result.err().unwrap_or_default() })
            }
            Command::RunAgent(turn) => Some(self.agent_turn(run, turn)),
            // the person's ask to a stop's agent: a turn that only reads, whose answer joins the stop
            Command::AnswerAsk(turn) => {
                let ask = turn.ask.clone();
                Some(Input::AskEnded { ask, outcome: self.answer_ask(run, turn) })
            }
            Command::RunRole { step, round, attempt, given, previous, answers, key } => {
                Some(self.role_step(run, step, round, attempt, given, previous, answers, key))
            }
            Command::RunChecks { step, attempt, fix, output, given, checks, .. } => {
                let results = self.checks(run, &step, &output, &given, &checks, None);
                let results = self.keep_failed_cases(run, &step, attempt, results);
                Some(Input::ChecksEnded { step, attempt, fix, results })
            }
            Command::Ask { question } => {
                self.show(run, &question);
                None
            }
            Command::Permit { .. } => None,
            Command::Deliver { digest, merge, .. } => {
                let (ok, reference, merged) = self.deliver(run, merge);
                if ok {
                    say(&format!("delivered: {reference}"));
                }
                Some(Input::DeliveryEnded { digest, ok, reference, merged })
            }
            Command::Notify { text } => {
                say(&format!("! {text}"));
                None
            }
        }
    }

    /// Deliver from the run's copy of its target, and land it on the person's own thing when the role's delivery
    /// lands (on a repository: the run's branch, and the merge into the branch it started from). Whether it was made,
    /// and what was made, and, asked to merge, whether it landed on the person's branch (already there counts).
    fn deliver(&self, run: &Run, merge: bool) -> (bool, String, Option<bool>) {
        let writers: Vec<String> = run
            .path()
            .into_iter()
            .filter(|&j| run.manifest.steps[j].reach.project == ProjectAccess::Write)
            .map(|j| run.manifest.steps[j].name.clone())
            .collect();
        // what is delivered, which leaves out what the role only shows: what the merge lands
        let mut delivered: Option<String> = None;
        let (ok, reference) = match &self.setup.copy {
            // the copy is as its checks judged it, or something other than Jaynes changed it after them
            Some(copy) => match copy.drifted() {
                Ok(changed) if !changed.is_empty() => {
                    let back = match copy.put_back() {
                        Ok(()) => "Jaynes put the copy back as its checks judged it; continue to deliver that".to_string(),
                        Err(e) => format!("Jaynes could not put the copy back: {e}"),
                    };
                    (false, format!(
                        "the run's copy of the project changed after its checks judged it, which only a program the run started and left running could do: {}. {back}",
                        changed.join(", ")
                    ))
                }
                Err(e) => (false, e),
                Ok(_) => match copy.deliver(&self.changes(&writers), &|path| run.manifest.only_shown(path)) {
                    Ok((r, head)) => {
                        delivered = Some(head);
                        (true, r)
                    }
                    Err(e) => (false, e),
                },
            },
            None => (true, "no project; the outputs are the delivery".to_string()),
        };
        let mut reference = reference;
        let mut landed = merge.then_some(false);
        if ok && merge
            && let Some(copy) = &self.setup.copy {
                let head = delivered.unwrap_or_default();
                // what the role only shows is not in what is delivered: the checks that ran the project's tests
                // judge that once more, and the merge waits on them
                let left_out = copy.head().is_ok_and(|h| h != head);
                if let Some(why) = left_out.then(|| self.judged_again(run, &head)).flatten() {
                    reference.push_str(&format!("; not merged: {why}"));
                    return (ok, reference, landed);
                }
                let tests = run.tests_changed();
                let note = if tests.is_empty() { String::new() } else { format!("Tests changed with the person's leave: {}.", tests.join(", ")) };
                let merged = copy.land(&head, &self.setup.run, &note);
                // For the tests of recovery: die where a kill can fall, once the merge landed.
                if std::env::var("JAYNES_DIE_AFTER_MERGE").is_ok() && matches!(merged, Ok(Some(_))) {
                    std::process::exit(9);
                }
                match merged {
                    Ok(Some(m)) => {
                        let mut merges = self.merges();
                        merges.retain(|x| *x != m);
                        merges.push(m.clone());
                        let _kept = std::fs::write(self.setup.dir.join("merges.json"), serde_json::to_string(&merges).unwrap_or_default());
                        reference.push_str(&format!("; merged as {}", &m[..m.len().min(12)]));
                        landed = Some(true);
                    }
                    Ok(None) => {
                        reference.push_str(crate::listing::NOTHING_NEW);
                        landed = Some(true);
                    }
                    Err(why) => reference.push_str(&format!("; not merged: {why}")),
                }
            }
        (ok, reference, landed)
    }

    /// The checks of the delivered steps that run the project's tests, once more on `head`, what the run delivers
    /// without what its role only shows. Why the merge may not go, when one does not pass; None when all do.
    fn judged_again(&self, run: &Run, head: &str) -> Option<String> {
        let started = std::time::Instant::now();
        let (mut why, mut ran) = (Vec::new(), false);
        for j in run.path() {
            let decl = &run.manifest.steps[j];
            let Some(done) = run.steps[j].done.as_ref().filter(|_| decl.delivered) else { continue };
            let names: Vec<String> = decl.checks.iter().filter(|c| c.project_tests).map(|c| c.name.clone()).collect();
            if names.is_empty() {
                continue;
            }
            ran = true;
            for r in self.checks(run, &decl.name, &done.output, &done.reads, &names, Some(head)) {
                match r.verdict {
                    CheckVerdict::Pass => {}
                    CheckVerdict::Fail { reasons, .. } => why.push(format!(
                        "the check {} of {} fails on what is delivered, without what the role only shows: {}",
                        r.check, decl.name, reasons.join("; ")
                    )),
                    CheckVerdict::Blocked { reason } => why.push(format!(
                        "the check {} of {} could not judge what is delivered, without what the role only shows: {reason}",
                        r.check, decl.name
                    )),
                }
            }
        }
        if ran {
            say(&format!("  checked again on what is delivered in {:.1}s", started.elapsed().as_secs_f64()));
        }
        (!why.is_empty()).then(|| why.join(". "))
    }

    fn cwd(&self, run: &Run, i: usize, attempt: u32) -> PathBuf {
        let decl = &run.manifest.steps[i];
        match (&self.setup.copy, decl.reach.project) {
            (Some(copy), ProjectAccess::Read | ProjectAccess::Write) => copy.dir().to_path_buf(),
            _ => self.setup.dir.join("steps").join(&decl.name).join(format!("attempt-{attempt}")),
        }
    }

    fn given(&self, reads: &[Read]) -> Result<Vec<Given>, String> {
        reads
            .iter()
            .map(|r| Ok(Given { step: r.step.clone(), output: self.store.get(&r.output)?["output"].clone() }))
            .collect()
    }

    fn output_of(&self, d: &Option<Digest>) -> Option<Value> {
        d.as_ref().and_then(|d| self.store.get(d).ok()).map(|v| v["output"].clone())
    }

    fn agent_turn(&mut self, run: &Run, turn: AgentTurn) -> Input {
        let fault = |reason: String| Input::AttemptEnded {
            step: turn.step.clone(),
            attempt: turn.attempt,
            fix: turn.fix,
            outcome: AttemptOutcome::Fault { reason, usage: Default::default() },
        };
        let Some(i) = run.manifest.index(&turn.step) else { return fault("no such step".into()) };
        if let Some(keep) = &turn.keep {
            let outcome = self.kept_alone(run, i, &turn, keep).unwrap_or_else(|reason| AttemptOutcome::Fault { reason, usage: Default::default() });
            return Input::AttemptEnded { step: turn.step.clone(), attempt: turn.attempt, fix: turn.fix, outcome };
        }
        let cwd = self.cwd(run, i, turn.attempt);
        let exchange = cwd.join(EXCHANGE);
        let given = match self.given(&turn.given) {
            Ok(g) => g,
            Err(e) => return fault(e),
        };
        let previous = self.output_of(&turn.previous);
        let left = if turn.fix == 0 {
            match self.prepare(run, i) {
                Ok(left) => left,
                Err(e) => return fault(e),
            }
        } else {
            false
        };
        let prompt = if turn.fix == 0 {
            let reply = self.setup.role.call(Call::Prompt {
                step: turn.step.clone(),
                request: run.request.clone(),
                round: turn.round,
                attempt: turn.attempt,
                given: given.clone(),
                previous: previous.clone(),
                notes: turn.notes.clone(),
                failure: turn.failure.clone(),
                checks: turn.checks.clone(),
            });
            let (text, schema) = match reply {
                Ok(Body::Prompt { text, schema }) => (text, schema),
                Ok(other) => return fault(format!("the role answered a prompt with {other:?}")),
                Err(e) => return fault(e),
            };
            if let Err(e) = write_exchange(&exchange, &given, previous.as_ref()) {
                return fault(e);
            }
            // what the reviewer said, when the work was sent back from it; otherwise none lingers from before
            let review = exchange.join("review.json");
            match turn.review.as_ref().map(|r| self.output_of(&Some(r.output.clone()))) {
                Some(Some(said)) => {
                    if let Err(e) = std::fs::write(&review, said.to_string()) {
                        return fault(e.to_string());
                    }
                }
                _ => {
                    let _gone = std::fs::remove_file(&review);
                }
            }
            // what the person brings back, each with the output it was in; none lingers from an earlier round
            let back = exchange.join("back");
            let _gone = std::fs::remove_dir_all(&back);
            for (n, b) in turn.back.iter().enumerate() {
                let Some(was) = self.output_of(&Some(b.output.clone())) else { continue };
                if let Err(e) = std::fs::create_dir_all(&back).and_then(|()| std::fs::write(back.join(format!("{}.json", n + 1)), was.to_string())) {
                    return fault(e.to_string());
                }
            }
            let name = &run.manifest.steps[i].name;
            let read = run.manifest.steps[i].stop.is_some()
                || run.manifest.steps.iter().any(|s| s.stop.as_ref().and_then(|st| st.redo.as_deref()) == Some(name.as_str()));
            let mut p = compose(&run.request, &text, &schema, &turn, &given, read);
            if left {
                p.push_str(LEFT_BY_THE_LAST_ATTEMPT);
            }
            p
        } else {
            let _gone = std::fs::remove_file(exchange.join("output.json"));
            let _gone = std::fs::remove_file(exchange.join(IMPOSSIBLE));
            fix_prompt(&turn)
        };
        say(&format!("· {} attempt {}{}", turn.step, turn.attempt, if turn.fix > 0 { format!(", fix {}", turn.fix) } else { String::new() }));
        let mut reads = self.setup.copy.as_ref().map(|c| c.reads()).unwrap_or_default();
        let mut git_lists = PersonGitLists::default();
        if let Ok(home) = std::env::var("HOME").map(PathBuf::from).and_then(|h| std::fs::canonicalize(h).map_err(|_| std::env::VarError::NotPresent)) {
            reads.extend(crate::project::toolchains(&std::env::var("PATH").unwrap_or_default(), &home));
            reads.extend(crate::project::browsers(&home, |k| std::env::var(k).ok()));
            git_lists = person_git_lists(&home, |k| std::env::var(k).ok());
            reads.extend(git_lists.reads.iter().cloned());
        }
        let mut env: Vec<(String, String)> = vec![("JAYNES_OUTPUT".into(), exchange.join("output.json").display().to_string())];
        // the toolchains' caches inside the box's own folder, offline
        let scratch = exchange.join("tmp");
        for (k, v) in [("GOCACHE", "go-cache"), ("GOPATH", "go-path"), ("npm_config_cache", "npm-cache")] {
            env.push((k.into(), scratch.join(v).display().to_string()));
        }
        env.push(("GOTOOLCHAIN".into(), "local".into()));
        env.push(("GOTELEMETRY".into(), "off".into()));
        // the agent's git reads none of the person's configuration (their identity, filters, global hooks), which the
        // box hides anyway, and commits in the copy as Jaynes's agent where a repository says no identity of its own:
        // what it commits there is never what is delivered
        let gitconfig = exchange.join("gitconfig");
        if let Err(e) = std::fs::write(&gitconfig, format!("{AGENT_GITCONFIG}{}", git_lists.config)) {
            return fault(format!("Jaynes could not write the agent's git configuration: {e}"));
        }
        env.push(("GIT_CONFIG_GLOBAL".into(), gitconfig.display().to_string()));
        for (k, v) in AGENT_GIT_ENV {
            env.push((k.to_string(), v.to_string()));
        }
        // offline, unless the step installs packages from their registries
        if turn.reach.network != jaynes_core::manifest::Network::Packages {
            env.push(("GOPROXY".into(), "off".into()));
            env.push(("npm_config_offline".into(), "true".into()));
        }
        if let Some(copy) = &self.setup.copy {
            let venv = copy.dir().join(".venv");
            env.push(("PYTHONPATH".into(), format!("{}:{}", copy.dir().join("src").display(), copy.dir().display())));
            env.push(("UV_NO_SYNC".into(), "1".into()));
            env.push(("UV_OFFLINE".into(), "1".into()));
            if venv.exists() {
                env.push(("VIRTUAL_ENV".into(), venv.display().to_string()));
                env.push(("PATH".into(), format!("{}:{}", venv.join("bin").display(), std::env::var("PATH").unwrap_or_default())));
            }
        }
        let spec = agent::Spec {
            cwd: &cwd,
            prompt: &prompt,
            session: turn.session.as_deref(),
            reach: &turn.reach,
            deadline: Duration::from_secs(turn.deadline_seconds.max(1)),
            reads: &reads,
            env,
            transcript: Some(self.setup.dir.join("steps").join(&turn.step).join(format!("turn-{}-{}.jsonl", turn.attempt, turn.fix))),
            pidfile: Some(self.setup.dir.join("agent.pid")),
            paused: Some(crate::permit::waiting_mark(self.setup.dir.parent().and_then(Path::parent).unwrap_or(Path::new("/")), &self.setup.run)),
            waits: self.setup.role.waits.clone(),
            on_session: {
                let (home, run, step, attempt) = (self.setup.dir.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default(), self.setup.run.clone(), turn.step.clone(), turn.attempt);
                Some(Box::new(move |session: String, confirmed: jaynes_core::event::Confirmed| {
                    // recorded at once, so a turn cut off from outside goes on in this session
                    let recorded = Store::open(&home.join("record.sqlite3")).and_then(|mut s| s.decide(&run, &Input::SessionStarted { step: step.clone(), attempt, session, confirmed }));
                    if let Err(e) = recorded {
                        say(&format!("the agent's session was not recorded: {e}"));
                    }
                }))
            },
            permit: std::env::current_exe().ok().map(|exe| {
                let home = self.setup.dir.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
                let argv = vec![
                    exe.display().to_string(), "permit".into(), "--run".into(), self.setup.run.clone(), "--step".into(),
                    turn.step.clone(), "--attempt".into(), turn.attempt.to_string(), "--cwd".into(), cwd.display().to_string(),
                ];
                (argv, vec![("JAYNES_ENGINE_HOME".to_string(), home.display().to_string())])
            }),
            // what the person chose for the project's ports, as its checks run with it
            servers: self.setup.servers || self.setup.any_port,
            any_port: self.setup.any_port,
            freed_ports: if self.setup.servers { self.setup.freed_ports.clone() } else { Vec::new() },
            budget: Some(turn.tokens_left),
            model: run.manifest.steps[i].model,
            turns: run.manifest.steps[i].turns,
        };
        if let Some(t) = &spec.transcript {
            let _made = std::fs::create_dir_all(t.parent().unwrap_or(Path::new(".")));
        }
        let done = self.setup.agent.turn(spec);
        let _kept = std::fs::write(exchange.join(format!("applied-{}-{}.json", turn.attempt, turn.fix)), done.applied.to_string());
        let outcome = match done.ended {
            Ended::Fault { reason, usage } => AttemptOutcome::Fault { reason, usage },
            Ended::Interrupted { reason, usage } => AttemptOutcome::Interrupted { reason, usage },
            Ended::Failed { reason, usage } => AttemptOutcome::Failed { reasons: vec![reason], usage },
            Ended::Ran { usage, session, .. } => self.delivered(run, i, &turn, &exchange, usage, session),
        };
        Input::AttemptEnded { step: turn.step, attempt: turn.attempt, fix: turn.fix, outcome }
    }

    /// Answer the person's question at a stop: a turn that only reads, in the session of the step that made what the
    /// stop shows (a session of its own, given that in asked-about.json, when there is none), and that redoes nothing.
    /// Nothing it asks beyond its settings reaches the person, and its session is never one a cut-off attempt goes on
    /// in. The answer is what it wrote in reply.md, else its result's last words. A turn that changed the copy fails,
    /// and what it changed is put back.
    pub(crate) fn answer_ask(&mut self, run: &Run, turn: AskTurn) -> AskOutcome {
        let failed = |reason: String, usage: jaynes_core::event::Usage| AskOutcome::Failed { reason, usage };
        let Some(i) = run.manifest.index(&turn.step) else { return failed("no such step".into(), Default::default()) };
        let copy = self.setup.copy.as_ref().filter(|_| run.manifest.steps[i].reach.project != ProjectAccess::None);
        let cwd = match copy {
            Some(c) => c.dir().to_path_buf(),
            None => self.setup.dir.join("steps").join(&turn.step).join(format!("ask-{}", turn.ask)),
        };
        let exchange = cwd.join(EXCHANGE);
        let about = self.output_of(&Some(turn.output.clone())).unwrap_or(Value::Null);
        let _gone = std::fs::remove_file(exchange.join(REPLY));
        if let Err(e) = std::fs::create_dir_all(&exchange).and_then(|()| std::fs::write(exchange.join(ASKED_ABOUT), about.to_string())) {
            return failed(format!("Jaynes cannot write {}: {e}", exchange.display()), Default::default());
        }
        let before = match copy.map(|c| c.drifted()).transpose() {
            Ok(b) => b.unwrap_or_default(),
            Err(e) => return failed(e, Default::default()),
        };
        let prompt = ask_prompt(&run.request, &turn);
        say(&format!("· {} answers a question", turn.step));
        let transcript = self.setup.dir.join("steps").join(&turn.step).join(format!("ask-{}.jsonl", turn.ask));
        let _made = std::fs::create_dir_all(transcript.parent().unwrap_or(Path::new(".")));
        let reads = copy.map(|c| c.reads()).unwrap_or_default();
        let spec = agent::Spec {
            cwd: &cwd,
            prompt: &prompt,
            session: turn.session.as_deref(),
            reach: &turn.reach,
            deadline: Duration::from_secs(turn.deadline_seconds.max(1)),
            reads: &reads,
            env: AGENT_GIT_ENV.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            transcript: Some(transcript),
            pidfile: Some(self.setup.dir.join("agent.pid")),
            paused: None,
            waits: self.setup.role.waits.clone(),
            on_session: None,
            permit: None,
            servers: self.setup.servers || self.setup.any_port,
            any_port: self.setup.any_port,
            freed_ports: if self.setup.servers { self.setup.freed_ports.clone() } else { Vec::new() },
            budget: Some(turn.tokens_left),
            model: run.manifest.steps[i].model,
            turns: None,
        };
        let done = self.setup.agent.turn(spec);
        let reply = reply_in(&exchange);
        let _gone = std::fs::remove_file(exchange.join(REPLY));
        let _gone = std::fs::remove_file(exchange.join(ASKED_ABOUT));
        let usage = match &done.ended {
            Ended::Ran { usage, .. } | Ended::Failed { usage, .. } | Ended::Fault { usage, .. } | Ended::Interrupted { usage, .. } => *usage,
        };
        if let Some(c) = copy {
            let changed: Vec<String> = match c.drifted() {
                Ok(after) => after.into_iter().filter(|p| !before.contains(p)).collect(),
                Err(e) => return failed(e, usage),
            };
            if !changed.is_empty() {
                let back = match c.put_back() {
                    Ok(()) => "they were put back".to_string(),
                    Err(e) => format!("putting them back failed: {e}"),
                };
                return failed(format!("the agent changed {} while it answered, which a question never may: {back}", changed.join(", ")), usage);
            }
        }
        match done.ended {
            Ended::Ran { usage, session, said, .. } => match reply.or_else(|| reply_text(&said)) {
                Some(text) => AskOutcome::Answered { text, usage, session },
                None => failed("the agent answered nothing: ask again".into(), usage),
            },
            Ended::Failed { reason, usage } => failed(reason, usage),
            Ended::Fault { reason, .. } => failed(reason, usage),
            Ended::Interrupted { reason, .. } => failed(format!("stopped from outside ({reason}): ask again"), usage),
        }
    }

    fn delivered(&self, run: &Run, i: usize, turn: &AgentTurn, exchange: &Path, usage: jaynes_core::event::Usage, session: Option<String>) -> AttemptOutcome {
        let failed = |why: String| AttemptOutcome::Failed { reasons: vec![why], usage };
        // the agent's way out: it says the step cannot be done as asked, and the person answers
        let root = exchange.parent().unwrap_or(exchange);
        if let Ok(why) = read_inside(root, &exchange.join(IMPOSSIBLE))
            && !why.trim().is_empty()
        {
            let _gone = std::fs::remove_file(exchange.join(IMPOSSIBLE));
            let reason: String = why.trim().chars().take(2000).collect();
            return AttemptOutcome::Impossible { reason, usage, session };
        }
        let text = match read_inside(root, &exchange.join("output.json")) {
            Ok(t) => t,
            Err(None) => return failed(format!("there is no output: write it as JSON to {EXCHANGE}/output.json")),
            Err(Some(why)) => return failed(format!("{EXCHANGE}/output.json {why}")),
        };
        let output: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => return failed(format!("{EXCHANGE}/output.json is not JSON: {e}")),
        };
        let decl = &run.manifest.steps[i];
        if decl.hidden {
            // What a hidden step wrote must not stay where a later agent works.
            let _gone = std::fs::remove_file(exchange.join("output.json"));
        }
        let mut changed = Vec::new();
        let mut tree = Value::Null;
        if let (Some(copy), ProjectAccess::Write | ProjectAccess::Read) = (&self.setup.copy, decl.reach.project) {
            let write = if decl.reach.project == ProjectAccess::Write { decl.reach.write.clone() } else { Vec::new() };
            match copy.measure(&write) {
                Ok((c, t)) => {
                    changed = c;
                    tree = Value::String(t);
                }
                Err(Ok(paths)) => return failed(format!("the step changed files it may not change: {}", paths.join(", "))),
                Err(Err(e)) => return AttemptOutcome::Fault { usage: Default::default(), reason: e },
            }
        }
        let object = json!({"output": output, "tree": tree});
        let digest = match self.store.put(&object) {
            Ok(d) => d,
            Err(e) => return AttemptOutcome::Fault { usage: Default::default(), reason: e },
        };
        let items = match self.setup.role.call(Call::Items { step: turn.step.clone(), output: object["output"].clone() }) {
            Ok(Body::Items { items }) => match self.keep_items(&items, exchange.parent().unwrap_or(exchange)) {
                Ok(i) => i,
                Err(e) => return AttemptOutcome::Fault { usage: Default::default(), reason: e },
            },
            Ok(other) => return AttemptOutcome::Fault { usage: Default::default(), reason: format!("the role answered items with {other:?}") },
            Err(e) => return AttemptOutcome::Fault { usage: Default::default(), reason: e },
        };
        // the agent's words to the person, beside its output and never inside it (thread A)
        let reply = reply_in(exchange);
        AttemptOutcome::Delivered { output: digest, items, changed, usage, session, tree: tree.as_str().map(str::to_string), reply }
    }

    /// A round of a stop that chooses, when the person chose without a note: the role keeps the chosen options of
    /// the step's last output, each as it was, with the same files; no agent works.
    fn kept_alone(&self, run: &Run, i: usize, turn: &AgentTurn, keep: &[String]) -> Result<AttemptOutcome, String> {
        say(&format!("· {}: keeping only what you chose", turn.step));
        self.prepare(run, i)?;
        let previous = turn.previous.as_ref().ok_or("there is no output to choose from")?;
        let object = self.store.get(previous)?;
        let output = match self.setup.role.call(Call::Narrow { step: turn.step.clone(), output: object["output"].clone(), keep: keep.to_vec() })? {
            Body::Result { output } => output,
            other => return Err(format!("the role could not keep what you chose: {other:?}")),
        };
        let digest = self.store.put(&json!({"output": output, "tree": object["tree"].clone()}))?;
        let shown = match self.setup.role.call(Call::Items { step: turn.step.clone(), output })? {
            Body::Items { items } => items,
            other => return Err(format!("the role answered items with {other:?}")),
        };
        let mut ids: Vec<&str> = shown.iter().map(|x| x.id.as_str()).collect();
        let mut want: Vec<&str> = keep.iter().map(String::as_str).collect();
        ids.sort_unstable();
        want.sort_unstable();
        if ids != want {
            return Err(format!("the role kept {} when you chose {}", ids.join(", "), want.join(", ")));
        }
        let items = self.keep_items(&shown, &self.cwd(run, i, turn.attempt))?;
        let changed = run.steps[i].done.as_ref().map(|d| d.changed.clone()).unwrap_or_default();
        Ok(AttemptOutcome::Delivered { output: digest, items, changed, usage: Default::default(), session: None, tree: object["tree"].as_str().map(str::to_string), reply: None })
    }

    /// The items as the core knows them, after keeping a copy of each picture by its fingerprint and how each item
    /// is shown. A picture outside the step's folder, or that does not read, is Jaynes's fault to report.
    fn keep_items(&self, items: &[crate::protocol::ShownItem], root: &Path) -> Result<Vec<jaynes_core::question::Item>, String> {
        let home = self.setup.dir.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
        let shelf = home.join("pictures");
        let mut out = Vec::new();
        for x in items {
            let mut kept = Vec::new();
            // a photographed page, kept too by its fingerprint, under its photograph's name: the person opens it
            let mut pages = serde_json::Map::new();
            for p in &x.pictures {
                let path = root.join(p);
                // a picture is read only as a plain file of the step's folder, reached through no link
                let real = plain_inside(root, &path).map_err(|e| format!("the picture {p} of {} does not read: {e}", x.id))?;
                let base = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
                let mut ext = real.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                let mut page_kept = None;
                // a page is photographed by Jaynes, and the photograph left beside the copy's files for later agents
                let real = if ext == "html" || ext == "htm" {
                    let rel = real.strip_prefix(&base).unwrap_or(&real).with_extension("png");
                    let photo = base.join(EXCHANGE).join("pictures").join(rel);
                    // the same page keeps the same photograph, so an option kept as it was stays as it was
                    let source = std::fs::read(&real).map_err(|e| e.to_string())?;
                    let page = Digest::of_bytes(&source).0;
                    // and the same page photographed in another window, or in another theme, is another photograph
                    let taken = photo_mark(&page, x.photo);
                    let name = format!("{page}.html");
                    std::fs::create_dir_all(&shelf).map_err(|e| e.to_string())?;
                    std::fs::write(shelf.join(&name), &source).map_err(|e| e.to_string())?;
                    page_kept = Some(name);
                    let of = photo.with_extension("png.of");
                    // Jaynes writes these in the copy, where an agent may have left a link to anywhere: the photograph is
                    // made in the run's own folder, out of the agent's reach, and put in place without following a link
                    let made = plain_inside(&base, &photo).is_ok();
                    if !(made && read_inside(&base, &of).is_ok_and(|d| d == taken)) {
                        let shot = self.setup.dir.join("photograph.png");
                        let _gone = std::fs::remove_file(&shot);
                        self.photograph(&real, &shot, x.photo).map_err(|e| format!("the picture {p} of {}: {e}", x.id))?;
                        let bytes = std::fs::read(&shot).map_err(|e| e.to_string())?;
                        write_inside(&base, &photo, &bytes).map_err(|e| format!("the picture {p} of {}: {e}", x.id))?;
                        write_inside(&base, &of, taken.as_bytes()).map_err(|e| format!("the picture {p} of {}: {e}", x.id))?;
                    }
                    ext = "png".into();
                    photo
                } else {
                    real
                };
                if !["png", "jpg", "jpeg", "gif", "webp", "svg"].contains(&ext.as_str()) {
                    return Err(format!("the picture {p} of {} is not an image Jaynes shows", x.id));
                }
                let bytes = std::fs::read(&real).map_err(|e| e.to_string())?;
                let name = format!("{}.{ext}", Digest::of_bytes(&bytes).0);
                std::fs::create_dir_all(&shelf).map_err(|e| e.to_string())?;
                std::fs::write(shelf.join(&name), &bytes).map_err(|e| e.to_string())?;
                if let Some(page) = page_kept {
                    pages.insert(name.clone(), page.into());
                }
                kept.push(name);
            }
            let item = x.item_with(&kept);
            if !x.detail.is_empty() || !kept.is_empty() || !x.aspects.is_empty() {
                self.store.put_display(&item.digest, &json!({"detail": x.detail, "pictures": kept, "pages": pages, "aspects": x.aspects}))?;
            }
            out.push(item);
        }
        Ok(out)
    }

    /// Photograph a page with the library's Chrome, in Jaynes's box and offline.
    fn photograph(&self, page: &Path, out: &Path, photo: Option<crate::protocol::Photo>) -> Result<(), String> {
        let python = std::env::var("JAYNES_PYTHON").unwrap_or_else(|_| "python3".to_string());
        let mut how = Vec::new();
        if let Some(p) = photo {
            how.push(format!("{}x{}", p.width, p.height));
            if p.whole {
                how.push("whole".to_string());
            }
        }
        let done = std::process::Command::new(python)
            .arg(self.setup.role.library.join("jaynes_box.py"))
            .arg("photograph")
            .arg(page)
            .arg(out)
            .args(how)
            .env("PYTHONPATH", &self.setup.role.library)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .output()
            .map_err(|e| format!("Jaynes could not start its photographer: {e}"))?;
        if done.status.success() && out.is_file() {
            return Ok(());
        }
        let said = String::from_utf8_lossy(&done.stdout).trim().to_string();
        Err(if said.is_empty() { String::from_utf8_lossy(&done.stderr).chars().take(400).collect() } else { said })
    }

    #[allow(clippy::too_many_arguments)]
    fn role_step(&mut self, run: &Run, step: String, round: u32, attempt: u32, given: Vec<Read>, previous: Option<Digest>, answers: Vec<Answer>, key: Option<String>) -> Input {
        let end = |outcome| Input::AttemptEnded { step: step.clone(), attempt, fix: 0, outcome };
        let given = match self.given(&given) {
            Ok(g) => g,
            Err(e) => return end(AttemptOutcome::Fault { usage: Default::default(), reason: e }),
        };
        let i = run.manifest.index(&step).unwrap_or(0);
        if let Err(e) = self.prepare(run, i) {
            return end(AttemptOutcome::Fault { usage: Default::default(), reason: e });
        }
        let reaches = run.manifest.steps[i].reach.project != ProjectAccess::None;
        let project = self.setup.copy.as_ref().filter(|_| reaches).map(|c| c.dir().display().to_string());
        let source = self.setup.copy.as_ref().filter(|_| reaches).map(|c| c.source().display().to_string());
        say(&format!("· {step} (the role's own step)"));
        let reply = self.setup.role.call(Call::Execute {
            step: step.clone(),
            request: run.request.clone(),
            round,
            attempt,
            given,
            previous: self.output_of(&previous),
            answers,
            key,
            project,
            source,
            any_port: self.setup.any_port,
            test_servers: self.setup.servers,
            freed_ports: self.setup.freed_ports.clone(),
        });
        let outcome = match reply {
            Err(e) => AttemptOutcome::Fault { usage: Default::default(), reason: e },
            Ok(Body::Result { output }) => {
                // What the role's own step changed in the run's copy is measured by Jaynes, as an agent's change is:
                // the risk is judged from it, and a change outside what the step may write is refused and thrown away.
                let decl = &run.manifest.steps[i];
                let mut changed = Vec::new();
                let mut tree = Value::Null;
                if let (Some(copy), ProjectAccess::Write | ProjectAccess::Read) = (&self.setup.copy, decl.reach.project) {
                    let write = if decl.reach.project == ProjectAccess::Write { decl.reach.write.clone() } else { Vec::new() };
                    match copy.measure(&write) {
                        Ok((c, t)) => {
                            changed = c;
                            tree = Value::String(t);
                        }
                        Err(Ok(paths)) => {
                            // its script ran to the end before this was found: what cannot be undone may be done
                            let reason = format!("the step changed files it may not change: {}", paths.join(", "));
                            // what may have acted is held by the core as an outcome not known, never as a refusal
                            return end(if decl.effect.irreversible() { AttemptOutcome::Fault { reason, usage: Default::default() } } else { AttemptOutcome::Refused { reason } });
                        }
                        Err(Err(e)) => return end(AttemptOutcome::Fault { usage: Default::default(), reason: e }),
                    }
                }
                let object = json!({"output": output, "tree": tree});
                match self.store.put(&object) {
                    Err(e) => AttemptOutcome::Fault { usage: Default::default(), reason: e },
                    Ok(digest) => match self.setup.role.call(Call::Items { step: step.clone(), output: object["output"].clone() }) {
                        Ok(Body::Items { items }) => AttemptOutcome::Delivered {
                            output: digest,
                            items: match self.keep_items(&items, &self.cwd(run, i, attempt)) {
                                Ok(i) => i,
                                Err(e) => return end(AttemptOutcome::Fault { usage: Default::default(), reason: e }),
                            },
                            changed,
                            usage: Default::default(),
                            session: None,
                            tree: tree.as_str().map(str::to_string),
                            reply: None,
                        },
                        Ok(other) => AttemptOutcome::Fault { usage: Default::default(), reason: format!("the role answered items with {other:?}") },
                        Err(e) => AttemptOutcome::Fault { usage: Default::default(), reason: e },
                    },
                }
            }
            Ok(Body::NeedsInput { items, offered, about }) => {
                match self.keep_items(&items, &self.cwd(run, i, attempt)) {
                    Ok(items) => AttemptOutcome::NeedsInput { items, offered, about },
                    Err(e) => AttemptOutcome::Fault { usage: Default::default(), reason: e },
                }
            }
            Ok(Body::Refused { reason }) => AttemptOutcome::Refused { reason },
            Ok(other) => AttemptOutcome::Fault { usage: Default::default(), reason: format!("the role answered execute with {other:?}") },
        };
        end(outcome)
    }

    /// The hidden cases that failed, kept for the person's card in the run's folder, which no agent reads, and taken
    /// out of what goes on to the core: the agent is told only each case's sentence (G3).
    fn keep_failed_cases(&self, run: &Run, step: &str, attempt: u32, results: Vec<CheckResult>) -> Vec<CheckResult> {
        let decl = run.manifest.index(step).map(|i| &run.manifest.steps[i]);
        let mut failed = Vec::new();
        let mut at = String::new();
        let results = results
            .into_iter()
            .map(|mut r| {
                if let CheckVerdict::Fail { cases, .. } = &mut r.verdict
                    && !cases.is_empty()
                {
                    let check = decl.and_then(|d| d.checks.iter().find(|c| c.name == r.check));
                    if check.is_some_and(|c| c.kind == jaynes_core::manifest::CheckKind::Hidden) {
                        // the hidden step that wrote the cases, among those the check reads
                        if at.is_empty() {
                            at = check
                                .into_iter()
                                .flat_map(|c| c.reads.iter())
                                .find(|n| run.manifest.index(n).is_some_and(|j| run.manifest.steps[j].hidden))
                                .cloned()
                                .unwrap_or_default();
                        }
                        failed.append(cases);
                    } else {
                        cases.clear();
                    }
                }
                r
            })
            .collect();
        if !failed.is_empty()
            && let Err(e) = crate::listing::keep_failed_cases(&self.setup.dir, step, attempt, &at, &failed)
        {
            say(&format!("  could not keep the hidden cases that failed for the person: {e}"));
        }
        results
    }

    /// The checks of `step` on what its attempt made, measured as `output`; or, `on`, on that state of the copy
    /// (what the run delivers) in its place.
    fn checks(&self, run: &Run, step: &str, output: &Digest, given: &[Read], checks: &[String], on: Option<&str>) -> Vec<CheckResult> {
        // the hidden cases the person took out of the run at this step, after seeing them fail: judged without
        let without = run.manifest.index(step).map(|i| run.cases_out(i)).unwrap_or_default();
        let blocked = |check: &str, reason: String| CheckResult { check: check.to_string(), verdict: CheckVerdict::Blocked { reason }, tree: None, tests_changed: Vec::new() };
        let (output, tree) = match self.store.get(output) {
            Ok(v) => (v["output"].clone(), on.map(str::to_string).or_else(|| v["tree"].as_str().map(str::to_string))),
            Err(e) => return checks.iter().map(|c| blocked(c, e.clone())).collect(),
        };
        let given = match self.given(given) {
            Ok(g) => g,
            Err(e) => return checks.iter().map(|c| blocked(c, e.clone())).collect(),
        };
        let i = run.manifest.index(step).unwrap_or(0);
        let decl = &run.manifest.steps[i];
        let reaches = decl.reach.project != ProjectAccess::None;
        let copy = self.setup.copy.as_ref().filter(|_| reaches);
        // the checks judge the state the attempt was measured at, laid out apart, never the copy as it is by then: a
        // program the step left running could have changed it
        let laid = match (copy, &tree) {
            (Some(c), Some(t)) => match c.lay_out(t) {
                Ok(l) => Some(l),
                Err(e) => return checks.iter().map(|c| blocked(c, e.clone())).collect(),
            },
            _ => None,
        };
        let judged = laid.as_ref().and(tree.clone());
        let project = laid.as_ref().map(|l| l.dir.display().to_string()).or_else(|| copy.map(|c| c.dir().display().to_string()));
        let source = copy.map(|c| c.source().display().to_string());
        // where the project stood when the run began, from the run's copy itself, never from the state laid out apart
        let start = copy.and_then(|c| crate::project::start_of(c.dir())).map(|(git, commit)| crate::protocol::Start { git: git.display().to_string(), commit });
        // what git ignores is in no state and no delivery: a check that fails or cannot judge says it was not given it
        let not_given = laid.as_ref().and(copy).and_then(|c| crate::project::not_given(&c.left_out()));
        let hidden = |name: &str| decl.checks.iter().any(|c| c.name == name && c.kind == jaynes_core::manifest::CheckKind::Hidden);
        // a check whose test server got a port past the box's window, again on the same change after the person's
        // Continue: the card then says the tests themselves likely take that many ports
        let past_before = self.store.events(&run.id).ok().is_some_and(|events| {
            let at = events.iter().rposition(|e| matches!(e, Event::Blocked { .. }));
            at.is_some_and(|at| {
                let Event::Blocked { step: Some(s), reason, .. } = &events[at] else { return false };
                s == step
                    && crate::listing::past_window_said(reason)
                    && events[at..].iter().any(|e| matches!(e, Event::Continued { .. }))
                    && !events[at..].iter().any(|e| matches!(e, Event::AttemptStarted { step: s, .. } if s == step))
            })
        });
        let again = |verdict: CheckVerdict| match verdict {
            CheckVerdict::Blocked { reason } if past_before && crate::listing::past_window_said(&reason) => {
                CheckVerdict::Blocked { reason: format!("{reason}{}", crate::listing::PAST_AGAIN) }
            }
            v => v,
        };
        let told = |name: &str, verdict: CheckVerdict| match (again(verdict), &not_given) {
            (CheckVerdict::Fail { mut reasons, mut detail, cases }, Some(n)) => {
                // a hidden check's agent gets only the sentence the check wrote (G3): the person reads it
                if hidden(name) {
                    detail = if detail.is_empty() { n.clone() } else { format!("{detail}\n{n}") };
                } else {
                    reasons.push(n.clone());
                }
                CheckVerdict::Fail { reasons, detail, cases }
            }
            (CheckVerdict::Blocked { reason }, Some(n)) => CheckVerdict::Blocked { reason: format!("{reason}; {n}") },
            (v, _) => v,
        };
        checks
            .iter()
            .map(|name| {
                say(&format!("  check {name}"));
                match self.setup.role.call(Call::Check {
                    step: step.to_string(),
                    check: name.clone(),
                    request: run.request.clone(),
                    output: output.clone(),
                    given: given.clone(),
                    project: project.clone(),
                    source: source.clone(),
                    start: start.clone(),
                    any_port: self.setup.any_port,
                    test_servers: self.setup.servers,
                    freed_ports: self.setup.freed_ports.clone(),
                    tests_leave: run.tests_leave.clone(),
                    without: without.clone(),
                }) {
                    Ok(Body::Verdict { verdict, tests_changed }) => CheckResult { check: name.clone(), verdict: told(name, verdict), tree: judged.clone(), tests_changed },
                    Ok(other) => CheckResult { tree: judged.clone(), ..blocked(name, format!("the role answered a check with {other:?}")) },
                    Err(e) => CheckResult { tree: judged.clone(), ..blocked(name, e) },
                }
            })
            .collect()
    }

    fn show(&self, run: &Run, q: &Question) {
        say(&format!("? {} asks you, at {} ({}):", q.id, q.step, if q.holds { "the run waits" } else { "the run goes on" }));
        if !q.about.is_empty() {
            say(&format!("  {}", q.about));
        }
        for item in &q.items {
            say(&format!("  - {}", item_line(&item.id, &item.label, ": ")));
        }
        let offered: Vec<String> = q.offered.iter().map(|v| format!("{v:?}").to_lowercase()).collect();
        say(&format!("  answer: jaynes answer {} {} [item=verdict] ({})", run.id, q.id, offered.join(", ")));
    }
}

fn write_exchange(exchange: &Path, given: &[Given], previous: Option<&Value>) -> Result<(), String> {
    let _gone = std::fs::remove_dir_all(exchange.join("inputs"));
    let _gone = std::fs::remove_file(exchange.join("output.json"));
    let _gone = std::fs::remove_file(exchange.join(IMPOSSIBLE));
    let _gone = std::fs::remove_file(exchange.join("previous.json"));
    let _gone = std::fs::remove_file(exchange.join(REPLY));
    std::fs::create_dir_all(exchange.join("inputs")).map_err(|e| format!("Jaynes cannot write {}: {e}", exchange.display()))?;
    for g in given {
        std::fs::write(exchange.join("inputs").join(format!("{}.json", g.step)), g.output.to_string()).map_err(|e| e.to_string())?;
    }
    if let Some(p) = previous {
        std::fs::write(exchange.join("previous.json"), p.to_string()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The prompt of an attempt: the role's instructions, then what Jaynes says the same way for every role.
/// `read` says whether the person reads what the step makes at a stop: then the agent writes them a reply beside it.
/// Said to an attempt that starts from the files the attempt before it left.
pub const LEFT_BY_THE_LAST_ATTEMPT: &str = "\n# Your last attempt's files\n\nThe project is as your last attempt left it, uncommitted: start from these files, change what made it fail, and do not make again what is already there.\n";

pub fn compose(request: &str, text: &str, schema: &Value, turn: &AgentTurn, given: &[Given], read: bool) -> String {
    let mut p = String::new();
    p.push_str(&format!("# The request\n\n{request}\n\n# Your step: {}\n\n{text}\n\n", turn.step));
    if !given.is_empty() {
        p.push_str("# What earlier steps made\n\nRead them in .jaynes/inputs/: ");
        p.push_str(&given.iter().map(|g| format!("{}.json", g.step)).collect::<Vec<_>>().join(", "));
        p.push_str(".\n\n");
    }
    if turn.previous.is_some() {
        p.push_str("# Your previous output\n\nThis step runs again on top of what it made before, in .jaynes/previous.json. Keep what nobody asked you to change.\n\n");
    }
    if !turn.notes.is_empty() {
        p.push_str("# The person's words for this round\n\n");
        for n in &turn.notes {
            p.push_str(&format!("- {n}\n"));
        }
        p.push('\n');
    }
    if !turn.earlier.is_empty() {
        p.push_str("# What the person asked in earlier rounds\n\nOldest first. Keep doing what still applies; do not undo it.\n\n");
        for n in &turn.earlier {
            p.push_str(&format!("- {n}\n"));
        }
        p.push('\n');
    }
    if !turn.asked.is_empty() {
        // thread A: what the person asked at the stop, so the question does not vanish with the round that answered it
        p.push_str("# What the person asked at the stop, and what you answered\n\nAt the stop before this round, the person asked you this, and you changed nothing to answer. Keep it in mind; it is not a request on its own.\n\n");
        for a in &turn.asked {
            match &a.answer {
                Some(answer) => p.push_str(&format!("- They asked: {}\n  You answered: {answer}\n", a.text)),
                None if read => p.push_str(&format!("- They asked: {}\n  No answer reached them: answer it in your reply.\n", a.text)),
                None => p.push_str(&format!("- They asked: {}\n  No answer reached them.\n", a.text)),
            }
        }
        p.push('\n');
    }
    if let Some(r) = &turn.review {
        p.push_str(&format!(
            "# What the reviewer said of your last version\n\nThe person sent your work back from {}, which reviewed it. Read what it said in .jaynes/review.json, and do not repeat what it found wrong.\n\n",
            r.step
        ));
    }
    if !turn.context.is_empty() {
        p.push_str("# What the person said for context\n\nIn their own words, at earlier answers. Take it into account; it is not a request on its own.\n\n");
        for c in &turn.context {
            p.push_str(&format!("- {c}\n"));
        }
        p.push('\n');
    }
    if !turn.failure.is_empty() {
        p.push_str("# Why the last attempt was rejected\n\n");
        for f in &turn.failure {
            p.push_str(&format!("- {f}\n"));
        }
        p.push('\n');
    }
    if !turn.locked.is_empty() {
        p.push_str("# Kept by the person: keep each exactly as it is, with its id\n\n");
        for i in &turn.locked {
            p.push_str(&format!("- {} ({})\n", i.label, i.id));
        }
        p.push('\n');
    }
    if !turn.back.is_empty() {
        p.push_str("# Brought back by the person\n\nThey dropped these in an earlier round and want them again. Each is in the output it was in, under .jaynes/back/; give it again with the same id.\n\n");
        for (n, b) in turn.back.iter().enumerate() {
            let how = if b.words.trim().is_empty() { "as it was".to_string() } else { format!("changed as they write: {}", b.words.trim()) };
            p.push_str(&format!("- {} ({}), {how}: .jaynes/back/{}.json\n", b.item.label, b.item.id, n + 1));
        }
        p.push('\n');
    }
    if !turn.dropped.is_empty() {
        p.push_str("# Dropped by the person: none of these may come back\n\n");
        for i in &turn.dropped {
            p.push_str(&format!("- {} ({})\n", i.label, i.id));
        }
        p.push('\n');
    }
    if !turn.checks.is_empty() {
        p.push_str("# Checks your output must pass\n\nCheck them yourself before you answer; a failure comes back to you here to fix.\n\n");
        for c in &turn.checks {
            p.push_str(&format!("- {}: {}\n", c.name, c.about));
        }
        p.push('\n');
    }
    p.push_str(
        "# If it cannot be done as asked\n\nIf the request contradicts itself, needs something you cannot reach, or could pass \
         the checks only by gaming them, do not write an output. Write why, in a few plain sentences, to \
         .jaynes/impossible.txt. The person reads it and tells you how to go on; it costs you no attempt. Never write \
         an output you know is wrong to get past the checks.\n\nA command that fails with \"Operation not permitted\", \
         \"Permission denied\" or EPERM was most likely refused by Jaynes's box, not by the project: never change the \
         project's tests or code to get around it (another port, a test removed, skipped or rewritten to need less); write \
         to .jaynes/impossible.txt the command and the error it printed instead. Wherever you tell of it, say \"Jaynes's box \
         refused\" and what it refused, then the error as the command printed it, never \"this environment's protection\" \
         nor a sandbox of your own.\n\n",
    );
    if read {
        p.push_str(
            "# Your reply to the person\n\nThe person reads what you make at a stop. When you are done, write to \
             .jaynes/reply.md, in their language and in a few plain sentences, what you made or changed and why, and \
             answer there any question they asked. It goes beside your work, never inside it: no code, no JSON, no file \
             names.\n\n",
        );
    }
    p.push_str(&format!(
        "# Your output\n\nWhen you are done, write your output as JSON to .jaynes/output.json, following this schema:\n\n{}\n",
        serde_json::to_string_pretty(schema).unwrap_or_default()
    ));
    p
}

/// What the agent is told when the person asks a question at a stop. An agent in the step's session knows the rest;
/// one in a session of its own is told the request and where what it is asked about is.
fn ask_prompt(request: &str, turn: &AskTurn) -> String {
    let mut p = String::new();
    if turn.session.is_none() {
        p.push_str(&format!(
            "# The request\n\n{request}\n\n# What the person is looking at\n\nWhat the step “{}” made, in .jaynes/{ASKED_ABOUT}.\n\n",
            turn.step
        ));
    }
    let text = turn.text.trim();
    let end = if text.ends_with(['.', '!', '?', '…']) { "" } else { "." };
    p.push_str(&format!(
        "# A question from the person\n\nThe person asks about “{}”: {text}{end} Answer in your reply, in their language, in a few \
         sentences. Change nothing: the option stays as it is. This turn only reads: look at the project if you need \
         to, change nothing in it, and redo nothing.\n\n# Your reply\n\nWrite your answer to .jaynes/reply.md, as \
         plain words for the person: no code, no JSON, no file names.\n",
        turn.label
    ));
    p
}

fn fix_prompt(turn: &AgentTurn) -> String {
    let mut p = String::from("Your output failed these checks. Fix it in place, then write .jaynes/output.json again:\n\n");
    for f in &turn.failure {
        p.push_str(&format!("- {f}\n"));
    }
    p
}

/// `r` is the run after `e`, which a run's end reads for the agents' work, counted as the page and the app count it.
pub fn narrate(e: &Event, r: &Run) {
    match e {
        Event::RunStarted { run, risk, reasons, ceiling, .. } => say(&format!(
            "run {run} started at {} risk{}; ceiling {} min, {} tokens",
            risk.word(),
            if reasons.is_empty() { String::new() } else { format!(" ({})", reasons.join("; ")) },
            ceiling.seconds / 60,
            ceiling.tokens
        )),
        Event::RiskRaised { to, reasons, .. } => say(&format!("risk raised to {}: {}", to.word(), reasons.join("; "))),
        Event::StepDone { step, round, .. } => say(&format!("✓ {step} (round {round})")),
        Event::AttemptFailed { step, reasons, spent: true, .. } => say(&format!("✗ {step}: {}", reasons.join("; "))),
        Event::FixAsked { step, reasons, .. } => say(&format!("↻ {step} fixes in place: {}", reasons.join("; "))),
        Event::ChecksUnjudged { step, .. } => say(&format!("… {step}: a check could not judge; continue has them check the same change again")),
        Event::PortsChosen { any: true, opened, .. } => say(&format!("this project's checks reach any local port of this machine from here: opened by you on {}", crate::summary::utc(*opened))),
        Event::PortsChosen { servers: true, opened, freed_ports, .. } => say(&format!(
            "this project's tests may start their own test servers from here: allowed by you on {}{}",
            crate::summary::utc(*opened),
            crate::listing::freed_said(freed_ports)
        )),
        Event::PortsChosen { .. } => say("this project's checks reach only the ports the run reserves and the ones a step declares, from here: closed by you"),
        Event::StepInvalidated { step, because, .. } => say(&format!("↺ {step} runs again: {}", because.join("; "))),
        Event::Waiting { why, .. } => say(&format!("… {why}")),
        Event::RunFinished { outcome, cost } => say(&format!(
            "run finished, {outcome:?}: {} s, {} tokens, {}, {} sent back",
            cost.usage.seconds,
            cost.usage.tokens,
            crate::listing::work(r).said(),
            cost.sent_back
        )),
        _ => {}
    }
}

/// An item of a question as the terminal says it: its id, by which the person answers it, then its label; the label
/// alone when it already starts with the id, as a role's whose id is the item's name ("Go: Go with ..." said once).
pub fn item_line(id: &str, label: &str, between: &str) -> String {
    if label == id || label.strip_prefix(id).is_some_and(|rest| rest.starts_with([':', ' ', '\u{2014}'])) {
        label.to_string()
    } else {
        format!("{id}{between}{label}")
    }
}

#[cfg(test)]
mod git_lists_tests {
    use super::*;

    #[test]
    fn the_agent_s_git_reads_the_person_s_ignore_and_attributes_and_never_a_credential() {
        let base = std::env::temp_dir().join(format!("jaynes-git-lists-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("home/.config/git")).expect("dir");
        let home = std::fs::canonicalize(base.join("home")).expect("home");
        std::fs::write(home.join(".config/git/attributes"), "*.bin binary\n").expect("write");
        std::fs::write(home.join(".config/git/ignore"), "*.log\n").expect("write");
        std::fs::write(home.join(".config/git/credentials"), "https://u:secret@example.invalid\n").expect("write");
        std::fs::write(home.join("my ignore"), "*.tmp\n").expect("write");
        // git's own places, when the person's configuration names none
        let lists = person_git_lists(&home, |_| None);
        assert_eq!(lists.reads, [home.join(".config/git/ignore"), home.join(".config/git/attributes")]);
        assert_eq!(lists.config, format!("[core]\n\texcludesFile = \"{}\"\n\tattributesFile = \"{}\"\n",
            home.join(".config/git/ignore").display(), home.join(".config/git/attributes").display()));
        // the file the person's ~/.gitconfig names, through ~ and with a space; never the credentials, even named
        std::fs::write(home.join(".gitconfig"), "[core]\n\texcludesFile = ~/my ignore\n\tattributesFile = ~/.config/git/credentials\n").expect("write");
        let lists = person_git_lists(&home, |_| None);
        assert_eq!(lists.reads, [home.join("my ignore")]);
        assert_eq!(lists.config, format!("[core]\n\texcludesFile = \"{}\"\n\tattributesFile = \"/dev/null\"\n", home.join("my ignore").display()));
        // nothing there: git looks for none in the hidden home
        std::fs::remove_dir_all(home.join(".config")).expect("gone");
        std::fs::remove_file(home.join(".gitconfig")).expect("gone");
        let lists = person_git_lists(&home, |_| None);
        assert!(lists.reads.is_empty());
        assert_eq!(lists.config, "[core]\n\texcludesFile = \"/dev/null\"\n\tattributesFile = \"/dev/null\"\n");
        let _gone = std::fs::remove_dir_all(&base);
    }
}

/// What a photograph is, written beside it in the copy (`.png.of`): the page's fingerprint, the window, and the theme
/// Jaynes photographs in (jaynes_box's LIGHT_THEME). A mark that differs, one written before the theme was said among
/// them, has the page photographed again.
fn photo_mark(page: &str, photo: Option<crate::protocol::Photo>) -> String {
    match photo {
        Some(p) => format!("{page} {}x{}{} light", p.width, p.height, if p.whole { " whole" } else { "" }),
        None => format!("{page} light"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // the engineer lead, 23/11: a run open before photographs were taken in the light theme kept a dark one of a page
    // that did not change, shown to the very person who saw the problem
    #[test]
    fn asked_a_photograph_s_mark_says_its_theme_and_one_without_it_is_taken_again() {
        let window = Some(crate::protocol::Photo { width: 390, height: 844, whole: true });
        assert_eq!(photo_mark("abc", window), "abc 390x844 whole light");
        assert_eq!(photo_mark("abc", None), "abc light");
        // the marks written before: the same page in the same window, without a theme, is not this photograph
        assert_ne!(photo_mark("abc", window), "abc 390x844 whole");
        assert_ne!(photo_mark("abc", None), "abc");
    }

    // the real-user, 22/11: "A parada 1 ofereceu uma opção só, com o nome repetido: \"Go com biblioteca padrão: Go com
    // biblioteca padrão:\"", and at kickoff "Marcar como visto — Marcar como visto: Ao ler um recado..."
    #[test]
    fn asked_an_item_s_name_is_said_once() {
        let label = "Go com biblioteca padrão: Um único programa em Go";
        assert_eq!(item_line("Go com biblioteca padrão", label, ": "), label);
        assert_eq!(item_line("Marcar como visto", "Marcar como visto: Ao ler um recado", " \u{2014} "), "Marcar como visto: Ao ler um recado");
        assert_eq!(item_line("Go", "Go", ": "), "Go");
        // an id the label does not start with is still said, since the person answers by it
        assert_eq!(item_line("stacks-1", "Go: one program", ": "), "stacks-1: Go: one program");
        assert_eq!(item_line("fix", "The total is right now", " \u{2014} "), "fix \u{2014} The total is right now");
        // a label that only begins with the same letters is another word
        assert_eq!(item_line("Go", "Gopher kit: a server", ": "), "Go: Gopher kit: a server");
    }
    use jaynes_core::manifest::{Executor, Manifest, Reach, SessionKind, StepDecl};
    use jaynes_core::question::By;
    use jaynes_core::risk::Changed;
    use jaynes_core::{AskOutcome, AskTurn};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_merge_shows_everything_the_delivery_brings_not_the_last_step_s_change_alone() {
        let home = std::env::temp_dir().join(format!("jaynes-delivery-diff-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        let dir = home.join("runs").join("r");
        let copy = dir.join("project");
        std::fs::create_dir_all(&copy).expect("copy");
        let git = |args: &[&str]| -> String {
            let out = crate::project::git_command().arg("-C").arg(&copy).args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"]).args(args).output().expect("git");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let commit = |files: &[(&str, &str)], message: &str| -> String {
            for (path, text) in files {
                std::fs::write(copy.join(path), text).expect("write");
            }
            git(&["add", "-A"]);
            git(&["commit", "-q", "-m", message]);
            git(&["rev-parse", "HEAD"])
        };
        git(&["init", "-q"]);
        let base = commit(&[("hero.txt", "one\n"), ("keep.txt", "same\n")], "base");
        std::fs::write(copy.join(".git/jaynes-base"), &base).expect("base");
        // "write" changes hero.txt and adds notes.txt; "fix", working on top of it, changes hero.txt again
        let write = commit(&[("hero.txt", "two\n"), ("notes.txt", "new\n")], "write");
        let fix = commit(&[("hero.txt", "three\n")], "fix");
        std::fs::write(dir.join("project-steps.json"), json!({"write": write, "fix": fix}).to_string()).expect("steps");
        std::fs::write(dir.join("project-parents.json"), json!({"write": base, "fix": write}).to_string()).expect("parents");
        let lands = [("notes.txt".to_string(), "write".to_string()), ("hero.txt".to_string(), "fix".to_string())];
        let all = delivery_diff_of(&home, "r", &lands).expect("the delivery's diff");
        let files: Vec<&str> = all.lines().filter_map(|l| l.strip_prefix("diff --git ")).collect();
        assert_eq!(files, ["a/hero.txt b/hero.txt", "a/notes.txt b/notes.txt"], "{all}");
        // hero.txt from the run's start to how "fix" left it, never "write"'s middle state
        assert!(all.contains("-one") && all.contains("+three") && !all.contains("+two") && !all.contains("-two"), "{all}");
        assert!(all.contains("+new"), "{all}");
        // the last step's change alone, as before, lacks notes.txt
        assert!(!step_diff(&home, "r", "fix").expect("fix's diff").contains("notes.txt"));
        // nothing lands, or no copy: nothing to show
        assert_eq!(delivery_diff_of(&home, "r", &[]), None);
        assert_eq!(delivery_diff_of(&home, "gone", &lands), None);
        let _gone = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_halt_stops_only_the_groups_that_descend_from_the_role_program() {
        use std::io::BufRead;
        use std::os::unix::process::CommandExt;
        let dir = std::env::temp_dir().join(format!("jaynes-box-groups-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(BOX_GROUPS)).expect("dir");
        // the role program: it starts a command in a group of its own, as the box does, and says its number
        let mut role = std::process::Command::new("sh")
            .args(["-c", "perl -e 'use POSIX; POSIX::setpgid(0, 0); sleep 30' </dev/null >/dev/null 2>&1 & echo $!; wait"])
            .stdout(std::process::Stdio::piped()).process_group(0).spawn().expect("role");
        let mut line = String::new();
        std::io::BufReader::new(role.stdout.take().expect("out")).read_line(&mut line).expect("read");
        let boxed: u32 = line.trim().parse().expect("pid");
        std::fs::write(dir.join("role.pid"), role.id().to_string()).expect("write");
        // a group that does not descend from it, started apart, and notes that name no group of the run's
        let mut stranger = std::process::Command::new("sleep").arg("30").process_group(0).spawn().expect("stranger");
        // the shell says the command's number before the command has made its group: wait until it leads one, or
        // a slow machine finds no group of the run's to stop (CI of 027bf9b8)
        let leads = |pid: u32| std::process::Command::new("ps").args(["-o", "pgid=", "-p", &pid.to_string()]).output().is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == pid.to_string());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !leads(boxed) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(leads(boxed), "the role's command made no group of its own");
        for note in [boxed.to_string(), stranger.id().to_string(), "1".into(), "not-a-number".into()] {
            std::fs::write(dir.join(BOX_GROUPS).join(note), "").expect("note");
        }
        assert!(stop_box_groups(&dir), "the role's command in the box was not stopped");
        let alive = |pid: u32| std::process::Command::new("kill").args(["-0", &pid.to_string()]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
        assert!(!alive(boxed) || std::process::Command::new("ps").args(["-o", "stat=", "-p", &boxed.to_string()]).output().is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains('Z')), "the role's command still runs");
        assert_eq!(stranger.try_wait().expect("wait"), None, "a group that does not descend from the role program was stopped");
        assert!(dir.join(BOX_GROUPS).join(stranger.id().to_string()).exists(), "the stranger's note was taken as the run's");
        // what the test started, it stops by its own handles
        let _k = stranger.kill();
        let _w = stranger.wait();
        let _k = role.kill();
        let _w = role.wait();
        let _gone = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_halt_keeps_the_mark_of_the_first() {
        // the first halt stopped a check's command and marked the call interrupted; a second, before the run's process
        // took the first, stops nothing and must not take the mark away, or the stopped check would count as failed
        let dir = std::env::temp_dir().join(format!("jaynes-box-mark-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join(HALTED_BOX), "").expect("mark");
        stop_agent_for(&dir, &Input::Halt { by: By::Person });
        assert!(dir.join(HALTED_BOX).exists(), "a second halt took away the first's mark");
        // a halt that stops nothing, with no mark before it, leaves none
        let _gone = std::fs::remove_file(dir.join(HALTED_BOX));
        stop_agent_for(&dir, &Input::Halt { by: By::Person });
        assert!(!dir.join(HALTED_BOX).exists(), "a halt that stopped nothing left a mark");
        let _gone = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_halt_that_stops_nothing_keeps_the_mark_the_first_left() {
        let dir = std::env::temp_dir().join(format!("jaynes-halted-mark-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let halt = Input::Halt { by: By::Person };
        // no agent runs: a halt marks nothing
        stop_agent_for(&dir, &halt);
        assert!(!dir.join(HALTED_AGENT).exists(), "a halt that stopped nothing left a mark");
        // the first halt stopped the agent and left its mark; a second, before the run's process took it, finds none
        std::fs::write(dir.join(HALTED_AGENT), "").expect("mark");
        stop_agent_for(&dir, &halt);
        assert!(dir.join(HALTED_AGENT).exists(), "the second halt took away the first one's mark: its turn would count as failed");
        let _gone = std::fs::remove_dir_all(&dir);
    }

    /// A record holding one run that an older engine started.
    fn old_run(home: &Path) -> Run {
        let step = StepDecl {
            name: "change".into(), needs: vec![], executor: Executor::Agent, session: SessionKind::Fresh, reach: Reach::default(),
            effect: Default::default(), checks: vec![], stop: None, from_risk: Default::default(), attempts: 3, fixes: 2, hidden: false, idempotent: false, about: String::new(),
            model: Default::default(),
            turns: None, delivered: true,
        };
        let manifest = Manifest { role: "r".into(), version: "1".into(), protocol: 1, steps: vec![step], target: Default::default(), delivery: Default::default() };
        let start = Input::Start { run: "old".into(), request: "x".into(), manifest, role_fingerprint: jaynes_core::Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        let out = jaynes_core::decide(None, &start).expect("start");
        let events: Vec<Event> = out
            .events
            .into_iter()
            .map(|e| {
                let mut v = serde_json::to_value(&e).expect("json");
                if v["type"] == "RunStarted" {
                    v["engine"] = "0.0.0+old".into();
                }
                serde_json::from_value(v).expect("event")
            })
            .collect();
        let mut store = Store::open(&home.join("record.sqlite3")).expect("store");
        store.append("old", &events).expect("append");
        std::fs::create_dir_all(home.join("runs/old")).expect("dir");
        store.state("old").expect("state").expect("run")
    }

    #[test]
    fn a_run_is_left_running_only_when_the_process_noted_as_its_driver_is_gone() {
        let home = std::env::temp_dir().join(format!("jaynes-left-running-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        let run = old_run(&home);
        assert!(matches!(run.phase, jaynes_core::run::Phase::Running), "{:?}", run.phase);
        // no note of its driver: not judged, as a run from before the note
        assert!(!left_running(&home, &run));
        // a driver that lives
        std::fs::write(home.join("runs/old").join(DRIVER_PID), std::process::id().to_string()).expect("write");
        assert!(!left_running(&home, &run));
        // one that is gone
        let mut gone = std::process::Command::new("true").spawn().expect("true");
        let pid = gone.id();
        let _ended = gone.wait();
        std::fs::write(home.join("runs/old").join(DRIVER_PID), pid.to_string()).expect("write");
        assert!(left_running(&home, &run));
        // a halted run is not left running: its halt says what it is
        let mut halted = run.clone();
        halted.halted = true;
        assert!(!left_running(&home, &halted));
        let _gone = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_run_works_on_the_target_its_setup_names_and_one_from_before_targets_on_a_repository() {
        let home = std::env::temp_dir().join(format!("jaynes-target-setup-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("home");
        let store = Store::open(&home.join("record.sqlite3")).expect("store");
        let place = |target: Option<&str>| {
            let mut s = json!({"role_given": "r", "fingerprint": "f", "agent": "claude", "project": "/somewhere", "branch": "jaynes/x"});
            if let Some(t) = target {
                s["target"] = t.into();
            }
            s
        };
        store.put_setup("before", &place(None)).expect("setup");
        store.put_setup("named", &place(Some("repository"))).expect("setup");
        store.put_setup("unknown", &place(Some("a-browser-session"))).expect("setup");
        for run in ["before", "named"] {
            let setup = Setup::load(&store, &home, run).expect("loads");
            let copy = setup.copy.expect("a copy");
            assert_eq!((copy.source(), copy.dir()), (Path::new("/somewhere"), home.join("runs").join(run).join("project").as_path()));
        }
        let refused = Setup::load(&store, &home, "unknown").err().expect("refused");
        assert!(refused.contains("a-browser-session"), "{refused}");
    }

    #[test]
    fn a_file_an_agent_left_is_read_only_as_a_plain_file_inside_its_folder() {
        let base = std::env::temp_dir().join(format!("jaynes-read-inside-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let root = base.join("step");
        std::fs::create_dir_all(root.join("sub")).expect("dir");
        std::fs::write(base.join("outside.txt"), "outside").expect("write");
        std::fs::write(root.join("plain.txt"), "plain").expect("write");
        std::os::unix::fs::symlink(base.join("outside.txt"), root.join("link.txt")).expect("link");
        std::os::unix::fs::symlink(&base, root.join("up")).expect("link");
        assert_eq!(read_inside(&root, &root.join("plain.txt")), Ok("plain".to_string()));
        assert_eq!(read_inside(&root, &root.join("none.txt")), Err(None));
        assert!(read_inside(&root, &root.join("link.txt")).is_err_and(|e| e.is_some()));
        assert!(read_inside(&root, &root.join("up/outside.txt")).is_err_and(|e| e.is_some_and(|w| w.contains("out of the step's folder"))));
        assert!(read_inside(&root, &root.join("sub")).is_err_and(|e| e.is_some()));
        let _gone = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_hard_link_an_agent_left_is_not_read() {
        let base = std::env::temp_dir().join(format!("jaynes-hard-link-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let root = base.join("step");
        std::fs::create_dir_all(&root).expect("dir");
        std::fs::write(base.join("secret.json"), "{}").expect("write");
        std::fs::hard_link(base.join("secret.json"), root.join("output.json")).expect("link");
        assert!(read_inside(&root, &root.join("output.json")).is_err_and(|e| e.is_some_and(|w| w.contains("link"))));
        let _gone = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn jaynes_writes_in_a_copy_only_where_no_link_leads_out_of_it() {
        let base = std::env::temp_dir().join(format!("jaynes-write-inside-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let root = base.join("copy");
        let outside = base.join("outside");
        std::fs::create_dir_all(root.join(".jaynes")).expect("dir");
        std::fs::create_dir_all(&outside).expect("dir");
        // a folder on the way that is a link out
        std::os::unix::fs::symlink(&outside, root.join(".jaynes/pictures")).expect("link");
        assert!(write_inside(&root, &root.join(".jaynes/pictures/a.png"), b"x").is_err());
        assert!(!outside.join("a.png").exists(), "Jaynes wrote out of the copy");
        // a link deeper in the way
        std::fs::remove_file(root.join(".jaynes/pictures")).expect("rm");
        std::fs::create_dir_all(root.join(".jaynes/pictures")).expect("dir");
        std::os::unix::fs::symlink(&outside, root.join(".jaynes/pictures/deep")).expect("link");
        assert!(write_inside(&root, &root.join(".jaynes/pictures/deep/b.png"), b"x").is_err());
        assert!(!outside.join("b.png").exists());
        // the file itself a link: replaced, and what it pointed at untouched
        std::fs::write(outside.join("c.txt"), "theirs").expect("write");
        std::os::unix::fs::symlink(outside.join("c.txt"), root.join(".jaynes/pictures/c.png")).expect("link");
        write_inside(&root, &root.join(".jaynes/pictures/c.png"), b"ours").expect("written");
        assert_eq!(std::fs::read_to_string(outside.join("c.txt")).expect("read"), "theirs");
        assert_eq!(std::fs::read(root.join(".jaynes/pictures/c.png")).expect("read"), b"ours");
        // folders made on the way, inside
        write_inside(&root, &root.join(".jaynes/pictures/new/d.png"), b"d").expect("written");
        assert!(root.join(".jaynes/pictures/new/d.png").is_file());
        let _gone = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_question_left_unanswered_is_reminded_once_a_day() {
        let home = std::env::temp_dir().join(format!("jaynes-remind-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("home");
        // a run of one person's step, which waits on them at once
        let step = StepDecl {
            name: "look".into(), needs: vec![], executor: Executor::Person, session: SessionKind::Fresh, reach: Reach::default(),
            effect: Default::default(), checks: vec![], stop: None, from_risk: Default::default(), attempts: 3, fixes: 2, hidden: false, idempotent: false, about: String::new(),
            model: Default::default(),
            turns: None, delivered: true,
        };
        let mut look = step.clone();
        look.name = "look".into();
        let mut first = step;
        first.name = "make".into();
        first.executor = Executor::Role;
        look.needs = vec!["make".into()];
        look.stop = Some(jaynes_core::manifest::StopDecl { settled: None, holds_unless: None, vouched_by: None,
            verdicts: vec![jaynes_core::question::VerdictKind::Keep], holds: true, from_risk: Default::default(), release: false, budget: None,
            audit_rate: jaynes_core::manifest::Rate(0.1), about: "Look?".into(), choose: false, redo: None, rounds: None, answers: Default::default(), note: String::new(),
            offers: Default::default(),
        });
        let manifest = Manifest { role: "r".into(), version: "1".into(), protocol: 1, steps: vec![first, look], target: Default::default(), delivery: Default::default() };
        let mut store = Store::open(&home.join("record.sqlite3")).expect("store");
        let start = Input::Start { run: "w".into(), request: "x".into(), manifest, role_fingerprint: jaynes_core::Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        store.decide("w", &start).expect("start");
        let run = store.state("w").expect("state").expect("run");
        let probe = Input::ProbeEnded { step: "make".into(), ok: true, reason: String::new() };
        store.decide("w", &probe).expect("probe");
        let attempt = store.state("w").expect("state").expect("run").steps[0].attempt;
        let done = Input::AttemptEnded { step: "make".into(), attempt, fix: 0, outcome: jaynes_core::AttemptOutcome::Delivered { output: jaynes_core::Digest("o".into()), items: vec![], changed: vec![], usage: Default::default(), session: None, tree: None, reply: None } };
        store.decide("w", &done).expect("done");
        let _ = run;
        let waiting = store.state("w").expect("state").expect("run");
        assert!(!waiting.told.is_empty(), "the run waits on the person: {:?}", waiting.phase);
        let now = crate::store::now();
        assert!(reminders_due(&home, &store, now).expect("due").is_empty(), "nothing is due as soon as it is told");
        let due = reminders_due(&home, &store, now + REMIND_EVERY + 1).expect("due");
        assert_eq!(due.len(), 1, "{due:?}");
        assert_eq!((due[0].run.as_str(), due[0].step.as_str()), ("w", "look"));
        mark_reminded(&home, "w", &due[0].question, now + REMIND_EVERY + 1).expect("marked");
        assert!(reminders_due(&home, &store, now + REMIND_EVERY + 2).expect("due").is_empty(), "reminded once, not again at once");
        assert_eq!(reminders_due(&home, &store, now + 2 * REMIND_EVERY + 2).expect("due").len(), 1, "and again a day later");
        let _gone = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_run_s_agent_keeps_the_model_it_started_with() {
        assert!(agent_named("claude:claude-sonnet-5-5").name().starts_with("claude claude-sonnet-5-5 "));
        assert!(agent_named("command:true").name().starts_with("command:"));
        for known in ["claude", "claude:claude-opus-5-5", "command:true"] {
            assert!(known_agent(known).is_ok(), "{known}");
        }
        for unknown in ["gemini:x", "gpt:x", "clade:x", "Claude", "command:", "command: ", ""] {
            assert!(known_agent(unknown).is_err_and(|e| e.contains("claude:MODEL") && e.contains("command:CMD")), "{unknown}");
        }
        assert!(known_agent("codex:gpt-5.5").is_ok() && known_agent("codex:").is_err());
        assert!(agent_named("codex:gpt-5.5").name().starts_with("codex gpt-5.5"));
    }

    #[test]
    fn a_run_is_decided_only_by_the_engine_it_started_on() {
        let home = std::env::temp_dir().join(format!("jaynes-engines-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        let run = old_run(&home);
        assert_eq!(run.engine, "0.0.0+old");
        // this engine records nothing for it
        let mut store = Store::open(&home.join("record.sqlite3")).expect("store");
        let refused = store.decide("old", &Input::Halt { by: By::Person }).expect_err("refused");
        assert!(refused.contains("0.0.0+old"), "{refused}");
        // with the older engine gone from the machine, an input is refused and says so
        let gone = submit(&home, Path::new("/bin/false"), &run, Input::Halt { by: By::Person }).expect_err("refused");
        assert!(gone.contains("0.0.0+old"), "{gone}");
        // with it kept, the input goes to that engine
        let kept = home.join("engines/0.0.0+old");
        std::fs::create_dir_all(&kept).expect("dir");
        let called = home.join("called");
        std::fs::write(kept.join("jaynes"), format!("#!/bin/sh\necho \"$@\" > {c}\ncat >> {c}\necho '{{\"ok\":true,\"recorded\":\"now\"}}'\n", c = called.display())).expect("stub");
        std::fs::set_permissions(kept.join("jaynes"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert_eq!(submit(&home, Path::new("/bin/false"), &run, Input::Halt { by: By::Person }), Ok("now"));
        let said = std::fs::read_to_string(&called).expect("called");
        assert!(said.starts_with("act old") && said.contains("\"halt\""), "{said}");
        // and a run started now keeps this engine's program for later
        keep_engine(&home).expect("kept");
        assert!(home.join("engines").join(jaynes_core::ENGINE).join("jaynes").is_file());
        assert!(jaynes_core::ENGINE.contains('+'), "the version names the core's code: {}", jaynes_core::ENGINE);
    }

    /// What a stand-in agent was given, and what it does: no model is ever reached.
    #[derive(Default)]
    struct Seen {
        prompt: String,
        session: Option<String>,
        reach: Option<Reach>,
        permit: bool,
        on_session: bool,
        cwd: PathBuf,
        transcript: Option<PathBuf>,
        previous: Option<String>,
        waits: Option<PathBuf>,
    }

    struct StandIn {
        seen: std::sync::Arc<std::sync::Mutex<Seen>>,
        /// What it writes in the exchange folder's reply.md, if anything.
        reply: Option<String>,
        /// What its result says last.
        said: String,
        /// A file of the project it writes, which a question may never do.
        writes: Option<&'static str>,
    }

    impl Agent for StandIn {
        fn name(&self) -> String {
            "stand-in".into()
        }
        fn probe(&self, _cwd: &Path) -> Result<(), String> {
            Ok(())
        }
        fn turn(&self, spec: agent::Spec<'_>) -> agent::Turn {
            let exchange = spec.cwd.join(EXCHANGE);
            {
                let mut seen = self.seen.lock().expect("seen");
                *seen = Seen {
                    prompt: spec.prompt.to_string(),
                    session: spec.session.map(str::to_string),
                    reach: Some(spec.reach.clone()),
                    permit: spec.permit.is_some(),
                    on_session: spec.on_session.is_some(),
                    cwd: spec.cwd.to_path_buf(),
                    transcript: spec.transcript.clone(),
                    previous: std::fs::read_to_string(exchange.join(ASKED_ABOUT)).ok(),
                    waits: spec.waits.as_ref().map(|w| w.0.clone()),
                };
            }
            if let Some(r) = &self.reply {
                std::fs::write(exchange.join("reply.md"), r).expect("reply");
            }
            if let Some(f) = self.writes {
                std::fs::write(spec.cwd.join(f), "changed while answering").expect("write");
            }
            let usage = jaynes_core::event::Usage { seconds: 3, tokens: 1200, cached: 9000, ..Default::default() };
            agent::Turn { ended: Ended::Ran { usage, session: spec.session.map(str::to_string), model: "stand-in".into(), said: self.said.clone() }, applied: json!({}) }
        }
    }

    /// A copy that only knows what changed in its folder since it was made: what a question checks.
    struct Folder {
        dir: PathBuf,
        was: std::collections::BTreeMap<String, Vec<u8>>,
    }

    impl Folder {
        fn new(dir: &Path) -> Folder {
            Folder { dir: dir.to_path_buf(), was: Folder::files(dir) }
        }
        fn files(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
            std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.file_name() != EXCHANGE && e.path().is_file())
                .map(|e| (e.file_name().to_string_lossy().to_string(), std::fs::read(e.path()).unwrap_or_default()))
                .collect()
        }
    }

    impl Workspace for Folder {
        fn dir(&self) -> &Path {
            &self.dir
        }
        fn source(&self) -> &Path {
            &self.dir
        }
        fn measure(&self, _write: &[String]) -> Result<(Vec<Changed>, String), Result<Vec<String>, String>> {
            unreachable!("a question measures nothing")
        }
        fn compose(&self, _changes: &[(String, String)]) -> Result<String, String> {
            unreachable!()
        }
        fn keep(&self, _state: Option<&str>, _parent: &str, _message: &str) -> Result<Option<String>, String> {
            unreachable!()
        }
        fn state_of(&self, _id: &str) -> Result<String, String> {
            unreachable!()
        }
        fn base(&self) -> Result<String, String> {
            unreachable!()
        }
        fn head(&self) -> Result<String, String> {
            unreachable!()
        }
        fn discard(&self) -> Result<(), String> {
            unreachable!()
        }
        fn deliver(&self, _changes: &[(String, String)], _shown: &dyn Fn(&str) -> bool) -> Result<(String, String), String> {
            unreachable!()
        }
        fn land(&self, _head: &str, _run: &str, _note: &str) -> Result<Option<String>, String> {
            unreachable!()
        }
        fn undo(&self, _landed: &[String], _run: &str) -> Result<String, String> {
            unreachable!()
        }
        fn reads(&self) -> Vec<PathBuf> {
            Vec::new()
        }
        fn lay_out(&self, _id: &str) -> Result<crate::project::LaidOut, String> {
            unreachable!()
        }
        fn left_out(&self) -> Vec<String> {
            Vec::new()
        }
        fn drifted(&self) -> Result<Vec<String>, String> {
            let now = Folder::files(&self.dir);
            let mut names: Vec<String> = now.iter().filter(|(k, v)| self.was.get(*k) != Some(v)).map(|(k, _)| k.clone()).collect();
            names.extend(self.was.keys().filter(|k| !now.contains_key(*k)).cloned());
            Ok(names)
        }
        fn put_back(&self) -> Result<(), String> {
            for k in Folder::files(&self.dir).keys().filter(|k| !self.was.contains_key(*k)) {
                std::fs::remove_file(self.dir.join(k)).map_err(|e| e.to_string())?;
            }
            for (k, v) in &self.was {
                std::fs::write(self.dir.join(k), v).map_err(|e| e.to_string())?;
            }
            Ok(())
        }
    }

    fn setup_waits(home: &Path) -> Option<PathBuf> {
        Some(crate::port_wait::folder(&home.join("runs/r")))
    }

    /// A run whose agent step "stack" stops to ask the person about what it made, driven with `agent` in a copy that
    /// holds main.py; and the output the stop shows, in the record.
    fn asking(name: &str, agent: StandIn) -> (PathBuf, Store, Setup, Run, Digest) {
        let home = std::env::temp_dir().join(format!("jaynes-ask-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        let copy = home.join("runs/r/project");
        std::fs::create_dir_all(&copy).expect("copy");
        std::fs::write(copy.join("main.py"), "print('hi')\n").expect("file");
        let store = Store::open(&home.join("record.sqlite3")).expect("store");
        let output = store.put(&json!({"output": {"options": [{"id": "py", "label": "Python + FastAPI"}]}})).expect("put");
        let mut stack = StepDecl {
            name: "stack".into(),
            needs: Vec::new(),
            executor: Executor::Agent,
            session: SessionKind::Fresh,
            reach: Reach { project: jaynes_core::manifest::ProjectAccess::Write, write: vec!["**".into()], ..Reach::default() },
            effect: jaynes_core::manifest::Effect::None,
            checks: Vec::new(),
            stop: None,
            from_risk: jaynes_core::risk::Risk::Low,
            attempts: 3,
            fixes: 2,
            hidden: false,
            idempotent: false,
            about: String::new(),
            model: Default::default(),
            turns: None, delivered: true,
        };
        stack.stop = Some(jaynes_core::manifest::StopDecl { settled: None, holds_unless: None, vouched_by: None,
            verdicts: vec![jaynes_core::question::VerdictKind::Keep, jaynes_core::question::VerdictKind::Change, jaynes_core::question::VerdictKind::Drop],
            holds: true,
            from_risk: jaynes_core::risk::Risk::Low,
            release: false,
            budget: None,
            audit_rate: jaynes_core::manifest::Rate(0.1),
            about: "Which stack?".into(),
            choose: true,
            redo: None,
            rounds: None,
            answers: Default::default(), note: String::new(),
            offers: Default::default(),
        });
        let manifest = Manifest { role: "greenfield".into(), version: "1".into(), protocol: 1, steps: vec![stack], target: Default::default(), delivery: Default::default() };
        let start = jaynes_core::decide(None, &Input::Start {
            run: "r".into(),
            request: "a habit tracker".into(),
            manifest,
            role_fingerprint: Digest("f".into()),
            agent: "claude:m1".into(),
            raise: None,
            attention: Default::default(),
            task: None,
            arm: None,
            never_merge: false,
            starter: None,
        })
        .expect("start");
        let run = Run::fold(&start.events).expect("run");
        let dir = home.join("runs/r");
        let setup = Setup {
            run: "r".into(),
            dir: dir.clone(),
            role: RoleProgram { waits: setup_waits(&home).map(|w| (w, "tracker · a habit tracker".into())), ..RoleProgram::new(&dir.join("role"), &dir.join("library")) },
            copy: Some(Box::new(Folder::new(&copy))),
            agent: Box::new(agent),
            any_port: false,
            servers: false,
            freed_ports: vec![],
            home: home.clone(),
            project: None,
        };
        (home, store, setup, run, output)
    }

    fn ask(output: &Digest, session: Option<&str>, text: &str) -> AskTurn {
        AskTurn {
            ask: "q1-stack-a1".into(),
            question: "q1-stack".into(),
            step: "stack".into(),
            round: 0,
            output: output.clone(),
            item: Some("py".into()),
            label: "Python + FastAPI".into(),
            text: text.into(),
            session: session.map(str::to_string),
            reach: Reach { project: jaynes_core::manifest::ProjectAccess::Read, ..Reach::default() },
            deadline_seconds: 600,
            tokens_left: 500_000,
        }
    }

    fn stand_in(reply: Option<&str>, said: &str, writes: Option<&'static str>) -> (StandIn, std::sync::Arc<std::sync::Mutex<Seen>>) {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Seen::default()));
        (StandIn { seen: seen.clone(), reply: reply.map(str::to_string), said: said.into(), writes }, seen)
    }

    #[test]
    fn asked_question_is_answered_without_redoing_the_step() {
        // Alan, 21/11, at the stack's one option: "e se fosse Go?"
        let (agent, seen) = stand_in(Some("Go would make one binary and use less memory; FastAPI is quicker to write.\n\nI changed nothing."), "", None);
        let (home, mut store, setup, run, output) = asking("answered", agent);
        let mut d = Driver::take(&mut store, setup).expect("driver");
        let outcome = d.answer_ask(&run, ask(&output, Some("s1"), "e se fosse Go?"));
        let AskOutcome::Answered { text, usage, session } = outcome else { panic!("{outcome:?}") };
        assert_eq!(text, "Go would make one binary and use less memory; FastAPI is quicker to write.\n\nI changed nothing.");
        assert_eq!((usage.tokens, session.as_deref()), (1200, Some("s1")));
        let seen = seen.lock().expect("seen");
        assert_eq!(seen.session.as_deref(), Some("s1"), "in the session of the step that made the option");
        let reach = seen.reach.clone().expect("reach");
        assert!(reach.project == jaynes_core::manifest::ProjectAccess::Read && reach.write.is_empty(), "only reads: {reach:?}");
        assert!(!seen.permit, "nothing it asks beyond its settings reaches the person: it is refused");
        assert!(!seen.on_session, "a question's session is never the one a cut-off attempt goes on in");
        // what its commands' box waits for a port another run holds is not the agent's time (T10, 23/11)
        assert_eq!(seen.waits, setup_waits(&home), "the question's turn knows the run's waits");
        assert!(seen.prompt.contains("e se fosse Go?") && seen.prompt.contains("“Python + FastAPI”"), "{}", seen.prompt);
        assert!(seen.prompt.contains("Change nothing") && seen.prompt.contains(".jaynes/reply.md"), "{}", seen.prompt);
        assert!(seen.previous.as_deref().is_some_and(|p| p.contains("Python + FastAPI")), "what the stop shows: {:?}", seen.previous);
        assert_eq!(seen.transcript.as_deref(), Some(home.join("runs/r/steps/stack/ask-q1-stack-a1.jsonl").as_path()));
        assert!(!seen.cwd.join(".jaynes/reply.md").exists(), "the reply file is taken, and none lingers for the next turn");
        assert_eq!(std::fs::read_to_string(seen.cwd.join("main.py")).expect("file"), "print('hi')\n");
    }

    #[test]
    fn a_question_whose_agent_changes_a_file_fails_and_the_file_is_put_back() {
        let (agent, _) = stand_in(Some("Done, I switched it to Go."), "", Some("main.py"));
        let (home, mut store, setup, run, output) = asking("changed", agent);
        let mut d = Driver::take(&mut store, setup).expect("driver");
        let outcome = d.answer_ask(&run, ask(&output, Some("s1"), "e se fosse Go?"));
        let AskOutcome::Failed { reason, usage } = outcome else { panic!("{outcome:?}") };
        assert!(reason.contains("main.py") && reason.contains("put back"), "{reason}");
        assert_eq!(usage.tokens, 1200, "what it cost is counted all the same");
        assert_eq!(std::fs::read_to_string(home.join("runs/r/project/main.py")).expect("file"), "print('hi')\n");
    }

    #[test]
    fn a_question_answered_without_a_reply_file_takes_the_agent_s_last_words_and_one_without_any_fails() {
        let (agent, _) = stand_in(None, "Go: one binary, less memory.", None);
        let (_home, mut store, setup, run, output) = asking("said", agent);
        let mut d = Driver::take(&mut store, setup).expect("driver");
        let outcome = d.answer_ask(&run, ask(&output, None, "e se fosse Go?"));
        assert!(matches!(&outcome, AskOutcome::Answered { text, .. } if text == "Go: one binary, less memory."), "{outcome:?}");
        let (agent, seen) = stand_in(None, "   ", None);
        let (_home, mut store, setup, run, output) = asking("nothing", agent);
        let mut d = Driver::take(&mut store, setup).expect("driver");
        let outcome = d.answer_ask(&run, ask(&output, None, "e se fosse Go?"));
        assert!(matches!(&outcome, AskOutcome::Failed { reason, .. } if reason.contains("answered nothing")), "{outcome:?}");
        assert!(seen.lock().expect("seen").prompt.contains(".jaynes/asked-about.json"), "a new session is told where the option is");
    }

    #[test]
    fn a_reply_reads_as_words_for_the_person_cleaned_and_cut() {
        assert_eq!(reply_text("  Changed the API.\r\n\r\n\r\n\r\nKept the rest.\u{1b}[31m\u{202e}  "), Some("Changed the API.\n\nKept the rest.[31m".to_string()));
        assert_eq!(reply_text("one\ttab"), Some("one tab".to_string()));
        assert_eq!(reply_text(" \n\u{200b}\n "), None);
        let long = "é".repeat(REPLY_MOST + 10);
        let cut = reply_text(&long).expect("cut");
        assert_eq!(cut.chars().count(), REPLY_MOST);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn a_round_the_person_reads_asks_the_agent_for_its_reply_and_one_nobody_reads_does_not() {
        let turn = |notes: Vec<String>| AgentTurn {
            step: "stack".into(),
            round: 1,
            attempt: 2,
            fix: 0,
            session: None,
            reach: Reach::default(),
            given: Vec::new(),
            previous: None,
            notes,
            context: Vec::new(),
            failure: Vec::new(),
            checks: Vec::new(),
            locked: Vec::new(),
            dropped: Vec::new(),
            keep: None,
            review: None,
            earlier: Vec::new(),
            back: Vec::new(),
            asked: Vec::new(),
            deadline_seconds: 60,
            tokens_left: 1000,
        };
        let read = compose("r", "make it", &json!({}), &turn(vec!["use htmx".into()]), &[], true);
        assert!(read.contains("# Your reply to the person") && read.contains(".jaynes/reply.md"), "{read}");
        assert!(read.find("# Your reply").expect("reply") < read.find("# Your output").expect("output"));
        let unread = compose("r", "make it", &json!({}), &turn(Vec::new()), &[], false);
        assert!(!unread.contains("reply.md"), "{unread}");
        // what Jaynes's box refuses is said, never got around: the agent rewrote the person's tests twice to pass
        // without a port (the real-user's runs of 22/11); the words agreed with the ai-engineer
        assert!(unread.contains(
            "A command that fails with \"Operation not permitted\", \"Permission denied\" or EPERM was most likely refused by \
             Jaynes's box, not by the project: never change the project's tests or code to get around it (another port, a test \
             removed, skipped or rewritten to need less); write to .jaynes/impossible.txt the command and the error it printed instead."
        ), "{unread}");
        // and named as the box's, wherever the agent says it (the tech-lead, 0.2.5: an agent called it "this environment's
        // protection" and the person could not tell who refused)
        assert!(unread.contains(
            "Wherever you tell of it, say \"Jaynes's box refused\" and what it refused, then the error as the command printed it, \
             never \"this environment's protection\" nor a sandbox of your own."
        ), "{unread}");
    }

    #[test]
    fn what_the_person_asked_at_the_stop_reaches_the_next_round_with_its_answer() {
        let mut turn = AgentTurn {
            step: "stack".into(),
            round: 2,
            attempt: 1,
            fix: 0,
            session: None,
            reach: Reach::default(),
            given: Vec::new(),
            previous: None,
            notes: vec!["keep it short".into()],
            context: Vec::new(),
            failure: Vec::new(),
            checks: Vec::new(),
            locked: Vec::new(),
            dropped: Vec::new(),
            keep: None,
            review: None,
            earlier: Vec::new(),
            back: Vec::new(),
            asked: Vec::new(),
            deadline_seconds: 60,
            tokens_left: 1000,
        };
        assert!(!compose("r", "make it", &json!({}), &turn, &[], true).contains("# What the person asked at the stop"));
        turn.asked = vec![
            jaynes_core::io::Asked { text: "e se fosse Go?".into(), answer: Some("Go would be faster to start.".into()) },
            jaynes_core::io::Asked { text: "and the tests?".into(), answer: None },
        ];
        let p = compose("r", "make it", &json!({}), &turn, &[], true);
        let at = p.find("# What the person asked at the stop, and what you answered").expect(&p);
        let section = &p[at..];
        assert!(section.contains("e se fosse Go?") && section.contains("Go would be faster to start."), "{p}");
        assert!(section.contains("and the tests?") && section.contains("answer it in your reply"), "an unanswered question is answered now: {p}");
        assert!(at < p.find("# Your reply to the person").expect("reply"), "{p}");
    }

    #[test]
    fn a_turn_s_exchange_keeps_no_reply_from_the_one_before() {
        let dir = std::env::temp_dir().join(format!("jaynes-reply-left-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("reply.md"), "from the round before").expect("old");
        write_exchange(&dir, &[], None).expect("written");
        assert!(!dir.join("reply.md").exists());
    }
}
