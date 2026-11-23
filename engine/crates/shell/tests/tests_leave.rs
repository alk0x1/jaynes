//! T1, the real-user's estoque (22/11, r1795365040-837): the project's old test brings a fake database up on 5432, so
//! the suite as the project defined it when the run began never passes in the box, and a run that changes the test
//! could deliver nothing. The agent says so; the person writes, on the page, "pode mudar o teste, usa uma porta livre";
//! Jaynes reads that as their leave; the check then runs the suite as the agent left it, and the merge and the claims
//! name the test changed. The agent and the model reading the words are shell commands here, so no model is reached.

use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");
const KEY: &str = "k3y";
const WORDS: &str = "pode mudar o teste, usa uma porta livre pro banco falso";

const ROLE: &str = r#"
role = "leave"
version = "1.0.0"
delivery = "merge"

[[step]]
name = "runner"
by = "jaynes"
use = "find-suite"

[[step]]
name = "change"
needs = ["runner"]
reach = { project = "write", write = ["**"] }
about = "Makes the change."
instructions = "Make the change: {{request}}"
checks = ["suite-passes"]

[step.output]
summary = { type = "string", about = "what changed" }
"#;

/// The estoque's test as the project had it: a fake database on 5432, which the box does not let a check take.
const OLD_TEST: &str = "import socket\ns = socket.socket()\ns.bind((\"127.0.0.1\", 5432))\nprint(\"banco falso na 5432\")\n";

/// The agent: its first turn says the suite as it began cannot pass in the box, and asks to change the test; the next,
/// after the person's words, moves the fake database off 5432 (into memory: a port of the system's choosing is a
/// finding of its own, 22/11) and delivers.
const AGENT: &str = r#"#!/bin/sh
cat > /dev/null
n=$(( $(cat "$BASE/count" 2>/dev/null || echo 0) + 1 ))
echo $n > "$BASE/count"
if [ $n = 1 ]; then
  echo 'the test brings its fake database up on 5432, which the box holds: may I change the test?' > .jaynes/impossible.txt
  echo '{"tokens":10,"session":"s1"}'; exit 0
fi
printf 'banco = {}\nprint("banco falso livre da 5432")\n' > tests/banco.py
printf '{"summary":"o ping passa pelo ESTOQUE_DB"}' > .jaynes/output.json
echo '{"tokens":10,"session":"s1"}'
"#;

struct Scratch {
    base: PathBuf,
    home: PathBuf,
    project: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _gone = std::fs::remove_dir_all(&self.base);
        }
    }
}

struct Page(std::process::Child);

impl Drop for Page {
    fn drop(&mut self) {
        let _killed = self.0.kill();
        let _reaped = self.0.wait();
    }
}

fn scratch(name: &str) -> Scratch {
    let base = std::env::temp_dir().join(format!("jaynes-leave-{name}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let project = base.join("estoque");
    std::fs::create_dir_all(project.join("tests")).expect("dir");
    std::fs::write(project.join("Makefile"), "test:\n\tpython3 tests/banco.py\n").expect("write");
    std::fs::write(project.join("tests/banco.py"), OLD_TEST).expect("write");
    for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]] {
        assert!(Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success(), "git {args:?}");
    }
    std::fs::create_dir_all(base.join("leave")).expect("dir");
    std::fs::write(base.join("leave/role.toml"), ROLE).expect("write");
    std::fs::write(base.join("agent.sh"), AGENT).expect("write");
    Scratch { home: base.join("home"), project, base }
}

impl Scratch {
    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(BIN);
        c.args(args)
            .env("JAYNES_ENGINE_HOME", &self.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "")
            .env("JAYNES_CHROME", "/nonexistent/no-test-browser").env("JAYNES_NO_UPDATE_CHECK", "1").env("BASE", &self.base);
        c
    }

    fn engine(&self, args: &[&str]) -> String {
        let out = self.command(args).output().expect("engine");
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    }

    fn events(&self, run: &str) -> Vec<Value> {
        self.engine(&["log", run]).lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }

    /// The page, with the person's key and a model that reads `read` from any words.
    fn page(&self, port: u16, read: &str) -> Page {
        let child = self
            .command(&["page", "--port", &port.to_string()])
            .env("JAYNES_PAGE_KEY", KEY).env("JAYNES_UNDERSTAND", format!("cat > /dev/null; echo '{read}'"))
            .stderr(std::process::Stdio::null()).stdout(std::process::Stdio::null())
            .spawn()
            .expect("page");
        for _ in 0..100 {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Page(child)
    }
}

/// A port for the page, at random among 20000, free on both of its addresses, as understand_guard.rs draws one.
fn port() -> u16 {
    use std::hash::{BuildHasher, Hasher};
    for _ in 0..200 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        let p = 20000 + (h.finish() % 20000) as u16;
        let four = std::net::TcpListener::bind(("127.0.0.1", p)).is_ok();
        let six = !std::net::TcpListener::bind(("::1", p)).is_err_and(|e| e.kind() == std::io::ErrorKind::AddrInUse);
        if four && six && TcpStream::connect_timeout(&format!("127.0.0.1:{p}").parse().expect("address"), Duration::from_millis(200)).is_err() {
            return p;
        }
    }
    panic!("no free port between 20000 and 40000")
}

/// A POST to the page as the person, with their key: the reply's body.
fn post(port: u16, path: &str, body: &Value) -> Value {
    let body = body.to_string();
    let raw = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://localhost:{port}\r\nContent-Type: application/json\r\nX-Jaynes-Key: {KEY}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
    stream.write_all(raw.as_bytes()).expect("write");
    let mut out = String::new();
    let _read = stream.read_to_string(&mut out);
    serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap_or("")).unwrap_or(Value::Null)
}

/// The estoque's run up to the agent's question, answered on the page with the person's words as the model `read`
/// them: Jaynes's sentences and the run's record once it finished or blocked.
fn answered(s: &Scratch, read: &str) -> (Vec<String>, Value, Vec<Value>) {
    let agent = format!("command:sh {}", s.base.join("agent.sh").display());
    let out = s.engine(&["start", "--run", "estoque", "--role", s.base.join("leave").to_str().expect("path"), "--project",
                         s.project.to_str().expect("path"), "--request", "o ping passa pelo ESTOQUE_DB", "--agent", &agent]);
    let e = s.events("estoque");
    let asked = e.iter().rfind(|e| e["type"] == "InputNeeded").unwrap_or_else(|| panic!("the agent asks: {out}"));
    let question: jaynes_core::question::Question = serde_json::from_value(asked["question"].clone()).expect("question");
    let read = read.replace("ITEM", &question.items[0].id);
    // on the page, the person writes their words and sends what Jaynes read of them
    let port = port();
    let _page = s.page(port, &read);
    let base = serde_json::json!({"run": "estoque", "question": question.id, "shown": question.shown().0});
    let mut ask = base.clone();
    ask["text"] = WORDS.into();
    let reading = post(port, "/api/understand", &ask);
    let says: Vec<String> = reading["sentences"].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_string)).collect();
    let mut send = base.clone();
    for (k, v) in reading["body"].as_object().unwrap_or_else(|| panic!("read: {reading}")) {
        send[k] = v.clone();
    }
    let sent = post(port, "/api/answer", &send);
    assert_eq!(sent["ok"], true, "{sent}");
    let mut e = Vec::new();
    for _ in 0..300 {
        e = s.events("estoque");
        if e.iter().any(|e| e["type"] == "RunFinished" || e["type"] == "Blocked") {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    (says, send, e)
}

fn judged(e: &[Value]) -> Value {
    e.iter().rfind(|e| e["type"] == "ChecksJudged" && e["step"] == "change").cloned().unwrap_or_else(|| panic!("judged: {:?}", e.iter().map(|e| e["type"].clone()).collect::<Vec<_>>()))
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn asked_the_estoque_with_the_person_s_leave_the_check_runs_the_test_the_agent_left_and_the_merge_names_it() {
    let s = scratch("with");
    let (says, sent, e) = answered(&s, r#"{"verdicts": {"ITEM": "change"}, "tests": true}"#);
    assert_eq!(sent["tests_leave"], WORDS, "{sent}");
    assert!(says.iter().any(|s| s.starts_with("You let this run change how the project's tests are defined")), "Jaynes says what it does: {says:?}");
    let types: Vec<&str> = e.iter().filter_map(|e| e["type"].as_str()).collect();
    let left = e.iter().find(|e| e["type"] == "TestsLeft").unwrap_or_else(|| panic!("the leave is in the record: {types:?}"));
    assert_eq!((left["words"].as_str(), left["by"].clone()), (Some(WORDS), serde_json::json!({"by": "person"})), "{left}");
    let last = judged(&e);
    assert_eq!(last["checks"][0]["passed"], true, "the test the agent left passes: {last}");
    assert_eq!(last["checks"][0]["tests_changed"], serde_json::json!(["tests/banco.py"]), "{last}");
    // the make test runs tests/banco.py outside any box later: the merge asks, and its card says the test changed
    let opened = e.iter().rfind(|e| e["type"] == "QuestionOpened").unwrap_or_else(|| panic!("the merge asks: {types:?}"));
    let merge: jaynes_core::question::Question = serde_json::from_value(opened["question"].clone()).expect("question");
    assert_eq!(merge.kind, jaynes_core::question::QuestionKind::Merge, "{opened}");
    let port = port();
    let _page = s.page(port, "{}");
    let keep: serde_json::Map<String, Value> = merge.items.iter().map(|i| (i.id.clone(), Value::from("keep"))).collect();
    let body = serde_json::json!({"run": "estoque", "question": merge.id, "shown": merge.shown().0, "verdicts": keep});
    let card = post(port, "/api/effects", &body);
    let card: Vec<&str> = card["sentences"].as_array().into_iter().flatten().filter_map(|s| s.as_str()).collect();
    assert!(card.contains(&"Tests changed with your leave: tests/banco.py."), "{card:?}");
    assert_eq!(post(port, "/api/answer", &body)["ok"], true);
    let mut e = Vec::new();
    for _ in 0..300 {
        e = s.events("estoque");
        if e.iter().any(|e| e["type"] == "RunFinished") {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(e.iter().any(|e| e["type"] == "RunFinished"), "{:?}", e.iter().map(|e| e["type"].clone()).collect::<Vec<_>>());
    // the merge on the person's branch, and the claims, name the test changed
    let merged = Command::new("git").arg("-C").arg(&s.project).args(["log", "-1", "--format=%B", "main"]).output().expect("git");
    let merged = String::from_utf8_lossy(&merged.stdout).to_string();
    assert!(merged.contains("Tests changed with the person's leave: tests/banco.py."), "{merged}");
    let claims = s.engine(&["summary", "estoque"]);
    assert!(claims.contains("- **suite-passes**, on what change made: tests changed with the person's leave: `tests/banco.py`"), "{claims}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn asked_the_estoque_without_the_leave_the_suite_as_it_began_still_judges_and_nothing_lands() {
    let s = scratch("without");
    let (says, sent, e) = answered(&s, r#"{"verdicts": {"ITEM": "change"}, "tests": false}"#);
    assert!(sent.get("tests_leave").is_none(), "{sent}");
    assert!(!says.iter().any(|s| s.contains("tests are defined")), "{says:?}");
    assert!(!e.iter().any(|e| e["type"] == "TestsLeft"));
    let last = judged(&e);
    assert_eq!(last["checks"][0]["passed"], false, "the old test on 5432 still judges: {last}");
    let merged = Command::new("git").arg("-C").arg(&s.project).args(["log", "-1", "--format=%s", "main"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&merged.stdout).trim(), "start", "nothing lands");
}
