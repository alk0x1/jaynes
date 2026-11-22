//! On a free owner's repository (the GitHub user gh is signed in as, or the owners the person set), what loses nothing
//! by what git holds passes without the guard asking (the person's decision of 22/11, through the tech-lead):
//! `git reset --hard` with the tree clean, when every commit it takes out of HEAD is in another ref or has its same
//! change there (the same patch id); a forced `git push` when every commit it takes off the remote is in what is pushed
//! or in another ref, by its patch too; and `git branch -D` of a branch whose changes were picked elsewhere. Anywhere
//! else, a repository of another owner, one with no remote or one whose owner the guard cannot tell, they ask as
//! before. Run as Claude Code runs the guard, on real repositories with a home of their own: the remote is a bare
//! repository in the test's folder, which the repository's push address names as a GitHub repository.

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
    /// A repository with main at one commit, gh signed in as alk0x1, and, when `owner` is given, a remote whose push
    /// address is github.com/<owner>/project and which fetches from a bare repository beside it.
    fn new(name: &str, owner: Option<&str>) -> World {
        let base = std::env::temp_dir().join(format!("jaynes-owned-kept-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        for dir in ["project", "jaynes", "gh"] {
            std::fs::create_dir_all(base.join(dir)).expect("dir");
        }
        std::fs::write(base.join("jaynes/guard.json"), "{}").expect("write");
        std::fs::write(base.join("gh/hosts.yml"), "github.com:\n    user: alk0x1\n    git_protocol: https\n").expect("write");
        let w = World { base };
        w.git(&["init", "-q", "-b", "main"]);
        w.change("first.txt", "first");
        if let Some(owner) = owner {
            let bare = w.base.join("remote.git");
            assert!(Command::new("git").args(["init", "-q", "--bare"]).arg(&bare).status().expect("git").success());
            w.git(&["remote", "add", "origin", &bare.display().to_string()]);
            w.git(&["remote", "set-url", "--push", "origin", &format!("https://github.com/{owner}/project.git")]);
        }
        w
    }

    fn project(&self) -> PathBuf {
        self.base.join("project")
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git").arg("-C").arg(self.project()).args(["-c", "user.name=a", "-c", "user.email=a@a"]).args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1")
            .output().expect("git");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Push what the bare remote fetches from: the test's own push, past the GitHub address the guard reads.
    fn publish(&self, refspec: &str) {
        let bare = self.base.join("remote.git").display().to_string();
        self.git(&["push", "-q", "-f", &bare, refspec]);
        self.git(&["fetch", "-q", "origin"]);
    }

    /// A commit that changes `file`, so a pick of it has the same patch id under another hash.
    fn change(&self, file: &str, what: &str) -> String {
        std::fs::write(self.project().join(file), format!("{what}\n")).expect("write");
        self.git(&["add", file]);
        self.git(&["commit", "-q", "-m", what]);
        self.git(&["rev-parse", "--short", "HEAD"])
    }

    fn bash(&self, command: &str) -> Option<Value> {
        let mut child = Command::new("python3")
            .arg(repo().join("hooks/guard.py"))
            .env("JAYNES_HOME", self.base.join("jaynes")).env("GH_CONFIG_DIR", self.base.join("gh")).env("CLAUDE_PLUGIN_ROOT", repo())
            .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().expect("guard");
        let event = json!({"session_id": "s", "hook_event_name": "PreToolUse", "tool_name": "Bash", "cwd": self.project(), "tool_input": {"command": command}});
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

fn why(said: &Option<Value>) -> String {
    said.as_ref().map(|v| v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap_or("").to_string()).unwrap_or_default()
}

/// The line the guard says as it lets an action pass: never a decision of its own, so Claude Code's permissions apply.
fn line(said: &Option<Value>) -> String {
    let v = said.as_ref().expect("the guard says a line");
    assert!(v.get("hookSpecificOutput").is_none(), "no decision, only a line: {v}");
    v["systemMessage"].as_str().unwrap_or("").to_string()
}

/// work two commits ahead of main, both published on the remote as origin/work, and checked out.
fn ahead_and_published(w: &World) {
    w.git(&["switch", "-q", "-c", "work"]);
    w.change("a.txt", "a");
    w.change("b.txt", "b");
    w.publish("work:refs/heads/work");
}

#[test]
fn a_reset_hard_that_loses_nothing_passes_on_a_free_owner_s_repository() {
    let w = World::new("reset-kept", Some("alk0x1"));
    ahead_and_published(&w);
    let said = w.bash("git reset --hard main");
    assert!(!asks(&said), "every commit it takes out of HEAD is in origin/work: {said:?}");
    assert!(line(&said).contains("nothing is lost"), "{}", line(&said));
}

#[test]
fn a_reset_hard_whose_commits_were_picked_elsewhere_passes_saying_where_they_are() {
    let w = World::new("reset-picked", Some("alk0x1"));
    w.git(&["switch", "-q", "-c", "work"]);
    w.change("a.txt", "a");
    w.change("b.txt", "b");
    w.git(&["switch", "-q", "main"]);
    w.change("main.txt", "main moves on");
    w.git(&["cherry-pick", "work~1", "work"]);
    let picked = w.git(&["rev-parse", "--short", "HEAD"]);
    w.git(&["switch", "-q", "work"]);
    let said = w.bash("git reset --hard main~2");
    assert!(!asks(&said), "{said:?}");
    let line = line(&said);
    assert!(line.contains("nothing is lost: the tree is clean and its changes are in main as") && line.contains(&picked) && line.contains("the old commits stay in the reflog"), "{line}");
}

#[test]
fn a_reset_hard_asks_as_before_with_a_change_in_the_tree_or_a_commit_nowhere_else() {
    let w = World::new("reset-lost", Some("alk0x1"));
    ahead_and_published(&w);
    std::fs::write(w.project().join("a.txt"), "not committed\n").expect("write");
    let said = w.bash("git reset --hard main");
    assert!(asks(&said) && why(&said).contains("git reset --hard throws away uncommitted work"), "a change in the tree: {said:?}");
    w.git(&["checkout", "--", "a.txt"]);
    std::fs::write(w.project().join("new.txt"), "untracked\n").expect("write");
    assert!(asks(&w.bash("git reset --hard main")), "an untracked file is in the tree too");
    std::fs::remove_file(w.project().join("new.txt")).expect("rm");
    w.change("c.txt", "c");
    assert!(asks(&w.bash("git reset --hard main")), "c is in no other ref");
}

#[test]
fn a_reset_hard_that_loses_nothing_asks_on_another_owner_s_repository_and_on_one_with_no_remote() {
    for (name, owner) in [("reset-other", Some("someone-else")), ("reset-none", None)] {
        let w = World::new(name, owner);
        w.git(&["switch", "-q", "-c", "work"]);
        w.change("a.txt", "a");
        w.git(&["branch", "kept"]);
        let said = w.bash("git reset --hard main");
        assert!(asks(&said) && why(&said).contains("git reset --hard throws away uncommitted work"), "{name}: {said:?}");
    }
}

/// work published with a and b; then main moves on and work is rebased onto it: a and b are in work as new commits.
fn rebased_after_publishing(w: &World) {
    ahead_and_published(w);
    w.git(&["switch", "-q", "main"]);
    w.change("main.txt", "main moves on");
    w.git(&["switch", "-q", "work"]);
    w.git(&["rebase", "-q", "main"]);
}

#[test]
fn a_forced_push_whose_commits_are_in_what_is_pushed_passes_on_a_free_owner_s_repository() {
    let w = World::new("push-kept", Some("alk0x1"));
    rebased_after_publishing(&w);
    w.git(&["branch", "-q", "--set-upstream-to=origin/work"]);
    for command in ["git push -f origin work", "git push --force-with-lease origin work", "git push origin +work", "git push -f origin HEAD:work", "git push -f"] {
        let said = w.bash(command);
        assert!(!asks(&said), "{command}: {said:?}");
        let line = line(&said);
        assert!(line.contains("nothing is lost: its changes are in what it pushes to work as"), "{command}: {line}");
    }
}

#[test]
fn a_forced_push_that_takes_a_commit_off_the_remote_asks_as_before() {
    let w = World::new("push-lost", Some("alk0x1"));
    ahead_and_published(&w);
    w.git(&["reset", "-q", "--hard", "work~1"]);
    let said = w.bash("git push -f origin work");
    assert!(asks(&said) && why(&said).contains("git push, forced or deleting, rewrites or removes"), "b leaves the remote: {said:?}");
    assert!(asks(&w.bash("git push origin --delete work")), "a deletion asks");
    assert!(asks(&w.bash("git push -f origin :work")), "a deletion asks");
}

#[test]
fn a_forced_push_that_loses_nothing_asks_on_another_owner_s_repository_and_with_no_remote() {
    let w = World::new("push-other", Some("someone-else"));
    rebased_after_publishing(&w);
    let said = w.bash("git push -f origin work");
    assert!(asks(&said) && why(&said).contains("git push, forced or deleting"), "{said:?}");
    let w = World::new("push-none", None);
    w.git(&["switch", "-q", "-c", "work"]);
    w.change("a.txt", "a");
    assert!(asks(&w.bash("git push -f origin work")), "no remote to read");
}

#[test]
fn a_branch_whose_changes_were_picked_elsewhere_is_deleted_without_asking_only_on_a_free_owner_s_repository() {
    for (name, owner, passes) in [("branch-mine", Some("alk0x1"), true), ("branch-other", Some("someone-else"), false), ("branch-none", None, false)] {
        let w = World::new(name, owner);
        w.git(&["switch", "-q", "-c", "picked"]);
        w.change("a.txt", "a");
        w.git(&["switch", "-q", "main"]);
        w.change("main.txt", "main moves on");
        w.git(&["cherry-pick", "picked"]);
        let said = w.bash("git branch -D picked");
        if passes {
            assert!(line(&said).contains("nothing is lost: its changes are in main as"), "{name}: {said:?}");
        } else {
            assert!(asks(&said) && why(&said).contains("git branch -D deletes the branch picked"), "{name}: {said:?}");
        }
    }
}
