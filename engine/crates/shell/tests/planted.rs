//! Defects planted on purpose, as an evaluation plants them to see whether the person looks (stage 2): someone of
//! the partner's who does not review plants one into a step's change while its stop waits for an answer, and takes it
//! out after. A stop shows a change already delivered, the person on the loop: the defect is only in what the stop
//! shows, never in what was or will be delivered, and while it is in nothing more is delivered and the run does not
//! end. The record keeps who planted it, where, and that it came out; the person who reviews never sees that it was
//! planted: not on the page, in the summary or in the log, only in the evaluation's export. And a run started never
//! to merge never merges, whatever is answered in it.
//!
//! The agent is a shell command, so no model is reached.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

/// The agent: says hi instead of hello, and is done.
const AGENT: &str = r#"command:perl -pi -e 's/hello/hi/' greeting.txt && printf '{"summary":"hello becomes hi"}' > .jaynes/output.json"#;

/// What the planter adds, in words nobody reviewing would read as planted.
const PLANTED_LINE: &str = "retries = 0";
const NOTE: &str = "off-by-one: retries set to zero";

struct Scratch {
    base: PathBuf,
    home: PathBuf,
    project: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _gone = std::fs::remove_dir_all(&self.base);
    }
}

fn scratch(name: &str) -> Scratch {
    let base = std::env::temp_dir().join(format!("jaynes-e2e-planted-{name}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let project = base.join("project");
    std::fs::create_dir_all(&project).expect("dir");
    std::fs::write(project.join("greeting.txt"), "hello\n").expect("write");
    std::fs::write(project.join("config.txt"), "retries = 3\n").expect("write");
    for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]] {
        assert!(Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success(), "git {args:?}");
    }
    Scratch { home: base.join("home"), project, base }
}

fn env(c: &mut Command, s: &Scratch) {
    c.env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "")
        .env("JAYNES_CHROME", std::env::var("JAYNES_CHROME").unwrap_or_else(|_| "/nonexistent/jaynes-test-browser".into()));
}

/// The engine as an agent or a script runs it: not at a terminal.
fn engine(s: &Scratch, args: &[&str]) -> (bool, String) {
    let mut c = Command::new(BIN);
    c.args(args);
    env(&mut c, s);
    let out = c.output().expect("engine");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// The same, at a terminal: a person.
fn person(s: &Scratch, args: &[&str]) -> String {
    person_with(s, args, &[])
}

/// A person, with more of the environment.
fn person_with(s: &Scratch, args: &[&str], more: &[(&str, &str)]) -> String {
    const TTY: &str = "import os, pty, sys\npid, fd = pty.fork()\nif pid == 0:\n    os.execvp(sys.argv[1], sys.argv[1:])\nwhile True:\n    try:\n        data = os.read(fd, 65536)\n    except OSError:\n        break\n    if not data:\n        break\n    os.write(1, data)\nsys.exit(os.waitstatus_to_exitcode(os.waitpid(pid, 0)[1]))\n";
    let mut c = Command::new("python3");
    c.args(["-c", TTY, BIN]).args(args);
    env(&mut c, s);
    c.envs(more.iter().copied());
    let out = c.output().expect("engine at a terminal");
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

/// A role of one agent step, with a stop that holds the run for the person who reviews (an evaluation plants while
/// it holds, before anything is delivered), delivered by `delivery`.
fn role(s: &Scratch, delivery: &str) -> PathBuf {
    let dir = s.base.join("role");
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("role.toml"), format!(
        "role = \"reviewed\"\nversion = \"1.0.0\"\ndelivery = \"{delivery}\"\n\n[[step]]\nname = \"change\"\nreach = {{ project = \"write\", write = [\"**\"], commands = true }}\ninstructions = \"Make the change: {{{{request}}}}\"\nstop = {{ ask = \"Is the change right?\", verdicts = [\"keep\", \"redo\"], holds = true }}\n\n[step.output]\nsummary = \"string\"\n"
    )).expect("write");
    dir
}

fn start(s: &Scratch, run: &str, delivery: &str, extra: &[&str]) -> String {
    let role = role(s, delivery);
    let mut args = vec!["start", "--run", run, "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"), "--request", "say hi", "--agent", AGENT];
    args.extend(extra);
    let (ok, out) = engine(s, &args);
    assert!(ok, "start failed:\n{out}");
    out
}

fn events(s: &Scratch, run: &str) -> Vec<Value> {
    let (ok, out) = engine(s, &["log", run]);
    assert!(ok, "{out}");
    out.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// The stop's question that waits, if any.
fn stop(s: &Scratch, run: &str) -> Option<String> {
    let e = events(s, run);
    let done: Vec<&str> = e.iter().filter(|e| e["type"] == "AnswerRecorded").filter_map(|e| e["answer"]["question"].as_str())
        .chain(e.iter().filter(|e| e["type"] == "QuestionSuperseded").filter_map(|e| e["question"].as_str())).collect();
    e.iter().rev().filter(|e| e["type"] == "QuestionOpened" && e["question"]["kind"] == "decision")
        .filter_map(|e| e["question"]["id"].as_str()).find(|id| !done.contains(id)).map(str::to_string)
}

/// A patch that sets the retries to zero, as the planter writes it.
fn patch(s: &Scratch) -> PathBuf {
    let p = s.base.join("planted.patch");
    std::fs::write(&p, format!("--- a/config.txt\n+++ b/config.txt\n@@ -1 +1 @@\n-retries = 3\n+{PLANTED_LINE}\n")).expect("write");
    p
}

fn plant(s: &Scratch, run: &str) -> String {
    let p = patch(s);
    person(s, &["plant", run, "--step", "change", "--patch", p.to_str().expect("path"), "--note", NOTE, "--by", "Planter Name"])
}

/// The id of the defect planted in `run`, from the evaluation's export.
fn planted_id(s: &Scratch, run: &str) -> String {
    let (ok, out) = engine(s, &["export", "--evaluation", run]);
    assert!(ok, "{out}");
    let v: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("the evaluation's export is not JSON ({e}): {out}"));
    v["defects"][0]["id"].as_str().unwrap_or_else(|| panic!("no defect in the export: {v}")).to_string()
}

/// The tree the checks of `step` judged, as the engine kept its change: a step's commit is made from the tree recorded
/// with its output.
fn judged(s: &Scratch, run: &str, step: &str) -> String {
    let ledger: Value = serde_json::from_str(&std::fs::read_to_string(s.home.join(format!("runs/{run}/project-steps.json"))).expect("ledger")).expect("json");
    let commit = ledger[step].as_str().expect("the step's change").to_string();
    let git = s.home.join(format!("runs/{run}/project.git"));
    let out = Command::new("git").arg("--git-dir").arg(&git).args(["rev-parse", &format!("{commit}^{{tree}}")]).output().expect("git");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn tree(dir: &Path, rev: &str) -> String {
    let out = Command::new("git").arg("-C").arg(dir).args(["rev-parse", &format!("{rev}^{{tree}}")]).output().expect("git");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The steps whose stop a person kept with a planted defect in it, once it is out, as the record says them.
fn missed(s: &Scratch, run: &str) -> Vec<String> {
    let store = jaynes_shell::store::Store::open(&s.home.join("record.sqlite3")).expect("the record");
    jaynes_shell::planted::missed(&store.events(run).expect("events"))
}

fn on(dir: &Path, rev: &str, file: &str) -> String {
    let out = Command::new("git").arg("-C").arg(dir).args(["show", &format!("{rev}:{file}")]).output().expect("git");
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet")]
fn a_planted_defect_is_in_what_the_stop_shows_and_nowhere_it_could_be_read_as_planted() {
    let s = scratch("shown");
    start(&s, "shown", "merge", &[]);
    assert!(stop(&s, "shown").is_some(), "the run waits at its stop");
    let out = plant(&s, "shown");
    assert!(out.contains("planted"), "{out}");
    // the reviewer's stop shows the change with the defect in it
    assert!(std::fs::read_to_string(s.home.join("runs/shown/project/config.txt")).unwrap_or_default().contains(PLANTED_LINE));
    assert!(stop(&s, "shown").is_some(), "the stop still waits for the reviewer");
    // and nothing the reviewer reads says it was planted
    let (_, shown) = engine(&s, &["show", "shown"]);
    let (_, log) = engine(&s, &["log", "shown"]);
    for (what, text) in [("show", &shown), ("log", &log)] {
        for word in [NOTE, "planted.patch", "Planter Name", "Planted", "planted", "Defect", "defect"] {
            assert!(!text.contains(word), "{what} says {word:?}:\n{text}");
        }
    }
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet")]
fn nothing_is_delivered_while_a_planted_defect_is_in_and_unplanting_lets_it_go_with_a_trail() {
    let s = scratch("held");
    start(&s, "held", "merge", &[]);
    plant(&s, "held");
    let q = stop(&s, "held").expect("the stop waits");
    let out = person(&s, &["answer", "held", &q, "change=keep", "--send"]);
    // kept at the stop, and the run does not end while the defect is in; what was delivered never had it
    assert!(!out.contains("finished: delivered"), "{out}");
    let (_, shown) = engine(&s, &["show", "held"]);
    assert!(shown.contains("a planted defect is still in"), "{shown}");
    assert!(missed(&s, "held").is_empty(), "the summary would say a defect was missed while it is still in");
    assert_eq!(on(&s.project, "main", "config.txt"), "retries = 3\n");
    assert_eq!(on(&s.project, "jaynes/held", "config.txt"), "retries = 3\n", "the run's branch went out with the defect");
    // taken out, the delivery goes on, without the defect and with the agent's change
    let id = planted_id(&s, "held");
    let out = person(&s, &["unplant", "held", &id, "--by", "Remover Name"]);
    assert!(out.contains("finished: delivered"), "{out}");
    assert_eq!(on(&s.project, "main", "config.txt"), "retries = 3\n");
    assert_eq!(on(&s.project, "main", "greeting.txt"), "hi\n");
    // the step's change is again exactly the one its checks judged, and that is what was delivered
    assert_eq!(tree(&s.project, "jaynes/held"), judged(&s, "held", "change"));
    // and once it is out, the person may be told they kept the stop that showed it
    assert_eq!(missed(&s, "held"), ["change"]);
    // the trail: who planted it, where, and that it came out before anything was delivered
    let (ok, export) = engine(&s, &["export", "--evaluation", "held"]);
    assert!(ok, "{export}");
    let v: Value = serde_json::from_str(&export).expect("json");
    let d = &v["defects"][0];
    assert_eq!((d["step"].as_str(), d["planter"].as_str(), d["note"].as_str()), (Some("change"), Some("Planter Name"), Some(NOTE)), "{d}");
    assert_eq!(d["files"], serde_json::json!(["config.txt"]), "{d}");
    assert!(d["patch"].as_str().unwrap_or("").contains(PLANTED_LINE), "{d}");
    assert_eq!((d["removed"]["by"].as_str(), d["removed"]["verified"].as_bool()), (Some("Remover Name"), Some(true)), "{d}");
    assert_eq!(d["never_delivered"].as_bool(), Some(true), "{d}");
    // the stop's answer where it was shown is there for the evaluator to judge: kept, so missed
    assert_eq!(d["answered"]["verdicts"]["change"], "keep", "{d}");
    assert_eq!((d["answered"]["result"].as_str(), d["still_in"].as_bool()), (Some("missed"), Some(false)), "{d}");
    // the summary the reviewer reads does not say it either
    let (_, summary) = engine(&s, &["summary", "held"]);
    for word in [NOTE, "Planter Name", "Remover Name", "planted", "defect"] {
        assert!(!summary.to_lowercase().contains(&word.to_lowercase()), "the summary says {word:?}:\n{summary}");
    }
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet")]
fn a_branch_delivery_waits_too() {
    let s = scratch("branch");
    start(&s, "branch", "branch", &[]);
    plant(&s, "branch");
    let q = stop(&s, "branch").expect("the stop waits");
    let out = person(&s, &["answer", "branch", &q, "change=keep", "--send"]);
    assert!(!out.contains("finished: delivered"), "{out}");
    assert_eq!(on(&s.project, "jaynes/branch", "config.txt"), "retries = 3\n", "the branch went out with the defect");
}

/// An agent that keeps what it saw of config.txt and a line for each round it works.
const SEEING: &str = r#"command:cp config.txt seen.txt && echo round >> rounds.txt && perl -pi -e 's/hello/hi/' greeting.txt && printf '{"summary":"hello becomes hi"}' > .jaynes/output.json"#;

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet")]
fn the_branch_never_has_the_defect_after_a_redo_and_the_run_does_not_end_while_it_is_in() {
    let s = scratch("redo");
    let role = role(&s, "branch");
    let (ok, out) = engine(&s, &["start", "--run", "redo", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"), "--request", "say hi", "--agent", SEEING]);
    assert!(ok, "{out}");
    plant(&s, "redo");
    let q = stop(&s, "redo").expect("the stop waits");
    // the person who reviews sends the change back: the step works again, from its change as it was
    person(&s, &["answer", "redo", &q, "change=redo:look again", "--send"]);
    let again = stop(&s, "redo").expect("the stop of the second round waits");
    assert_ne!(again, q);
    assert_eq!(on(&s.project, "jaynes/redo", "rounds.txt"), "round\n", "a change was delivered while a defect was in");
    // kept, and still the run does not end while the defect is in
    let out = person(&s, &["answer", "redo", &again, "change=keep", "--send"]);
    assert!(!out.contains("finished"), "{out}");
    let (_, shown) = engine(&s, &["show", "redo"]);
    assert!(shown.contains("a planted defect is still in"), "{shown}");
    // taken out, the second round goes out, without the defect, and the round saw none
    let out = person(&s, &["unplant", "redo", &planted_id(&s, "redo"), "--by", "Remover Name"]);
    assert!(out.contains("finished: delivered"), "{out}");
    assert_eq!(on(&s.project, "jaynes/redo", "rounds.txt"), "round\nround\n");
    assert_eq!(on(&s.project, "jaynes/redo", "config.txt"), "retries = 3\n", "the branch went out with the defect");
    assert_eq!(on(&s.project, "jaynes/redo", "seen.txt"), "retries = 3\n", "the second round worked from the defect");
    let (_, export) = engine(&s, &["export", "--evaluation", "redo"]);
    let v: Value = serde_json::from_str(&export).expect("json");
    assert_eq!((v["defects"][0]["answered"]["result"].as_str(), v["defects"][0]["never_delivered"].as_bool()), (Some("caught"), Some(true)), "{v}");
    assert!(missed(&s, "redo").is_empty(), "a stop that sent the defect back was told as missed");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet")]
fn only_a_person_plants_or_unplants() {
    let s = scratch("person");
    start(&s, "person", "merge", &[]);
    let p = patch(&s);
    let (ok, out) = engine(&s, &["plant", "person", "--step", "change", "--patch", p.to_str().expect("path"), "--note", NOTE, "--by", "x"]);
    assert!(!ok && out.contains("only you"), "an agent planted: {out}");
    plant(&s, "person");
    let id = planted_id(&s, "person");
    let (ok, out) = engine(&s, &["unplant", "person", &id, "--by", "x"]);
    assert!(!ok && out.contains("only you"), "an agent unplanted: {out}");
    // and unplanting twice, or what was never planted, is refused
    person(&s, &["unplant", "person", &id, "--by", "y"]);
    let again = person(&s, &["unplant", "person", &id, "--by", "y"]);
    assert!(again.contains("already"), "{again}");
    let none = person(&s, &["unplant", "person", "d-none", "--by", "y"]);
    assert!(none.contains("no planted defect"), "{none}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet")]
fn a_run_started_never_to_merge_never_merges_whatever_is_answered() {
    let s = scratch("never");
    start(&s, "never", "merge", &["--never-merge"]);
    let e = events(&s, "never");
    assert_eq!(e[0]["never_merge"], true, "the mark is recorded as the run starts: {}", e[0]);
    let q = stop(&s, "never").expect("the stop waits");
    let out = person(&s, &["answer", "never", &q, "change=keep", "--send"]);
    assert!(out.contains("finished: delivered"), "{out}");
    // delivered on the run's branch only
    assert_eq!(on(&s.project, "main", "greeting.txt"), "hello\n", "a run never to merge merged");
    assert_eq!(on(&s.project, "jaynes/never", "greeting.txt"), "hi\n");
    let (_, summary) = engine(&s, &["summary", "never"]);
    assert!(summary.contains("never to merge"), "the summary does not say it: {summary}");
    // and nothing lifts the mark: raising the risk, or a correction, merges nothing
    person(&s, &["raise", "never", "high"]);
    assert_eq!(on(&s.project, "main", "greeting.txt"), "hello\n");
}

/// A role of two agent steps, each with a stop that does not hold: the second works on the first's change.
fn two_steps(s: &Scratch) -> PathBuf {
    let dir = s.base.join("two");
    std::fs::create_dir_all(&dir).expect("dir");
    let step = |name: &str, needs: &str| format!(
        "[[step]]\nname = \"{name}\"\n{needs}reach = {{ project = \"write\", write = [\"**\"], commands = true }}\ninstructions = \"Make the change: {{{{request}}}}\"\nstop = {{ ask = \"Is it right?\", verdicts = [\"keep\", \"redo\"] }}\n\n[step.output]\nsummary = \"string\"\n\n"
    );
    std::fs::write(dir.join("role.toml"), format!("role = \"two\"\nversion = \"1.0.0\"\ndelivery = \"branch\"\n\n{}{}", step("change", ""), step("then", "needs = [\"change\"]\n"))).expect("write");
    dir
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet")]
fn a_later_step_done_again_while_a_defect_is_in_never_brings_it_in() {
    let s = scratch("later");
    let role = two_steps(&s);
    let (ok, out) = engine(&s, &["start", "--run", "later", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"), "--request", "say hi", "--agent", SEEING]);
    assert!(ok, "{out}");
    plant(&s, "later");
    // the later step is sent back while the defect is in the first: it works again on the first's change as it was
    let e = events(&s, "later");
    let then = e.iter().rev().find(|e| e["type"] == "QuestionOpened" && e["question"]["step"] == "then").and_then(|e| e["question"]["id"].as_str()).expect("the later step's stop").to_string();
    person(&s, &["answer", "later", &then, "then=redo:once more", "--send"]);
    // every stop still waiting is kept, the later step's second round among them
    for _ in 0..4 {
        let Some(q) = stop(&s, "later") else { break };
        let item = events(&s, "later").iter().find(|e| e["type"] == "QuestionOpened" && e["question"]["id"] == q.as_str()).and_then(|e| e["question"]["step"].as_str().map(str::to_string)).expect("step");
        person(&s, &["answer", "later", &q, &format!("{item}=keep"), "--send"]);
    }
    assert!(stop(&s, "later").is_none(), "a stop still waits");
    let out = person(&s, &["unplant", "later", &planted_id(&s, "later"), "--by", "Remover Name"]);
    assert!(out.contains("finished: delivered"), "{out}");
    assert_eq!(on(&s.project, "jaynes/later", "seen.txt"), "retries = 3\n", "the later step worked from the defect");
    assert_eq!(on(&s.project, "jaynes/later", "config.txt"), "retries = 3\n", "the branch went out with the defect");
    assert_eq!(on(&s.project, "jaynes/later", "rounds.txt"), "round\nround\nround\n");
    // what went out is the tree the later step's checks judged
    assert_eq!(tree(&s.project, "jaynes/later"), judged(&s, "later", "then"));
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet")]
fn a_defect_taken_out_by_a_process_that_died_is_closed_by_a_second_unplant() {
    let s = scratch("died");
    start(&s, "died", "merge", &[]);
    plant(&s, "died");
    let q = stop(&s, "died").expect("the stop waits");
    person(&s, &["answer", "died", &q, "change=keep", "--send"]);
    let id = planted_id(&s, "died");
    // the process dies once the record holds the defect taken out, before it says so beside the run
    person_with(&s, &["unplant", "died", &id, "--by", "Remover Name"], &[("JAYNES_DIE_AFTER_UNPLANT", "1")]);
    let (_, export) = engine(&s, &["export", "--evaluation", "died"]);
    let v: Value = serde_json::from_str(&export).expect("json");
    // the record says it is out before the file beside the run does
    assert!(v["defects"][0]["removed"].is_null() && v["defects"][0]["still_in"] == false, "{v}");
    // a second unplant says it and the run goes on to its end
    let out = person(&s, &["unplant", "died", &id, "--by", "Remover Name"]);
    assert!(out.contains("finished: delivered"), "{out}");
    let (_, export) = engine(&s, &["export", "--evaluation", "died"]);
    let v: Value = serde_json::from_str(&export).expect("json");
    let d = &v["defects"][0];
    assert_eq!((d["removed"]["by"].as_str(), d["removed"]["verified"].as_bool()), (Some("Remover Name"), Some(true)), "{d}");
    assert_eq!(tree(&s.project, "jaynes/died"), judged(&s, "died", "change"));
    // and a third is refused: it is out
    assert!(person(&s, &["unplant", "died", &id, "--by", "Remover Name"]).contains("already"));
}
