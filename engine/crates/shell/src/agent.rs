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

pub struct Spec<'a> {
    pub cwd: &'a Path,
    pub prompt: &'a str,
    pub session: Option<&'a str>,
    pub reach: &'a Reach,
    pub deadline: Duration,
    /// Folders the step's commands must read outside its folder, such as linked dependencies.
    pub reads: &'a [PathBuf],
    pub env: Vec<(String, String)>,
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

/// Run a program with `input` on its standard input, killed with its whole group at the deadline.
fn run(mut cmd: Command, input: &str, deadline: Duration) -> Result<(Option<i32>, String, String, Duration, bool), String> {
    use std::os::unix::process::CommandExt;
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|e| format!("the agent program did not start: {e}"))?;
    let pid = child.id();
    if let Some(mut stdin) = child.stdin.take() {
        let _wrote = stdin.write_all(input.as_bytes());
    }
    let mut stdout = child.stdout.take().ok_or("no output pipe")?;
    let mut stderr = child.stderr.take().ok_or("no error pipe")?;
    let out = std::thread::spawn(move || {
        let mut s = String::new();
        let _read = stdout.read_to_string(&mut s);
        s
    });
    let err = std::thread::spawn(move || {
        let mut s = String::new();
        let _read = stderr.read_to_string(&mut s);
        s
    });
    let mut timed_out = false;
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if started.elapsed() > deadline => {
                timed_out = true;
                let _term = Command::new("kill").args(["-TERM", &format!("-{pid}")]).status();
                std::thread::sleep(Duration::from_secs(3));
                let _kill = Command::new("kill").args(["-KILL", &format!("-{pid}")]).status();
                let _waited = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(format!("the agent program could not be watched: {e}")),
        }
    };
    Ok((code, out.join().unwrap_or_default(), err.join().unwrap_or_default(), started.elapsed(), timed_out))
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
        let ended = match run(cmd, spec.prompt, spec.deadline) {
            Err(reason) => Ended::Fault { reason },
            Ok((code, out, err, took, timed_out)) => {
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
    pub fn settings(&self, cwd: &Path, reach: &Reach, reads: &[PathBuf]) -> Value {
        let web = reach.network == Network::Web;
        let mut allow: Vec<String> = vec!["Read(./**)".into()];
        if reach.project == ProjectAccess::Write || reach.project == ProjectAccess::None {
            allow.extend(["Edit(./**)".into(), "Write(./**)".into()]);
        } else {
            allow.extend([format!("Edit(./{}/**)", crate::project::EXCHANGE), format!("Write(./{}/**)", crate::project::EXCHANGE)]);
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
        json!({
            "permissions": {"defaultMode": "dontAsk", "allow": allow, "deny": deny,
                            "blockReadsOutsideWorkingDirectories": true, "disableBypassPermissionsMode": "disable"},
            "sandbox": {"enabled": true, "allowUnsandboxedCommands": false, "failIfUnavailable": true,
                        "autoAllowBashIfSandboxed": true,
                        "network": {"allowedDomains": [], "strictAllowlist": true, "allowLocalBinding": true},
                        "filesystem": {"denyRead": ["~/"], "allowRead": read,
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
        if cfg!(target_os = "macos") && !Path::new("/usr/bin/sandbox-exec").exists() {
            return Err("the macOS sandbox Claude Code's guards need is not there".to_string());
        }
        probe_folder(cwd)
    }

    fn turn(&self, spec: Spec<'_>) -> Turn {
        let settings = self.settings(spec.cwd, spec.reach, spec.reads);
        let Some(claude) = binary("claude") else {
            return Turn { ended: Ended::Fault { reason: "Claude Code is not installed".to_string() }, applied: settings };
        };
        let tmp = spec.cwd.join(crate::project::EXCHANGE).join("tmp");
        let _made = std::fs::create_dir_all(&tmp);
        let mut cmd = Command::new(claude);
        cmd.args(["-p", "--output-format", "stream-json", "--verbose", "--setting-sources", ""])
            .arg("--settings")
            .arg(settings.to_string())
            .args(["--strict-mcp-config", "--mcp-config", r#"{"mcpServers":{}}"#, "--permission-mode", "dontAsk"])
            .args(["--max-turns", &self.max_messages.to_string(), "--model", &self.model, "--effort", &self.effort]);
        if let Some(s) = spec.session {
            cmd.args(["--resume", s]);
        }
        cmd.current_dir(spec.cwd).env("TMPDIR", &tmp).env("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "1");
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        let applied = json!({"program": "claude", "model": self.model, "effort": self.effort, "settings": settings,
                             "resume": spec.session});
        let ended = match run(cmd, spec.prompt, spec.deadline) {
            Err(reason) => Ended::Fault { reason },
            Ok((code, out, err, took, timed_out)) => claude_ended(&self.model, code, &out, &err, took, timed_out),
        };
        Turn { ended, applied }
    }
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
    let answered = result["modelUsage"]
        .as_object()
        .and_then(|m| m.iter().max_by_key(|(_, v)| v["outputTokens"].as_u64().unwrap_or(0)).map(|(k, _)| k.clone()))
        .unwrap_or_else(|| model.to_string());
    if !answered.contains(model) {
        return Ended::Fault { reason: format!("asked for {model} and {answered} answered; this turn does not count") };
    }
    Ended::Ran { usage, session, model: answered }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_only_step_writes_only_its_exchange_folder() {
        let reach = Reach { project: ProjectAccess::Read, write: vec![], network: Network::None, commands: false };
        let s = Claude::default().settings(Path::new("/w"), &reach, &[]);
        let allow: Vec<&str> = s["permissions"]["allow"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        assert!(allow.contains(&"Edit(./.jaynes/**)"));
        assert!(!allow.contains(&"Edit(./**)"));
        assert!(s["permissions"]["deny"].as_array().into_iter().flatten().any(|v| v == "Bash"));
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
}
