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

/// A stand-in for Claude Code that asks Jaynes's leave server for each shell command, as Claude Code asks it for a
/// call its settings do not allow, setting dangerouslyDisableSandbox as an agent may, and runs what comes back with
/// /bin/sh in the step's folder, as Claude Code's Bash runs it; it notes what each printed, or that it was refused.
fn claude_asking(s: &Scratch, commands: &[String]) {
    let bin = s.path("bin");
    std::fs::create_dir_all(&bin).expect("dir");
    let script = format!(
        r#"#!/usr/bin/python3
import json, os, subprocess, sys
args = sys.argv[1:]
with open({args_file:?}, "a") as f:
    f.write(" ".join(args) + "\n")
flag = lambda name: args[args.index(name) + 1] if name in args else ""
sys.stdin.read()
server = json.loads(flag("--mcp-config"))["mcpServers"]["jaynes"]
leave = subprocess.Popen([server["command"], *server["args"]], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
                         env={{**os.environ, **server.get("env", {{}})}})
n = 0
def call(method, params):
    global n
    n += 1
    leave.stdin.write(json.dumps({{"jsonrpc": "2.0", "id": n, "method": method, "params": params}}) + "\n")
    leave.stdin.flush()
    return json.loads(leave.stdout.readline())
call("initialize", {{"protocolVersion": "2025-06-18", "capabilities": {{}}, "clientInfo": {{"name": "stand-in", "version": "1"}}}})
said = []
for command in {commands}:
    answer = call("tools/call", {{"name": "permit", "arguments": {{"tool_name": "Bash", "input": {{"command": command, "dangerouslyDisableSandbox": True}}}}}})
    decision = json.loads(answer["result"]["content"][0]["text"])
    if decision["behavior"] != "allow":
        said.append("refused: " + decision.get("message", ""))
        continue
    ran = subprocess.run(["/bin/sh", "-c", decision["updatedInput"]["command"]], capture_output=True, text=True)
    said.append(ran.stdout.strip() if ran.returncode == 0 else "failed: " + ran.stderr.strip())
leave.stdin.close()
with open({said_file:?}, "w") as f:
    json.dump(said, f)
tools = flag("--tools").split(",")
print(json.dumps({{"type": "system", "subtype": "init", "session_id": "s1", "model": flag("--model"), "permissionMode": flag("--permission-mode"), "tools": tools}}))
print(json.dumps({{"type": "assistant", "parent_tool_use_id": None, "session_id": "s1", "message": {{"model": flag("--model"), "content": []}}}}))
os.makedirs(".jaynes", exist_ok=True)
text = open("greeting.txt").read().replace("hello", "hi")
open("greeting.txt", "w").write(text)
open(".jaynes/output.json", "w").write(json.dumps({{"summary": "The greeting now says hi.", "files": ["greeting.txt"]}}))
print(json.dumps({{"type": "result", "subtype": "success", "is_error": False, "result": "done", "session_id": "s1", "usage": {{"input_tokens": 10, "output_tokens": 5}}}}))
"#,
        args_file = s.path("claude.args").display().to_string(),
        said_file = s.path("claude.said").display().to_string(),
        commands = serde_json::to_string(commands).expect("json"),
    );
    std::fs::write(bin.join("claude"), script).expect("write");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "Jaynes's box is macOS's sandbox-exec")]
fn each_shell_command_of_a_claude_code_agent_runs_in_jaynes_s_box_and_a_cd_holds_for_the_next() {
    // the single box (the engineer lead, 22/11): the shell commands of Claude Code reach Jaynes's leave server, which
    // gives each back wrapped in Jaynes's box, never as the agent asked for it, not even when it asks for no sandbox
    let s = Scratch::new("box");
    let escaped = s.path("escaped");
    claude_asking(&s, &["mkdir -p sub && cd sub".into(), "pwd -P".into(), format!("touch {}", escaped.display()), "pwd -P".into(), "cd /usr && pwd -P".into(), "pwd -P".into()]);
    let (out, turns) = s.start("box", &small_change());
    assert_eq!(turns.len(), 1, "{out}");
    let turn = &turns[0];
    // Bash leaves the allow list, so every command comes to the leave server
    assert!(turn.contains("--permission-prompt-tool mcp__jaynes__permit"), "{turn}");
    let said: Vec<String> = serde_json::from_str(&std::fs::read_to_string(s.path("claude.said")).expect("said")).expect("json");
    assert_eq!(said.len(), 6, "{said:?}");
    // where the agent went, the next command starts
    assert!(said[1].ends_with("/sub") && said[1].contains("/runs/box/"), "{said:?}");
    // outside the step's folder nothing is written, however the agent asked
    assert!(said[2].starts_with("failed:"), "{said:?}");
    assert!(!escaped.exists());
    assert_eq!(said[3], said[1], "{said:?}");
    // a folder outside the step's is not kept: the next command starts at the step's top
    assert_eq!(said[4], "/usr", "{said:?}");
    assert_eq!(Path::new(&said[5]), Path::new(&said[1]).parent().expect("top"), "{said:?}");
    assert!(out.contains("finished: delivered"), "{out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "Jaynes's box is macOS's sandbox-exec")]
fn without_its_box_a_claude_code_agent_never_starts_and_the_turn_is_jaynes_s_fault() {
    // the engineer lead (22/11): without the box, or with the box's python3 failing, no command runs unboxed
    let s = Scratch::new("nobox");
    claude_asking(&s, &["pwd -P".into()]);
    // the python3 Jaynes runs its box with fails there, as one broken would; the role's program still runs
    std::fs::write(s.path("bin").join("python3"), "#!/bin/sh\ncase \"$*\" in *agent-run*) echo 'a broken python3' >&2; exit 1;; esac\nexec /usr/bin/python3 \"$@\"\n").expect("write");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(s.path("bin").join("python3"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let (out, turns) = s.start("nobox", &small_change());
    assert!(turns.is_empty(), "Claude Code started: {turns:?}");
    assert!(!s.path("claude.said").exists());
    let e = s.events("nobox");
    let fault = e.iter().find(|e| e.to_string().contains("a broken python3")).unwrap_or_else(|| panic!("no fault with its reason: {out}\n{e:?}"));
    assert_eq!((&fault["type"], &fault["step"], &fault["spent"]), (&serde_json::json!("AttemptFailed"), &serde_json::json!("change"), &serde_json::json!(false)), "{fault}");
    assert!(out.contains("blocked (Jaynes)"), "{out}");
}

/// A command run at a terminal, as when the person types it (`Scratch::at_terminal`): the program and its arguments follow.
/// Python's pty is the same on macOS and Linux, where script(1) takes its arguments differently. The terminal is opened
/// here and its program's side stays open here too, so what the program wrote before it ended is read whole and never
/// thrown away with the terminal (a test once read an empty `open-ports` on CI); the program's end is waited for, not
/// the terminal's, so a process it left holding the terminal never keeps the test waiting. When no terminal can be
/// had, the helper says why on its output, where the test's message shows it. What TTY_TYPED holds is typed at the
/// terminal, as the person answers a question.
const TTY: &str = r#"
import fcntl, os, select, sys, termios, time
typed = os.environ.pop("TTY_TYPED", "")
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
# the slave stays open here too, so what the program wrote before it ended is never thrown away with the terminal
if typed:
    os.write(master, typed.encode())
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

/// Databases and caches whose ports Jaynes holds while the project's test servers run; never 5432, which a person's
/// own Postgres may hold: the first free one is the test's.
const HELD_FREE: [u16; 6] = [3306, 6379, 27017, 11211, 9200, 5672];

/// A port free on 127.0.0.1, ::1, 0.0.0.0 and :: now, the first of `ports`.
fn free_everywhere(ports: &[u16]) -> u16 {
    use std::net::{Ipv4Addr, Ipv6Addr, TcpListener};
    *ports.iter().find(|&&p| {
        [Ipv4Addr::LOCALHOST.into(), Ipv6Addr::LOCALHOST.into(), Ipv4Addr::UNSPECIFIED.into(), Ipv6Addr::UNSPECIFIED.into()]
            .iter().all(|a: &std::net::IpAddr| TcpListener::bind((*a, p)).is_ok())
    }).expect("one of the ports is free")
}

impl Scratch {
    /// `jaynes` at a terminal, as the person types it, with `typed` as the answers.
    fn at_terminal(&self, args: &[&str], typed: &str) -> String {
        let out = Command::new("python3").args(["-c", TTY, BIN]).args(args)
            .env("HOME", self.path("person"))
            .env("PATH", format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", self.path("bin").display()))
            .env("JAYNES_ENGINE_HOME", self.path("home")).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "")
            .env("TTY_TYPED", typed)
            .output().expect("terminal");
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    }
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the window for test servers is macOS's box (Linux's box has a network of its own)")]
fn a_port_the_person_let_the_project_s_tests_use_the_agent_s_commands_serve_on_as_the_checks_do() {
    // the real-user's estoque (22/11): with the test servers on and a database's port let for the tests, the check
    // served on it and the agent's commands could not, so the agent could not run the suite the check runs
    let s = Scratch::new("freed");
    let project = s.path("project").to_str().expect("path").to_string();
    let port = free_everywhere(&HELD_FREE);
    claude_asking(&s, &[format!("python3 -c 'import socket; s=socket.socket(); s.bind((\"127.0.0.1\", {port})); s.listen(); \
                                 socket.create_connection((\"127.0.0.1\", {port}), timeout=3).close(); print(\"served on {port}\")'")]);
    assert!(s.at_terminal(&["project", "test-servers", &project], "y\n").contains("Done."));
    let said = s.at_terminal(&["project", "test-servers", &project, &port.to_string()], "y\n");
    assert!(said.contains("Done."), "{said}");
    let (out, turns) = s.start("freed", &small_change());
    assert_eq!(turns.len(), 1, "{out}");
    let said: Vec<String> = serde_json::from_str(&std::fs::read_to_string(s.path("claude.said")).expect("said")).expect("json");
    assert_eq!(said, vec![format!("served on {port}")], "{out}");
    assert!(out.contains("finished: delivered"), "{out}");
}
