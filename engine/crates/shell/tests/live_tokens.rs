//! The run's ceiling of tokens (G5) counts what a turn spent as its stream says it, while the turn runs (the
//! real-user, 22/11: a turn that died at its deadline showed "0 of 2000000 tokens" though its stream had said about
//! 129 thousand new tokens and 2.86 million read from the cache). A turn that dies, cut off by its deadline, a kill or
//! a crash, counts what its stream said up to there; a turn that passes the run's ceiling is cut as it passes it; and
//! a turn that died before its stream said anything is "not counted yet", never 0. The tokens read back from the
//! cache are kept apart, beside the count, and do not count towards the ceiling.
//! Claude Code here is a stand-in on the PATH whose stream says what it spent and stops midway: no model is reached.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

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
        let base = std::env::temp_dir().join(format!("jaynes-live-tokens-{name}-{}", std::process::id()));
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
        let bin = base.join("bin");
        std::fs::create_dir_all(&bin).expect("dir");
        std::fs::write(bin.join("claude"), STAND_IN).expect("write");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
        Scratch { base }
    }

    fn path(&self, p: &str) -> PathBuf {
        self.base.join(p)
    }

    fn jaynes(&self, args: &[&str], stream: &str) -> (bool, String) {
        let out = Command::new(BIN)
            .args(args)
            .env("HOME", self.path("person"))
            .env("PATH", format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", self.path("bin").display()))
            .env("JAYNES_ENGINE_HOME", self.path("home")).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "")
            .env_remove("JAYNES_CLAUDE_MODEL").env_remove("JAYNES_CLAUDE_EFFORT").env_remove("JAYNES_AGENT").env_remove("JAYNES_CODEX")
            .env("STREAM", stream)
            .output().expect("jaynes");
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    }

    fn events(&self, run: &str) -> Vec<Value> {
        let (ok, out) = self.jaynes(&["log", run], "");
        assert!(ok, "{out}");
        out.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }

    /// Start a small change whose Claude Code speaks `stream`; what Jaynes said, and how long it took.
    fn start(&self, run: &str, stream: &str) -> (String, Duration) {
        let role = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/roles/small-change");
        let project = self.path("project");
        let began = Instant::now();
        let (_ok, out) = self.jaynes(&["start", "--run", run, "--role", role.to_str().expect("path"), "--project", project.to_str().expect("path"),
                                       "--request", "say hi", "--agent", "claude"], stream);
        (out, began.elapsed())
    }

    /// The change's attempt as recorded: its usage, and why it ended.
    fn attempt(&self, run: &str) -> (Value, String) {
        let e = self.events(run);
        let ended = e.iter().find(|e| e["type"] == "AttemptFailed" || e["type"] == "AttemptInterrupted").unwrap_or_else(|| panic!("{e:?}"));
        let why = ended["reasons"].as_array().map(|r| r.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; "))
            .or_else(|| ended["reason"].as_str().map(str::to_string)).unwrap_or_default();
        (ended["usage"].clone(), why)
    }
}

/// A stand-in for Claude Code whose stream says what each message spent as Claude Code's does: a message's usage as
/// it starts (`message_start`), again on each of its whole lines (`assistant`, one a content block), its last output
/// count as it ends (`message_delta`); a subagent's messages beside the agent's. STREAM picks how it ends.
const STAND_IN: &str = r#"#!/usr/bin/env python3
import json, os, signal, sys, time
args = sys.argv[1:]
model = args[args.index("--model") + 1]
sys.stdin.read()
how = os.environ.get("STREAM", "")
def say(v):
    print(json.dumps(v), flush=True)
def start(id, usage, parent=None):
    say({"type": "stream_event", "session_id": "s1", "parent_tool_use_id": parent,
         "event": {"type": "message_start", "message": {"id": id, "model": model, "usage": usage}}})
def whole(id, usage, content, parent=None):
    say({"type": "assistant", "session_id": "s1", "parent_tool_use_id": parent,
         "message": {"id": id, "model": model, "content": [content], "usage": usage}})
def delta(output, parent=None):
    say({"type": "stream_event", "session_id": "s1", "parent_tool_use_id": parent,
         "event": {"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": output}}})
def die():
    os.kill(os.getpid(), signal.SIGKILL)
say({"type": "system", "subtype": "init", "session_id": "s1", "model": model, "permissionMode": "default", "tools": args[args.index("--tools") + 1].split(",")})
if how == "silent":
    die()
if how == "runs-on":
    for n, given in enumerate([1200000, 1000000]):
        u = {"input_tokens": given, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0, "output_tokens": 1}
        start("big%d" % n, u)
        whole("big%d" % n, u, {"type": "text", "text": "reading"})
    time.sleep(60)
    die()
if how == "quiet":
    # at work, and its stream has said nothing of what it spent yet
    with open(os.environ["STAND_IN_PID"], "w") as f:
        f.write(str(os.getpid()))
    time.sleep(30)
    die()
if how == "midway":
    # m1 and a subagent's message are said, and the turn goes on working: what show and the page read meanwhile
    with open(os.environ["STAND_IN_PID"], "w") as f:
        f.write(str(os.getpid()))
    u1 = {"input_tokens": 1000, "cache_creation_input_tokens": 200, "cache_read_input_tokens": 50000, "output_tokens": 3}
    start("m1", u1)
    whole("m1", u1, {"type": "text", "text": "Looking."})
    delta(400)
    s = {"input_tokens": 300, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0, "output_tokens": 20}
    start("sub1", s, "t1")
    whole("sub1", s, {"type": "text", "text": "found it"}, "t1")
    time.sleep(30)
    die()
# m1: two content blocks, then its last output count
u1 = {"input_tokens": 1000, "cache_creation_input_tokens": 200, "cache_read_input_tokens": 50000, "output_tokens": 3}
start("m1", u1)
whole("m1", u1, {"type": "text", "text": "Looking."})
whole("m1", u1, {"type": "tool_use", "id": "t1", "name": "Task", "input": {}})
delta(400)
# a subagent's message, beside the agent's
s = {"input_tokens": 300, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0, "output_tokens": 20}
start("sub1", s, "t1")
whole("sub1", s, {"type": "text", "text": "found it"}, "t1")
# m2 starts, and the turn dies before it ends
u2 = {"input_tokens": 1500, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 51000, "output_tokens": 1}
start("m2", u2)
whole("m2", u2, {"type": "tool_use", "id": "t2", "name": "Bash", "input": {"command": "make test"}})
die()
"#;

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_turn_that_dies_midway_counts_what_its_stream_said_up_to_there() {
    let s = Scratch::new("dies");
    let (out, _) = s.start("dies", "dies");
    let (usage, why) = s.attempt("dies");
    // m1 1000 + 200 + 400 (its last output count, once though two lines carried it), the subagent's 300 + 20, m2
    // 1500 + 1 as it started; the cache's reads apart
    assert_eq!(usage["tokens"], 3421, "{why}\n{out}");
    assert_eq!(usage["cached"], 101_000, "{usage}");
    assert!(usage.get("uncounted").is_none(), "{usage}");
    let (_, shown) = s.jaynes(&["show", "dies"], "");
    assert!(shown.contains("3421 of 2000000 tokens (101000 read from cache)"), "{shown}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_turn_is_cut_as_it_passes_the_run_s_ceiling_of_tokens() {
    let s = Scratch::new("over");
    let (out, took) = s.start("over", "runs-on");
    // its stream says 2.2 million new tokens against a ceiling of 2 million, then it would run a minute more
    assert!(took < Duration::from_secs(30), "cut as it passed, not at its end: {took:?}\n{out}");
    let (usage, why) = s.attempt("over");
    assert_eq!(usage["tokens"], 2_200_002, "{why}");
    assert!(why.contains("the turn was cut as it passed the run's ceiling: it spent 2200002 tokens, and the ceiling left it 2000000"), "{why}");
    let e = s.events("over");
    let blocked = e.iter().find(|e| e["type"] == "Blocked").unwrap_or_else(|| panic!("{e:?}"));
    assert_eq!(blocked["fault"], "ceiling", "{blocked}");
    assert!(out.contains("the run reached its ceiling: ") && out.contains("2200002 of 2000000 tokens"), "{out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_turn_that_dies_before_its_stream_says_what_it_spent_is_not_counted_yet() {
    let s = Scratch::new("silent");
    let (out, _) = s.start("silent", "silent");
    let (usage, why) = s.attempt("silent");
    assert_eq!(usage["tokens"], 0, "{why}\n{out}");
    assert_eq!(usage["uncounted"], 1, "{usage}");
    let (_, shown) = s.jaynes(&["show", "silent"], "");
    assert!(shown.contains("tokens not counted yet, of 2000000") && !shown.contains("0 of 2000000 tokens"), "{shown}");
}

/// The page, stopped when dropped.
struct Page(std::process::Child);

impl Drop for Page {
    fn drop(&mut self) {
        let _killed = self.0.kill();
        let _reaped = self.0.wait();
    }
}

/// The page on a port the system gives, and that port.
fn page(s: &Scratch) -> (Page, u16) {
    use std::io::BufRead;
    let mut child = Command::new(BIN)
        .args(["page", "--port", "0"])
        .env("HOME", s.path("person"))
        .env("JAYNES_ENGINE_HOME", s.path("home")).env("JAYNES_NO_PAGE", "1").env("JAYNES_NO_UPDATE_CHECK", "1").env("JAYNES_PAGE_KEY", "k3y")
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("page");
    let out = child.stderr.take().expect("what the page says");
    let page = Page(child);
    for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
        if let Some(port) = line.split("the page: http://localhost:").nth(1).and_then(|r| r.split('/').next()).and_then(|p| p.parse::<u16>().ok()) {
            return (page, port);
        }
    }
    panic!("the page said no port")
}

fn get(port: u16, path: &str) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(20))).expect("timeout");
    stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nX-Jaynes-Key: k3y\r\nConnection: close\r\n\r\n").as_bytes()).expect("write");
    let mut out = Vec::new();
    let _read = stream.read_to_end(&mut out);
    String::from_utf8_lossy(&out).to_string()
}

/// Start a small change in the background whose Claude Code speaks `stream` and writes its pid to `pid`.
fn started(s: &Scratch, run: &str, stream: &str, pid: &Path) -> std::process::Child {
    let role = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/roles/small-change");
    Command::new(BIN)
        .args(["start", "--run", run, "--role", role.to_str().expect("path"), "--project", s.path("project").to_str().expect("path"), "--request", "say hi", "--agent", "claude"])
        .env("HOME", s.path("person"))
        .env("PATH", format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", s.path("bin").display()))
        .env("JAYNES_ENGINE_HOME", s.path("home")).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "")
        .env_remove("JAYNES_CLAUDE_MODEL").env_remove("JAYNES_CLAUDE_EFFORT").env_remove("JAYNES_AGENT").env_remove("JAYNES_CODEX")
        .env("STREAM", stream).env("STAND_IN_PID", pid)
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .spawn().expect("start")
}

/// Stop the stand-in by the pid it wrote, and wait for the run's process.
fn stopped(pid: &Path, mut run: std::process::Child) {
    if let Ok(p) = std::fs::read_to_string(pid) {
        let _killed = Command::new("kill").args(["-KILL", p.trim()]).status();
    }
    let _ended = run.wait();
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn while_a_turn_whose_stream_said_nothing_yet_runs_its_tokens_are_not_counted_yet_never_0() {
    let s = Scratch::new("quiet");
    let pid = s.path("stand-in.pid");
    let run = started(&s, "quiet", "quiet", &pid);
    let began = Instant::now();
    while !pid.exists() && began.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(200));
    }
    let shown = s.jaynes(&["show", "quiet"], "").1;
    let (page, port) = page(&s);
    let html = get(port, "/run/quiet");
    drop(page);
    stopped(&pid, run);
    assert!(shown.contains("tokens not counted yet, of 2000000"), "{shown}");
    assert!(html.contains("tokens not counted yet, of 2,000,000") && !html.contains("0 of 2,000,000 tokens"), "{html}");
}

// The real-user, 22/11 (aa08fedc): with the agent working for over a minute and 13 lines of usage in its stream,
// show and the page said "spent 0 of 1200 s, 0 of 2000000 tokens (0 read from cache)".
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn while_a_turn_runs_show_and_the_page_say_what_its_stream_counted_so_far() {
    let s = Scratch::new("midway");
    let pid = s.path("stand-in.pid");
    let run = started(&s, "midway", "midway", &pid);
    // m1 1000 + 200 + 400 (its last output count), the subagent's 300 + 20; the cache's reads apart
    let counted = "1920 of 2000000 tokens (50000 read from cache)";
    let began = Instant::now();
    let mut shown = String::new();
    while began.elapsed() < Duration::from_secs(20) {
        shown = s.jaynes(&["show", "midway"], "").1;
        if shown.contains(counted) && !shown.contains("spent 0 of") {
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let (page, port) = page(&s);
    let (html, home) = (get(port, "/run/midway"), get(port, "/"));
    drop(page);
    stopped(&pid, run);
    assert!(shown.contains("running"), "the turn still runs: {shown}");
    assert!(shown.contains(counted), "show says what the stream counted so far: {shown}");
    assert!(!shown.contains("spent 0 of"), "the seconds of the turn running count too: {shown}");
    assert!(html.contains("1,920 of 2,000,000 tokens (50,000 read from the cache)"), "the run's page says it too: {html}");
    assert!(home.contains("1920 tokens"), "and the home: {home}");
}
