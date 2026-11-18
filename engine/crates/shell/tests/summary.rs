//! The summary of a run end to end, as someone who never installed Jaynes reads it: the real binary, the real
//! small-change role, a real git project, an agent that is a shell command, and the page.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

struct Scratch {
    home: PathBuf,
    project: PathBuf,
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
    Scratch { home: base.join("home"), project }
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
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: jaynes_key=k3y\r\n\r\n").as_bytes()).expect("write");
    let mut out = String::new();
    let _read = s.read_to_string(&mut out);
    out
}

fn post(port: u16, path: &str, body: &str) -> serde_json::Value {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    let raw = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nContent-Type: application/json\r\nCookie: jaynes_key=k3y\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(raw.as_bytes()).expect("write");
    let mut out = String::new();
    let _read = s.read_to_string(&mut out);
    serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap_or("")).unwrap_or(serde_json::Value::Null)
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_delivered_run_is_summed_up_in_markdown_by_the_command() {
    let s = scratch("cli");
    start(&s, "small", &format!("sed -i '' 's/hello/hi/' greeting.txt && {OUTPUT} && echo '{{\"tokens\":1234,\"session\":\"s1\"}}'"));
    let (ok, told) = engine(&s, &["summary", "small"]);
    assert!(ok, "{told}");
    assert!(told.starts_with("## Jaynes run summary\n\n**Request:** say hi instead of hello\n\n- Run `small`: role small-change 3.0.0 (fingerprint "), "{told}");
    // the agent here is a line of shell with this machine's paths: the summary keeps it out
    assert!(told.contains("), agent a shell command, low risk\n- Outcome: delivered as jaynes/small at "), "{told}");
    assert!(told.contains("1. **runner** (Jaynes): done.\n2. **change** (agent): done, 1 attempt. Checks passed: field-required, field-max-length, suite-passes.\n"), "{told}");
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
    start(&s, "asked", &format!("sed -i '' 's/hello/hi/' greeting.txt && printf 'test:\\n\\ttrue\\n' > Makefile && {OUTPUT}"));
    let (ok, early) = engine(&s, &["summary", "asked"]);
    assert!(!ok && early.contains("run asked is not finished: a summary tells what a finished run did"), "{early}");
    let port = 19750 + (std::process::id() % 1000) as u16;
    let _page = Command::new(BIN)
        .args(["page", "--port", &port.to_string()])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_NO_UPDATE_CHECK", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "").env("JAYNES_PAGE_KEY", "k3y")
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(Page)
        .expect("page");
    std::thread::sleep(std::time::Duration::from_millis(500));
    // a run that still waits shows no summary
    let waiting = get(port, "/run/asked");
    assert!(waiting.contains("Answer: ") && !waiting.contains("id=\"sumtext\""), "{waiting}");
    let (_, log) = engine(&s, &["log", "asked"]);
    let opened: serde_json::Value = log.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).find(|e| e["type"] == "QuestionOpened").expect("asked");
    let shown = jaynes_core::question::Question::shown(&serde_json::from_value(opened["question"].clone()).expect("question")).0;
    let keep = format!(r#"{{"run":"asked","question":"{}","shown":"{shown}","verdicts":{{}}}}"#, opened["question"]["id"].as_str().expect("id"));
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
    let text = told.split("<pre class=\"said\" id=\"sumtext\">").nth(1).and_then(|t| t.split("</pre>").next()).expect("the summary on the page");
    assert!(text.starts_with("## Jaynes run summary\n\n**Request:** say hi instead of hello\n"), "{text}");
    assert!(text.contains(", high risk\n- Outcome: delivered as jaynes/asked at "), "{text}");
    assert!(text.contains("2. **change** (agent): done, 1 attempt. Checks passed: field-required, field-max-length, suite-passes. A person answered 1 question.\n"), "{text}");
    assert!(text.contains("### Promises proved of the role\n\n- Nothing is delivered before the project&#39;s suite passed on the change\n") || text.contains("### Promises proved of the role\n\n- Nothing is delivered before the project's suite passed on the change\n"), "{text}");
    // the person answered one stop: their time is not measured, and is not said as zero
    assert!(text.contains("- People: 1 stop answered by a person, 0 sent back. Their time on Jaynes"), "{text}");
    assert!(text.trim_end().ends_with("screens: not measured."), "{text}");
    // this role's stop does not hold the run, which went on and delivered: it stood at no stop
    assert!(!text.contains("The run stood"), "{text}");
    // the command says the same as the page
    let (ok, cli) = engine(&s, &["summary", "asked"]);
    assert!(ok && cli.contains("- People: 1 stop answered by a person, 0 sent back. Their time on Jaynes's screens: not measured.\n"), "{cli}");
}
