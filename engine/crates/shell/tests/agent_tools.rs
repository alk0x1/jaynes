//! The agent of a step gets only the tools its step needs (the engineer lead, 22/11, after the real-user saw in a
//! SessionStarted that Jaynes's agents were given SendMessage, ListAgents, SendToProject, Workflow, RemoteTrigger and
//! CronCreate): an agent that messages another Claude session on the Mac, or schedules work in the cloud, leaves the
//! reach Jaynes promises, and its message can reach a person's other sessions as a colleague's. So Jaynes names the
//! tools Claude Code makes available (`--tools`), never the ones it takes away: reading, searching and editing files,
//! the shell when the step runs commands, the web tools when it reaches the web. And a turn whose Claude Code says it
//! runs with a tool beyond them does not count, whatever it did.
//! Claude Code here is a stand-in on the PATH that notes its arguments and answers in its own format: no model is reached.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

struct Scratch {
    base: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _gone = std::fs::remove_dir_all(&self.base);
    }
}

impl Scratch {
    fn new(name: &str) -> Scratch {
        let base = std::env::temp_dir().join(format!("jaynes-agent-tools-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let project = base.join("project");
        std::fs::create_dir_all(&project).expect("dir");
        std::fs::create_dir_all(base.join("person")).expect("dir");
        std::fs::write(project.join("greeting.txt"), "hello\n").expect("write");
        std::fs::write(project.join("Makefile"), "test:\n\tgrep -q hi greeting.txt\n").expect("write");
        for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]] {
            let ok = Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success();
            assert!(ok, "git {args:?}");
        }
        Scratch { base }
    }

    fn path(&self, p: &str) -> PathBuf {
        self.base.join(p)
    }

    fn jaynes(&self, args: &[&str]) -> (bool, String) {
        let out = Command::new(BIN)
            .args(args)
            .env("HOME", self.path("person"))
            .env("PATH", format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", self.path("bin").display()))
            .env("JAYNES_ENGINE_HOME", self.path("home")).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "")
            .env_remove("JAYNES_CLAUDE_MODEL").env_remove("JAYNES_CLAUDE_EFFORT").env_remove("JAYNES_AGENT").env_remove("JAYNES_CODEX")
            .output().expect("jaynes");
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    }

    fn events(&self, run: &str) -> Vec<Value> {
        let (ok, out) = self.jaynes(&["log", run]);
        assert!(ok, "{out}");
        out.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }

    /// Start a run of `role` with Claude Code, the stand-in; what Jaynes said, and the stand-in's arguments, a turn a line.
    fn start(&self, run: &str, role: &Path) -> (String, Vec<String>) {
        let project = self.path("project");
        let (_ok, out) = self.jaynes(&["start", "--run", run, "--role", role.to_str().expect("path"), "--project", project.to_str().expect("path"),
                                       "--request", "say hi", "--agent", "claude"]);
        let args = std::fs::read_to_string(self.path("claude.args")).unwrap_or_default();
        (out, args.lines().map(str::to_string).collect())
    }
}

/// A stand-in for Claude Code: notes its arguments, one line a turn, makes the change and answers in Claude Code's
/// format. It says it runs with the tools it was given (`--tools`), and with `more` beside them.
fn claude(s: &Scratch, more: &str) {
    let bin = s.path("bin");
    std::fs::create_dir_all(&bin).expect("dir");
    let script = format!(
        r#"#!/bin/sh
echo "$*" >> '{args}'
model=''; tools=''; prev=''
for a in "$@"; do
  case "$prev" in --model) model="$a";; --tools) tools="$a";; esac
  prev="$a"
done
cat > /dev/null
list=$(printf '%s' "$tools{more}" | sed 's/,/","/g')
echo '{{"type":"system","subtype":"init","session_id":"s1","model":"'$model'","permissionMode":"dontAsk","tools":["'"$list"'"]}}'
echo '{{"type":"assistant","parent_tool_use_id":null,"session_id":"s1","message":{{"model":"'$model'","content":[]}}}}'
mkdir -p .jaynes
perl -pi -e 's/hello/hi/' greeting.txt
printf '{{"summary":"The greeting now says hi.","files":["greeting.txt"]}}' > .jaynes/output.json
echo '{{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"s1","usage":{{"input_tokens":10,"output_tokens":5}}}}'
"#,
        args = s.path("claude.args").display()
    );
    std::fs::write(bin.join("claude"), script).expect("write");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

fn small_change() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/roles/small-change")
}

/// The value of `flag` in a line of the stand-in's arguments.
fn value<'a>(line: &'a str, flag: &str) -> Option<&'a str> {
    let mut words = line.split(' ');
    words.by_ref().find(|w| *w == flag)?;
    words.next()
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn asked_a_step_s_agent_is_given_only_the_tools_its_step_needs() {
    let s = Scratch::new("only");
    claude(&s, "");
    let (out, turns) = s.start("only", &small_change());
    assert_eq!(turns.len(), 1, "{out}");
    // the change runs commands and reaches no web: the file tools and the shell, nothing else
    assert_eq!(value(&turns[0], "--tools"), Some("Bash,Edit,Glob,Grep,Read,Write"), "{}", turns[0]);
    assert!(out.contains("finished: delivered"), "{out}");
    // what the agent program said it runs with is recorded with its session, as the real-user read it
    let e = s.events("only");
    let started = e.iter().find(|e| e["type"] == "SessionStarted").expect("session");
    let tools: Vec<&str> = started["confirmed"]["tools"].as_array().expect("tools").iter().filter_map(Value::as_str).collect();
    assert_eq!(tools, ["Bash", "Edit", "Glob", "Grep", "Read", "Write"]);
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_step_without_commands_or_with_the_web_gets_its_tools_and_no_others() {
    let s = Scratch::new("reach");
    claude(&s, "");
    let role = s.path("reads");
    std::fs::create_dir_all(&role).expect("dir");
    let toml = std::fs::read_to_string(small_change().join("role.toml")).expect("role")
        .replace("reach = { project = \"write\", write = [\"**\"], commands = true }", "reach = { project = \"write\", write = [\"**\"], network = \"web\" }")
        .replace("  \"suite-passes\",\n", "")
        .replace("[[promise]]\nsays = \"Nothing is delivered before the project's suite passed on the change\"\nnever = \"deliver\"\nunless = \"check suite-passes of change passed\"\n", "");
    std::fs::write(role.join("role.toml"), toml).expect("write");
    let (out, turns) = s.start("reach", &role);
    assert_eq!(turns.len(), 1, "{out}");
    assert_eq!(value(&turns[0], "--tools"), Some("Edit,Glob,Grep,Read,WebFetch,WebSearch,Write"), "no shell without commands, the web tools with the web: {}", turns[0]);
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_turn_whose_claude_code_runs_with_a_tool_it_was_not_given_does_not_count() {
    // a Claude Code that ignores --tools, or adds tools of its own, gives the agent what Jaynes did not
    let s = Scratch::new("beyond");
    claude(&s, ",SendMessage,CronCreate");
    let (out, _turns) = s.start("beyond", &small_change());
    assert!(!out.contains("finished: delivered"), "{out}");
    let e = s.events("beyond");
    let ended: Vec<String> = e.iter().filter(|e| e["type"] == "AttemptFaulted" || e["type"] == "AttemptEnded" || e["type"] == "RunBlocked").map(Value::to_string).collect();
    let said = format!("{out}\n{}", ended.join("\n"));
    assert!(said.contains("Claude Code gave the agent tools Jaynes did not: SendMessage, CronCreate; this turn does not count"), "{said}");
}
