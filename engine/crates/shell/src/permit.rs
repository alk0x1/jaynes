//! The leave server: what Claude Code asks while a turn runs, when it wants something its settings did not allow.
//!
//! Claude Code starts it as an MCP server (`--permission-prompt-tool mcp__jaynes__permit`). Each request becomes a
//! `PermissionAsked` for the core, recorded at once: a rule in the core grants a routine leave (reading inside the
//! project), and anything else opens a question for the person on the page. The server waits for the answer in
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
        let ask = ask_of(tool, &input, &self.cwd);
        let mut store = match Store::open(&self.home.join("record.sqlite3")) {
            Ok(s) => s,
            Err(e) => return Self::deny(format!("Jaynes could not read its record: {e}")),
        };
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

    #[test]
    fn a_read_inside_the_step_s_folder_is_relative() {
        let a = ask_of("Read", &json!({"file_path": "/w/src/a.rs"}), Path::new("/w"));
        assert_eq!((a.kind, a.target.as_str()), (AskKind::Read, "src/a.rs"));
        let b = ask_of("Read", &json!({"file_path": "/etc/hosts"}), Path::new("/w"));
        assert_eq!(b.target, "/etc/hosts");
        let c = ask_of("WebFetch", &json!({"url": "https://example.com"}), Path::new("/w"));
        assert_eq!(c.kind, AskKind::Network);
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
