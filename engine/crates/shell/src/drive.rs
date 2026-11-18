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
use jaynes_core::{AgentTurn, AttemptOutcome, CheckResult, CheckVerdict, Command, Digest, Input, Run};
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
}

impl Setup {
    pub fn load(store: &Store, home: &Path, run: &str) -> Result<Setup, String> {
        let s = store.setup(run)?;
        let dir = home.join("runs").join(run);
        let mut role = RoleProgram::new(&dir.join("role"), &dir.join("library"));
        role.pidfile = Some(dir.join("role.pid"));
        // a run started before targets names none: it works on a repository
        let target = match s.get("target") {
            Some(t) => serde_json::from_value(t.clone()).map_err(|e| format!("the run's target is not one this engine knows: {e}"))?,
            None => jaynes_core::manifest::Target::Repository,
        };
        let copy = match (s["project"].as_str(), s["branch"].as_str()) {
            (Some(p), Some(b)) => Some(crate::project::open(target, Path::new(p), &dir.join("project"), b)),
            _ => None,
        };
        let agent = agent_named(s["agent"].as_str().unwrap_or("claude"));
        Ok(Setup { run: run.to_string(), dir, role, copy, agent })
    }
}

/// The agent a run's setup names: `command:CMD`, or `claude:MODEL` with the model fixed when the run started.
pub fn agent_named(spec: &str) -> Box<dyn Agent> {
    if let Some(cmd) = spec.strip_prefix("command:") {
        return Box::new(agent::Shell { command: cmd.to_string() });
    }
    let mut claude = agent::Claude::default();
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
pub fn launch(home: &Path, exe: &Path, role: &str, project: &str, request: &str, high: bool) -> Result<String, String> {
    crate::install::find_role(home, role)?;
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?;
    let run = format!("r{}-{:03}", t.as_secs(), t.subsec_millis());
    let mut argv = vec!["start", "--run", &run, "--role", role, "--project", project, "--request", request];
    if high {
        argv.extend(["--risk", "high"]);
    }
    std::fs::create_dir_all(home).map_err(|e| e.to_string())?;
    let log = std::fs::File::create(home.join(format!("start-{run}.log"))).map_err(|e| e.to_string())?;
    let err = log.try_clone().map_err(|e| e.to_string())?;
    std::process::Command::new(exe)
        .args(&argv)
        .env("JAYNES_ENGINE_HOME", home)
        .stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(err)
        .spawn()
        .map_err(|e| format!("the run could not start: {e}"))?;
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
        // An input that starts work is left to the process carried on here, which recovers first and then takes it:
        // decided here, the attempt it begins would be taken by that process for one Jaynes stopped. The rest is
        // recorded at once.
        if matches!(input, Input::Answer { .. } | Input::Continue { .. } | Input::Resume { .. }) {
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
        stop_agent_for(&dir, &input)?;
        Ok("inbox")
    }
}

/// What the run's process finds when the agent it waits for was stopped by a halt: the turn it gets back is
/// then interrupted, not an attempt that failed.
const HALTED_AGENT: &str = "agent-halted";

/// Stop the agent of a run left an abandon or a halt, once that input is in its inbox, so its process starts
/// no other turn: nothing the agent would do from here is wanted, and it costs tokens. An agent that could not be
/// stopped is said, never left silent: the input is recorded all the same.
pub fn stop_agent_for(dir: &Path, input: &Input) -> Result<(), String> {
    let unstopped = |e: String| format!("the input is recorded, but Jaynes could not stop the run's agent: {e}");
    // the process that drives the run stops the agent it holds: this one only asks, and waits until it did
    let patience = Duration::from_secs(10);
    match input {
        Input::Abandon { .. } => {
            agent::ask_stop(&dir.join("agent.pid"), patience).map_err(unstopped)?;
        }
        Input::Halt { .. } => {
            // the mark goes first: the process may get the turn back as soon as the agent is gone
            let _marked = std::fs::write(dir.join(HALTED_AGENT), "");
            if !agent::ask_stop(&dir.join("agent.pid"), patience).map_err(unstopped)? {
                let _gone = std::fs::remove_file(dir.join(HALTED_AGENT));
            }
        }
        _ => {}
    }
    Ok(())
}

// --- each run on its own engine ----------------------------------------------------------------------------------

/// Where the program of each engine that started a run is kept, by version.
fn engines(home: &Path) -> PathBuf {
    home.join("engines")
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
    let exe = engine_exe(home, &run.engine).ok_or_else(|| {
        format!("run {} started on engine {}, which is no longer on this machine: it cannot be carried on by this one ({})", run.id, run.engine, jaynes_core::ENGINE)
    })?;
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
        return Ok(jaynes_core::effects(run, question, answer));
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

/// The git settings of every agent's turn: no configuration of the person or of the system, and an identity of its own.
pub const AGENT_GIT_ENV: &[(&str, &str)] = &[
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_AUTHOR_NAME", "Jaynes agent"),
    ("GIT_AUTHOR_EMAIL", "agent@jaynes.invalid"),
    ("GIT_COMMITTER_NAME", "Jaynes agent"),
    ("GIT_COMMITTER_EMAIL", "agent@jaynes.invalid"),
];

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

/// What a person sees as the process goes: one line each.
pub fn say(line: &str) {
    eprintln!("{line}");
}

/// Decide on the run's latest state and record the events, in one transaction of the record.
pub fn decide_and_record(store: &mut Store, run: &str, input: &Input) -> Result<(Run, Vec<Event>, Vec<Command>), String> {
    store.decide(run, input)
}

/// What taking a run says when another process drives it: the one refusal that leaves an input in the inbox.
pub const DRIVEN: &str = "another process is driving this run";

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
        lock.try_lock().map_err(|_| DRIVEN.to_string())?;
        // macOS tells no child that its parent died: an agent the last process started may still be writing.
        let unstopped = |e: String| format!("a process that died left the run's agent or role running, and Jaynes could not stop it: {e}");
        if agent::stop_left(&setup.dir.join("agent.pid")).map_err(unstopped)? {
            say("stopped an agent program that a process which died had left running");
        }
        if agent::stop_left(&setup.dir.join("role.pid")).map_err(unstopped)? {
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
            let Some(mut input) = queue.pop_front() else { break };
            // A turn whose agent a halt stopped is interrupted, so it spends no attempt and runs again on resume.
            if matches!(input, Input::AttemptEnded { .. }) && std::fs::remove_file(self.setup.dir.join(HALTED_AGENT)).is_ok() {
                input = Input::Recovered;
            }
            let (state, events, commands) = match decide_and_record(self.store, &self.setup.run, &input) {
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
            self.follow_project(&state, &events)?;
            if events.iter().any(|e| matches!(e, Event::RunFinished { .. })) {
                clear_caches(&self.setup.dir);
            }
            for e in &events {
                narrate(e);
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
                if matches!(c, Command::RunAgent(_) | Command::RunRole { .. }) {
                    let mut came = Vec::new();
                    while let Some(input) = self.store.inbox_take(&self.setup.run)? {
                        came.push(input);
                    }
                    for input in came {
                        match decide_and_record(self.store, &self.setup.run, &input) {
                            Ok((now, events, more)) => {
                                for e in &events {
                                    narrate(e);
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
                                narrate(e);
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

    /// Put the copy where step `i` works: the base with the changes of the earlier steps it uses, and, when it
    /// runs again, its own previous change on top.
    fn prepare(&self, run: &Run, i: usize) -> Result<(), String> {
        let Some(copy) = &self.setup.copy else { return Ok(()) };
        if run.manifest.steps[i].reach.project == ProjectAccess::None {
            return Ok(());
        }
        let writers: Vec<String> = run
            .sources(i)
            .into_iter()
            .filter(|&j| run.manifest.steps[j].reach.project == ProjectAccess::Write)
            .map(|j| run.manifest.steps[j].name.clone())
            .collect();
        let parent = copy.compose(&self.changes(&writers))?;
        let name = run.manifest.steps[i].name.clone();
        let mut parents = self.parents();
        parents.insert(name.clone(), parent);
        std::fs::write(self.setup.dir.join("project-parents.json"), serde_json::to_string(&parents).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if run.manifest.steps[i].reach.project == ProjectAccess::Write && run.steps[i].round > 0 {
            let own = self.changes(std::slice::from_ref(&name));
            let mut all = self.changes(&writers);
            all.extend(own);
            copy.compose(&all).map(|_| ())?;
        }
        Ok(())
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
                Some((output, round)) => self.commit_done(step, output, round)?,
                None => copy.discard()?,
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
            let have = ledger.get(&decl.name).map(|c| copy.state_of(c)).transpose()?;
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
            Command::RunRole { step, round, attempt, given, previous, answers, key } => {
                Some(self.role_step(run, step, round, attempt, given, previous, answers, key))
            }
            Command::RunChecks { step, attempt, fix, output, given, checks } => {
                let results = self.checks(run, &step, &output, &given, &checks);
                Some(Input::ChecksEnded { step, attempt, fix, results })
            }
            Command::Ask { question } => {
                self.show(run, &question);
                None
            }
            Command::Permit { .. } => None,
            Command::Deliver { digest, merge, .. } => {
                let (ok, reference) = self.deliver(run, merge);
                if ok {
                    say(&format!("delivered: {reference}"));
                }
                Some(Input::DeliveryEnded { digest, ok, reference })
            }
            Command::Notify { text } => {
                say(&format!("! {text}"));
                None
            }
        }
    }

    /// Deliver from the run's copy of its target, and land it on the person's own thing when the role's delivery
    /// lands (on a repository: the run's branch, and the merge into the branch it started from). Whether it was made,
    /// and what was made.
    fn deliver(&self, run: &Run, merge: bool) -> (bool, String) {
        let writers: Vec<String> = run
            .path()
            .into_iter()
            .filter(|&j| run.manifest.steps[j].reach.project == ProjectAccess::Write)
            .map(|j| run.manifest.steps[j].name.clone())
            .collect();
        let (ok, reference) = match &self.setup.copy {
            Some(copy) => match copy.deliver(&self.changes(&writers)) {
                Ok(r) => (true, r),
                Err(e) => (false, e),
            },
            None => (true, "no project; the outputs are the delivery".to_string()),
        };
        let mut reference = reference;
        if ok && merge
            && let Some(copy) = &self.setup.copy {
                let head = copy.head().unwrap_or_default();
                let merged = copy.land(&head, &self.setup.run);
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
                    }
                    Ok(None) => reference.push_str("; nothing new to merge"),
                    Err(why) => reference.push_str(&format!("; not merged: {why}")),
                }
            }
        (ok, reference)
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
            outcome: AttemptOutcome::Fault { reason },
        };
        let Some(i) = run.manifest.index(&turn.step) else { return fault("no such step".into()) };
        if let Some(keep) = &turn.keep {
            let outcome = self.kept_alone(run, i, &turn, keep).unwrap_or_else(|reason| AttemptOutcome::Fault { reason });
            return Input::AttemptEnded { step: turn.step.clone(), attempt: turn.attempt, fix: turn.fix, outcome };
        }
        let cwd = self.cwd(run, i, turn.attempt);
        let exchange = cwd.join(EXCHANGE);
        let given = match self.given(&turn.given) {
            Ok(g) => g,
            Err(e) => return fault(e),
        };
        let previous = self.output_of(&turn.previous);
        if turn.fix == 0
            && let Err(e) = self.prepare(run, i) {
                return fault(e);
            }
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
            compose(&run.request, &text, &schema, &turn, &given)
        } else {
            let _gone = std::fs::remove_file(exchange.join("output.json"));
            let _gone = std::fs::remove_file(exchange.join(IMPOSSIBLE));
            fix_prompt(&turn)
        };
        say(&format!("· {} attempt {}{}", turn.step, turn.attempt, if turn.fix > 0 { format!(", fix {}", turn.fix) } else { String::new() }));
        let mut reads = self.setup.copy.as_ref().map(|c| c.reads()).unwrap_or_default();
        if let Ok(home) = std::env::var("HOME").map(PathBuf::from).and_then(|h| std::fs::canonicalize(h).map_err(|_| std::env::VarError::NotPresent)) {
            reads.extend(crate::project::toolchains(&std::env::var("PATH").unwrap_or_default(), &home));
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
        // box hides anyway, and commits in the copy as Jaynes's agent: what it commits there is never what is delivered
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
        };
        if let Some(t) = &spec.transcript {
            let _made = std::fs::create_dir_all(t.parent().unwrap_or(Path::new(".")));
        }
        let done = self.setup.agent.turn(spec);
        let _kept = std::fs::write(exchange.join(format!("applied-{}-{}.json", turn.attempt, turn.fix)), done.applied.to_string());
        let outcome = match done.ended {
            Ended::Fault { reason } => AttemptOutcome::Fault { reason },
            Ended::Interrupted { reason } => AttemptOutcome::Interrupted { reason },
            Ended::Failed { reason, usage } => AttemptOutcome::Failed { reasons: vec![reason], usage },
            Ended::Ran { usage, session, .. } => self.delivered(run, i, &turn, &exchange, usage, session),
        };
        Input::AttemptEnded { step: turn.step, attempt: turn.attempt, fix: turn.fix, outcome }
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
                Err(Err(e)) => return AttemptOutcome::Fault { reason: e },
            }
        }
        let object = json!({"output": output, "tree": tree});
        let digest = match self.store.put(&object) {
            Ok(d) => d,
            Err(e) => return AttemptOutcome::Fault { reason: e },
        };
        let items = match self.setup.role.call(Call::Items { step: turn.step.clone(), output: object["output"].clone() }) {
            Ok(Body::Items { items }) => match self.keep_items(&items, exchange.parent().unwrap_or(exchange)) {
                Ok(i) => i,
                Err(e) => return AttemptOutcome::Fault { reason: e },
            },
            Ok(other) => return AttemptOutcome::Fault { reason: format!("the role answered items with {other:?}") },
            Err(e) => return AttemptOutcome::Fault { reason: e },
        };
        AttemptOutcome::Delivered { output: digest, items, changed, usage, session }
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
        Ok(AttemptOutcome::Delivered { output: digest, items, changed, usage: Default::default(), session: None })
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
                    let name = format!("{page}.html");
                    std::fs::create_dir_all(&shelf).map_err(|e| e.to_string())?;
                    std::fs::write(shelf.join(&name), &source).map_err(|e| e.to_string())?;
                    page_kept = Some(name);
                    let of = photo.with_extension("png.of");
                    // Jaynes writes these in the copy, where an agent may have left a link to anywhere: the photograph is
                    // made in the run's own folder, out of the agent's reach, and put in place without following a link
                    let made = plain_inside(&base, &photo).is_ok();
                    if !(made && read_inside(&base, &of).is_ok_and(|d| d == page)) {
                        let shot = self.setup.dir.join("photograph.png");
                        let _gone = std::fs::remove_file(&shot);
                        self.photograph(&real, &shot).map_err(|e| format!("the picture {p} of {}: {e}", x.id))?;
                        let bytes = std::fs::read(&shot).map_err(|e| e.to_string())?;
                        write_inside(&base, &photo, &bytes).map_err(|e| format!("the picture {p} of {}: {e}", x.id))?;
                        write_inside(&base, &of, page.as_bytes()).map_err(|e| format!("the picture {p} of {}: {e}", x.id))?;
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
    fn photograph(&self, page: &Path, out: &Path) -> Result<(), String> {
        let python = std::env::var("JAYNES_PYTHON").unwrap_or_else(|_| "python3".to_string());
        let done = std::process::Command::new(python)
            .arg(self.setup.role.library.join("jaynes_box.py"))
            .arg("photograph")
            .arg(page)
            .arg(out)
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
            Err(e) => return end(AttemptOutcome::Fault { reason: e }),
        };
        let i = run.manifest.index(&step).unwrap_or(0);
        if let Err(e) = self.prepare(run, i) {
            return end(AttemptOutcome::Fault { reason: e });
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
        });
        let outcome = match reply {
            Err(e) => AttemptOutcome::Fault { reason: e },
            Ok(Body::Result { output }) => {
                let object = json!({"output": output, "tree": Value::Null});
                match self.store.put(&object) {
                    Err(e) => AttemptOutcome::Fault { reason: e },
                    Ok(digest) => match self.setup.role.call(Call::Items { step: step.clone(), output: object["output"].clone() }) {
                        Ok(Body::Items { items }) => AttemptOutcome::Delivered {
                            output: digest,
                            items: match self.keep_items(&items, &self.cwd(run, i, attempt)) {
                                Ok(i) => i,
                                Err(e) => return end(AttemptOutcome::Fault { reason: e }),
                            },
                            changed: Vec::new(),
                            usage: Default::default(),
                            session: None,
                        },
                        Ok(other) => AttemptOutcome::Fault { reason: format!("the role answered items with {other:?}") },
                        Err(e) => AttemptOutcome::Fault { reason: e },
                    },
                }
            }
            Ok(Body::NeedsInput { items, offered, about }) => {
                match self.keep_items(&items, &self.cwd(run, i, attempt)) {
                    Ok(items) => AttemptOutcome::NeedsInput { items, offered, about },
                    Err(e) => AttemptOutcome::Fault { reason: e },
                }
            }
            Ok(Body::Refused { reason }) => AttemptOutcome::Refused { reason },
            Ok(other) => AttemptOutcome::Fault { reason: format!("the role answered execute with {other:?}") },
        };
        end(outcome)
    }

    fn checks(&self, run: &Run, step: &str, output: &Digest, given: &[Read], checks: &[String]) -> Vec<CheckResult> {
        let blocked = |check: &str, reason: String| CheckResult { check: check.to_string(), verdict: CheckVerdict::Blocked { reason } };
        let output = match self.store.get(output) {
            Ok(v) => v["output"].clone(),
            Err(e) => return checks.iter().map(|c| blocked(c, e.clone())).collect(),
        };
        let given = match self.given(given) {
            Ok(g) => g,
            Err(e) => return checks.iter().map(|c| blocked(c, e.clone())).collect(),
        };
        let i = run.manifest.index(step).unwrap_or(0);
        let reaches = run.manifest.steps[i].reach.project != ProjectAccess::None;
        let project = self.setup.copy.as_ref().filter(|_| reaches).map(|c| c.dir().display().to_string());
        let source = self.setup.copy.as_ref().filter(|_| reaches).map(|c| c.source().display().to_string());
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
                }) {
                    Ok(Body::Verdict { verdict }) => CheckResult { check: name.clone(), verdict },
                    Ok(other) => blocked(name, format!("the role answered a check with {other:?}")),
                    Err(e) => blocked(name, e),
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
            say(&format!("  - {}: {}", item.id, item.label));
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
pub fn compose(request: &str, text: &str, schema: &Value, turn: &AgentTurn, given: &[Given]) -> String {
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
         an output you know is wrong to get past the checks.\n\n",
    );
    p.push_str(&format!(
        "# Your output\n\nWhen you are done, write your output as JSON to .jaynes/output.json, following this schema:\n\n{}\n",
        serde_json::to_string_pretty(schema).unwrap_or_default()
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

pub fn narrate(e: &Event) {
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
        Event::StepInvalidated { step, because, .. } => say(&format!("↺ {step} runs again: {}", because.join("; "))),
        Event::Waiting { why, .. } => say(&format!("… {why}")),
        Event::RunFinished { outcome, cost } => say(&format!(
            "run finished, {outcome:?}: {} s, {} tokens, {} attempts ({} spent by the agent), {} fixes, {} sent back",
            cost.usage.seconds, cost.usage.tokens, cost.attempts, cost.spent, cost.fixes, cost.sent_back
        )),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jaynes_core::manifest::{Executor, Manifest, Reach, SessionKind, StepDecl};
    use jaynes_core::question::By;
    use std::os::unix::fs::PermissionsExt;

    /// A record holding one run that an older engine started.
    fn old_run(home: &Path) -> Run {
        let step = StepDecl {
            name: "change".into(), needs: vec![], executor: Executor::Agent, session: SessionKind::Fresh, reach: Reach::default(),
            effect: Default::default(), checks: vec![], stop: None, from_risk: Default::default(), attempts: 3, fixes: 2, hidden: false, about: String::new(),
        };
        let manifest = Manifest { role: "r".into(), version: "1".into(), protocol: 1, steps: vec![step], target: Default::default(), delivery: Default::default() };
        let start = Input::Start { run: "old".into(), request: "x".into(), manifest, role_fingerprint: jaynes_core::Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default() };
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
    }

    #[test]
    fn a_run_s_agent_keeps_the_model_it_started_with() {
        assert!(agent_named("claude:claude-sonnet-5-5").name().starts_with("claude claude-sonnet-5-5 "));
        assert!(agent_named("command:true").name().starts_with("command:"));
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
}
