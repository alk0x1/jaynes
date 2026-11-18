//! The guard does nothing where it was not needed: no network and no file written before it has to ask. The person's
//! GitHub user, which passes freely when no owners are set, is read from gh's own hosts file, never asked of GitHub,
//! and never saved. Run as Claude Code runs it, with a gh and a git on the PATH that note every call.

use serde_json::{json, Value};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
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
    fn new(name: &str, gh_user: Option<&str>) -> World {
        let base = std::env::temp_dir().join(format!("jaynes-guard-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let bin = base.join("bin");
        std::fs::create_dir_all(&bin).expect("dir");
        std::fs::create_dir_all(base.join("gh")).expect("dir");
        let calls = base.join("calls.txt");
        let real_git = String::from_utf8(Command::new("sh").args(["-c", "command -v git"]).output().expect("git").stdout).expect("path").trim().to_string();
        for (name, then) in [("gh", "exit 1".to_string()), ("git", format!("exec {real_git} \"$@\""))] {
            let script = format!("#!/bin/sh\necho \"{name} $*\" >> '{}'\n{then}\n", calls.display());
            std::fs::write(bin.join(name), script).expect("write");
            std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        if let Some(user) = gh_user {
            std::fs::write(base.join("gh/hosts.yml"), format!("github.com:\n    git_protocol: https\n    users:\n        {user}:\n    user: {user}\n")).expect("write");
        }
        let project = base.join("project");
        std::fs::create_dir_all(&project).expect("dir");
        assert!(Command::new(&real_git).arg("-C").arg(&project).args(["init", "-q"]).status().expect("git").success());
        World { base }
    }

    fn home(&self) -> PathBuf {
        self.base.join("jaynes")
    }

    fn project(&self) -> PathBuf {
        self.base.join("project")
    }

    fn remote(&self, url: &str) {
        assert!(Command::new("git").arg("-C").arg(self.project()).args(["remote", "add", "origin", url]).status().expect("git").success());
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.base.join("calls.txt")).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default()
    }

    fn forget_calls(&self) {
        let _gone = std::fs::remove_file(self.base.join("calls.txt"));
    }

    /// What the guard answers to a tool call: None when it stays silent.
    fn guard(&self, tool: &str, input: Value) -> Option<Value> {
        let path = format!("{}:{}", self.base.join("bin").display(), std::env::var("PATH").unwrap_or_default());
        let mut child = Command::new("python3")
            .arg(repo().join("hooks/guard.py"))
            .env("PATH", path).env("JAYNES_HOME", self.home()).env("GH_CONFIG_DIR", self.base.join("gh")).env("CLAUDE_PLUGIN_ROOT", repo())
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().expect("guard");
        let event = json!({"session_id": "s", "hook_event_name": "PreToolUse", "tool_name": tool, "cwd": self.project(), "tool_input": input});
        child.stdin.take().expect("stdin").write_all(event.to_string().as_bytes()).expect("event");
        let done = child.wait_with_output().expect("guard");
        assert!(done.status.success() && done.stderr.is_empty(), "{}", String::from_utf8_lossy(&done.stderr));
        let printed = String::from_utf8_lossy(&done.stdout).trim().to_string();
        (!printed.is_empty()).then(|| serde_json::from_str(&printed).expect("json"))
    }
}

fn asks(said: &Option<Value>) -> bool {
    said.as_ref().is_some_and(|v| v["hookSpecificOutput"]["permissionDecision"] == "ask")
}

fn bash(command: &str) -> Value {
    json!({"command": command})
}

#[test]
fn a_push_to_someone_else_s_repository_asks_without_asking_github_or_writing() {
    let w = World::new("other", Some("me"));
    w.remote("https://github.com/someone-else/thing.git");
    let said = w.guard("Bash", bash("git push origin main"));
    assert!(asks(&said), "{said:?}");
    assert!(!w.calls().iter().any(|c| c.starts_with("gh ")), "the guard asked GitHub: {:?}", w.calls());
    assert!(!w.home().exists(), "the guard wrote {}", w.home().display());
}

#[test]
fn a_push_to_the_person_s_own_repository_passes_by_gh_s_own_file() {
    let w = World::new("own", Some("me"));
    w.remote("git@github.com:Me/thing.git");
    let said = w.guard("Bash", bash("git push origin main"));
    assert!(!asks(&said), "the person's own repository: {said:?}");
    assert!(!w.calls().iter().any(|c| c.starts_with("gh ")), "{:?}", w.calls());
    assert!(!w.home().exists(), "the guard wrote {}", w.home().display());
    // a repository created by a bare name lands under that same user, and passes
    let said = w.guard("Bash", bash("gh repo create thing --private"));
    assert!(!asks(&said), "{said:?}");
}

#[test]
fn with_no_gh_user_and_no_owners_a_push_to_github_asks() {
    let w = World::new("nouser", None);
    w.remote("https://github.com/me/thing.git");
    let said = w.guard("Bash", bash("git push origin main"));
    assert!(asks(&said), "{said:?}");
    let why = said.as_ref().map(|v| v["hookSpecificOutput"]["permissionDecisionReason"].to_string()).unwrap_or_default();
    assert!(why.contains("no free owners"), "{why}");
    assert!(!w.calls().iter().any(|c| c.starts_with("gh ")), "{:?}", w.calls());
    assert!(!w.home().exists(), "the guard wrote {}", w.home().display());
}

#[test]
fn owners_set_by_the_person_decide_and_gh_s_user_does_not_add_to_them() {
    let w = World::new("owners", Some("me"));
    std::fs::create_dir_all(w.home()).expect("dir");
    std::fs::write(w.home().join("guard.json"), r#"{"owners": ["team"]}"#).expect("write");
    w.remote("https://github.com/me/thing.git");
    assert!(asks(&w.guard("Bash", bash("git push origin main"))), "only the owners the person set pass");
    let before = std::fs::read_to_string(w.home().join("guard.json")).expect("read");
    assert_eq!(before, r#"{"owners": ["team"]}"#, "the settings are left as the person wrote them");
}

#[test]
fn what_cannot_be_held_runs_no_program_at_all() {
    let w = World::new("plain", Some("me"));
    w.remote("https://github.com/someone-else/thing.git");
    w.forget_calls();
    for (tool, input) in [("Bash", bash("ls -la && cargo test -p x")), ("Bash", bash("git status")), ("Edit", json!({"file_path": "src/a.rs", "old_string": "a", "new_string": "b"}))] {
        assert_eq!(w.guard(tool, input.clone()), None, "{tool} {input}");
    }
    assert_eq!(w.calls(), Vec::<String>::new(), "the guard ran programs for commands it does not hold");
    assert!(!w.home().exists());
}
