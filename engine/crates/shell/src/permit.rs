//! The leave server: what Claude Code asks while a turn runs, when it wants something its settings did not allow.
//!
//! Claude Code starts it as an MCP server (`--permission-prompt-tool mcp__jaynes__permit`). Each request becomes a
//! `PermissionAsked` for the core, recorded at once: a rule in the core grants a routine leave (reading inside the
//! project), and anything else opens a question for the person on the page. The server waits for the answer in
//! the record and replies allow or deny. A question nobody answers before the turn's deadline is a deny.

use crate::drive::say;
use crate::store::Store;
use jaynes_core::event::Event;
use jaynes_core::question::{Ask, AskKind, Verdict};
use jaynes_core::Input;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct Permit {
    pub home: PathBuf,
    pub run: String,
    pub step: String,
    pub attempt: u32,
    pub cwd: PathBuf,
    pub wait: Duration,
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
        say(&format!("? leave asked at {}: {} {}", self.step, tool, ask.target));
        let started = Instant::now();
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
            if started.elapsed() > self.wait {
                return Self::deny("nobody answered in time. Go on without it.");
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
}
