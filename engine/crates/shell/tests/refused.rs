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

/// A test's folder, taken away when the test ends, even when it panics.
struct Base(std::path::PathBuf);

impl Drop for Base {
    fn drop(&mut self) {
        let _gone = std::fs::remove_dir_all(&self.0);
    }
}

/// How long a test waits on the page.
const PAGE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// A run waiting at its stop, and the page on it at a port the system gives: the folder, the home, the question's id
/// and what it shows, the page and its port.
struct Asked {
    _base: Base,
    home: std::path::PathBuf,
    id: String,
    shown: String,
    page: Option<Page>,
    port: u16,
}

impl Asked {
    fn engine(&self, args: &[&str]) -> (bool, String) {
        let out = Command::new(BIN).args(args).env("JAYNES_ENGINE_HOME", &self.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "").output().expect("engine");
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    }

    /// The person keeps the change and the run finishes; the page stops.
    fn finish(&mut self) {
        let keep = format!(r#"{{"run":"tried","question":"{}","shown":"{}","verdicts":{{}}}}"#, self.id, self.shown);
        assert_eq!(json(&post(self.port, "/api/answer", &keep, &format!("http://localhost:{}", self.port), "k3y"))["ok"], true);
        let mut finished = false;
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if self.engine(&["log", "tried"]).1.contains("RunFinished") {
                finished = true;
                break;
            }
        }
        assert!(finished, "the run did not finish");
        self.page = None;
    }
}

fn asked(name: &str) -> Asked {
    let base = std::env::temp_dir().join(format!("jaynes-refused-{name}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let project = base.join("project");
    std::fs::create_dir_all(&project).expect("dir");
    std::fs::write(project.join("greeting.txt"), "hello\n").expect("write");
    std::fs::write(project.join("Makefile"), "test:\n\tgrep -q hi greeting.txt\n").expect("write");
    for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]] {
        assert!(Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success(), "git {args:?}");
    }
    let home = base.join("home");
    let mut a = Asked { _base: Base(base.clone()), home, id: String::new(), shown: String::new(), page: None, port: 0 };
    let role = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/roles/small-change");
    // the Makefile changed raises the risk: the person is asked about the change
    let agent = r#"command:printf 'hi\n' > greeting.txt && printf 'test:\n\tgrep -q hi greeting.txt\n\n.PHONY: test\n' > Makefile && printf '{"summary":"hi","files":["greeting.txt"]}' > .jaynes/output.json"#;
    let (ok, out) = a.engine(&["start", "--run", "tried", "--role", role.to_str().expect("path"), "--project", project.to_str().expect("path"), "--request", "say hi", "--agent", agent]);
    assert!(ok, "{out}");
    let (_, log) = a.engine(&["log", "tried"]);
    let opened: serde_json::Value = log.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).find(|e| e["type"] == "QuestionOpened").expect("asked");
    a.id = opened["question"]["id"].as_str().expect("id").to_string();
    a.shown = jaynes_core::question::Question::shown(&serde_json::from_value(opened["question"].clone()).expect("question")).0;
    let said_file = base.join("said.txt");
    let said = std::fs::File::create(&said_file).expect("said");
    let child = Command::new(BIN)
        .args(["page", "--port", "0"])
        .env("JAYNES_ENGINE_HOME", &a.home).env("JAYNES_HOME", base.join("jaynes")).env("JAYNES_NO_PAGE", "1").env("JAYNES_NO_UPDATE_CHECK", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "").env("JAYNES_PAGE_KEY", "k3y")
        .stdout(said.try_clone().expect("said"))
        .stderr(said)
        .spawn()
        .expect("page");
    a.page = Some(Page(child));
    let until = std::time::Instant::now() + PAGE_WAIT;
    a.port = loop {
        let said = std::fs::read_to_string(&said_file).unwrap_or_default();
        if let Some(p) = said.split("the page: http://localhost:").nth(1).and_then(|r| r.split('/').next()).and_then(|p| p.parse::<u16>().ok()) {
            break p;
        }
        assert!(std::time::Instant::now() < until, "the page never said its port: {said}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    a
}

fn http(port: u16, raw: &str) -> String {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(PAGE_WAIT)).expect("timeout");
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
    let mut a = asked("trace");
    let (port, id, shown) = (a.port, a.id.clone(), a.shown.clone());
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
    a.finish();
    // the summary says it too, apart from what was proved and what was not
    let (ok, told) = a.engine(&["summary", "tried"]);
    assert!(ok, "{told}");
    let refused = told.split("### Refused\n\n").nth(1).and_then(|t| t.split("\n### ").next()).unwrap_or_else(|| panic!("no Refused section: {told}"));
    assert_eq!(refused.lines().filter(|l| l.starts_with("- ")).count(), 3, "{refused}");
    assert!(refused.contains("answer to change, by a client without the page's key:"), "{refused}");
    assert!(refused.contains("act undo, by a client without the page's key:"), "{refused}");
    assert!(told.contains("kept beside the record, outside its chain"), "{told}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_trace_keeps_none_of_the_words_of_the_request_it_refused() {
    // the summary is copied out of the machine: what a request said never goes with it, whatever it says, whoever
    // sends it and wherever it puts it
    let mut a = asked("words");
    let (port, id, shown) = (a.port, a.id.clone(), a.shown.clone());
    let ours = format!("http://localhost:{port}");
    const SAID: &str = "LINE-FROM-THE-PARTNER-REPO";
    let tries = [
        // an action the page does not know, from an agent and from the person
        (format!(r#"{{"run":"tried","action":"{SAID}"}}"#), "/api/act", ours.clone(), ""),
        (format!(r#"{{"run":"tried","action":"{SAID}"}}"#), "/api/act", ours.clone(), "k3y"),
        // a correction of a step the role does not have
        (format!(r#"{{"run":"tried","action":"correct","note":"{SAID}","step":"{SAID}"}}"#), "/api/act", ours.clone(), "k3y"),
        // a verdict the stop does not offer, and an item it does not have
        (format!(r#"{{"run":"tried","question":"{id}","shown":"{shown}","verdicts":{{"change":"{SAID}"}}}}"#), "/api/answer", ours.clone(), "k3y"),
        (format!(r#"{{"run":"tried","question":"{id}","shown":"{shown}","verdicts":{{"{SAID}":"keep"}}}}"#), "/api/answer", ours.clone(), "k3y"),
        // a question the run does not have
        (format!(r#"{{"run":"tried","question":"{SAID}","shown":"{shown}","verdicts":{{}}}}"#), "/api/answer", ours.clone(), "k3y"),
        // another origin, which says it in its own name
        (r#"{"run":"tried","action":"halt"}"#.to_string(), "/api/act", format!("http://evil.example/{SAID}"), "k3y"),
        (r#"{"run":"tried","action":"halt"}"#.to_string(), "/api/act", "http://evil.example:8080".to_string(), "k3y"),
    ];
    for (body, path, origin, key) in &tries {
        let reply = post(port, path, body, origin, key);
        assert!(reply.starts_with("HTTP/1.1 403") || json(&reply)["ok"] == false, "{path} {body} from {origin}: {reply}");
    }
    let screen = http(port, &format!("GET /run/tried HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"));
    assert!(screen.contains(&format!("{} attempts on this run were refused", tries.len())), "{screen}");
    assert!(!screen.contains(SAID), "the run's screen keeps what a request said: {screen}");
    // what is kept says what was tried, in the engine's words
    assert!(screen.contains("an unknown action, by a client without the page&#39;s key") || screen.contains("an unknown action, by a client without the page's key"), "{screen}");
    assert!(screen.contains("by a request from another origin: it did not come from this page"), "{screen}");
    // the run's screen is the person's own, on this machine: an origin of an origin's shape is named there
    assert!(screen.contains("by a request from http://evil.example:8080: it did not come from this page"), "{screen}");
    a.finish();
    let (ok, told) = a.engine(&["summary", "tried"]);
    assert!(ok, "{told}");
    assert!(told.contains("### Refused"), "{told}");
    assert!(!told.contains(SAID), "the summary keeps what a request said: {told}");
    // an origin is a name its sender chose, and can carry words: the summary, which leaves the machine, names none
    assert!(!told.contains("evil.example") && told.contains("by a request from another origin: it did not come from this page"), "{told}");
}
