//! Jaynes turned off in one session: with JAYNES_OFF set, the plugin's conducting part (the skill and the MCP tools)
//! does nothing there, its guard keeps its own switch, and the session says which of the two is on. Run as Claude
//! Code runs them: the MCP server through its program, the hooks through python3 with an event on standard input.

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().expect("repo")
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _gone = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    let base = std::env::temp_dir().join(format!("jaynes-off-{name}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).expect("dir");
    Scratch(base)
}

/// What the MCP server answers to initialize, tools/list and a call of `home`, with JAYNES_OFF as given.
fn mcp(home: &Path, off: Option<&str>) -> (Value, Value, Value) {
    let mut cmd = Command::new(BIN);
    cmd.arg("mcp").env("JAYNES_ENGINE_HOME", home).env("JAYNES_NO_PAGE", "1").env_remove("JAYNES_OFF");
    if let Some(v) = off {
        cmd.env("JAYNES_OFF", v);
    }
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().expect("mcp");
    let mut out = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    let mut said = Vec::new();
    for (n, (method, params)) in [
        ("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}})),
        ("tools/list", json!({})),
        ("tools/call", json!({"name": "home", "arguments": {}})),
    ]
    .into_iter()
    .enumerate()
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        writeln!(stdin, "{}", json!({"jsonrpc": "2.0", "id": n + 1, "method": method, "params": params})).expect("write");
        stdin.flush().expect("flush");
        let mut line = String::new();
        out.read_line(&mut line).expect("read");
        said.push(serde_json::from_str::<Value>(&line).expect("json"));
    }
    let _killed = child.kill();
    let _reaped = child.wait();
    let mut said = said.into_iter();
    (said.next().expect("init"), said.next().expect("list"), said.next().expect("call"))
}

/// A hook of the plugin, run as Claude Code runs it: what it printed, parsed when it printed anything.
fn hook(script: &str, home: &Path, off: Option<&str>, event: &Value) -> Option<Value> {
    let mut cmd = Command::new("python3");
    cmd.arg(repo().join("hooks").join(script)).env("JAYNES_HOME", home).env("CLAUDE_PLUGIN_ROOT", repo()).env_remove("JAYNES_OFF");
    if let Some(v) = off {
        cmd.env("JAYNES_OFF", v);
    }
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("hook");
    child.stdin.take().expect("stdin").write_all(event.to_string().as_bytes()).expect("event");
    let done = child.wait_with_output().expect("hook");
    assert!(done.status.success(), "{script}: {}", String::from_utf8_lossy(&done.stderr));
    assert!(done.stderr.is_empty(), "{script} said: {}", String::from_utf8_lossy(&done.stderr));
    let printed = String::from_utf8_lossy(&done.stdout).trim().to_string();
    (!printed.is_empty()).then(|| serde_json::from_str(&printed).unwrap_or_else(|e| panic!("{script} printed no JSON ({e}): {printed}")))
}

fn session_start() -> Value {
    json!({"session_id": "s", "hook_event_name": "SessionStart", "source": "startup", "cwd": "/tmp"})
}

fn files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
    names.sort();
    names
}

#[test]
fn with_jaynes_off_the_mcp_server_offers_no_tool_and_says_why() {
    let s = scratch("mcp");
    for off in ["1", "true", "yes"] {
        let (init, list, call) = mcp(&s.0, Some(off));
        let instructions = init["result"]["instructions"].as_str().unwrap_or("");
        assert!(instructions.contains("off in this session") && instructions.contains("JAYNES_OFF"), "{off}: {instructions}");
        assert_eq!(list["result"]["tools"], json!([]), "{off}");
        assert_eq!(call["result"]["isError"], true, "{off}: {call}");
        assert!(call["result"]["content"][0]["text"].as_str().unwrap_or("").contains("JAYNES_OFF"), "{call}");
    }
    // unset, empty or 0, the tools are there
    for off in [None, Some(""), Some("0"), Some("false")] {
        let (_, list, call) = mcp(&s.0, off);
        assert_eq!(list["result"]["tools"].as_array().map(Vec::len), Some(7), "{off:?}");
        assert_eq!(call["result"]["isError"], false, "{off:?}: {call}");
    }
}

#[test]
fn the_session_says_what_is_off_and_nothing_when_all_is_on() {
    let s = scratch("session");
    let home = s.0.join("jaynes");
    // all on: nothing said, no token spent
    assert_eq!(hook("session.py", &home, None, &session_start()), None);
    assert_eq!(hook("session.py", &home, Some("0"), &session_start()), None);
    // the conducting part off: the person reads it, and so does the model, and the guard is said to be on
    let said = hook("session.py", &home, Some("1"), &session_start()).expect("said");
    let person = said["systemMessage"].as_str().unwrap_or("");
    assert!(person.contains("Jaynes is off in this session") && person.contains("guard is on"), "{said}");
    assert_eq!(said["hookSpecificOutput"]["hookEventName"], "SessionStart", "{said}");
    let model = said["hookSpecificOutput"]["additionalContext"].as_str().unwrap_or("");
    assert!(model.contains("do not use the jaynes skill") && model.contains("yourself"), "{said}");
    // the guard off by its own switch: said, and the conducting part is not
    std::fs::create_dir_all(&home).expect("dir");
    std::fs::write(home.join("guard.json"), r#"{"on": false}"#).expect("write");
    let said = hook("session.py", &home, None, &session_start()).expect("said");
    let person = said["systemMessage"].as_str().unwrap_or("");
    assert!(person.contains("guard is off") && !person.contains("Jaynes is off"), "{said}");
    assert!(said["hookSpecificOutput"]["additionalContext"].is_null(), "the model needs nothing: {said}");
    // both off
    let said = hook("session.py", &home, Some("1"), &session_start()).expect("said");
    let person = said["systemMessage"].as_str().unwrap_or("");
    assert!(person.contains("Jaynes is off in this session") && person.contains("guard is off"), "{said}");
}

#[test]
fn the_session_hook_writes_nothing_and_reaches_nothing() {
    let s = scratch("quiet");
    let home = s.0.join("jaynes");
    for off in [None, Some("1")] {
        let _said = hook("session.py", &home, off, &session_start());
        assert!(!home.exists(), "{off:?}: it made {}", home.display());
    }
    let source = std::fs::read_to_string(repo().join("hooks/session.py")).expect("read");
    for reach in ["subprocess", "socket", "urllib", "os.system", "makedirs", "mkdir"] {
        assert!(!source.contains(reach), "the session hook holds {reach}");
    }
}

#[test]
fn the_guard_keeps_its_own_switch_when_jaynes_is_off() {
    let s = scratch("guard");
    let home = s.0.join("jaynes");
    std::fs::create_dir_all(&home).expect("dir");
    std::fs::write(home.join("guard.json"), r#"{"owners": ["me"]}"#).expect("write");
    let project = s.0.join("project");
    std::fs::create_dir_all(&project).expect("dir");
    for args in [vec!["init", "-q"], vec!["remote", "add", "origin", "https://github.com/someone-else/thing.git"]] {
        assert!(Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success());
    }
    let push = json!({"session_id": "s", "hook_event_name": "PreToolUse", "tool_name": "Bash", "cwd": project,
                      "tool_input": {"command": "git push --force origin main"}});
    for off in [None, Some("1")] {
        let said = hook("guard.py", &home, off, &push).unwrap_or(Value::Null);
        assert_eq!(said["hookSpecificOutput"]["permissionDecision"], "ask", "{off:?}: {said}");
    }
    assert_eq!(files(&home), ["guard.json"], "the guard wrote nothing new");
}

#[test]
fn the_plugin_wires_the_session_hook_and_the_skill_stands_aside() {
    let hooks: Value = serde_json::from_str(&std::fs::read_to_string(repo().join("hooks/hooks.json")).expect("read")).expect("json");
    let start = hooks["hooks"]["SessionStart"].to_string();
    assert!(start.contains("hooks/session.py"), "{hooks}");
    assert!(hooks["hooks"]["PreToolUse"].to_string().contains("hooks/guard.py"), "{hooks}");
    let skill = std::fs::read_to_string(repo().join("skills/jaynes/SKILL.md")).expect("read");
    let description = skill.lines().find(|l| l.starts_with("description:")).expect("description");
    assert!(description.contains("JAYNES_OFF"), "{description}");
}
