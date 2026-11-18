//! The agent programs, behind one narrow contract: start a turn in a folder with a prompt, maybe in an earlier
//! session, within a deadline; report the outcome, the time, the tokens and the session.
//!
//! Jaynes does not confine a step. It sets the agent program's own guards for the step from what the step may
//! reach, and records the settings it applied (`Turn::applied`).

use jaynes_core::event::Usage;
use jaynes_core::manifest::{Network, ProjectAccess, Reach};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How to start a server: its command line and its environment.
pub type Server = (Vec<String>, Vec<(String, String)>);

pub struct Spec<'a> {
    pub cwd: &'a Path,
    pub prompt: &'a str,
    pub session: Option<&'a str>,
    pub reach: &'a Reach,
    pub deadline: Duration,
    /// Folders the step's commands must read outside its folder, such as linked dependencies.
    pub reads: &'a [PathBuf],
    pub env: Vec<(String, String)>,
    /// Where to keep what the agent program printed, for the person and for diagnosis.
    pub transcript: Option<PathBuf>,
    /// Where to note the agent program's process group while it runs, so a process that takes the run after a
    /// crash can stop what was left running.
    pub pidfile: Option<PathBuf>,
    /// How to start Jaynes's leave server for this turn, so what the agent asks beyond its settings comes to
    /// Jaynes instead of being refused.
    pub permit: Option<Server>,
    /// A file that exists while the turn waits for the person to answer a leave: that time is not the agent's, so
    /// it counts neither towards the deadline nor in the turn's time.
    pub paused: Option<PathBuf>,
    /// Told the session as soon as the agent program names it, so a turn cut off from outside can go on in it, with
    /// what the program says it runs with.
    pub on_session: OnSession,
}

/// The session a line of the agent program's output names as its turn starts (`{"type":"system","session_id":..}`),
/// and what it says it runs with: its model, its permission mode, its tools.
fn session_of(line: &str) -> Option<(String, jaynes_core::event::Confirmed)> {
    let v: Value = serde_json::from_str(line).ok()?;
    // Codex names its thread as its turn starts, and nothing it runs with
    if v["type"] == "thread.started" {
        return Some((v["thread_id"].as_str()?.to_string(), Default::default()));
    }
    if v["type"] != "system" {
        return None;
    }
    let session = v["session_id"].as_str()?.to_string();
    let text = |k: &str| v[k].as_str().unwrap_or("").to_string();
    let tools = v["tools"].as_array().map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_string)).collect()).unwrap_or_default();
    Some((session, jaynes_core::event::Confirmed { model: text("model"), permission_mode: text("permissionMode"), tools }))
}

#[derive(Debug)]
pub enum Ended {
    Ran { usage: Usage, session: Option<String>, #[allow(dead_code)] model: String },
    /// The agent's fault, with what it cost.
    Failed { reason: String, usage: Usage },
    /// Jaynes's or the machine's.
    Fault { reason: String },
    /// From outside: a quota.
    Interrupted { reason: String },
}

pub struct Turn {
    pub ended: Ended,
    /// The guards Jaynes set for the turn, as the agent program read them.
    pub applied: Value,
}

pub trait Agent {
    fn name(&self) -> String;
    /// Whether the program is there and its guards can be set, before any attempt is spent.
    fn probe(&self, cwd: &Path) -> Result<(), String>;
    fn turn(&self, spec: Spec<'_>) -> Turn;
}

/// Run a program with `input` on its standard input, killed with its whole group at the deadline. The time
/// `paused` exists is not counted: neither towards the deadline nor in the time returned.
pub type OnSession = Option<Box<dyn FnMut(String, jaynes_core::event::Confirmed) + Send>>;

/// What a watched program did.
#[derive(Debug)]
pub struct Watched {
    pub code: Option<i32>,
    pub out: String,
    pub err: String,
    pub took: Duration,
    pub timed_out: bool,
    /// Stopped because it was silent too long while no tool of its was at work: its connection to the model is gone.
    pub stalled: bool,
}

/// How long an agent may be silent while it waits for its model: a model at work sends its text as it comes, so
/// silence this long means the connection is gone. A tool at work is let be; its limits are the agent program's.
pub const SILENCE: Duration = Duration::from_secs(120);
/// How many times a silent connection is taken up again in the same session within one attempt.
const STALLS: u32 = 2;
/// What the agent is told when its silent connection is taken up again.
const RESUME: &str = "Your connection to the model dropped in the middle of the work. Continue the task where you left off; nothing you did is lost.";

/// A turn's transcript, emptied as the turn starts: the program's output is added to it as it comes, and it is
/// written whole when the turn ends.
fn live(transcript: Option<&Path>) -> Option<&Path> {
    if let Some(t) = transcript {
        let _emptied = std::fs::write(t, "");
    }
    transcript
}

/// `live`: a file the program's output is added to line by line as it comes, so the person can watch the agent work.
#[allow(clippy::too_many_arguments)]
fn run(mut cmd: Command, input: &str, deadline: Duration, idle: Option<Duration>, pidfile: Option<&Path>, paused: Option<&Path>, on_session: OnSession, live: Option<&Path>) -> Result<Watched, String> {
    use std::os::unix::process::CommandExt;
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    let started = Instant::now();
    // a mark left by a stop asked for an earlier turn is not this turn's
    if let Some(f) = pidfile {
        let _old = std::fs::remove_file(stop_mark(f));
    }
    let mut child = cmd.spawn().map_err(|e| format!("the agent program did not start: {e}"))?;
    let pid = child.id();
    if let Some(f) = pidfile {
        note_process(f, pid).map_err(|e| format!("Jaynes could not note the agent's process: {e}"))?;
    }
    let result = watch(&mut child, pid, input, deadline, idle, started, paused, pidfile, on_session, live);
    if let Some(f) = pidfile {
        let _gone = std::fs::remove_file(f);
        let _gone = std::fs::remove_file(stop_mark(f));
    }
    result
}

/// What Jaynes does to a process group: ask whether any of it is left, ask it to stop, or stop it for good.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sig {
    Probe,
    Term,
    Kill,
}

/// What came of a signal to a process group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sent {
    /// Some process of the group took it.
    Took,
    /// No process is in the group.
    Gone,
    /// The system refused it all along: on macOS, a group whose processes have all ended and wait to be collected.
    Refused,
}

/// Signal the process group `pgid` straight from this program, so no kill program is needed on the machine. The
/// group of process 1, and Jaynes's own group, are never signalled: that would reach every process.
pub fn signal_group(pgid: u32, sig: Sig) -> Result<Sent, String> {
    use rustix::process::{getpgrp, kill_process_group, test_kill_process_group, Pid, Signal};
    let pid = i32::try_from(pgid).ok().and_then(Pid::from_raw).filter(|p| p.as_raw_nonzero().get() > 1 && *p != getpgrp());
    let Some(pid) = pid else { return Err(format!("Jaynes refused to signal the process group {pgid}")) };
    let send = || match sig {
        Sig::Probe => test_kill_process_group(pid),
        Sig::Term => kill_process_group(pid, Signal::TERM),
        Sig::Kill => kill_process_group(pid, Signal::KILL),
    };
    // macOS refuses a signal to a group whose processes have all ended, until their parent collects them
    for _ in 0..10 {
        match send() {
            Ok(()) => return Ok(Sent::Took),
            Err(rustix::io::Errno::SRCH) => return Ok(Sent::Gone),
            Err(rustix::io::Errno::PERM) => std::thread::sleep(Duration::from_millis(30)),
            Err(e) => return Err(format!("the system refused to signal the process group {pgid} ({sig:?}): {e}")),
        }
    }
    Ok(Sent::Refused)
}

/// Whether some process of the group `pgid` still runs: one that ended and waits to be collected does not, though
/// the system still counts it in the group, as in a container whose process 1 collects nobody. On Linux each member
/// is read in /proc; on macOS, a group the system refuses to signal is one of such ended processes, since an agent's
/// group is its user's own.
pub fn group_runs(pgid: u32) -> Result<bool, String> {
    match signal_group(pgid, Sig::Probe)? {
        Sent::Gone => Ok(false),
        Sent::Refused => Ok(linux_members_running(pgid).unwrap_or(false)),
        Sent::Took => Ok(linux_members_running(pgid).unwrap_or(true)),
    }
}

/// On Linux, whether a member of the group `pgid` runs, read in /proc; None where there is no /proc to read.
fn linux_members_running(pgid: u32) -> Option<bool> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let entries = std::fs::read_dir("/proc").ok()?;
    for e in entries.flatten() {
        if !e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Some((state, group)) = std::fs::read_to_string(e.path().join("stat")).ok().as_deref().and_then(stat_state_and_group) else { continue };
        if group == pgid && state != 'Z' && state != 'X' {
            return Some(true);
        }
    }
    Some(false)
}

/// A process's state and group from its line in /proc/<pid>/stat: after the name in parentheses, which may hold
/// any character, come the state, the parent and the group.
fn stat_state_and_group(stat: &str) -> Option<(char, u32)> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut fields = rest.split_whitespace();
    let state = fields.next()?.chars().next()?;
    let _parent = fields.next()?;
    Some((state, fields.next()?.parse().ok()?))
}

/// When a process started, to tell it from a later one given the same number: on Linux its start in /proc, on
/// macOS what ps says. None where neither can be read.
fn started_at(pid: u32) -> Option<String> {
    if cfg!(target_os = "linux") {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // the 22nd field, counted from the state as the 3rd
        return stat[stat.rfind(')')? + 1..].split_whitespace().nth(19).map(str::to_string);
    }
    let out = Command::new("ps").args(["-o", "lstart=", "-p", &pid.to_string()]).output().ok()?;
    let said = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !said.is_empty()).then_some(said)
}

/// Note the agent's or the role's process in `pidfile`: its group and when it started, so a process that takes
/// the run after a crash stops that group and no later one given the same number.
pub fn note_process(pidfile: &Path, pid: u32) -> std::io::Result<()> {
    std::fs::write(pidfile, format!("{pid} {}", started_at(pid).unwrap_or_default()))
}

/// Stop the process group `pgid`, gently then for good: true when something of it ran. An error when the system
/// refused a signal, or a process of the group still runs after the last one.
pub fn stop_group(pgid: u32, patience: Duration) -> Result<bool, String> {
    stop_group_reaping(pgid, patience, || {})
}

/// `stop_group` for a group whose leader is Jaynes's own child: `reap` collects it once it ends.
pub fn stop_group_reaping(pgid: u32, patience: Duration, mut reap: impl FnMut()) -> Result<bool, String> {
    reap();
    if !group_runs(pgid)? {
        return Ok(false);
    }
    signal_group(pgid, Sig::Term)?;
    let until = Instant::now() + patience;
    while Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
        reap();
        if !group_runs(pgid)? {
            return Ok(true);
        }
    }
    signal_group(pgid, Sig::Kill)?;
    for _ in 0..40 {
        reap();
        if !group_runs(pgid)? {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(format!("a process of the group {pgid} still runs after Jaynes stopped it for good"))
}

/// The file whose presence asks the process that drives a run to stop the agent it holds: a halt or an abandon from
/// another process leaves it, and the driver stops its own child, never a group read from a file.
pub fn stop_mark(pidfile: &Path) -> PathBuf {
    pidfile.with_extension("pid.stop")
}

/// Ask the process that drives a run to stop the agent noted in `pidfile`, and wait until it did. Nothing to do
/// when no agent is at work; an error when it is not stopped in time.
pub fn ask_stop(pidfile: &Path, patience: Duration) -> Result<bool, String> {
    if !pidfile.exists() {
        return Ok(false);
    }
    let mark = stop_mark(pidfile);
    std::fs::write(&mark, "").map_err(|e| format!("Jaynes could not ask for the agent to stop: {e}"))?;
    let until = Instant::now() + patience;
    while Instant::now() < until {
        if !mark.exists() || !pidfile.exists() {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _gone = std::fs::remove_file(&mark);
    Err(format!("the process that drives the run did not stop its agent within {} s", patience.as_secs()))
}

/// Stop what an agent program left running when the process that started it died: its whole process group, gently
/// then for good, if the group is still the one noted (a later process given the same number is left alone). True
/// when something was stopped; an error when it could not be. The note stays until the group is stopped.
pub fn stop_left(pidfile: &Path) -> Result<bool, String> {
    let Ok(text) = std::fs::read_to_string(pidfile) else { return Ok(false) };
    let mut said = text.split_whitespace();
    let Some(pgid) = said.next().and_then(|p| p.parse::<u32>().ok()) else {
        let _gone = std::fs::remove_file(pidfile);
        return Ok(false);
    };
    let noted = said.collect::<Vec<_>>().join(" ");
    // the leader, if it is still there, must be the process noted; without it the number cannot have been given
    // to another group, since a group's number is not given out while the group has members
    if let Some(now) = started_at(pgid)
        && !noted.is_empty()
        && now != noted
    {
        let _gone = std::fs::remove_file(pidfile);
        return Ok(false);
    }
    let stopped = stop_group(pgid, Duration::from_secs(3))?;
    let _gone = std::fs::remove_file(pidfile);
    Ok(stopped)
}

/// End Jaynes's own child: its group, then the child itself if it left the group, and collect it, so no turn waits
/// on a process that moved out of its group.
fn end_child(child: &mut std::process::Child, pid: u32, patience: Duration) -> Result<(), String> {
    let stopped = stop_group_reaping(pid, patience, || drop(child.try_wait()));
    if matches!(child.try_wait(), Ok(None)) {
        let _killed = child.kill();
    }
    let _waited = child.wait();
    stopped.map(drop)
}

#[allow(clippy::too_many_arguments)]
fn watch(child: &mut std::process::Child, pid: u32, input: &str, deadline: Duration, idle: Option<Duration>, started: Instant, paused: Option<&Path>, pidfile: Option<&Path>, mut on_session: OnSession, live: Option<&Path>) -> Result<Watched, String> {
    if let Some(mut stdin) = child.stdin.take() {
        let _wrote = stdin.write_all(input.as_bytes());
    }
    let stdout = child.stdout.take().ok_or("no output pipe")?;
    let mut stderr = child.stderr.take().ok_or("no error pipe")?;
    // what is read so far, kept outside the readers: a process the turn left behind may hold the pipes open
    let out_text = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let err_text = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let kept = out_text.clone();
    // when the program last said anything, and how many of its tools are at work
    let life = std::sync::Arc::new(std::sync::Mutex::new((Instant::now(), 0i64)));
    let beat = life.clone();
    let mut live = live.and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok());
    let out = std::thread::spawn(move || {
        use std::io::BufRead;
        let mut lines = std::io::BufReader::new(stdout);
        let mut line = String::new();
        while lines.read_line(&mut line).is_ok_and(|n| n > 0) {
            if let Ok(mut b) = beat.lock() {
                b.0 = Instant::now();
                b.1 += tools_at_work(&line);
            }
            if let Some(f) = on_session.as_mut()
                && let Some((session, confirmed)) = session_of(&line)
            {
                f(session, confirmed);
                on_session = None;
            }
            // the model's text as it comes is only a sign of life; the whole messages follow it
            if !line.starts_with(r#"{"type":"stream_event""#)
                && let Ok(mut t) = kept.lock()
            {
                t.push_str(&line);
                if let Some(f) = live.as_mut() {
                    let _shown = f.write_all(line.as_bytes());
                }
            }
            line.clear();
        }
    });
    let kept = err_text.clone();
    let err = std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = stderr.read(&mut buf) {
            if n == 0 {
                break;
            }
            if let Ok(mut t) = kept.lock() {
                t.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        }
    });
    let mut timed_out = false;
    let mut stalled = false;
    let mut waited = Duration::ZERO;
    let mut last = Instant::now();
    let code = loop {
        let now = Instant::now();
        if paused.is_some_and(Path::exists) {
            waited += now - last;
        }
        last = now;
        // a halt or an abandon from another process: this process stops its own child
        if let Some(mark) = pidfile.map(stop_mark)
            && mark.exists()
        {
            let ended = end_child(child, pid, Duration::from_secs(2));
            let _done = std::fs::remove_file(&mark);
            ended.map_err(|e| format!("the agent could not be stopped: {e}"))?;
            break None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if started.elapsed().saturating_sub(waited) > deadline => {
                timed_out = true;
                end_child(child, pid, Duration::from_secs(3))?;
                break None;
            }
            Ok(None) if idle.is_some_and(|limit| {
                let (said, working) = life.lock().map(|b| *b).unwrap_or((Instant::now(), 0));
                working <= 0 && !paused.is_some_and(Path::exists) && said.elapsed() > limit
            }) =>
            {
                stalled = true;
                end_child(child, pid, Duration::from_secs(2))?;
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(format!("the agent program could not be watched: {e}")),
        }
    };
    let took = started.elapsed().saturating_sub(waited);
    // what the turn left running in its group ends with it
    stop_group(pid, Duration::from_secs(2)).map_err(|e| format!("what the agent left running could not be stopped: {e}"))?;
    // a process that left the group may still hold the pipes: read for a moment more, then take what came
    let until = Instant::now() + Duration::from_secs(3);
    while !(out.is_finished() && err.is_finished()) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    let text = |t: &std::sync::Arc<std::sync::Mutex<String>>| t.lock().map(|t| t.clone()).unwrap_or_default();
    Ok(Watched { code, out: text(&out_text), err: text(&err_text), took, timed_out, stalled })
}

/// How a line of Claude Code's stream changes the count of its tools at work: a tool it calls starts one, a tool's
/// result ends one.
fn tools_at_work(line: &str) -> i64 {
    if !line.contains("tool_use") && !line.contains("tool_result") && !line.contains("\"item.") {
        return 0;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else { return 0 };
    // Codex: a command it starts is at work until its item completes
    let working = |v: &Value| matches!(v["item"]["type"].as_str(), Some("command_execution" | "mcp_tool_call" | "web_search"));
    match v["type"].as_str() {
        Some("item.started") if working(&v) => return 1,
        Some("item.completed") if working(&v) && v["item"]["status"] != "declined" => return -1,
        _ => {}
    }
    let count = |kind: &str| v["message"]["content"].as_array().map(|a| a.iter().filter(|c| c["type"] == kind).count() as i64).unwrap_or(0);
    match v["type"].as_str() {
        Some("assistant") => count("tool_use"),
        Some("user") => -count("tool_result"),
        _ => 0,
    }
}

// --- a command of the person's: for tests and for agent programs without an adapter --------------------------

/// Runs `sh -c <command>` in the step's folder with the prompt on standard input. Exit 0 ran, 75 is a quota,
/// 3 is a fault of the machine, anything else the agent's failure. A last line of JSON may give `tokens` and
/// `session`.
pub struct Shell {
    pub command: String,
}

impl Agent for Shell {
    fn name(&self) -> String {
        format!("command: {}", self.command)
    }

    fn probe(&self, cwd: &Path) -> Result<(), String> {
        probe_folder(cwd)
    }

    fn turn(&self, spec: Spec<'_>) -> Turn {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(&self.command).current_dir(spec.cwd);
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        if let Some(s) = spec.session {
            cmd.env("JAYNES_SESSION", s);
        }
        let applied = json!({"command": self.command});
        let ended = match run(cmd, spec.prompt, spec.deadline, None, spec.pidfile.as_deref(), spec.paused.as_deref(), spec.on_session, live(spec.transcript.as_deref())) {
            Err(reason) => Ended::Fault { reason },
            Ok(Watched { code, out, err, took, timed_out, .. }) => {
                if let Some(t) = &spec.transcript {
                    let _kept = std::fs::write(t, format!("{out}\n--- stderr ---\n{err}"));
                }
                let said: Value = out.lines().rev().find_map(|l| serde_json::from_str(l).ok()).unwrap_or(Value::Null);
                let usage = Usage { seconds: took.as_secs(), tokens: said["tokens"].as_u64().unwrap_or(0), cached: 0 };
                let session = said["session"].as_str().map(str::to_string);
                match code {
                    _ if timed_out => Ended::Failed { reason: "the turn ran past its deadline".to_string(), usage },
                    Some(0) => Ended::Ran { usage, session, model: "command".to_string() },
                    Some(75) => Ended::Interrupted { reason: "quota".to_string() },
                    Some(3) => Ended::Fault { reason: err.trim().chars().take(300).collect() },
                    _ => Ended::Failed { reason: format!("the agent exited with {code:?}: {}", err.trim().chars().take(300).collect::<String>()), usage },
                }
            }
        };
        Turn { ended, applied }
    }
}

fn probe_folder(cwd: &Path) -> Result<(), String> {
    let probe = cwd.join(crate::project::EXCHANGE).join("probe");
    std::fs::create_dir_all(cwd.join(crate::project::EXCHANGE)).map_err(|e| format!("Jaynes cannot write in {}: {e}", cwd.display()))?;
    std::fs::write(&probe, b"probe").map_err(|e| format!("Jaynes cannot write in {}: {e}", cwd.display()))?;
    std::fs::remove_file(&probe).map_err(|e| e.to_string())
}

// --- Claude Code -------------------------------------------------------------------------------------------------

/// How long Claude Code waits on a call to Jaynes's leave server: a week, so a leave waits for the person.
const LEAVE_PATIENCE_MS: &str = "604800000";

pub struct Claude {
    pub model: String,
    pub effort: String,
    pub max_messages: u32,
}

impl Default for Claude {
    fn default() -> Self {
        Claude {
            model: std::env::var("JAYNES_CLAUDE_MODEL").unwrap_or_else(|_| "claude-opus-5-5".to_string()),
            effort: std::env::var("JAYNES_CLAUDE_EFFORT").unwrap_or_else(|_| "high".to_string()),
            max_messages: 200,
        }
    }
}

const WEB_TOOLS: &[&str] = &["WebFetch", "WebSearch"];

/// Everything in `runs` but the way down to `cwd`: the other runs, and in the step's own run every folder and file
/// that does not hold `cwd`. Claude Code's box lets a hidden folder win over a readable one inside it, so the runs
/// cannot be hidden whole; they are hidden one by one as they are when the step starts. A run that begins later is
/// not among them: with the engine's home under the person's, as it is by default, `~/` hides it anyway (measured
/// with a real run); with JAYNES_ENGINE_HOME elsewhere, it stays readable to this step. When `cwd` is not under `runs`,
/// all of `runs` is hidden.
fn beside(runs: &Path, cwd: &Path) -> Vec<PathBuf> {
    let (Ok(runs), Ok(cwd)) = (std::fs::canonicalize(runs), std::fs::canonicalize(cwd)) else {
        return vec![runs.to_path_buf()];
    };
    if !cwd.starts_with(&runs) || cwd == runs {
        return vec![runs];
    }
    let mut out = Vec::new();
    let mut at = runs;
    for part in cwd.strip_prefix(&at).map(|r| r.components().map(|c| c.as_os_str().to_owned()).collect::<Vec<_>>()).unwrap_or_default() {
        if let Ok(entries) = std::fs::read_dir(&at) {
            let mut others: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.file_name() != Some(part.as_os_str())).collect();
            others.sort();
            out.extend(others);
        }
        at = at.join(part);
    }
    out
}

/// `p` to hide, or, when it holds one of `reads`, everything in it but the way down to each of them.
fn around(p: &Path, reads: &[PathBuf]) -> Vec<PathBuf> {
    let canon = |x: &Path| std::fs::canonicalize(x).unwrap_or_else(|_| x.to_path_buf());
    let p = canon(p);
    let inside: Vec<PathBuf> = reads.iter().map(|r| canon(r)).filter(|r| r.starts_with(&p)).collect();
    if inside.is_empty() {
        return vec![p];
    }
    if inside.contains(&p) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for e in std::fs::read_dir(&p).into_iter().flatten().flatten() {
        out.extend(around(&e.path(), &inside));
    }
    out.sort();
    out
}

/// What the engine keeps under its home that no step reads, as the project commands' box hides it
/// (`ENGINE_SECRETS` in jaynes_box.py).
pub const ENGINE_SECRETS: &[&str] =
    &["page.key", "page.log", "app-key.pub", "record.sqlite3", "record.sqlite3-wal", "record.sqlite3-shm"];

/// The hosts a step that installs packages may reach from its commands, and nothing else.
pub const PACKAGE_HOSTS: &[&str] = &[
    "registry.npmjs.org", "registry.yarnpkg.com", "proxy.golang.org", "sum.golang.org", "pypi.org",
    "files.pythonhosted.org", "crates.io", "index.crates.io", "static.crates.io",
];

fn binary(name: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = path.split(':').map(PathBuf::from).collect();
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(Path::new(&home).join(".local/bin"));
    }
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

impl Claude {
    /// The settings for one step, generated from what it may reach. Ported from the Python engine's runtime.
    pub fn settings(&self, cwd: &Path, reach: &Reach, reads: &[PathBuf], asks: bool) -> Value {
        let web = reach.network == Network::Web;
        let mut allow: Vec<String> = vec!["Read(./**)".into()];
        if reach.project == ProjectAccess::Write || reach.project == ProjectAccess::None {
            // Edit(path) is the rule of every tool that writes a file; Claude Code matches no Write(path) rule
            allow.push("Edit(./**)".into());
        } else {
            allow.push(format!("Edit(./{}/**)", crate::project::EXCHANGE));
        }
        if reach.commands {
            allow.push("Bash".into());
        }
        if web {
            allow.extend(WEB_TOOLS.iter().map(|s| s.to_string()));
        }
        let mut deny: Vec<String> = vec!["Agent".into(), "Task".into(), "NotebookEdit".into()];
        if !web {
            deny.extend(WEB_TOOLS.iter().map(|s| s.to_string()));
        }
        if !reach.commands {
            deny.push("Bash".into());
        }
        let mut read: Vec<String> = vec![cwd.display().to_string()];
        read.extend(reads.iter().map(|p| p.display().to_string()));
        // the home and what the engine keeps, wherever its home is: the page's key and log, the record, the runs
        let mut hide: Vec<String> = vec!["~/".into()];
        let home = crate::home();
        hide.extend(ENGINE_SECRETS.iter().map(|f| home.join(f).display().to_string()));
        // what the person chose for each project, which an agent neither reads nor writes
        hide.push(home.join(crate::project_prefs::FOLDER).display().to_string());
        // what the step may read is never hidden: a folder that holds one is hidden around it, one entry at a time
        for p in beside(&home.join("runs"), cwd) {
            hide.extend(around(&p, reads).iter().map(|p| p.display().to_string()));
        }
        json!({
            "permissions": {"defaultMode": if asks { "default" } else { "dontAsk" }, "allow": allow, "deny": deny,
                            "blockReadsOutsideWorkingDirectories": true, "disableBypassPermissionsMode": "disable"},
            "sandbox": {"enabled": true, "allowUnsandboxedCommands": false, "failIfUnavailable": true,
                        "autoAllowBashIfSandboxed": true,
                        "network": {"allowedDomains": if reach.network == Network::Packages { PACKAGE_HOSTS.to_vec() } else { Vec::new() }, "strictAllowlist": true, "allowLocalBinding": true},
                        "filesystem": {"denyRead": hide, "allowRead": read,
                                       "allowWrite": [cwd.join(crate::project::EXCHANGE).join("tmp").display().to_string()]}},
            "disableAllHooks": true, "autoMemoryEnabled": false, "disableClaudeAiConnectors": true,
            "syncClaudeAiPlugins": false, "syncClaudeAiSkills": false, "disableBundledSkills": true
        })
    }
}

impl Agent for Claude {
    fn name(&self) -> String {
        format!("claude {} at {} effort", self.model, self.effort)
    }

    fn probe(&self, cwd: &Path) -> Result<(), String> {
        if binary("claude").is_none() {
            return Err("Claude Code is not installed".to_string());
        }
        if cwd.components().any(|c| c.as_os_str() == ".claude") {
            return Err(format!("Claude Code refuses to write under a .claude folder, and the step's folder is {}", cwd.display()));
        }
        if cfg!(target_os = "macos") && !Path::new("/usr/bin/sandbox-exec").exists() {
            return Err("the macOS sandbox Claude Code's guards need is not there".to_string());
        }
        probe_folder(cwd)
    }

    fn turn(&self, spec: Spec<'_>) -> Turn {
        let settings = self.settings(spec.cwd, spec.reach, spec.reads, spec.permit.is_some());
        let Some(claude) = binary("claude") else {
            return Turn { ended: Ended::Fault { reason: "Claude Code is not installed".to_string() }, applied: settings };
        };
        let tmp = spec.cwd.join(crate::project::EXCHANGE).join("tmp");
        let _made = std::fs::create_dir_all(&tmp);
        // What the agent asks beyond its settings goes to Jaynes's leave server; without one, it is refused.
        let (servers, mode) = match &spec.permit {
            Some((argv, env)) => {
                let env: serde_json::Map<String, Value> = env.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
                (json!({"mcpServers": {"jaynes": {"type": "stdio", "command": argv[0], "args": argv[1..], "env": env}}}), "default")
            }
            None => (json!({"mcpServers": {}}), "dontAsk"),
        };
        // the model's text comes as it is written, so a silent connection shows
        let command = |session: Option<&str>| {
            let mut cmd = Command::new(&claude);
            cmd.args(["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--setting-sources", ""])
                .arg("--settings")
                .arg(settings.to_string())
                .args(["--strict-mcp-config", "--mcp-config", &servers.to_string(), "--permission-mode", mode])
                .args(["--max-turns", &self.max_messages.to_string(), "--model", &self.model, "--effort", &self.effort]);
            if let Some(s) = session {
                cmd.args(["--resume", s]);
            }
            if spec.permit.is_some() {
                cmd.args(["--permission-prompt-tool", "mcp__jaynes__permit"]);
            }
            // a leave waits for the person as long as they take: Claude Code must not give up on the call meanwhile
            cmd.current_dir(spec.cwd).env("TMPDIR", &tmp).env("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "1").env("MCP_TOOL_TIMEOUT", LEAVE_PATIENCE_MS);
            for (k, v) in &spec.env {
                cmd.env(k, v);
            }
            cmd
        };
        let applied = json!({"program": "claude", "model": self.model, "effort": self.effort, "settings": settings,
                             "resume": spec.session});
        // A connection that went silent is taken up again in the same session, a few times, with nothing lost.
        let mut on_session = spec.on_session;
        let transcript = live(spec.transcript.as_deref());
        let mut session: Option<String> = spec.session.map(str::to_string);
        let mut prompt = spec.prompt.to_string();
        let mut deadline = spec.deadline;
        let mut said = (String::new(), String::new(), Duration::ZERO);
        let mut watched = None;
        let mut failure = "the agent program did not start".to_string();
        for _ in 0..=STALLS {
            let w = match run(command(session.as_deref()), &prompt, deadline, Some(SILENCE), spec.pidfile.as_deref(), spec.paused.as_deref(), on_session.take(), transcript) {
                Err(reason) => {
                    failure = reason;
                    break;
                }
                Ok(w) => w,
            };
            said.0.push_str(&w.out);
            said.1.push_str(&w.err);
            said.2 += w.took;
            deadline = deadline.saturating_sub(w.took);
            let stalled = w.stalled;
            session = w.out.lines().find_map(|l| session_of(l).map(|(s, _)| s)).or(session);
            watched = Some(w);
            if !stalled || session.is_none() {
                break;
            }
            prompt = RESUME.to_string();
        }
        let watched = watched.map(|w| Watched { out: said.0.clone(), err: said.1.clone(), took: said.2, ..w });
        let ended = match watched.ok_or(failure) {
            Err(reason) => Ended::Fault { reason },
            Ok(Watched { stalled: true, .. }) => Ended::Interrupted { reason: format!("its connection to the model went silent {} times; continue the run to take it up again", STALLS + 1) },
            Ok(Watched { code, out, err, took, timed_out, .. }) => {
                if let Some(t) = &spec.transcript {
                    let _kept = std::fs::write(t, format!("{out}\n--- stderr ---\n{err}"));
                }
                match refused_writes(&out, spec.cwd, spec.reach) {
                    Some(paths) => Ended::Fault {
                        reason: format!(
                            "Claude Code's guards, as Jaynes set them, refused writes the step may make: {}",
                            paths.join(", ")
                        ),
                    },
                    None => claude_ended(&self.model, code, &out, &err, took, timed_out),
                }
            }
        };
        Turn { ended, applied }
    }
}

// --- Codex -------------------------------------------------------------------------------------------------------

/// Why the real Codex does not run yet.
pub const CODEX_NOT_YET: &str = "Codex does not run yet: Jaynes runs it only inside its own box, and from there Codex cannot reach its model until Jaynes opens a way to it";

/// `codex exec --json`, inside Jaynes's box: never with only its own sandbox, which reads the whole disk, the engine's
/// home included. Its own sandbox is off inside the box (macOS opens no box inside another), and so are its approvals,
/// the person's settings and rules. The format it speaks is that of codex-rs/exec's exec_events.rs.
pub struct Codex {
    pub model: String,
    /// The program: a stand-in named by JAYNES_CODEX. None while the real Codex cannot reach its model from the box.
    pub program: Option<PathBuf>,
    /// The role library, whose box runs the program.
    pub library: Option<PathBuf>,
}

impl Codex {
    pub fn new(model: &str, library: Option<&Path>) -> Codex {
        Codex { model: model.to_string(), program: std::env::var("JAYNES_CODEX").ok().map(PathBuf::from), library: library.map(Path::to_path_buf) }
    }

    /// The arguments of one turn, in `session` when it goes on in one.
    pub fn arguments(&self, cwd: &Path, session: Option<&str>) -> Vec<String> {
        let mut args: Vec<String> = ["exec", "--json", "--model", &self.model, "--skip-git-repo-check", "--ignore-user-config", "--ignore-rules", "--sandbox", "danger-full-access", "--cd"]
            .iter().map(|s| s.to_string()).collect();
        args.push(cwd.display().to_string());
        for c in ["approval_policy=\"never\"", "project_doc_max_bytes=0", "web_search=\"disabled\""] {
            args.extend(["-c".to_string(), c.to_string()]);
        }
        if let Some(s) = session {
            args.extend(["resume".to_string(), s.to_string()]);
        }
        // the prompt on its input
        args.push("-".to_string());
        args
    }

    /// The command line that runs `args` inside Jaynes's box, from the role library's box.
    /// As Claude Code's settings follow the step's reach, so does the box: a step that reads the project writes only
    /// Jaynes's exchange in it, and one without commands starts nothing but the program.
    fn boxed(&self, program: &Path, cwd: &Path, tmp: &Path, reach: &Reach, args: &[String]) -> Result<Vec<String>, String> {
        let library = self.library.as_ref().ok_or("Jaynes has no box library for Codex in this run")?;
        let python = std::env::var("JAYNES_PYTHON").unwrap_or_else(|_| "python3".to_string());
        let mut flags: Vec<&str> = Vec::new();
        if reach.project == ProjectAccess::Read {
            flags.push("--read-only");
        }
        if !reach.commands {
            flags.push("--no-commands");
        }
        let done = Command::new(python)
            .arg(library.join("jaynes_box.py"))
            .args(["agent", &cwd.display().to_string(), &tmp.display().to_string()])
            .args(&flags)
            .args(["--", &program.display().to_string()])
            .args(args)
            .env("PYTHONPATH", library)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .output()
            .map_err(|e| format!("Jaynes could not start its box for Codex: {e}"))?;
        let said = String::from_utf8_lossy(&done.stdout).trim().to_string();
        if !done.status.success() {
            return Err(if said.is_empty() { String::from_utf8_lossy(&done.stderr).chars().take(300).collect() } else { said });
        }
        serde_json::from_str(&said).map_err(|e| format!("Jaynes's box gave no command line for Codex: {e}"))
    }
}

impl Agent for Codex {
    fn name(&self) -> String {
        format!("codex {}", self.model)
    }

    fn probe(&self, cwd: &Path) -> Result<(), String> {
        let Some(program) = &self.program else { return Err(CODEX_NOT_YET.to_string()) };
        if !program.is_file() {
            return Err(format!("the Codex program {} is not there", program.display()));
        }
        if self.library.is_none() {
            return Err("Jaynes has no box library for Codex in this run".to_string());
        }
        probe_folder(cwd)
    }

    fn turn(&self, spec: Spec<'_>) -> Turn {
        let args = self.arguments(spec.cwd, spec.session);
        let applied = json!({"program": "codex", "model": self.model, "arguments": args, "in": "Jaynes's box", "resume": spec.session});
        let Some(program) = &self.program else {
            return Turn { ended: Ended::Fault { reason: CODEX_NOT_YET.to_string() }, applied };
        };
        let tmp = spec.cwd.join(crate::project::EXCHANGE).join("tmp");
        let _made = std::fs::create_dir_all(&tmp);
        let argv = match self.boxed(program, spec.cwd, &tmp, spec.reach, &args) {
            Ok(argv) if !argv.is_empty() => argv,
            Ok(_) => return Turn { ended: Ended::Fault { reason: "Jaynes's box gave an empty command line for Codex".into() }, applied },
            Err(reason) => return Turn { ended: Ended::Fault { reason }, applied },
        };
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]).current_dir(spec.cwd).env("TMPDIR", &tmp);
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        let ended = match run(cmd, spec.prompt, spec.deadline, Some(SILENCE), spec.pidfile.as_deref(), spec.paused.as_deref(), spec.on_session, live(spec.transcript.as_deref())) {
            Err(reason) => Ended::Fault { reason },
            Ok(Watched { stalled: true, .. }) => Ended::Interrupted { reason: "its connection to the model went silent; continue the run to take it up again".to_string() },
            Ok(Watched { code, out, err, took, timed_out, .. }) => {
                if let Some(t) = &spec.transcript {
                    let _kept = std::fs::write(t, format!("{out}\n--- stderr ---\n{err}"));
                }
                let read = CodexTurn::read(&out);
                for command in &read.declined {
                    say(&format!("Codex declined a command it was not allowed to run, and could not ask you: {command}"));
                }
                // a write the box refused although the step may make it: the guards Jaynes set are wrong, which is
                // Jaynes's fault and must not spend the agent's attempt
                let refused: Vec<&String> = read.unwritten.iter().filter(|p| may_write(p, spec.cwd, spec.reach)).collect();
                if refused.is_empty() {
                    read.ended(code, &err, took, timed_out)
                } else {
                    Ended::Fault { reason: format!("Jaynes's box, as Jaynes set it for Codex, refused writes the step may make: {}", refused.iter().map(|p| p.as_str()).collect::<Vec<_>>().join(", ")) }
                }
            }
        };
        Turn { ended, applied }
    }
}

fn say(line: &str) {
    eprintln!("{line}");
}

/// What a turn of `codex exec --json` said.
#[derive(Debug, Default)]
struct CodexTurn {
    session: Option<String>,
    /// From turn.completed: input (the cached input counted inside it), cached input, output.
    usage: Option<(u64, u64, u64)>,
    /// From turn.failed and error.
    failures: Vec<String>,
    declined: Vec<String>,
    /// The files of changes that did not apply.
    unwritten: Vec<String>,
    /// Lines that are not its events.
    other: usize,
}

impl CodexTurn {
    fn read(out: &str) -> CodexTurn {
        let mut t = CodexTurn::default();
        for line in out.lines().filter(|l| !l.trim().is_empty()) {
            let Ok(e) = serde_json::from_str::<Value>(line) else {
                t.other += 1;
                continue;
            };
            let n = |v: &Value, k: &str| v[k].as_u64().unwrap_or(0);
            match e["type"].as_str() {
                Some("thread.started") => t.session = e["thread_id"].as_str().map(str::to_string),
                Some("turn.completed") => {
                    let u = &e["usage"];
                    t.usage = Some((n(u, "input_tokens"), n(u, "cached_input_tokens"), n(u, "output_tokens")));
                }
                Some("turn.failed") => t.failures.push(e["error"]["message"].as_str().unwrap_or("the turn failed").to_string()),
                Some("error") => t.failures.push(e["message"].as_str().unwrap_or("an error").to_string()),
                Some("item.completed") if e["item"]["type"] == "command_execution" && e["item"]["status"] == "declined" => {
                    t.declined.push(e["item"]["command"].as_str().unwrap_or("").to_string());
                }
                Some("item.completed") if e["item"]["type"] == "file_change" && e["item"]["status"] == "failed" => {
                    for c in e["item"]["changes"].as_array().into_iter().flatten() {
                        if let Some(p) = c["path"].as_str().filter(|p| !t.unwritten.iter().any(|u| u == p)) {
                            t.unwritten.push(p.to_string());
                        }
                    }
                }
                Some(_) => {}
                None => t.other += 1,
            }
        }
        t
    }

    fn ended(self, code: Option<i32>, err: &str, took: Duration, timed_out: bool) -> Ended {
        let (input, cached, output) = self.usage.unwrap_or((0, 0, 0));
        // the cached input is inside the input for Codex: the tokens are the new input and the output
        let usage = Usage { seconds: took.as_secs(), tokens: input.saturating_sub(cached) + output, cached };
        if timed_out {
            return Ended::Failed { reason: "the turn ran past its deadline".to_string(), usage };
        }
        let said = self.failures.last().cloned().unwrap_or_default();
        let lower = format!("{} {err}", self.failures.join(" ")).to_lowercase();
        if lower.contains("usage limit") || lower.contains("rate limit") || lower.contains("quota") {
            return Ended::Interrupted { reason: "quota".to_string() };
        }
        if self.usage.is_none() && self.failures.is_empty() {
            let tail: String = err.trim().chars().take(300).collect();
            return Ended::Fault { reason: format!("Codex ended with no completed turn ({code:?}), {} lines not its events: {tail}", self.other) };
        }
        if !self.failures.is_empty() {
            return Ended::Failed { reason: format!("the Codex turn failed: {}", said.chars().take(300).collect::<String>()), usage };
        }
        if code != Some(0) {
            return Ended::Failed { reason: format!("Codex exited with {code:?}: {}", err.trim().chars().take(300).collect::<String>()), usage };
        }
        Ended::Ran { usage, session: self.session, model: String::new() }
    }
}

/// Whether the step may write `path`, absolute or from its folder: Jaynes's exchange always, the project as its reach says.
fn may_write(path: &str, cwd: &Path, reach: &Reach) -> bool {
    let p = Path::new(path);
    let rel = if p.is_absolute() { p.strip_prefix(cwd).ok().map(|r| r.display().to_string()) } else { Some(path.trim_start_matches("./").to_string()) };
    match rel {
        Some(r) if r.split('/').any(|c| c == "..") => false,
        Some(r) if r.starts_with(crate::project::EXCHANGE) => true,
        Some(r) => reach.project == ProjectAccess::Write && jaynes_core::glob::allowed(&r, &reach.write),
        None => false,
    }
}

/// The writes Claude Code's own guards refused although the step may make them: a sign the guards Jaynes set are
/// wrong, which is Jaynes's fault and must not spend the agent's attempt.
fn refused_writes(out: &str, cwd: &Path, reach: &Reach) -> Option<Vec<String>> {
    let mut asked: std::collections::BTreeMap<String, String> = Default::default();
    let mut refused = Vec::new();
    for line in out.lines() {
        let Ok(e) = serde_json::from_str::<Value>(line) else { continue };
        for b in e["message"]["content"].as_array().into_iter().flatten() {
            if b["type"] == "tool_use" && (b["name"] == "Edit" || b["name"] == "Write")
                && let (Some(id), Some(path)) = (b["id"].as_str(), b["input"]["file_path"].as_str()) {
                    asked.insert(id.to_string(), path.to_string());
                }
            if b["type"] == "tool_result" && b["is_error"].as_bool().unwrap_or(false) {
                let text = b["content"].as_str().map(str::to_string).unwrap_or_else(|| b["content"].to_string());
                let Some(path) = b["tool_use_id"].as_str().and_then(|id| asked.get(id)) else { continue };
                if !text.contains("Permission to use") && !text.contains("not permitted") {
                    continue;
                }
                let rel = Path::new(path).strip_prefix(cwd).map(|p| p.display().to_string()).ok();
                let allowed = match rel {
                    Some(r) if r.starts_with(crate::project::EXCHANGE) => true,
                    Some(r) => reach.project == ProjectAccess::Write && jaynes_core::glob::allowed(&r, &reach.write),
                    None => false,
                };
                if allowed && !refused.contains(path) {
                    refused.push(path.clone());
                }
            }
        }
    }
    (!refused.is_empty()).then_some(refused)
}

fn claude_ended(model: &str, code: Option<i32>, out: &str, err: &str, took: Duration, timed_out: bool) -> Ended {
    let mut result = Value::Null;
    let mut session: Option<String> = None;
    for line in out.lines() {
        let Ok(e) = serde_json::from_str::<Value>(line) else { continue };
        if let Some(s) = e["session_id"].as_str() {
            session = Some(s.to_string());
        }
        if e["type"] == "result" {
            result = e;
        }
    }
    let u = &result["usage"];
    let n = |k: &str| u[k].as_u64().unwrap_or(0);
    let usage = Usage {
        seconds: took.as_secs(),
        tokens: n("input_tokens") + n("cache_creation_input_tokens") + n("output_tokens"),
        cached: n("cache_read_input_tokens"),
    };
    let said = result["result"].as_str().unwrap_or("").to_string();
    if timed_out {
        return Ended::Failed { reason: "the turn ran past its deadline".to_string(), usage };
    }
    let text = format!("{said} {}", err.chars().rev().take(400).collect::<String>().chars().rev().collect::<String>());
    let lower = text.to_lowercase();
    if result.is_null() {
        if lower.contains("limit") || lower.contains("quota") {
            return Ended::Interrupted { reason: "quota".to_string() };
        }
        return Ended::Fault { reason: format!("Claude Code ended with no result ({code:?}): {}", text.trim().chars().take(300).collect::<String>()) };
    }
    if result["is_error"].as_bool().unwrap_or(false) || code != Some(0) {
        let subtype = result["subtype"].as_str().unwrap_or("");
        if subtype == "error_max_turns" {
            return Ended::Failed { reason: "the turn ran out of messages before it finished".to_string(), usage };
        }
        if lower.contains("usage limit") || lower.contains("hit your limit") || lower.contains("rate limit") {
            return Ended::Interrupted { reason: "quota".to_string() };
        }
        return Ended::Fault { reason: format!("the Claude Code turn did not complete ({subtype}): {}", said.chars().take(300).collect::<String>()) };
    }
    match answered_by(model, out, &result) {
        Ok(answered) => Ended::Ran { usage, session, model: answered },
        Err(other) => Ended::Fault { reason: format!("asked for {model} and {other} answered; this turn does not count") },
    }
}

/// The model that answered a turn asked of `model`, or the other model that answered some of it.
///
/// What counts is the conversation itself: every message of the assistant that is not from inside a tool names its
/// model, and each must be the one asked for. A tool's own model does not count, such as the small one Claude Code's
/// web tool reads a page with, however much it wrote. Only when no message names a model does the turn's total per
/// model decide, and then the model asked for must have written something.
fn answered_by(model: &str, out: &str, result: &Value) -> Result<String, String> {
    let mut answered: Option<String> = None;
    for line in out.lines() {
        let Ok(e) = serde_json::from_str::<Value>(line) else { continue };
        if e["type"] != "assistant" || !e["parent_tool_use_id"].is_null() {
            continue;
        }
        // a message Claude Code writes itself, such as an error's text, is no model's
        let Some(name) = e["message"]["model"].as_str().filter(|n| !n.is_empty() && *n != "<synthetic>") else { continue };
        if !name.contains(model) {
            return Err(name.to_string());
        }
        answered = Some(name.to_string());
    }
    if let Some(name) = answered {
        return Ok(name);
    }
    let Some(used) = result["modelUsage"].as_object() else { return Ok(model.to_string()) };
    let wrote = |v: &Value| v["outputTokens"].as_u64().unwrap_or(0);
    match used.iter().find(|(k, v)| k.contains(model) && wrote(v) > 0) {
        Some((name, _)) => Ok(name.clone()),
        None => Err(used.iter().max_by_key(|(_, v)| wrote(v)).map(|(k, _)| k.clone()).unwrap_or_else(|| "no model".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_ends_even_when_something_it_left_running_holds_its_output() {
        // a process in the turn's group, and one that left the group, both keep the output pipe open for 30 s
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("sleep 30 & python3 -c 'import os,time; os.setsid(); time.sleep(30)' & echo hi");
        let started = Instant::now();
        let w = run(cmd, "", Duration::from_secs(60), None, None, None, None, None).expect("ran");
        assert_eq!((w.code, w.timed_out), (Some(0), false));
        assert!(w.out.contains("hi"), "{}", w.out);
        assert!(started.elapsed() < Duration::from_secs(15), "the turn waited {:?} for what it left running", started.elapsed());
    }

    #[test]
    fn a_group_is_signalled_from_the_program_and_never_the_whole_machine() {
        use std::os::unix::process::CommandExt;
        // process 1's group, 0, a number past what the system takes, and Jaynes's own group are refused, not
        // signalled; the probe is what is tried, so a guard that gave way would still kill nothing
        for refused in [0, 1, u32::MAX, i32::MAX as u32 + 1] {
            assert!(signal_group(refused, Sig::Probe).is_err(), "{refused}");
        }
        let own = u32::try_from(rustix::process::getpgrp().as_raw_nonzero().get()).expect("pgid");
        assert!(signal_group(own, Sig::Probe).is_err(), "Jaynes never signals its own group");
        // a group with a process left in it, and the same group once it is stopped
        let mut child = Command::new("sh").arg("-c").arg("sleep 30 & sleep 30").process_group(0).spawn().expect("spawn");
        let pgid = child.id();
        assert_eq!(signal_group(pgid, Sig::Probe), Ok(Sent::Took));
        assert_eq!(group_runs(pgid), Ok(true));
        assert_eq!(stop_group_reaping(pgid, Duration::from_secs(2), || drop(child.try_wait())), Ok(true));
        assert_eq!(group_runs(pgid), Ok(false), "nothing of the group runs");
        assert_eq!(stop_group(pgid, Duration::from_secs(1)), Ok(false), "nothing to stop is not an error");
    }

    #[test]
    fn a_group_of_processes_that_ended_and_wait_to_be_collected_does_not_run() {
        use std::os::unix::process::CommandExt;
        // the leader ends and nobody collects it yet, as in a container whose process 1 collects nobody
        let mut child = Command::new("true").process_group(0).spawn().expect("spawn");
        let pgid = child.id();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(group_runs(pgid), Ok(false), "an ended process waiting to be collected does not run");
        let asked = Instant::now();
        assert_eq!(stop_group(pgid, Duration::from_secs(3)), Ok(false));
        assert!(asked.elapsed() < Duration::from_secs(1), "nothing is waited for: {:?}", asked.elapsed());
        let _collected = child.wait();
    }

    #[test]
    fn an_agent_that_leaves_its_group_is_still_ended_at_its_deadline() {
        // it moves into the group of the process that started it, which Jaynes never signals: the deadline still
        // ends it, by its own number, and the turn does not wait on it
        let own = rustix::process::getpgrp().as_raw_nonzero().get().to_string();
        let mut cmd = Command::new("python3");
        cmd.args(["-c", "import os, sys, time; os.setpgid(0, int(sys.argv[1])); time.sleep(30)", &own]);
        let started = Instant::now();
        let w = run(cmd, "", Duration::from_secs(1), None, None, None, None, None).expect("ran");
        assert!(w.timed_out, "{w:?}");
        assert!(started.elapsed() < Duration::from_secs(8), "the turn waited on it: {:?}", started.elapsed());
    }

    #[test]
    fn a_process_s_state_and_group_read_past_any_name() {
        assert_eq!(stat_state_and_group("42 (sh) S 1 42 42 0 -1"), Some(('S', 42)));
        assert_eq!(stat_state_and_group("43 (a) b) (c) Z 42 42 42 0 -1"), Some(('Z', 42)), "a name with parentheses and spaces");
        assert_eq!(stat_state_and_group("garbage"), None);
    }

    #[test]
    fn a_note_of_a_process_names_it_and_a_later_one_with_its_number_is_left_alone() {
        use std::os::unix::process::CommandExt;
        let dir = std::env::temp_dir().join(format!("jaynes-note-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let note = dir.join("agent.pid");
        let mut child = Command::new("sleep").arg("30").process_group(0).spawn().expect("spawn");
        note_process(&note, child.id()).expect("note");
        let noted = std::fs::read_to_string(&note).expect("read");
        assert!(noted.starts_with(&format!("{} ", child.id())) && noted.trim().len() > child.id().to_string().len(), "the group and when it started: {noted}");
        // a note whose start does not match the process with that number now stops nothing, and goes
        std::fs::write(&note, format!("{} not-when-it-started", child.id())).expect("write");
        assert_eq!(stop_left(&note), Ok(false));
        assert!(!note.exists());
        assert_eq!(group_runs(child.id()), Ok(true), "the process with that number was left alone");
        // the right note stops it, and goes once it is stopped
        note_process(&note, child.id()).expect("note");
        let pid = child.id();
        let collector = std::thread::spawn(move || child.wait());
        assert_eq!(stop_left(&note), Ok(true));
        assert!(!note.exists());
        let _collected = collector.join();
        assert_eq!(group_runs(pid), Ok(false));
    }

    #[test]
    fn an_agent_silent_while_it_waits_for_its_model_is_stopped_and_a_tool_at_work_is_not() {
        // a line, then silence: the connection to the model is gone
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(r#"echo '{"type":"system","session_id":"s1"}'; sleep 20"#);
        let started = Instant::now();
        let w = run(cmd, "", Duration::from_secs(60), Some(Duration::from_secs(1)), None, None, None, None).expect("ran");
        assert!(w.stalled && !w.timed_out, "{w:?}");
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        // a tool at work is silent too, and is let be: its own limits are the agent program's
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(
            r#"echo '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash"}]}}'; sleep 3; echo '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1"}]}}'; echo '{"type":"result"}'"#,
        );
        let w = run(cmd, "", Duration::from_secs(60), Some(Duration::from_secs(1)), None, None, None, None).expect("ran");
        assert!(!w.stalled && w.code == Some(0), "{w:?}");
        // the model's text as it comes is a sign of life, and is not kept in the turn's output
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(r#"for i in 1 2 3 4; do echo '{"type":"stream_event","event":{"type":"content_block_delta"}}'; sleep 0.6; done; echo '{"type":"result"}'"#);
        let w = run(cmd, "", Duration::from_secs(60), Some(Duration::from_secs(1)), None, None, None, None).expect("ran");
        assert!(!w.stalled && !w.out.contains("stream_event"), "{w:?}");
    }

    #[test]
    fn what_the_agent_says_is_kept_as_it_says_it_so_the_person_can_watch_it_work() {
        let dir = std::env::temp_dir().join(format!("jaynes-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let live = dir.join("turn-1-0.jsonl");
        let _gone = std::fs::remove_file(&live);
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(
            r#"echo '{"type":"assistant","said":"first"}'; echo '{"type":"stream_event","event":{}}'; sleep 2; echo '{"type":"result","said":"last"}'"#,
        );
        let kept = live.clone();
        let turn = std::thread::spawn(move || run(cmd, "", Duration::from_secs(60), None, None, None, None, Some(&kept)));
        std::thread::sleep(Duration::from_millis(1000));
        let meanwhile = std::fs::read_to_string(&live).unwrap_or_default();
        assert!(meanwhile.contains(r#""said":"first""#) && !meanwhile.contains("last"), "while it works: {meanwhile:?}");
        assert!(!meanwhile.contains("stream_event"), "the model's text as it comes is not kept: {meanwhile:?}");
        let w = turn.join().expect("thread").expect("ran");
        assert_eq!(w.code, Some(0));
        assert!(std::fs::read_to_string(&live).unwrap_or_default().contains(r#""said":"last""#));
    }

    #[test]
    fn time_spent_waiting_for_the_person_is_not_the_agent_s() {
        let dir = std::env::temp_dir().join(format!("jaynes-pause-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let mark = dir.join("leave-waiting");
        // the program waits 2 s for the person, with a deadline of 1 s: it is not cut off, and the 2 s are not its
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(format!("touch {m}; sleep 2; rm {m}", m = mark.display()));
        let w = run(cmd, "", Duration::from_secs(1), None, None, Some(&mark), None, None).expect("ran");
        assert_eq!((w.code, w.timed_out), (Some(0), false));
        assert!(w.took < Duration::from_millis(900), "{:?}", w.took);
        // without the person, the same deadline cuts it off
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("sleep 2");
        let w = run(cmd, "", Duration::from_secs(1), None, None, Some(&mark), None, None).expect("ran");
        assert!(w.timed_out);
    }

    #[test]
    fn what_a_step_may_read_beside_it_stays_readable_and_the_rest_around_it_hidden() {
        let base = std::env::temp_dir().join(format!("jaynes-around-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let git = base.join("project.git");
        for d in ["objects/ab", "hooks", "refs"] {
            std::fs::create_dir_all(git.join(d)).expect("dir");
        }
        std::fs::write(git.join("config"), "").expect("write");
        let real = std::fs::canonicalize(&git).expect("canon");
        let hidden = around(&git, &[git.join("objects")]);
        assert_eq!(hidden, vec![real.join("config"), real.join("hooks"), real.join("refs")]);
        assert_eq!(around(&git, &[]), vec![real.clone()]);
        let _gone = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_runs_beside_the_step_s_folder_are_hidden_one_by_one() {
        let base = std::env::temp_dir().join(format!("jaynes-beside-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        for d in ["runs/r-other", "runs/r-own/steps/cases", "runs/r-own/project/src"] {
            std::fs::create_dir_all(base.join(d)).expect("dir");
        }
        std::fs::write(base.join("runs/r-own/lock"), "").expect("write");
        let runs = std::fs::canonicalize(base.join("runs")).expect("runs");
        let hidden = beside(&runs, &runs.join("r-own/project"));
        assert_eq!(hidden, vec![runs.join("r-other"), runs.join("r-own/lock"), runs.join("r-own/steps")]);
        // a step that works outside the runs reads none of them
        assert_eq!(beside(&runs, &base), vec![runs.clone()]);
        let _gone = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn every_path_rule_is_one_claude_code_matches() {
        // Claude Code matches a file's path rule as Read(path) or Edit(path), Edit for every tool that writes a file
        // (Write, Edit, NotebookEdit); a Write(path), NotebookEdit(path) or Glob(path) rule matches nothing
        for project in [ProjectAccess::Read, ProjectAccess::Write] {
            let reach = Reach { project, write: vec!["**".into()], network: Network::None, commands: true, local_ports: vec![] };
            let s = Claude::default().settings(Path::new("/w"), &reach, &[], false);
            for list in ["allow", "deny", "ask"] {
                for rule in s["permissions"][list].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    for never in ["Write(", "NotebookEdit(", "Glob("] {
                        assert!(!rule.starts_with(never), "{list} has {rule}, a path rule Claude Code never matches");
                    }
                }
            }
        }
    }

    #[test]
    fn a_read_only_step_writes_only_its_exchange_folder() {
        let reach = Reach { project: ProjectAccess::Read, write: vec![], network: Network::None, commands: false, local_ports: vec![] };
        let s = Claude::default().settings(Path::new("/w"), &reach, &[], false);
        let allow: Vec<&str> = s["permissions"]["allow"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        assert!(allow.contains(&"Edit(./.jaynes/**)"));
        assert!(!allow.contains(&"Edit(./**)"));
        assert!(s["permissions"]["deny"].as_array().into_iter().flatten().any(|v| v == "Bash"));
        // the shell's box reads nothing in the person's home but the step's folder (a real run: `cat ~/file` was
        // refused with "Operation not permitted")
        let hidden: Vec<&str> = s["sandbox"]["filesystem"]["denyRead"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        assert_eq!(hidden[0], "~/");
        assert_eq!(s["sandbox"]["filesystem"]["allowRead"][0], "/w");
        // and what the engine keeps, even when its home is not under the person's
        let home = crate::home();
        for f in ENGINE_SECRETS.iter().map(|f| home.join(f)).chain([home.join(crate::project_prefs::FOLDER)]) {
            assert!(hidden.contains(&f.display().to_string().as_str()), "{f:?} is not hidden: {hidden:?}");
        }
    }

    #[test]
    fn a_write_the_guards_refused_inside_the_reach_is_jaynes_s_fault() {
        let out = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Write","input":{"file_path":"/w/.jaynes/output.json"}},{"type":"tool_use","id":"t2","name":"Edit","input":{"file_path":"/elsewhere/x"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":"Permission to use Write has been denied"},{"type":"tool_result","tool_use_id":"t2","is_error":true,"content":"Permission to use Edit has been denied"}]}}"#;
        let reach = Reach { project: ProjectAccess::Write, write: vec!["**".into()], network: Network::None, commands: true, local_ports: vec![] };
        assert_eq!(refused_writes(out, Path::new("/w"), &reach), Some(vec!["/w/.jaynes/output.json".to_string()]));
        let none = r#"{"type":"user","message":{"content":[]}}"#;
        assert_eq!(refused_writes(none, Path::new("/w"), &reach), None);
    }

    #[test]
    fn a_step_that_installs_packages_reaches_only_their_registries() {
        let c = Claude { model: "m".into(), effort: "high".into(), max_messages: 10 };
        let reach = Reach { project: ProjectAccess::Write, write: vec!["**".into()], network: Network::Packages, commands: true, local_ports: vec![] };
        let s = c.settings(Path::new("/w"), &reach, &[], false);
        assert_eq!(s["sandbox"]["network"]["allowedDomains"], json!(PACKAGE_HOSTS));
        assert!(s["permissions"]["deny"].as_array().into_iter().flatten().any(|v| v == "WebFetch"), "no web tools");
        let none = c.settings(Path::new("/w"), &Reach { network: Network::None, ..reach }, &[], false);
        assert_eq!(none["sandbox"]["network"]["allowedDomains"], json!([]));
    }

    #[test]
    fn a_result_names_its_cost_and_session() {
        let out = r#"{"type":"system","session_id":"abc"}
{"type":"result","is_error":false,"result":"done","usage":{"input_tokens":10,"cache_creation_input_tokens":5,"cache_read_input_tokens":100,"output_tokens":7},"modelUsage":{"claude-opus-5-5":{"outputTokens":7}}}"#;
        match claude_ended("claude-opus-5-5", Some(0), out, "", Duration::from_secs(4), false) {
            Ended::Ran { usage, session, .. } => {
                assert_eq!((usage.tokens, usage.cached, usage.seconds), (22, 100, 4));
                assert_eq!(session.as_deref(), Some("abc"));
            }
            other => panic!("{other:?}"),
        }
    }

    /// A turn's output: the main conversation's messages, each by the model named, then the result with what
    /// each model wrote in all.
    fn turn_of(messages: &[(&str, Option<&str>)], usage: &[(&str, u64)]) -> String {
        let mut out = String::from("{\"type\":\"system\",\"session_id\":\"abc\"}\n");
        for (model, inside) in messages {
            let parent = inside.map(|t| format!("\"{t}\"")).unwrap_or_else(|| "null".to_string());
            out.push_str(&format!("{{\"type\":\"assistant\",\"message\":{{\"model\":\"{model}\",\"content\":[]}},\"parent_tool_use_id\":{parent}}}\n"));
        }
        let used: Vec<String> = usage.iter().map(|(m, n)| format!("\"{m}\":{{\"outputTokens\":{n}}}")).collect();
        out.push_str(&format!("{{\"type\":\"result\",\"is_error\":false,\"result\":\"done\",\"usage\":{{\"output_tokens\":7}},\"modelUsage\":{{{}}}}}", used.join(",")));
        out
    }

    fn ended(out: &str) -> Ended {
        claude_ended("claude-opus-5-5", Some(0), out, "", Duration::from_secs(4), false)
    }

    #[test]
    fn a_tool_s_own_model_that_wrote_more_than_the_model_asked_for_does_not_stop_the_turn() {
        // a step that reads the web: Claude Code's web tool reads each page with a small model of its own, which
        // wrote more than the model asked for (the numbers of a real turn with seven pages read)
        let out = turn_of(&[("claude-opus-5-5", None), ("claude-opus-5-5", None)], &[("claude-opus-5-5", 6366), ("claude-haiku-5-5", 10510)]);
        match ended(&out) {
            Ended::Ran { model, .. } => assert_eq!(model, "claude-opus-5-5"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_message_from_inside_a_tool_is_not_the_conversation() {
        let out = turn_of(&[("claude-opus-5-5", None), ("claude-haiku-5-5", Some("toolu_1")), ("claude-opus-5-5", None)], &[("claude-opus-5-5", 10), ("claude-haiku-5-5", 900)]);
        assert!(matches!(ended(&out), Ended::Ran { .. }), "{:?}", ended(&out));
    }

    #[test]
    fn a_conversation_that_changes_model_midway_does_not_count() {
        // the model asked for wrote the most, and still part of the conversation was another model's
        let out = turn_of(&[("claude-opus-5-5", None), ("claude-sonnet-5-5", None)], &[("claude-opus-5-5", 5000), ("claude-sonnet-5-5", 40)]);
        match ended(&out) {
            Ended::Fault { reason } => assert!(reason.contains("asked for claude-opus-5-5 and claude-sonnet-5-5 answered"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_turn_the_model_asked_for_never_answered_does_not_count() {
        // the conversation's messages name no model here, so what each model wrote in all decides
        let out = turn_of(&[], &[("claude-haiku-5-5", 300)]);
        match ended(&out) {
            Ended::Fault { reason } => assert!(reason.contains("asked for claude-opus-5-5 and claude-haiku-5-5 answered"), "{reason}"),
            other => panic!("{other:?}"),
        }
        let silent = turn_of(&[], &[("claude-opus-5-5", 0), ("claude-haiku-5-5", 300)]);
        assert!(matches!(ended(&silent), Ended::Fault { .. }), "{:?}", ended(&silent));
        // and with no message naming a model, the model asked for having written something is enough
        let helped = turn_of(&[], &[("claude-opus-5-5", 20), ("claude-haiku-5-5", 300)]);
        assert!(matches!(ended(&helped), Ended::Ran { .. }), "{:?}", ended(&helped));
    }

    #[test]
    fn a_message_claude_code_wrote_itself_names_no_model() {
        let out = turn_of(&[("claude-opus-5-5", None), ("<synthetic>", None)], &[("claude-opus-5-5", 10)]);
        assert!(matches!(ended(&out), Ended::Ran { .. }), "{:?}", ended(&out));
    }
}
