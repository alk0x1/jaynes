//! A write outside the step's folder is refused at once, with no question (the engineer lead, 22/11, after the
//! real-user almost allowed a write into the real project, tidepool/test/screens.test.js): Claude Code's file tools run
//! outside Jaynes's box, so a write allowed there lands on the Mac, with no merge and no Undo. The agent reads where the
//! same file is in its copy, the run's story says what was refused, and the file outside stays as it was.
//! Claude Code here is a stand-in on the PATH that asks Jaynes's own leave server, started from the MCP configuration
//! Jaynes gives it, as Claude Code asks it for a write its settings do not allow: no model is reached.

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
    /// A project with a link that leads out of it, to a folder beside it.
    fn new(name: &str) -> Scratch {
        let base = std::env::temp_dir().join(format!("jaynes-write-outside-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let project = base.join("project");
        std::fs::create_dir_all(&project).expect("dir");
        std::fs::create_dir_all(base.join("person")).expect("dir");
        std::fs::create_dir_all(base.join("elsewhere")).expect("dir");
        std::fs::write(project.join("greeting.txt"), "hello\n").expect("write");
        std::fs::write(project.join("Makefile"), "test:\n\tgrep -q hi greeting.txt\n").expect("write");
        std::os::unix::fs::symlink(base.join("elsewhere"), project.join("out")).expect("link");
        for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]] {
            let ok = Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success();
            assert!(ok, "git {args:?}");
        }
        Scratch { base }
    }

    fn path(&self, p: &str) -> PathBuf {
        self.base.join(p)
    }

    fn jaynes(&self, args: &[&str], targets: &str) -> (bool, String) {
        let out = Command::new(BIN)
            .args(args)
            .env("HOME", self.path("person"))
            .env("PATH", format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", self.path("bin").display()))
            .env("JAYNES_ENGINE_HOME", self.path("home")).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "")
            .env_remove("JAYNES_CLAUDE_MODEL").env_remove("JAYNES_CLAUDE_EFFORT").env_remove("JAYNES_AGENT").env_remove("JAYNES_CODEX")
            .env("OUTSIDE_TARGETS", targets).env("AGENT_READ", self.path("agent-read.json"))
            .output().expect("jaynes");
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    }

    fn events(&self, run: &str) -> Vec<Value> {
        let (ok, out) = self.jaynes(&["log", run], "");
        assert!(ok, "{out}");
        out.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }

    /// Start a small change whose agent first asks to write each of `targets`; what Jaynes said, and what the agent read.
    fn start(&self, run: &str, targets: &[String]) -> (String, Vec<Value>) {
        let role = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/roles/small-change");
        let project = self.path("project");
        let (_ok, out) = self.jaynes(&["start", "--run", run, "--role", role.to_str().expect("path"), "--project", project.to_str().expect("path"),
                                       "--request", "say hi", "--agent", "claude"], &targets.join("|"));
        let read = std::fs::read_to_string(self.path("agent-read.json")).unwrap_or_default();
        (out, serde_json::from_str(&read).unwrap_or_default())
    }
}

/// A stand-in for Claude Code: starts the leave server Jaynes names in --mcp-config, asks it for each write in
/// OUTSIDE_TARGETS (a path from the step's folder or a whole one), writes what it was answered where Claude Code writes
/// a tool's result, keeps what it read in AGENT_READ, then makes the change in its folder.
const STAND_IN: &str = r#"#!/usr/bin/env python3
import json, os, subprocess, sys
args = sys.argv[1:]
def val(flag):
    return args[args.index(flag) + 1] if flag in args else None
sys.stdin.read()
model = val("--model")
def say(v):
    print(json.dumps(v), flush=True)
say({"type": "system", "subtype": "init", "session_id": "s1", "model": model, "permissionMode": "default",
     "tools": val("--tools").split(",") + ["mcp__jaynes__permit"]})
server = json.loads(val("--mcp-config"))["mcpServers"]["jaynes"]
env = dict(os.environ)
env.update(server.get("env", {}))
leave = subprocess.Popen([server["command"]] + server["args"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env, text=True)
def call(i, method, params):
    leave.stdin.write(json.dumps({"jsonrpc": "2.0", "id": i, "method": method, "params": params}) + "\n")
    leave.stdin.flush()
    return json.loads(leave.stdout.readline())
call(1, "initialize", {"protocolVersion": "2025-06-18"})
read = []
for n, target in enumerate(t for t in os.environ.get("OUTSIDE_TARGETS", "").split("|") if t):
    path = os.path.join(os.getcwd(), target)
    reply = call(2 + n, "tools/call", {"name": "permit", "arguments": {"tool_name": "Write", "input": {"file_path": path, "content": "written from the step\n"}}})
    decision = json.loads(reply["result"]["content"][0]["text"])
    read.append(decision)
    say({"type": "assistant", "parent_tool_use_id": None, "session_id": "s1",
         "message": {"model": model, "content": [{"type": "tool_use", "id": "t%d" % n, "name": "Write", "input": {"file_path": path}}]}})
    say({"type": "user", "session_id": "s1", "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "t%d" % n, "is_error": True, "content": decision.get("message", "")}]}})
    if decision.get("behavior") == "allow":
        open(path, "w").write("written from the step\n")
leave.stdin.close()
leave.wait()
open(os.environ["AGENT_READ"], "w").write(json.dumps(read))
text = open("greeting.txt").read().replace("hello", "hi")
open("greeting.txt", "w").write(text)
os.makedirs(".jaynes", exist_ok=True)
open(".jaynes/output.json", "w").write(json.dumps({"summary": "The greeting now says hi.", "files": ["greeting.txt"]}))
say({"type": "result", "subtype": "success", "is_error": False, "result": "done", "session_id": "s1", "usage": {"input_tokens": 10, "output_tokens": 5}})
"#;

fn claude(s: &Scratch) {
    let bin = s.path("bin");
    std::fs::create_dir_all(&bin).expect("dir");
    std::fs::write(bin.join("claude"), STAND_IN).expect("write");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

fn real(p: &Path) -> String {
    std::fs::canonicalize(p).expect("real").display().to_string()
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn asked_where_the_agent_wants_to_write() {
    let s = Scratch::new("project");
    claude(&s);
    let original = s.path("project/greeting.txt");
    let target = original.display().to_string();
    let (out, read) = s.start("outside", &[target.clone(), target]);
    // refused at once, never asked: no question opened, and the run goes on to its end
    assert_eq!(read.len(), 2, "{out}");
    let copy = s.path("home/runs/outside/project");
    for d in &read {
        assert_eq!(d["behavior"], "deny", "{d}");
        assert_eq!(d["message"].as_str().unwrap_or(""), format!(
            "Jaynes refused this write: {}/greeting.txt is outside this step's folder, {c}, and nothing is written outside it. The same file in your copy of the project is {c}/greeting.txt: write it there.",
            real(&s.path("project")), c = copy.display()
        ));
    }
    let e = s.events("outside");
    let types: Vec<&str> = e.iter().filter_map(|e| e["type"].as_str()).collect();
    assert!(!types.contains(&"QuestionOpened") && !types.contains(&"PermissionAsked"), "{types:?}");
    assert!(out.contains("finished: delivered"), "{out}");
    // the file outside stays as it was
    assert_eq!(std::fs::read_to_string(&original).expect("read"), "hello\n");
    // the run's story says what was refused, once, and where the agent was told to write
    let story = jaynes_shell::listing::story(&s.path("home"), "outside");
    let (_, lines) = story.iter().find(|(step, _)| step == "change").expect("the change's story");
    let at = lines.iter().position(|l| l.starts_with("◇ ")).unwrap_or_else(|| panic!("{lines:?}"));
    assert_eq!(lines[at], format!("◇ Jaynes refused a write outside project's copy: {}/greeting.txt · 2 times", real(&s.path("project"))));
    assert_eq!(lines[at + 1], "told the agent to write greeting.txt in its copy");
    assert_eq!(lines.iter().filter(|l| l.starts_with("◇ ")).count(), 1, "{lines:?}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_write_through_a_link_or_up_with_dots_out_of_the_copy_is_refused_too() {
    let s = Scratch::new("link");
    claude(&s);
    let (out, read) = s.start("link", &["out/planted.txt".to_string(), "../../escaped.txt".to_string()]);
    assert_eq!(read.len(), 2, "{out}");
    let elsewhere = real(&s.path("elsewhere"));
    let copy = s.path("home/runs/link/project");
    assert_eq!(read[0]["message"].as_str().unwrap_or(""), format!(
        "Jaynes refused this write: {elsewhere}/planted.txt is outside this step's folder, {c}, and nothing is written outside it. Write only under {c}.", c = copy.display()
    ));
    assert_eq!(read[1]["behavior"], "deny", "{}", read[1]);
    assert!(read[1]["message"].as_str().unwrap_or("").starts_with(&format!("Jaynes refused this write: {}/escaped.txt is outside", real(&s.path("home/runs")))), "{}", read[1]);
    assert!(!s.path("elsewhere/planted.txt").exists() && !s.path("home/runs/escaped.txt").exists());
    let story = jaynes_shell::listing::story(&s.path("home"), "link");
    let (_, lines) = story.iter().find(|(step, _)| step == "change").expect("the change's story");
    assert!(lines.contains(&format!("◇ Jaynes refused a write outside project's copy: {elsewhere}/planted.txt")), "{lines:?}");
    assert!(!lines.iter().any(|l| l.starts_with("told the agent")), "no file of the copy to name: {lines:?}");
}
