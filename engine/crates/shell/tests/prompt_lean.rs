//! A turn that goes on in a session the agent already has is not told again what that session already heard, word
//! for word: the request, what to do when it cannot be done as asked, how to reply to the person (the tech-lead's cost
//! lever for 0.2.5; the engineer lead's rule: a text is left out only when the same session already got exactly that
//! text). What is new since the session's last turn comes whole: the person's words, why the last attempt was
//! rejected. A new session hears everything, and so does one whose last turn Claude Code compacted, or whose last
//! turn's log the engine cannot read. The agent here is a shell command that keeps each prompt it is given and
//! speaks as Claude Code's stream does, so no model is reached.

use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

/// A command at a terminal, as when the person types it, as carry_on.rs has it.
const TTY: &str = r#"
import fcntl, os, select, sys, termios, time
for tried in range(10):
    try:
        master, slave = os.openpty()
        break
    except OSError as e:
        why = e
        time.sleep(0.2)
else:
    print(f"the test's terminal could not be opened: {why}")
    sys.exit(97)
pid = os.fork()
if pid == 0:
    os.close(master)
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    for fd in (0, 1, 2):
        os.dup2(slave, fd)
    if slave > 2:
        os.close(slave)
    os.execvp(sys.argv[1], sys.argv[1:])
status = None
while True:
    ready, _, _ = select.select([master], [], [], 0.05)
    if ready:
        data = os.read(master, 65536)
        if data:
            os.write(1, data)
            continue
    if status is not None:
        break
    done, st = os.waitpid(pid, os.WNOHANG)
    if done:
        status = st
sys.exit(os.waitstatus_to_exitcode(status))
"#;

/// `look` starts the session; `change` goes on in it and stops for the person.
const ROLE: &str = r#"
role = "lean"
version = "1.0.0"

[[step]]
name = "runner"
by = "jaynes"
use = "find-suite"

[[step]]
name = "look"
reach = { project = "read", commands = true }
about = "Looks at the project."
instructions = "Look at the project for the request."
checks = ["output-follows-schema"]

[step.output]
seen = { type = "string", about = "what it saw" }

[[step]]
name = "change"
needs = ["runner", "look"]
session = "continue"
reach = { project = "write", write = ["**"], commands = true }
about = "Makes the change."
instructions = "Make the change the request asks."
checks = ["suite-passes"]
stop = { ask = "Keep it?", verdicts = ["keep", "change", "redo"] }
show = { label = "{{summary}}" }

[step.output]
summary = { type = "string", about = "what changed" }
"#;

/// Each turn keeps its prompt as prompt-STEP-N, says the session it is in as Claude Code's stream does (the one it
/// was given, else a new one), and a compaction where compact-N names the change step's turn. The change step's
/// attempts do what MODE says, one line each: `deliver` says hi, `miss` delivers without it, so the suite fails; a fix
/// it is asked in place changes nothing, so an attempt that missed fails whole and the next one starts.
const AGENT: &str = r#"#!/bin/sh
cat > "$BASE/in-$$"
step=$(sed -n 's/^# Your step: //p' "$BASE/in-$$" | head -1)
[ -z "$step" ] && step=fixing
k=$(( $(cat "$BASE/count-$step" 2>/dev/null || echo 0) + 1 ))
echo $k > "$BASE/count-$step"
mv "$BASE/in-$$" "$BASE/prompt-$step-$k"
n=$(( $(cat "$BASE/sessions" 2>/dev/null || echo 0) + 1 ))
echo $n > "$BASE/sessions"
session=${JAYNES_SESSION:-s$n}
echo "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"$session\"}"
if [ "$step" = look ]; then
  printf '{"seen":"a greeting"}' > .jaynes/output.json
elif [ "$step" = change ]; then
  [ -e "$BASE/compact-$k" ] && echo "{\"type\":\"system\",\"subtype\":\"compact_boundary\",\"session_id\":\"$session\",\"compact_metadata\":{\"trigger\":\"auto\",\"pre_tokens\":180000}}"
  [ "$(sed -n "${k}p" "$BASE/mode")" = deliver ] && perl -pi -e 's/hello/hi/' greeting.txt
  printf '{"summary":"hello becomes hi"}' > .jaynes/output.json
else
  printf '{"summary":"hello becomes hi"}' > .jaynes/output.json
fi
echo "{\"tokens\":100,\"session\":\"$session\"}"
"#;

struct Scratch {
    base: PathBuf,
    home: PathBuf,
    project: PathBuf,
    role: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _gone = std::fs::remove_dir_all(&self.base);
        }
    }
}

fn scratch(name: &str, role: &str, mode: &[&str]) -> Scratch {
    let base = std::env::temp_dir().join(format!("jaynes-lean-{name}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let project = base.join("project");
    std::fs::create_dir_all(&project).expect("dir");
    std::fs::write(project.join("greeting.txt"), "hello\n").expect("write");
    std::fs::write(project.join("Makefile"), "test:\n\tgrep -q hi greeting.txt\n").expect("write");
    for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]] {
        let ok = Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success();
        assert!(ok, "git {args:?}");
    }
    let dir = base.join("lean");
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("role.toml"), role).expect("write");
    std::fs::write(base.join("agent.sh"), AGENT).expect("write");
    std::fs::write(base.join("mode"), mode.join("\n") + "\n").expect("write");
    Scratch { home: base.join("home"), project, role: dir, base }
}

impl Scratch {
    fn env(&self, c: &mut Command) {
        c.env("JAYNES_ENGINE_HOME", &self.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "")
            .env("JAYNES_CHROME", "/nonexistent/no-test-browser").env("BASE", &self.base);
    }

    fn engine(&self, args: &[&str]) -> String {
        let mut c = Command::new(BIN);
        c.args(args);
        self.env(&mut c);
        let out = c.output().expect("engine");
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    }

    fn person(&self, args: &[&str]) -> String {
        let mut c = Command::new("python3");
        c.args(["-c", TTY, BIN]).args(args);
        self.env(&mut c);
        let out = c.output().expect("person");
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    }

    fn start(&self, run: &str) -> String {
        let agent = format!("command:sh {}", self.base.join("agent.sh").display());
        self.engine(&["start", "--run", run, "--role", self.role.to_str().expect("path"), "--project", self.project.to_str().expect("path"),
                      "--request", "say hi instead of hello", "--agent", &agent])
    }

    fn events(&self, run: &str) -> Vec<Value> {
        self.engine(&["log", run]).lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }

    fn prompt(&self, step: &str, k: usize) -> String {
        std::fs::read_to_string(self.base.join(format!("prompt-{step}-{k}"))).unwrap_or_else(|_| panic!("no prompt {k} of {step}"))
    }

    fn asked(&self, run: &str) -> Vec<String> {
        self.events(run).iter().filter(|e| e["type"] == "QuestionOpened" && e["question"]["step"] == "change")
            .filter_map(|e| e["question"]["id"].as_str().map(str::to_string)).collect()
    }

    /// The person sends the change back with `verdict`; once the next round asked again, its prompt.
    fn send_back(&self, run: &str, verdict: &str) -> String {
        let q = self.asked(run).first().cloned().expect("asked");
        let said = self.person(&["answer", run, &q, &format!("change={verdict}"), "--send"]);
        for _ in 0..150 {
            if self.asked(run).len() > 1 {
                return self.prompt("change", 2);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        panic!("the round sent back never asked again: {said}\n{:?}", self.events(run).iter().map(|e| e["type"].clone()).collect::<Vec<_>>());
    }
}

const REQUEST: &str = "# The request\n\nsay hi instead of hello\n\n";
const CANNOT: &str = "# If it cannot be done as asked";
const REPLY: &str = "# Your reply to the person";

fn hears_everything(p: &str) {
    assert!(p.starts_with(REQUEST), "{p}");
    assert!(p.contains(CANNOT) && p.contains("Jaynes's box refused"), "{p}");
    assert!(p.contains(REPLY), "{p}");
}

fn hears_what_is_its_own(p: &str) {
    assert!(p.contains("# Your step: change\n\nMake the change the request asks."), "the step's own text comes whole: {p}");
    assert!(p.contains("# Your output") && p.contains(r#"{"properties":{"summary""#), "and the schema: {p}");
}

/// Case 1: The step that goes on in the session of the one before it is not told again the request nor what to do when it
/// cannot be done, which that session heard; a round the person sends back with words brings the words whole, and
/// none of what the session already heard, its reply's text either.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_turn_in_a_session_that_heard_the_fixed_texts_gets_only_what_is_new() {
    let s = scratch("words", ROLE, &["deliver", "deliver"]);
    let out = s.start("words");
    assert!(out.contains("waiting"), "{out}");
    hears_everything_but_the_reply(&s.prompt("look", 1));
    let change = s.prompt("change", 1);
    assert!(!change.contains("# The request") && !change.contains(CANNOT), "the session heard them at look: {change}");
    assert!(change.contains(REPLY), "the reply's text is new to the session: {change}");
    hears_what_is_its_own(&change);
    let next = s.send_back("words", "change:say it in capitals");
    assert!(next.contains("# The person's words for this round\n\n- Change “hello becomes hi”: say it in capitals\n"), "the person's words come whole: {next}");
    assert!(!next.contains("# The request") && !next.contains(CANNOT) && !next.contains(REPLY), "{next}");
    hears_what_is_its_own(&next);
}

fn hears_everything_but_the_reply(p: &str) {
    assert!(p.starts_with(REQUEST), "{p}");
    assert!(p.contains(CANNOT), "{p}");
    assert!(!p.contains(REPLY), "nobody reads look's reply: {p}");
}

/// Case 2: The attempt after one the checks rejected goes on in its session, told why whole, and not the fixed texts again.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_next_attempt_hears_why_the_last_was_rejected_and_not_the_fixed_texts_again() {
    let s = scratch("again", ROLE, &["miss", "deliver"]);
    let out = s.start("again");
    assert!(out.contains("waiting") && s.base.join("prompt-change-2").exists(), "{out}");
    let next = s.prompt("change", 2);
    assert!(next.contains("# Why the last attempt was rejected\n\n- "), "the reason comes whole: {next}");
    assert!(!next.contains("# The request") && !next.contains(CANNOT) && !next.contains(REPLY), "{next}");
    hears_what_is_its_own(&next);
}

/// Case 3: The same step in a session of its own hears everything again, though another session heard it before.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_same_step_in_a_new_session_hears_everything() {
    let s = scratch("redo", &ROLE.replace("session = \"continue\"\n", ""), &["deliver", "deliver"]);
    let out = s.start("redo");
    assert!(out.contains("waiting"), "{out}");
    hears_everything(&s.prompt("change", 1));
    let next = s.send_back("redo", "redo");
    hears_everything(&next);
    hears_what_is_its_own(&next);
}

/// Case 4: A session Claude Code compacted lost what it heard before: the turn after hears everything, and why the last
/// attempt was rejected.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_turn_after_claude_code_compacted_the_session_hears_everything() {
    let s = scratch("compact", ROLE, &["miss", "deliver"]);
    std::fs::write(s.base.join("compact-1"), "").expect("write");
    let out = s.start("compact");
    assert!(out.contains("waiting"), "{out}");
    let next = s.prompt("change", 2);
    hears_everything(&next);
    assert!(next.contains("# Why the last attempt was rejected\n\n- "), "{next}");
    hears_what_is_its_own(&next);
}
