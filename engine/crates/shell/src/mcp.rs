//! The server agents in Claude Code use (`jaynes mcp`, started by the plugin): what waits, the roles, starting a run,
//! where a run is, and the few things an agent may do to one. Whatever comes through here is an agent's: the core
//! refuses what only the person may do.

use crate::drive;
use crate::install;
use crate::store::Store;
use jaynes_core::question::{Answer, By, Verdict};
use jaynes_core::run::{Phase, Run};
use jaynes_core::Input;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::Path;

fn tools() -> Value {
    let obj = |props: Value, required: &[&str]| json!({"type": "object", "properties": props, "required": required});
    json!([
        {"name": "home", "description": "What waits for the person, what is blocked and what runs, across every run.", "inputSchema": obj(json!({}), &[])},
        {"name": "roles", "description": "The roles installed, by name.", "inputSchema": obj(json!({}), &[])},
        {"name": "start", "description": "Start a run of a role on a project with the agreed plan as its request. It goes on by itself; the person answers on the page.",
         "inputSchema": obj(json!({"role": {"type": "string"}, "project": {"type": "string", "description": "the project's folder"},
                                   "request": {"type": "string", "description": "the whole agreed plan"},
                                   "risk": {"type": "string", "enum": ["low", "high"], "description": "high when the change is not small"}}), &["role", "project", "request"])},
        {"name": "run", "description": "Where a run is: its steps, what it waits for, what it spent.", "inputSchema": obj(json!({"run": {"type": "string"}}), &["run"])},
        {"name": "answer", "description": "Let a result go on as it is, at a stop the role marks as a release. Only that: everything else is the person's.",
         "inputSchema": obj(json!({"run": {"type": "string"}, "question": {"type": "string"}}), &["run", "question"])},
        {"name": "halt", "description": "Halt a run at once: its agent stops where it is, the step under way runs again on resume, and only the person resumes it.", "inputSchema": obj(json!({"run": {"type": "string"}}), &["run"])},
        {"name": "correct", "description": "Record that a delivered run was wrong, and in which step if known, so the rework counts.",
         "inputSchema": obj(json!({"run": {"type": "string"}, "note": {"type": "string"}, "step": {"type": "string"}}), &["run", "note"])},
    ])
}

/// Whether Jaynes is turned off in this session: JAYNES_OFF set to anything but nothing, 0, false, no or off. The
/// plugin's session hook reads it the same way (hooks/session.py).
pub fn off() -> bool {
    std::env::var("JAYNES_OFF").is_ok_and(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "" | "0" | "false" | "no" | "off"))
}

const OFF: &str = "Jaynes is off in this session (JAYNES_OFF is set): it offers no tools here, and agreed changes are made without it. Its guard keeps its own switch. Unset JAYNES_OFF to use Jaynes.";

pub fn serve(home: &Path) -> Result<(), String> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    // read once: the server is the session's, started with its environment
    let off = off();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(id) = msg.get("id").cloned() else { continue };
        let reply = match msg["method"].as_str().unwrap_or("") {
            "initialize" => json!({"jsonrpc": "2.0", "id": id, "result": {
                "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": {"tools": {}}, "serverInfo": {"name": "jaynes", "version": jaynes_core::ENGINE},
                "instructions": if off { OFF } else { "Jaynes runs agents through a role's method one step at a time and asks the person only where it matters. These tools read what waits and do the little an agent may; the person answers the rest on the page." }}}),
            "tools/list" => json!({"jsonrpc": "2.0", "id": id, "result": {"tools": if off { json!([]) } else { tools() }}}),
            "tools/call" => {
                let called = if off { Err(OFF.to_string()) } else { call(home, msg["params"]["name"].as_str().unwrap_or(""), &msg["params"]["arguments"]) };
                let (text, error) = match called {
                    Ok(t) => (t, false),
                    Err(e) => (e, true),
                };
                json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": text}], "isError": error}})
            }
            "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
            other => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("no method {other}")}}),
        };
        writeln!(out, "{reply}").map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The project's switch when it turns Jaynes off there: `.jaynes.toml` at the root of the git repository holding
/// `project` (found going up until a folder holds `.git`), saying `conduct = false`. The plugin's hooks read the same
/// file (hooks/switch.py); a file above that root is not the project's.
pub fn conducting_off_in(project: &Path) -> Option<std::path::PathBuf> {
    let mut here = project.canonicalize().ok()?;
    let root = loop {
        if here.join(".git").exists() {
            break here;
        }
        here = here.parent()?.to_path_buf();
    };
    let file = root.join(".jaynes.toml");
    let text = std::fs::read_to_string(&file).ok()?;
    // read line by line as the hooks read it, so a file one TOML reader would refuse means the same to both
    let off = text
        .lines()
        .take_while(|l| !l.trim_start().starts_with('['))
        .filter_map(|l| l.split_once('='))
        .any(|(k, v)| k.trim() == "conduct" && v.split('#').next().unwrap_or("").trim() == "false");
    off.then_some(file)
}

fn agent() -> By {
    By::Agent { who: "an agent through the Jaynes plugin".into() }
}

fn state(home: &Path, run: &str) -> Result<Run, String> {
    Store::open(&home.join("record.sqlite3"))?.state(run)?.ok_or_else(|| format!("there is no run {run}"))
}

fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args[key].as_str().filter(|s| !s.trim().is_empty()).ok_or_else(|| format!("{key} is needed"))
}

fn call(home: &Path, tool: &str, args: &Value) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    match tool {
        "home" => home_text(home),
        "roles" => {
            let roles = install::list(home);
            if roles.is_empty() {
                return Ok("No role is installed. `jaynes guide` says how to install the ones that come with Jaynes.".into());
            }
            Ok(roles.into_iter().map(|(n, v, a, _)| format!("{n} {v}: {a}")).collect::<Vec<_>>().join("\n"))
        }
        "start" => {
            let role = text(args, "role")?;
            let project = text(args, "project")?;
            let request = text(args, "request")?;
            if let Some(file) = conducting_off_in(Path::new(project)) {
                return Err(format!("{} says conduct = false: Jaynes stays out of this project, so make the change without it. The person removes that line to use Jaynes here.", file.display()));
            }
            let run = drive::launch(home, &exe, role, project, request, args["risk"].as_str() == Some("high"))?;
            Ok(format!(
                "Run {run} started; it goes on by itself. What needs the person is on the page, {}. `run` with {run} says where it is.",
                install::ensure_page(home)
            ))
        }
        "run" => Ok(run_text(&state(home, text(args, "run")?)?)),
        "answer" => {
            let run = state(home, text(args, "run")?)?;
            let qid = text(args, "question")?;
            let q = run.questions.get(qid).ok_or_else(|| format!("there is no question {qid}"))?;
            let verdicts = q.question.items.iter().map(|x| (x.id.clone(), Verdict::Keep)).collect();
            let answer = Answer { back: Default::default(), question: qid.to_string(), shown: q.question.shown(), verdicts, by: agent(), context: String::new() };
            let effects = drive::effects(home, &run, &q.question, &answer)?;
            let when = drive::submit(home, &exe, &run, Input::Answer { answer })?;
            Ok(format!("Let go on ({when}). {}", effects.sentences.join(" ")))
        }
        "halt" => {
            let run = state(home, text(args, "run")?)?;
            drive::submit(home, &exe, &run, Input::Halt { by: agent() })?;
            Ok(format!("Run {} halts now: its agent stops, and the step under way runs again when the person resumes it.", run.id))
        }
        "correct" => {
            let run = state(home, text(args, "run")?)?;
            let step = args["step"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
            drive::submit(home, &exe, &run, Input::Correct { note: text(args, "note")?.to_string(), by: agent(), step })?;
            Ok(format!("The correction of run {} is recorded.", run.id))
        }
        other => Err(format!("no tool {other}")),
    }
}

fn home_text(home: &Path) -> Result<String, String> {
    let store = Store::open(&home.join("record.sqlite3"))?;
    let mut waiting = Vec::new();
    let mut others = Vec::new();
    for id in store.runs()? {
        let Some(run) = store.state(&id)? else { continue };
        if run.finished() {
            continue;
        }
        for q in run.open_questions() {
            let q = &q.question;
            waiting.push(format!(
                "- run {} ({}), question {} at {}: {} [{}]{}",
                run.id,
                run.manifest.role,
                q.id,
                q.step,
                if q.about.is_empty() { "does what it made go on?" } else { &q.about },
                q.items.iter().map(|x| x.label.as_str()).collect::<Vec<_>>().join("; "),
                if q.holds { " — the run waits for it" } else { "" }
            ));
        }
        match &run.phase {
            Phase::Blocked { reason, .. } => others.push(format!("- run {} is blocked: {reason}", run.id)),
            Phase::Running => others.push(format!("- run {} runs", run.id)),
            _ => {}
        }
    }
    let mut out = String::new();
    out.push_str(if waiting.is_empty() { "Nothing waits for the person.\n" } else { "Waiting for the person (they answer on the page):\n" });
    for w in waiting {
        out.push_str(&w);
        out.push('\n');
    }
    for o in others {
        out.push_str(&o);
        out.push('\n');
    }
    Ok(out)
}

fn run_text(r: &Run) -> String {
    let path = r.path();
    let mut out = format!(
        "run {} — {} {} — {} risk — {}\nspent {} of {} s, {} of {} tokens\n",
        r.id,
        r.manifest.role,
        r.manifest.version,
        r.risk.word(),
        match &r.phase {
            Phase::Running => "running".to_string(),
            Phase::Waiting { why, .. } => format!("waiting: {why}"),
            Phase::Blocked { reason, .. } => format!("blocked: {reason}"),
            Phase::Finished { outcome } => format!("finished: {outcome:?}"),
        },
        r.spent.seconds,
        r.ceiling.seconds,
        r.spent.tokens,
        r.ceiling.tokens
    );
    for (i, d) in r.manifest.steps.iter().enumerate() {
        let state = if !path.contains(&i) {
            "left out".to_string()
        } else if r.fresh(i) {
            "done".to_string()
        } else {
            format!("{:?}", r.steps[i].status).to_lowercase()
        };
        out.push_str(&format!("  {} (round {}): {state}\n", d.name, r.steps[i].round));
    }
    out
}
