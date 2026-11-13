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
    // a program without a terminal is an agent: it may not send the change back
    let state: Vec<Value> = events(&s, "risky");
    let q = state.iter().find(|e| e["type"] == "QuestionOpened").expect("asked")["question"]["id"].as_str().expect("id").to_string();
    let (ok, out) = engine(&s, &["answer", "risky", &q, "change=redo:smaller", "--send"]);
    assert!(!ok && out.contains("sending it back is yours"), "{out}");
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
    assert_eq!(out.trim(), on_disk.trim(), "regenerate it: jaynes-engine schema > engine/protocol/role-protocol-1.schema.json");
}

#[test]
fn the_small_change_role_conforms() {
    let s = scratch("conform");
    let (ok, out) = engine(&s, &["conformance", root().join("roles/small-change").to_str().expect("path")]);
    assert!(ok, "{out}");
}

#[test]
fn an_input_sent_while_another_process_drives_waits_in_the_inbox() {
    let s = scratch("inbox");
    let role = root().join("roles/small-change");
    let agent = format!("sleep 4 && sed -i '' 's/hello/hi/' greeting.txt && {OUTPUT}");
    let child = Command::new(BIN)
        .args(["start", "--run", "inbox", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home)
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", root().join("..").join("src"))
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
    let role = root().join("roles/fix-bug");
    let out = Command::new(BIN)
        .args(["start", "--run", "fixbug", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "greet should say hi with the name, not hello", "--agent", &format!("command:sh {}", agent.display())])
        .env("JAYNES_ENGINE_HOME", &s.home)
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", root().join("..").join("src"))
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
