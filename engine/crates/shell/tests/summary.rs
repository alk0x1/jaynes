//! The summary of a run end to end, as someone who never installed Jaynes reads it: the real binary, the real
//! small-change role, a real git project, an agent that is a shell command, and the page. The agents' commands are
//! plain POSIX shell, with no tool that differs between macOS and Linux (such as sed -i).

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

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
            let _sent = s.write_all(format!("GET /api/version HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes());
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

fn scratch(name: &str) -> Scratch {
    let base = std::env::temp_dir().join(format!("jaynes-summary-{name}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let project = base.join("project");
    std::fs::create_dir_all(&project).expect("dir");
    std::fs::write(project.join("greeting.txt"), "hello\n").expect("write");
    std::fs::write(project.join("Makefile"), "test:\n\tgrep -q hi greeting.txt\n").expect("write");
    for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]] {
        assert!(Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success(), "git {args:?}");
    }
    Scratch { home: base.join("home"), project, base }
}

fn engine(s: &Scratch, args: &[&str]) -> (bool, String) {
    let out = Command::new(BIN).args(args).env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "").output().expect("engine");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

fn start(s: &Scratch, run: &str, agent: &str) {
    let role = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/roles/small-change");
    let (ok, out) = engine(s, &["start", "--run", run, "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"), "--request", "say hi instead of hello", "--agent", &format!("command:{agent}")]);
    assert!(ok, "start failed:\n{out}");
}

const OUTPUT: &str = r#"printf '{"summary":"hello becomes hi","files":["greeting.txt"]}' > .jaynes/output.json"#;

/// A page process that dies with its test, even when the test panics.
struct Page(std::process::Child);

impl Drop for Page {
    fn drop(&mut self) {
        let _killed = self.0.kill();
        let _reaped = self.0.wait();
    }
}

fn get(port: u16, path: &str) -> String {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(PAGE_WAIT)).expect("timeout");
    s.set_write_timeout(Some(PAGE_WAIT)).expect("timeout");
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes()).expect("write");
    let mut out = String::new();
    if let Err(e) = s.read_to_string(&mut out) {
        panic!("the page on port {port} did not answer within {} s ({e}); what came: {}", PAGE_WAIT.as_secs(), out.chars().take(300).collect::<String>());
    }
    out
}

fn post(port: u16, path: &str, body: &str) -> serde_json::Value {
    post_as(port, path, body, "jaynes_key=k3y")
}

/// A POST from the page's own origin; `me` is "jaynes_key=KEY" for the person, sent as the page's script sends it
/// (the key in its header), or "" for a client without the key.
fn post_as(port: u16, path: &str, body: &str, me: &str) -> serde_json::Value {
    use std::io::{Read, Write};
    let key = me.strip_prefix("jaynes_key=").map(|k| format!("X-Jaynes-Key: {k}\r\n")).unwrap_or_default();
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(PAGE_WAIT)).expect("timeout");
    s.set_write_timeout(Some(PAGE_WAIT)).expect("timeout");
    let raw = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://localhost:{port}\r\nContent-Type: application/json\r\n{key}Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(raw.as_bytes()).expect("write");
    let mut out = String::new();
    if let Err(e) = s.read_to_string(&mut out) {
        panic!("the page on port {port} did not answer within {} s ({e}); what came: {}", PAGE_WAIT.as_secs(), out.chars().take(300).collect::<String>());
    }
    serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap_or("")).unwrap_or(serde_json::Value::Null)
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_delivered_run_is_summed_up_in_markdown_by_the_command() {
    let s = scratch("cli");
    start(&s, "small", &format!("printf 'hi\\n' > greeting.txt && {OUTPUT} && echo '{{\"tokens\":1234,\"session\":\"s1\"}}'"));
    let (ok, told) = engine(&s, &["summary", "small"]);
    assert!(ok, "{told}");
    assert!(told.starts_with("## say hi instead of hello\n\n### Delivered\n\njaynes/small at "), "{told}");
    assert!(told.contains(", in a repository. The files Jaynes measured:\n\n- `greeting.txt`: 2 lines changed\n\n### Proved by\n\n- **field-required**, on what change made\n- **field-max-length**, on what change made\n- **suite-passes**, on what change made\n"), "{told}");
    // nothing a person was asked: the change was small, so its stop did not ask
    assert!(told.contains("### Not proved\n\n- The stop at change asks only at high risk, and this run was low: no person looked at what change made.\n"), "{told}");
    assert!(told.contains("- Run `small`: role small-change 3.0.0 (fingerprint "), "{told}");
    // the promises were proved: what is not proved never says they were not read
    assert!(!told.contains("The role's promises:"), "{told}");
    // the agent here is a line of shell with this machine's paths: the summary keeps it out
    assert!(told.contains(&format!("), agent a shell command, low risk, engine {}\n", jaynes_core::ENGINE)), "{told}");
    assert!(told.contains("1. **runner** (Jaynes): done.\n2. **change** (agent): done, 1 attempt.\n"), "{told}");
    // the role's promise, proved again from the run's own copy of the role
    assert!(told.contains("### Promises proved of the role\n\n- Nothing is delivered before the project's suite passed on the change\n"), "{told}");
    assert!(told.contains("1,234 tokens, 1 attempt (0 spent by the agent's own fault), 0 fixes.\n- People: no stop answered by a person.\n"), "{told}");
    // nobody was asked, so the run stood at no stop
    assert!(!told.contains("That is waiting"), "{told}");
    // the proof is kept, so the next reading does not prove again
    let kept: Vec<_> = std::fs::read_dir(s.home.join("proofs")).expect("proofs").flatten().collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    let (ok, again) = engine(&s, &["summary", "small"]);
    assert!(ok && again == told, "{again}");
    let (ok, none) = engine(&s, &["summary", "no-such-run"]);
    assert!(!ok && none.contains("there is no run no-such-run"), "{none}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_run_s_screen_shows_the_summary_to_copy_once_the_run_is_finished() {
    let s = scratch("page");
    // a change to the Makefile runs later outside any box: the risk goes up and the person is asked
    start(&s, "asked", &format!("printf 'hi\\n' > greeting.txt && printf 'test:\\n\\ttrue\\n' > Makefile && {OUTPUT}"));
    let (ok, early) = engine(&s, &["summary", "asked"]);
    assert!(!ok && early.contains("run asked is not finished: a summary tells what a finished run did"), "{early}");
    let port = free_port();
    let _page = Command::new(BIN)
        .args(["page", "--port", &port.to_string()])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_NO_UPDATE_CHECK", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "").env("JAYNES_PAGE_KEY", "k3y")
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(Page)
        .expect("page");
    page_up(port);
    // a run that still waits shows no summary
    let waiting = get(port, "/run/asked");
    assert!(waiting.contains("Answer: ") && !waiting.contains("id=\"sumtext\""), "{waiting}");
    let (_, log) = engine(&s, &["log", "asked"]);
    let opened: serde_json::Value = log.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).find(|e| e["type"] == "QuestionOpened").expect("asked");
    let shown = jaynes_core::question::Question::shown(&serde_json::from_value(opened["question"].clone()).expect("question")).0;
    let id = opened["question"]["id"].as_str().expect("id");
    // the person is on the stop's screen: the page says so; without the page's key it does not count
    let look = format!(r#"{{"run":"asked","question":"{id}"}}"#);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let r = post_as(port, "/api/look", &look, "");
    assert!(r["ok"] == false && r["refused"] == "only the person's presence counts", "{r}");
    let r = post(port, "/api/look", &look);
    assert!(r["ok"] == true && r["recorded"] == true, "{r}");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let keep = format!(r#"{{"run":"asked","question":"{id}","shown":"{shown}","verdicts":{{}}}}"#);
    assert_eq!(post(port, "/api/answer", &keep)["ok"], true);
    let mut told = String::new();
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        told = get(port, "/run/asked");
        // the promises are proved beside the page, which never waits for them
        if told.contains("id=\"sumtext\"") && !told.contains("Being proved again now") {
            break;
        }
    }
    assert!(told.contains("<button class=\"copy\" id=\"copysum\">Copy as Markdown</button>"), "{told}");
    // once the answer is in the record, a late signal is dropped (and the summary cuts any span at the answer)
    let r = post(port, "/api/look", &look);
    assert!(r["ok"] == true && r["recorded"] == false, "{r}");
    let text = told.split("<pre class=\"said\" id=\"sumtext\">").nth(1).and_then(|t| t.split("</pre>").next()).expect("the summary on the page");
    assert!(text.starts_with("## say hi instead of hello\n\n### Delivered\n\njaynes/asked at "), "{text}");
    assert!(text.contains(&format!(", high risk, engine {}\n", jaynes_core::ENGINE)), "{text}");
    assert!(text.contains("2. **change** (agent): done, 1 attempt. A person answered 1 question.\n"), "{text}");
    assert!(text.contains("### Promises proved of the role\n\n- Nothing is delivered before the project&#39;s suite passed on the change\n") || text.contains("### Promises proved of the role\n\n- Nothing is delivered before the project's suite passed on the change\n"), "{text}");
    // the person answered one stop, and their time on its screen was measured: seconds, never the wait
    assert!(text.contains("- People: 1 stop answered by a person, 0 sent back. Their time on Jaynes"), "{text}");
    let time = text.split("screens: ").nth(1).and_then(|t| t.split(".\n").next()).expect("their time");
    assert!(time.ends_with(" s") && time.trim_end_matches(" s").parse::<u64>().is_ok_and(|s| (1..=15).contains(&s)), "{time}");
    // this role's stop does not hold the run, which went on and delivered: it stood at no stop
    assert!(!text.contains("The run stood"), "{text}");
    // the command says the same as the page
    let (ok, cli) = engine(&s, &["summary", "asked"]);
    assert!(ok && cli.contains(&format!("- People: 1 stop answered by a person, 0 sent back. Their time on Jaynes's screens: {time}.\n")), "{cli}");
}
