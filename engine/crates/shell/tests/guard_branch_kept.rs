//! Deleting a branch loses something only when its commits are nowhere else. When another ref reaches the branch's tip
//! (another branch, a tag or a remote-tracking ref; the branch itself does not count), the guard lets
//! `git branch -D`, a forced `git branch -d` and `git update-ref -d refs/heads/...` pass, and says in one line
//! "nothing is lost: its commits stay in <ref>" (the tech-lead's decision of 21/11, after the desktop-engineer and the
//! engineer lead stood still at the person's question with the person away from the Mac). When some commit would stay
//! only in the reflog, which expires, it goes on asking, and the question says how many commits are lost and the tip's
//! hash to bring them back; unless every such commit has its same change in another ref (the same patch, as a
//! cherry-pick leaves it), which the guard says as it is: the changes are there under other hashes, the old commits
//! stay in the reflog. Run as Claude Code runs it, on a real repository with a home of its own.

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
    /// A repository with main at one commit and the guard's settings empty.
    fn new(name: &str) -> World {
        let base = std::env::temp_dir().join(format!("jaynes-branch-kept-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("project")).expect("dir");
        std::fs::create_dir_all(base.join("jaynes")).expect("dir");
        std::fs::write(base.join("jaynes/guard.json"), "{}").expect("write");
        let w = World { base };
        w.git(&["init", "-q", "-b", "main"]);
        w.commit("first");
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

    fn commit(&self, what: &str) -> String {
        self.git(&["commit", "-q", "--allow-empty", "-m", what]);
        self.git(&["rev-parse", "--short", "HEAD"])
    }

    fn bash_in(&self, cwd: &Path, command: &str) -> Option<Value> {
        let mut child = Command::new("python3")
            .arg(repo().join("hooks/guard.py"))
            .env("JAYNES_HOME", self.base.join("jaynes")).env("GH_CONFIG_DIR", self.base.join("gh")).env("CLAUDE_PLUGIN_ROOT", repo())
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().expect("guard");
        let event = json!({"session_id": "s", "hook_event_name": "PreToolUse", "tool_name": "Bash", "cwd": cwd, "tool_input": {"command": command}});
        child.stdin.take().expect("stdin").write_all(event.to_string().as_bytes()).expect("event");
        let done = child.wait_with_output().expect("guard");
        assert!(done.status.success() && done.stderr.is_empty(), "{}", String::from_utf8_lossy(&done.stderr));
        let printed = String::from_utf8_lossy(&done.stdout).trim().to_string();
        (!printed.is_empty()).then(|| serde_json::from_str(&printed).expect("json"))
    }

    fn bash(&self, command: &str) -> Option<Value> {
        self.bash_in(&self.project(), command)
    }
}

fn asks(said: &Option<Value>) -> bool {
    said.as_ref().is_some_and(|v| v["hookSpecificOutput"]["permissionDecision"] == "ask")
}

fn why(said: &Option<Value>) -> String {
    said.as_ref().map(|v| v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap_or("").to_string()).unwrap_or_default()
}

/// The one line the guard says when it lets a deletion pass, shown to the person; never a decision of its own, so
/// Claude Code's permissions still apply.
fn line(said: &Option<Value>) -> String {
    let v = said.as_ref().expect("the guard says a line");
    assert!(v.get("hookSpecificOutput").is_none(), "no decision, only a line: {v}");
    v["systemMessage"].as_str().unwrap_or("").to_string()
}

/// The engineer lead's case (lote-025l, contained in lote-025k): the branch's commits are on a later branch.
#[test]
fn a_branch_whose_commits_a_later_branch_holds_is_deleted_without_asking() {
    let w = World::new("later");
    w.git(&["switch", "-q", "-c", "lote-l"]);
    w.commit("l");
    w.git(&["switch", "-q", "-c", "lote-k"]);
    w.commit("k");
    w.git(&["switch", "-q", "main"]);
    for command in ["git branch -D lote-l", "git branch -d --force lote-l", "git branch --delete --force lote-l", "git update-ref -d refs/heads/lote-l"] {
        let said = w.bash(command);
        assert!(!asks(&said), "{command}: {said:?}");
        let line = line(&said);
        assert!(line.contains("nothing is lost: its commits stay in lote-k") && line.contains("lote-l"), "{command}: {line}");
    }
}

/// The desktop-engineer's case (app-run-name-025 on the same commit as app-blocked-025b): another branch at the tip.
#[test]
fn a_branch_at_the_same_commit_as_another_is_deleted_without_asking() {
    let w = World::new("same");
    w.git(&["switch", "-q", "-c", "app-run-name"]);
    w.commit("run name");
    w.git(&["branch", "app-blocked"]);
    w.git(&["switch", "-q", "main"]);
    let said = w.bash("git branch -D app-run-name");
    assert!(!asks(&said), "{said:?}");
    assert!(line(&said).contains("nothing is lost: its commits stay in app-blocked"), "{}", line(&said));
}

#[test]
fn a_branch_a_remote_or_a_tag_holds_is_deleted_without_asking() {
    let w = World::new("remote");
    w.git(&["switch", "-q", "-c", "pushed"]);
    let tip = w.commit("pushed");
    w.git(&["update-ref", "refs/remotes/origin/pushed", &tip]);
    w.git(&["switch", "-q", "-c", "tagged", "main"]);
    w.commit("tagged");
    w.git(&["tag", "v1"]);
    w.git(&["switch", "-q", "main"]);
    let said = w.bash("git branch -D pushed");
    assert!(!asks(&said) && line(&said).contains("nothing is lost: its commits stay in origin/pushed"), "{said:?}");
    let said = w.bash("git -C . branch -D tagged");
    assert!(!asks(&said) && line(&said).contains("nothing is lost: its commits stay in v1"), "{said:?}");
}

#[test]
fn a_branch_whose_commits_are_its_own_asks_saying_how_many_are_lost_and_the_tip() {
    let w = World::new("own");
    w.git(&["switch", "-q", "-c", "tmp-s3"]);
    w.commit("one");
    let tip = w.commit("two");
    w.git(&["switch", "-q", "main"]);
    for command in ["git branch -D tmp-s3", "git branch -d -f tmp-s3", "git update-ref -d refs/heads/tmp-s3"] {
        let said = w.bash(command);
        assert!(asks(&said), "{command}: {said:?}");
        let why = why(&said);
        assert!(why.contains("tmp-s3") && why.contains("2 commits") && why.contains(&tip), "{command}: {why}");
    }
}

#[test]
fn branches_deleted_together_do_not_hold_each_other() {
    let w = World::new("together");
    w.git(&["switch", "-q", "-c", "a"]);
    w.commit("a");
    w.git(&["switch", "-q", "-c", "b"]);
    let tip = w.commit("b");
    w.git(&["switch", "-q", "main"]);
    let said = w.bash("git branch -D a b");
    assert!(asks(&said), "a is held only by b, which goes too: {said:?}");
    assert!(why(&said).contains(&tip), "{}", why(&said));
    let said = w.bash("git branch -D a && git branch -D b");
    assert!(asks(&said), "the same in two commands: {said:?}");
}

#[test]
fn what_the_guard_cannot_see_still_asks() {
    let w = World::new("unseen");
    for command in [
        "git branch -D not-there",
        "git branch -D -r origin/main",
        "git update-ref -d refs/tags/v1",
        "git branch -D $B",
    ] {
        assert!(asks(&w.bash(command)), "{command}");
    }
    let elsewhere = w.base.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("dir");
    assert!(asks(&w.bash_in(&elsewhere, "git branch -D main")), "outside any repository");
}

#[test]
fn a_kept_branch_beside_something_else_held_asks_for_that_and_says_the_branch_is_kept() {
    let w = World::new("beside");
    w.git(&["switch", "-q", "-c", "old"]);
    w.commit("old");
    w.git(&["branch", "new"]);
    w.git(&["switch", "-q", "main"]);
    let said = w.bash("git branch -D old && git reset --hard HEAD");
    assert!(asks(&said), "{said:?}");
    let why = why(&said);
    assert!(why.contains("git reset --hard") && why.contains("nothing is lost: its commits stay in new"), "{why}");
}

/// The rd-engineer's case (tty-and-v6, its patch in main under another commit): every commit only the branch holds is
/// in another ref as the same change.
#[test]
fn a_branch_whose_changes_were_picked_into_another_is_deleted_saying_where_they_are() {
    let w = World::new("picked");
    w.git(&["switch", "-q", "-c", "tty-and-v6"]);
    std::fs::write(w.project().join("tty.txt"), "v6\n").expect("write");
    w.git(&["add", "tty.txt"]);
    w.git(&["commit", "-q", "-m", "tty"]);
    std::fs::write(w.project().join("v6.txt"), "v6\n").expect("write");
    w.git(&["add", "v6.txt"]);
    w.git(&["commit", "-q", "-m", "v6"]);
    w.git(&["switch", "-q", "main"]);
    w.commit("main moves on");
    w.git(&["cherry-pick", "tty-and-v6~1", "tty-and-v6"]);
    let picked = w.git(&["rev-parse", "--short", "HEAD"]);
    let first = w.git(&["rev-parse", "--short", "HEAD~1"]);
    for command in ["git branch -D tty-and-v6", "git update-ref -d refs/heads/tty-and-v6"] {
        let said = w.bash(command);
        assert!(!asks(&said), "{command}: {said:?}");
        let line = line(&said);
        assert!(line.contains("nothing is lost: its changes are in main as") && line.contains(&picked) && line.contains(&first)
            && line.contains("the old commits stay in the reflog"), "{command}: {line}");
    }
}

#[test]
fn one_commit_without_its_same_change_elsewhere_asks_as_before() {
    let w = World::new("half-picked");
    w.git(&["switch", "-q", "-c", "half"]);
    std::fs::write(w.project().join("a.txt"), "a\n").expect("write");
    w.git(&["add", "a.txt"]);
    w.git(&["commit", "-q", "-m", "a"]);
    std::fs::write(w.project().join("b.txt"), "b\n").expect("write");
    w.git(&["add", "b.txt"]);
    w.git(&["commit", "-q", "-m", "b"]);
    let tip = w.git(&["rev-parse", "--short", "HEAD"]);
    w.git(&["switch", "-q", "main"]);
    w.commit("main moves on"); // else the pick, same parent and same second, is the very same commit
    w.git(&["cherry-pick", "half~1"]);
    let said = w.bash("git branch -D half");
    assert!(asks(&said), "b is nowhere else: {said:?}");
    assert!(why(&said).contains(&tip) && why(&said).contains("2 commits"), "{}", why(&said));
}
