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

struct Scratch {
    home: PathBuf,
    project: PathBuf,
}

fn scratch(name: &str) -> Scratch {
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
    Scratch { home: base.join("home"), project }
}

fn engine(s: &Scratch, args: &[&str]) -> (bool, String) {
    let out = Command::new(BIN)
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
fn a_small_change_is_delivered_as_a_branch_with_no_stop() {
    let s = scratch("small");
    let prompt = s.home.parent().expect("base").join("prompt.txt");
    let agent = format!("cat > {} && sed -i '' 's/hello/hi/' greeting.txt && {OUTPUT} && echo '{{\"tokens\":1234,\"session\":\"s1\"}}'", prompt.display());
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
fn a_failing_suite_is_fixed_in_the_same_session_without_spending_an_attempt() {
    let s = scratch("fix");
    // first the agent writes "hey" (the suite wants "hi"); asked to fix, in its session, it writes "hi"
    let agent = format!(
        "if [ -n \"$JAYNES_SESSION\" ]; then sed -i '' 's/hey/hi/' greeting.txt; else sed -i '' 's/hello/hey/' greeting.txt; fi && {OUTPUT} && echo '{{\"session\":\"s1\"}}'"
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
fn a_change_to_a_file_that_runs_later_raises_the_risk_and_asks_the_person() {
    let s = scratch("risky");
    let agent = format!("sed -i '' 's/hello/hi/' greeting.txt && printf 'test:\\n\\ttrue\\n' > Makefile && {OUTPUT}");
    let out = start(&s, "risky", &agent);
    assert!(out.contains("risk raised to high"), "{out}");
    assert!(out.contains("waiting: delivered; your answers can still correct it"), "{out}");
    // a program without a terminal is an agent: it may not send the change back
    let state: Vec<Value> = events(&s, "risky");
    let q = state.iter().find(|e| e["type"] == "QuestionOpened").expect("asked")["question"]["id"].as_str().expect("id").to_string();
    let (ok, out) = engine(&s, &["answer", "risky", &q, "change=redo:smaller", "--send"]);
    assert!(!ok && out.contains("sending it back is yours"), "{out}");
    // the person, at a terminal, keeps it
    let script = Command::new("script")
        .args(["-q", "/dev/null", BIN, "answer", "risky", &q, "--send"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .output()
        .expect("script");
    let said = String::from_utf8_lossy(&script.stdout);
    assert!(said.contains("is kept") && said.contains("finished: delivered"), "{said}");
}

#[test]
fn an_agent_lets_a_change_that_is_not_small_go_on_and_it_counts_as_the_agent_s() {
    let s = scratch("release");
    let agent = format!("sed -i '' 's/hello/hi/' greeting.txt && printf 'test:\\n\\ttrue\\n' > Makefile && {OUTPUT}");
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
fn an_input_sent_while_another_process_drives_waits_in_the_inbox() {
    let s = scratch("inbox");
    let role = root().join("crates/shell/tests/roles/small-change");
    let agent = format!("sleep 4 && sed -i '' 's/hello/hi/' greeting.txt && {OUTPUT}");
    let child = Command::new(BIN)
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
    // the attempt under way finished; the halt was taken before the delivery
    assert!(said.contains("(halted)"), "{said}");
    let t = types(&events(&s, "inbox"));
    assert!(t.contains(&"Halted".to_string()), "{t:?}");
    assert!(!t.contains(&"Delivered".to_string()), "nothing is delivered while halted: {t:?}");
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
fn fix_bug_proves_the_fix_with_cases_the_maker_never_sees() {
    let s = bug_project("fixbug");
    let prompts = s.home.join("prompts");
    std::fs::create_dir_all(&prompts).expect("dir");
    let agent = root().join("crates/shell/tests/agents/fix-bug.sh");
    let role = root().join("crates/shell/tests/roles/fix-bug");
    let out = Command::new(BIN)
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
    s.write_all(raw.as_bytes()).expect("write");
    let mut out = String::new();
    let _read = s.read_to_string(&mut out);
    out
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
fn the_page_answers_as_the_person_only_with_its_key() {
    let s = scratch("page");
    let agent = format!("sed -i '' 's/hello/hi/' greeting.txt && printf 'test:\\n\\ttrue\\n' > Makefile && {OUTPUT}");
    start(&s, "page", &agent);
    let e = events(&s, "page");
    let q = e.iter().find(|e| e["type"] == "QuestionOpened").expect("asked")["question"].clone();
    let shown = jaynes_core::question::Question::shown(&serde_json::from_value(q.clone()).expect("question")).0;
    let port = 18750 + (std::process::id() % 1000) as u16;
    let _page = Command::new(BIN)
        .args(["page", "--port", &port.to_string()])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .env("JAYNES_PAGE_KEY", "k3y")
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(Page)
        .expect("page");
    std::thread::sleep(std::time::Duration::from_millis(500));
    // another address is refused, the key sets a cookie, and the question is on the page
    assert!(http(port, &format!("GET / HTTP/1.1\r\nHost: evil.example:{port}\r\n\r\n")).starts_with("HTTP/1.1 403"));
    assert!(http(port, &format!("GET /?key=k3y HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")).contains("Set-Cookie: jaynes_key=k3y"));
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
fn sending_an_earlier_step_back_reruns_it_without_the_later_steps_changes() {
    let s = bug_project("redo");
    let prompts = s.home.join("prompts");
    std::fs::create_dir_all(&prompts).expect("dir");
    let agent = root().join("crates/shell/tests/agents/fix-bug.sh");
    let role = root().join("crates/shell/tests/roles/fix-bug");
    let env = |c: &mut Command| {
        c.env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "").env("PROMPTS", &prompts);
    };
    let mut c = Command::new(BIN);
    c.args(["start", "--run", "redo", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
            "--request", "greet should say hi with the name", "--agent", &format!("command:sh {}", agent.display())]);
    env(&mut c);
    assert!(c.output().expect("start").status.success());
    let q = events(&s, "redo").into_iter().find(|e| e["type"] == "QuestionOpened" && e["question"]["step"] == "reproduce").expect("asked")["question"]["id"].as_str().expect("id").to_string();
    // the person, at a terminal, sends reproduce back after the fix was made
    let mut c = Command::new("script");
    c.args(["-q", "/dev/null", BIN, "answer", "redo", &q, "examples-1=redo:also an empty name", "--send"]);
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
fn a_crash_in_the_middle_of_a_turn_stops_the_agent_left_running_and_spends_no_attempt() {
    let s = scratch("crash");
    let mark = s.home.join("mark");
    std::fs::create_dir_all(&mark).expect("dir");
    let agent = format!(
        "if [ ! -e {m}/started ]; then echo '{{\"type\":\"system\",\"session_id\":\"s9\",\"model\":\"m9\",\"permissionMode\":\"default\",\"tools\":[\"Read\",\"Bash\"]}}'; touch {m}/started; sleep 20; touch {m}/orphan-finished; exit 0; fi; echo \"$JAYNES_SESSION\" > {m}/resumed; sed -i '' 's/hello/hi/' greeting.txt && {OUTPUT}",
        m = mark.display()
    );
    let role = root().join("crates/shell/tests/roles/small-change");
    let mut engine_process = Command::new(BIN)
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
    let alive = |g: &str| Command::new("kill").args(["-0", &format!("-{}", g.trim())]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
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
    let agent = format!("sed -i '' 's/hello/hi/' greeting.txt && {OUTPUT}");
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
fn the_page_halts_resumes_and_records_a_correction() {
    let s = scratch("acts");
    let agent = format!("sed -i '' 's/hello/hi/' greeting.txt && printf 'test:\\n\\ttrue\\n' > Makefile && {OUTPUT}");
    start(&s, "acts", &agent);
    let port = 19750 + (std::process::id() % 1000) as u16;
    let _page = Command::new(BIN)
        .args(["page", "--port", &port.to_string()])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .env("JAYNES_PAGE_KEY", "k3y")
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(Page)
        .expect("page");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let me = "jaynes_key=k3y";
    let view = http(port, &format!("GET /run/acts HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: {me}\r\n\r\n"));
    assert!(view.contains("class=\"stages told\"") && view.contains("waiting for you") && view.contains("data-act=\"halt\""), "{view}");
    // what each check said, and where its answer comes from
    assert!(view.contains("suite-passes (open, read runner) passed"), "{view}");
    // what the step changed, line by line, as git shows it
    assert!(view.contains("What change changed · 2 files") && view.contains("<span class=\"del\"><i>-</i>hello") && view.contains("<span class=\"file\">greeting.txt</span>"), "{view}");
    // an agent may halt, only the person resumes
    assert_eq!(post(port, "/api/act", r#"{"run":"acts","action":"halt"}"#, "")["ok"], true);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(post(port, "/api/act", r#"{"run":"acts","action":"resume"}"#, "")["ok"], false);
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
fn a_leave_the_agent_asks_mid_turn_is_granted_by_a_rule_or_by_the_person() {
    let s = scratch("leave");
    let mark = s.home.join("mark");
    std::fs::create_dir_all(&mark).expect("dir");
    // the agent works for a while: the leave server is asked while its turn runs
    let agent = format!("touch {}/started; sleep 8; sed -i '' 's/hello/hi/' greeting.txt && {OUTPUT}", mark.display());
    let role = root().join("crates/shell/tests/roles/small-change");
    let run = Command::new(BIN)
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
    let mut child = Command::new(BIN)
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
    let answered = Command::new("script")
        .args(["-q", "/dev/null", BIN, "answer", "leave", &q, "leave=drop", "--context", "not outside the project", "--send"])
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
    let out = Command::new(BIN)
        .args(["start", "--run", run, "--role", root().join("crates/shell/tests/roles/fix-bug").to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "greet should say hi with the name", "--agent", &format!("command:sh {}", agent.display())])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "").env("PROMPTS", &prompts)
        .output().expect("start");
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

#[test]
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
    let undo = Command::new("script")
        .args(["-q", "/dev/null", BIN, "undo", "merge", "--note", "not now"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .output().expect("undo");
    assert!(String::from_utf8_lossy(&undo.stdout).contains("undone"), "{}", String::from_utf8_lossy(&undo.stdout));
    assert_eq!(std::fs::read_to_string(s.project.join("greet.sh")).expect("read"), "echo \"hello $1\"\n");
    assert!(types(&events(&s, "merge")).contains(&"MergeUndone".to_string()));
}

#[test]
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
    let port = 20750 + (std::process::id() % 1000) as u16;
    let _page = Command::new(BIN)
        .args(["page", "--port", &port.to_string()])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PAGE_KEY", "k3y")
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(Page)
        .expect("page");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let html = http(port, &format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: jaynes_key=k3y\r\n\r\n"));
    assert!(html.contains("ink on newsprint"), "the item's sentence is shown");
    assert!(html.contains("<img src=\"data:image/svg+xml;base64,"), "the item's picture is shown");
    // the picture was kept by its fingerprint, apart from the run's copy
    let kept: Vec<_> = std::fs::read_dir(s.home.join("pictures")).expect("kept").flatten().collect();
    assert_eq!(kept.len(), 1);
}

#[test]
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
    let agent = format!("sed -i '' 's/hello/hi/' greeting.txt && {OUTPUT}");
    let (ok, out) = engine(&s, &["start", "--run", "byname", "--role", "small-change", "--project", s.project.to_str().expect("path"),
                                 "--request", "say hi", "--agent", &format!("command:{agent}")]);
    assert!(ok && out.contains("finished: delivered"), "{out}");
    let (ok, out) = engine(&s, &["guide"]);
    assert!(ok && out.contains("jaynes start --role"), "{out}");
}

#[test]
fn agents_in_claude_code_use_the_mcp_server_and_may_only_release() {
    let s = scratch("mcp");
    assert!(engine(&s, &["add", root().join("crates/shell/tests/roles/small-change").to_str().expect("path")]).0);
    let agent = format!("sed -i '' 's/hello/hi/' greeting.txt && printf 'test:\\n\\ttrue\\n' > Makefile && {OUTPUT}");
    // the agent program of the run is a command; the MCP start uses Claude by default, so start here and read through MCP
    start(&s, "viamcp", &agent);
    let child = Command::new(BIN)
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
fn abandoning_a_run_stops_its_agent_at_once() {
    let s = scratch("abandon");
    let mark = s.home.join("mark");
    std::fs::create_dir_all(&mark).expect("dir");
    let agent = format!("touch {m}/started; sleep 30; touch {m}/finished", m = mark.display());
    let role = root().join("crates/shell/tests/roles/small-change");
    let mut run = Command::new(BIN)
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
    let abandoned = Command::new("script")
        .args(["-q", "/dev/null", BIN, "abandon", "abandon"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .output().expect("abandon");
    let said = String::from_utf8_lossy(&abandoned.stdout).to_string();
    let alive = Command::new("kill").args(["-0", &format!("-{}", pgid.trim())]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
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
fn the_person_starts_a_run_from_the_page() {
    let s = scratch("new");
    let agent = format!("sed -i '' 's/hello/hi/' greeting.txt && {OUTPUT}");
    let port = 17750 + (std::process::id() % 1000) as u16;
    let _page = Command::new(BIN)
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
    std::thread::sleep(std::time::Duration::from_millis(500));
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
    let port = 16750 + (std::process::id() % 1000) as u16;
    let _page = Command::new(BIN)
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
    std::thread::sleep(std::time::Duration::from_millis(500));
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
fn an_agent_that_says_it_cannot_be_done_waits_for_the_person_and_goes_on_from_their_words() {
    let s = scratch("impossible");
    let prompts = s.home.parent().expect("base").join("prompts");
    std::fs::create_dir_all(&prompts).expect("dir");
    // first turn: it cannot be done as asked; second turn, in the same session: it does what the person said
    let agent = format!(
        "cat > {p}/turn-${{JAYNES_SESSION:-1}}; if [ -n \"$JAYNES_SESSION\" ]; then sed -i '' 's/hello/hi/' greeting.txt && {OUTPUT}; else echo 'the request asks for hi and for hello at once' > .jaynes/impossible.txt; echo '{{\"session\":\"s1\"}}'; fi",
        p = prompts.display()
    );
    let out = start(&s, "impossible", &agent);
    assert!(out.contains("cannot be done as asked"), "{out}");
    let first = std::fs::read_to_string(prompts.join("turn-1")).expect("prompt");
    assert!(first.contains(".jaynes/impossible.txt"), "the agent is told the way out: {first}");
    let e = events(&s, "impossible");
    assert!(e.iter().any(|e| e["type"] == "AttemptImpossible"), "{:?}", types(&e));
    let q = e.iter().rfind(|e| e["type"] == "InputNeeded").expect("asked")["question"]["id"].as_str().expect("id").to_string();
    let answered = Command::new("script")
        .args(["-q", "/dev/null", BIN, "answer", "impossible", &q, "impossible=change:only hi", "--send"])
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
        let mut first = Command::new(BIN);
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
            let mut again = Command::new(BIN);
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
            let mut c = Command::new("script");
            c.args(["-q", "/dev/null", BIN, "continue", &run]);
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
fn a_finished_run_s_copy_is_cleared_after_a_while_and_its_changes_still_show() {
    let s = scratch("sweep");
    let agent = format!("sed -i '' 's/hello/hi/' greeting.txt && {OUTPUT}");
    start(&s, "old", &agent);
    assert!(s.home.join("runs/old/project/.git").exists());
    // the next start clears the copies of runs finished longer ago than the time kept (none kept here)
    let role = root().join("crates/shell/tests/roles/small-change");
    let out = Command::new(BIN)
        .args(["start", "--run", "new", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .env("JAYNES_KEEP_COPIES_SECONDS", "0")
        .output().expect("start");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!s.home.join("runs/old/project").exists(), "the finished run's copy is cleared");
    assert!(s.home.join("runs/new/project/.git").exists(), "the run just started keeps its copy");
    // the page still shows what the old run changed
    let port = 15750 + (std::process::id() % 1000) as u16;
    let _page = Command::new(BIN)
        .args(["page", "--port", &port.to_string()])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PAGE_KEY", "k3y").env("JAYNES_NO_UPDATE_CHECK", "1")
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(Page)
        .expect("page");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let view = http(port, &format!("GET /run/old HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: jaynes_key=k3y\r\n\r\n"));
    assert!(view.contains("What change changed") && view.contains("<span class=\"add\"><i>+</i>hi</span>"), "{view}");
}

#[test]
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
        let mut child = Command::new(BIN)
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
fn a_step_done_in_the_record_keeps_its_change_when_the_process_dies_before_committing_it() {
    let s = scratch("diedone");
    let agent = format!("sed -i '' 's/hello/hi/' greeting.txt && {OUTPUT}");
    let role = root().join("crates/shell/tests/roles/small-change");
    let out = Command::new(BIN)
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
fn a_merge_that_landed_before_the_process_died_lands_once_and_can_be_undone() {
    let s = bug_project("diemerge");
    let prompts = s.home.join("prompts");
    std::fs::create_dir_all(&prompts).expect("dir");
    let agent = root().join("crates/shell/tests/agents/fix-bug.sh");
    let out = Command::new(BIN)
        .args(["start", "--run", "diemerge", "--role", root().join("crates/shell/tests/roles/fix-bug").to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "greet should say hi with the name", "--agent", &format!("command:sh {}", agent.display())])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "").env("PROMPTS", &prompts)
        .env("JAYNES_DIE_AFTER_MERGE", "1")
        .output().expect("start");
    assert_eq!(out.status.code(), Some(9), "{}", String::from_utf8_lossy(&out.stderr));
    // the delivery made again a second later is made of new commits, with the same change
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let mut c = Command::new(BIN);
    c.args(["continue", "diemerge"]).env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "").env("PROMPTS", &prompts);
    let said = String::from_utf8_lossy(&c.output().expect("continue").stdout).to_string();
    assert!(said.contains("delivered"), "{said}");
    let log = Command::new("git").arg("-C").arg(&s.project).args(["log", "--format=%s", "main"]).output().expect("git");
    let merges = String::from_utf8_lossy(&log.stdout).lines().filter(|l| l.contains("merged")).count();
    assert_eq!(merges, 1, "the fix landed {merges} times: {said}");
    // the merge made before the process died is the run's, so Undo reverts it
    let undo = Command::new("script")
        .args(["-q", "/dev/null", BIN, "undo", "diemerge", "--note", "not now"])
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
    let out = Command::new(BIN)
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
