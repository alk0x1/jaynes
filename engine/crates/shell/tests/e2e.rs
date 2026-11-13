//! The engine end to end, as a person runs it: the real binary, the real small-change role over the protocol, a
//! real git project, and an agent that is a shell command, so no tokens are spent.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_jaynes-engine");

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn python() -> String {
    let venv = root().join("../.venv/bin/python");
    if venv.is_file() { venv.display().to_string() } else { "python3".to_string() }
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
        .env("JAYNES_ENGINE_HOME", &s.home)
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", root().join("..").join("src"))
        .output()
        .expect("engine");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

fn start(s: &Scratch, run: &str, agent: &str) -> String {
    let role = root().join("roles/small-change");
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
    let agent = format!("sed -i '' 's/hello/hi/' greeting.txt && {OUTPUT} && echo '{{\"tokens\":1234,\"session\":\"s1\"}}'");
    let out = start(&s, "small", &agent);
    assert!(out.contains("finished: delivered"), "{out}");
    let e = events(&s, "small");
    let t = types(&e);
    assert!(!t.contains(&"QuestionOpened".to_string()), "no stop on a small change: {t:?}");
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
    // a program without a terminal is not the person
    let state: Vec<Value> = events(&s, "risky");
    let q = state.iter().find(|e| e["type"] == "QuestionOpened").expect("asked")["question"]["id"].as_str().expect("id").to_string();
    let (ok, out) = engine(&s, &["answer", "risky", &q, "--send"]);
    assert!(!ok && out.contains("an agent never answers"), "{out}");
    // the person, at a terminal, keeps it
    let script = Command::new("script")
        .args(["-q", "/dev/null", BIN, "answer", "risky", &q, "--send"])
        .env("JAYNES_ENGINE_HOME", &s.home)
        .env("JAYNES_PYTHON", python())
        .output()
        .expect("script");
    let said = String::from_utf8_lossy(&script.stdout);
    assert!(said.contains("is kept") && said.contains("finished: delivered"), "{said}");
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
    assert_eq!(out.trim(), on_disk.trim(), "regenerate it: jaynes-engine schema > engine/protocol/role-protocol-1.schema.json");
}

#[test]
fn the_small_change_role_conforms() {
    let s = scratch("conform");
    let (ok, out) = engine(&s, &["conformance", root().join("roles/small-change").to_str().expect("path")]);
    assert!(ok, "{out}");
}
