//! The guard that only watches: with "watch" in the person's settings, beside "on", what it would ask that can be taken
//! back (a plain push, a pull request opened or merged) it does not ask, and notes instead, one line each in
//! guard-watch.jsonl beside the settings: when, the session, the folder, each action, where it lands and its owner,
//! never the command's words. What cannot be undone still asks, and is not noted. Every action it holds is noted or
//! asked, never neither: a note it cannot write is asked. Its own files still ask. The notes are the person's alone
//! (0600) and keep only the newest 200. Run as Claude Code runs it, with a gh that answers nothing and a home of its own.

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
    /// A repository whose remote is github.com/OWNER/thing, with the person's settings as given.
    fn new(name: &str, owner: &str, settings: Value) -> World {
        let base = std::env::temp_dir().join(format!("jaynes-watch-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("bin")).expect("dir");
        std::fs::write(base.join("bin/gh"), "#!/bin/sh\nexit 1\n").expect("write");
        std::fs::set_permissions(base.join("bin/gh"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
        std::fs::create_dir_all(base.join("jaynes")).expect("dir");
        std::fs::write(base.join("jaynes/guard.json"), settings.to_string()).expect("write");
        let project = base.join("project");
        std::fs::create_dir_all(&project).expect("dir");
        for args in [vec!["init", "-q"], vec!["remote", "add", "origin", &format!("https://github.com/{owner}/thing.git")]] {
            assert!(Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success());
        }
        World { base }
    }

    fn project(&self) -> PathBuf {
        self.base.join("project")
    }

    fn notes_path(&self) -> PathBuf {
        self.base.join("jaynes/guard-watch.jsonl")
    }

    fn notes(&self) -> Vec<Value> {
        std::fs::read_to_string(self.notes_path())
            .map(|t| t.lines().map(|l| serde_json::from_str(l).expect("a note is one JSON line")).collect())
            .unwrap_or_default()
    }

    fn run(&self, hook: &str, event: Value) -> Option<Value> {
        let path = format!("{}:{}", self.base.join("bin").display(), std::env::var("PATH").unwrap_or_default());
        let mut child = Command::new("python3")
            .arg(repo().join("hooks").join(hook))
            .env("PATH", path).env("JAYNES_HOME", self.base.join("jaynes")).env("GH_CONFIG_DIR", self.base.join("gh")).env("CLAUDE_PLUGIN_ROOT", repo())
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().expect("hook");
        child.stdin.take().expect("stdin").write_all(event.to_string().as_bytes()).expect("event");
        let done = child.wait_with_output().expect("hook");
        assert!(done.status.success() && done.stderr.is_empty(), "{}", String::from_utf8_lossy(&done.stderr));
        let printed = String::from_utf8_lossy(&done.stdout).trim().to_string();
        (!printed.is_empty()).then(|| serde_json::from_str(&printed).expect("json"))
    }

    fn guard(&self, tool: &str, input: Value) -> Option<Value> {
        self.run("guard.py", json!({"session_id": "s-1", "hook_event_name": "PreToolUse", "tool_name": tool, "cwd": self.project(), "tool_input": input}))
    }

    fn bash(&self, command: &str) -> Option<Value> {
        self.guard("Bash", json!({"command": command}))
    }
}

fn asks(said: &Option<Value>) -> bool {
    said.as_ref().is_some_and(|v| v["hookSpecificOutput"]["permissionDecision"] == "ask")
}

fn why(said: &Option<Value>) -> String {
    said.as_ref().map(|v| v["hookSpecificOutput"]["permissionDecisionReason"].to_string()).unwrap_or_default()
}

fn watching() -> Value {
    json!({"on": true, "watch": true, "owners": ["me"]})
}

#[test]
fn watching_notes_what_can_be_taken_back_and_asks_nothing() {
    let w = World::new("notes", "acme", watching());
    for command in ["git push origin topic-word-17", "gh pr create --fill --title 'title-word-17'", "gh pr merge 3 --squash"] {
        let said = w.bash(command);
        assert_eq!(said, None, "{command}: watching asks nothing it could take back");
    }
    let notes = w.notes();
    assert_eq!(notes.len(), 3, "{notes:?}");
    let actions: Vec<&str> = notes.iter().map(|n| n["held"][0]["action"].as_str().unwrap_or("")).collect();
    assert_eq!(actions, ["git push", "gh pr create", "gh pr merge"]);
    for n in &notes {
        assert_eq!(n["held"][0]["where"], "github.com/acme/thing", "{n}");
        assert_eq!(n["held"][0]["owner"], "acme", "{n}");
        assert_eq!(n["session"], "s-1", "{n}");
        assert_eq!(n["cwd"], w.project().display().to_string(), "{n}");
        assert!(n["at"].as_str().is_some_and(|t| t.len() >= 19), "{n}");
        assert!(!n.to_string().contains("word-17"), "a note never holds the command's words: {n}");
    }
    let mode = std::fs::metadata(w.notes_path()).expect("notes").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "the notes are the person's alone");
}

#[test]
fn watching_notes_nothing_that_would_not_ask() {
    let w = World::new("quiet", "me", watching());
    for command in ["ls -la", "git status", "git push --dry-run origin main", "git push origin main", "gh pr merge 3"] {
        assert_eq!(w.bash(command), None, "{command}");
    }
    assert!(!w.notes_path().exists(), "a free owner's push, and what the guard does not hold, note nothing");
}

#[test]
fn what_cannot_be_undone_still_asks_while_it_watches_and_is_not_noted() {
    let w = World::new("irreversible", "acme", watching());
    for command in [
        "git push --force origin main",
        "git push -f origin main",
        "git push --force-with-lease origin main",
        "git push --force-with-lease=main origin main",
        "git push --force-if-includes origin main",
        "git push --mirror origin",
        "git push --delete origin topic",
        "git push -d origin topic",
        "git push --prune origin",
        "git push origin +main",
        "git push origin :topic",
        "git reset --hard HEAD~1",
        "git clean -fd",
        "git branch -D topic",
        "git branch --delete --force topic",
        "git tag -d v1",
        "gh pr close 3",
        "gh repo create acme/new --private",
        "gh repo delete acme/thing --yes",
        "gh repo archive acme/thing",
        "git push origin main && git push --force origin main",
    ] {
        assert!(asks(&w.bash(command)), "{command}: what cannot be undone asks while the guard watches");
    }
    // on the person's own repositories too
    let own = World::new("irreversible-own", "me", watching());
    assert!(asks(&own.bash("git push --force origin main")));
    assert!(!w.notes_path().exists() && !own.notes_path().exists(), "what was asked is not noted: {:?}", w.notes());
}

#[test]
fn its_own_files_still_ask_while_it_watches() {
    let w = World::new("own", "acme", watching());
    let settings = w.base.join("jaynes/guard.json");
    let notes = w.notes_path();
    assert!(asks(&w.guard("Edit", json!({"file_path": settings, "old_string": "true", "new_string": "false"}))));
    assert!(asks(&w.guard("Write", json!({"file_path": w.project().join(".jaynes.toml"), "content": "guard = \"strict\"\n"}))));
    assert!(asks(&w.bash(&format!("rm {}", notes.display()))), "clearing the notes is the person's, in Setup");
    assert!(asks(&w.bash(&format!("echo '{{}}' > {}", notes.display()))));
    assert!(!notes.exists());
}

#[test]
fn a_note_it_cannot_write_is_asked() {
    let w = World::new("unwritable", "acme", watching());
    std::fs::create_dir_all(w.notes_path()).expect("a folder where the notes go");
    let said = w.bash("git push origin main");
    assert!(asks(&said), "{said:?}");
    assert!(why(&said).contains("could not note"), "{}", why(&said));
    // nor through a link another wrote in its place
    let w = World::new("linked", "acme", watching());
    let elsewhere = w.base.join("elsewhere.txt");
    std::fs::write(&elsewhere, "").expect("write");
    std::os::unix::fs::symlink(&elsewhere, w.notes_path()).expect("link");
    assert!(asks(&w.bash("git push origin main")));
    assert_eq!(std::fs::read_to_string(&elsewhere).expect("read"), "", "the guard wrote through a link");
}

#[test]
fn off_and_off_in_a_project_note_nothing() {
    let off = World::new("off", "acme", json!({"on": false, "watch": true}));
    assert_eq!(off.bash("git push origin main"), None);
    assert!(!off.notes_path().exists());
    let w = World::new("offin", "acme", json!({"on": true, "watch": true}));
    std::fs::write(w.base.join("jaynes/guard.json"), json!({"on": true, "watch": true, "off_in": [w.project()]}).to_string()).expect("write");
    assert_eq!(w.bash("git push origin main"), None);
    assert!(!w.notes_path().exists());
}

#[test]
fn a_strict_project_notes_every_push_while_it_watches() {
    let w = World::new("strict", "me", watching());
    std::fs::write(w.project().join(".jaynes.toml"), "guard = \"strict\"\n").expect("write");
    assert_eq!(w.bash("git push origin main"), None);
    assert_eq!(w.notes().len(), 1, "{:?}", w.notes());
    assert_eq!(w.notes()[0]["held"][0]["owner"], "me");
}

#[test]
fn where_it_cannot_tell_the_owner_a_push_is_noted_by_where() {
    let w = World::new("elsewhere", "acme", watching());
    assert!(Command::new("git").arg("-C").arg(w.project()).args(["remote", "set-url", "origin", "https://gitlab.example.com/team/thing.git"]).status().expect("git").success());
    assert_eq!(w.bash("git push origin main"), None);
    let notes = w.notes();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0]["held"][0]["where"].as_str().is_some_and(|s| s.contains("gitlab.example.com/team/thing")), "{notes:?}");
    assert!(notes[0]["held"][0]["owner"].is_null(), "{notes:?}");
}

#[test]
fn the_notes_keep_only_the_newest_200() {
    let w = World::new("cap", "acme", watching());
    let old: String = (0..1000).map(|k| format!("{}\n", json!({"at": "2026-11-01T00:00:00-03:00", "session": format!("old-{k}"), "cwd": "/", "held": []}))).collect();
    std::fs::write(w.notes_path(), old).expect("write");
    std::fs::set_permissions(w.notes_path(), std::fs::Permissions::from_mode(0o644)).expect("chmod");
    for _ in 0..3 {
        assert_eq!(w.bash("git push origin main"), None);
    }
    let notes = w.notes();
    assert_eq!(notes.len(), 200, "only the newest 200 stay");
    assert_eq!(notes[0]["session"], "old-803", "the oldest kept is the 200th from the end");
    assert!(notes[197..].iter().all(|n| n["session"] == "s-1"), "the newest three are the pushes");
    let mode = std::fs::metadata(w.notes_path()).expect("notes").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn the_session_says_the_guard_only_watches() {
    let w = World::new("session", "acme", watching());
    let said = w.run("session.py", json!({"session_id": "s-1", "hook_event_name": "SessionStart", "source": "startup", "cwd": w.project()}));
    let text = said.as_ref().map(|v| v["systemMessage"].to_string()).unwrap_or_default();
    assert!(text.contains("only watches") && text.contains("guard-watch.jsonl") && text.contains("cannot be undone"), "{said:?}");
}
