//! The leave server: what Claude Code asks while a turn runs, when it wants something its settings did not allow.
//!
//! Claude Code starts it as an MCP server (`--permission-prompt-tool mcp__jaynes__permit`). Each request becomes a
//! `PermissionAsked` for the core, recorded at once: a rule in the core grants a routine leave (reading inside the
//! project), and anything else opens a question for the person on the page, but a write outside the step's folder,
//! which is refused at once. The server waits for the answer in
//! the record and replies allow or deny. It waits as long as the person takes: while it waits, a file marks the wait,
//! and the turn's clock does not count that time against the agent.

use crate::drive::say;
use crate::store::Store;
use jaynes_core::event::Event;
use jaynes_core::question::{Ask, AskKind, Verdict};
use jaynes_core::Input;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct Permit {
    pub home: PathBuf,
    pub run: String,
    pub step: String,
    pub attempt: u32,
    pub cwd: PathBuf,
    /// The turn's box for the agent's shell commands (the single box, macOS: agent.rs writes it), read once as the
    /// server starts: every Bash call is given back as that box around the command, never as it came.
    pub boxed: Option<BoxSpec>,
    /// A turn whose agent may ask nothing beyond its settings (an answer at a stop): only its commands are boxed,
    /// anything else is refused, and nothing reaches the record.
    pub wrap_only: bool,
}

/// The file beside the turn's box that says why a command could not be put in it: the turn is then Jaynes's fault.
pub const BOX_FAILED: &str = "box-failed";

/// `s` as one word of a POSIX shell's line.
fn quoted(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The turn's box for the agent's shell commands, as the leave server read it when it started, before the agent
/// could act: its file is in the turn's own folder, which Claude Code's own tools may be let write, so it is never
/// read again.
pub struct BoxSpec {
    /// Where it was read, so a box that could not be read is said beside it.
    at: PathBuf,
    spec: Result<Value, String>,
}

impl BoxSpec {
    pub fn load(at: &Path) -> BoxSpec {
        let spec = std::fs::read_to_string(at).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()));
        BoxSpec { at: at.to_path_buf(), spec }
    }
}

/// What Claude Code runs for a Bash call of the agent: Jaynes's box around the command (jaynes_box.py agent-run, with
/// the box itself in the line), outside Claude Code's own sandbox, with the call's other fields kept. Never the call
/// as it came, even when the agent asked for no sandbox: without the box, the call is refused, and the reason is kept
/// beside the box's file so the turn ends as Jaynes's fault.
pub fn wrapped(boxed: &BoxSpec, input: &Value) -> Value {
    let refuse = |why: String| {
        let _kept = std::fs::write(boxed.at.with_file_name(BOX_FAILED), &why);
        json!({"behavior": "deny", "message": format!("Jaynes ran none of this command: {why}")})
    };
    let spec = match &boxed.spec {
        Ok(v) => v,
        Err(e) => return refuse(format!("it could not read the box of the agent's commands ({e})")),
    };
    let (Some(python), Some(script)) = (spec["python"].as_str(), spec["box"].as_str()) else {
        return refuse("the box of the agent's commands names no interpreter or script".into());
    };
    let Some(command) = input["command"].as_str() else {
        return json!({"behavior": "deny", "message": "the call names no command"});
    };
    let line = format!("{} -I {} agent-run --spec {} -- {}", quoted(python), quoted(script), quoted(&spec.to_string()), quoted(command));
    let mut given = input.as_object().cloned().unwrap_or_default();
    given.insert("command".into(), Value::String(line));
    given.insert("dangerouslyDisableSandbox".into(), Value::Bool(true));
    json!({"behavior": "allow", "updatedInput": given})
}

/// The file that exists while a leave of the run waits for the person.
pub fn waiting_mark(home: &Path, run: &str) -> PathBuf {
    home.join("runs").join(run).join("leave-waiting")
}

/// Marks a wait for the person while it lives.
struct Waiting(PathBuf);

impl Drop for Waiting {
    fn drop(&mut self) {
        let _gone = std::fs::remove_file(&self.0);
    }
}

/// What a tool call asks for, as the core reads a leave.
pub fn ask_of(tool: &str, input: &Value, cwd: &Path) -> Ask {
    let path = |v: &Value| -> String {
        let p = v.as_str().unwrap_or("");
        Path::new(p).strip_prefix(cwd).map(|r| r.display().to_string()).unwrap_or_else(|_| p.to_string())
    };
    match tool {
        "Read" | "Glob" | "Grep" | "LS" => {
            let target = ["file_path", "path", "pattern"].iter().map(|k| &input[*k]).find(|v| v.is_string()).map(path).unwrap_or_default();
            Ask { kind: AskKind::Read, target }
        }
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            let target = ["file_path", "notebook_path"].iter().map(|k| &input[*k]).find(|v| v.is_string()).map(path).unwrap_or_default();
            Ask { kind: AskKind::Write, target }
        }
        "WebFetch" => Ask { kind: AskKind::Network, target: input["url"].as_str().unwrap_or("").to_string() },
        "WebSearch" => Ask { kind: AskKind::Network, target: format!("search: {}", input["query"].as_str().unwrap_or("")) },
        "Bash" => Ask { kind: AskKind::Command, target: input["command"].as_str().unwrap_or("").to_string() },
        other => Ask { kind: AskKind::Command, target: format!("{other} {}", input) },
    }
}

/// Where `path` leads, its `..` and its links followed in the order they come: a link then `..` goes up from where
/// the link leads, as the system goes. A path that does not exist yet leads where it is written.
fn resolved(path: &Path) -> PathBuf {
    fn walk(path: &Path, depth: u32) -> PathBuf {
        let mut at = PathBuf::from("/");
        for c in path.components() {
            match c {
                std::path::Component::ParentDir => {
                    at.pop();
                }
                std::path::Component::Normal(n) => {
                    let next = at.join(n);
                    at = match std::fs::read_link(&next) {
                        // a link's own target, absolute or from the folder that holds it
                        Ok(to) if depth < 40 => walk(&at.join(to), depth + 1),
                        _ => next,
                    };
                }
                _ => {}
            }
        }
        at
    }
    walk(path, 0)
}

const REFUSED: &str = "Jaynes refused this write: ";
const OUTSIDE: &str = " is outside this step's folder, ";
const NOTHING: &str = ", and nothing is written outside it.";
const SAME: &str = " The same file in your copy of the project is ";
const THERE: &str = ": write it there.";

/// The refusal a write to a file outside `cwd`, the step's folder, gets at once, in the design-lead's words: Claude
/// Code's file tools run outside Jaynes's box, so a write allowed there would land on the Mac with no merge and no
/// Undo. Where it leads is read through its `..` and its links. When the file is the person's project's, at a path
/// the copy has too, the agent is told the copy's. None for any other call.
pub fn refusal(tool: &str, input: &Value, cwd: &Path, project: Option<&Path>) -> Option<String> {
    if !matches!(tool, "Write" | "Edit" | "MultiEdit" | "NotebookEdit") {
        return None;
    }
    let given = ["file_path", "notebook_path"].iter().find_map(|k| input[*k].as_str())?;
    let lands = resolved(&cwd.join(given));
    if lands.starts_with(resolved(cwd)) {
        return None;
    }
    let folder = cwd.display();
    let copy = project
        .and_then(|p| lands.strip_prefix(resolved(p)).ok())
        .filter(|rest| !rest.as_os_str().is_empty())
        .map(|rest| cwd.join(rest))
        .filter(|c| c.is_file() && resolved(c).starts_with(resolved(cwd)));
    Some(match copy {
        Some(c) => format!("{REFUSED}{}{OUTSIDE}{folder}{NOTHING}{SAME}{}{THERE}", lands.display(), c.display()),
        None => format!("{REFUSED}{}{OUTSIDE}{folder}{NOTHING} Write only under {folder}.", lands.display()),
    })
}

/// What a refusal of a write outside the step's folder said: the path refused, and the file of the copy the agent
/// was told to write instead, from the step's folder. None for any other text.
pub fn refused_said(text: &str) -> Option<(String, Option<String>)> {
    let rest = &text[text.find(REFUSED)? + REFUSED.len()..];
    let (path, rest) = rest.split_once(OUTSIDE)?;
    let (folder, rest) = rest.split_once(NOTHING)?;
    let copy = rest.strip_prefix(SAME).and_then(|r| r.split_once(THERE)).map(|(c, _)| c);
    let told = copy.and_then(|c| c.strip_prefix(folder)).map(|r| r.trim_start_matches('/').to_string());
    Some((path.to_string(), told))
}

/// The most of a file Jaynes keeps to show what an agent wants to write in it.
const WANTS_MOST: u64 = 1 << 20;

/// What a tool call wants to write, for the person to see before they allow it: the file as it is in `cwd`, the
/// step's copy, and as the call would leave it. None for a call that writes no file's text (a read, a notebook, a
/// file too big to show).
pub fn wants_of(tool: &str, input: &Value, cwd: &Path) -> Option<crate::stop::Wants> {
    let path = input["file_path"].as_str()?;
    let at = if Path::new(path).is_absolute() { PathBuf::from(path) } else { cwd.join(path) };
    let file = at.strip_prefix(cwd).map(|r| r.display().to_string()).unwrap_or_else(|_| path.to_string());
    let before = match std::fs::metadata(&at) {
        Ok(m) if m.len() > WANTS_MOST => return None,
        Ok(_) => Some(std::fs::read_to_string(&at).ok()?),
        Err(_) => None,
    };
    let edit = |text: &str, e: &Value| -> Option<String> {
        let (old, new) = (e["old_string"].as_str()?, e["new_string"].as_str()?);
        Some(if e["replace_all"].as_bool() == Some(true) { text.replace(old, new) } else { text.replacen(old, new, 1) })
    };
    let after = match tool {
        "Write" => input["content"].as_str()?.to_string(),
        "Edit" => edit(before.as_deref().unwrap_or(""), input)?,
        "MultiEdit" => input["edits"].as_array()?.iter().try_fold(before.clone().unwrap_or_default(), |text, e| edit(&text, e))?,
        _ => return None,
    };
    Some(crate::stop::Wants { file, before, after })
}

fn wants_path(home: &Path, run: &str, question: &str) -> PathBuf {
    home.join("runs").join(run).join("wants").join(format!("{question}.json"))
}

/// Keeps what the agent wants to write beside the run, under the question that asks leave for it.
pub fn keep_wants(home: &Path, run: &str, question: &str, wants: &crate::stop::Wants) -> Result<(), String> {
    let at = wants_path(home, run, question);
    std::fs::create_dir_all(at.parent().expect("a folder")).map_err(|e| e.to_string())?;
    std::fs::write(&at, serde_json::to_vec(wants).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// What the agent wanted to write, as kept for `question`, if anything was.
pub fn wants_kept(home: &Path, run: &str, question: &str) -> Option<crate::stop::Wants> {
    serde_json::from_slice(&std::fs::read(wants_path(home, run, question)).ok()?).ok()
}

impl Permit {
    pub fn serve(&self) -> Result<(), String> {
        let stdin = std::io::stdin();
        let mut out = std::io::stdout();
        for line in stdin.lock().lines() {
            let line = line.map_err(|e| e.to_string())?;
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
            let Some(id) = msg.get("id").cloned() else { continue }; // notifications ask for no reply
            let result = match msg["method"].as_str().unwrap_or("") {
                "initialize" => json!({
                    "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "jaynes", "version": jaynes_core::ENGINE},
                }),
                "tools/list" => json!({"tools": [{
                    "name": "permit",
                    "description": "Asks Jaynes for leave to use a tool the step's settings did not allow.",
                    "inputSchema": {"type": "object", "properties": {"tool_name": {"type": "string"}, "input": {"type": "object"}}, "required": ["tool_name"]},
                }]}),
                "tools/call" => {
                    let decision = self.decide(&msg["params"]["arguments"]);
                    json!({"content": [{"type": "text", "text": decision.to_string()}]})
                }
                "ping" => json!({}),
                other => {
                    let reply = json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("no method {other}")}});
                    writeln!(out, "{reply}").map_err(|e| e.to_string())?;
                    out.flush().map_err(|e| e.to_string())?;
                    continue;
                }
            };
            writeln!(out, "{}", json!({"jsonrpc": "2.0", "id": id, "result": result})).map_err(|e| e.to_string())?;
            out.flush().map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn deny(message: impl Into<String>) -> Value {
        json!({"behavior": "deny", "message": message.into()})
    }

    /// Ask the core, then wait in the record for the answer.
    pub fn decide(&self, args: &Value) -> Value {
        let tool = args["tool_name"].as_str().or_else(|| args["toolName"].as_str()).unwrap_or("");
        let input = if args["input"].is_object() { args["input"].clone() } else { args.clone() };
        if let (Some(boxed), "Bash") = (&self.boxed, tool) {
            return wrapped(boxed, &input);
        }
        if self.wrap_only {
            return Self::deny("this turn may use nothing beyond its settings");
        }
        let ask = ask_of(tool, &input, &self.cwd);
        let mut store = match Store::open(&self.home.join("record.sqlite3")) {
            Ok(s) => s,
            Err(e) => return Self::deny(format!("Jaynes could not read its record: {e}")),
        };
        // a write outside the step's folder is refused at once, with no question: nothing allowed there is undone
        let project = store.setup(&self.run).ok().and_then(|s| s["project"].as_str().map(PathBuf::from));
        if let Some(refused) = refusal(tool, &input, &self.cwd, project.as_deref()) {
            say(&format!("! a write outside {}'s folder refused: {}", self.step, ask.target));
            return Self::deny(refused);
        }
        let asked = Input::PermissionAsked { step: self.step.clone(), attempt: self.attempt, ask: ask.clone() };
        let events = match store.decide(&self.run, &asked) {
            Ok((_, events, _)) => events,
            Err(e) => return Self::deny(format!("Jaynes refused the request: {e}")),
        };
        let Some(question) = events.iter().find_map(|e| match e {
            Event::QuestionOpened { question } => Some(question.id.clone()),
            _ => None,
        }) else {
            return Self::deny("this attempt is no longer under way");
        };
        if let Some(wants) = wants_of(tool, &input, &self.cwd)
            && let Err(e) = keep_wants(&self.home, &self.run, &question, &wants) {
                say(&format!("! what the agent wants to write could not be kept: {e}"));
            }
        say(&format!("? leave asked at {}: {} {}", self.step, tool, ask.target));
        let mark = waiting_mark(&self.home, &self.run);
        let _waiting = std::fs::write(&mark, b"").ok().map(|()| Waiting(mark));
        loop {
            if let Ok(Some(run)) = store.state(&self.run)
                && let Some(q) = run.questions.get(&question) {
                    if let Some(a) = &q.answer {
                        return if a.verdict("leave") == Verdict::Keep {
                            json!({"behavior": "allow", "updatedInput": input})
                        } else {
                            let note = if a.context.trim().is_empty() { String::new() } else { format!(": {}", a.context.trim()) };
                            Self::deny(format!("the person refused leave to {}{note}. Go on without it.", q.question.about))
                        };
                    }
                    if q.superseded {
                        return Self::deny("the question was replaced");
                    }
                }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A turn's box file as agent.rs writes it, with `python` as the box's interpreter.
    fn box_file(dir: &Path, python: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).expect("dir");
        let at = dir.join("box.json");
        std::fs::write(&at, json!({"python": python, "box": dir.join("jaynes_box.py")}).to_string()).expect("write");
        at
    }

    #[test]
    fn a_bash_call_is_always_jaynes_s_box_around_its_command_never_the_call_as_it_came() {
        // the escape: the agent asks for no sandbox, with Claude Code letting unsandboxed commands through
        let dir = std::env::temp_dir().join(format!("jaynes-wrap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        // a stand-in for the box's interpreter: it says the box and the command it was given, and runs nothing
        let python = dir.join("python-stand-in");
        std::fs::write(&python, "#!/bin/sh\n[ \"$1\" = -I ] && [ \"$3\" = agent-run ] && [ \"$4\" = --spec ] && [ \"$6\" = -- ] && [ $# = 7 ] && printf '%s\\n%s' \"$5\" \"$7\"\n").expect("script");
        std::fs::set_permissions(&python, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("mode");
        let at = box_file(&dir, &python);
        let first = std::fs::read_to_string(&at).expect("box");
        let boxed = BoxSpec::load(&at);
        // the file is the turn's, in a folder Claude Code's own tools may be let write: what the agent puts there
        // later never widens its box
        std::fs::write(&at, json!({"python": python, "box": dir.join("jaynes_box.py"), "writable": ["/"]}).to_string()).expect("write");
        for command in ["curl https://example.com", "echo 'a b' \"c\" $HOME; touch ran", "x=$(touch ran) `touch ran` 'it''s'"] {
            let input = json!({"command": command, "dangerouslyDisableSandbox": true, "timeout": 5000, "run_in_background": false, "description": "d"});
            let said = wrapped(&boxed, &input);
            assert_eq!(said["behavior"], "allow", "{said}");
            let given = &said["updatedInput"];
            assert_eq!(given["dangerouslyDisableSandbox"], json!(true));
            assert_eq!((&given["timeout"], &given["run_in_background"], &given["description"]), (&json!(5000), &json!(false), &json!("d")));
            let line = given["command"].as_str().expect("a command");
            assert_ne!(line, command);
            // run as Claude Code's shell runs it: the box gets the box it was started with and the agent's command
            // whole, and nothing of it ran outside
            let ran = std::process::Command::new("/bin/sh").args(["-c", line]).current_dir(&dir).output().expect("ran");
            let out = String::from_utf8_lossy(&ran.stdout).to_string();
            let (spec, got) = out.split_once('\n').unwrap_or_else(|| panic!("{line} said {out}"));
            assert_eq!(serde_json::from_str::<Value>(spec).expect("spec"), serde_json::from_str::<Value>(&first).expect("first"), "{line}");
            assert_eq!(got, command, "{line}");
            assert!(!dir.join("ran").exists(), "{line}");
        }
        assert!(!dir.join(BOX_FAILED).exists());
        let _gone = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_its_box_a_bash_call_is_refused_and_the_turn_is_jaynes_s_fault() {
        let dir = std::env::temp_dir().join(format!("jaynes-nobox-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let input = json!({"command": "echo hi", "dangerouslyDisableSandbox": true});
        let said = wrapped(&BoxSpec::load(&dir.join("box.json")), &input);
        assert_eq!(said["behavior"], "deny", "{said}");
        assert!(said.get("updatedInput").is_none());
        assert!(std::fs::read_to_string(dir.join(BOX_FAILED)).expect("kept").contains("could not read the box"));
        // a box that names no interpreter is no box either
        std::fs::write(dir.join("box.json"), "{}").expect("write");
        assert_eq!(wrapped(&BoxSpec::load(&dir.join("box.json")), &input)["behavior"], "deny");
        let _gone = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_turn_that_may_ask_nothing_boxes_its_commands_and_refuses_the_rest_without_the_record() {
        let dir = std::env::temp_dir().join(format!("jaynes-wraponly-{}", std::process::id()));
        let boxed = box_file(&dir, Path::new("/nowhere/python3"));
        // a home with no record: a call that reached it would say so
        let p = Permit { home: dir.join("no-home"), run: String::new(), step: String::new(), attempt: 0, cwd: PathBuf::new(), boxed: Some(BoxSpec::load(&boxed)), wrap_only: true };
        assert_eq!(p.decide(&json!({"tool_name": "Bash", "input": {"command": "ls"}}))["behavior"], "allow");
        let other = p.decide(&json!({"tool_name": "Write", "input": {"file_path": "/x", "content": "y"}}));
        assert_eq!(other["behavior"], "deny");
        assert!(!other["message"].as_str().unwrap_or("").contains("record"), "{other}");
        assert!(!dir.join("no-home").exists());
        let _gone = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_read_inside_the_step_s_folder_is_relative() {
        let a = ask_of("Read", &json!({"file_path": "/w/src/a.rs"}), Path::new("/w"));
        assert_eq!((a.kind, a.target.as_str()), (AskKind::Read, "src/a.rs"));
        let b = ask_of("Read", &json!({"file_path": "/etc/hosts"}), Path::new("/w"));
        assert_eq!(b.target, "/etc/hosts");
        let c = ask_of("WebFetch", &json!({"url": "https://example.com"}), Path::new("/w"));
        assert_eq!(c.kind, AskKind::Network);
    }

    /// The engineer lead's ask (22/11), after the real-user almost allowed a write into the real project
    /// (tidepool/test/screens.test.js): Claude Code's file tools run outside the box, so a write the person allows
    /// outside the step's folder lands on the Mac, with no merge and no Undo. Jaynes refuses it at once, the target
    /// resolved through its `..` and its links, in the design-lead's words (permissao.md).
    #[test]
    fn asked_a_write_outside_the_step_s_folder_is_refused_wherever_its_path_leads() {
        let base = std::env::temp_dir().join(format!("jaynes-outside-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let (project, cwd, elsewhere) = (base.join("tidepool"), base.join("runs/r/project"), base.join("elsewhere"));
        for d in [project.join("test"), cwd.join("test"), elsewhere.clone()] {
            std::fs::create_dir_all(&d).expect("dir");
        }
        std::fs::write(project.join("test/screens.test.js"), "real\n").expect("write");
        std::fs::write(cwd.join("test/screens.test.js"), "copy\n").expect("write");
        std::os::unix::fs::symlink(&elsewhere, cwd.join("out")).expect("link");
        std::os::unix::fs::symlink(&elsewhere, cwd.join("test/up")).expect("link");
        let real = |p: &Path| std::fs::canonicalize(p).expect("real");
        let write = |path: &str| refusal("Write", &json!({"file_path": path, "content": "x"}), &cwd, Some(&project));
        let (c, f) = (cwd.display().to_string(), real(&base).display().to_string());
        // the file of the person's project whose same file is in the copy: the agent is told where it is
        let into = project.join("test/screens.test.js").display().to_string();
        assert_eq!(write(&into).as_deref(), Some(format!(
            "Jaynes refused this write: {f}/tidepool/test/screens.test.js is outside this step's folder, {c}, and nothing is written outside it. The same file in your copy of the project is {c}/test/screens.test.js: write it there."
        ).as_str()));
        for tool in ["Edit", "MultiEdit"] {
            assert!(refusal(tool, &json!({"file_path": into, "old_string": "a", "new_string": "b"}), &cwd, Some(&project)).is_some(), "{tool}");
        }
        // one the copy has not: only where to write
        let new = project.join("test/new.test.js").display().to_string();
        assert_eq!(write(&new).as_deref(), Some(format!(
            "Jaynes refused this write: {f}/tidepool/test/new.test.js is outside this step's folder, {c}, and nothing is written outside it. Write only under {c}."
        ).as_str()));
        // up and out with .., a link of the copy that leads out, and a link followed by ..
        for (path, lands) in [
            (format!("{c}/../../../elsewhere/a.txt"), format!("{f}/elsewhere/a.txt")),
            ("../outside.txt".to_string(), format!("{f}/runs/r/outside.txt")),
            (format!("{c}/out/a.txt"), format!("{f}/elsewhere/a.txt")),
            ("out/a.txt".to_string(), format!("{f}/elsewhere/a.txt")),
            (format!("{c}/test/up/../escaped.txt"), format!("{f}/escaped.txt")),
        ] {
            let said = write(&path).unwrap_or_else(|| panic!("{path} was let through"));
            assert!(said.starts_with(&format!("Jaynes refused this write: {lands} is outside this step's folder, {c},")), "{path}: {said}");
        }
        // inside the step's folder, as given or by a path that comes back into it, nothing is refused
        for path in [format!("{c}/test/screens.test.js"), "test/a.js".to_string(), format!("{c}/test/../b.js"), format!("{}/test/c.js", real(&cwd).display())] {
            assert_eq!(write(&path), None, "{path}");
        }
        assert_eq!(refusal("Read", &json!({"file_path": into}), &cwd, Some(&project)), None, "a read is not a write");
        // without a project, the same words without the copy's file
        assert!(refusal("Write", &json!({"file_path": into}), &cwd, None).is_some_and(|s| s.ends_with(&format!("Write only under {c}."))));
        // the story reads back what was refused, and where the agent was told to write
        let said = write(&into).expect("refused");
        assert_eq!(refused_said(&said), Some((format!("{f}/tidepool/test/screens.test.js"), Some("test/screens.test.js".to_string()))));
        assert_eq!(refused_said(&write(&new).expect("refused")), Some((format!("{f}/tidepool/test/new.test.js"), None)));
        assert_eq!(refused_said("the person refused leave to write a.txt. Go on without it."), None);
        let _gone = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn what_the_agent_wants_to_write_is_the_file_as_it_would_leave_it() {
        let cwd = std::env::temp_dir().join(format!("jaynes-wants-{}", std::process::id()));
        std::fs::create_dir_all(cwd.join("tests")).expect("dir");
        std::fs::write(cwd.join("tests/a.py"), "x = 1\ny = 1\n").expect("write");
        let at = |f: &str| cwd.join(f).display().to_string();
        let new = wants_of("Write", &json!({"file_path": at("tests/conftest.py"), "content": "import pytest\n"}), &cwd).expect("wants");
        assert_eq!(new, crate::stop::Wants { file: "tests/conftest.py".into(), before: None, after: "import pytest\n".into() });
        let edit = wants_of("Edit", &json!({"file_path": at("tests/a.py"), "old_string": "1", "new_string": "2"}), &cwd).expect("wants");
        assert_eq!((edit.before.as_deref(), edit.after.as_str()), (Some("x = 1\ny = 1\n"), "x = 2\ny = 1\n"));
        let all = wants_of("Edit", &json!({"file_path": at("tests/a.py"), "old_string": "1", "new_string": "2", "replace_all": true}), &cwd).expect("wants");
        assert_eq!(all.after, "x = 2\ny = 2\n");
        let multi = wants_of("MultiEdit", &json!({"file_path": at("tests/a.py"), "edits": [{"old_string": "x", "new_string": "a"}, {"old_string": "y", "new_string": "b"}]}), &cwd).expect("wants");
        assert_eq!(multi.after, "a = 1\nb = 1\n");
        assert_eq!(wants_of("Read", &json!({"file_path": at("tests/a.py")}), &cwd), None);
        let _gone = std::fs::remove_dir_all(&cwd);
    }
}
