//! An attempt to answer or act on a run that the page refuses leaves a trace, outside the record's chain: when, on
//! what, by whom, and why. The run's screen and its summary show it.

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

/// A page process that dies with its test, even when the test panics.
struct Page(std::process::Child);

impl Drop for Page {
    fn drop(&mut self) {
        let _killed = self.0.kill();
        let _reaped = self.0.wait();
    }
}

fn http(port: u16, raw: &str) -> String {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.write_all(raw.as_bytes()).expect("write");
    let mut out = String::new();
    let _read = s.read_to_string(&mut out);
    out
}

fn post(port: u16, path: &str, body: &str, origin: &str, key: &str) -> String {
    let key = if key.is_empty() { String::new() } else { format!("X-Jaynes-Key: {key}\r\n") };
    http(port, &format!("POST {path} HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: {origin}\r\nContent-Type: application/json\r\n{key}Content-Length: {}\r\n\r\n{body}", body.len()))
}

fn json(reply: &str) -> serde_json::Value {
    serde_json::from_str(reply.split("\r\n\r\n").nth(1).unwrap_or("")).unwrap_or(serde_json::Value::Null)
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_refused_attempt_to_answer_or_act_leaves_a_trace_on_the_run() {
    let base = std::env::temp_dir().join(format!("jaynes-refused-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let project = base.join("project");
    std::fs::create_dir_all(&project).expect("dir");
    std::fs::write(project.join("greeting.txt"), "hello\n").expect("write");
    std::fs::write(project.join("Makefile"), "test:\n\tgrep -q hi greeting.txt\n").expect("write");
    for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]] {
        assert!(Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success(), "git {args:?}");
    }
    let home = base.join("home");
    let engine = |args: &[&str]| {
        let out = Command::new(BIN).args(args).env("JAYNES_ENGINE_HOME", &home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "").output().expect("engine");
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    };
    let role = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/roles/small-change");
    // the Makefile changed raises the risk: the person is asked about the change
    let agent = r#"command:printf 'hi\n' > greeting.txt && printf 'test:\n\ttrue\n' > Makefile && printf '{"summary":"hi","files":["greeting.txt"]}' > .jaynes/output.json"#;
    let (ok, out) = engine(&["start", "--run", "tried", "--role", role.to_str().expect("path"), "--project", project.to_str().expect("path"), "--request", "say hi", "--agent", agent]);
    assert!(ok, "{out}");
    let (_, log) = engine(&["log", "tried"]);
    let opened: serde_json::Value = log.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).find(|e| e["type"] == "QuestionOpened").expect("asked");
    let id = opened["question"]["id"].as_str().expect("id").to_string();
    let shown = jaynes_core::question::Question::shown(&serde_json::from_value(opened["question"].clone()).expect("question")).0;
    let port = 24750 + (std::process::id() % 1000) as u16;
    let page = Command::new(BIN)
        .args(["page", "--port", &port.to_string()])
        .env("JAYNES_ENGINE_HOME", &home).env("JAYNES_HOME", base.join("jaynes")).env("JAYNES_NO_PAGE", "1").env("JAYNES_NO_UPDATE_CHECK", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "").env("JAYNES_PAGE_KEY", "k3y")
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(Page)
        .expect("page");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let ours = format!("http://localhost:{port}");
    // an agent, without the page's key, tries to send the change back: only the person may
    let redo = format!(r#"{{"run":"tried","question":"{id}","shown":"{shown}","verdicts":{{"change":"redo"}},"notes":{{"change":"smaller"}}}}"#);
    let r = json(&post(port, "/api/answer", &redo, &ours, ""));
    assert_eq!(r["ok"], false, "{r}");
    // another site, key and all, tries to answer: refused before anything is read
    assert!(post(port, "/api/answer", &redo, "https://example.com", "k3y").starts_with("HTTP/1.1 403"));
    // an agent tries an action only the person takes
    let undo = r#"{"run":"tried","action":"undo"}"#;
    assert_eq!(json(&post(port, "/api/act", undo, &ours, ""))["ok"], false);
    // what the page refuses is on the run's screen
    let screen = http(port, &format!("GET /run/tried HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"));
    assert!(screen.contains("3 attempts on this run were refused"), "{screen}");
    assert!(screen.contains("answer to change, by a client without the page&#39;s key") || screen.contains("answer to change, by a client without the page's key"), "{screen}");
    assert!(screen.contains("answer, by a request from https://example.com: it did not come from this page"), "{screen}");
    // the person's own answer is not refused, and leaves no trace
    let keep = format!(r#"{{"run":"tried","question":"{id}","shown":"{shown}","verdicts":{{}}}}"#);
    assert_eq!(json(&post(port, "/api/answer", &keep, &ours, "k3y"))["ok"], true);
    let mut finished = false;
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if engine(&["log", "tried"]).1.contains("RunFinished") {
            finished = true;
            break;
        }
    }
    assert!(finished, "the run did not finish");
    drop(page);
    // the summary says it too, apart from what was proved and what was not
    let (ok, told) = engine(&["summary", "tried"]);
    assert!(ok, "{told}");
    let refused = told.split("### Refused\n\n").nth(1).and_then(|t| t.split("\n### ").next()).unwrap_or_else(|| panic!("no Refused section: {told}"));
    assert_eq!(refused.lines().filter(|l| l.starts_with("- ")).count(), 3, "{refused}");
    assert!(refused.contains("answer to change, by a client without the page's key:"), "{refused}");
    assert!(refused.contains("act undo, by a client without the page's key:"), "{refused}");
    assert!(told.contains("kept beside the record, outside its chain"), "{told}");
}
