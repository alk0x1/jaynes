//! Opening or closing a project's local ports or its network (`jaynes project open-ports`, `close-ports`,
//! `open-network`, `close-network`) is the person's alone, at a terminal of their own. The command line refuses a
//! program without a terminal, but an agent can give the command one (`script -q /dev/null jaynes ...`), so the guard
//! refuses these commands in an agent's session: its first deny, never an ask, and in every form it reads, with the
//! guard on or off, here or in a folder where the person turned it off. Reading the choices (`jaynes project ports`)
//! is not refused, and neither are words that only talk about the commands: a search, a commit message, a pull
//! request's text (which the guard may still ask about for its own reasons).
//! Run as Claude Code runs it, with a home of its own.

use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().expect("repo")
}

struct World {
    base: PathBuf,
}

impl Drop for World {
    fn drop(&mut self) {
        let _gone = std::fs::remove_dir_all(&self.base);
    }
}

impl World {
    /// A project folder and the person's guard settings.
    fn new(name: &str, settings: Value) -> World {
        let base = std::env::temp_dir().join(format!("jaynes-project-reach-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("project")).expect("dir");
        std::fs::create_dir_all(base.join("jaynes")).expect("dir");
        let settings = match settings {
            Value::Object(mut map) => {
                if map.remove("off_here").is_some() {
                    map.insert("off_in".into(), json!([base.join("project")]));
                }
                Value::Object(map)
            }
            other => other,
        };
        std::fs::write(base.join("jaynes/guard.json"), settings.to_string()).expect("write");
        World { base }
    }

    fn bash(&self, command: &str) -> Option<Value> {
        let mut child = Command::new("python3")
            .arg(repo().join("hooks/guard.py"))
            .env("JAYNES_HOME", self.base.join("jaynes")).env("GH_CONFIG_DIR", self.base.join("gh")).env("CLAUDE_PLUGIN_ROOT", repo())
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().expect("guard");
        let event = json!({"session_id": "s", "hook_event_name": "PreToolUse", "tool_name": "Bash", "cwd": self.base.join("project"), "tool_input": {"command": command}});
        child.stdin.take().expect("stdin").write_all(event.to_string().as_bytes()).expect("event");
        let done = child.wait_with_output().expect("guard");
        assert!(done.status.success() && done.stderr.is_empty(), "{}", String::from_utf8_lossy(&done.stderr));
        let printed = String::from_utf8_lossy(&done.stdout).trim().to_string();
        (!printed.is_empty()).then(|| serde_json::from_str(&printed).expect("json"))
    }
}

fn decision(said: &Option<Value>) -> Option<String> {
    said.as_ref().map(|v| v["hookSpecificOutput"]["permissionDecision"].as_str().unwrap_or("").to_string())
}

fn why(said: &Option<Value>) -> String {
    said.as_ref().map(|v| v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap_or("").to_string()).unwrap_or_default()
}

/// Every form an agent's command may take that the guard reads.
const REFUSED: &[&str] = &[
    "jaynes project open-ports .",
    "jaynes project close-ports .",
    "jaynes project open-network .",
    "jaynes project close-network .",
    "/Users/someone/.local/bin/jaynes project open-network /Users/someone/thing",
    "env JAYNES_HOME=/tmp/x jaynes project open-ports .",
    "cd /tmp && jaynes project open-network .",
    "sh -c 'jaynes project open-ports .'",
    "bash -lc \"jaynes project open-network .\"",
    "eval jaynes project open-ports .",
    "script -q /dev/null jaynes project open-ports .",
    "script -qc 'jaynes project open-network .' /dev/null",
    "timeout 5 jaynes project open-ports .",
    "nice -n 5 jaynes project close-network .",
    "J=jaynes; $J project open-ports .",
    "\"$(command -v jaynes)\" project open-network .",
    "cargo run -q -p jaynes-shell --bin jaynes -- project open-ports .",
    "python3 -c \"import subprocess; subprocess.run(['jaynes', 'project', 'open-network', '.'])\"",
    "echo 'jaynes project open-ports .' | sh",
    "ls && jaynes project open-ports . && echo done",
];

#[test]
fn an_agent_is_refused_opening_or_closing_a_project_s_ports_or_network_in_every_form() {
    let w = World::new("forms", json!({}));
    for command in REFUSED {
        let said = w.bash(command);
        assert_eq!(decision(&said).as_deref(), Some("deny"), "{command}: {said:?}");
        let why = why(&said);
        assert!(why.contains("your own terminal") && why.contains("jaynes project"), "{command}: {why}");
    }
}

#[test]
fn the_refusal_names_the_command_the_person_runs_and_says_it_is_theirs() {
    let w = World::new("reason", json!({}));
    let why = why(&w.bash("jaynes project open-network ."));
    assert!(why.contains("jaynes project open-network"), "{why}");
    assert!(why.contains("the person's alone") || why.contains("yours alone"), "{why}");
    assert!(why.contains("network"), "{why}");
    let why = why_of(&w, "script -q /dev/null jaynes project close-ports .");
    assert!(why.contains("jaynes project close-ports"), "{why}");
}

fn why_of(w: &World, command: &str) -> String {
    why(&w.bash(command))
}

#[test]
fn the_refusal_holds_with_the_guard_off_and_where_the_person_turned_it_off() {
    for (name, settings) in [("off", json!({"on": false})), ("watching", json!({"on": true, "watch": true})), ("off-here", json!({"off_here": true}))] {
        let w = World::new(name, settings);
        for command in ["jaynes project open-ports .", "script -q /dev/null jaynes project open-network ."] {
            assert_eq!(decision(&w.bash(command)).as_deref(), Some("deny"), "{name}: {command}");
        }
    }
}

#[test]
fn reading_the_choices_and_talking_about_the_commands_pass() {
    let w = World::new("pass", json!({}));
    for command in [
        "jaynes project ports .",
        "jaynes project ports /Users/someone/thing",
        "grep -rn 'project open-ports' engine docs",
        "rg 'jaynes project open-network' .",
        "git commit -q -m 'jaynes project open-network DIR lets the network reach the checks'",
        "git log --grep 'project open-ports'",
        "gh pr create --title 'jaynes project close-network' --body 'run jaynes project open-ports at a terminal'",
        "cargo test -p jaynes-shell --test e2e open_ports",
        "cat docs/where-your-data-goes.md",
    ] {
        let said = w.bash(command);
        assert_ne!(decision(&said).as_deref(), Some("deny"), "{command}: {said:?}");
    }
}
