//! Free owners pass only what can be taken back: a plain push, a pull request opened or merged. What cannot be undone
//! asks always, even on the person's own repositories: a forced push, deleting a branch or a tag, throwing work away,
//! deleting or archiving a repository. An owner not yet free is asked about with a line saying how to make it free
//! from now on, in the page's Setup, which only the person writes; the guard asks before an agent edits its settings.
//! Run as Claude Code runs it, with a gh on the PATH that answers nothing and a home of its own.

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
    /// A repository whose remote is github.com/OWNER/thing, with the person's settings naming `owners` free.
    fn new(name: &str, owner: &str, owners: &[&str]) -> World {
        let base = std::env::temp_dir().join(format!("jaynes-owners-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("bin")).expect("dir");
        std::fs::write(base.join("bin/gh"), "#!/bin/sh\nexit 1\n").expect("write");
        std::fs::set_permissions(base.join("bin/gh"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
        std::fs::create_dir_all(base.join("jaynes")).expect("dir");
        std::fs::write(base.join("jaynes/guard.json"), json!({"owners": owners}).to_string()).expect("write");
        let project = base.join("project");
        std::fs::create_dir_all(&project).expect("dir");
        for args in [vec!["init", "-q"], vec!["remote", "add", "origin", &format!("https://github.com/{owner}/thing.git")]] {
            assert!(Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success());
        }
        World { base }
    }

    fn settings(&self) -> PathBuf {
        self.base.join("jaynes/guard.json")
    }

    fn guard(&self, tool: &str, input: Value) -> Option<Value> {
        let path = format!("{}:{}", self.base.join("bin").display(), std::env::var("PATH").unwrap_or_default());
        let mut child = Command::new("python3")
            .arg(repo().join("hooks/guard.py"))
            .env("PATH", path).env("JAYNES_HOME", self.base.join("jaynes")).env("GH_CONFIG_DIR", self.base.join("gh")).env("CLAUDE_PLUGIN_ROOT", repo())
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().expect("guard");
        let event = json!({"session_id": "s", "hook_event_name": "PreToolUse", "tool_name": tool, "cwd": self.base.join("project"), "tool_input": input});
        child.stdin.take().expect("stdin").write_all(event.to_string().as_bytes()).expect("event");
        let done = child.wait_with_output().expect("guard");
        assert!(done.status.success() && done.stderr.is_empty(), "{}", String::from_utf8_lossy(&done.stderr));
        let printed = String::from_utf8_lossy(&done.stdout).trim().to_string();
        (!printed.is_empty()).then(|| serde_json::from_str(&printed).expect("json"))
    }

    fn bash(&self, command: &str) -> Option<Value> {
        self.guard("Bash", json!({"command": command}))
    }
}

fn asks(said: &Option<Value>) -> bool {
    said.as_ref().is_some_and(|v| v["hookSpecificOutput"]["permissionDecision"] == "ask")
}

fn why(said: &Option<Value>) -> String {
    said.as_ref().map(|v| v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap_or("").to_string()).unwrap_or_default()
}

#[test]
fn an_owner_not_yet_free_is_asked_about_with_how_to_make_it_free_in_setup() {
    let w = World::new("unlisted", "acme", &["me"]);
    let said = w.bash("git push origin main");
    assert!(asks(&said), "{said:?}");
    let why = why(&said);
    assert!(why.contains("acme") && why.contains("Setup") && why.contains("http://localhost:8750"), "{why}");
    assert!(!why.contains("jaynes guard"), "no command an agent could run itself: {why}");
    // the line comes every time, and the guard writes nothing to say it
    assert!(why_again(&w).contains("Setup"));
    assert_eq!(std::fs::read_to_string(w.settings()).expect("read"), json!({"owners": ["me"]}).to_string());
}

fn why_again(w: &World) -> String {
    why(&w.bash("git push origin main"))
}

#[test]
fn a_free_owner_passes_a_plain_push_and_pull_requests_opened_or_merged() {
    let w = World::new("free", "acme", &["acme"]);
    for command in ["git push origin main", "git push", "git push -u origin feature", "gh pr create --fill", "gh pr merge 12 --squash"] {
        assert!(!asks(&w.bash(command)), "{command}");
    }
}

#[test]
fn what_cannot_be_undone_asks_always_even_on_a_free_owner_s_repository() {
    let w = World::new("irreversible", "acme", &["acme"]);
    for command in [
        "git push --force origin main",
        "git push -f origin main",
        "git push --force-with-lease origin main",
        "git push origin +main",
        "git push origin +HEAD:main",
        "git push --delete origin feature",
        "git push origin :feature",
        "git push --mirror origin",
        "git reset --hard HEAD~1",
        "git clean -fd",
        "git branch -D feature",
        "git tag -d v1",
        "gh repo delete acme/thing --yes",
        "gh repo archive acme/thing",
        "gh pr close 12",
    ] {
        let said = w.bash(command);
        assert!(asks(&said), "{command}");
        assert!(!why(&said).contains("Setup:"), "making an owner free does not change these: {command}: {}", why(&said));
    }
}

#[test]
fn a_whole_organization_is_free_by_its_name() {
    let w = World::new("org", "acme-corp", &["Acme-Corp"]);
    assert!(!asks(&w.bash("git push origin main")), "an organization in the list frees its repositories, whatever its capitals");
}

#[test]
fn an_agent_is_asked_before_it_changes_the_free_owners() {
    let w = World::new("agent", "acme", &["me"]);
    let settings = w.settings();
    let s = settings.display().to_string();
    for (tool, input) in [
        ("Write", json!({"file_path": settings, "content": "{\"owners\": [\"acme\"]}"})),
        ("Edit", json!({"file_path": settings, "old_string": "me", "new_string": "acme"})),
        ("Bash", json!({"command": format!("echo '{{\"owners\": [\"acme\"]}}' > {s}")})),
        ("Bash", json!({"command": format!("sed -i '' s/me/acme/ {s}")})),
        ("Bash", json!({"command": format!("cp /tmp/x.json {s}")})),
        ("Bash", json!({"command": format!("mv /tmp/x.json {s}")})),
        ("Bash", json!({"command": format!("tee {s} < /tmp/x.json")})),
    ] {
        assert!(asks(&w.guard(tool, input.clone())), "{tool} {input}");
    }
}
