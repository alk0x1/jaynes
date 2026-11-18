//! The engine end to end, as a person runs it: the real binary, the real small-change role over the protocol, a
//! real git project, and an agent that is a shell command, so no tokens are spent.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The system's Python: roles need nothing of the Python engine, only the library inside the program.
fn python() -> String {
    "python3".to_string()
}

/// A command run at a terminal, as when the person types it: what follows `tty()` is the program and its arguments.
/// Python's pty is the same on macOS and Linux, where script(1) takes its arguments differently. Its own loop, not
/// pty.spawn, which on macOS's Python 3.9 waits for ever once the program is done.
const TTY: &str = r#"
import os, pty, sys
pid, fd = pty.fork()
if pid == 0:
    os.execvp(sys.argv[1], sys.argv[1:])
while True:
    try:
        data = os.read(fd, 65536)
    except OSError:
        break
    if not data:
        break
    os.write(1, data)
sys.exit(os.waitstatus_to_exitcode(os.waitpid(pid, 0)[1]))
"#;

fn tty() -> Command {
    let mut c = Command::new(python());
    c.args(["-c", TTY]).env("JAYNES_CHROME", test_browser().unwrap_or_else(|| PathBuf::from(NO_BROWSER)));
    c
}

/// The engine as the tests run it: it photographs only with a test browser, never the person's own Chrome. Without
/// a test browser it is named one that is not there, so it photographs with none.
fn jaynes() -> Command {
    let mut c = Command::new(BIN);
    c.env("JAYNES_CHROME", test_browser().unwrap_or_else(|| PathBuf::from(NO_BROWSER)));
    c
}

const NO_BROWSER: &str = "/nonexistent/jaynes-test-browser";

/// The test browser the photographs take: JAYNES_CHROME when the one running the tests names one, else Playwright's
/// headless shell, else its Chrome for Testing, the newest first. Never a browser where macOS keeps the person's
/// apps, whatever names it.
fn test_browser() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let mut roots: Vec<PathBuf> = std::env::var("PLAYWRIGHT_BROWSERS_PATH").map(PathBuf::from).into_iter().collect();
    roots.extend([home.join("Library/Caches/ms-playwright"), home.join(".cache/ms-playwright")]);
    test_browser_in(std::env::var("JAYNES_CHROME").ok().filter(|n| !n.is_empty()), &home, &roots)
}

fn test_browser_in(named: Option<String>, home: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let the_person_s = |p: &Path| {
        let real = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        [Path::new("/Applications"), &home.join("Applications")].iter().any(|apps| p.starts_with(apps) || real.starts_with(apps))
    };
    if let Some(named) = named {
        let p = PathBuf::from(named);
        return (p.is_file() && !the_person_s(&p)).then_some(p);
    }
    // (headless shell first, newest version first) among what Playwright installed
    let mut found: Vec<(u8, u32, PathBuf)> = Vec::new();
    for root in roots {
        for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let version = name.rsplit('-').next().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
            let (kind, inside): (u8, &[&str]) = if name.starts_with("chromium_headless_shell-") {
                (0, &["chrome-headless-shell-mac-arm64/chrome-headless-shell", "chrome-headless-shell-mac-x64/chrome-headless-shell",
                      "chrome-headless-shell-linux64/chrome-headless-shell", "chrome-linux/headless_shell"])
            } else if name.starts_with("chromium-") {
                (1, &["chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
                      "chrome-mac-x64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
                      "chrome-linux64/chrome", "chrome-linux/chrome"])
            } else {
                continue;
            };
            found.extend(inside.iter().map(|i| e.path().join(i)).filter(|p| p.is_file()).map(|p| (kind, u32::MAX - version, p)));
        }
    }
    found.sort();
    found.into_iter().map(|(_, _, p)| p).find(|p| !the_person_s(p))
}

#[test]
fn the_tests_photograph_with_a_test_browser_and_never_the_person_s_chrome() {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    // the person's Chrome is never taken, even named
    for apps in [PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"), home.join("Applications/Google Chrome.app/Contents/MacOS/Google Chrome")] {
        assert_eq!(test_browser_in(Some(apps.display().to_string()), &home, &[]), None, "{}", apps.display());
    }
    // what these tests take, here, is a test browser or none
    let chosen = test_browser();
    if let Some(b) = &chosen {
        assert!(!b.starts_with("/Applications") && !b.starts_with(home.join("Applications")), "{}", b.display());
    }
    eprintln!("the test browser here: {}", chosen.map(|b| b.display().to_string()).unwrap_or_else(|| "none, so the photographs are skipped".into()));
}

/// Without a test browser, a test that photographs says it skipped, and is not run on the person's Chrome.
fn skipped_without_a_test_browser() -> bool {
    let none = test_browser().is_none();
    if none {
        eprintln!("skipped: no test browser (JAYNES_CHROME, or Playwright's headless shell or Chrome for Testing: npx playwright install chromium-headless-shell)");
    }
    none
}

/// A port free now, the system's choice, so two runs of these tests at once never ask for the same one.
/// How long a test waits on a page: past it, the test fails saying what answered, instead of holding the queue.
const PAGE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Wait until Jaynes's page answers on `port`: its /api/version, with the record's sequence, and nothing else's.
/// A program of someone else's listening there fails the test at once, saying what it answered.
fn page_up(port: u16) {
    let until = std::time::Instant::now() + PAGE_WAIT;
    let mut said = String::new();
    while std::time::Instant::now() < until {
        if let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) {
            use std::io::{Read, Write};
            let _timed = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
            let _timed = s.set_write_timeout(Some(std::time::Duration::from_secs(2)));
            let _sent = s.write_all(format!("GET /api/version HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes());
            said.clear();
            let _read = s.read_to_string(&mut said);
            if said.starts_with("HTTP/1.1 200") && said.contains("\"seq\"") {
                return;
            }
            assert!(said.is_empty(), "something that is not Jaynes's page answers on port {port}: {}", said.chars().take(300).collect::<String>());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("Jaynes's page did not come up on port {port} within {} s", PAGE_WAIT.as_secs());
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port()).expect("a free port")
}

struct Scratch {
    base: PathBuf,
    home: PathBuf,
    project: PathBuf,
}

/// A test's folders go when it passes; a failed one keeps them for whoever reads why, and JAYNES_KEEP_TEST_RUNS=1
/// keeps every one, for the step of the CI that proves the Lean model against the records the runs left.
impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() && std::env::var("JAYNES_KEEP_TEST_RUNS").as_deref() != Ok("1") {
            let _gone = std::fs::remove_dir_all(&self.base);
        }
    }
}

/// A run of these tests that was stopped before its folders went (killed, or kept for the Lean model) leaves them
/// behind: what an earlier run left more than an hour ago is swept here, so a machine that runs the tests often does
/// not fill up.
fn sweep() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
        let mine = format!("-{}", std::process::id());
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with("jaynes-e2e-") || name.ends_with(&mine) {
                continue;
            }
            let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > 3600);
            if old {
                let _gone = std::fs::remove_dir_all(e.path());
            }
        }
    });
}

fn scratch(name: &str) -> Scratch {
    sweep();
    let base = std::env::temp_dir().join(format!("jaynes-e2e-{name}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let project = base.join("project");
    std::fs::create_dir_all(&project).expect("dir");
    std::fs::write(project.join("greeting.txt"), "hello\n").expect("write");
    std::fs::write(project.join("Makefile"), "test:\n\tgrep -q hi greeting.txt\n").expect("write");
    for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]] {
        let ok = Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success();
        assert!(ok, "git {args:?}");
    }
    Scratch { home: base.join("home"), project, base }
}

fn engine(s: &Scratch, args: &[&str]) -> (bool, String) {
    let out = jaynes()
        .args(args)
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .output()
        .expect("engine");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

fn start(s: &Scratch, run: &str, agent: &str) -> String {
    let role = root().join("crates/shell/tests/roles/small-change");
    let (ok, out) = engine(s, &[
        "start", "--run", run, "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
        "--request", "say hi instead of hello", "--agent", &format!("command:{agent}"),
    ]);
    assert!(ok, "start failed:\n{out}");
    out
}

fn events(s: &Scratch, run: &str) -> Vec<Value> {
    let (ok, out) = engine(s, &["log", run]);
    assert!(ok, "{out}");
    out.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn types(e: &[Value]) -> Vec<String> {
    e.iter().map(|e| e["type"].as_str().unwrap_or("").to_string()).collect()
}

const OUTPUT: &str = r#"printf '{"summary":"hello becomes hi","files":["greeting.txt"]}' > .jaynes/output.json"#;

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_small_change_is_delivered_as_a_branch_with_no_stop() {
    let s = scratch("small");
    let prompt = s.home.parent().expect("base").join("prompt.txt");
    let agent = format!("cat > {} && perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT} && echo '{{\"tokens\":1234,\"session\":\"s1\"}}'", prompt.display());
    let out = start(&s, "small", &agent);
    // the role's instructions, with the data Jaynes filled in wrapped in tags
    let said = std::fs::read_to_string(&prompt).expect("prompt");
    assert!(said.contains("<output step=\"runner\" field=\"suite\">make test</output>"), "{said}");
    assert!(said.contains("field-required: the output has summary, not empty") && said.contains("suite-passes: the project's whole suite passes"), "{said}");
    assert!(out.contains("finished: delivered"), "{out}");
    let e = events(&s, "small");
    let t = types(&e);
    assert!(!t.contains(&"QuestionOpened".to_string()), "no stop on a small change: {t:?}");
    assert!(e[0]["agent"].as_str().unwrap_or("").starts_with("command:cat"), "the run records its agent: {}", e[0]);
    // the person's working tree is untouched, and the change is on the run's branch
    assert_eq!(std::fs::read_to_string(s.project.join("greeting.txt")).expect("read"), "hello\n");
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/small:greeting.txt"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "hi\n");
    // G5: the tokens the agent program reported are recorded
    let finished = e.iter().find(|e| e["type"] == "RunFinished").expect("finished");
    assert_eq!(finished["cost"]["usage"]["tokens"], 1234);
    // G15: the record holds
    let (ok, out) = engine(&s, &["verify"]);
    assert!(ok, "{out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_failing_suite_is_fixed_in_the_same_session_without_spending_an_attempt() {
    let s = scratch("fix");
    // first the agent writes "hey" (the suite wants "hi"); asked to fix, in its session, it writes "hi"
    let agent = format!(
        "if [ -n \"$JAYNES_SESSION\" ]; then perl -pi -e 's/hey/hi/' greeting.txt; else perl -pi -e 's/hello/hey/' greeting.txt; fi && {OUTPUT} && echo '{{\"session\":\"s1\"}}'"
    );
    let out = start(&s, "fix", &agent);
    assert!(out.contains("finished: delivered"), "{out}");
    let e = events(&s, "fix");
    let t = types(&e);
    assert_eq!(t.iter().filter(|x| *x == "FixAsked").count(), 1, "{t:?}");
    assert_eq!(t.iter().filter(|x| *x == "AttemptStarted").count(), 2, "one attempt for runner, one for change: {t:?}");
    let finished = e.iter().find(|e| e["type"] == "RunFinished").expect("finished");
    assert_eq!(finished["cost"]["spent"], 0);
    assert_eq!(finished["cost"]["fixes"], 1);
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_change_to_a_file_that_runs_later_raises_the_risk_and_asks_the_person() {
    let s = scratch("risky");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && printf 'test:\\n\\ttrue\\n' > Makefile && {OUTPUT}");
    let out = start(&s, "risky", &agent);
    assert!(out.contains("risk raised to high"), "{out}");
    assert!(out.contains("waiting: delivered; your answers can still correct it"), "{out}");
    // a program without a terminal is an agent: it may not send the change back
    let state: Vec<Value> = events(&s, "risky");
    let q = state.iter().find(|e| e["type"] == "QuestionOpened").expect("asked")["question"]["id"].as_str().expect("id").to_string();
    let (ok, out) = engine(&s, &["answer", "risky", &q, "change=redo:smaller", "--send"]);
    assert!(!ok && out.contains("sending it back is yours"), "{out}");
    // the person, at a terminal, keeps it
    let script = tty()
        .args([BIN, "answer", "risky", &q, "--send"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .output()
        .expect("script");
    let said = String::from_utf8_lossy(&script.stdout);
    assert!(said.contains("is kept") && said.contains("finished: delivered"), "{said}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn an_agent_lets_a_change_that_is_not_small_go_on_and_it_counts_as_the_agent_s() {
    let s = scratch("release");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && printf 'test:\\n\\ttrue\\n' > Makefile && {OUTPUT}");
    start(&s, "release", &agent);
    let e = events(&s, "release");
    let q = e.iter().find(|e| e["type"] == "QuestionOpened").expect("asked")["question"]["id"].as_str().expect("id").to_string();
    let (ok, out) = engine(&s, &["answer", "release", &q, "--send"]);
    assert!(ok && out.contains("counts as the agent's") && out.contains("finished: delivered"), "{out}");
    let e = events(&s, "release");
    let answered = e.iter().find(|e| e["type"] == "AnswerRecorded").expect("answered");
    assert_eq!(answered["answer"]["by"]["by"], "agent");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn an_agent_program_that_does_not_start_spends_no_attempt() {
    let s = scratch("fault");
    let out = start(&s, "fault", "echo 'no such program' >&2; exit 3");
    assert!(out.contains("blocked (Jaynes)"), "{out}");
    let e = events(&s, "fault");
    assert!(e.iter().all(|e| !(e["type"] == "AttemptFailed" && e["spent"] == true)), "{e:?}");
}

#[test]
fn the_protocol_schema_on_disk_is_the_one_the_engine_speaks() {
    let s = scratch("schema");
    let (ok, out) = engine(&s, &["schema"]);
    assert!(ok);
    let on_disk = std::fs::read_to_string(root().join("protocol/role-protocol-1.schema.json")).expect("schema file");
    assert_eq!(out.trim(), on_disk.trim(), "regenerate it: jaynes schema > engine/protocol/role-protocol-1.schema.json");
}

#[test]
fn the_small_change_role_conforms() {
    let s = scratch("conform");
    let (ok, out) = engine(&s, &["conformance", root().join("crates/shell/tests/roles/small-change").to_str().expect("path")]);
    assert!(ok, "{out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn an_input_sent_while_another_process_drives_waits_in_the_inbox() {
    let s = scratch("inbox");
    let role = root().join("crates/shell/tests/roles/small-change");
    let agent = format!("sleep 4 && perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    let child = jaynes()
        .args(["start", "--run", "inbox", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start");
    std::thread::sleep(std::time::Duration::from_secs(2));
    let (ok, out) = engine(&s, &["halt", "inbox"]);
    assert!(ok && out.contains("recorded for the process that drives the run"), "{out}");
    let done = child.wait_with_output().expect("wait");
    let said = format!("{}{}", String::from_utf8_lossy(&done.stdout), String::from_utf8_lossy(&done.stderr));
    // the halt stopped the attempt under way; nothing was delivered
    assert!(said.contains("(halted)"), "{said}");
    let t = types(&events(&s, "inbox"));
    assert!(t.contains(&"Halted".to_string()), "{t:?}");
    assert!(!t.contains(&"Delivered".to_string()), "nothing is delivered while halted: {t:?}");
}

/// A halt stops the agent on a machine without the kill program, as a minimal Linux image is: Jaynes signals the
/// agent's process group itself. The role has no step that needs the box, so this holds on every system.
#[test]
fn a_halt_stops_the_agent_on_a_machine_without_the_kill_program() {
    let s = scratch("no-kill");
    let base = s.home.parent().expect("base").to_path_buf();
    // a PATH with only what the run and its agent use, and no kill
    let bin = base.join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    for tool in ["sh", "git", "python3", "sleep", "touch", "cat", "echo", "printf", "mkdir", "rm"] {
        let found = Command::new("sh").arg("-c").arg(format!("command -v {tool}")).output().expect("which");
        let path = String::from_utf8_lossy(&found.stdout).trim().to_string();
        assert!(!path.is_empty(), "{tool} is on this machine");
        std::os::unix::fs::symlink(&path, bin.join(tool)).expect("link");
    }
    assert!(!bin.join("kill").exists(), "the PATH, only this folder, has no kill program");
    let role = base.join("wait");
    std::fs::create_dir_all(&role).expect("role");
    std::fs::write(role.join("role.toml"), r#"
role = "wait"
version = "1.0.0"

[[step]]
name = "change"
reach = { project = "write", write = ["**"], commands = true }
instructions = "Do {{request}}."

[step.output]
summary = { type = "string", about = "what changed" }
"#).expect("write");
    let pid = base.join("agent.pid");
    let agent = format!("echo $$ > {}; sleep 60", pid.display());
    let child = Command::new(BIN)
        .args(["start", "--run", "no-kill", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "wait", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", "python3")
        .env("PYTHONPATH", "")
        .env("PATH", &bin)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start");
    let mut child = child;
    let waiting = std::time::Instant::now();
    while !pid.exists() {
        if let Ok(Some(_)) = child.try_wait() {
            let done = child.wait_with_output().expect("wait");
            panic!("the run ended before its agent started:\n{}{}", String::from_utf8_lossy(&done.stdout), String::from_utf8_lossy(&done.stderr));
        }
        assert!(waiting.elapsed() < std::time::Duration::from_secs(60), "the agent never started");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    let agent_pid = std::fs::read_to_string(&pid).expect("pid").trim().to_string();
    let asked = std::time::Instant::now();
    let halted = Command::new(BIN).args(["halt", "no-kill"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("PATH", &bin)
        .output().expect("halt");
    assert!(halted.status.success(), "{}{}", String::from_utf8_lossy(&halted.stdout), String::from_utf8_lossy(&halted.stderr));
    let done = child.wait_with_output().expect("wait");
    let said = format!("{}{}", String::from_utf8_lossy(&done.stdout), String::from_utf8_lossy(&done.stderr));
    assert!(asked.elapsed() < std::time::Duration::from_secs(10), "the agent went on after the halt: {:?}\n{said}", asked.elapsed());
    assert!(said.contains("(halted)"), "{said}");
    let alive = Command::new("kill").args(["-0", &agent_pid]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    assert!(!alive, "the agent is still running: {said}");
}

/// Where Jaynes has no box for a project's commands, a run never runs them open: it stops at once, says why, and
/// nothing of the project runs, the agent included.
#[test]
#[cfg(not(target_os = "macos"))]
fn without_a_box_a_run_stops_at_once_and_nothing_of_the_project_runs() {
    let s = scratch("no-box");
    let ran = s.home.parent().expect("base").join("agent-ran");
    let role = root().join("crates/shell/tests/roles/small-change");
    let started = std::time::Instant::now();
    let out = jaynes()
        .args(["start", "--run", "no-box", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi", "--agent", &format!("command:touch {} && sleep 60", ran.display())])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .env_remove("JAYNES_UNBOXED")
        .output()
        .expect("start");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(started.elapsed() < std::time::Duration::from_secs(30), "the run waited: {:?}\n{said}", started.elapsed());
    assert!(said.contains("blocked") && said.contains("no box for project commands"), "the person is told why: {said}");
    assert!(!ran.exists(), "the agent ran without a box: {said}");
    let e = events(&s, "no-box");
    let t = types(&e);
    assert!(t.contains(&"Blocked".to_string()), "{t:?}");
    assert!(e.iter().any(|e| e["type"] == "Blocked" && e["fault"] == "jaynes"), "the machine's fault, not the role's: {e:?}");
    assert!(e.iter().filter(|e| e["type"] == "AttemptFailed").all(|e| e["spent"] == false), "a missing box spends no attempt: {e:?}");
    assert!(!t.iter().any(|e| e == "StepDone" || e == "Delivered" || e == "SessionStarted"), "nothing ran: {t:?}");
    // the missing box is the machine's to fix, as a missing Chrome is: not the role's, and no attempt is spent
    assert!(said.contains("No attempt was spent") && !said.contains("the role says"), "{said}");
    assert_eq!(std::fs::read_to_string(s.project.join("greeting.txt")).expect("read"), "hello\n", "the project is as it was");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_halt_stops_the_agent_at_once_and_its_attempt_is_not_spent() {
    let s = scratch("halt-now");
    let role = root().join("crates/shell/tests/roles/small-change");
    // the first turn would work for a minute; the one after the resume is quick
    let once = s.home.parent().expect("base").join("once");
    let agent = format!("if [ -e {o} ]; then perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}; else touch {o}; sleep 60; fi", o = once.display());
    let child = jaynes()
        .args(["start", "--run", "halt-now", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start");
    // the agent starts within seconds, or the run ended before it could: then say why rather than wait for ever
    let mut child = child;
    let waiting = std::time::Instant::now();
    while !once.exists() {
        if let Ok(Some(_)) = child.try_wait() {
            let done = child.wait_with_output().expect("wait");
            panic!("the run ended before its agent started:\n{}{}", String::from_utf8_lossy(&done.stdout), String::from_utf8_lossy(&done.stderr));
        }
        assert!(waiting.elapsed() < std::time::Duration::from_secs(60), "the agent never started");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let asked = std::time::Instant::now();
    let (ok, out) = engine(&s, &["halt", "halt-now"]);
    assert!(ok, "{out}");
    let done = child.wait_with_output().expect("wait");
    let said = format!("{}{}", String::from_utf8_lossy(&done.stdout), String::from_utf8_lossy(&done.stderr));
    assert!(asked.elapsed() < std::time::Duration::from_secs(10), "the agent went on after the halt: {:?}\n{said}", asked.elapsed());
    assert!(said.contains("(halted)"), "{said}");
    let t = types(&events(&s, "halt-now"));
    assert!(t.contains(&"Halted".to_string()) && t.contains(&"AttemptInterrupted".to_string()), "{t:?}");
    assert!(!t.contains(&"AttemptFailed".to_string()), "a halt spends no attempt: {t:?}");
    // the person resumes, at a terminal (script gives the command one), and the step runs again from where it was
    let out = tty()
        .args([BIN, "resume", "halt-now"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .output()
        .expect("resume");
    let out = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.contains("finished: delivered"), "{out}");
    let t = types(&events(&s, "halt-now"));
    assert!(t.contains(&"Resumed".to_string()) && !t.contains(&"AttemptFailed".to_string()), "{t:?}");
}

fn bug_project(name: &str) -> Scratch {
    let s = scratch(name);
    std::fs::write(s.project.join("greet.sh"), "echo \"hello $1\"\n").expect("write");
    std::fs::write(s.project.join("Makefile"), "test:\n\tfor t in tests/*.sh; do [ -e \"$$t\" ] || continue; sh \"$$t\" || exit 1; done\n").expect("write");
    for args in [vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "bug"]] {
        assert!(Command::new("git").arg("-C").arg(&s.project).args(&args).status().expect("git").success());
    }
    s
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn fix_bug_proves_the_fix_with_cases_the_maker_never_sees() {
    let s = bug_project("fixbug");
    let prompts = s.home.join("prompts");
    std::fs::create_dir_all(&prompts).expect("dir");
    let agent = root().join("crates/shell/tests/agents/fix-bug.sh");
    let role = root().join("crates/shell/tests/roles/fix-bug");
    let out = jaynes()
        .args(["start", "--run", "fixbug", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "greet should say hi with the name, not hello", "--agent", &format!("command:sh {}", agent.display())])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .env("PROMPTS", &prompts)
        .output()
        .expect("start");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(said.contains("delivered: jaynes/fixbug"), "{said}");
    let e = events(&s, "fixbug");
    // the incomplete fix passed the open checks and was caught by the hidden one, spending one attempt
    let failed: Vec<&Value> = e.iter().filter(|e| e["type"] == "AttemptFailed" && e["step"] == "fix").collect();
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0]["reasons"][0], "A check you cannot see found that this does not hold: greet says hi with the name");
    // G3: no prompt but the hidden step's own shows the case's command, and no agent was told the check's name
    let mut seen_sentence = false;
    for f in std::fs::read_dir(&prompts).expect("prompts").flatten() {
        let p = std::fs::read_to_string(f.path()).expect("read");
        if p.contains("# Your step: cases") {
            continue;
        }
        assert!(!p.contains("grep -q") && !p.contains("proof"), "a prompt leaked the hidden check:\n{p}");
        seen_sentence |= p.contains("greet says hi with the name");
    }
    assert!(seen_sentence, "the second fix was told the sentence of the failed case");
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/fixbug:greet.sh"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "echo \"hi $1\"\n");
}

fn http(port: u16, raw: &str) -> String {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(PAGE_WAIT)).expect("timeout");
    s.set_write_timeout(Some(PAGE_WAIT)).expect("timeout");
    s.write_all(raw.as_bytes()).expect("write");
    let mut out = Vec::new();
    if let Err(e) = s.read_to_end(&mut out) {
        panic!("the page on port {port} did not answer within {} s ({e}); what came: {}", PAGE_WAIT.as_secs(), String::from_utf8_lossy(&out).chars().take(300).collect::<String>());
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn post(port: u16, path: &str, body: &str, cookie: &str) -> Value {
    let raw = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nContent-Type: application/json\r\nCookie: {cookie}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let out = http(port, &raw);
    serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap_or("")).unwrap_or(Value::Null)
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_page_answers_as_the_person_only_with_its_key() {
    let s = scratch("page");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && printf 'test:\\n\\ttrue\\n' > Makefile && {OUTPUT}");
    start(&s, "page", &agent);
    let e = events(&s, "page");
    let q = e.iter().find(|e| e["type"] == "QuestionOpened").expect("asked")["question"].clone();
    let shown = jaynes_core::question::Question::shown(&serde_json::from_value(q.clone()).expect("question")).0;
    let port = free_port();
    let _page = jaynes()
        .args(["page", "--port", &port.to_string()])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .env("JAYNES_PAGE_KEY", "k3y")
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(Page)
        .expect("page");
    page_up(port);
    // another address is refused, the key sets a cookie, and the question is on the page
    assert!(http(port, &format!("GET / HTTP/1.1\r\nHost: evil.example:{port}\r\n\r\n")).starts_with("HTTP/1.1 403"));
    let keyed = http(port, &format!("GET /?key=k3y HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"));
    assert!(keyed.contains("Set-Cookie: jaynes_key=k3y"));
    // the cookie outlives the browser's restarts, as the key outlives the page's: the link is opened once
    assert!(keyed.contains("Max-Age=31536000"), "{keyed}");
    let home = http(port, &format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: jaynes_key=k3y\r\n\r\n"));
    assert!(home.contains("The change turned out bigger than a small one"), "{home}");
    // each run shows its steps as a strip, as the earlier page did
    assert!(home.contains("<div class=\"strip\"><i class=\"\" title=\"runner\"></i><i class=\"waiting\" title=\"change\"></i></div>"), "{home}");
    assert!(home.contains("step 2 of 2") || home.contains("step 1 of 2"), "{home}");
    let id = q["id"].as_str().expect("id");
    // without the key it is an agent: it may not send the change back
    let redo = format!(r#"{{"run":"page","question":"{id}","shown":"{shown}","verdicts":{{"change":"redo"}},"notes":{{"change":"smaller"}}}}"#);
    let r = post(port, "/api/effects", &redo, "");
    assert_eq!(r["ok"], false, "{r}");
    // with the key, the core writes what the answer will do, and the answer is the person's
    let keep = format!(r#"{{"run":"page","question":"{id}","shown":"{shown}","verdicts":{{}},"context":"for accountants"}}"#);
    let r = post(port, "/api/effects", &keep, "jaynes_key=k3y");
    assert!(r["ok"] == true && r["sentences"].to_string().contains("Your context goes to every step after change"), "{r}");
    let r = post(port, "/api/answer", &keep, "jaynes_key=k3y");
    assert_eq!(r["ok"], true, "{r}");
    // the run goes on in its own process and finishes
    let mut finished = false;
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if events(&s, "page").iter().any(|e| e["type"] == "RunFinished") {
            finished = true;
            break;
        }
    }
    assert!(finished, "{:?}", types(&events(&s, "page")));
    let answered = events(&s, "page").into_iter().find(|e| e["type"] == "AnswerRecorded").expect("answered");
    assert_eq!(answered["answer"]["by"]["by"], "person");
    // with nothing waiting, the page opens on home: what was delivered, newest first, each a link to its run
    let home = http(port, &format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: jaynes_key=k3y\r\n\r\n"));
    assert!(home.contains("<h2>Delivered</h2>") && home.contains("<a class=\"hrow\" href=\"/run/page\">") && home.contains("Nothing waits for you"), "{home}");
    assert!(http(port, &format!("GET /home HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: jaynes_key=k3y\r\n\r\n")).contains("<a class=\"hrow\" href=\"/run/page\">"));
    assert_eq!(answered["answer"]["context"], "for accountants");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn sending_an_earlier_step_back_reruns_it_without_the_later_steps_changes() {
    let s = bug_project("redo");
    let prompts = s.home.join("prompts");
    std::fs::create_dir_all(&prompts).expect("dir");
    let agent = root().join("crates/shell/tests/agents/fix-bug.sh");
    let role = root().join("crates/shell/tests/roles/fix-bug");
    let env = |c: &mut Command| {
        c.env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "").env("PROMPTS", &prompts);
    };
    let mut c = jaynes();
    c.args(["start", "--run", "redo", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
            "--request", "greet should say hi with the name", "--agent", &format!("command:sh {}", agent.display())]);
    env(&mut c);
    assert!(c.output().expect("start").status.success());
    let q = events(&s, "redo").into_iter().find(|e| e["type"] == "QuestionOpened" && e["question"]["step"] == "reproduce").expect("asked")["question"]["id"].as_str().expect("id").to_string();
    // the person, at a terminal, sends reproduce back after the fix was made
    let mut c = tty();
    c.args([BIN, "answer", "redo", &q, "examples-1=redo:also an empty name", "--send"]);
    env(&mut c);
    let said = String::from_utf8_lossy(&c.output().expect("answer").stdout).to_string();
    let e = events(&s, "redo");
    let t = types(&e);
    // reproduce reran over the base, where the bug still is, and passed; fix ran again on top and was delivered
    let reproduce_failed = e.iter().filter(|e| e["type"] == "AttemptFailed" && e["step"] == "reproduce").count();
    assert_eq!(reproduce_failed, 0, "{said}\n{t:?}");
    assert!(e.iter().filter(|e| e["type"] == "StepDone" && e["step"] == "reproduce").count() == 2, "{t:?}");
    assert!(e.iter().filter(|e| e["type"] == "StepDone" && e["step"] == "fix").count() == 2, "{t:?}");
    let delivered = e.iter().filter(|e| e["type"] == "Delivered").count();
    assert_eq!(delivered, 2, "{t:?}");
    {
        let (file, want) = ("greet.sh", "echo \"hi $1\"\n");
        let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", &format!("jaynes/redo:{file}")]).output().expect("git");
        assert_eq!(String::from_utf8_lossy(&shown.stdout), want);
    }
    for test in ["tests/test_hi.sh", "tests/test_empty.sh"] {
        let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", &format!("jaynes/redo:{test}")]).output().expect("git");
        assert!(shown.status.success(), "{test} is in the delivery");
    }
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_crash_in_the_middle_of_a_turn_stops_the_agent_left_running_and_spends_no_attempt() {
    let s = scratch("crash");
    let mark = s.home.join("mark");
    std::fs::create_dir_all(&mark).expect("dir");
    let agent = format!(
        "if [ ! -e {m}/started ]; then echo '{{\"type\":\"system\",\"session_id\":\"s9\",\"model\":\"m9\",\"permissionMode\":\"default\",\"tools\":[\"Read\",\"Bash\"]}}'; touch {m}/started; sleep 20; touch {m}/orphan-finished; exit 0; fi; echo \"$JAYNES_SESSION\" > {m}/resumed; perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}",
        m = mark.display()
    );
    let role = root().join("crates/shell/tests/roles/small-change");
    let mut engine_process = jaynes()
        .args(["start", "--run", "crash", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start");
    for _ in 0..100 {
        if mark.join("started").exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(mark.join("started").exists(), "the agent started");
    // the engine dies at once, as with kill -9: nothing of it runs any more, and its agent goes on
    let _killed = engine_process.kill();
    let _waited = engine_process.wait();
    let pidfile = s.home.join("runs/crash/agent.pid");
    let pgid = std::fs::read_to_string(&pidfile).expect("the agent's process group is noted");
    let alive = |g: &str| Command::new("kill").args(["-0", "--", &format!("-{}", g.trim())]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    assert!(alive(&pgid), "the agent outlived the engine");
    // the next process stops what was left, and the run goes on with an attempt that is not counted against the agent
    let (ok, out) = engine(&s, &["continue", "crash"]);
    assert!(ok && out.contains("stopped an agent program") && out.contains("finished: delivered"), "{out}");
    assert!(!alive(&pgid), "the agent left running was stopped");
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!mark.join("orphan-finished").exists(), "the stopped agent never finished its work");
    // the session was recorded as the turn started, and the next attempt went on in it
    assert_eq!(std::fs::read_to_string(mark.join("resumed")).expect("resumed").trim(), "s9");
    // what the agent program said it runs with, in its first message, is in the record
    let started = events(&s, "crash").into_iter().find(|e| e["type"] == "SessionStarted").expect("session");
    assert_eq!((started["confirmed"]["model"].as_str(), started["confirmed"]["tools"][1].as_str()), (Some("m9"), Some("Bash")), "{started}");
    let e = events(&s, "crash");
    assert!(e.iter().any(|e| e["type"] == "AttemptInterrupted"), "{:?}", types(&e));
    let finished = e.iter().find(|e| e["type"] == "RunFinished").expect("finished");
    assert_eq!(finished["cost"]["spent"], 0);
}

#[test]
fn no_run_goes_on_over_a_record_that_was_changed() {
    let s = scratch("tamper");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    start(&s, "tamper", &agent);
    let db = s.home.join("record.sqlite3");
    let ok = Command::new("sqlite3")
        .arg(&db)
        .arg("update events set body = replace(body, 'say hi instead of hello', 'push to production') where type = 'RunStarted'")
        .status()
        .expect("sqlite3");
    assert!(ok.success());
    let (ok, out) = engine(&s, &["verify"]);
    assert!(!ok && out.contains("changed at entry 1"), "{out}");
    let (ok, out) = engine(&s, &["continue", "tamper"]);
    assert!(!ok && out.contains("drives no run over a record that does not hold"), "{out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_page_halts_resumes_and_records_a_correction() {
    let s = scratch("acts");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && printf 'test:\\n\\ttrue\\n' > Makefile && {OUTPUT}");
    start(&s, "acts", &agent);
    let port = free_port();
    let _page = jaynes()
        .args(["page", "--port", &port.to_string()])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .env("JAYNES_PAGE_KEY", "k3y")
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(Page)
        .expect("page");
    page_up(port);
    let me = "jaynes_key=k3y";
    let view = http(port, &format!("GET /run/acts HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: {me}\r\n\r\n"));
    assert!(view.contains("class=\"stages told\"") && view.contains("waiting for you"), "{view}");
    // what the run waits for is the first thing to do there: a way to the question, apart from the run's controls
    assert!(view.contains("<a class=\"answer\" href=\"/q/acts/"), "{view}");
    // a run that waits for the person has nothing at work to halt and is far from its ceiling: only abandoning is offered
    assert!(view.contains("data-act=\"abandon\""), "{view}");
    assert!(!view.contains("data-act=\"halt\"") && !view.contains("data-act=\"raise-ceiling\""), "{view}");
    // what each check said, and where its answer comes from
    assert!(view.contains("suite-passes (open, read runner) passed"), "{view}");
    // what the step changed, line by line, as git shows it
    assert!(view.contains("What change changed · 2 files") && view.contains("<span class=\"del\"><i>-</i>hello") && view.contains("<span class=\"file\">greeting.txt</span>"), "{view}");
    // an agent may halt, only the person resumes
    assert_eq!(post(port, "/api/act", r#"{"run":"acts","action":"halt"}"#, "")["ok"], true);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(post(port, "/api/act", r#"{"run":"acts","action":"resume"}"#, "")["ok"], false);
    let halted = http(port, &format!("GET /run/acts HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: {me}\r\n\r\n"));
    assert!(halted.contains("data-act=\"resume\""), "{halted}");
    assert_eq!(post(port, "/api/act", r#"{"run":"acts","action":"resume"}"#, me)["ok"], true);
    std::thread::sleep(std::time::Duration::from_millis(300));
    // a correction of the delivery, on the step that erred
    let r = post(port, "/api/act", r#"{"run":"acts","action":"correct","note":"the Makefile lost its tests","step":"change"}"#, me);
    assert_eq!(r["ok"], true, "{r}");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let version = http(port, &format!("GET /api/version HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"));
    assert!(version.contains("\"seq\""), "{version}");
    let t = types(&events(&s, "acts"));
    for want in ["Halted", "Resumed", "DeliveryCorrected"] {
        assert!(t.contains(&want.to_string()), "{want} missing: {t:?}");
    }
}

struct Mcp {
    child: std::process::Child,
    out: std::io::BufReader<std::process::ChildStdout>,
    n: u64,
}

impl Mcp {
    fn call(&mut self, method: &str, params: Value) -> Value {
        use std::io::{BufRead, Write};
        self.n += 1;
        let msg = serde_json::json!({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params});
        let stdin = self.child.stdin.as_mut().expect("stdin");
        writeln!(stdin, "{msg}").expect("write");
        stdin.flush().expect("flush");
        let mut line = String::new();
        self.out.read_line(&mut line).expect("read");
        serde_json::from_str(&line).expect("json")
    }
    fn permit(&mut self, tool: &str, input: Value) -> Value {
        let r = self.call("tools/call", serde_json::json!({"name": "permit", "arguments": {"tool_name": tool, "input": input}}));
        serde_json::from_str(r["result"]["content"][0]["text"].as_str().expect("text")).expect("decision")
    }
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_leave_the_agent_asks_mid_turn_is_granted_by_a_rule_or_by_the_person() {
    let s = scratch("leave");
    let mark = s.home.join("mark");
    std::fs::create_dir_all(&mark).expect("dir");
    // the agent works for a while: the leave server is asked while its turn runs
    let agent = format!("touch {}/started; sleep 8; perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}", mark.display());
    let role = root().join("crates/shell/tests/roles/small-change");
    let run = jaynes()
        .args(["start", "--run", "leave", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .spawn().expect("start");
    for _ in 0..100 {
        if mark.join("started").exists() { break; }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let attempt = events(&s, "leave").iter().filter(|e| e["type"] == "AttemptStarted" && e["step"] == "change").count();
    let cwd = s.home.join("runs/leave/project");
    let mut child = jaynes()
        .args(["permit", "--run", "leave", "--step", "change", "--attempt", &attempt.to_string(), "--cwd", cwd.to_str().expect("path")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null())
        .spawn().expect("permit");
    let out = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    let mut mcp = Mcp { child, out, n: 0 };
    let init = mcp.call("initialize", serde_json::json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}));
    assert_eq!(init["result"]["serverInfo"]["name"], "jaynes");
    assert_eq!(mcp.call("tools/list", serde_json::json!({}))["result"]["tools"][0]["name"], "permit");
    // reading inside the project is routine: a rule grants it at once
    let inside = mcp.permit("Read", serde_json::json!({"file_path": cwd.join("greeting.txt").display().to_string()}));
    assert_eq!(inside["behavior"], "allow", "{inside}");
    // reading outside is the person's: the server waits, the person refuses on the command line
    let asking = std::thread::spawn(move || {
        let d = mcp.permit("Read", serde_json::json!({"file_path": "/etc/hosts"}));
        (d, mcp)
    });
    let mut q = String::new();
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if let Some(e) = events(&s, "leave").into_iter().rfind(|e| e["type"] == "QuestionOpened")
            && e["question"]["kind"] == "permission" && e["question"]["about"].as_str().unwrap_or("").contains("/etc/hosts") {
                q = e["question"]["id"].as_str().expect("id").to_string();
                break;
            }
    }
    assert!(!q.is_empty(), "the leave was asked of the person");
    // while it waits for the person, the turn's clock stands still: the server marks the wait
    let waiting = s.home.join("runs/leave/leave-waiting");
    assert!(waiting.exists(), "the wait is marked");
    let answered = tty()
        .args([BIN, "answer", "leave", &q, "leave=drop", "--context", "not outside the project", "--send"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .output().expect("answer");
    assert!(String::from_utf8_lossy(&answered.stdout).contains("recorded"), "{}", String::from_utf8_lossy(&answered.stdout));
    let (decided, mut mcp) = asking.join().expect("asked");
    assert_eq!(decided["behavior"], "deny", "{decided}");
    assert!(decided["message"].as_str().unwrap_or("").contains("not outside the project"), "{decided}");
    assert!(!waiting.exists(), "the wait is over");
    // the same leave asked again gets the person's answer at once, without asking them again
    let again = mcp.permit("Read", serde_json::json!({"file_path": "/etc/hosts"}));
    assert_eq!(again["behavior"], "deny", "{again}");
    assert!(again["message"].as_str().unwrap_or("").contains("not outside the project"), "{again}");
    let _killed = mcp.child.kill();
    let _done = run.wait_with_output();
    let e = events(&s, "leave");
    let rules = e.iter().filter(|e| e["type"] == "AnswerRecorded" && e["answer"]["by"]["by"] == "rule").count();
    let people = e.iter().filter(|e| e["type"] == "AnswerRecorded" && e["answer"]["by"]["by"] == "person").count();
    assert_eq!((rules, people), (2, 1));
    let repeated = e.iter().filter(|e| e["type"] == "AnswerRecorded" && e["answer"]["by"]["rule"] == "answered-before").count();
    assert_eq!(repeated, 1, "the repeated leave names the rule that answered it");
    let finished = e.iter().find(|e| e["type"] == "RunFinished").unwrap_or_else(|| panic!("the run went on and finished: {:?}", types(&e)));
    assert_eq!(finished["cost"]["sent_back"], 0, "a leave refused is not work sent back");
}

fn fix_bug_run(s: &Scratch, run: &str) -> String {
    let prompts = s.home.join("prompts");
    std::fs::create_dir_all(&prompts).expect("dir");
    let agent = root().join("crates/shell/tests/agents/fix-bug.sh");
    let out = jaynes()
        .args(["start", "--run", run, "--role", root().join("crates/shell/tests/roles/fix-bug").to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "greet should say hi with the name", "--agent", &format!("command:sh {}", agent.display())])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "").env("PROMPTS", &prompts)
        .output().expect("start");
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_role_that_delivers_by_merge_merges_into_the_person_s_branch_and_undo_reverts_it() {
    let s = bug_project("merge");
    let said = fix_bug_run(&s, "merge");
    assert!(said.contains("merged as"), "{said}");
    // the person's checkout moved forward: their working tree has the fix, and nothing was pushed
    assert_eq!(std::fs::read_to_string(s.project.join("greet.sh")).expect("read"), "echo \"hi $1\"\n");
    let log = Command::new("git").arg("-C").arg(&s.project).args(["log", "--format=%s", "-1", "main"]).output().expect("git");
    assert!(String::from_utf8_lossy(&log.stdout).contains("Jaynes: run merge, merged"));
    // an agent may not undo; the person at a terminal does
    let (ok, out) = engine(&s, &["undo", "merge", "--note", "not now"]);
    assert!(!ok && out.contains("only you undo"), "{out}");
    let undo = tty()
        .args([BIN, "undo", "merge", "--note", "not now"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .output().expect("undo");
    assert!(String::from_utf8_lossy(&undo.stdout).contains("undone"), "{}", String::from_utf8_lossy(&undo.stdout));
    assert_eq!(std::fs::read_to_string(s.project.join("greet.sh")).expect("read"), "echo \"hello $1\"\n");
    assert!(types(&events(&s, "merge")).contains(&"MergeUndone".to_string()));
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_merge_waits_for_a_clean_checkout_and_says_why() {
    let s = bug_project("dirty");
    std::fs::write(s.project.join("greet.sh"), "echo \"hello there $1\"\n").expect("write");
    let said = fix_bug_run(&s, "dirty");
    assert!(said.contains("not merged: your checkout of main has uncommitted changes"), "{said}");
    assert_eq!(std::fs::read_to_string(s.project.join("greet.sh")).expect("read"), "echo \"hello there $1\"\n", "the person's work is untouched");
    let branch = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/dirty:greet.sh"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&branch.stdout), "echo \"hi $1\"\n", "the branch is delivered all the same");
}

#[test]
fn the_page_shows_each_item_with_its_sentence_and_picture() {
    let s = scratch("looks");
    let svg = r#"<svg xmlns='http://www.w3.org/2000/svg' width='40' height='40'><rect width='40' height='40' fill='#2b59c3'/></svg>"#;
    let agent = format!(
        "printf \"%s\" \"{svg}\" > .jaynes/blue.svg && printf '{{\"looks\":[{{\"id\":\"blue\",\"name\":\"Blue\",\"why\":\"ink on newsprint\",\"picture\":\".jaynes/blue.svg\"}}]}}' > .jaynes/output.json"
    );
    let role = root().join("crates/shell/tests/roles/looks");
    let (ok, out) = engine(&s, &["start", "--run", "looks", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                                 "--request", "make looks", "--agent", &format!("command:{agent}")]);
    assert!(ok && out.contains("Which looks go on?"), "{out}");
    let port = free_port();
    let _page = jaynes()
        .args(["page", "--port", &port.to_string()])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PAGE_KEY", "k3y")
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(Page)
        .expect("page");
    page_up(port);
    let html = http(port, &format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: jaynes_key=k3y\r\n\r\n"));
    assert!(html.contains("ink on newsprint"), "the item's sentence is shown");
    assert!(html.contains("<img src=\"data:image/svg+xml;base64,"), "the item's picture is shown");
    // the picture was kept by its fingerprint, apart from the run's copy
    let kept: Vec<_> = std::fs::read_dir(s.home.join("pictures")).expect("kept").flatten().collect();
    assert_eq!(kept.len(), 1);
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_role_is_installed_by_name_and_only_when_it_conforms() {
    let s = scratch("roles");
    let (ok, out) = engine(&s, &["add", root().join("crates/shell/tests/roles/small-change").to_str().expect("path")]);
    assert!(ok && out.contains("role small-change installed"), "{out}");
    let (ok, out) = engine(&s, &["roles"]);
    assert!(ok && out.contains("small-change 3.0.0"), "{out}");
    // a folder that is not a role is refused
    let bad = s.home.join("bad");
    std::fs::create_dir_all(&bad).expect("dir");
    std::fs::write(bad.join("role.py"), "print('not a role')\n").expect("write");
    let (ok, out) = engine(&s, &["add", bad.to_str().expect("path")]);
    assert!(!ok && out.contains("does not conform"), "{out}");
    // a run starts with the installed role by its name
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    let (ok, out) = engine(&s, &["start", "--run", "byname", "--role", "small-change", "--project", s.project.to_str().expect("path"),
                                 "--request", "say hi", "--agent", &format!("command:{agent}")]);
    assert!(ok && out.contains("finished: delivered"), "{out}");
    let (ok, out) = engine(&s, &["guide"]);
    assert!(ok && out.contains("jaynes start --role"), "{out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn agents_in_claude_code_use_the_mcp_server_and_may_only_release() {
    let s = scratch("mcp");
    assert!(engine(&s, &["add", root().join("crates/shell/tests/roles/small-change").to_str().expect("path")]).0);
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && printf 'test:\\n\\ttrue\\n' > Makefile && {OUTPUT}");
    // the agent program of the run is a command; the MCP start uses Claude by default, so start here and read through MCP
    start(&s, "viamcp", &agent);
    let child = jaynes()
        .arg("mcp")
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python())
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null())
        .spawn().expect("mcp");
    let mut child = child;
    let out = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    let mut mcp = Mcp { child, out, n: 0 };
    mcp.call("initialize", serde_json::json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}));
    let names: Vec<String> = mcp.call("tools/list", serde_json::json!({}))["result"]["tools"].as_array().expect("tools").iter().map(|t| t["name"].as_str().unwrap_or("").to_string()).collect();
    assert_eq!(names, ["home", "roles", "start", "run", "answer", "halt", "correct"]);
    let tool = |mcp: &mut Mcp, name: &str, args: Value| -> (bool, String) {
        let r = mcp.call("tools/call", serde_json::json!({"name": name, "arguments": args}));
        (r["result"]["isError"] == false, r["result"]["content"][0]["text"].as_str().unwrap_or("").to_string())
    };
    let (_, home) = tool(&mut mcp, "home", serde_json::json!({}));
    assert!(home.contains("run viamcp") && home.contains("bigger than a small one"), "{home}");
    let (_, roles) = tool(&mut mcp, "roles", serde_json::json!({}));
    assert!(roles.contains("small-change"), "{roles}");
    let q = events(&s, "viamcp").into_iter().find(|e| e["type"] == "QuestionOpened").expect("asked")["question"]["id"].as_str().expect("id").to_string();
    // small-change's stop is a release: an agent may let it go on
    let (ok, said) = tool(&mut mcp, "answer", serde_json::json!({"run": "viamcp", "question": q}));
    assert!(ok && said.contains("counts as the agent's"), "{said}");
    let (ok, said) = tool(&mut mcp, "run", serde_json::json!({"run": "viamcp"}));
    assert!(ok && said.contains("run viamcp"), "{said}");
    let (ok, said) = tool(&mut mcp, "halt", serde_json::json!({"run": "nope"}));
    assert!(!ok && said.contains("no run nope"), "{said}");
    let _killed = mcp.child.kill();
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_machine_that_cannot_run_the_tests_stops_the_run_before_any_agent_works() {
    if Command::new("python3").args(["-c", "import pytest"]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipped: this machine's python3 has pytest");
        return;
    }
    let s = scratch("nopytest");
    std::fs::write(s.project.join("pytest.ini"), "[pytest]\n").expect("write");
    std::fs::write(s.project.join("test_greet.py"), "def test_it():\n    assert True\n").expect("write");
    std::fs::remove_file(s.project.join("Makefile")).expect("rm");
    for args in [vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "pytest"]] {
        assert!(Command::new("git").arg("-C").arg(&s.project).args(&args).status().expect("git").success());
    }
    let out = start(&s, "nopytest", "touch agent-ran");
    assert!(out.contains("cannot run on this machine") && out.contains("install pytest"), "{out}");
    let t = types(&events(&s, "nopytest"));
    assert_eq!(t.iter().filter(|x| *x == "AttemptStarted").count(), 1, "only the role's own step started: {t:?}");
    assert!(!s.home.join("runs/nopytest/project/agent-ran").exists(), "no agent worked");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn abandoning_a_run_stops_its_agent_at_once() {
    let s = scratch("abandon");
    let mark = s.home.join("mark");
    std::fs::create_dir_all(&mark).expect("dir");
    let agent = format!("touch {m}/started; sleep 30; touch {m}/finished", m = mark.display());
    let role = root().join("crates/shell/tests/roles/small-change");
    let mut run = jaynes()
        .args(["start", "--run", "abandon", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .spawn().expect("start");
    for _ in 0..100 {
        if mark.join("started").exists() { break; }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let pgid = std::fs::read_to_string(s.home.join("runs/abandon/agent.pid")).expect("noted");
    let abandoned = tty()
        .args([BIN, "abandon", "abandon"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .output().expect("abandon");
    let said = String::from_utf8_lossy(&abandoned.stdout).to_string();
    let alive = Command::new("kill").args(["-0", "--", &format!("-{}", pgid.trim())]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    assert!(!alive, "the agent was stopped at once: {said}");
    let _done = run.wait();
    assert!(!mark.join("finished").exists());
    assert!(types(&events(&s, "abandon")).contains(&"RunFinished".to_string()));
}

/// A page process that dies with its test, even when the test panics, so it never holds the test's output open.
struct Page(std::process::Child);

impl Drop for Page {
    fn drop(&mut self) {
        let _killed = self.0.kill();
        let _reaped = self.0.wait();
    }
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_person_starts_a_run_from_the_page() {
    let s = scratch("new");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    let port = free_port();
    let _page = jaynes()
        .args(["page", "--port", &port.to_string()])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .env("JAYNES_PAGE_KEY", "k3y")
        .env("JAYNES_AGENT", format!("command:{agent}"))
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(Page)
        .expect("page");
    page_up(port);
    let me = "jaynes_key=k3y";
    let form = http(port, &format!("GET /new HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: {me}\r\n\r\n"));
    assert!(form.contains("<form id=\"start\"") && form.contains("name=\"request\""), "{form}");
    let role = root().join("crates/shell/tests/roles/small-change");
    let body = serde_json::json!({"role": role, "project": s.project, "request": "say hi instead of hello", "risk": "low"}).to_string();
    // only the person starts a run from the page
    assert_eq!(post(port, "/api/start", &body, "")["ok"], false);
    let r = post(port, "/api/start", &body, me);
    assert_eq!(r["ok"], true, "{r}");
    let run = r["run"].as_str().expect("run").to_string();
    let mut finished = false;
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if s.home.join("record.sqlite3").exists() && events(&s, &run).iter().any(|e| e["type"] == "RunFinished") {
            finished = true;
            break;
        }
    }
    assert!(finished, "{}", std::fs::read_to_string(s.home.join(format!("start-{run}.log"))).unwrap_or_default());
    let view = http(port, &format!("GET /run/{run} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: {me}\r\n\r\n"));
    assert!(view.contains("say hi instead of hello") && view.contains("Finished: delivered"), "{view}");
}

#[test]
fn the_page_sets_the_guard_and_says_when_jaynes_was_updated() {
    let s = scratch("setup");
    let settings = s.home.parent().expect("base").join("settings");
    let install = s.home.parent().expect("base").join("install");
    std::fs::create_dir_all(&install).expect("dir");
    let installed = |commit: &str| {
        let line = format!("[v1]\n\"jaynes-shell 0.1.0 (git+https://github.com/alk0x1/jaynes#{commit})\" = [\"jaynes\"]\n");
        std::fs::write(install.join(".crates.toml"), line).expect("write");
    };
    installed(&"a".repeat(40));
    let port = free_port();
    let _page = jaynes()
        .args(["page", "--port", &port.to_string()])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_HOME", &settings)
        .env("JAYNES_INSTALL_ROOT", &install)
        .env("JAYNES_NO_UPDATE_CHECK", "1")
        .env("JAYNES_PAGE_KEY", "k3y")
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(Page)
        .expect("page");
    page_up(port);
    let me = "jaynes_key=k3y";
    let get = |path: &str| http(port, &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: {me}\r\n\r\n"));
    let setup = get("/setup");
    assert!(setup.contains("<h2>Guard</h2>") && setup.contains("chip green\">on") && setup.contains("aaaaaaa"), "{setup}");
    // only the person changes the guard
    assert_eq!(post(port, "/api/setup", r#"{"action":"guard-off"}"#, "")["ok"], false);
    assert_eq!(post(port, "/api/setup", r#"{"action":"guard-off"}"#, me)["ok"], true);
    assert_eq!(post(port, "/api/setup", r#"{"action":"owner-add","owner":"someone"}"#, me)["ok"], true);
    let g: Value = serde_json::from_str(&std::fs::read_to_string(settings.join("guard.json")).expect("guard")).expect("json");
    assert_eq!(g["on"], false);
    assert_eq!(g["owners"], serde_json::json!(["someone"]));
    assert_eq!(post(port, "/api/setup", r#"{"action":"owner-remove","owner":"someone"}"#, me)["ok"], true);
    // a newer Jaynes installed on disk: the page asks to be restarted
    assert!(!get("/home").contains("Restart the page"));
    installed(&"b".repeat(40));
    assert!(get("/home").contains("Restart the page"));
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn an_agent_that_says_it_cannot_be_done_waits_for_the_person_and_goes_on_from_their_words() {
    let s = scratch("impossible");
    let prompts = s.home.parent().expect("base").join("prompts");
    std::fs::create_dir_all(&prompts).expect("dir");
    // first turn: it cannot be done as asked; second turn, in the same session: it does what the person said
    let agent = format!(
        "cat > {p}/turn-${{JAYNES_SESSION:-1}}; if [ -n \"$JAYNES_SESSION\" ]; then perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}; else echo 'the request asks for hi and for hello at once' > .jaynes/impossible.txt; echo '{{\"session\":\"s1\"}}'; fi",
        p = prompts.display()
    );
    let out = start(&s, "impossible", &agent);
    assert!(out.contains("cannot be done as asked"), "{out}");
    let first = std::fs::read_to_string(prompts.join("turn-1")).expect("prompt");
    assert!(first.contains(".jaynes/impossible.txt"), "the agent is told the way out: {first}");
    let e = events(&s, "impossible");
    assert!(e.iter().any(|e| e["type"] == "AttemptImpossible"), "{:?}", types(&e));
    let q = e.iter().rfind(|e| e["type"] == "InputNeeded").expect("asked")["question"]["id"].as_str().expect("id").to_string();
    let answered = tty()
        .args([BIN, "answer", "impossible", &q, "impossible=change:only hi", "--send"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .output().expect("answer");
    assert!(answered.status.success(), "{}", String::from_utf8_lossy(&answered.stdout));
    let mut finished = false;
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if events(&s, "impossible").iter().any(|e| e["type"] == "RunFinished") {
            finished = true;
            break;
        }
    }
    let e = events(&s, "impossible");
    assert!(finished, "{:?}", types(&e));
    let second = std::fs::read_to_string(prompts.join("turn-s1")).expect("the second turn resumed the session");
    assert!(second.contains("only hi"), "{second}");
    let spent: u64 = e.iter().filter(|e| e["type"] == "AttemptFailed" && e["spent"] == true).count() as u64;
    assert_eq!(spent, 0, "saying so spent no attempt");
}

/// The engine killed at many moments of a run that merges: each time the next process recovers, the run delivers,
/// the fix lands exactly once, nothing is left running and the record holds.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_run_killed_at_any_moment_recovers_and_lands_once() {
    let agent = root().join("crates/shell/tests/agents/fix-bug.sh");
    let role = root().join("crates/shell/tests/roles/fix-bug");
    let mut outcomes = Vec::new();
    for trial in 0..16u64 {
        let s = bug_project(&format!("kill{trial}"));
        let prompts = s.home.join("prompts");
        std::fs::create_dir_all(&prompts).expect("dir");
        let run = format!("kill{trial}");
        let envs = |c: &mut Command| {
            c.env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "").env("PROMPTS", &prompts);
        };
        let mut first = jaynes();
        first.args(["start", "--run", &run, "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                    "--request", "greet should say hi with the name", "--agent", &format!("command:sh {}", agent.display())]);
        envs(&mut first);
        let mut p = first.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().expect("start");
        // the first six in the start, where the run is being set up; the rest spread over the run's length
        let at = if trial < 6 { trial * 50 } else { 40 + (trial * 389) % 2600 };
        std::thread::sleep(std::time::Duration::from_millis(at));
        let _killed = p.kill();
        let _waited = p.wait();
        if events(&s, &run).is_empty() {
            // killed before the run was recorded: it never started, and starting it again works
            let mut again = jaynes();
            again.args(["start", "--run", &run, "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                        "--request", "greet should say hi with the name", "--agent", &format!("command:sh {}", agent.display())]);
            envs(&mut again);
            let out = again.output().expect("start again");
            assert!(out.status.success(), "trial {trial}: {}", String::from_utf8_lossy(&out.stderr));
        }
        let mut said = String::new();
        for _ in 0..6 {
            if events(&s, &run).iter().any(|e| e["type"] == "Delivered") {
                break;
            }
            let mut c = tty();
            c.args([BIN, "continue", &run]);
            envs(&mut c);
            let out = c.output().expect("continue");
            said.push_str(&String::from_utf8_lossy(&out.stdout));
        }
        let e = events(&s, &run);
        // the run delivers, and waits only for answers that can still correct it
        assert!(e.iter().any(|e| e["type"] == "Delivered"), "trial {trial}: {said}\n{:?}", types(&e));
        let (ok, out) = engine(&s, &["verify"]);
        assert!(ok, "trial {trial}: {out}");
        let log = Command::new("git").arg("-C").arg(&s.project).args(["log", "--format=%s", "main"]).output().expect("git");
        let merges = String::from_utf8_lossy(&log.stdout).lines().filter(|l| l.contains("merged")).count();
        assert_eq!(merges, 1, "trial {trial}: the fix landed {merges} times");
        assert_eq!(std::fs::read_to_string(s.project.join("greet.sh")).expect("read"), "echo \"hi $1\"\n", "trial {trial}");
        let pid = s.home.join(format!("runs/{run}/agent.pid"));
        assert!(!pid.exists(), "trial {trial}: an agent was left noted as running");
        outcomes.push(e.iter().any(|e| e["type"] == "AttemptInterrupted"));
    }
    assert!(outcomes.iter().any(|&x| x), "some kill fell inside a turn: {outcomes:?}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_finished_run_leaves_no_toolchain_cache_behind() {
    let s = scratch("caches");
    // what Go leaves: a build cache, and modules it makes read-only
    let fill = r#"mkdir -p "$GOCACHE/ab" "$GOPATH/pkg/mod/m@v1" "$npm_config_cache" && echo x > "$GOCACHE/ab/o" && echo x > "$GOPATH/pkg/mod/m@v1/m.go" && echo x > "$npm_config_cache/p" && chmod -R a-w "$GOPATH/pkg/mod""#;
    let agent = format!("{fill} && perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    let out = start(&s, "caches", &agent);
    assert!(out.contains("finished: delivered"), "{out}");
    let exchange = s.home.join("runs/caches/project/.jaynes");
    assert!(exchange.exists(), "the exchange stays");
    assert!(!exchange.join("tmp").exists(), "the toolchains' caches are gone once the run is finished");
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/caches:greeting.txt"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "hi\n");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_finished_run_s_copy_is_cleared_after_a_while_and_its_changes_still_show() {
    let s = scratch("sweep");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    start(&s, "old", &agent);
    assert!(s.home.join("runs/old/project/.git").exists());
    // the next start clears the copies of runs finished longer ago than the time kept (none kept here)
    let role = root().join("crates/shell/tests/roles/small-change");
    let out = jaynes()
        .args(["start", "--run", "new", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .env("JAYNES_KEEP_COPIES_SECONDS", "0")
        .output().expect("start");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!s.home.join("runs/old/project").exists(), "the finished run's copy is cleared");
    assert!(s.home.join("runs/new/project/.git").exists(), "the run just started keeps its copy");
    // the page still shows what the old run changed
    let port = free_port();
    let _page = jaynes()
        .args(["page", "--port", &port.to_string()])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PAGE_KEY", "k3y").env("JAYNES_NO_UPDATE_CHECK", "1")
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(Page)
        .expect("page");
    page_up(port);
    let view = http(port, &format!("GET /run/old HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: jaynes_key=k3y\r\n\r\n"));
    assert!(view.contains("What change changed") && view.contains("<span class=\"add\"><i>+</i>hi</span>"), "{view}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_role_s_own_check_is_a_script_run_in_the_box_judging_by_its_exit() {
    let s = scratch("script");
    let role = s.home.parent().expect("base").join("odd");
    std::fs::create_dir_all(role.join("checks")).expect("dir");
    std::fs::write(role.join("role.toml"), r#"
role = "odd"
version = "1.0.0"

[[step]]
name = "say"
reach = { project = "read" }
instructions = "Say something about {{request}}."
checks = [{ script = "checks/odd.sh", about = "the output says nothing bad" }]
"#).expect("write");
    // reads the call on standard input; tries to write outside the box, which must fail
    std::fs::write(role.join("checks/odd.sh"), r#"
input=$(cat)
if touch "$HOME/.jaynes-escaped-the-box" 2>/dev/null; then echo "wrote outside the box"; exit 1; fi
[ -f greeting.txt ] || { echo "not run on the project"; exit 1; }
case "$input" in
  *'"bad"'*) echo "it says bad"; echo "and that is all"; exit 1 ;;
  *'"unsure"'*) echo "cannot tell"; exit 3 ;;
esac
exit 0
"#).expect("write");
    let (ok, out) = engine(&s, &["check", role.to_str().expect("path")]);
    assert!(ok, "{out}");
    let verdict = |said: &str| -> Value {
        let call = serde_json::json!({"jaynes": 1, "id": "x", "call": "check", "step": "say", "check": "odd", "request": "r",
            "output": {"word": said}, "given": [], "project": s.project, "source": s.project});
        let mut child = jaynes()
            .args(["role-serve", role.to_str().expect("path")])
            .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
            .spawn().expect("serve");
        use std::io::Write;
        child.stdin.take().expect("stdin").write_all(format!("{call}\n").as_bytes()).expect("write");
        let out = child.wait_with_output().expect("out");
        serde_json::from_slice::<Value>(&out.stdout).expect("reply")["verdict"].clone()
    };
    assert_eq!(verdict("good")["verdict"], "pass", "{}", verdict("good"));
    let bad = verdict("bad");
    assert_eq!(bad["verdict"], "fail");
    assert_eq!(bad["reasons"], serde_json::json!(["it says bad", "and that is all"]));
    let unsure = verdict("unsure");
    assert_eq!((unsure["verdict"].as_str(), unsure["reason"].as_str()), (Some("blocked"), Some("cannot tell")));
    assert!(!Path::new(&std::env::var("HOME").expect("home")).join(".jaynes-escaped-the-box").exists());
}

#[test]
fn a_role_whose_promise_does_not_hold_is_refused_with_the_path_that_breaks_it() {
    let s = scratch("promise");
    let role = s.home.parent().expect("base").join("asks-late");
    std::fs::create_dir_all(&role).expect("dir");
    let text = std::fs::read_to_string(root().join("crates/shell/tests/roles/small-change/role.toml")).expect("read")
        .replace("role = \"small-change\"", "role = \"asks-late\"")
        + "\n[[promise]]\nsays = \"Nothing is delivered unless you answered at change\"\nnever = \"deliver\"\nunless = \"person answered at change\"\n";
    std::fs::write(role.join("role.toml"), text).expect("write");
    let (ok, out) = engine(&s, &["check", role.to_str().expect("path")]);
    assert!(!ok, "{out}");
    assert!(out.contains("✔ \"Nothing is delivered before the project's suite passed on the change\": proved"), "{out}");
    assert!(out.contains("✘ \"Nothing is delivered unless you answered at change\" (role.toml:"), "{out}");
    assert!(out.contains("low risk → runner's reach is tested → runner done → change's reach is tested → change delivers a small change → change's checks pass → change's stop asks only from high risk → deliver, with no answer of yours at change"), "{out}");
    let (ok, out) = engine(&s, &["add", role.to_str().expect("path")]);
    assert!(!ok && out.contains("a promise of the role does not hold, so it was not installed"), "{out}");
    let (_, roles) = engine(&s, &["roles"]);
    assert!(!roles.contains("asks-late"), "{roles}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_step_done_in_the_record_keeps_its_change_when_the_process_dies_before_committing_it() {
    let s = scratch("diedone");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    let role = root().join("crates/shell/tests/roles/small-change");
    let out = jaynes()
        .args(["start", "--run", "diedone", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi instead of hello", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .env("JAYNES_DIE_AFTER_DONE", "change")
        .output().expect("start");
    assert_eq!(out.status.code(), Some(9), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(types(&events(&s, "diedone")).contains(&"StepDone".to_string()));
    let (ok, said) = engine(&s, &["continue", "diedone"]);
    assert!(ok && said.contains("finished: delivered"), "{said}");
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/diedone:greeting.txt"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "hi\n", "the delivery lost the change of a step the record says is done");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_merge_that_landed_before_the_process_died_lands_once_and_can_be_undone() {
    let s = bug_project("diemerge");
    let prompts = s.home.join("prompts");
    std::fs::create_dir_all(&prompts).expect("dir");
    let agent = root().join("crates/shell/tests/agents/fix-bug.sh");
    let out = jaynes()
        .args(["start", "--run", "diemerge", "--role", root().join("crates/shell/tests/roles/fix-bug").to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "greet should say hi with the name", "--agent", &format!("command:sh {}", agent.display())])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "").env("PROMPTS", &prompts)
        .env("JAYNES_DIE_AFTER_MERGE", "1")
        .output().expect("start");
    assert_eq!(out.status.code(), Some(9), "{}", String::from_utf8_lossy(&out.stderr));
    // the delivery made again a second later is made of new commits, with the same change
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let mut c = jaynes();
    c.args(["continue", "diemerge"]).env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "").env("PROMPTS", &prompts);
    let said = String::from_utf8_lossy(&c.output().expect("continue").stdout).to_string();
    assert!(said.contains("delivered"), "{said}");
    let log = Command::new("git").arg("-C").arg(&s.project).args(["log", "--format=%s", "main"]).output().expect("git");
    let merges = String::from_utf8_lossy(&log.stdout).lines().filter(|l| l.contains("merged")).count();
    assert_eq!(merges, 1, "the fix landed {merges} times: {said}");
    // the merge made before the process died is the run's, so Undo reverts it
    let undo = tty()
        .args([BIN, "undo", "diemerge", "--note", "not now"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .output().expect("undo");
    assert!(String::from_utf8_lossy(&undo.stdout).contains("undone"), "{}", String::from_utf8_lossy(&undo.stdout));
    assert_eq!(std::fs::read_to_string(s.project.join("greet.sh")).expect("read"), "echo \"hello $1\"\n");
}

/// An index in a git repository, with small-change in it, its entry written by `jaynes index check --write`.
fn an_index(s: &Scratch) -> PathBuf {
    let index = s.home.parent().expect("base").join("index");
    std::fs::create_dir_all(index.join("roles")).expect("dir");
    let from = root().join("crates/shell/tests/roles/small-change");
    let ok = Command::new("cp").arg("-R").arg(&from).arg(index.join("roles/small-change")).status().expect("cp").success();
    assert!(ok);
    std::fs::write(index.join("index.toml"), "[[role]]\nname = \"small-change\"\nversion = \"3.0.0\"\nsource = \"roles/small-change\"\nfingerprint = \"\"\n").expect("write");
    let (ok, out) = engine(s, &["index", "check", index.to_str().expect("path"), "--write"]);
    assert!(ok, "{out}");
    let (ok, out) = engine(s, &["index", "check", index.to_str().expect("path")]);
    assert!(ok && out.contains("✔ small-change 3.0.0") && out.contains("proved"), "{out}");
    for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "index"]] {
        assert!(Command::new("git").arg("-C").arg(&index).args(&args).status().expect("git").success());
    }
    index
}

fn engine_with_index(s: &Scratch, index: &str, args: &[&str]) -> (bool, String) {
    let out = jaynes()
        .args(args)
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .env("JAYNES_INDEX", index)
        .output()
        .expect("engine");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn a_role_is_installed_by_name_from_the_index_once_its_entry_holds() {
    let s = scratch("index");
    let index = an_index(&s);
    let url = format!("file://{}", index.display());
    let (ok, out) = engine_with_index(&s, &url, &["add", "small-change"]);
    assert!(ok && out.contains("role small-change installed"), "{out}");
    let origin = std::fs::read_to_string(s.home.join("roles/small-change/.jaynes-origin")).expect("origin");
    assert!(origin.contains("\"index:small-change\""), "{origin}");
    let (ok, out) = engine_with_index(&s, &url, &["add", "no-such-role"]);
    assert!(!ok && out.contains("the index has no role no-such-role"), "{out}");
}

#[test]
fn an_index_entry_that_does_not_hold_installs_nothing() {
    let s = scratch("badindex");
    let index = an_index(&s);
    let entry = index.join("index.toml");
    let listed = std::fs::read_to_string(&entry).expect("read");
    // a promise the index lists and the role does not make
    std::fs::write(&entry, listed.replace("promises = [", "promises = [\"Nothing is ever delivered\", ")).expect("write");
    let (ok, out) = engine_with_index(&s, index.to_str().expect("path"), &["add", "small-change"]);
    assert!(!ok && out.contains("does not hold, so it was not installed") && out.contains("Nothing is ever delivered"), "{out}");
    // the role's content changed after it was listed
    std::fs::write(&entry, &listed).expect("write");
    let role = index.join("roles/small-change/role.toml");
    std::fs::write(&role, std::fs::read_to_string(&role).expect("read") + "\n# changed\n").expect("write");
    let (ok, out) = engine_with_index(&s, index.to_str().expect("path"), &["add", "small-change"]);
    assert!(!ok && out.contains("content is not what the index lists"), "{out}");
    let (ok, out) = engine(&s, &["index", "check", index.to_str().expect("path")]);
    assert!(!ok && out.contains("✘ small-change"), "{out}");
    assert!(!s.home.join("roles/small-change").exists());
}

/// The agent of the directions test role: two pages and an output naming them.
const DRAW: &str = "mkdir -p design && printf '<html><body style=\"background:#1d4d3a\"><h1>Ink</h1></body></html>' > design/ink.html \
    && printf '<html><body style=\"background:#f2c14e\"><h1>Sun</h1></body></html>' > design/sun.html \
    && printf '{\"directions\":[{\"name\":\"Ink\",\"why\":\"dark green\",\"page\":\"design/ink.html\"},{\"name\":\"Sun\",\"why\":\"warm yellow\",\"page\":\"design/sun.html\"}]}' > .jaynes/output.json";

fn copy_of(s: &Scratch, run: &str) -> PathBuf {
    let runs = s.home.join("runs").join(run);
    let found = walk(&runs).into_iter().find(|p| p.ends_with(".git") && p.is_dir()).expect("the run's copy");
    found.parent().expect("copy").to_path_buf()
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        out.push(p.clone());
        if p.is_dir() && !p.ends_with(".git") && !p.is_symlink() {
            out.extend(walk(&p));
        }
    }
    out
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands and a Chrome in it, which Linux does not have yet (slice 2)")]
fn a_page_shown_as_a_picture_is_photographed_by_jaynes() {
    if skipped_without_a_test_browser() {
        return;
    }
    let s = scratch("photo");
    let role = root().join("crates/shell/tests/roles/directions");
    let (ok, out) = engine(&s, &["start", "--run", "photo", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                                 "--request", "two looks", "--agent", &format!("command:{DRAW}")]);
    assert!(ok && out.contains("Which direction goes on?") && out.contains("- Ink: Ink"), "{out}");
    // each page was photographed, kept by its fingerprint as a picture, and left in the copy for later agents
    let kept: Vec<String> = std::fs::read_dir(s.home.join("pictures")).expect("kept").flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    assert_eq!(kept.iter().filter(|k| k.ends_with(".png")).count(), 2, "{kept:?}");
    // the page itself is kept beside its photograph, so the person can open it
    let pages: Vec<&String> = kept.iter().filter(|k| k.ends_with(".html")).collect();
    assert_eq!(pages.len(), 2, "{kept:?}");
    let copy = copy_of(&s, "photo");
    let photo = std::fs::read(copy.join(".jaynes/pictures/design/ink.png")).expect("the photograph is in the copy");
    assert_eq!(&photo[1..4], b"PNG");
    // on the page the photograph opens the page, served apart: sandboxed, so what an agent wrote cannot answer
    let port = free_port();
    let _page = jaynes()
        .args(["page", "--port", &port.to_string()])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PAGE_KEY", "k3y")
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(Page)
        .expect("page");
    page_up(port);
    let html = http(port, &format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: jaynes_key=k3y\r\n\r\n"));
    let link = html.split("<a class=\"open\" href=\"/page/").nth(1).and_then(|r| r.split('"').next()).expect("a link to the page").to_string();
    assert!(pages.contains(&&link), "{link} {pages:?}");
    let served = http(port, &format!("GET /page/{link} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"));
    assert!(served.starts_with("HTTP/1.1 200") && served.contains("<h1>"), "{served}");
    assert!(served.contains("Content-Security-Policy: sandbox allow-scripts;"), "{served}");
    // only a kept page by its fingerprint is served, never a path
    assert!(http(port, &format!("GET /page/..%2Fpage.key HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")).starts_with("HTTP/1.1 404"));
}

/// Answer as the person, at a terminal.
fn as_person(s: &Scratch, args: &[&str]) -> String {
    let mut all = vec![BIN];
    all.extend_from_slice(args);
    let out = tty()
        .args(&all)
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .output()
        .expect("script");
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn question_of(s: &Scratch, run: &str, n: usize) -> Value {
    events(s, run).into_iter().filter(|e| e["type"] == "QuestionOpened").nth(n).expect("asked")["question"].clone()
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands and a Chrome in it, which Linux does not have yet (slice 2)")]
fn choosing_a_direction_keeps_it_alone_without_the_agent_and_the_run_goes_on() {
    if skipped_without_a_test_browser() {
        return;
    }
    let s = scratch("choose");
    let calls = s.home.parent().expect("base").join("calls.txt");
    let agent = format!("echo x >> {} && {DRAW}", calls.display());
    let role = root().join("crates/shell/tests/roles/directions");
    let (ok, out) = engine(&s, &["start", "--run", "choose", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                                 "--request", "two looks", "--agent", &format!("command:{agent}")]);
    assert!(ok && out.contains("Which direction goes on?"), "{out}");
    let q = question_of(&s, "choose", 0);
    assert_eq!(q["choose"], true, "{q}");
    let said = as_person(&s, &["answer", "choose", q["id"].as_str().expect("id"), "--choose", "Sun", "--send"]);
    assert!(said.contains("“Sun” is chosen") && said.contains("keeps only what you chose"), "{said}");
    assert!(said.contains("finished: delivered"), "{said}");
    // the agent worked twice: directions and apply; keeping Sun alone was Jaynes's
    assert_eq!(std::fs::read_to_string(&calls).expect("calls").lines().count(), 2);
    assert_eq!(events(&s, "choose").iter().filter(|e| e["type"] == "QuestionOpened").count(), 1, "not asked again");
    // apply read Sun alone
    let given = std::fs::read_to_string(copy_of(&s, "choose").join(".jaynes/inputs/directions.json")).expect("given");
    assert!(given.contains("Sun") && !given.contains("Ink"), "{given}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands and a Chrome in it, which Linux does not have yet (slice 2)")]
fn choosing_with_a_note_has_the_agent_combine_and_asks_about_the_result_alone() {
    if skipped_without_a_test_browser() {
        return;
    }
    let s = scratch("adjust");
    // the second time it draws, the agent gives Sun alone with Ink's colour
    let agent = format!(
        "if [ -f .jaynes/previous.json ] && grep -q Ink .jaynes/previous.json && ! [ -f .jaynes/inputs/directions.json ]; then \
         printf '{{\"directions\":[{{\"name\":\"Sun\",\"why\":\"warm with dark green\",\"page\":\"design/sun.html\"}}]}}' > .jaynes/output.json; \
         printf '<html><body style=\"background:#1d4d3a\"><h1>Sun</h1></body></html>' > design/sun.html; else {DRAW}; fi"
    );
    let role = root().join("crates/shell/tests/roles/directions");
    let (ok, out) = engine(&s, &["start", "--run", "adjust", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                                 "--request", "two looks", "--agent", &format!("command:{agent}")]);
    assert!(ok, "{out}");
    let q = question_of(&s, "adjust", 0);
    let said = as_person(&s, &["answer", "adjust", q["id"].as_str().expect("id"), "--choose", "Sun", "--adjust", "with the green of Ink", "--send"]);
    assert!(said.contains("adjusted as you wrote"), "{said}");
    let q2 = question_of(&s, "adjust", 1);
    let ids: Vec<&str> = q2["items"].as_array().expect("items").iter().filter_map(|x| x["id"].as_str()).collect();
    assert_eq!(ids, ["Sun"], "{q2}");
    let said = as_person(&s, &["answer", "adjust", q2["id"].as_str().expect("id"), "--choose", "Sun", "--send"]);
    assert!(said.contains("finished: delivered"), "{said}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands and a Chrome in it, which Linux does not have yet (slice 2)")]
fn an_answer_from_the_page_that_sends_a_step_back_starts_it_once() {
    if skipped_without_a_test_browser() {
        return;
    }
    let s = scratch("again");
    // sent back, the agent gives Ink alone, changed
    let agent = format!(
        "if [ -f .jaynes/previous.json ]; then \
         printf '{{\"directions\":[{{\"name\":\"Ink\",\"why\":\"greener\",\"page\":\"design/ink.html\"}}]}}' > .jaynes/output.json; \
         printf '<html><body style=\"background:#0d6d3a\"><h1>Ink</h1></body></html>' > design/ink.html; else {DRAW}; fi"
    );
    let role = root().join("crates/shell/tests/roles/directions");
    let (ok, out) = engine(&s, &["start", "--run", "again", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                                 "--request", "two looks", "--agent", &format!("command:{agent}")]);
    assert!(ok && out.contains("Which direction goes on?"), "{out}");
    let q = question_of(&s, "again", 0);
    let shown = jaynes_core::question::Question::shown(&serde_json::from_value(q.clone()).expect("question")).0;
    let port = free_port();
    let _page = jaynes()
        .args(["page", "--port", &port.to_string()])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .env("JAYNES_PAGE_KEY", "k3y")
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(Page)
        .expect("page");
    page_up(port);
    // the person keeps Ink with a change: the step that drew it works again
    let id = q["id"].as_str().expect("id");
    let body = format!(r#"{{"run":"again","question":"{id}","shown":"{shown}","verdicts":{{"Ink":"change","Sun":"drop"}},"notes":{{"Ink":"greener"}}}}"#);
    let r = post(port, "/api/answer", &body, "jaynes_key=k3y");
    assert_eq!(r["ok"], true, "{r}");
    let mut asked = false;
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if events(&s, "again").iter().filter(|e| e["type"] == "QuestionOpened").count() == 2 {
            asked = true;
            break;
        }
    }
    let t = types(&events(&s, "again"));
    assert!(asked, "{t:?}");
    // the page leaves the answer to the run's process, so no attempt is begun where no agent runs it
    assert!(!t.contains(&"AttemptInterrupted".to_string()), "{t:?}");
    let started = events(&s, "again").iter().filter(|e| e["type"] == "AttemptStarted" && e["step"] == "directions").count();
    assert_eq!(started, 2, "{t:?}");
}

/// Run a role's script check through `role-serve`, as the engine does, with `call` and extra environment.
fn serve_check(s: &Scratch, role: &Path, call: Value, env: &[(&str, &str)]) -> Value {
    let mut c = jaynes();
    c.args(["role-serve", role.to_str().expect("path")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());
    for (k, v) in env {
        c.env(k, v);
    }
    let mut child = c.spawn().expect("serve");
    use std::io::Write;
    child.stdin.take().expect("stdin").write_all(format!("{call}\n").as_bytes()).expect("write");
    let out = child.wait_with_output().expect("out");
    serde_json::from_slice::<Value>(&out.stdout).expect("reply")["verdict"].clone()
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which is macOS's sandbox-exec")]
fn a_project_s_command_reads_nothing_the_engine_keeps_nor_another_run_and_sends_no_udp_nor_opens_a_terminal() {
    let s = scratch("wall");
    std::fs::create_dir_all(&s.home).expect("home");
    // what the engine keeps that no project command may read: the page's key and log, the record, other runs, and
    // the run's own folder but for its copy of the project (its steps, a hidden step's cases among them)
    for f in ["page.key", "page.log", "app-key.pub", "record.sqlite3", "record.sqlite3-wal", "record.sqlite3-shm"] {
        std::fs::write(s.home.join(f), "secret\n").expect("write");
    }
    let own = s.home.join("runs/r-own");
    for d in ["runs/r-other", "runs/r-own/steps/cases", "runs/r-own/role"] {
        std::fs::create_dir_all(s.home.join(d)).expect("dir");
    }
    std::fs::write(s.home.join("runs/r-other/output.json"), "secret\n").expect("write");
    std::fs::write(own.join("steps/cases/output.json"), "secret\n").expect("write");
    std::fs::write(own.join("role/tool.txt"), "the run's own role\n").expect("write");
    let copy = own.join("project");
    std::fs::create_dir_all(&copy).expect("dir");
    std::fs::write(copy.join("greeting.txt"), "hello\n").expect("write");

    let role = s.home.parent().expect("base").join("wall");
    std::fs::create_dir_all(role.join("checks")).expect("dir");
    std::fs::write(role.join("role.toml"), r#"
role = "wall"
version = "1.0.0"

[[step]]
name = "look"
reach = { project = "read" }
instructions = "Look at {{request}}."
checks = [{ script = "checks/wall.py", about = "the box holds" }]
"#).expect("write");
    // Exit 0 when every wall holds, 1 naming each one that did not.
    std::fs::write(role.join("checks/wall.py"), r#"#!/usr/bin/env python3
import json, os, socket, sys
said = json.load(sys.stdin)["output"]
home, own = said["home"], said["own"]
holes = []
for f in ["page.key", "page.log", "app-key.pub", "record.sqlite3", "record.sqlite3-wal", "record.sqlite3-shm",
          "runs/r-other/output.json", "runs/r-own/steps/cases/output.json", "runs/r-own/role/tool.txt"]:
    try:
        open(os.path.join(home, f)).read()
        holes.append("read " + f)
    except OSError:
        pass
try:
    open(os.path.join(own, "project/greeting.txt")).read()
except OSError as e:
    holes.append(f"could not read the run's own copy of the project: {e}")
# a suite's own servers on port 0 still answer it
for host, family in [("127.0.0.1", socket.AF_INET), ("::1", socket.AF_INET6)]:
    mine = socket.socket(family)
    mine.bind((host, 0))
    mine.listen()
    try:
        socket.create_connection((host, mine.getsockname()[1]), timeout=3).close()
    except OSError as e:
        holes.append(f"its own server on {host} port 0 was not reached: {e}")
# no UDP, not even to its own machine
u = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
try:
    u.sendto(b"", ("127.0.0.1", 9))
    holes.append("sent UDP to 127.0.0.1")
except PermissionError:
    pass
# a terminal that is not the box's: neither read nor written
for mode in (os.O_RDONLY, os.O_WRONLY):
    try:
        os.close(os.open(said["tty"], mode | os.O_NOCTTY))
        holes.append(f"opened the terminal {said['tty']}")
    except PermissionError:
        pass
if not os.path.isfile("greeting.txt"):
    holes.append("not run on the project")
print("\n".join(holes))
sys.exit(1 if holes else 0)
"#).expect("write");
    let script = role.join("checks/wall.py");
    let mut mode = std::fs::metadata(&script).expect("meta").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    std::fs::set_permissions(&script, mode).expect("chmod");
    let (ok, out) = engine(&s, &["check", role.to_str().expect("path")]);
    assert!(ok, "{out}");

    // a terminal of the person's: a pty held open outside the box for the length of the test
    let mut holder = Command::new(python())
        .args(["-c", "import os, pty, sys, time\nm, s = pty.openpty()\nprint(os.ttyname(s), flush=True)\ntime.sleep(60)"])
        .stdout(std::process::Stdio::piped()).spawn().expect("pty");
    let mut tty = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(holder.stdout.take().expect("out")), &mut tty).expect("tty");
    let call = serde_json::json!({"jaynes": 1, "id": "x", "call": "check", "step": "look", "check": "wall", "request": "r",
        "output": {"home": s.home, "own": own, "tty": tty.trim()}, "given": [], "project": copy, "source": s.project});
    let held = serve_check(&s, &role, call, &[]);
    let _gone = holder.kill();
    let _reaped = holder.wait();
    assert_eq!(held["verdict"], "pass", "{held}");
    // the planted files are not a record: CI compares the Lean model with every jaynes-e2e-*/home/record.sqlite3
    // the tests leave, so this test leaves none
    let _gone = std::fs::remove_dir_all(s.home.parent().expect("base"));
}

/// Inside another program's sandbox (Claude Code's Bash with its sandbox on: measured) macOS opens no box: the
/// project's commands are refused, saying how to start the run outside it, never run under that sandbox.
#[test]
fn inside_another_program_s_sandbox_the_project_s_commands_are_refused() {
    let script = r#"
import jaynes_box as b
from pathlib import Path
b.inside = lambda: True
b.available = lambda: True
try:
    b.command_in_box("true", b.Box((Path("/tmp"),)))
    print("ran under the other sandbox")
except b.Unavailable as e:
    print("refused:", e)
"#;
    let out = Command::new(python()).args(["-c", script]).env("PYTHONPATH", root().join("python")).env("JAYNES_UNBOXED", "1").output().expect("python");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!said.contains("ran under the other sandbox"), "{said}");
    assert!(said.contains("refused: Jaynes was started inside another program's sandbox and cannot put its own box around your project's commands"), "{said}");
    assert!(said.contains("mcp__plugin_jaynes_jaynes__start") && said.contains("in an ordinary terminal"), "both ways out, each named: {said}");
}

/// A role that is a program runs with the person's rights, outside any box: jaynes add says so, and says nothing of
/// the kind for a role in role.toml, whose commands stay in the box.
#[test]
fn adding_a_role_that_is_a_program_says_it_runs_with_your_rights() {
    let s = scratch("add-program");
    let add = |role: &str| engine(&s, &["add", root().join("crates/shell/tests/roles").join(role).to_str().expect("path")]);
    let (ok, out) = add("looks");
    assert!(ok, "{out}");
    assert!(out.contains("looks is a program, not a role.toml") && out.contains("runs with your rights, outside any box"), "{out}");
    let (ok, out) = add("small-change");
    assert!(ok, "{out}");
    assert!(!out.contains("runs with your rights"), "a role in role.toml is not told so: {out}");
}

/// An agent that writes in the copy's .git what git runs: every hook git knows, and the configuration keys that run a
/// program. Each one that runs leaves a mark.
fn planting_agent(marks: &Path) -> String {
    let hooks = [
        "applypatch-msg", "pre-applypatch", "post-applypatch", "pre-commit", "pre-merge-commit", "prepare-commit-msg", "commit-msg",
        "post-commit", "pre-rebase", "post-checkout", "post-merge", "pre-push", "pre-receive", "update", "proc-receive", "post-receive",
        "post-update", "reference-transaction", "push-to-checkout", "pre-auto-gc", "post-rewrite", "sendemail-validate",
        "fsmonitor-watchman", "post-index-change",
    ];
    let m = marks.display();
    let mut s = String::new();
    s.push_str(&format!("mkdir -p .git/hooks .git/info {m} && "));
    for h in hooks {
        s.push_str(&format!("printf '#!/bin/sh\\ntouch {m}/hook-{h}\\n' > .git/hooks/{h} && chmod +x .git/hooks/{h} && "));
    }
    let run = |name: &str| format!("{m}/run-{name}.sh");
    for key in ["fsmonitor", "clean", "smudge", "diff", "pager", "ssh", "askpass"] {
        s.push_str(&format!("printf '#!/bin/sh\\ntouch {m}/config-{key}\\ncat\\n' > {} && chmod +x {} && ", run(key), run(key)));
    }
    s.push_str(&format!(
        "git config core.fsmonitor {} && git config filter.evil.clean {} && git config filter.evil.smudge {} && git config diff.external {} && \
         git config core.pager {} && git config core.sshCommand {} && git config core.askPass {} && git config core.hooksPath .git/hooks && ",
        run("fsmonitor"), run("clean"), run("smudge"), run("diff"), run("pager"), run("ssh"), run("askpass")
    ));
    s.push_str("printf '* filter=evil diff=evil\\n' > .git/info/attributes && printf 'greeting.txt filter=evil\\n' > .gitattributes && ");
    s.push_str(&format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}"));
    s
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn nothing_an_agent_writes_in_the_copy_s_git_runs_outside_its_box() {
    let s = scratch("git-hooks");
    let marks = s.home.parent().expect("base").join("marks");
    let out = start(&s, "hooks", &planting_agent(&marks));
    // the attributes file in the tree is on the floor, so the run is high risk and delivers with its stop open
    assert!(out.contains("delivered"), "{out}");
    let ran: Vec<String> = std::fs::read_dir(&marks).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with("run-")).collect()).unwrap_or_default();
    assert!(ran.is_empty(), "the engine's git ran what the agent wrote in the copy's .git: {ran:?}");
    // the change is measured as it is: the greeting and the attributes file the agent wrote in the tree
    let e = events(&s, "hooks");
    let done = e.iter().find(|e| e["type"] == "StepDone" && e["step"] == "change").expect("done");
    let mut paths: Vec<String> = done["changed"].as_array().expect("changed").iter().map(|c| c["what"].as_str().unwrap_or("").to_string()).collect();
    paths.sort();
    assert_eq!(paths, [".gitattributes", "greeting.txt"]);
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/hooks:greeting.txt"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "hi\n");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn what_an_agent_writes_in_the_copy_s_git_is_not_what_is_delivered() {
    let s = scratch("git-refs");
    // the agent commits a file of its own in the copy's .git and moves every branch there to it
    let agent = format!(
        "perl -pi -e 's/hello/hi/' greeting.txt && echo planted > planted.txt && git add planted.txt && \
         git -c user.name=a -c user.email=a@a commit -q -m planted && for r in $(git for-each-ref --format='%(refname)' refs/heads); do git update-ref $r HEAD; done && \
         git rm -q --cached planted.txt && rm planted.txt && {OUTPUT}"
    );
    let out = start(&s, "refs", &agent);
    assert!(out.contains("finished: delivered"), "{out}");
    let files = Command::new("git").arg("-C").arg(&s.project).args(["ls-tree", "-r", "--name-only", "jaynes/refs"]).output().expect("git");
    let files = String::from_utf8_lossy(&files.stdout);
    assert!(!files.contains("planted.txt"), "the delivery carries what the agent committed in the copy's .git:\n{files}");
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/refs:greeting.txt"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "hi\n");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn an_output_that_is_a_link_out_of_the_copy_is_not_read() {
    let s = scratch("output-link");
    // what the agent points at: a file of this account outside the copy, which Jaynes must never take in
    let secret = s.home.parent().expect("base").join("secret.json");
    std::fs::write(&secret, r#"{"summary":"a secret of the account","files":["greeting.txt"]}"#).expect("write");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && ln -sf {} .jaynes/output.json", secret.display());
    let (_, out) = engine(&s, &[
        "start", "--run", "link", "--role", root().join("crates/shell/tests/roles/small-change").to_str().expect("path"),
        "--project", s.project.to_str().expect("path"), "--request", "say hi", "--agent", &format!("command:{agent}"),
    ]);
    assert!(out.contains("is not a plain file"), "{out}");
    let (_, log) = engine(&s, &["log", "link"]);
    assert!(!log.contains("a secret of the account"), "the record holds what the link pointed at");
}

/// A marker program: it writes its name in `marks` and passes its input through, as a filter would.
fn marker(marks: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(marks).expect("marks");
    let p = marks.join(format!("run-{name}.sh"));
    std::fs::write(&p, format!("#!/bin/sh\ntouch {}/ran-{name}\ncat\n", marks.display())).expect("write");
    std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("chmod");
    p
}

fn ran(marks: &Path) -> Vec<String> {
    std::fs::read_dir(marks).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("ran-")).collect()).unwrap_or_default()
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_repository_an_agent_nests_in_the_copy_is_refused_and_nothing_of_it_runs() {
    let s = scratch("git-nested");
    let marks = s.home.parent().expect("base").join("marks");
    let (fsmon, clean) = (marker(&marks, "fsmonitor"), marker(&marks, "clean"));
    let hook = marker(&marks, "post-index-change");
    let agent = format!(
        // the agent makes the nested repository first and sets up what runs after, so nothing it runs itself leaves a mark
        "perl -pi -e 's/hello/hi/' greeting.txt && mkdir -p sub && cd sub && git init -q && echo x > f.txt && printf 'f.txt filter=mine\\n' > .gitattributes && \
         git add -A && git -c user.name=a -c user.email=a@a commit -q -m nested && git config core.fsmonitor {fsmon} && \
         git config filter.mine.clean {clean} && cp {hook} .git/hooks/post-index-change && echo y > f.txt && cd .. && {OUTPUT}",
        fsmon = fsmon.display(), clean = clean.display(), hook = hook.display()
    );
    let out = start(&s, "nested", &agent);
    assert!(out.contains("sub/.git"), "the nested repository is not named as refused: {out}");
    assert!(ran(&marks).is_empty(), "the engine's git ran what the nested repository set up: {:?}", ran(&marks));
    let e = events(&s, "nested");
    assert!(!e.iter().any(|e| e["type"] == "StepDone" && e["step"] == "change"), "a change with a nested repository was taken");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_engine_s_git_reads_none_of_the_person_s_own_configuration_on_the_copy() {
    let s = scratch("git-global");
    let marks = s.home.parent().expect("base").join("marks");
    // the person's own configuration defines a filter, as git-lfs does; the agent's attributes call it on every file
    let home = s.home.parent().expect("base").join("person-home");
    std::fs::create_dir_all(&home).expect("home");
    let clean = marker(&marks, "global-clean");
    std::fs::write(home.join(".gitconfig"), format!("[filter \"x\"]\n\tclean = {}\n\tsmudge = {}\n", clean.display(), clean.display())).expect("config");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && printf '* filter=x\\n' > .gitattributes && {OUTPUT}");
    let role = root().join("crates/shell/tests/roles/small-change");
    let out = Command::new(BIN)
        .args(["start", "--run", "global", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .env("HOME", &home)
        .output()
        .expect("engine");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(said.contains("delivered"), "{said}");
    assert!(ran(&marks).is_empty(), "the engine's git ran a filter of the person's own configuration on the agent's files: {:?}", ran(&marks));
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_git_file_pointing_elsewhere_and_a_bare_repository_in_the_tree_change_nothing_the_engine_does() {
    let s = scratch("git-pointer");
    let marks = s.home.parent().expect("base").join("marks");
    let hook = marker(&marks, "elsewhere-hook");
    let elsewhere = s.home.parent().expect("base").join("elsewhere.git");
    let ok = Command::new("git").args(["init", "-q", "--bare"]).arg(&elsewhere).status().expect("git").success();
    assert!(ok);
    std::fs::copy(&hook, elsewhere.join("hooks/post-index-change")).expect("hook");
    std::fs::copy(&hook, elsewhere.join("hooks/reference-transaction")).expect("hook");
    // the agent swaps its .git for a file pointing at another repository, and leaves a bare repository in the tree
    let agent = format!(
        "perl -pi -e 's/hello/hi/' greeting.txt && rm -rf .git && printf 'gitdir: {}\\n' > .git && git init -q --bare vendor/inner.git && \
         cp {} vendor/inner.git/hooks/post-index-change && {OUTPUT}",
        elsewhere.display(), hook.display()
    );
    let out = start(&s, "pointer", &agent);
    assert!(out.contains("delivered"), "{out}");
    assert!(ran(&marks).is_empty(), "the engine's git ran a hook of a repository the agent pointed at: {:?}", ran(&marks));
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/pointer:greeting.txt"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "hi\n");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_start_says_when_the_copy_lacks_git_lfs_content_or_submodules() {
    let s = scratch("git-notes");
    std::fs::write(s.project.join(".gitattributes"), "*.bin filter=lfs diff=lfs merge=lfs -text\n").expect("write");
    std::fs::write(s.project.join(".gitmodules"), "[submodule \"lib\"]\n\tpath = lib\n\turl = ../lib\n").expect("write");
    let out = start(&s, "notes", &format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}"));
    assert!(out.contains("note: the project keeps files in git-lfs"), "{out}");
    assert!(out.contains("note: the project has submodules"), "{out}");
    assert!(out.contains("delivered"), "the run is not refused: {out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_agent_s_git_reads_none_of_the_person_s_configuration_and_its_commits_are_not_the_delivery() {
    let s = scratch("agent-git");
    let said = s.home.parent().expect("base").join("said.txt");
    // the agent reads its history, asks for the person's identity, and commits in its own repository
    let agent = format!(
        "perl -pi -e 's/hello/hi/' greeting.txt && (git log -1 --format=%s; git config --global user.email || echo no-global; \
         git commit -qam mine && git log -1 --format='%an <%ae>') > {} 2>&1 && {OUTPUT}",
        said.display()
    );
    let out = start(&s, "agentgit", &agent);
    assert!(out.contains("delivered"), "{out}");
    let said = std::fs::read_to_string(&said).expect("said");
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(lines, ["start", "no-global", "Jaynes agent <agent@jaynes.invalid>"], "{said}");
    // what the agent committed is not what is delivered: the delivery is the change Jaynes measured
    let log = Command::new("git").arg("-C").arg(&s.project).args(["log", "--format=%an %s", "jaynes/agentgit"]).output().expect("git");
    assert!(!String::from_utf8_lossy(&log.stdout).contains("Jaynes agent mine"), "{}", String::from_utf8_lossy(&log.stdout));
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/agentgit:greeting.txt"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "hi\n");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which is macOS's sandbox-exec here")]
fn in_a_real_run_the_project_s_suite_neither_reads_nor_writes_the_pasteboard_nor_reaches_the_keychain() {
    // the path a real run takes: small-change runs the project's suite in the box on the run's copy under runs/. The
    // suite is a probe: it fails when it reads what was copied, writes the pasteboard, or searches the keychain.
    let s = scratch("mach");
    let pb = |args: &[&str], input: Option<&[u8]>| -> Option<Vec<u8>> {
        let mut c = Command::new(args[0]);
        c.args(&args[1..]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
        let mut child = c.spawn().ok()?;
        if let Some(i) = input {
            use std::io::Write;
            child.stdin.take()?.write_all(i).ok()?;
        }
        drop(child.stdin.take());
        let out = child.wait_with_output().ok()?;
        out.status.success().then_some(out.stdout)
    };
    // the find pasteboard, not the one the person copies with; what it held is put back at the end
    let saved = pb(&["/usr/bin/pbpaste", "-pboard", "find"], None);
    let token = format!("jaynes-mach-{}", std::process::id());
    let pasteboard = pb(&["/usr/bin/pbcopy", "-pboard", "find"], Some(token.as_bytes())).is_some()
        && pb(&["/usr/bin/pbpaste", "-pboard", "find"], None).as_deref() == Some(token.as_bytes());
    // the keychain's service answers outside the box: it lists the keychains (main's box let it answer, too)
    let keychain = Command::new("/usr/bin/security").arg("list-keychains").output().is_ok_and(|o| o.status.success());
    std::fs::write(s.project.join("probe.sh"), format!(r#"
if [ "$(/usr/bin/pbpaste -pboard find 2>/dev/null)" = "{token}" ]; then echo "read the pasteboard"; exit 1; fi
printf written-from-the-box | /usr/bin/pbcopy -pboard find 2>/dev/null
if /usr/bin/security list-keychains >/dev/null 2>&1; then echo "reached the keychain"; exit 1; fi
grep -q hi greeting.txt
"#)).expect("write");
    std::fs::write(s.project.join("Makefile"), "test:\n\tsh probe.sh\n").expect("write");
    for args in [&["add", "-A"][..], &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "probe"][..]] {
        assert!(Command::new("git").arg("-C").arg(&s.project).args(args).status().expect("git").success());
    }
    let out = start(&s, "mach", &format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}"));
    let after = pb(&["/usr/bin/pbpaste", "-pboard", "find"], None);
    if let Some(v) = &saved {
        let _back = pb(&["/usr/bin/pbcopy", "-pboard", "find"], Some(v));
    }
    let e = events(&s, "mach");
    assert!(types(&e).iter().any(|t| t == "Delivered"), "the suite failed in the box:\n{out}\n{:?}", types(&e));
    if pasteboard {
        assert_eq!(after.as_deref(), Some(token.as_bytes()), "the suite wrote the pasteboard");
    }
    assert!(pasteboard || keychain, "neither the pasteboard nor the keychain answers here, so this test proves nothing");
}

/// A test that finds someone else's program on its page's port (as one that had listened on 18082 for three days,
/// and held a whole queue of tests for 55 minutes) fails within its wait, saying what answered, and never hangs.
#[test]
fn a_page_test_that_reaches_another_program_fails_within_its_wait() {
    use std::io::{Read, Write};
    // one that takes the connection and says nothing
    let silent = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    let port = silent.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        let held: Vec<_> = silent.incoming().take(64).flatten().collect();
        std::thread::sleep(std::time::Duration::from_secs(60));
        drop(held);
    });
    let started = std::time::Instant::now();
    assert!(std::panic::catch_unwind(|| page_up(port)).is_err(), "a silent program is not the page");
    assert!(started.elapsed() < PAGE_WAIT + std::time::Duration::from_secs(5), "it waited {:?}", started.elapsed());
    // one that answers something else is told at once
    let other = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    let port = other.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for mut c in other.incoming().take(8).flatten() {
            let mut buf = [0u8; 512];
            let _read = c.read(&mut buf);
            let _sent = c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nnot jaynes");
        }
    });
    let started = std::time::Instant::now();
    let said = std::panic::catch_unwind(|| page_up(port)).expect_err("another program is not the page");
    let said = said.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(said.contains("not Jaynes's page") && said.contains("not jaynes"), "{said}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
}
