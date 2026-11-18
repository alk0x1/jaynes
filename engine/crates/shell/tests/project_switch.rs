//! A project's own switch, `.jaynes.toml` at its root: it may turn Jaynes's conducting part off there and make the
//! guard stricter, never turn the guard off. Turning the guard off in a project is the person's alone, in their
//! settings (guard.json "off_in"), and the guard asks before any edit of a project's .jaynes.toml. Run as Claude Code
//! runs them: the hooks through python3 with an event on standard input, the MCP server through its program.

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

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
    /// A project in git with a GitHub remote of `owner`, gh signed in as "me", and the project's switch if given.
    fn new(name: &str, owner: &str, switch: Option<&str>) -> World {
        let base = std::env::temp_dir().join(format!("jaynes-switch-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let project = base.join("project");
        std::fs::create_dir_all(project.join("src/deep")).expect("dir");
        std::fs::create_dir_all(base.join("gh")).expect("dir");
        std::fs::write(base.join("gh/hosts.yml"), "github.com:\n    user: me\n").expect("write");
        for args in [vec!["init", "-q"], vec!["remote", "add", "origin", &format!("https://github.com/{owner}/thing.git")]] {
            assert!(Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success());
        }
        if let Some(text) = switch {
            std::fs::write(project.join(".jaynes.toml"), text).expect("write");
        }
        World { base }
    }

    fn project(&self) -> PathBuf {
        self.base.canonicalize().expect("base").join("project")
    }

    fn home(&self) -> PathBuf {
        self.base.join("jaynes")
    }

    fn settings(&self, text: &str) {
        std::fs::create_dir_all(self.home()).expect("dir");
        std::fs::write(self.home().join("guard.json"), text).expect("write");
    }

    fn hook(&self, script: &str, event: &Value) -> Option<Value> {
        let mut child = Command::new("python3")
            .arg(repo().join("hooks").join(script))
            .env("JAYNES_HOME", self.home()).env("GH_CONFIG_DIR", self.base.join("gh")).env("CLAUDE_PLUGIN_ROOT", repo()).env_remove("JAYNES_OFF")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().expect("hook");
        child.stdin.take().expect("stdin").write_all(event.to_string().as_bytes()).expect("event");
        let done = child.wait_with_output().expect("hook");
        assert!(done.status.success() && done.stderr.is_empty(), "{script}: {}", String::from_utf8_lossy(&done.stderr));
        let printed = String::from_utf8_lossy(&done.stdout).trim().to_string();
        (!printed.is_empty()).then(|| serde_json::from_str(&printed).expect("json"))
    }

    fn session(&self, cwd: &Path) -> Option<Value> {
        self.hook("session.py", &json!({"session_id": "s", "hook_event_name": "SessionStart", "source": "startup", "cwd": cwd}))
    }

    fn guard(&self, cwd: &Path, tool: &str, input: Value) -> Option<Value> {
        self.hook("guard.py", &json!({"session_id": "s", "hook_event_name": "PreToolUse", "tool_name": tool, "cwd": cwd, "tool_input": input}))
    }
}

fn asks(said: &Option<Value>) -> bool {
    said.as_ref().is_some_and(|v| v["hookSpecificOutput"]["permissionDecision"] == "ask")
}

fn person(said: &Option<Value>) -> String {
    said.as_ref().and_then(|v| v["systemMessage"].as_str()).unwrap_or("").to_string()
}

fn push() -> Value {
    json!({"command": "git push origin main"})
}

#[test]
fn a_project_turns_the_conducting_part_off_and_the_session_says_so_from_any_folder_in_it() {
    let w = World::new("conduct", "someone-else", Some("# Jaynes stands aside here\nconduct = false\n"));
    for cwd in [w.project(), w.project().join("src/deep")] {
        let said = w.session(&cwd);
        let says = person(&said);
        assert!(says.contains("Jaynes is off in this project") && says.contains(".jaynes.toml") && says.contains("guard is on"), "{said:?}");
        let model = said.as_ref().map(|v| v["hookSpecificOutput"]["additionalContext"].to_string()).unwrap_or_default();
        assert!(model.contains("do not use the jaynes skill"), "{said:?}");
    }
    // the guard is not turned off with it
    assert!(asks(&w.guard(&w.project(), "Bash", push())));
}

#[test]
fn a_project_that_says_guard_false_keeps_the_guard_on_and_is_told_why() {
    let w = World::new("cloned", "someone-else", Some("guard = false\n"));
    assert!(asks(&w.guard(&w.project(), "Bash", push())), "a project's file turned the guard off");
    let says = person(&w.session(&w.project()));
    assert!(says.contains("guard = false") && says.contains("Setup"), "{says}");
}

#[test]
fn an_agent_that_writes_the_switch_is_asked_first_and_gains_nothing() {
    let w = World::new("agent", "someone-else", None);
    let file = w.project().join(".jaynes.toml");
    for (tool, input) in [
        ("Write", json!({"file_path": file, "content": "guard = false\n"})),
        ("Edit", json!({"file_path": file, "old_string": "a", "new_string": "b"})),
        ("Bash", json!({"command": "echo 'guard = false' > .jaynes.toml"})),
        ("Bash", json!({"command": "cp /tmp/x src/../.jaynes.toml"})),
        ("Bash", json!({"command": "rm .jaynes.toml"})),
    ] {
        assert!(asks(&w.guard(&w.project(), tool, input.clone())), "{tool} {input}");
    }
    // and had it written anyway, the guard is on
    std::fs::write(&file, "guard = false\n").expect("write");
    assert!(asks(&w.guard(&w.project(), "Bash", push())));
}

#[test]
fn a_strict_project_asks_even_for_the_person_s_own_repository() {
    let own = World::new("own", "me", None);
    assert!(!asks(&own.guard(&own.project(), "Bash", push())), "the person's own repository passes by default");
    let strict = World::new("strict", "me", Some("guard = \"strict\"\n"));
    let said = strict.guard(&strict.project(), "Bash", push());
    assert!(asks(&said), "{said:?}");
    let why = said.as_ref().map(|v| v["hookSpecificOutput"]["permissionDecisionReason"].to_string()).unwrap_or_default();
    assert!(why.contains(".jaynes.toml"), "{why}");
}

#[test]
fn only_the_person_s_settings_turn_the_guard_off_in_a_project() {
    let w = World::new("person", "someone-else", None);
    w.settings(&json!({"off_in": [w.project()]}).to_string());
    for cwd in [w.project(), w.project().join("src/deep")] {
        assert!(!asks(&w.guard(&cwd, "Bash", push())), "{}", cwd.display());
    }
    assert!(person(&w.session(&w.project())).contains("guard is off in this project"));
    // another project is still guarded
    let other = World::new("person-other", "someone-else", None);
    other.settings(&json!({"off_in": [w.project()]}).to_string());
    assert!(asks(&other.guard(&other.project(), "Bash", push())));
}

#[test]
fn a_switch_above_the_project_s_repository_is_not_the_project_s() {
    let w = World::new("above", "someone-else", None);
    std::fs::write(w.base.join(".jaynes.toml"), "conduct = false\n").expect("write");
    assert_eq!(w.session(&w.project()), None, "a file outside the repository spoke for it");
}

#[test]
fn the_mcp_server_starts_no_run_in_a_project_that_turns_jaynes_off() {
    let w = World::new("mcp", "someone-else", Some("conduct = false\n"));
    let mut child = Command::new(BIN)
        .arg("mcp").env("JAYNES_ENGINE_HOME", w.home()).env("JAYNES_NO_PAGE", "1").env_remove("JAYNES_OFF")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("mcp");
    let mut out = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    let stdin = child.stdin.as_mut().expect("stdin");
    let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "start", "arguments": {
        "role": "small-change", "project": w.project().join("src"), "request": "say hi"}}});
    writeln!(stdin, "{call}").expect("write");
    stdin.flush().expect("flush");
    let mut line = String::new();
    out.read_line(&mut line).expect("read");
    let _killed = child.kill();
    let _reaped = child.wait();
    let said: Value = serde_json::from_str(&line).expect("json");
    let text = said["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert_eq!(said["result"]["isError"], true, "{said}");
    assert!(text.contains(".jaynes.toml") && text.contains("conduct = false"), "{text}");
    assert!(!w.home().join("runs").exists(), "a run was started");
}
