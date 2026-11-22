//! The roles-engineer's ask for fix-bug's cases step (22/11): a step says the size of model its agent works with
//! (`model = "small"`) and how many turns it may take (`turns = N`), and Jaynes gives Claude Code the model of that
//! size and the cap. A turn the cap ends is judged by its checks as any other, since the step asked for no more.
//! Claude Code here is a stand-in on the PATH that notes its arguments and answers in its own format, naming the model
//! it was asked for: no model is reached.

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
        let base = std::env::temp_dir().join(format!("jaynes-step-model-{name}-{}", std::process::id()));
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
}

/// A stand-in for Claude Code: notes the model and the cap it was given, answers naming the model it was asked for,
/// and writes the step's output. The turn capped at 12 turns ends as Claude Code ends a turn that ran out of them.
fn claude(s: &Scratch) {
    let bin = s.path("bin");
    std::fs::create_dir_all(&bin).expect("dir");
    let script = format!(
        r#"#!/bin/sh
model=''; turns=''; prev=''
for a in "$@"; do
  case "$prev" in --model) model="$a";; --max-turns) turns="$a";; esac
  prev="$a"
done
cat > /dev/null
echo "model=$model turns=$turns" >> '{log}'
mkdir -p .jaynes
echo '{{"type":"system","subtype":"init","session_id":"s-'$turns'","model":"'$model'","permissionMode":"dontAsk","tools":[]}}'
echo '{{"type":"assistant","parent_tool_use_id":null,"session_id":"s-'$turns'","message":{{"model":"'$model'","content":[]}}}}'
if [ "$turns" = 12 ]; then
  printf '{{"summary":"two cases"}}' > .jaynes/output.json
  echo '{{"type":"result","subtype":"error_max_turns","is_error":true,"session_id":"s-12","usage":{{"input_tokens":10,"output_tokens":5}}}}'
  exit 1
fi
perl -pi -e 's/hello/hi/' greeting.txt
printf '{{"summary":"hello becomes hi"}}' > .jaynes/output.json
echo '{{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"s-'$turns'","usage":{{"input_tokens":10,"output_tokens":5}}}}'
"#,
        log = s.path("claude.log").display()
    );
    std::fs::write(bin.join("claude"), script).expect("write");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

const ROLE: &str = r#"
role = "sized"
version = "1.0.0"

[[step]]
name = "runner"
by = "jaynes"
use = "find-suite"

[[step]]
name = "cases"
needs = ["runner"]
model = "small"
turns = 12
reach = { project = "read" }
about = "Lists cases."
instructions = "List the cases of {{request}}."
checks = [{ use = "field-required", field = "summary" }]

[step.output]
summary = { type = "string", about = "the cases" }

[[step]]
name = "change"
needs = ["runner", "cases"]
reach = { project = "write", write = ["**"], commands = true }
about = "Makes the change."
instructions = "Make the change: {{request}}"
checks = ["suite-passes"]

[step.output]
summary = { type = "string", about = "what changed" }
"#;

fn role(s: &Scratch) -> PathBuf {
    let dir = s.path("sized");
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("role.toml"), ROLE).expect("write");
    dir
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn asked_a_step_s_agent_works_with_the_model_and_the_turns_its_step_says() {
    let s = Scratch::new("sized");
    claude(&s);
    let role = role(&s);
    let project = s.path("project");
    let (_ok, out) = s.jaynes(&["start", "--run", "sized", "--role", path(&role), "--project", path(&project), "--request", "say hi", "--agent", "claude"]);
    let log = std::fs::read_to_string(s.path("claude.log")).unwrap_or_default();
    let turns: Vec<&str> = log.lines().collect();
    // Jaynes names the small model, the role never does; the step without a size works with the run's own
    assert_eq!(turns, ["model=claude-sonnet-5-5 turns=12", "model=claude-opus-5-5 turns=200"], "{out}");
    // the turn that ran out of its 12 turns is judged by its checks, and passes them
    assert!(out.contains("finished: delivered"), "{out}");
    let e = s.events("sized");
    let types: Vec<String> = e.iter().filter(|e| e["step"] == "cases").filter_map(|e| e["type"].as_str().map(str::to_string)).collect();
    assert!(types.contains(&"AttemptDelivered".to_string()) && !types.contains(&"AttemptFailed".to_string()), "{types:?}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_turn_that_runs_out_of_its_agent_program_s_own_messages_still_fails() {
    // without a cap of the step's, running out is Claude Code's ceiling, and the turn did not finish
    let s = Scratch::new("ceiling");
    claude(&s);
    let role = s.path("ceiling");
    std::fs::create_dir_all(&role).expect("dir");
    // the stand-in runs out at 12, so the step asks for 12 turns and the engine's own ceiling stands in for it there
    std::fs::write(role.join("role.toml"), ROLE.replace("model = \"small\"\nturns = 12\n", "")).expect("write");
    let project = s.path("project");
    let (_ok, out) = s.jaynes(&["start", "--run", "ceiling", "--role", path(&role), "--project", path(&project), "--request", "say hi", "--agent", "claude"]);
    let log = std::fs::read_to_string(s.path("claude.log")).unwrap_or_default();
    assert!(log.lines().all(|l| l == "model=claude-opus-5-5 turns=200"), "no step names a size or a cap: {log}\n{out}");
}

fn path(p: &Path) -> &str {
    p.to_str().expect("path")
}
