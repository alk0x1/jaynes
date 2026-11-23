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

/// The estoque's test as the project had it: it reaches its database on 5432, which the box does not let a check
/// reach. It only reaches it: a test that brings a server up of its own stops the run at its start since T5
/// (949c8475), before any agent, and the person's way out there is the test servers, not a leave in words.
const OLD_TEST: &str = "import socket\nsocket.create_connection((\"127.0.0.1\", 5432), timeout=3)\nprint(\"banco na 5432\")\n";

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
    scratch_with(name, ROLE, AGENT)
}

fn scratch_with(name: &str, role: &str, agent: &str) -> Scratch {
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
    std::fs::write(base.join("leave/role.toml"), role).expect("write");
    std::fs::write(base.join("agent.sh"), agent).expect("write");
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

/// The fix-bug role's check, tests-pass (the real-user's contas, 23/11, r1795449920-193): the tests the step names pass,
/// then the whole suite. The leave was recorded (TestsLeft) and the page promised it, and tests-pass still put the
/// suite of the start back, failing "as the project defined it when the run began" after the person's words.
const ROLE_TESTS_PASS: &str = r#"
role = "leave"
version = "1.0.0"
delivery = "merge"

[[step]]
name = "runner"
by = "jaynes"
use = "find-suite"

[[step]]
name = "reproduce"
needs = ["runner"]
about = "Names the tests that show the bug."
instructions = "Name the tests: {{request}}"

[step.output]
tests = { type = "array", about = "the tests that show it" }

[[step]]
name = "change"
needs = ["runner", "reproduce"]
reach = { project = "write", write = ["**"] }
about = "Makes the change."
instructions = "Make the change: {{request}}"
checks = [{ use = "tests-pass", tests_from = "reproduce" }]

[step.output]
summary = { type = "string", about = "what changed" }
"#;

const AGENT_TESTS: &str = r#"#!/bin/sh
cat > /dev/null
n=$(( $(cat "$BASE/count" 2>/dev/null || echo 0) + 1 ))
echo $n > "$BASE/count"
if [ $n = 1 ]; then
  printf '{"tests":["tests/banco.py"]}' > .jaynes/output.json
  echo '{"tokens":10,"session":"s0"}'; exit 0
fi
if [ $n = 2 ]; then
  echo 'the test reaches its database on 5432, which the box holds: may I change the test?' > .jaynes/impossible.txt
  echo '{"tokens":10,"session":"s1"}'; exit 0
fi
printf 'banco = {}\nprint("banco falso livre da 5432")\n' > tests/banco.py
printf '{"summary":"o ping passa pelo ESTOQUE_DB"}' > .jaynes/output.json
echo '{"tokens":10,"session":"s1"}'
"#;

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn asked_the_contas_tests_pass_judges_the_tests_as_the_agent_left_them_with_the_person_s_leave() {
    let s = scratch_with("tests-pass", ROLE_TESTS_PASS, AGENT_TESTS);
    let (_, sent, e) = answered(&s, r#"{"verdicts": {"ITEM": "change"}, "tests": true}"#);
    assert_eq!(sent["tests_leave"], WORDS, "{sent}");
    assert!(e.iter().any(|e| e["type"] == "TestsLeft"), "the leave is in the record");
    let last = judged(&e);
    assert_eq!((&last["checks"][0]["check"], &last["checks"][0]["passed"]), (&serde_json::json!("tests-pass"), &serde_json::json!(true)), "the tests the agent left pass: {last}");
    assert_eq!(last["checks"][0]["tests_changed"], serde_json::json!(["tests/banco.py"]), "{last}");
    let asked: Vec<&Value> = e.iter().filter(|e| e["type"] == "FixAsked").collect();
    assert!(asked.is_empty(), "no fix asked after the leave: {asked:?}");
}

/// The real-user's contas (23/11, main 2c386d20, r1795467794-731 and -734): the fix's stop offers keep and redo, and
/// the person's leave in words reached the run only with a keep or at the merge, where nothing runs again (TestsLeft,
/// seq 978, then Merged); the Redo with the same words made the fix again without it, and the check refused the
/// change. Here the suite is make's, so the Makefile says how the tests run, as pyproject.toml did there: the agent
/// first fixes the greeting and says the old test is still out of the suite; sent back, it makes the suite run it.
const ROLE_CONTAS: &str = r#"
role = "contas"
version = "1.0.0"
delivery = "merge"

[[step]]
name = "runner"
by = "jaynes"
use = "find-suite"

[[step]]
name = "fix"
needs = ["runner"]
reach = { project = "write", write = ["**"] }
about = "Fixes it."
instructions = "Fix it: {{request}}"
checks = ["tests-defined-as-they-began", "suite-passes"]
stop = { ask = "Keep it?", verdicts = ["keep", "redo"] }
show = { label = "{{summary}}" }

[step.output]
summary = { type = "string", about = "what changed" }
"#;

const AGENT_CONTAS: &str = r#"#!/bin/sh
cat > /dev/null
n=$(( $(cat "$BASE/count" 2>/dev/null || echo 0) + 1 ))
echo $n > "$BASE/count"
if [ $n = 1 ]; then
  perl -pi -e 's/hello/hi/' greeting.txt
  printf '{"summary":"hello becomes hi; tests/old.sh is still outside the suite, which is yours to change"}' > .jaynes/output.json
else
  printf 'test:\n\tsh tests/new.sh\n\tsh tests/old.sh\n' > Makefile
  printf '{"summary":"the suite runs tests/old.sh too"}' > .jaynes/output.json
fi
echo '{"tokens":10,"session":"s1"}'
"#;

const CONTAS_WORDS: &str = "refaz fazendo o tests/old.sh rodar na suíte; pode mudar o Makefile pra isso";

fn contas(name: &str) -> Scratch {
    let s = scratch_with(name, ROLE_CONTAS, AGENT_CONTAS);
    std::fs::remove_file(s.project.join("tests/banco.py")).expect("rm");
    std::fs::write(s.project.join("greeting.txt"), "hello\n").expect("write");
    std::fs::write(s.project.join("Makefile"), "test:\n\tsh tests/new.sh\n").expect("write");
    std::fs::write(s.project.join("tests/new.sh"), "grep -q hi greeting.txt\n").expect("write");
    std::fs::write(s.project.join("tests/old.sh"), "grep -q hi greeting.txt\n").expect("write");
    for args in [vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "contas"]] {
        assert!(Command::new("git").arg("-C").arg(&s.project).args(&args).status().expect("git").success(), "git {args:?}");
    }
    let agent = format!("command:sh {}", s.base.join("agent.sh").display());
    let out = s.engine(&["start", "--run", name, "--role", s.base.join("leave").to_str().expect("path"), "--project",
                         s.project.to_str().expect("path"), "--request", "say hi; tests/old.sh never ran", "--agent", &agent]);
    assert!(out.contains("waiting"), "{out}");
    s
}

/// The open question of `kind` in the run, and the body the page starts an answer to it from.
fn open(s: &Scratch, run: &str, kind: &str) -> (jaynes_core::question::Question, Value) {
    let e = s.events(run);
    let opened = e.iter().rfind(|e| e["type"] == "QuestionOpened" && e["question"]["kind"] == kind).unwrap_or_else(|| panic!("no {kind}: {e:?}"));
    let q: jaynes_core::question::Question = serde_json::from_value(opened["question"].clone()).expect("question");
    let base = serde_json::json!({"run": run, "question": q.id, "shown": q.shown().0});
    (q, base)
}

fn sentences(v: &Value) -> Vec<String> {
    v["sentences"].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_string)).collect()
}

/// The person's words read on the page, as `read` says; what Jaynes said they do, and the body it would send.
fn read_words(port: u16, base: &Value, words: &str) -> (Vec<String>, Value) {
    let mut ask = base.clone();
    ask["text"] = words.into();
    let reading = post(port, "/api/understand", &ask);
    let mut send = base.clone();
    for (k, v) in reading["body"].as_object().unwrap_or_else(|| panic!("read: {reading}")) {
        send[k] = v.clone();
    }
    (sentences(&reading), send)
}

/// Once the fix, sent back, was judged again: the run's record.
fn judged_again(s: &Scratch, run: &str) -> Vec<Value> {
    for _ in 0..300 {
        let e = s.events(run);
        if e.iter().filter(|e| e["type"] == "ChecksJudged" && e["step"] == "fix").count() >= 2 || e.iter().any(|e| e["type"] == "Blocked") {
            return e;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("the fix was never judged again: {:?}", s.events(run).iter().map(|e| e["type"].clone()).collect::<Vec<_>>())
}

/// The leave recorded before the attempt that made the fix again, and that attempt's checks passed with it.
fn left_before_the_next_attempt(e: &[Value]) {
    let types: Vec<&str> = e.iter().filter_map(|e| e["type"].as_str()).collect();
    let left = e.iter().position(|e| e["type"] == "TestsLeft").unwrap_or_else(|| panic!("the leave is in the record: {types:?}"));
    assert_eq!((e[left]["words"].as_str(), e[left]["by"].clone()), (Some(CONTAS_WORDS), serde_json::json!({"by": "person"})), "{}", e[left]);
    let again = e.iter().rposition(|e| e["type"] == "AttemptStarted" && e["step"] == "fix").expect("an attempt");
    assert!(left < again, "the leave comes before the attempt that makes the fix again: {types:?}");
    let last = e.iter().rfind(|e| e["type"] == "ChecksJudged" && e["step"] == "fix").expect("judged");
    let defined = last["checks"].as_array().into_iter().flatten().find(|c| c["check"] == "tests-defined-as-they-began").expect("the check");
    assert_eq!((&defined["passed"], &defined["tests_changed"]), (&serde_json::json!(true), &serde_json::json!(["Makefile", "tests/old.sh"])), "the Makefile changed with the leave, and the script it now runs by its path: {last}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn asked_the_contas_words_read_as_a_redo_with_the_leave_record_it_before_the_next_attempt_and_its_change_passes() {
    let s = contas("contas-redo");
    let (q, base) = open(&s, "contas-redo", "decision");
    let port = port();
    let _page = s.page(port, &r#"{"verdicts": {"ITEM": "redo"}, "tests": true}"#.replace("ITEM", &q.items[0].id));
    let (says, send) = read_words(port, &base, CONTAS_WORDS);
    assert!(says.iter().any(|s| s.starts_with("You let this run change how the project's tests are defined")), "{says:?}");
    assert_eq!(send["verdicts"][&q.items[0].id], "redo", "{send}");
    assert_eq!(post(port, "/api/answer", &send)["ok"], true);
    left_before_the_next_attempt(&judged_again(&s, "contas-redo"));
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn asked_the_contas_words_read_as_a_keep_never_give_the_leave_and_the_redo_with_them_does() {
    let s = contas("contas-button");
    let (q, base) = open(&s, "contas-button", "decision");
    let port = port();
    // the reader takes the words for a keep, as it did at the real-user's fix: nothing runs again, so no leave
    let _page = s.page(port, &r#"{"verdicts": {"ITEM": "keep"}, "tests": true}"#.replace("ITEM", &q.items[0].id));
    let (says, send) = read_words(port, &base, CONTAS_WORDS);
    assert!(says.contains(&"Nothing will run again, so this can't change the tests.".to_string()), "{says:?}");
    assert!(!says.iter().any(|s| s.starts_with("You let this run change")), "{says:?}");
    assert!(send.get("tests_leave").is_none(), "{send}");
    // the Redo button, with the same words in its field and in the words box, as the page sends it
    let mut redo = base.clone();
    redo["verdicts"] = serde_json::json!({ q.items[0].id.clone(): "redo" });
    redo["notes"] = serde_json::json!({ q.items[0].id.clone(): CONTAS_WORDS });
    redo["context"] = CONTAS_WORDS.into();
    let fx = sentences(&post(port, "/api/effects", &redo));
    assert!(fx.iter().any(|s| s.starts_with("You let this run change how the project's tests are defined")), "what the Redo does says the leave: {fx:?}");
    assert_eq!(post(port, "/api/answer", &redo)["ok"], true);
    left_before_the_next_attempt(&judged_again(&s, "contas-button"));
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn asked_the_contas_words_at_the_merge_give_no_leave_and_say_nothing_will_run_again() {
    let s = contas("contas-merge");
    let (q, base) = open(&s, "contas-merge", "merge");
    let keep: serde_json::Map<String, Value> = q.items.iter().map(|i| (i.id.clone(), Value::from("keep"))).collect();
    let port = port();
    let _page = s.page(port, &serde_json::json!({"verdicts": keep, "tests": true}).to_string());
    let (says, send) = read_words(port, &base, CONTAS_WORDS);
    assert!(says.contains(&"Nothing will run again, so this can't change the tests.".to_string()), "{says:?}");
    assert!(!says.iter().any(|s| s.starts_with("You let this run change")), "{says:?}");
    assert!(send.get("tests_leave").is_none(), "{send}");
    assert_eq!(post(port, "/api/answer", &send)["ok"], true);
    let mut e = Vec::new();
    for _ in 0..300 {
        e = s.events("contas-merge");
        if e.iter().any(|e| e["type"] == "RunFinished") {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let types: Vec<&str> = e.iter().filter_map(|e| e["type"].as_str()).collect();
    assert!(types.contains(&"Merged"), "{types:?}");
    assert!(!types.contains(&"TestsLeft"), "no leave where nothing runs again: {types:?}");
}
